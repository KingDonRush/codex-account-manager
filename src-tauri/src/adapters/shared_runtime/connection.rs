use crate::error::AppError;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[cfg(unix)]
pub(super) struct Connection {
    socket: tungstenite::WebSocket<std::os::unix::net::UnixStream>,
    next_id: u64,
    deadline: Instant,
}

#[cfg(unix)]
impl Connection {
    pub(super) fn connect(path: &Path, pid: u32) -> Result<Self, AppError> {
        use std::os::unix::net::UnixStream;
        let raw = socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None)?;
        raw.connect_timeout(&socket2::SockAddr::unix(path)?, Duration::from_secs(2))?;
        let stream: UnixStream = raw.into();
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
        verify_peer(&stream, pid)?;
        let config = tungstenite::protocol::WebSocketConfig::default()
            .max_message_size(Some(2 * 1024 * 1024))
            .max_frame_size(Some(2 * 1024 * 1024));
        let (socket, _) = tungstenite::client::client_with_config(
            "ws://codex-app-server/rpc",
            stream,
            Some(config),
        )
        .map_err(|_| {
            AppError::Runtime(
                "Não foi possível negociar o protocolo do servidor compartilhado.".to_owned(),
            )
        })?;
        socket
            .get_ref()
            .set_read_timeout(Some(Duration::from_millis(100)))?;
        Ok(Self {
            socket,
            next_id: 1,
            deadline: Instant::now() + Duration::from_secs(8),
        })
    }

    pub(super) fn initialize(&mut self, cancel: &AtomicBool) -> Result<(), AppError> {
        self.request("initialize", json!({"clientInfo":{"name":"codex-account-manager-runtime","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}), cancel)?;
        self.send(json!({"method":"initialized"}))
    }

    pub(super) fn request(
        &mut self,
        method: &str,
        params: Value,
        cancel: &AtomicBool,
    ) -> Result<Value, AppError> {
        if cancel.load(Ordering::Acquire) {
            return Err(AppError::Cancelled);
        }
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"id":id,"method":method,"params":params}))?;
        loop {
            if cancel.load(Ordering::Acquire) {
                return Err(AppError::Cancelled);
            }
            if Instant::now() >= self.deadline {
                return Err(AppError::Runtime(
                    "O servidor compartilhado não respondeu no prazo.".to_owned(),
                ));
            }
            let frame = match self.socket.read() {
                Ok(frame) => frame,
                Err(tungstenite::Error::Io(error))
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    continue
                }
                Err(_) => {
                    return Err(AppError::Runtime(
                        "A conexão do servidor compartilhado encerrou.".to_owned(),
                    ))
                }
            };
            let tungstenite::Message::Text(text) = frame else {
                continue;
            };
            let message: Value = serde_json::from_str(&text)?;
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                return Err(AppError::Rpc {
                    code: error.get("code").and_then(Value::as_i64).unwrap_or(-32000),
                    message: "O servidor não disponibilizou a consulta de atividade solicitada."
                        .to_owned(),
                });
            }
            return message
                .get("result")
                .cloned()
                .ok_or_else(|| AppError::Runtime("Resposta de atividade incompleta.".to_owned()));
        }
    }

    fn send(&mut self, value: Value) -> Result<(), AppError> {
        self.socket
            .send(tungstenite::Message::Text(value.to_string().into()))
            .map_err(|_| {
                AppError::Runtime("Falha ao consultar o servidor compartilhado.".to_owned())
            })
    }
}

#[cfg(all(unix, target_os = "linux"))]
fn verify_peer(stream: &std::os::unix::net::UnixStream, expected_pid: u32) -> Result<(), AppError> {
    use std::os::fd::AsRawFd;
    let mut peer = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: the kernel fills the correctly sized ucred for this live socket.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            std::ptr::addr_of_mut!(peer).cast(),
            &mut length,
        )
    };
    // SAFETY: geteuid has no arguments/pointers and only reads process identity.
    let own_uid = unsafe { libc::geteuid() };
    if result != 0 || u32::try_from(peer.pid).ok() != Some(expected_pid) || peer.uid != own_uid {
        return Err(AppError::Runtime(
            "O socket não pertence ao daemon verificado deste usuário.".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(all(unix, not(target_os = "linux")))]
fn verify_peer(
    _stream: &std::os::unix::net::UnixStream,
    _expected_pid: u32,
) -> Result<(), AppError> {
    Err(AppError::Runtime(
        "A identidade do socket não pode ser validada nesta plataforma.".to_owned(),
    ))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn unix_websocket_preserves_rpc_codes_and_rejects_a_different_owner() {
        let fixture = tempfile::tempdir().expect("fixture");
        let path = fixture.path().join("control.sock");
        let listener = UnixListener::bind(&path).expect("listener");
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("peer");
            let mut socket = tungstenite::accept(stream).expect("handshake");
            let request = socket.read().expect("request");
            let request: Value =
                serde_json::from_str(request.to_text().expect("text")).expect("json");
            socket
                .send(tungstenite::Message::Text(
                    json!({"id":request["id"],"error":{"code":-32600,"message":"synthetic"}})
                        .to_string()
                        .into(),
                ))
                .expect("response");
        });
        let mut connection = Connection::connect(&path, std::process::id()).expect("verified peer");
        let result = connection.request("thread/loaded/list", json!({}), &AtomicBool::new(false));
        server.join().expect("fixture stopped");
        assert!(matches!(result, Err(AppError::Rpc { code: -32600, .. })));
        let wrong = Connection::connect(&path, std::process::id().wrapping_add(1));
        assert!(wrong.is_err());
    }

    #[test]
    #[ignore = "requires installed Codex; isolated home, no account, no model calls"]
    fn installed_unix_server_activity_is_readable_without_resuming_threads() {
        use std::process::{Command, Stdio};
        let binary = std::env::var_os("CODEX_MANAGER_CODEX_BINARY").expect("explicit Codex binary");
        let fixture = tempfile::tempdir().expect("private home");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(fixture.path(), std::fs::Permissions::from_mode(0o700))
            .expect("private socket parent");
        let socket = fixture.path().join("control.sock");
        let mut command = Command::new(binary);
        command
            .args(["app-server", "--listen"])
            .arg(format!("unix://{}", socket.display()))
            .env("CODEX_HOME", fixture.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let (mut child, supervisor) =
            crate::adapters::process_launcher::spawn_supervised(command).expect("owned server");
        let result = (|| -> Result<(), AppError> {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !socket.exists() && child.try_wait()?.is_none() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(25));
            }
            let mut connection = Connection::connect(&socket, child.id())?;
            let cancel = AtomicBool::new(false);
            connection.initialize(&cancel)?;
            let list = connection.request("thread/loaded/list", json!({"limit":128}), &cancel)?;
            assert!(super::super::activity::complete_list(&list)?.is_empty());
            assert!(!fixture.path().join("auth.json").exists());
            Ok(())
        })();
        supervisor
            .terminate(&mut child)
            .expect("owned server cleanup");
        let _ = child.wait();
        if result.is_err() {
            use std::io::Read;
            let mut diagnostics = String::new();
            if let Some(stderr) = child.stderr.take() {
                let _ = stderr.take(4096).read_to_string(&mut diagnostics);
            }
            eprintln!("{}", crate::adapters::logging::sanitize_error(&diagnostics));
        }
        result.expect("real Unix protocol");
    }
}

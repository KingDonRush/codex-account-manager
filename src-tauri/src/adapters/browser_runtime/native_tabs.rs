//! Managed Chromium's tab API through its native extension, not UI inference.
#[cfg(target_os = "linux")]
use super::native_bridge::ORIGIN;
use super::native_bridge::{self as wire, unavailable};
use crate::error::AppError;
use serde_json::{json, Value};
#[cfg(target_os = "linux")]
use std::{
    fs,
    io::{Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixListener},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};
#[cfg(target_os = "linux")]
type NativeStream = std::os::unix::net::UnixStream;
#[cfg(windows)]
pub(super) mod windows;
#[cfg(windows)]
pub(super) use windows::PendingNativeSession;
#[cfg(windows)]
type NativeStream = windows::BridgeStream;

#[cfg(target_os = "linux")]
pub(super) struct PendingNativeSession {
    directory: tempfile::TempDir,
    listener: UnixListener,
    secret: zeroize::Zeroizing<String>,
    host: PathBuf,
}
struct Connection {
    stream: NativeStream,
    next_id: u64,
    usable: bool,
}
pub(super) struct NativeSession {
    connection: parking_lot::Mutex<Connection>,
    pid: u32,
    started: u64,
    #[cfg(target_os = "linux")]
    _directory: tempfile::TempDir,
    #[cfg(windows)]
    _listener: windows::PrivateListener,
}

#[cfg(target_os = "linux")]
fn reject_link(path: &Path) -> Result<(), AppError> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(unavailable()),
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(unavailable()),
        _ => Ok(()),
    }
}

#[cfg(target_os = "linux")]
impl PendingNativeSession {
    pub(super) fn prepare(profile: &Path, host: &Path) -> Result<Self, AppError> {
        if !host.is_file() {
            return Err(AppError::Browser("O componente de navegador da instalação está incompleto. Os perfis e sessões foram preservados.".into()));
        }
        let hosts = profile.join("NativeMessagingHosts");
        reject_link(&hosts)?;
        fs::create_dir_all(&hosts)?;
        fs::set_permissions(&hosts, fs::Permissions::from_mode(0o700))?;
        let manifest = hosts.join(format!("{}.json", wire::HOST));
        reject_link(&manifest)?;
        if manifest.exists() {
            use std::os::unix::fs::OpenOptionsExt;
            let file = fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&manifest)?;
            let mut bytes = Vec::new();
            file.take(wire::MAX_FRAME as u64 + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() > wire::MAX_FRAME {
                return Err(unavailable());
            }
            let existing: Value = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
            if existing["name"] != wire::HOST
                || existing["description"] != "Codex Account Manager private browser bridge"
            {
                return Err(unavailable());
            }
        }
        let mut temporary = tempfile::NamedTempFile::new_in(&hosts)?;
        serde_json::to_writer(
            &mut temporary,
            &json!({"name":wire::HOST,"description":"Codex Account Manager private browser bridge", "path":host.canonicalize()?,"type":"stdio","allowed_origins":[ORIGIN]}),
        )?;
        temporary.write_all(b"\n")?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(&manifest)
            .map_err(|error| AppError::Io(error.error))?;
        let directory = tempfile::Builder::new().prefix(".cam-tabs-").tempdir()?;
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))?;
        let path = directory.path().join("control.sock");
        let listener = UnixListener::bind(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(|_| unavailable())?;
        Ok(Self {
            directory,
            listener,
            secret: zeroize::Zeroizing::new(hex::encode(random)),
            host: host.into(),
        })
    }
    pub(super) fn configure(&self, command: &mut Command) {
        command
            .env(wire::SOCKET_ENV, self.directory.path().join("control.sock"))
            .env(wire::SECRET_ENV, self.secret.as_str());
    }
    pub(super) fn connect(self, pid: u32) -> Result<NativeSession, AppError> {
        let started = wire::started_ticks(pid)?;
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut attempts = 0;
        loop {
            if Instant::now() >= deadline || attempts >= 16 {
                return Err(unavailable());
            }
            match self.listener.accept() {
                Ok((mut stream, _)) => {
                    attempts += 1;
                    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
                    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
                    if wire::peer_pid(&stream, &self.host, pid).is_err() {
                        continue;
                    }
                    let Ok(hello) = wire::read_frame(&mut stream) else {
                        continue;
                    };
                    if hello["origin"] != ORIGIN
                        || hello["secret"].as_str() != Some(self.secret.as_str())
                    {
                        continue;
                    }
                    let ready = wire::read_frame(&mut stream)?;
                    if ready["event"] != "ready" || ready["protocol"] != 1 {
                        return Err(unavailable());
                    }
                    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
                    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
                    return Ok(NativeSession {
                        connection: parking_lot::Mutex::new(Connection {
                            stream,
                            next_id: 1,
                            usable: true,
                        }),
                        pid,
                        started,
                        _directory: self.directory,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Err(_) => return Err(unavailable()),
            }
        }
    }
}

impl NativeSession {
    fn request(&self, method: &str, parameters: Value) -> Result<Value, AppError> {
        if wire::started_ticks(self.pid)? != self.started {
            return Err(unavailable());
        }
        let mut connection = self.connection.lock();
        if !connection.usable {
            return Err(unavailable());
        }
        #[cfg(windows)]
        connection
            .stream
            .set_timeout(std::time::Duration::from_secs(5));
        let id = connection.next_id;
        connection.next_id = id
            .checked_add(1)
            .filter(|id| *id < (1u64 << 53))
            .ok_or_else(unavailable)?;
        let mut message = parameters.as_object().cloned().ok_or_else(unavailable)?;
        message.insert("id".into(), json!(id));
        message.insert("method".into(), json!(method));
        if wire::write_frame(&mut connection.stream, &Value::Object(message)).is_err() {
            connection.usable = false;
            return Err(unavailable());
        }
        for _ in 0..32 {
            let response = match wire::read_frame(&mut connection.stream) {
                Ok(value) => value,
                Err(error) => {
                    connection.usable = false;
                    return Err(error);
                }
            };
            if response["id"] != id {
                continue;
            }
            if response.get("error").is_some() {
                // The complete error frame leaves the channel usable for
                // compensation using the operation reserved before creation.
                return Err(AppError::Browser("O navegador não concluiu esta ação. Suas outras abas e as contas salvas foram preservadas.".into()));
            }
            return response.get("result").cloned().ok_or_else(unavailable);
        }
        connection.usable = false;
        Err(unavailable())
    }
    pub(super) fn create_operation(
        &self,
        operation: &str,
        url: &str,
        mail: Option<&str>,
    ) -> Result<(), AppError> {
        let urls = std::iter::once(url).chain(mail).collect::<Vec<_>>();
        self.request(
            "createOperation",
            json!({"operation":operation,"urls":urls}),
        )?;
        Ok(())
    }
    pub(super) fn operation_alive(&self, operation: &str) -> Result<bool, AppError> {
        self.request("operationStatus", json!({"operation":operation}))?["alive"]
            .as_bool()
            .ok_or_else(unavailable)
    }
    pub(super) fn close_operation(&self, operation: &str) -> Result<bool, AppError> {
        self.request("closeOperation", json!({"operation":operation}))?["empty"]
            .as_bool()
            .ok_or_else(unavailable)
    }
    pub(super) fn open_manual(&self, url: &str) -> Result<(), AppError> {
        self.request("openManual", json!({"url":url}))?;
        Ok(())
    }
    pub(super) fn close_browser(&self) -> Result<(), AppError> {
        self.request("closeBrowser", json!({}))?;
        Ok(())
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    #[test]
    fn native_host_registration_never_follows_links_or_overwrites_another_owner() {
        let directory = tempfile::tempdir().expect("own fixture");
        let host = std::env::current_exe().expect("fixture path");
        let outside = directory.path().join("outside");
        fs::create_dir(&outside).expect("outside");
        let profile = directory.path().join("profile");
        fs::create_dir(&profile).expect("profile");
        std::os::unix::fs::symlink(&outside, profile.join("NativeMessagingHosts"))
            .expect("fixture link");
        assert!(PendingNativeSession::prepare(&profile, &host).is_err());
        assert_eq!(
            fs::read_dir(&outside).expect("outside preserved").count(),
            0
        );
        fs::remove_file(profile.join("NativeMessagingHosts")).expect("remove own link");
        fs::create_dir(profile.join("NativeMessagingHosts")).expect("own directory");
        let manifest = profile
            .join("NativeMessagingHosts")
            .join(format!("{}.json", wire::HOST));
        let original = br#"{"name":"foreign","description":"not this app"}"#;
        fs::write(&manifest, original).expect("foreign fixture");
        assert!(PendingNativeSession::prepare(&profile, &host).is_err());
        assert_eq!(fs::read(manifest).expect("unchanged"), original);
    }
}

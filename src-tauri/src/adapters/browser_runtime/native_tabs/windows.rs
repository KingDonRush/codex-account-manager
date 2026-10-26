//! Windows side of the same private browser protocol. Pipe security and peer
//! identity reuse the existing OS transport; no account/API authority is reused.
use super::{wire, Connection, NativeSession};
use crate::{
    adapters::file_lock::FileLock,
    application::local_transport::{
        windows::{connect_at, PipeListener, PipeStream},
        LocalStream, PeerIdentity,
    },
    error::AppError,
};
use serde_json::Value;
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::{Duration, Instant},
};
use winreg::{
    enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WOW64_32KEY, KEY_WRITE},
    RegKey,
};

const REGISTRY: &str =
    r"SOFTWARE\Google\Chrome\NativeMessagingHosts\com.codex.account_manager.browser";
const SERVER_ENV: &str = "CAM_BROWSER_CONTROL_SERVER";
const SERVER_PID_ENV: &str = "CAM_BROWSER_CONTROL_SERVER_PID";

/// Chrome on Windows requires a registry lookup. Hold the app's startup lease
/// only through handshake, then restore the exact prior value before OAuth.
/// A new root never overwrites a foreign host and normal use leaves no new key.
struct Registration {
    root: RegKey,
    previous: Option<String>,
    path: String,
    restored: bool,
    _lease: FileLock,
}
impl Registration {
    fn prepare(host: &Path) -> Result<Self, AppError> {
        let canonical = host
            .parent()
            .ok_or_else(wire::unavailable)?
            .join("runtime/browser-control/windows-host.json")
            .canonicalize()?;
        // Win32's verbatim prefix disables dot-segment normalization, while
        // Chrome resolves our packaged relative exe path from this directory.
        let text = canonical.to_string_lossy();
        let path = PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text));
        let manifest = read_manifest(&path)?;
        let expected = path
            .parent()
            .ok_or_else(wire::unavailable)?
            .join(manifest["path"].as_str().ok_or_else(wire::unavailable)?)
            .canonicalize()?;
        if expected != host.canonicalize()? {
            return Err(wire::unavailable());
        }
        #[cfg(not(test))]
        let lock = dirs::data_local_dir()
            .ok_or_else(wire::unavailable)?
            .join("com.codex.account-manager/browser-native-host.lock");
        #[cfg(test)]
        let lock = host.with_file_name("browser-native-host.lock");
        let lease = FileLock::acquire_cancellable(
            &lock,
            &std::sync::atomic::AtomicBool::new(false),
            Duration::from_secs(8),
        )?;
        let root = RegKey::predef(HKEY_CURRENT_USER);
        let previous = match root.open_subkey_with_flags(REGISTRY, KEY_READ | KEY_WOW64_32KEY) {
            Ok(key) => {
                let previous: String = key.get_value("")?;
                let old = read_manifest(Path::new(&previous))?;
                let old_host = Path::new(&previous)
                    .parent()
                    .ok_or_else(wire::unavailable)?
                    .join(old["path"].as_str().ok_or_else(wire::unavailable)?)
                    .canonicalize()?;
                if old_host != host.canonicalize()? {
                    return Err(wire::unavailable());
                }
                Some(previous)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let registration = Self {
            root,
            previous,
            path: path.to_string_lossy().into_owned(),
            restored: false,
            _lease: lease,
        };
        let (key, _) = registration
            .root
            .create_subkey_with_flags(REGISTRY, KEY_READ | KEY_WRITE | KEY_WOW64_32KEY)?;
        // Drop compensates even if setting the value fails after key creation.
        key.set_value("", &registration.path)?;
        Ok(registration)
    }
    fn restore(&mut self) -> Result<(), AppError> {
        if self.restored {
            return Ok(());
        }
        let key = self
            .root
            .open_subkey_with_flags(REGISTRY, KEY_READ | KEY_WRITE | KEY_WOW64_32KEY)?;
        let current: String = key.get_value("")?;
        if current != self.path {
            return Err(wire::unavailable());
        }
        if let Some(previous) = &self.previous {
            key.set_value("", previous)?;
        } else {
            drop(key);
            self.root
                .delete_subkey_with_flags(REGISTRY, KEY_WOW64_32KEY)?;
        }
        self.restored = true;
        Ok(())
    }
}
impl Drop for Registration {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn read_manifest(path: &Path) -> Result<Value, AppError> {
    use std::os::windows::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file()
        || metadata.file_attributes() & 0x400 != 0
        || metadata.len() > wire::MAX_FRAME as u64
    {
        return Err(wire::unavailable());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(wire::MAX_FRAME as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > wire::MAX_FRAME {
        return Err(wire::unavailable());
    }
    let value: Value = serde_json::from_slice(&bytes)?;
    if value["name"] != wire::HOST
        || value["description"] != "Codex Account Manager private browser bridge"
        || value["type"] != "stdio"
        || value["allowed_origins"] != serde_json::json!([wire::ORIGIN])
    {
        return Err(wire::unavailable());
    }
    Ok(value)
}

pub(in super::super) type PrivateListener = PipeListener;
pub(in super::super) struct PendingNativeSession {
    listener: PipeListener,
    name: String,
    host: PathBuf,
    secret: zeroize::Zeroizing<String>,
    registration: Registration,
}
impl PendingNativeSession {
    pub(in super::super) fn prepare(_profile: &Path, host: &Path) -> Result<Self, AppError> {
        let registration = Registration::prepare(host)?;
        let name = format!(r"\\.\pipe\cam-browser-{}", uuid::Uuid::new_v4());
        let listener = PipeListener::bind_at(name.clone())?;
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(|_| wire::unavailable())?;
        Ok(Self {
            listener,
            name,
            host: host.into(),
            secret: zeroize::Zeroizing::new(hex::encode(random)),
            registration,
        })
    }
    pub(in super::super) fn configure(&self, command: &mut Command) -> Result<(), AppError> {
        command
            .env(wire::SOCKET_ENV, &self.name)
            .env(wire::SECRET_ENV, self.secret.as_str())
            .env(SERVER_ENV, std::env::current_exe()?)
            .env(SERVER_PID_ENV, std::process::id().to_string());
        Ok(())
    }
    pub(in super::super) fn connect(mut self, pid: u32) -> Result<NativeSession, AppError> {
        let started = wire::started_ticks(pid)?;
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut attempts = 0;
        while Instant::now() < deadline && attempts < 16 {
            let Some(pipe) = self.listener.accept()? else {
                continue;
            };
            attempts += 1;
            let mut stream = BridgeStream::new(pipe);
            stream.set_timeout(Duration::from_secs(2));
            if wire::peer_pid(&stream, &self.host, pid).is_err() {
                continue;
            }
            let Ok(hello) = wire::read_frame(&mut stream) else {
                continue;
            };
            if hello["origin"] != wire::ORIGIN
                || hello["secret"].as_str() != Some(self.secret.as_str())
            {
                continue;
            }
            let ready = wire::read_frame(&mut stream)?;
            if ready["event"] != "ready" || ready["protocol"] != 1 {
                return Err(wire::unavailable());
            }
            self.registration.restore()?;
            return Ok(NativeSession {
                connection: parking_lot::Mutex::new(Connection {
                    stream,
                    next_id: 1,
                    usable: true,
                }),
                pid,
                started,
                _listener: self.listener,
            });
        }
        Err(wire::unavailable())
    }
}

/// Tokio owns overlapped pipe I/O. Shared clones do not hold the mutex while
/// waiting, so an idle reader never blocks the independent writer.
#[derive(Clone)]
pub(in super::super) struct BridgeStream {
    pipe: Arc<parking_lot::Mutex<PipeStream>>,
    deadline: Option<Instant>,
}
impl BridgeStream {
    fn new(pipe: PipeStream) -> Self {
        Self {
            pipe: Arc::new(parking_lot::Mutex::new(pipe)),
            deadline: None,
        }
    }
    pub(in super::super) fn peer(&self) -> Result<PeerIdentity, AppError> {
        self.pipe.lock().peer()
    }
    pub(in super::super) fn set_timeout(&mut self, timeout: Duration) {
        self.deadline = Some(Instant::now() + timeout);
    }
    pub(in super::super) fn try_clone(&self) -> std::io::Result<Self> {
        Ok(self.clone())
    }
    fn retry<T>(
        &mut self,
        mut action: impl FnMut(&mut PipeStream) -> std::io::Result<T>,
    ) -> std::io::Result<T> {
        loop {
            if self
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
            {
                return Err(std::io::ErrorKind::TimedOut.into());
            }
            let result = action(&mut self.pipe.lock());
            match result {
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                result => return result,
            }
        }
    }
}
impl Read for BridgeStream {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.retry(|pipe| pipe.read(bytes))
    }
}
impl Write for BridgeStream {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.retry(|pipe| pipe.write(bytes))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.pipe.lock().flush()
    }
}
pub(in super::super) fn connect_host() -> Result<BridgeStream, AppError> {
    let name = std::env::var(wire::SOCKET_ENV).map_err(|_| wire::unavailable())?;
    if !name.starts_with(r"\\.\pipe\cam-browser-") {
        return Err(wire::unavailable());
    }
    let expected = std::env::var_os(SERVER_ENV).ok_or_else(wire::unavailable)?;
    let expected_pid: u32 = std::env::var(SERVER_PID_ENV)
        .map_err(|_| wire::unavailable())?
        .parse()
        .map_err(|_| wire::unavailable())?;
    let stream = BridgeStream::new(connect_at(&name, Path::new(&expected))?);
    if stream.peer()?.pid != expected_pid {
        return Err(wire::unavailable());
    }
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_frames_over_real_private_pipe_have_deadline_exact_peer_and_no_namespace_gap() {
        let name = format!(r"\\.\pipe\cam-browser-test-{}", uuid::Uuid::new_v4());
        let mut listener = PipeListener::bind_at(name.clone()).expect("private listener");
        assert!(PipeListener::bind_at(name.clone()).is_err());
        let exe = std::env::current_exe().expect("own executable");
        let mut client = BridgeStream::new(connect_at(&name, &exe).expect("verified server"));
        let mut server = BridgeStream::new(listener.accept().expect("accept").expect("connected"));
        client.set_timeout(Duration::from_secs(1));
        server.set_timeout(Duration::from_secs(1));
        let value = serde_json::json!({"id":1,"synthetic":"ação 🎛️"});
        wire::write_frame(&mut client, &value).expect("write native frame");
        assert_eq!(
            wire::read_frame(&mut server).expect("read native frame"),
            value
        );
        assert_eq!(
            wire::peer_pid(&server, &exe, std::process::id()).expect("exact peer"),
            std::process::id()
        );
        assert!(wire::peer_pid(&server, &exe, 0).is_err());
        assert!(wire::peer_pid(
            &server,
            &exe.with_file_name("foreign-missing.exe"),
            std::process::id()
        )
        .is_err());
        server.set_timeout(Duration::from_millis(60));
        let started = Instant::now();
        assert!(wire::read_frame(&mut server).is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
        drop(server);
        drop(client);
        drop(listener);
        assert!(PipeListener::bind_at(name).is_ok());
    }

    #[test]
    fn foreign_and_oversized_native_host_manifests_are_rejected() {
        let directory = tempfile::tempdir().expect("private metadata fixture");
        let path = directory.path().join("host.json");
        std::fs::write(&path, br#"{"name":"foreign","description":"not ours"}"#)
            .expect("foreign fixture");
        assert!(read_manifest(&path).is_err());
        std::fs::write(&path, vec![b' '; wire::MAX_FRAME + 1]).expect("oversized fixture");
        assert!(read_manifest(&path).is_err());
    }
}

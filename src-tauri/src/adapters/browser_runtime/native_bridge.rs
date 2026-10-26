//! Chrome Native Messaging framing and a private, same-user stdio bridge.
//! This executable path never starts Tauri, reads accounts or evaluates code.
use crate::error::AppError;
use serde_json::{json, Value};
#[cfg(target_os = "linux")]
use std::os::unix::net::UnixStream;
use std::{
    io::{Read, Write},
    path::Path,
};

pub(super) const MAX_FRAME: usize = 64 * 1024;
pub(super) const ORIGIN: &str = "chrome-extension://cjobpmmgbncdflmiccbcjfpghmlnkbhg/";
pub(super) const HOST: &str = "com.codex.account_manager.browser";
pub(super) const SOCKET_ENV: &str = "CAM_BROWSER_CONTROL_SOCKET";
pub(super) const SECRET_ENV: &str = "CAM_BROWSER_CONTROL_SECRET";

pub(super) fn unavailable() -> AppError {
    AppError::Browser("O componente privado de abas do navegador não respondeu. As contas e as janelas de outros aplicativos foram preservadas.".into())
}

pub(super) fn read_frame(reader: &mut impl Read) -> Result<Value, AppError> {
    let mut prefix = [0u8; 4];
    reader.read_exact(&mut prefix).map_err(|_| unavailable())?;
    let length = u32::from_ne_bytes(prefix) as usize;
    if length == 0 || length > MAX_FRAME {
        return Err(unavailable());
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).map_err(|_| unavailable())?;
    serde_json::from_slice(&body).map_err(|_| unavailable())
}

pub(super) fn write_frame(writer: &mut impl Write, value: &Value) -> Result<(), AppError> {
    let body = serde_json::to_vec(value).map_err(|_| unavailable())?;
    if body.len() > MAX_FRAME {
        return Err(unavailable());
    }
    writer
        .write_all(&(body.len() as u32).to_ne_bytes())
        .map_err(|_| unavailable())?;
    writer.write_all(&body).map_err(|_| unavailable())?;
    writer.flush().map_err(|_| unavailable())
}

pub(crate) fn is_host_request() -> bool {
    std::env::args()
        .nth(1)
        .is_some_and(|arg| arg.starts_with("chrome-extension://"))
}

pub(crate) fn run_host() -> Result<(), AppError> {
    if std::env::args().nth(1).as_deref() != Some(ORIGIN) {
        return Err(unavailable());
    }
    let secret = zeroize::Zeroizing::new(std::env::var(SECRET_ENV).map_err(|_| unavailable())?);
    if secret.len() != 64 {
        return Err(unavailable());
    }
    #[cfg(target_os = "linux")]
    let mut writer = UnixStream::connect(std::env::var_os(SOCKET_ENV).ok_or_else(unavailable)?)
        .map_err(|_| unavailable())?;
    #[cfg(windows)]
    let mut writer = super::native_tabs::windows::connect_host()?;
    write_frame(
        &mut writer,
        &json!({"origin":ORIGIN,"secret":secret.as_str()}),
    )?;
    let mut reader = writer.try_clone().map_err(|_| unavailable())?;
    std::thread::Builder::new()
        .name("browser-native-stdout".into())
        .spawn(move || {
            let mut output = std::io::stdout().lock();
            while let Ok(message) = read_frame(&mut reader) {
                if write_frame(&mut output, &message).is_err() {
                    break;
                }
            }
            // This dedicated helper owns only stdio/socket FDs. Exit closes stdin
            // too; a half-closed native port cannot leave a waiting copy thread.
            std::process::exit(0);
        })
        .map_err(|_| unavailable())?;
    // Explicit frames keep byte-length limits and flush binary stdout, which
    // Rust otherwise line-buffers. No frame contents are logged or persisted.
    let mut input = std::io::stdin().lock();
    while let Ok(message) = read_frame(&mut input) {
        if write_frame(&mut writer, &message).is_err() {
            break;
        }
    }
    std::process::exit(0)
}

#[cfg(target_os = "linux")]
pub(super) fn peer_pid(
    stream: &UnixStream,
    executable: &Path,
    ancestor: u32,
) -> Result<u32, AppError> {
    use std::os::fd::AsRawFd;
    // SAFETY: ucred is a plain C structure initialized by a successful getsockopt.
    let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
    let mut size = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: live connected socket, writable correctly sized ucred storage.
    if unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut size,
        )
    } != 0
    {
        return Err(unavailable());
    }
    // SAFETY: geteuid has no pointer arguments or side effects.
    if credentials.uid != unsafe { libc::geteuid() } || credentials.pid <= 0 {
        return Err(unavailable());
    }
    let pid = credentials.pid as u32;
    let actual = std::fs::read_link(format!("/proc/{pid}/exe")).map_err(|_| unavailable())?;
    if actual != executable.canonicalize().map_err(|_| unavailable())? {
        return Err(unavailable());
    }
    let mut current = pid;
    for _ in 0..32 {
        if current == ancestor {
            return Ok(pid);
        }
        let stat =
            std::fs::read_to_string(format!("/proc/{current}/stat")).map_err(|_| unavailable())?;
        current = stat
            .rsplit_once(") ")
            .and_then(|(_, rest)| rest.split_whitespace().nth(1))
            .and_then(|value| value.parse().ok())
            .filter(|value| *value > 1)
            .ok_or_else(unavailable)?;
    }
    Err(unavailable())
}

pub(super) fn started_ticks(pid: u32) -> Result<u64, AppError> {
    #[cfg(target_os = "linux")]
    return super::super::process_supervisor::linux_started_ticks(pid as i32);
    #[cfg(windows)]
    return crate::adapters::desktop_control::process_identity::created_ticks(pid);
}

#[cfg(windows)]
pub(super) fn peer_pid(
    stream: &super::native_tabs::windows::BridgeStream,
    executable: &Path,
    ancestor: u32,
) -> Result<u32, AppError> {
    let peer = stream.peer()?;
    if Path::new(&peer.executable)
        .canonicalize()?
        .to_string_lossy()
        .to_lowercase()
        != executable.canonicalize()?.to_string_lossy().to_lowercase()
    {
        return Err(unavailable());
    }
    let mut system = sysinfo::System::new();
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::All,
        true,
        sysinfo::ProcessRefreshKind::nothing(),
    );
    let mut current = sysinfo::Pid::from_u32(peer.pid);
    for _ in 0..32 {
        if current.as_u32() == ancestor {
            return Ok(peer.pid);
        }
        current = system
            .process(current)
            .and_then(sysinfo::Process::parent)
            .ok_or_else(unavailable)?;
    }
    Err(unavailable())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_frames_use_utf8_byte_length_and_reject_truncation_and_oversize() {
        let value = json!({"message":"ação 🎛️"});
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &value).expect("native frame");
        assert_eq!(
            u32::from_ne_bytes(bytes[..4].try_into().expect("length")) as usize,
            bytes.len() - 4
        );
        assert_eq!(read_frame(&mut &bytes[..]).expect("read"), value);
        assert!(read_frame(&mut &bytes[..bytes.len() - 1]).is_err());
        assert!(read_frame(&mut &((MAX_FRAME as u32 + 1).to_ne_bytes())[..]).is_err());
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn native_peer_must_be_same_user_expected_executable_and_descendant() {
        let (left, _right) = UnixStream::pair().expect("private endpoint");
        let current = std::env::current_exe().expect("fixture executable");
        assert_eq!(
            peer_pid(&left, &current, std::process::id()).expect("verified peer"),
            std::process::id()
        );
        assert!(peer_pid(
            &left,
            Path::new("/nonexistent/native-helper"),
            std::process::id()
        )
        .is_err());
        assert!(peer_pid(&left, &current, 0).is_err());
    }
}

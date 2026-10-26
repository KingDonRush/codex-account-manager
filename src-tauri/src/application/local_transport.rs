use crate::error::AppError;
use serde::{de::DeserializeOwned, Serialize};
use std::io::{Read, Write};
use std::time::{Duration, Instant};

pub const MAX_FRAME: usize = 256 * 1024;

#[cfg(windows)]
pub mod windows;

/// Platform transport owns nonblocking I/O and an OS-verified peer, not consent.
pub trait LocalStream: Read + Write {
    fn prepare(&mut self) -> std::io::Result<()>;
    fn peer(&self) -> Result<PeerIdentity, AppError>;
}

#[cfg(unix)]
impl LocalStream for std::os::unix::net::UnixStream {
    fn prepare(&mut self) -> std::io::Result<()> {
        self.set_nonblocking(true)
    }
    fn peer(&self) -> Result<PeerIdentity, AppError> {
        peer_identity(self)
    }
}

#[cfg(unix)]
pub fn connect_local() -> Result<impl LocalStream, AppError> {
    let directory = dirs::data_dir()
        .ok_or_else(|| AppError::Internal("Diretório de dados indisponível.".to_owned()))?
        .join("com.codex.account-manager");
    let mut stream =
        std::os::unix::net::UnixStream::connect(directory.join("integration-v2.sock"))?;
    stream.prepare()?;
    Ok(stream)
}

#[cfg(windows)]
pub fn connect_local() -> Result<impl LocalStream, AppError> {
    windows::connect()
}

#[derive(Clone)]
pub struct PeerIdentity {
    pub pid: u32,
    pub executable: String,
    pub identity: String,
}

pub fn read_frame<T: DeserializeOwned>(
    stream: &mut impl Read,
    timeout: Duration,
) -> Result<T, AppError> {
    let deadline = Instant::now() + timeout;
    let mut header = [0_u8; 4];
    read_exact_until(stream, &mut header, deadline, None)?;
    let size = u32::from_be_bytes(header) as usize;
    if size > MAX_FRAME {
        return Err(AppError::Validation(
            "Mensagem excede o limite do protocolo.".to_owned(),
        ));
    }
    let mut body = vec![0_u8; size];
    read_exact_until(stream, &mut body, deadline, None)?;
    Ok(serde_json::from_slice(&body)?)
}

pub fn write_frame<T: Serialize>(stream: &mut impl Write, value: &T) -> Result<(), AppError> {
    let body = serde_json::to_vec(value)?;
    if body.len() > MAX_FRAME {
        return Err(AppError::Validation(
            "Mensagem excede o limite do protocolo.".to_owned(),
        ));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    write_until(stream, &(body.len() as u32).to_be_bytes(), deadline)?;
    write_until(stream, &body, deadline)
}

fn read_exact_until(
    stream: &mut impl Read,
    bytes: &mut [u8],
    deadline: Instant,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<(), AppError> {
    let mut offset = 0;
    while offset < bytes.len() {
        if cancel.is_some_and(|cancel| cancel.load(std::sync::atomic::Ordering::Acquire)) {
            return Err(AppError::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(AppError::TimedOut);
        }
        match stream.read(&mut bytes[offset..]) {
            Ok(0) => return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into()),
            Ok(read) => offset += read,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) =>
            {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub fn read_frame_cancellable<T: DeserializeOwned>(
    stream: &mut impl Read,
    timeout: Duration,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<T, AppError> {
    let deadline = Instant::now() + timeout;
    let mut header = [0_u8; 4];
    read_exact_until(stream, &mut header, deadline, Some(cancel))?;
    let size = u32::from_be_bytes(header) as usize;
    if size > MAX_FRAME {
        return Err(AppError::Validation(
            "Mensagem excede o limite do protocolo.".to_owned(),
        ));
    }
    let mut body = vec![0_u8; size];
    read_exact_until(stream, &mut body, deadline, Some(cancel))?;
    Ok(serde_json::from_slice(&body)?)
}

fn write_until(stream: &mut impl Write, bytes: &[u8], deadline: Instant) -> Result<(), AppError> {
    let mut offset = 0;
    while offset < bytes.len() {
        if Instant::now() >= deadline {
            return Err(AppError::TimedOut);
        }
        match stream.write(&bytes[offset..]) {
            Ok(0) => return Err(std::io::Error::from(std::io::ErrorKind::WriteZero).into()),
            Ok(written) => offset += written,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

#[cfg(unix)]
pub fn peer_identity(stream: &std::os::unix::net::UnixStream) -> Result<PeerIdentity, AppError> {
    use std::os::fd::AsRawFd;
    let mut peer: libc::ucred = unsafe { std::mem::zeroed() };
    let mut size = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: valid connected socket and correctly sized writable ucred storage.
    if unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut peer as *mut libc::ucred).cast(),
            &mut size,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: geteuid has no preconditions or pointer arguments.
    if peer.uid != unsafe { libc::geteuid() } {
        return Err(AppError::AccessDenied(
            "Outro usuário do sistema.".to_owned(),
        ));
    }
    let executable = std::fs::read_link(format!("/proc/{}/exe", peer.pid))?
        .to_string_lossy()
        .into_owned();
    Ok(PeerIdentity {
        pid: peer.pid as u32,
        identity: format!("{}:{executable}", peer.uid),
        executable,
    })
}

//! Per-user/session local pipes. The shared protocol still owns authorization.
mod security;
#[cfg(test)]
mod tests;
use super::{LocalStream, PeerIdentity};
use crate::error::AppError;
use std::{
    io::{Read, Write},
    os::windows::io::AsRawHandle,
    path::Path,
    time::Duration,
};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient, NamedPipeServer};
use windows::Win32::{
    Foundation::HANDLE,
    System::Pipes::{GetNamedPipeClientProcessId, GetNamedPipeServerProcessId},
};

enum Pipe {
    Server(NamedPipeServer),
    Client(NamedPipeClient),
}
pub struct PipeStream {
    pipe: Pipe,
    peer: PeerIdentity,
}
impl Read for PipeStream {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        match &self.pipe {
            Pipe::Server(pipe) => pipe.try_read(bytes),
            Pipe::Client(pipe) => pipe.try_read(bytes),
        }
    }
}
impl Write for PipeStream {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        match &self.pipe {
            Pipe::Server(pipe) => pipe.try_write(bytes),
            Pipe::Client(pipe) => pipe.try_write(bytes),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl LocalStream for PipeStream {
    fn prepare(&mut self) -> std::io::Result<()> {
        Ok(())
    }
    fn peer(&self) -> Result<PeerIdentity, AppError> {
        Ok(self.peer.clone())
    }
}

fn endpoint() -> Result<String, AppError> {
    let (sid, session) = security::current_scope()?;
    Ok(format!(
        r"\\.\pipe\codex-account-manager-v2-{sid}-{session}"
    ))
}

pub fn connect() -> Result<PipeStream, AppError> {
    let expected = std::env::current_exe()?.with_file_name("codex-account-manager.exe");
    connect_at(&endpoint()?, &expected)
}
pub(crate) fn connect_at(name: &str, expected: &Path) -> Result<PipeStream, AppError> {
    let client = tauri::async_runtime::block_on(async { ClientOptions::new().open(name) })?;
    let mut pid = 0;
    // SAFETY: live client pipe and initialized PID output; no trust in client args.
    unsafe { GetNamedPipeServerProcessId(HANDLE(client.as_raw_handle()), &mut pid) }
        .map_err(security::native_error)?;
    let peer = security::peer(pid)?;
    security::verify_server(&peer, expected)?;
    Ok(PipeStream {
        pipe: Pipe::Client(client),
        peer,
    })
}

pub struct PipeListener {
    name: String,
    pending: NamedPipeServer,
}
impl PipeListener {
    pub fn bind() -> Result<Self, AppError> {
        Self::bind_at(endpoint()?)
    }
    pub(crate) fn bind_at(name: String) -> Result<Self, AppError> {
        let pending = security::create_server(&name, true)?;
        Ok(Self { name, pending })
    }
    /// One pending instance stays alive while accepted clients are transferred;
    /// there is no namespace gap for a competing server or orphan pipe file.
    pub fn accept(&mut self) -> Result<Option<PipeStream>, AppError> {
        let connected = tauri::async_runtime::block_on(async {
            tokio::time::timeout(Duration::from_millis(25), self.pending.connect()).await
        });
        let Ok(connected) = connected else {
            return Ok(None);
        };
        connected?;
        let next = security::create_server(&self.name, false)?;
        let server = std::mem::replace(&mut self.pending, next);
        let mut pid = 0;
        // SAFETY: connected server pipe and initialized output remain valid.
        unsafe { GetNamedPipeClientProcessId(HANDLE(server.as_raw_handle()), &mut pid) }
            .map_err(security::native_error)?;
        Ok(Some(PipeStream {
            peer: security::peer(pid)?,
            pipe: Pipe::Server(server),
        }))
    }
}

use crate::{application::local_transport::PeerIdentity, error::AppError};
use std::{mem::size_of, path::Path};
use windows::core::{Owned, PCWSTR, PWSTR};
use windows::Win32::{
    Foundation::{HANDLE, HLOCAL},
    Security::{
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        },
        GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
        TOKEN_USER,
    },
    System::{
        RemoteDesktop::ProcessIdToSessionId,
        Threading::{
            GetCurrentProcess, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW,
            PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
        },
    },
};

pub(super) fn native_error(error: windows::core::Error) -> AppError {
    AppError::AccessDenied(format!(
        "Identidade do transporte Windows não confirmada: {error}"
    ))
}

pub(super) fn current_scope() -> Result<(String, u32), AppError> {
    // SAFETY: pseudo-handle is borrowed; process_sid owns only the token it opens.
    let sid = process_sid(unsafe { GetCurrentProcess() })?;
    Ok((sid, session_id(std::process::id())?))
}

fn session_id(pid: u32) -> Result<u32, AppError> {
    let mut session = 0;
    // SAFETY: initialized output and OS-owned PID, no caller-controlled pointers.
    unsafe { ProcessIdToSessionId(pid, &mut session) }.map_err(native_error)?;
    Ok(session)
}

fn process_sid(process: HANDLE) -> Result<String, AppError> {
    let mut token = HANDLE::default();
    // SAFETY: live process handle and writable token output; TOKEN_QUERY only.
    unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }.map_err(native_error)?;
    let token = unsafe { Owned::new(token) };
    let mut needed = 0;
    // SAFETY: size-only query; no output buffer is supplied.
    let _ = unsafe { GetTokenInformation(*token, TokenUser, None, 0, &mut needed) };
    if needed < size_of::<TOKEN_USER>() as u32 || needed > 64 * 1024 {
        return Err(AppError::AccessDenied(
            "SID do usuário não disponível.".into(),
        ));
    }
    let mut buffer = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
    // SAFETY: word-aligned allocation covers the requested byte length.
    unsafe {
        GetTokenInformation(
            *token,
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            needed,
            &mut needed,
        )
    }
    .map_err(native_error)?;
    // SAFETY: successful TokenUser query populated the aligned TOKEN_USER prefix.
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    let mut text = PWSTR::null();
    // SAFETY: SID lives in buffer; conversion allocates a NUL-terminated string.
    unsafe { ConvertSidToStringSidW(user.User.Sid, &mut text) }.map_err(native_error)?;
    let _text_owner = unsafe { Owned::new(HLOCAL(text.0.cast())) };
    // SAFETY: successful conversion guarantees the UTF-16 string while owner lives.
    unsafe { text.to_string() }.map_err(|_| AppError::AccessDenied("SID textual inválido.".into()))
}

pub(super) fn peer(pid: u32) -> Result<PeerIdentity, AppError> {
    // SAFETY: query-only handle; ownership retained during identity extraction.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .map_err(native_error)?;
    let process = unsafe { Owned::new(handle) };
    let sid = process_sid(*process)?;
    let (expected, session) = current_scope()?;
    if sid != expected || session_id(pid)? != session {
        return Err(AppError::AccessDenied(
            "Cliente de outro usuário ou sessão Windows.".into(),
        ));
    }
    let mut executable = vec![0u16; 32768];
    let mut length = executable.len() as u32;
    // SAFETY: buffer and length cover the full path; process handle remains live.
    unsafe {
        QueryFullProcessImageNameW(
            *process,
            PROCESS_NAME_FORMAT(0),
            PWSTR(executable.as_mut_ptr()),
            &mut length,
        )
    }
    .map_err(native_error)?;
    let executable = String::from_utf16(&executable[..length as usize])
        .map_err(|_| AppError::AccessDenied("Caminho do cliente inválido.".into()))?;
    Ok(PeerIdentity {
        pid,
        // Session gates the live channel, but is not persisted identity: Windows
        // can assign a different session after reboot without changing the user.
        identity: format!("{sid}:{}", executable.to_lowercase()),
        executable,
    })
}

pub(super) fn verify_server(peer: &PeerIdentity, expected: &Path) -> Result<(), AppError> {
    let actual = Path::new(&peer.executable).canonicalize()?;
    let expected = expected.canonicalize()?;
    if actual.to_string_lossy().to_lowercase() != expected.to_string_lossy().to_lowercase() {
        return Err(AppError::AccessDenied(
            "O pipe não pertence ao gerenciador instalado junto desta CLI.".into(),
        ));
    }
    Ok(())
}

pub(super) fn create_server(
    name: &str,
    first: bool,
) -> Result<tokio::net::windows::named_pipe::NamedPipeServer, AppError> {
    let (sid, _) = current_scope()?;
    // No Everyone/Anonymous/default ACL. Session is checked again from the OS peer.
    let descriptor: Vec<u16> = format!("D:P(A;;GA;;;{sid})")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut pointer = PSECURITY_DESCRIPTOR::default();
    // SAFETY: valid terminated SDDL; success returns LocalAlloc-owned descriptor.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(descriptor.as_ptr()),
            1,
            &mut pointer,
            None,
        )
    }
    .map_err(native_error)?;
    let _descriptor_owner = unsafe { Owned::new(HLOCAL(pointer.0)) };
    let mut attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: pointer.0,
        bInheritHandle: false.into(),
    };
    tauri::async_runtime::block_on(async {
        // SAFETY: attributes/descriptor outlive creation, non-inheritable; Tokio
        // owns overlapped I/O, closes the handle and cancels pending I/O on drop.
        unsafe {
            tokio::net::windows::named_pipe::ServerOptions::new()
                .first_pipe_instance(first)
                .reject_remote_clients(true)
                .max_instances(18)
                .in_buffer_size(64 * 1024)
                .out_buffer_size(64 * 1024)
                .create_with_security_attributes_raw(
                    name,
                    std::ptr::addr_of_mut!(attributes).cast(),
                )
        }
        .map_err(AppError::Io)
    })
}

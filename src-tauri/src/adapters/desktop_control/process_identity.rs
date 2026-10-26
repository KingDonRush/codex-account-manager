use windows::core::Owned;
use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

use crate::error::AppError;

pub(crate) fn created_ticks(pid: u32) -> Result<u64, AppError> {
    // SAFETY: query-only access; ownership closes the returned handle once.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .map_err(discovery_error)?;
    let process = unsafe { Owned::new(handle) };
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: the initialized writable FILETIMEs outlive this synchronous call.
    unsafe { GetProcessTimes(*process, &mut created, &mut exited, &mut kernel, &mut user) }
        .map_err(discovery_error)?;
    Ok((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

pub(super) fn same_session(pid: u32) -> Result<bool, AppError> {
    let mut owner_session = 0;
    let mut candidate_session = 0;
    // SAFETY: both pointers refer to initialized writable session identifiers.
    unsafe { ProcessIdToSessionId(std::process::id(), &mut owner_session) }
        .map_err(discovery_error)?;
    unsafe { ProcessIdToSessionId(pid, &mut candidate_session) }.map_err(discovery_error)?;
    Ok(owner_session == candidate_session)
}

fn discovery_error(error: windows::core::Error) -> AppError {
    AppError::DesktopDiscovery(format!("Consulta nativa Windows: {error}"))
}

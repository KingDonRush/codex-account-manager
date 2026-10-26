use std::collections::HashSet;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use windows::core::PWSTR;
use windows::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS, FILETIME, WIN32_ERROR};
use windows::Win32::System::RestartManager::{
    RmCancelCurrentTask, RmEndSession, RmGetList, RmRegisterResources, RmShutdown, RmStartSession,
    CCH_RM_SESSION_KEY, RM_PROCESS_INFO, RM_UNIQUE_PROCESS,
};

use super::process_identity as native;
use crate::error::AppError;

const REQUEST_DEADLINE: Duration = Duration::from_secs(10);
const MAX_TARGETS: usize = 128;

struct Session(u32);

impl Session {
    fn start() -> Result<Self, AppError> {
        let mut handle = 0;
        let mut key = [0u16; CCH_RM_SESSION_KEY as usize + 1];
        // SAFETY: the handle and bounded writable session key live through the
        // synchronous call. The key never leaves the backend or enters a log.
        check(unsafe { RmStartSession(&mut handle, None, PWSTR(key.as_mut_ptr())) })?;
        Ok(Self(handle))
    }

    fn register(&self, targets: &[RM_UNIQUE_PROCESS]) -> Result<(), AppError> {
        // SAFETY: only validated PID+creation-time pairs; no file/service
        // registration that could expand the affected application set.
        check(unsafe { RmRegisterResources(self.0, None, Some(targets), None) })
    }

    fn affected(&self) -> Result<Vec<RM_UNIQUE_PROCESS>, AppError> {
        let mut needed = 0;
        let mut count = MAX_TARGETS as u32;
        let mut reboot = 0;
        let mut information = vec![RM_PROCESS_INFO::default(); MAX_TARGETS];
        // SAFETY: the initialized array contains count entries and all output
        // pointers are valid until this bounded, synchronous call returns.
        let result = unsafe {
            RmGetList(
                self.0,
                &mut needed,
                &mut count,
                Some(information.as_mut_ptr()),
                &mut reboot,
            )
        };
        if result == ERROR_MORE_DATA || reboot != 0 {
            return Err(pending(
                "O Windows não confirmou uma saída limitada aos processos esperados.",
            ));
        }
        check(result)?;
        information.truncate(count as usize);
        Ok(information.into_iter().map(|item| item.Process).collect())
    }

    fn shutdown(&self) -> Result<(), AppError> {
        let (finished, receive) = mpsc::sync_channel::<()>(1);
        let handle = self.0;
        let deadline = thread::Builder::new()
            .name("desktop-quit-deadline".into())
            .spawn(move || {
                if receive.recv_timeout(REQUEST_DEADLINE).is_err() {
                    // SAFETY: Session outlives this joined watchdog. Cancellation
                    // addresses only its own request; it never kills the Desktop.
                    let _ = unsafe { RmCancelCurrentTask(handle) };
                }
            })?;
        // SAFETY: owned session and revalidated targets. Flags are deliberately
        // zero: never RmForceShutdown, reboot, logout, or automatic restart.
        let result = unsafe { RmShutdown(self.0, 0, None) };
        let _ = finished.send(());
        let _ = deadline.join();
        check(result)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: this is the sole owner; the cancellation watchdog was joined.
        let _ = unsafe { RmEndSession(self.0) };
    }
}

/// Requests application shutdown, not window closure. WM_CLOSE can merely hide
/// Codex to the tray, leaving its App Server and authentication alive on Windows.
pub(super) fn request_shutdown(targets: &[RM_UNIQUE_PROCESS]) -> Result<(), AppError> {
    if targets.is_empty() || targets.len() > MAX_TARGETS {
        return Err(pending(
            "A saída exige um conjunto limitado de processos identificados.",
        ));
    }
    let session = Session::start()?;
    session.register(targets)?;
    ensure_expected(targets, &session.affected()?)?;
    // The registry/list calls can take time; a recycled PID is not authorization
    // to close its replacement, even if it has the same executable filename.
    for identity in targets {
        let (_, ticks) = process_key(identity);
        let _ = target(identity.dwProcessId, Some(ticks))?;
    }
    session.shutdown()
}

pub(super) fn target(pid: u32, expected: Option<u64>) -> Result<RM_UNIQUE_PROCESS, AppError> {
    let ticks = expected.ok_or_else(|| pending("Identidade de criação indisponível."))?;
    if !native::same_session(pid)? || native::created_ticks(pid)? != ticks {
        return Err(pending(
            "O processo mudou antes da saída; nenhuma interrupção foi autorizada.",
        ));
    }
    Ok(RM_UNIQUE_PROCESS {
        dwProcessId: pid,
        ProcessStartTime: FILETIME {
            dwLowDateTime: ticks as u32,
            dwHighDateTime: (ticks >> 32) as u32,
        },
    })
}

fn ensure_expected(
    expected: &[RM_UNIQUE_PROCESS],
    actual: &[RM_UNIQUE_PROCESS],
) -> Result<(), AppError> {
    let expected = expected.iter().map(process_key).collect::<HashSet<_>>();
    if actual
        .iter()
        .map(process_key)
        .any(|identity| !expected.contains(&identity))
    {
        return Err(pending(
            "A consulta incluiu outro aplicativo; saída cancelada para preservá-lo.",
        ));
    }
    Ok(())
}

fn process_key(process: &RM_UNIQUE_PROCESS) -> (u32, u64) {
    (
        process.dwProcessId,
        (u64::from(process.ProcessStartTime.dwHighDateTime) << 32)
            | u64::from(process.ProcessStartTime.dwLowDateTime),
    )
}

fn check(result: WIN32_ERROR) -> Result<(), AppError> {
    if result == ERROR_SUCCESS {
        return Ok(());
    }
    Err(pending(&format!(
        "A saída normal do aplicativo não foi confirmada (Windows {}). A conta foi preservada.",
        result.0
    )))
}

fn pending(message: &str) -> AppError {
    AppError::DesktopClosePending(message.to_owned())
}

#[cfg(test)]
mod tests;

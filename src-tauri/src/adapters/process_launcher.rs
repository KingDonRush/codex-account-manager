use super::process_supervisor::{configure_supervised_process, ProcessSupervisor};
use crate::error::AppError;
use std::process::{Child, Command};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::sync::{mpsc, OnceLock};
use std::time::Duration;

struct Request {
    command: Command,
    response: mpsc::SyncSender<Result<PendingChild, AppError>>,
    cancelled: Arc<AtomicBool>,
}

struct PendingChild(Option<(Child, ProcessSupervisor)>);

impl Drop for PendingChild {
    fn drop(&mut self) {
        if let Some((child, supervisor)) = self.0.as_mut() {
            let _ = supervisor.terminate(child);
            let _ = child.try_wait();
        }
    }
}

static LAUNCHER: OnceLock<Result<mpsc::SyncSender<Request>, String>> = OnceLock::new();

/// Linux PDEATHSIG belongs to the spawning thread, not the entire parent
/// process. This one in-process launcher outlives login/IPC worker threads.
/// It has no listener, credentials, restart loop or independent daemon.
pub fn spawn_supervised(command: Command) -> Result<(Child, ProcessSupervisor), AppError> {
    let sender = LAUNCHER
        .get_or_init(start)
        .as_ref()
        .map_err(|message| AppError::Internal(message.clone()))?;
    let (response, result) = mpsc::sync_channel(1);
    let cancelled = Arc::new(AtomicBool::new(false));
    sender
        .try_send(Request {
            command,
            response,
            cancelled: cancelled.clone(),
        })
        .map_err(|_| {
            AppError::Busy("O lançador de processos está ocupado ou indisponível.".to_owned())
        })?;
    match result.recv_timeout(Duration::from_secs(3)) {
        Ok(result) => result.and_then(|mut owned| {
            owned
                .0
                .take()
                .ok_or_else(|| AppError::Internal("Processo sem proprietário.".to_owned()))
        }),
        Err(_) => {
            cancelled.store(true, Ordering::Release);
            Err(AppError::Runtime(
                "O processo não pôde ser iniciado no prazo.".to_owned(),
            ))
        }
    }
}

fn start() -> Result<mpsc::SyncSender<Request>, String> {
    let (sender, requests) = mpsc::sync_channel::<Request>(16);
    std::thread::Builder::new()
        .name("owned-process-launcher".to_owned())
        .spawn(move || {
            for mut request in requests {
                if request.cancelled.load(Ordering::Acquire) {
                    continue;
                }
                configure_supervised_process(&mut request.command);
                let pending = spawn_owned(&mut request.command);
                let _ = request.response.send(pending);
            }
        })
        .map_err(|error| error.to_string())?;
    Ok(sender)
}

fn spawn_owned(command: &mut Command) -> Result<PendingChild, AppError> {
    let mut child = command.spawn()?;
    match ProcessSupervisor::attach(&child) {
        Ok(supervisor) => Ok(PendingChild(Some((child, supervisor)))),
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(error)
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn browser_survives_the_operation_thread_but_remains_supervised() {
        let (mut child, supervisor) = std::thread::spawn(|| {
            let mut command = Command::new("sh");
            command.args(["-c", "sleep 30 & wait"]);
            spawn_supervised(command).expect("process")
        })
        .join()
        .expect("operation finished");
        std::thread::sleep(Duration::from_millis(60));
        let alive = child.try_wait().expect("state").is_none();
        supervisor.terminate(&mut child).expect("cleanup");
        let _ = child.wait();
        assert!(
            alive,
            "the worker's exit must not terminate a browser retained by the app"
        );
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use std::io::Read;
    use std::process::Stdio;

    fn run_probe(program: &std::path::Path, arguments: &[&str]) -> String {
        let mut command = Command::new(program);
        command
            .args(arguments)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let (mut child, supervisor) = spawn_supervised(command).expect("owned child");
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(status) = child.try_wait().expect("status") {
                assert!(status.success(), "probe failed");
                break;
            }
            if std::time::Instant::now() >= deadline {
                supervisor.terminate(&mut child).expect("cleanup");
                let _ = child.wait();
                panic!("probe timed out");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let mut output = String::new();
        child
            .stdout
            .take()
            .expect("stdout")
            .read_to_string(&mut output)
            .expect("read");
        output.trim().to_owned()
    }

    const CONSOLE_PROBE: &str = "Add-Type -TypeDefinition 'using System; using System.Runtime.InteropServices; public static class CamProbe { [DllImport(\"kernel32.dll\")] public static extern IntPtr GetConsoleWindow(); }'; [CamProbe]::GetConsoleWindow().ToInt64()";

    #[test]
    fn native_auxiliary_has_no_console_window() {
        let output = run_probe(
            std::path::Path::new("powershell.exe"),
            &["-NoProfile", "-NonInteractive", "-Command", CONSOLE_PROBE],
        );
        assert_eq!(output, "0");
    }

    #[test]
    fn a_batch_wrapper_in_a_spaced_path_also_has_no_console_window() {
        let fixture = tempfile::Builder::new()
            .prefix("cam wrapper ")
            .tempdir()
            .expect("fixture");
        let wrapper = fixture.path().join("codex.cmd");
        std::fs::write(
            &wrapper,
            format!(
                "@echo off\r\npowershell.exe -NoProfile -NonInteractive -Command \"{}\"\r\n",
                CONSOLE_PROBE.replace('"', "\\\"")
            ),
        )
        .expect("wrapper");
        assert_eq!(run_probe(&wrapper, &[]), "0");
    }

    #[test]
    fn the_child_is_in_its_job_before_its_first_instruction() {
        let script = "Add-Type -TypeDefinition 'using System; using System.Runtime.InteropServices; public static class CamJobProbe { [DllImport(\"kernel32.dll\")] public static extern IntPtr GetCurrentProcess(); [DllImport(\"kernel32.dll\")] public static extern bool IsProcessInJob(IntPtr p, IntPtr job, out bool contained); }'; $contained=$false; if(-not [CamJobProbe]::IsProcessInJob([CamJobProbe]::GetCurrentProcess(),[IntPtr]::Zero,[ref]$contained)){exit 2}; $contained";
        assert_eq!(
            run_probe(
                std::path::Path::new("powershell.exe"),
                &["-NoProfile", "-NonInteractive", "-Command", script]
            ),
            "True"
        );
    }
}

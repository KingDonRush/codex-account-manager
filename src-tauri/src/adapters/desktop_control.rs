use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::error::AppError;
use crate::ports::DesktopLifecycle;

mod installation;
mod processes;
#[cfg(windows)]
pub(crate) use processes::known_desktop_process_ids;
use processes::{wait_until_stopped, DesktopProcess};
#[cfg(windows)]
pub(crate) mod process_identity;
#[cfg(windows)]
mod restart_manager;
#[cfg(windows)]
mod windows;
use installation::DesktopInstallation;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopLaunchOutcome {
    NotRequired,
    Observed { pid: u32 },
}

impl DesktopLaunchOutcome {
    pub fn was_observed(self) -> bool {
        matches!(self, Self::Observed { .. })
    }
}

#[derive(Debug, Default)]
pub struct DesktopState {
    installation: Option<DesktopInstallation>,
    was_running: bool,
}

impl DesktopState {
    pub(super) fn request_open(&mut self) {
        // "Use this account" is an explicit activation even if the Desktop was
        // already closed, including after a user-authorized interruption.
        self.was_running |= self.installation.is_some();
    }
}

#[derive(Default)]
pub struct DesktopController {
    database: Option<super::database::Database>,
}

const GRACEFUL_CLOSE_TIMEOUT: Duration = Duration::from_secs(3);
#[cfg(not(windows))]
const FORCE_CLOSE_TIMEOUT: Duration = Duration::from_secs(3);

fn matching_installation(
    installation: Option<&DesktopInstallation>,
) -> Result<Vec<DesktopProcess>, AppError> {
    #[cfg(windows)]
    let allow_store_family = installation.is_some_and(|item| item.app_user_model_id.is_some());
    #[cfg(not(windows))]
    let allow_store_family = false;
    processes::matching_processes(
        installation.map(|item| item.executable.as_path()),
        allow_store_family,
    )
}

impl DesktopLifecycle for DesktopController {
    fn stop(&self) -> Result<DesktopState, AppError> {
        let mut trace = |_: &'static str, _: Duration| {};
        self.stop_with_trace(&mut trace)
    }

    fn stop_with_trace(
        &self,
        trace: &mut dyn FnMut(&'static str, Duration),
    ) -> Result<DesktopState, AppError> {
        let started = Instant::now();
        let installation = self.discover_installation();
        trace("desktop_installation_discovery", started.elapsed());
        Self::stop_with_trace(trace, installation?)
    }

    fn restart(&self, state: &DesktopState) -> Result<DesktopLaunchOutcome, AppError> {
        Self::restart(state)
    }

    fn open_or_focus(&self) -> Result<(), AppError> {
        DesktopController::open_or_focus(self)
    }

    fn select_executable(&self, owner: isize) -> Result<bool, AppError> {
        #[cfg(windows)]
        {
            let Some(path) = windows::pick_executable(owner)? else {
                return Ok(false);
            };
            let installation = installation::configured(path)?;
            let database = self.database.as_ref().ok_or_else(|| {
                AppError::DesktopDiscovery("Preferencia de Desktop indisponivel.".to_owned())
            })?;
            database.save_desktop_executable(&installation.executable)?;
            Ok(true)
        }
        #[cfg(not(windows))]
        {
            let _ = owner;
            Err(AppError::DesktopDiscovery(
                "A selecao nativa deste reparo pertence ao Windows.".to_owned(),
            ))
        }
    }

    fn confirm_close(&self, owner: isize) -> Result<bool, AppError> {
        #[cfg(windows)]
        {
            let installation = self.discover_installation()?;
            let processes = matching_installation(installation.as_ref())?;
            if processes.is_empty() {
                return Ok(true);
            }
            if processes.iter().any(|root| !root.identity_is_known()) {
                return Err(AppError::DesktopDiscovery("A identidade de parte dos processos não foi confirmada; nenhuma interrupção foi realizada.".to_owned()));
            }
            if !windows::confirm_interruption(owner)? {
                return Ok(false);
            }
            let current = matching_installation(installation.as_ref())?;
            let expected = processes
                .iter()
                .flat_map(|root| root.scope())
                .collect::<std::collections::HashSet<_>>();
            let observed = current
                .iter()
                .flat_map(|root| root.scope())
                .collect::<std::collections::HashSet<_>>();
            if expected != observed {
                return Err(AppError::DesktopClosePending("Os processos do Codex mudaram durante a confirmação. Nenhuma interrupção foi realizada; confira novamente.".to_owned()));
            }
            let mut closed = true;
            // Every authorized root must be attempted. Iterator::all would
            // short-circuit after a failure and leave the other roots untouched.
            for root in &current {
                closed &= root.force_terminate();
            }
            if !closed {
                return Err(AppError::DesktopClosePending("Não consegui encerrar todos os processos autorizados; nenhuma conta foi trocada.".to_owned()));
            }
            wait_until_stopped(&current, Duration::from_secs(3))?;
            Ok(true)
        }
        #[cfg(not(windows))]
        {
            let _ = owner;
            Err(AppError::DesktopClosePending(
                "Este atalho de interrupção é específico do Windows.".to_owned(),
            ))
        }
    }
}

impl DesktopController {
    pub fn with_database(database: super::database::Database) -> Self {
        Self {
            database: Some(database),
        }
    }

    fn discover_installation(&self) -> Result<Option<DesktopInstallation>, AppError> {
        let selection = self
            .database
            .as_ref()
            .map(|database| database.desktop_executable())
            .transpose()?
            .flatten();
        installation::discover_selected(selection)
    }

    /// Requests the OS launcher to open/raise Codex; never changes authentication.
    pub fn open_or_focus(&self) -> Result<(), AppError> {
        let installation = self.discover_installation()?.ok_or_else(|| {
            AppError::DesktopDiscovery(
                "Não foi possível localizar o Codex instalado; configure seu executável."
                    .to_owned(),
            )
        })?;
        Self::restart(&DesktopState {
            installation: Some(installation),
            was_running: true,
        })
        .map(|_| ())
    }
    fn stop_with_trace(
        trace: &mut dyn FnMut(&'static str, Duration),
        installation: Option<DesktopInstallation>,
    ) -> Result<DesktopState, AppError> {
        let started = Instant::now();
        let processes = matching_installation(installation.as_ref())?;
        trace("desktop_process_discovery", started.elapsed());
        if let Some(item) = &installation {
            trace(
                match item.source {
                    "store" => "desktop_discovered_store",
                    "configured" => "desktop_discovered_configured",
                    _ => "desktop_discovered_system",
                },
                Duration::ZERO,
            );
        }
        if processes.is_empty() {
            return Ok(DesktopState {
                installation,
                was_running: false,
            });
        }
        // Do not use `Iterator::all` here: it short-circuits on the first
        // failure and would leave the remaining desktop processes alive.
        let started = Instant::now();
        #[cfg(windows)]
        let request = processes
            .iter()
            .map(|process| {
                restart_manager::target(
                    process.identity.pid.as_u32(),
                    process.identity.started_ticks,
                )
            })
            .collect::<Result<Vec<_>, _>>()
            .and_then(|targets| restart_manager::request_shutdown(&targets));
        #[cfg(not(windows))]
        let _graceful_signal_sent = processes
            .iter()
            .fold(true, |all_sent, process| process.terminate() && all_sent);
        trace("desktop_graceful_signal", started.elapsed());
        #[cfg(windows)]
        request?;
        let started = Instant::now();
        let graceful_wait = wait_until_stopped(&processes, GRACEFUL_CLOSE_TIMEOUT);
        trace("desktop_graceful_wait", started.elapsed());
        if graceful_wait.is_ok() {
            return Ok(DesktopState {
                installation,
                was_running: true,
            });
        }

        #[cfg(windows)]
        return Err(AppError::DesktopClosePending(
            "O Codex não concluiu o fechamento normal. A conta não foi alterada; aguarde o trabalho terminar ou feche a janela para tentar novamente.".to_owned(),
        ));

        #[cfg(not(windows))]
        {
            // A desktop client can ignore or delay SIGTERM while its renderer is
            // unhealthy. The account switch has already reserved its journal, so
            // a bounded force-close is safer than leaving the operation blocked
            // for twenty seconds and reporting a misleading generic failure.
            let started = Instant::now();
            let force_signal_sent = processes.iter().fold(true, |all_sent, process| {
                process.force_terminate() && all_sent
            });
            trace("desktop_force_signal", started.elapsed());
            let started = Instant::now();
            let force_wait = wait_until_stopped(&processes, FORCE_CLOSE_TIMEOUT);
            trace("desktop_force_wait", started.elapsed());
            if force_signal_sent && force_wait.is_ok() {
                return Ok(DesktopState {
                    installation,
                    was_running: true,
                });
            }
            Err(AppError::DesktopClosePending(
                "o Codex não encerrou com segurança; nenhuma conta foi trocada".to_owned(),
            ))
        }
    }

    pub fn restart(state: &DesktopState) -> Result<DesktopLaunchOutcome, AppError> {
        if !state.was_running {
            return Ok(DesktopLaunchOutcome::NotRequired);
        }
        let installation = state.installation.as_ref().ok_or_else(|| {
            AppError::DesktopLaunch(
                "o executável do Codex não pôde ser recuperado para reinício".to_owned(),
            )
        })?;
        #[cfg(windows)]
        if let Some(id) = &installation.app_user_model_id {
            let _ = windows::activate(id)?;
            return wait_until_opened(installation);
        }
        let executable = installation.executable.as_path();
        let override_launcher = std::env::var_os("CODEX_DESKTOP_LAUNCHER")
            .map(PathBuf::from)
            .filter(|path| path.is_file());
        let gtk_launcher = if cfg!(target_os = "linux") {
            which::which("gtk-launch").ok()
        } else {
            None
        };
        let mut command = desktop_launch_command(
            executable,
            override_launcher.as_deref(),
            gtk_launcher.as_deref(),
        );
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_detached(&mut command);
        launch_and_reap_desktop(&mut command)?;
        wait_until_opened(installation)
    }
}

fn wait_until_opened(installation: &DesktopInstallation) -> Result<DesktopLaunchOutcome, AppError> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let processes = matching_installation(Some(installation))?;
        if let Some(process) = processes
            .iter()
            .find(|process| desktop_window_ready(process))
        {
            return Ok(DesktopLaunchOutcome::Observed {
                pid: process.identity.pid.as_u32(),
            });
        }
        if Instant::now() >= deadline {
            return Err(AppError::DesktopLaunch("O pedido de abertura foi enviado, mas o Desktop não apareceu no prazo. A sessão ativada foi preservada.".to_owned()));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn desktop_window_ready(process: &DesktopProcess) -> bool {
    #[cfg(windows)]
    {
        windows::has_window(process.identity.pid.as_u32())
    }
    #[cfg(not(windows))]
    {
        let _ = process;
        true
    }
}

fn launch_and_reap_desktop(command: &mut Command) -> Result<(), AppError> {
    let (sender, receiver) = std::sync::mpsc::sync_channel::<std::process::Child>(1);
    // Child::drop does not reap. Start its owner before creating the process,
    // so failure to start a reaper cannot leave a detached child behind.
    thread::Builder::new()
        .name("codex-desktop-reaper".to_owned())
        .spawn(move || {
            if let Ok(mut child) = receiver.recv() {
                let _ = child.wait();
            }
        })?;
    let child = command.spawn().map_err(|error| {
        AppError::Internal(format!("não foi possível reiniciar o Codex: {error}"))
    })?;
    if let Err(error) = sender.send(child) {
        let mut child = error.0;
        let _ = child.kill();
        let _ = child.wait();
        return Err(AppError::Internal(
            "o supervisor de reinício não está disponível".to_owned(),
        ));
    }
    Ok(())
}

fn desktop_launch_command(
    executable: &Path,
    override_launcher: Option<&Path>,
    gtk_launcher: Option<&Path>,
) -> Command {
    if let Some(launcher) = override_launcher {
        return Command::new(launcher);
    }
    if executable == Path::new("/usr/lib/chatgpt/ChatGPT") {
        if let Some(launcher) = gtk_launcher {
            // Respect the user's desktop entry, including its startup fixes.
            // Executable identity for termination remains the actual binary.
            let mut command = Command::new(launcher);
            command.arg("chatgpt.desktop");
            return command;
        }
    }
    Command::new(executable)
}

#[cfg(unix)]
fn configure_detached(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(windows)]
fn configure_detached(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
}

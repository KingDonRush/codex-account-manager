#[cfg(windows)]
use super::{installation, windows};
use crate::error::AppError;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};
#[cfg(not(windows))]
use sysinfo::Signal;
use sysinfo::{ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, System};

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub(super) struct ProcessIdentity {
    pub(super) pid: sysinfo::Pid,
    pub(super) started_at: u64,
    #[cfg(target_os = "linux")]
    pub(super) started_ticks: Option<u64>,
    #[cfg(windows)]
    pub(super) started_ticks: Option<u64>,
}

impl ProcessIdentity {
    fn new(pid: sysinfo::Pid, process: &sysinfo::Process) -> Self {
        Self {
            pid,
            started_at: process.start_time(),
            #[cfg(target_os = "linux")]
            started_ticks: crate::adapters::process_supervisor::linux_started_ticks(
                pid.as_u32() as i32
            )
            .ok(),
            #[cfg(windows)]
            started_ticks: windows::created_ticks(pid.as_u32()).ok(),
        }
    }

    fn matches(self, process: &sysinfo::Process) -> Result<bool, AppError> {
        if self.started_at != process.start_time() {
            return Ok(false);
        }
        #[cfg(target_os = "linux")]
        {
            let expected = self.started_ticks.ok_or_else(|| {
                AppError::DesktopDiscovery(
                    "Identidade do processo indisponível; sessão preservada.".to_owned(),
                )
            })?;
            Ok(expected
                == crate::adapters::process_supervisor::linux_started_ticks(
                    self.pid.as_u32() as i32
                )?)
        }
        #[cfg(windows)]
        {
            let expected = self.started_ticks.ok_or_else(|| {
                AppError::DesktopDiscovery(
                    "Identidade do processo indisponível; sessão preservada.".to_owned(),
                )
            })?;
            Ok(expected == windows::created_ticks(self.pid.as_u32())?)
        }
        #[cfg(not(any(target_os = "linux", windows)))]
        {
            Ok(true)
        }
    }
}

#[derive(Clone)]
pub(super) struct DesktopProcess {
    pub(super) identity: ProcessIdentity,
    descendants: Vec<ProcessIdentity>,
}

impl DesktopProcess {
    #[cfg(not(windows))]
    pub(super) fn terminate(&self) -> bool {
        // Refresh only this PID instead of scanning the whole process table;
        // this keeps the close path bounded when the machine has many
        // processes. The refresh is still required because `System::new()`
        // starts without process entries.
        let mut system = System::new();
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(std::slice::from_ref(&self.identity.pid)),
            true,
            ProcessRefreshKind::nothing(),
        );
        let Some(process) = system.process(self.identity.pid) else {
            return true;
        };
        match self.identity.matches(process) {
            Ok(false) => return true,
            Err(_) => return false,
            Ok(true) => {}
        }
        process
            .kill_with(Signal::Term)
            .unwrap_or_else(|| process.kill())
    }

    pub(super) fn force_terminate(&self) -> bool {
        let mut system = System::new();
        let mut all_succeeded = true;
        let mut identities = self.descendants.clone();
        identities.push(self.identity);
        let mut seen = HashSet::new();
        identities.retain(|identity| seen.insert(*identity));
        let pids = identities
            .iter()
            .map(|identity| identity.pid)
            .collect::<Vec<_>>();
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&pids),
            true,
            ProcessRefreshKind::nothing(),
        );
        for identity in identities {
            #[cfg(windows)]
            {
                all_succeeded &=
                    windows::terminate_verified(identity.pid.as_u32(), identity.started_ticks);
            }
            #[cfg(not(windows))]
            {
                let succeeded = system.process(identity.pid).is_none_or(|process| {
                    match identity.matches(process) {
                        Ok(false) => true,
                        Ok(true) => process.kill(),
                        Err(_) => false,
                    }
                });
                all_succeeded &= succeeded;
            }
        }
        all_succeeded
    }

    #[cfg(windows)]
    pub(super) fn scope(&self) -> Vec<ProcessIdentity> {
        let mut identities = self.descendants.clone();
        identities.push(self.identity);
        identities.sort_by_key(|identity| identity.pid.as_u32());
        identities
    }

    #[cfg(windows)]
    pub(super) fn identity_is_known(&self) -> bool {
        self.identity.started_ticks.is_some()
            && self
                .descendants
                .iter()
                .all(|identity| identity.started_ticks.is_some())
    }
}

pub(super) fn matching_processes(
    configured: Option<&Path>,
    allow_store_family: bool,
) -> Result<Vec<DesktopProcess>, AppError> {
    #[cfg(not(windows))]
    let _ = allow_store_family;
    let configured_keys = configured_match_keys(configured);
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_exe(sysinfo::UpdateKind::Always),
    );
    let mut candidate_pids = Vec::new();
    for (pid, process) in system.processes() {
        let Some(executable) = process.exe() else {
            #[cfg(windows)]
            if matches!(process.name().to_str(), Some("ChatGPT.exe" | "Codex.exe"))
                && windows::same_session(pid.as_u32())?
            {
                return Err(AppError::DesktopDiscovery("Há um Desktop cuja identidade não pôde ser consultada; troca bloqueada para preservar a sessão.".to_owned()));
            }
            continue;
        };
        let exact_match = is_codex_desktop(executable, &configured_keys);
        #[cfg(windows)]
        let exact_match = exact_match
            || (allow_store_family
                && configured.and_then(Path::file_name) == executable.file_name()
                && windows::belongs_to_codex_package(pid.as_u32())?);
        #[cfg(windows)]
        if exact_match && !windows::same_session(pid.as_u32())? {
            continue;
        }
        if exact_match {
            candidate_pids.push(*pid);
        }
    }
    if candidate_pids.is_empty() {
        return Ok(Vec::new());
    }
    let children = process_children(system.processes());
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&candidate_pids),
        true,
        ProcessRefreshKind::nothing().with_cmd(sysinfo::UpdateKind::Always),
    );
    let candidate_set = candidate_pids.iter().copied().collect::<HashSet<_>>();
    let roots = candidate_pids
        .into_iter()
        .filter_map(|pid| {
            let process = system.process(pid)?;
            if is_helper(process.cmd())
                || process
                    .parent()
                    .is_some_and(|parent| candidate_set.contains(&parent))
            {
                return None;
            }
            Some((
                ProcessIdentity::new(pid, process),
                process.exe()?.to_path_buf(),
            ))
        })
        .collect::<Vec<_>>();
    Ok(roots
        .into_iter()
        .map(|(identity, _)| DesktopProcess {
            identity,
            descendants: descendants_of(identity.pid, &children)
                .into_iter()
                .filter_map(|pid| {
                    system
                        .process(pid)
                        .map(|process| ProcessIdentity::new(pid, process))
                })
                .collect(),
        })
        .collect())
}

#[cfg(windows)]
pub(crate) fn known_desktop_process_ids(
    selection: Option<std::path::PathBuf>,
) -> Result<HashSet<u32>, AppError> {
    let installation = installation::discover_selected(selection)?;
    let processes = matching_processes(
        installation.as_ref().map(|item| item.executable.as_path()),
        installation
            .as_ref()
            .is_some_and(|item| item.app_user_model_id.is_some()),
    )?;
    Ok(processes
        .iter()
        .flat_map(|root| {
            root.descendants
                .iter()
                .chain(std::iter::once(&root.identity))
        })
        .map(|identity| identity.pid.as_u32())
        .collect())
}

fn process_children(
    processes: &HashMap<sysinfo::Pid, sysinfo::Process>,
) -> HashMap<sysinfo::Pid, Vec<sysinfo::Pid>> {
    let mut children: HashMap<sysinfo::Pid, Vec<sysinfo::Pid>> = HashMap::new();
    for (pid, process) in processes {
        if let Some(parent) = process.parent() {
            children.entry(parent).or_default().push(*pid);
        }
    }
    children
}

fn descendants_of(
    root: sysinfo::Pid,
    children: &HashMap<sysinfo::Pid, Vec<sysinfo::Pid>>,
) -> Vec<sysinfo::Pid> {
    let mut descendants = Vec::new();
    let mut pending = vec![root];
    let mut seen = HashSet::from([root]);
    while let Some(parent) = pending.pop() {
        if let Some(children) = children.get(&parent) {
            for child in children {
                if seen.insert(*child) {
                    descendants.push(*child);
                    pending.push(*child);
                }
            }
        }
    }
    descendants
}

fn configured_match_keys(configured: Option<&Path>) -> HashSet<String> {
    let Some(configured) = configured else {
        return HashSet::new();
    };
    let mut keys = HashSet::from([executable_key(configured)]);
    if let Ok(canonical) = configured.canonicalize() {
        keys.insert(executable_key(&canonical));
    }
    keys
}

fn is_codex_desktop(executable: &Path, configured_keys: &HashSet<String>) -> bool {
    let normalized = executable_key(executable);
    configured_keys.contains(&normalized)
}

fn executable_key(executable: &Path) -> String {
    let path = executable.to_string_lossy();
    #[cfg(windows)]
    {
        windows_path_key(&path)
    }
    #[cfg(not(windows))]
    {
        path.into_owned()
    }
}

#[cfg(any(windows, test))]
pub(super) fn windows_path_key(path: &str) -> String {
    let path = path.replace('\\', "/").to_ascii_lowercase();
    if let Some(path) = path.strip_prefix("//?/unc/") {
        return format!("//{path}");
    }
    path.strip_prefix("//?/").unwrap_or(&path).to_owned()
}

fn is_helper(arguments: &[std::ffi::OsString]) -> bool {
    arguments
        .iter()
        .skip(1)
        .any(|argument| argument.to_string_lossy().starts_with("--type="))
}

pub(super) fn wait_until_stopped(
    processes: &[DesktopProcess],
    timeout: Duration,
) -> Result<(), AppError> {
    let mut identities = processes
        .iter()
        .flat_map(|process| {
            process
                .descendants
                .iter()
                .copied()
                .chain(std::iter::once(process.identity))
        })
        .collect::<Vec<_>>();
    identities.sort_unstable_by_key(|identity| (identity.pid.as_u32(), identity.started_at));
    identities.dedup();
    let mut pids = identities
        .iter()
        .map(|identity| identity.pid)
        .collect::<Vec<_>>();
    pids.sort_unstable();
    pids.dedup();
    let deadline = Instant::now() + timeout;
    let mut system = System::new();
    while Instant::now() < deadline {
        system.refresh_processes(ProcessesToUpdate::Some(&pids), true);
        if identities.iter().all(|identity| {
            system.process(identity.pid).is_none_or(|process| {
                identity.matches(process).is_ok_and(|same| !same)
                    || matches!(process.status(), ProcessStatus::Zombie)
            })
        }) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(200));
    }
    Err(AppError::Internal(
        "o Codex não encerrou com segurança; nenhuma conta foi trocada".to_owned(),
    ))
}

#[cfg(all(test, unix))]
mod tests {
    use std::process::Command;
    use std::time::{Duration, Instant};

    use super::{matching_processes, wait_until_stopped, DesktopProcess, ProcessIdentity};

    #[test]
    fn a_skipped_restart_is_not_an_observed_launch() {
        let outcome =
            super::super::DesktopController::restart(&super::super::DesktopState::default())
                .expect("not required");
        assert!(!outcome.was_observed());
    }

    #[test]
    fn windows_canonical_and_process_paths_keep_the_same_identity() {
        assert_eq!(
            super::windows_path_key(r"\\?\C:\Program Files\WindowsApps\Codex\app\ChatGPT.exe"),
            super::windows_path_key(r"C:\Program Files\WindowsApps\Codex\app\ChatGPT.exe")
        );
        assert_eq!(
            super::windows_path_key(r"\\?\UNC\server\app\ChatGPT.exe"),
            super::windows_path_key(r"\\server\app\ChatGPT.exe")
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn unknown_process_identity_does_not_count_as_confirmed_exit() {
        let mut child = Command::new("sleep").arg("30").spawn().expect("fixture");
        let mut identity = process_identity(child.id());
        identity.started_ticks = None;
        let process = DesktopProcess {
            identity,
            descendants: Vec::new(),
        };
        assert!(!process.terminate());
        assert!(
            wait_until_stopped(std::slice::from_ref(&process), Duration::from_millis(20)).is_err()
        );
        assert!(child.try_wait().expect("alive").is_none());
        child.kill().expect("cleanup");
        let _ = child.wait();
    }

    #[test]
    fn targeted_command_refresh_keeps_the_configured_candidate() {
        let executable = std::env::current_exe().expect("test executable");
        let current_pid = sysinfo::get_current_pid().expect("test pid");

        let matches = matching_processes(Some(&executable), false).expect("discovery");

        assert!(matches
            .iter()
            .any(|process| process.identity.pid == current_pid));
    }

    #[test]
    fn configured_desktop_does_not_match_another_interface_with_the_same_filename() {
        let configured = std::path::Path::new("/fixture/original/ChatGPT");
        let keys = super::configured_match_keys(Some(configured));
        assert!(super::is_codex_desktop(configured, &keys));
        assert!(!super::is_codex_desktop(
            std::path::Path::new("/fixture/other/ChatGPT"),
            &keys
        ));
    }

    #[test]
    fn restart_respects_the_desktop_entry_and_an_explicit_launcher() {
        let executable = std::path::Path::new("/usr/lib/chatgpt/ChatGPT");
        let gtk = std::path::Path::new("/fixture/gtk-launch");
        let command = super::super::desktop_launch_command(executable, None, Some(gtk));
        assert_eq!(command.get_program(), gtk);
        assert_eq!(
            command.get_args().next(),
            Some(std::ffi::OsStr::new("chatgpt.desktop"))
        );
        let custom = std::path::Path::new("/fixture/user-launcher");
        assert_eq!(
            super::super::desktop_launch_command(executable, Some(custom), Some(gtk)).get_program(),
            custom
        );
    }

    #[test]
    fn terminate_sends_a_signal_to_an_existing_process() {
        let mut child = Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("test process");
        let process = DesktopProcess {
            identity: process_identity(child.id()),
            descendants: Vec::new(),
        };

        assert!(process.terminate());
        let status = child.wait().expect("wait for test process");
        assert!(!status.success());
    }

    #[test]
    fn force_terminate_removes_a_stubborn_process_tree() {
        let mut child = Command::new("sh")
            .args(["-c", "trap '' TERM; sleep 30 & wait"])
            .spawn()
            .expect("stubborn process tree");
        let descendant = wait_for_descendant(child.id());
        let process = DesktopProcess {
            identity: process_identity(child.id()),
            descendants: vec![process_identity(descendant)],
        };

        assert!(process.terminate());
        std::thread::sleep(Duration::from_millis(50));
        assert!(child.try_wait().expect("parent status").is_none());
        assert!(process.force_terminate());
        wait_until_stopped(std::slice::from_ref(&process), Duration::from_secs(1))
            .expect("process tree stopped");
        let _ = child.wait();
    }

    #[test]
    fn stale_process_identity_never_terminates_a_reused_pid() {
        let mut child = Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("test process");
        let current = process_identity(child.id());
        let process = DesktopProcess {
            identity: ProcessIdentity {
                pid: current.pid,
                started_at: current.started_at.wrapping_add(1),
                #[cfg(target_os = "linux")]
                started_ticks: current.started_ticks,
            },
            descendants: Vec::new(),
        };

        assert!(process.terminate());
        assert!(child.try_wait().expect("child status").is_none());
        child.kill().expect("cleanup test process");
        let _ = child.wait();
    }

    fn process_identity(pid: u32) -> ProcessIdentity {
        let pid = sysinfo::Pid::from_u32(pid);
        let mut system = sysinfo::System::new();
        system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
        let process = system.process(pid).expect("test process identity");
        ProcessIdentity::new(pid, process)
    }

    fn wait_for_descendant(parent: u32) -> u32 {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let path = format!("/proc/{parent}/task/{parent}/children");
            if let Ok(content) = std::fs::read_to_string(path) {
                if let Some(pid) = content
                    .split_whitespace()
                    .next()
                    .and_then(|value| value.parse().ok())
                {
                    return pid;
                }
            }
            assert!(Instant::now() < deadline, "descendant did not start");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

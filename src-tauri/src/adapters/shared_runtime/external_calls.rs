use crate::error::AppError;
#[cfg(not(windows))]
use std::path::Path;
#[cfg(any(target_os = "linux", test))]
use std::path::PathBuf;

/// Read-only safeguard for uncoordinated CLI/App Server. Never signals a foreign
/// process. The cooperative file lease remains the race-free integration path.
#[cfg(not(windows))]
pub(super) fn active_calls(home: &Path) -> Result<Vec<u32>, AppError> {
    #[cfg(target_os = "linux")]
    {
        active_linux_calls(home)
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = home;
        Ok(Vec::new())
    }
}

#[cfg(windows)]
pub(super) fn active_windows_calls(
    selection: Option<std::path::PathBuf>,
) -> Result<Vec<u32>, AppError> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_exe(UpdateKind::Always),
    );
    let desktop = crate::adapters::desktop_control::known_desktop_process_ids(selection)?;
    let owner = sysinfo::get_current_pid().map_err(|error| AppError::Runtime(error.to_string()))?;
    let mut calls = Vec::new();
    for (pid, process) in system.processes() {
        if !matches!(process.name().to_str(), Some("codex.exe" | "codex")) {
            continue;
        }
        if desktop.contains(&pid.as_u32()) || belongs_to_owner(*pid, owner, &system) {
            continue;
        }
        // Auth homes of foreign processes cannot be established safely from
        // their environment on Windows. Unknown is busy, never absence.
        calls.push(pid.as_u32());
        if calls.len() > 128 {
            return Err(AppError::RuntimeBusy(
                "Muitos consumidores externos; sessão preservada.".to_owned(),
            ));
        }
    }
    Ok(calls)
}

#[cfg(windows)]
fn belongs_to_owner(mut pid: sysinfo::Pid, owner: sysinfo::Pid, system: &sysinfo::System) -> bool {
    for _ in 0..32 {
        if pid == owner {
            return true;
        }
        let Some(parent) = system.process(pid).and_then(|process| process.parent()) else {
            return false;
        };
        if parent == pid {
            return false;
        }
        pid = parent;
    }
    false
}

#[cfg(target_os = "linux")]
fn active_linux_calls(home: &Path) -> Result<Vec<u32>, AppError> {
    use std::os::unix::fs::MetadataExt;
    let uid = std::fs::metadata("/proc/self")?.uid();
    let home = home.canonicalize()?;
    let mut calls = Vec::new();
    for entry in std::fs::read_dir("/proc")?.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        if entry
            .metadata()
            .ok()
            .is_none_or(|metadata| metadata.uid() != uid)
        {
            continue;
        }
        if is_shared_consumer(pid, &home)? {
            calls.push(pid);
        }
        if calls.len() > 128 {
            return Err(AppError::Runtime(
                "Muitas chamadas externas; troca bloqueada para preservar as sessões.".to_owned(),
            ));
        }
    }
    Ok(calls)
}

#[cfg(target_os = "linux")]
fn is_shared_consumer(pid: u32, home: &Path) -> Result<bool, AppError> {
    let root = PathBuf::from(format!("/proc/{pid}"));
    let Ok(exe) = std::fs::read_link(root.join("exe")) else {
        return Ok(false);
    };
    if exe.file_name().and_then(|name| name.to_str()) != Some("codex") {
        return Ok(false);
    }
    if super::discovery::running_ticks(pid)?.is_none() {
        return Ok(false);
    }
    let arguments = read_bounded(&root.join("cmdline"))?;
    if !consumer_needs_lease(&arguments) {
        return Ok(false);
    }
    if belongs_to_desktop(pid)? {
        return Ok(false);
    }
    let environment = match read_bounded(&root.join("environ")) {
        Ok(environment) => environment,
        Err(_) => return Ok(true), // Known CLI, unknown auth scope: do not switch beneath it.
    };
    let configured = environment_home(&environment);
    Ok(configured
        .and_then(|path| path.canonicalize().ok())
        .is_none_or(|path| path == home))
}

#[cfg(target_os = "linux")]
fn consumer_needs_lease(arguments: &[u8]) -> bool {
    let parts = arguments
        .split(|byte| *byte == 0)
        .skip(1)
        .collect::<Vec<_>>();
    if parts.contains(&b"--managed-daemon".as_slice()) {
        return false;
    }
    parts.contains(&b"exec".as_slice())
        || parts.contains(&b"app-server".as_slice()) && !parts.contains(&b"daemon".as_slice())
}

#[cfg(target_os = "linux")]
fn belongs_to_desktop(mut pid: u32) -> Result<bool, AppError> {
    let desktop = std::env::var_os("CODEX_DESKTOP_EXECUTABLE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/usr/lib/chatgpt/ChatGPT"));
    let desktop = desktop.canonicalize().unwrap_or(desktop);
    for _ in 0..32 {
        let root = PathBuf::from(format!("/proc/{pid}"));
        if std::fs::read_link(root.join("exe")).ok().as_ref() == Some(&desktop) {
            return Ok(true);
        }
        let stat = match std::fs::read_to_string(root.join("stat")) {
            Ok(stat) => stat,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        let parent = stat
            .rsplit_once(") ")
            .and_then(|(_, fields)| fields.split_whitespace().nth(1))
            .and_then(|value| value.parse().ok());
        match parent {
            Some(parent) if parent > 1 && parent != pid => pid = parent,
            _ => return Ok(false),
        }
    }
    Ok(false)
}

#[cfg(target_os = "linux")]
fn read_bounded(path: &Path) -> Result<Vec<u8>, AppError> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(65537)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 65536 {
        return Err(AppError::Runtime(
            "Escopo da chamada externa desconhecido.".to_owned(),
        ));
    }
    Ok(bytes)
}

#[cfg(any(target_os = "linux", test))]
fn environment_home(environment: &[u8]) -> Option<PathBuf> {
    let value = |name: &[u8]| {
        environment
            .split(|byte| *byte == 0)
            .find_map(|item| item.strip_prefix(name))
    };
    if let Some(home) = value(b"CODEX_HOME=") {
        return Some(PathBuf::from(std::str::from_utf8(home).ok()?));
    }
    value(b"HOME=")
        .and_then(|home| std::str::from_utf8(home).ok())
        .map(|home| PathBuf::from(home).join(".codex"))
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    #[test]
    fn external_stdio_servers_need_a_lease_but_managed_daemon_is_inspected_by_rpc() {
        assert!(super::consumer_needs_lease(b"codex\0exec\0--ephemeral\0"));
        assert!(super::consumer_needs_lease(
            b"codex\0app-server\0--listen\0stdio://\0"
        ));
        assert!(!super::consumer_needs_lease(
            b"codex\0app-server\0--managed-daemon\0"
        ));
        assert!(!super::consumer_needs_lease(
            b"codex\0app-server\0daemon\0stop\0"
        ));
    }
    #[test]
    fn temporary_cwd_and_ephemeral_flag_do_not_isolate_authentication() {
        let environment = b"TMPDIR=/tmp/fixture\0HOME=/home/test\0";
        assert_eq!(
            super::environment_home(environment),
            Some(std::path::PathBuf::from("/home/test/.codex"))
        );
        assert_eq!(
            super::environment_home(b"HOME=/home/test\0CODEX_HOME=/tmp/private\0"),
            Some(std::path::PathBuf::from("/tmp/private"))
        );
        assert!(super::environment_home(b"OTHER=private\0").is_none());
    }
}

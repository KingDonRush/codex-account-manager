use crate::error::AppError;
#[cfg(target_os = "linux")]
use serde::Deserialize;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub(super) struct DaemonIdentity {
    pub pid: u32,
    pub started_ticks: u64,
    pub executable: PathBuf,
    #[cfg(unix)]
    pub socket: PathBuf,
}

#[cfg(target_os = "linux")]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredIdentity {
    pid: u32,
    process_identity: ProcessIdentity,
}
#[cfg(target_os = "linux")]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProcessIdentity {
    boot_id: String,
    start_ticks: u64,
}

pub(super) fn discover(home: &Path) -> Result<Option<DaemonIdentity>, AppError> {
    let metadata = home.join("app-server-daemon/daemon.pid");
    let file = match open_metadata(&metadata) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(AppError::Io(error)),
    };
    let mut bytes = Vec::new();
    file.take(16385).read_to_end(&mut bytes)?;
    if bytes.len() > 16384 {
        return Err(AppError::Runtime(
            "Registro de daemon excessivo.".to_owned(),
        ));
    }
    #[cfg(target_os = "linux")]
    {
        let stored: StoredIdentity = serde_json::from_slice(&bytes)?;
        discover_linux(home, stored)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = bytes;
        Err(AppError::Runtime("O daemon requer verificação específica desta plataforma; nenhuma sessão foi encerrada.".to_owned()))
    }
}

fn open_metadata(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path)
}

#[cfg(target_os = "linux")]
fn discover_linux(home: &Path, stored: StoredIdentity) -> Result<Option<DaemonIdentity>, AppError> {
    let Some(ticks) = running_ticks(stored.pid)? else {
        return Ok(None);
    };
    let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
    if ticks != stored.process_identity.start_ticks
        || boot.trim() != stored.process_identity.boot_id
    {
        return Ok(None);
    }
    let executable = std::fs::read_link(format!("/proc/{}/exe", stored.pid))?;
    let packages = home.join("packages/app-server-daemon").canonicalize()?;
    if !executable.starts_with(packages)
        || executable.file_name().and_then(|name| name.to_str()) != Some("codex")
    {
        return Err(AppError::Runtime(
            "O PID não pertence à instalação gerenciada do Codex.".to_owned(),
        ));
    }
    let arguments = std::fs::read(format!("/proc/{}/cmdline", stored.pid))?;
    if !arguments
        .split(|byte| *byte == 0)
        .any(|arg| arg == b"--managed-daemon")
    {
        return Err(AppError::Runtime(
            "O processo registrado não é um daemon gerenciado.".to_owned(),
        ));
    }
    Ok(Some(DaemonIdentity {
        pid: stored.pid,
        started_ticks: ticks,
        executable,
        socket: home.join("app-server-control/app-server-control.sock"),
    }))
}

#[cfg(target_os = "linux")]
pub(super) fn running_ticks(pid: u32) -> Result<Option<u64>, AppError> {
    let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(AppError::Io(error)),
    };
    let fields = stat
        .rsplit_once(") ")
        .ok_or_else(|| AppError::Runtime("Identidade de processo inválida.".to_owned()))?
        .1
        .split_whitespace()
        .collect::<Vec<_>>();
    if fields
        .first()
        .is_some_and(|state| matches!(*state, "Z" | "X"))
    {
        return Ok(None);
    }
    fields
        .get(19)
        .and_then(|ticks| ticks.parse().ok())
        .map(Some)
        .ok_or_else(|| AppError::Runtime("Identidade de processo incompleta.".to_owned()))
}

impl DaemonIdentity {
    pub(super) fn is_current(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            running_ticks(self.pid).ok().flatten() == Some(self.started_ticks)
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn stale_incarnation_is_absent_and_foreign_executable_is_rejected() {
        let home = tempfile::tempdir().expect("home");
        let metadata = home.path().join("app-server-daemon");
        std::fs::create_dir(&metadata).expect("metadata");
        std::fs::create_dir_all(home.path().join("packages/app-server-daemon"))
            .expect("trusted root");
        let pid = std::process::id();
        let ticks = running_ticks(pid).expect("identity").expect("running");
        let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").expect("boot");
        let document = |start| serde_json::json!({"pid":pid,"processIdentity":{"bootId":boot.trim(),"startTicks":start}});
        std::fs::write(metadata.join("daemon.pid"), document(ticks + 1).to_string())
            .expect("stale");
        assert!(discover(home.path()).expect("stale ignored").is_none());
        std::fs::write(metadata.join("daemon.pid"), document(ticks).to_string()).expect("foreign");
        assert!(matches!(discover(home.path()), Err(AppError::Runtime(_))));
    }

    #[test]
    fn pid_metadata_cannot_redirect_discovery_via_symlink() {
        let home = tempfile::tempdir().expect("home");
        std::fs::create_dir(home.path().join("app-server-daemon")).expect("metadata");
        std::fs::write(home.path().join("foreign"), "{}").expect("foreign");
        std::os::unix::fs::symlink(
            home.path().join("foreign"),
            home.path().join("app-server-daemon/daemon.pid"),
        )
        .expect("link");
        assert!(discover(home.path()).is_err());
    }
}

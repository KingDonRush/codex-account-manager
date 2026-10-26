use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;

use crate::error::AppError;

/// An advisory lock released by the OS on close or process exit. Keep the
/// file's inode stable: removing a lock file can let two owners acquire it.
pub struct FileLock {
    _file: File,
}

impl FileLock {
    /// Wait in bounded slices, so a competing owner never freezes cancellation.
    pub fn acquire_cancellable(
        path: &Path,
        cancel: &std::sync::atomic::AtomicBool,
        timeout: std::time::Duration,
    ) -> Result<Self, AppError> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if cancel.load(std::sync::atomic::Ordering::Acquire) {
                return Err(AppError::Cancelled);
            }
            match Self::acquire(path) {
                Ok(lease) => return Ok(lease),
                Err(AppError::Busy(_)) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(25))
                }
                Err(error) => return Err(error),
            }
        }
    }
    pub fn acquire(path: &Path) -> Result<Self, AppError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options.open(path)?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(TryLockError::WouldBlock) => Err(AppError::Busy(
                "Outra interface está alterando esta sessão. Aguarde a operação terminar e tente novamente.".to_owned(),
            )),
            Err(TryLockError::Error(error)) => Err(AppError::Io(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FileLock;
    use crate::error::AppError;

    #[test]
    fn competing_owner_is_rejected_and_close_releases_the_lock() {
        let directory = tempfile::tempdir().expect("fixture");
        let path = directory.path().join("auth-switch.lock");
        let first = FileLock::acquire(&path).expect("first owner");
        assert!(matches!(FileLock::acquire(&path), Err(AppError::Busy(_))));
        drop(first);
        assert!(FileLock::acquire(&path).is_ok());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rust_lease_blocks_python_and_python_lease_blocks_rust() {
        let directory = tempfile::tempdir().expect("fixture");
        let lock = directory.path().join("auth-switch.lock");
        let first = FileLock::acquire(&lock).expect("rust owner");
        let helper = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../integration");
        let result = std::process::Command::new("python3").args(["-B", "-c", "from pathlib import Path; from codex_session import session_lease\nwith session_lease(Path(__import__('sys').argv[1]), timeout=0): pass"]).arg(directory.path()).current_dir(&helper).output().expect("python");
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("SessionBusy"));
        drop(first);
        let mut child = std::process::Command::new("python3").args(["-B", "-c", "from pathlib import Path; from codex_session import session_lease\nwith session_lease(Path(__import__('sys').argv[1]), timeout=0):\n print('ready', flush=True)\n __import__('sys').stdin.read(1)"]).arg(directory.path()).current_dir(&helper).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().expect("python owner");
        use std::io::BufRead;
        let mut ready = String::new();
        std::io::BufReader::new(child.stdout.take().expect("stdout"))
            .read_line(&mut ready)
            .expect("ready");
        assert_eq!(ready.trim(), "ready");
        assert!(FileLock::acquire(&lock).is_err());
        drop(child.stdin.take());
        assert!(child.wait().expect("exit").success());
        assert!(FileLock::acquire(&lock).is_ok());
    }
}

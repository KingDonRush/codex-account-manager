use std::process::{Child, Command};

use crate::error::AppError;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use self::windows::WindowsJob;

/// Owns the operating-system primitive that contains a child process tree.
/// Dropping the application can then terminate Chromium and App Server
/// descendants instead of leaving background processes behind.
pub struct ProcessSupervisor {
    #[cfg(unix)]
    process_group: i32,
    #[cfg(target_os = "linux")]
    leader_started_ticks: u64,
    #[cfg(windows)]
    job: WindowsJob,
}

impl ProcessSupervisor {
    pub fn attach(child: &Child) -> Result<Self, AppError> {
        #[cfg(unix)]
        {
            let process_group = i32::try_from(child.id()).map_err(|_| {
                AppError::Internal("PID do processo filho fora do intervalo".to_owned())
            })?;
            let actual_group = unsafe { libc::getpgid(process_group) };
            if actual_group != process_group {
                return Err(AppError::Internal(
                    "o processo filho não iniciou em um grupo isolado".to_owned(),
                ));
            }
            Ok(Self {
                process_group,
                #[cfg(target_os = "linux")]
                leader_started_ticks: linux_started_ticks(process_group)?,
            })
        }
        #[cfg(windows)]
        {
            Ok(Self {
                job: WindowsJob::attach(child)?,
            })
        }
    }

    pub fn terminate(&self, _child: &mut Child) -> Result<(), AppError> {
        #[cfg(unix)]
        {
            #[cfg(target_os = "linux")]
            match linux_started_ticks(self.process_group) {
                Ok(started) if started != self.leader_started_ticks => {
                    return Err(AppError::Busy(
                        "O PID do processo supervisionado foi reutilizado; encerramento bloqueado."
                            .to_owned(),
                    ))
                }
                Err(AppError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
                Ok(_) => {}
            }
            terminate_process_group(self.process_group, _child)
        }
        #[cfg(windows)]
        {
            self.job.terminate()
        }
    }
}

pub fn configure_supervised_process(command: &mut Command) {
    configure_platform_process(command);
}

#[cfg(unix)]
fn configure_platform_process(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    command.process_group(0);
    #[cfg(target_os = "linux")]
    unsafe {
        let expected_parent = libc::getpid();
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() != expected_parent {
                return Err(std::io::Error::other(
                    "O proprietário terminou durante o spawn.",
                ));
            }
            Ok(())
        });
    }
}

#[cfg(unix)]
fn terminate_process_group(group: i32, child: &mut Child) -> Result<(), AppError> {
    let result = unsafe { libc::kill(-group, libc::SIGTERM) };
    if result != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(AppError::Io(error));
        }
    }
    for _ in 0..10 {
        let _ = child.try_wait()?;
        if !process_group_is_running(group) {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let result = unsafe { libc::kill(-group, libc::SIGKILL) };
    if result != 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
        return Err(AppError::Io(std::io::Error::last_os_error()));
    }
    for _ in 0..20 {
        let _ = child.try_wait()?;
        if !process_group_is_running(group) {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    Err(AppError::Internal(
        "A árvore do processo não encerrou no prazo.".to_owned(),
    ))
}

#[cfg(target_os = "linux")]
pub(super) fn linux_started_ticks(pid: i32) -> Result<u64, AppError> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    stat.rsplit_once(") ")
        .and_then(|(_, fields)| fields.split_whitespace().nth(19))
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| AppError::Internal("Identidade de processo inválida.".to_owned()))
}

#[cfg(unix)]
fn process_group_is_running(group: i32) -> bool {
    #[cfg(target_os = "linux")]
    {
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return true;
        };
        entries.flatten().any(|entry| {
            let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
                return false;
            };
            let Some((_, fields)) = stat.rsplit_once(") ") else {
                return false;
            };
            let fields = fields.split_whitespace().take(3).collect::<Vec<_>>();
            fields.get(2).and_then(|value| value.parse::<i32>().ok()) == Some(group)
                && fields
                    .first()
                    .is_some_and(|state| !matches!(*state, "Z" | "X"))
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        unsafe { libc::kill(-group, 0) == 0 }
    }
}

#[cfg(windows)]
fn configure_platform_process(command: &mut Command) {
    use std::os::windows::process::CommandExt;

    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_SUSPENDED: u32 = 0x0000_0004;
    // The launcher assigns the job before resuming the child's primary thread.
    // CREATE_NO_WINDOW affects console helpers, not Chromium's GUI windows.
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW | CREATE_SUSPENDED);
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::process::Command;
    use std::time::{Duration, Instant};

    use super::{configure_supervised_process, ProcessSupervisor};

    #[test]
    fn termination_removes_the_supervised_process_tree_within_the_bound() {
        let mut command = Command::new("sh");
        configure_supervised_process(&mut command);
        command.args(["-c", "sleep 30 & wait"]);
        let mut child = command.spawn().expect("process tree");
        let supervisor = ProcessSupervisor::attach(&child).expect("supervisor");
        let descendant = wait_for_descendant(child.id());

        let started = Instant::now();
        supervisor.terminate(&mut child).expect("terminate tree");
        let _ = child.wait();

        assert!(started.elapsed() <= Duration::from_millis(800));
        assert!(wait_until_gone(descendant));
    }

    #[test]
    fn exited_leader_does_not_leave_a_sigterm_resistant_descendant() {
        let mut command = Command::new("sh");
        configure_supervised_process(&mut command);
        command.args([
            "-c",
            "sh -c 'trap \"\" TERM; while :; do sleep 30; done' & wait",
        ]);
        let mut child = command.spawn().expect("owned tree");
        let supervisor = ProcessSupervisor::attach(&child).expect("supervisor");
        let descendant = wait_for_descendant(child.id());
        std::thread::sleep(Duration::from_millis(40));
        // SAFETY: this is our unreaped child, whose identity cannot be reused.
        assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
        let _ = child.wait().expect("leader exited");
        assert!(super::process_group_is_running(child.id() as i32));
        let started = Instant::now();
        supervisor.terminate(&mut child).expect("entire group");
        assert!(!super::process_group_is_running(child.id() as i32));
        assert!(started.elapsed() < Duration::from_millis(1200));
        let _ = descendant;
    }

    fn wait_for_descendant(parent: u32) -> i32 {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let path = format!("/proc/{parent}/task/{parent}/children");
            if let Ok(content) = std::fs::read_to_string(path) {
                if let Some(pid) = content
                    .split_whitespace()
                    .next()
                    .and_then(|value| value.parse::<i32>().ok())
                {
                    return pid;
                }
            }
            assert!(Instant::now() < deadline, "descendant did not start");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_until_gone(pid: i32) -> bool {
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            let result = unsafe { libc::kill(pid, 0) };
            if result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }
}

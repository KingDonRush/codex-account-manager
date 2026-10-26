use std::collections::VecDeque;
#[cfg(all(test, unix))]
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use std::time::Instant;

use parking_lot::Mutex as ParkingMutex;

#[cfg(any(test, not(any(target_os = "linux", windows))))]
use crate::adapters::loopback_redirect::LoopbackRedirect;
use crate::adapters::process_launcher::spawn_supervised;
use crate::adapters::process_supervisor::{configure_supervised_process, ProcessSupervisor};
use crate::adapters::runtime_installer::{
    required_download_size, validate_active_runtime, ActiveRuntime, RuntimeInstaller,
};
use crate::domain::RuntimeStatus;
use crate::error::AppError;
#[cfg(all(unix, any(test, not(any(target_os = "linux", windows)))))]
mod cdp;
mod controlled;
#[cfg(any(target_os = "linux", windows))]
pub(crate) mod native_bridge;
#[cfg(any(target_os = "linux", windows))]
mod native_tabs;
#[cfg(all(test, any(target_os = "linux", windows)))]
mod native_tests;
mod startup;

#[derive(Clone)]
pub struct BrowserRuntime {
    app_data_dir: PathBuf,
    bundled_runtime_dir: Option<PathBuf>,
    install_lock: Arc<ParkingMutex<()>>,
    #[cfg(any(unix, windows))]
    controlled_sessions: Arc<ParkingMutex<std::collections::HashMap<PathBuf, ControlledSession>>>,
}

#[cfg(any(unix, windows))]
struct ControlledSession {
    handle: BrowserHandle,
    #[cfg(all(unix, any(test, not(any(target_os = "linux", windows)))))]
    pending_targets: Vec<String>,
    #[cfg(any(target_os = "linux", windows))]
    pending_native_operations: Vec<String>,
}

#[cfg(any(unix, windows))]
impl ControlledSession {
    fn close_pending(&mut self) -> Result<(), AppError> {
        #[cfg(any(target_os = "linux", windows))]
        if !self.pending_native_operations.is_empty() {
            let native = self.handle.native.as_ref().ok_or_else(|| {
                AppError::Browser("Controle nativo indisponível; abas preservadas.".into())
            })?;
            for operation in &self.pending_native_operations {
                if let Err(error) = native.close_operation(operation) {
                    if self.handle.is_alive()? {
                        return Err(error);
                    }
                    break; // The owned root exited; none of its targets can survive.
                }
            }
            self.pending_native_operations.clear();
        }
        #[cfg(all(unix, any(test, not(any(target_os = "linux", windows)))))]
        {
            if self.pending_targets.is_empty() {
                return Ok(());
            }
            let pipe = self.handle.controller.as_ref().ok_or_else(|| {
                AppError::Browser(
                    "Controle de abas pendentes indisponível; perfil preservado.".to_owned(),
                )
            })?;
            pipe.close_targets(&self.pending_targets)?;
            self.pending_targets.clear();
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct BrowserHandle {
    child: Arc<Mutex<Child>>,
    supervisor: Arc<ProcessSupervisor>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    #[cfg(any(test, not(any(target_os = "linux", windows))))]
    redirects: Arc<Vec<LoopbackRedirect>>,
    #[cfg(any(test, not(any(target_os = "linux", windows))))]
    navigation_retry: Option<Arc<NavigationRetry>>,
    #[cfg(all(unix, any(test, not(any(target_os = "linux", windows)))))]
    controller: Option<Arc<cdp::CdpPipe>>,
    #[cfg(all(unix, any(test, not(any(target_os = "linux", windows)))))]
    owned_targets: Vec<String>,
    #[cfg(any(target_os = "linux", windows))]
    native: Option<Arc<native_tabs::NativeSession>>,
    #[cfg(any(target_os = "linux", windows))]
    native_operation: Option<String>,
}

#[cfg(any(test, not(any(target_os = "linux", windows))))]
struct NavigationRetry {
    executable: PathBuf,
    profile_dir: PathBuf,
    launch_url: String,
}

#[cfg(any(test, not(any(target_os = "linux", windows))))]
const NAVIGATION_RETRY_PROCESS_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(any(target_os = "linux", windows))]
const BROWSER_NORMAL_EXIT_TIMEOUT: Duration = Duration::from_secs(3);

impl BrowserHandle {
    #[cfg(any(target_os = "linux", windows))]
    fn wait_for_normal_exit(&self) -> Result<bool, AppError> {
        let deadline = Instant::now() + BROWSER_NORMAL_EXIT_TIMEOUT;
        loop {
            if !self.is_alive()? {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
    pub fn same_process(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.child, &other.child)
    }

    pub fn pid(&self) -> Result<u32, AppError> {
        let child = self
            .child
            .lock()
            .map_err(|_| AppError::Internal("supervisor do navegador indisponível".to_owned()))?;
        Ok(child.id())
    }

    pub fn is_alive(&self) -> Result<bool, AppError> {
        let mut child = self
            .child
            .lock()
            .map_err(|_| AppError::Internal("supervisor do navegador indisponível".to_owned()))?;
        Ok(child.try_wait()?.is_none())
    }

    pub fn ensure_alive(&self) -> Result<(), AppError> {
        let mut child = self
            .child
            .lock()
            .map_err(|_| AppError::Internal("supervisor do navegador indisponível".to_owned()))?;
        let Some(status) = child.try_wait()? else {
            drop(child);
            #[cfg(all(unix, any(test, not(any(target_os = "linux", windows)))))]
            if let Some(pipe) = &self.controller {
                if !pipe.owns_alive_target(&self.owned_targets)? {
                    return Err(AppError::Cancelled);
                }
            }
            #[cfg(any(target_os = "linux", windows))]
            if let Some(native) = &self.native {
                if let Some(operation) = self.native_operation.as_deref() {
                    if !native.operation_alive(operation)? {
                        return Err(AppError::Cancelled);
                    }
                }
            }
            return Ok(());
        };
        if status.success() {
            return Err(AppError::Cancelled);
        }
        let tail = self
            .stderr_tail
            .lock()
            .map_err(|_| AppError::Internal("diagnóstico do navegador indisponível".to_owned()))?;
        let detail = tail.iter().rev().take(3).cloned().collect::<Vec<_>>();
        let detail = if detail.is_empty() {
            String::new()
        } else {
            format!(
                "; diagnóstico: {}",
                detail.into_iter().rev().collect::<Vec<_>>().join(" | ")
            )
        };
        Err(AppError::Browser(format!(
            "o Chromium encerrou antes do login ({status}){detail}"
        )))
    }

    pub fn terminate(&self) -> Result<(), AppError> {
        #[cfg(all(unix, any(test, not(any(target_os = "linux", windows)))))]
        let alive = self.is_alive()?;
        #[cfg(all(unix, any(test, not(any(target_os = "linux", windows)))))]
        if let Some(pipe) = &self.controller {
            if alive && !self.owned_targets.is_empty() {
                match pipe.close_targets(&self.owned_targets) {
                    Ok(false) => return Ok(()),
                    Ok(true) => {}
                    Err(error) => {
                        if self.is_alive()? {
                            return Err(error);
                        }
                    }
                }
            }
        }
        #[cfg(any(target_os = "linux", windows))]
        if let Some(native) = &self.native {
            if self.is_alive()? && self.native_operation.is_some() {
                match native.close_operation(
                    self.native_operation
                        .as_deref()
                        .ok_or_else(native_bridge::unavailable)?,
                ) {
                    Ok(false) => {
                        return Ok(());
                    }
                    Ok(true) => {
                        // The last window is gone, but Chromium may still be
                        // flushing its profile. Don't SIGTERM that normal exit.
                        if !self.wait_for_normal_exit()? {
                            return Err(AppError::Browser("O navegador está concluindo o fechamento; seu perfil foi preservado sem interrupção forçada.".into()));
                        }
                    }
                    Err(error) => {
                        // Native Messaging may disconnect during a normal quit,
                        // before the supervised child can be reaped.
                        if !self.wait_for_normal_exit()? {
                            return Err(error);
                        }
                    }
                }
            }
        }
        #[cfg(any(test, not(any(target_os = "linux", windows))))]
        for redirect in self.redirects.iter() {
            redirect.stop();
        }
        let mut child = self
            .child
            .lock()
            .map_err(|_| AppError::Internal("supervisor do navegador indisponível".to_owned()))?;
        self.supervisor.terminate(&mut child)?;
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        loop {
            if child.try_wait()?.is_some() {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(AppError::Browser(
                    "O processo do navegador não encerrou no prazo.".to_owned(),
                ));
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn wait_for_initial_navigation(&self, _timeout: Duration) -> bool {
        #[cfg(any(target_os = "linux", windows))]
        if self.native.is_some() {
            return true;
        }
        #[cfg(all(unix, any(test, not(any(target_os = "linux", windows)))))]
        if self.controller.is_some() {
            return true;
        }
        #[cfg(any(test, not(any(target_os = "linux", windows))))]
        return self
            .redirects
            .first()
            .is_some_and(|redirect| redirect.wait_until_delivered(_timeout));
        #[cfg(all(not(test), any(target_os = "linux", windows)))]
        false
    }

    pub fn retry_initial_navigation(&self) -> Result<bool, AppError> {
        #[cfg(all(not(test), any(target_os = "linux", windows)))]
        return Ok(false);
        #[cfg(any(test, not(any(target_os = "linux", windows))))]
        {
            let Some(redirect) = self.redirects.first() else {
                return Ok(false);
            };
            if redirect.is_delivered() || !redirect.can_retry() {
                return Ok(false);
            }
            self.ensure_alive()?;
            let Some(retry) = self.navigation_retry.as_deref() else {
                return Ok(false);
            };
            spawn_navigation_retry(retry)?;
            Ok(true)
        }
    }

    #[cfg(all(test, unix))]
    pub(crate) fn stubborn_test_process() -> Result<Self, AppError> {
        let mut command = Command::new("sh");
        configure_supervised_process(&mut command);
        command
            .args([
                "-c",
                "trap '' TERM; printf 'ready\\n'; while :; do sleep 30 & wait; done",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn()?;
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(AppError::Internal(
                "stdout do navegador de teste".to_owned(),
            ));
        };
        let mut ready = String::new();
        if let Err(error) = BufReader::new(stdout).read_line(&mut ready) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(AppError::Io(error));
        }
        if ready.trim() != "ready" {
            let _ = child.kill();
            let _ = child.wait();
            return Err(AppError::Internal(
                "navegador de teste não ficou pronto".to_owned(),
            ));
        }
        let stderr_tail = Arc::new(Mutex::new(VecDeque::with_capacity(24)));
        if let Some(stderr) = child.stderr.take() {
            capture_stderr(stderr, stderr_tail.clone());
        }
        let supervisor = match ProcessSupervisor::attach(&child) {
            Ok(supervisor) => Arc::new(supervisor),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        Ok(Self {
            child: Arc::new(Mutex::new(child)),
            supervisor,
            stderr_tail,
            redirects: Arc::new(Vec::new()),
            navigation_retry: None,
            #[cfg(unix)]
            controller: None,
            #[cfg(unix)]
            owned_targets: Vec::new(),
            #[cfg(target_os = "linux")]
            native: None,
            #[cfg(target_os = "linux")]
            native_operation: None,
        })
    }
}

impl Drop for BrowserHandle {
    fn drop(&mut self) {
        if Arc::strong_count(&self.child) != 1 {
            return;
        }
        #[cfg(any(test, not(any(target_os = "linux", windows))))]
        for redirect in self.redirects.iter() {
            redirect.stop();
        }
        if let Ok(mut child) = self.child.lock() {
            let _ = self.supervisor.terminate(&mut child);
            let _ = child.try_wait();
        }
    }
}

impl BrowserRuntime {
    pub fn with_bundled_runtime(app_data_dir: &Path, bundled_runtime_dir: Option<PathBuf>) -> Self {
        Self {
            app_data_dir: app_data_dir.to_path_buf(),
            bundled_runtime_dir,
            install_lock: Arc::new(ParkingMutex::new(())),
            #[cfg(any(unix, windows))]
            controlled_sessions: Arc::new(ParkingMutex::new(std::collections::HashMap::new())),
        }
    }

    pub fn status(&self) -> RuntimeStatus {
        if let Some(executable) = configured_managed_executable() {
            return RuntimeStatus {
                runtime_kind: "managed".to_owned(),
                version: None,
                installed: executable.is_file(),
                repair_required: false,
                download_size_bytes: required_download_size(),
            };
        }
        match self.bundled_runtime() {
            Ok(Some((active, executable))) => {
                return RuntimeStatus {
                    runtime_kind: "managed".to_owned(),
                    version: Some(active.version),
                    installed: executable.is_file(),
                    repair_required: !executable.is_file(),
                    download_size_bytes: None,
                };
            }
            Err(_) => {
                return RuntimeStatus {
                    runtime_kind: "managed".to_owned(),
                    version: None,
                    installed: false,
                    repair_required: true,
                    download_size_bytes: None,
                };
            }
            Ok(None) if self.bundled_runtime_dir.is_some() => {
                return RuntimeStatus {
                    runtime_kind: "managed".to_owned(),
                    version: None,
                    installed: false,
                    repair_required: true,
                    download_size_bytes: None,
                };
            }
            Ok(None) => {}
        }
        let installer = RuntimeInstaller::new(&self.app_data_dir);
        match installer.active() {
            Ok(Some(active)) => {
                let executable = self
                    .app_data_dir
                    .join("runtime/chromium")
                    .join(active.executable);
                return RuntimeStatus {
                    runtime_kind: "managed".to_owned(),
                    version: Some(active.version),
                    installed: executable.is_file(),
                    repair_required: !executable.is_file(),
                    download_size_bytes: required_download_size(),
                };
            }
            Err(_) => {
                return RuntimeStatus {
                    runtime_kind: "managed".to_owned(),
                    version: None,
                    installed: false,
                    repair_required: true,
                    download_size_bytes: required_download_size(),
                };
            }
            Ok(None) => {}
        }
        let executable = system_executable();
        RuntimeStatus {
            runtime_kind: "system".to_owned(),
            installed: executable.is_some(),
            version: None,
            repair_required: false,
            download_size_bytes: required_download_size(),
        }
    }

    pub fn install_managed(&self) -> Result<RuntimeStatus, AppError> {
        let _guard = self.install_lock.lock();
        if self.bundled_runtime_dir.is_some() {
            let Some((_, executable)) = self.bundled_runtime()? else {
                return Err(AppError::Runtime(
                    "o Chromium integrado não foi encontrado; reinstale o aplicativo".to_owned(),
                ));
            };
            if !executable.is_file() {
                return Err(AppError::Runtime(
                    "o Chromium integrado está incompleto; reinstale o aplicativo".to_owned(),
                ));
            }
            return Ok(self.status());
        }
        RuntimeInstaller::new(&self.app_data_dir).install()?;
        Ok(self.status())
    }

    #[cfg(not(any(target_os = "linux", windows)))]
    pub fn launch(
        &self,
        profile_dir: &Path,
        oauth_url: &str,
        mail_url: Option<&str>,
    ) -> Result<BrowserHandle, AppError> {
        let executable = self
            .managed_executable()
            .filter(|path| path.is_file())
            .or_else(system_executable)
            .ok_or_else(|| AppError::Browser("Chromium, Chrome ou Edge não foi encontrado. Defina CODEX_MANAGER_CHROMIUM para um executável autorizado.".to_owned()))?;
        std::fs::create_dir_all(profile_dir)?;
        restrict_directory(profile_dir)?;

        startup::prepare_profile_startup(profile_dir)?;

        let (oauth_launch_url, oauth_redirect) = LoopbackRedirect::start(oauth_url)?;
        let mut redirects = vec![oauth_redirect];
        let mail_launch_url = match mail_url {
            Some(url) => {
                let (launch_url, redirect) = match LoopbackRedirect::start(url) {
                    Ok(redirect) => redirect,
                    Err(error) => {
                        stop_redirects(&redirects);
                        return Err(error);
                    }
                };
                redirects.push(redirect);
                Some(launch_url)
            }
            None => None,
        };

        let navigation_retry = NavigationRetry {
            executable: executable.clone(),
            profile_dir: profile_dir.to_path_buf(),
            launch_url: oauth_launch_url.clone(),
        };
        let mut command = Command::new(&executable);
        configure_supervised_process(&mut command);
        command
            .arg(format!("--user-data-dir={}", profile_dir.display()))
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("--disable-background-mode")
            .arg("--disable-session-crashed-bubble")
            .arg("--new-window")
            .arg(&oauth_launch_url)
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if let Some(url) = mail_launch_url.as_deref() {
            command.arg(url);
        }
        let (mut child, supervisor) = spawn_supervised(command).map_err(|error| {
            stop_redirects(&redirects);
            AppError::Browser(error.to_string())
        })?;
        let stderr_tail = Arc::new(Mutex::new(VecDeque::with_capacity(24)));
        if let Some(stderr) = child.stderr.take() {
            capture_stderr(stderr, stderr_tail.clone());
        }
        Ok(BrowserHandle {
            child: Arc::new(Mutex::new(child)),
            supervisor: Arc::new(supervisor),
            stderr_tail,
            redirects: Arc::new(redirects),
            navigation_retry: Some(Arc::new(navigation_retry)),
            #[cfg(unix)]
            controller: None,
            #[cfg(unix)]
            owned_targets: Vec::new(),
        })
    }

    pub fn open_tab(&self, profile_dir: &Path, url: &str) -> Result<(), AppError> {
        #[cfg(any(target_os = "linux", windows))]
        {
            let mut registry = self.controlled_sessions.lock();
            if let Some(session) = registry.get_mut(profile_dir) {
                session.close_pending()?;
                if let Some(native) = &session.handle.native {
                    if session.handle.is_alive()? {
                        return native.open_manual(url);
                    }
                }
                #[cfg(all(test, unix))]
                if let Some(pipe) = &session.handle.controller {
                    pipe.create_target(url)?;
                    return Ok(());
                }
            }
            drop(registry);
            self.launch_controlled(profile_dir, url, None)?;
            Ok(())
        }
        #[cfg(all(unix, not(target_os = "linux")))]
        {
            #[cfg(unix)]
            if let Some(session) = self.controlled_sessions.lock().get_mut(profile_dir) {
                session.close_pending()?;
                if let Some(pipe) = &session.handle.controller {
                    pipe.create_target(url)?;
                    return Ok(());
                }
            }
            #[cfg(unix)]
            {
                // A stale/missing controller is not a reason to launch another
                // restore-enabled process through the legacy URL bridge.
                self.launch_controlled(profile_dir, url, None)?;
                Ok(())
            }
        }
        #[cfg(all(not(unix), not(windows)))]
        self.open_legacy_tab(profile_dir, url)
    }

    #[cfg(not(any(target_os = "linux", windows)))]
    fn open_legacy_tab(&self, profile_dir: &Path, url: &str) -> Result<(), AppError> {
        let executable = self
            .managed_executable()
            .filter(|path| path.is_file())
            .or_else(system_executable)
            .ok_or_else(|| {
                AppError::Browser("Chromium, Chrome ou Edge não foi encontrado".to_owned())
            })?;
        std::fs::create_dir_all(profile_dir)?;
        let (launch_url, redirect) = LoopbackRedirect::start(url)?;
        let mut command = Command::new(executable);
        configure_supervised_process(&mut command);
        command
            .arg(format!("--user-data-dir={}", profile_dir.display()))
            .arg("--new-tab")
            .arg(&launch_url)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let (child, supervisor) = spawn_supervised(command).map_err(|error| {
            redirect.stop();
            AppError::Browser(error.to_string())
        })?;
        spawn_retry_reaper(child, supervisor)
    }

    fn managed_executable(&self) -> Option<PathBuf> {
        if let Some(path) = configured_managed_executable() {
            return Some(path);
        }
        if let Ok(Some((_, path))) = self.bundled_runtime() {
            if path.is_file() {
                return Some(path);
            }
        }
        if let Ok(Some(path)) = RuntimeInstaller::new(&self.app_data_dir).active_executable() {
            if path.is_file() {
                return Some(path);
            }
        }
        let root = self.app_data_dir.join("runtime").join("chromium");
        let candidates = if cfg!(windows) {
            vec![root.join("chrome.exe"), root.join("chromium.exe")]
        } else if cfg!(target_os = "macos") {
            vec![root.join("Chromium.app/Contents/MacOS/Chromium")]
        } else {
            vec![root.join("chrome"), root.join("chromium")]
        };
        candidates.into_iter().find(|path| path.is_file())
    }

    fn bundled_runtime(&self) -> Result<Option<(ActiveRuntime, PathBuf)>, AppError> {
        let Some(root) = self.bundled_runtime_dir.as_deref() else {
            return Ok(None);
        };
        let marker = root.join("active.json");
        let content = match std::fs::read_to_string(marker) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(AppError::Io(error)),
        };
        let active: ActiveRuntime = serde_json::from_str(&content)?;
        validate_active_runtime(&active)?;
        let executable = root.join(&active.executable);
        Ok(Some((active, executable)))
    }
}

#[cfg(any(test, not(any(target_os = "linux", windows))))]
fn spawn_navigation_retry(retry: &NavigationRetry) -> Result<(), AppError> {
    let mut command = Command::new(&retry.executable);
    configure_supervised_process(&mut command);
    command
        .arg(format!("--user-data-dir={}", retry.profile_dir.display()))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-background-mode")
        .arg("--new-tab")
        .arg(&retry.launch_url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let (mut child, supervisor) = spawn_supervised(command)?;
    if let Some(status) = child.try_wait()? {
        return if status.success() {
            Ok(())
        } else {
            Err(AppError::Browser(format!(
                "nova tentativa de navegação encerrou com {status}"
            )))
        };
    }
    spawn_retry_reaper(child, supervisor)
}

#[cfg(any(test, not(any(target_os = "linux", windows))))]
fn spawn_retry_reaper(child: Child, supervisor: ProcessSupervisor) -> Result<(), AppError> {
    let resources = Arc::new(Mutex::new(Some((child, supervisor))));
    let worker_resources = resources.clone();
    let worker = thread::Builder::new()
        .name("chromium-navigation-retry".to_owned())
        .spawn(move || {
            let Ok(mut resources) = worker_resources.lock() else {
                return;
            };
            let Some((mut child, supervisor)) = resources.take() else {
                return;
            };
            drop(resources);
            reap_retry_process(&mut child, &supervisor);
        });
    if let Err(error) = worker {
        if let Ok(mut resources) = resources.lock() {
            if let Some((mut child, supervisor)) = resources.take() {
                let _ = supervisor.terminate(&mut child);
                let _ = child.wait();
            }
        }
        return Err(AppError::Internal(format!(
            "reaper da nova tentativa de navegação: {error}"
        )));
    }
    Ok(())
}

#[cfg(any(test, not(any(target_os = "linux", windows))))]
fn reap_retry_process(child: &mut Child, supervisor: &ProcessSupervisor) {
    let deadline = Instant::now() + NAVIGATION_RETRY_PROCESS_TIMEOUT;
    while Instant::now() < deadline {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let _ = supervisor.terminate(child);
    let _ = child.try_wait();
}

#[cfg(not(any(target_os = "linux", windows)))]
fn stop_redirects(redirects: &[LoopbackRedirect]) {
    for redirect in redirects {
        redirect.stop();
    }
}

fn capture_stderr(stderr: std::process::ChildStderr, tail: Arc<Mutex<VecDeque<String>>>) {
    let _ = thread::Builder::new()
        .name("chromium-stderr".to_owned())
        .spawn(move || {
            crate::adapters::logging::read_stderr_tail(stderr, tail, 24, sanitize_browser_line)
        });
}

fn sanitize_browser_line(line: &str) -> String {
    let line = crate::adapters::logging::sanitize_error(line);
    line.split_whitespace()
        .take(32)
        .map(|part| {
            let lower = part.to_ascii_lowercase();
            if part.contains("://")
                || lower.contains("token")
                || lower.contains("code=")
                || lower.contains("cookie")
            {
                "[redacted]"
            } else {
                part
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(480)
        .collect()
}

fn configured_managed_executable() -> Option<PathBuf> {
    let value = std::env::var("CODEX_MANAGER_CHROMIUM").ok()?;
    let path = PathBuf::from(value);
    path.is_file().then_some(path)
}

fn system_executable() -> Option<PathBuf> {
    #[cfg(windows)]
    if let Some(path) = windows_browser_candidates()
        .into_iter()
        .find(|path| path.is_file())
    {
        return Some(path);
    }
    let names: &[&str] = if cfg!(target_os = "macos") {
        &[
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "chromium",
        ]
    } else if cfg!(windows) {
        &["chrome.exe", "msedge.exe", "chromium.exe"]
    } else {
        &[
            "google-chrome",
            "google-chrome-stable",
            "chromium",
            "chromium-browser",
            "microsoft-edge",
        ]
    };
    names.iter().find_map(|name| {
        let candidate = PathBuf::from(name);
        if candidate.is_file() {
            Some(candidate)
        } else {
            which::which(name).ok()
        }
    })
}

#[cfg(windows)]
fn windows_browser_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(root) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(root).join("Google/Chrome/Application/chrome.exe"));
    }
    for variable in ["PROGRAMFILES", "PROGRAMFILES(X86)"] {
        let Some(root) = std::env::var_os(variable) else {
            continue;
        };
        let root = PathBuf::from(root);
        candidates.push(root.join("Google/Chrome/Application/chrome.exe"));
        candidates.push(root.join("Microsoft/Edge/Application/msedge.exe"));
    }
    candidates
}

fn restrict_directory(_path: &Path) -> Result<(), AppError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(_path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::BrowserHandle;
    use super::BrowserRuntime;
    #[cfg(unix)]
    use crate::adapters::loopback_redirect::LoopbackRedirect;
    #[cfg(unix)]
    use crate::error::AppError;

    #[test]
    fn browser_diagnostics_redact_separated_bearer_and_json_values() {
        let sanitized = super::sanitize_browser_line(
            r#"Authorization: Bearer synthetic-private {"accessToken": "second-private"} https://login.example/callback?code=private"#,
        );

        assert!(!sanitized.contains("synthetic-private"));
        assert!(!sanitized.contains("second-private"));
        assert!(!sanitized.contains("login.example"));
        assert!(sanitized.len() <= 480);
    }

    #[cfg(unix)]
    fn shell_handle(script: &str) -> BrowserHandle {
        use std::collections::VecDeque;
        use std::process::{Command, Stdio};
        use std::sync::{Arc, Mutex};

        use crate::adapters::process_supervisor::{
            configure_supervised_process, ProcessSupervisor,
        };

        let mut command = Command::new("sh");
        configure_supervised_process(&mut command);
        command
            .args(["-c", script])
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = command.spawn().expect("test browser");
        let stderr_tail = Arc::new(Mutex::new(VecDeque::with_capacity(24)));
        if let Some(stderr) = child.stderr.take() {
            super::capture_stderr(stderr, stderr_tail.clone());
        }
        let supervisor = ProcessSupervisor::attach(&child).expect("supervisor");
        BrowserHandle {
            #[cfg(target_os = "linux")]
            native: None,
            #[cfg(target_os = "linux")]
            native_operation: None,
            child: Arc::new(Mutex::new(child)),
            supervisor: Arc::new(supervisor),
            stderr_tail,
            redirects: Arc::new(Vec::new()),
            navigation_retry: None,
            controller: None,
            owned_targets: Vec::new(),
        }
    }

    #[test]
    fn bundled_runtime_is_ready_without_a_download() {
        let app_data = tempfile::tempdir().expect("app data");
        let bundled = tempfile::tempdir().expect("bundled runtime");
        let executable = bundled.path().join("browser/chrome");
        std::fs::create_dir_all(executable.parent().expect("browser parent"))
            .expect("browser directory");
        std::fs::write(&executable, b"bundled-browser").expect("browser fixture");
        std::fs::write(
            bundled.path().join("active.json"),
            format!(
                "{{\"version\":\"153.0.8010.12\",\"executable\":\"browser/chrome\",\"archiveSha256\":\"{}\"}}",
                "a".repeat(64)
            ),
        )
        .expect("runtime marker");

        let runtime = BrowserRuntime::with_bundled_runtime(
            app_data.path(),
            Some(bundled.path().to_path_buf()),
        );
        let status = runtime.status();

        assert_eq!(status.runtime_kind, "managed");
        assert_eq!(status.version.as_deref(), Some("153.0.8010.12"));
        assert!(status.installed);
        assert!(!status.repair_required);
        assert_eq!(status.download_size_bytes, None);
    }

    #[test]
    fn missing_bundled_runtime_requires_reinstall_instead_of_a_download() {
        let app_data = tempfile::tempdir().expect("app data");
        let bundled = tempfile::tempdir().expect("bundled runtime");
        let runtime = BrowserRuntime::with_bundled_runtime(
            app_data.path(),
            Some(bundled.path().to_path_buf()),
        );

        let status = runtime.status();

        assert_eq!(status.runtime_kind, "managed");
        assert!(!status.installed);
        assert!(status.repair_required);
        assert_eq!(status.download_size_bytes, None);
    }

    #[cfg(unix)]
    #[test]
    fn clean_browser_close_is_a_user_cancellation() {
        let handle = shell_handle("exit 0");
        std::thread::sleep(std::time::Duration::from_millis(20));

        assert!(matches!(handle.ensure_alive(), Err(AppError::Cancelled)));
    }

    #[cfg(unix)]
    #[test]
    fn early_browser_failure_is_typed_and_sanitized() {
        let handle = shell_handle(
            "printf '%s\\n' 'token=super-secret https://oauth.example/code=secret' >&2; exit 7",
        );
        std::thread::sleep(std::time::Duration::from_millis(30));

        let error = handle.ensure_alive().expect_err("browser crash");
        let message = error.to_string();
        assert!(matches!(error, AppError::Browser(_)));
        assert!(message.contains("diagnóstico"));
        assert!(!message.contains("super-secret"));
        assert!(!message.contains("oauth.example"));
    }

    #[cfg(unix)]
    #[test]
    fn oversized_browser_stderr_is_drained_before_reporting_the_real_exit() {
        use std::time::{Duration, Instant};
        let handle = shell_handle(
            "head -c 3145728 /dev/zero | tr '\\0' x >&2; printf '\\nAuthorization: Bearer native-test-private\\nready after oversized record\\n' >&2; exit 7",
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let drained = handle
                .stderr_tail
                .lock()
                .expect("tail")
                .iter()
                .any(|line| line == "ready after oversized record");
            if drained && !handle.is_alive().expect("process") {
                break;
            }
            assert!(Instant::now() < deadline, "owned stderr must reach EOF");
            std::thread::sleep(Duration::from_millis(5));
        }

        let error = handle.ensure_alive().expect_err("exit 7").to_string();
        assert!(error.contains("ready after oversized record"));
        assert!(!error.contains("native-test-private"));
        let tail = handle.stderr_tail.lock().expect("tail");
        assert!(tail.iter().any(|line| line.contains("omitido")));
        assert!(tail.iter().all(|line| line.chars().count() <= 480));
        assert!(tail.len() <= 24);
    }

    #[cfg(unix)]
    #[test]
    fn navigation_retry_reuses_only_the_local_bridge_and_is_reaped() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::Arc;
        use std::time::{Duration, Instant};

        let fixture = tempfile::tempdir().expect("fixture");
        let script = fixture.path().join("retry-browser.sh");
        let log = fixture.path().join("arguments.txt");
        let log_path = log.to_string_lossy().replace('\'', "'\\''");
        std::fs::write(
            &script,
            format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{log_path}'\nexit 0\n"),
        )
        .expect("retry browser script");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))
            .expect("script permissions");

        let target = "https://oauth.example.test/callback?code=secret";
        let (launch_url, redirect) = LoopbackRedirect::start(target).expect("bridge");
        let mut handle = shell_handle("sleep 30");
        handle.redirects = Arc::new(vec![redirect.clone()]);
        handle.navigation_retry = Some(Arc::new(super::NavigationRetry {
            executable: script,
            profile_dir: fixture.path().join("profile"),
            launch_url: launch_url.clone(),
        }));

        assert!(handle.retry_initial_navigation().expect("retry"));
        let deadline = Instant::now() + Duration::from_secs(1);
        while !log.exists() {
            assert!(Instant::now() < deadline, "retry process did not run");
            std::thread::sleep(Duration::from_millis(10));
        }
        let arguments = std::fs::read_to_string(&log).expect("retry arguments");
        assert!(arguments.contains(&launch_url));
        assert!(!arguments.contains("oauth.example.test"));
        handle.terminate().expect("original browser cleanup");
        redirect.stop();
    }
}

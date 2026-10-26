use super::*;

impl BrowserRuntime {
    pub fn shutdown_controlled(&self) -> Result<(), AppError> {
        #[cfg(any(unix, windows))]
        {
            let mut registry = self.controlled_sessions.lock();
            for session in registry.values() {
                let handle = &session.handle;
                #[cfg(any(target_os = "linux", windows))]
                if let Some(native) = &handle.native {
                    if handle.is_alive()? {
                        // Whole app exit requests normal native closure first,
                        // so session cookies can be committed by Chromium.
                        // Its owned process tree is the last-resort cleanup only.
                        let _ = native.close_browser();
                        if handle.wait_for_normal_exit()? {
                            continue;
                        }
                    }
                }
                let mut child = handle.child.lock().map_err(|_| {
                    AppError::Internal("Supervisor do navegador indisponível.".to_owned())
                })?;
                handle.supervisor.terminate(&mut child)?;
            }
            registry.clear();
        }
        Ok(())
    }
    #[cfg(unix)]
    pub fn launch_controlled(
        &self,
        profile_dir: &Path,
        url: &str,
        mail: Option<&str>,
    ) -> Result<BrowserHandle, AppError> {
        #[cfg(target_os = "linux")]
        {
            // Private JSONL/CDP fixtures retain their wire tests, not production OAuth.
            #[cfg(test)]
            if self
                .managed_executable()
                .is_some_and(|path| path.extension().is_some_and(|ext| ext == "py"))
            {
                return self.launch_cdp(profile_dir, url, mail);
            }
            self.launch_native(profile_dir, url, mail)
        }
        #[cfg(not(target_os = "linux"))]
        self.launch_cdp(profile_dir, url, mail)
    }

    #[cfg(all(unix, any(test, not(target_os = "linux"))))]
    fn launch_cdp(
        &self,
        profile_dir: &Path,
        url: &str,
        mail: Option<&str>,
    ) -> Result<BrowserHandle, AppError> {
        let mut registry = self.controlled_sessions.lock();
        if let Some(session) = registry.get_mut(profile_dir) {
            if session.handle.is_alive()? {
                session.close_pending()?;
                let pipe = session.handle.controller.as_ref().ok_or_else(|| {
                    AppError::Browser(
                        "Controle do perfil indisponível; janela preservada.".to_owned(),
                    )
                })?;
                let mut targets = Vec::new();
                if let Err(error) = pipe.create_targets_into(url, mail, &mut targets) {
                    if pipe.close_targets(&targets).is_err() {
                        session.pending_targets = targets;
                        return Err(AppError::Browser("Não foi possível abrir todas as abas. O aplicativo preservou as abas pendentes e tentará fechá-las antes da próxima conexão.".to_owned()));
                    }
                    return Err(error);
                }
                let mut existing = session.handle.clone();
                existing.owned_targets = targets;
                return Ok(existing);
            }
        }
        registry.remove(profile_dir);
        std::fs::create_dir_all(profile_dir)?;
        restrict_directory(profile_dir)?;
        startup::prepare_profile_startup(profile_dir)?;
        let executable = self
            .managed_executable()
            .filter(|path| path.is_file())
            .or_else(system_executable)
            .ok_or_else(|| AppError::Browser("Chromium não encontrado.".to_owned()))?;
        let mut command = Command::new(executable);
        configure_supervised_process(&mut command);
        command
            .arg(format!("--user-data-dir={}", profile_dir.display()))
            .args([
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-background-mode",
                "--disable-session-crashed-bubble",
                "--no-startup-window",
                "--remote-debugging-pipe",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let pipe = Arc::new(cdp::CdpPipe::prepare(&mut command)?);
        let (mut child, supervisor) = spawn_supervised(command)?;
        let supervisor = Arc::new(supervisor);
        let stderr_tail = Arc::new(Mutex::new(VecDeque::with_capacity(24)));
        if let Some(stderr) = child.stderr.take() {
            capture_stderr(stderr, stderr_tail.clone());
        }
        let mut handle = BrowserHandle {
            child: Arc::new(Mutex::new(child)),
            supervisor,
            stderr_tail,
            redirects: Arc::new(Vec::new()),
            navigation_retry: None,
            controller: Some(pipe.clone()),
            owned_targets: Vec::new(),
            #[cfg(target_os = "linux")]
            native: None,
            #[cfg(target_os = "linux")]
            native_operation: None,
        };
        let targets = pipe.create_operation_targets(url, mail);
        match targets {
            Ok(targets) => handle.owned_targets = targets,
            Err(error) => {
                let _ = handle.terminate();
                return Err(error);
            }
        }
        registry.insert(
            profile_dir.to_path_buf(),
            ControlledSession {
                handle: handle.clone(),
                pending_targets: Vec::new(),
                #[cfg(target_os = "linux")]
                pending_native_operations: Vec::new(),
            },
        );
        Ok(handle)
    }
    #[cfg(windows)]
    pub fn launch_controlled(
        &self,
        profile_dir: &Path,
        url: &str,
        mail: Option<&str>,
    ) -> Result<BrowserHandle, AppError> {
        self.launch_native(profile_dir, url, mail)
    }
}

#[cfg(any(target_os = "linux", windows))]
impl BrowserRuntime {
    fn launch_native(
        &self,
        profile_dir: &Path,
        url: &str,
        mail: Option<&str>,
    ) -> Result<BrowserHandle, AppError> {
        let mut registry = self.controlled_sessions.lock();
        if let Some(session) = registry.get_mut(profile_dir) {
            if session.handle.is_alive()? {
                session.close_pending()?;
            }
        }
        let alive = registry
            .get(profile_dir)
            .map(|session| session.handle.is_alive())
            .transpose()?
            .unwrap_or(false);
        if !alive {
            registry.remove(profile_dir);
            let handle = self.start_native_browser(profile_dir)?;
            registry.insert(
                profile_dir.into(),
                ControlledSession {
                    handle,
                    #[cfg(all(test, unix))]
                    pending_targets: Vec::new(),
                    pending_native_operations: Vec::new(),
                },
            );
        }
        let session = registry
            .get_mut(profile_dir)
            .ok_or_else(native_bridge::unavailable)?;
        let native = session
            .handle
            .native
            .as_ref()
            .ok_or_else(native_bridge::unavailable)?;
        let operation = uuid::Uuid::new_v4().simple().to_string();
        if let Err(error) = native.create_operation(&operation, url, mail) {
            // UUID reservation is retained even when creation/ack partially
            // succeeds. The browser's map, not a guessed UI object, owns cleanup.
            if native.close_operation(&operation).is_err() && session.handle.is_alive()? {
                session.pending_native_operations.push(operation);
            }
            return Err(error);
        }
        let mut handle = session.handle.clone();
        handle.native_operation = Some(operation);
        Ok(handle)
    }
    fn start_native_browser(&self, profile_dir: &Path) -> Result<BrowserHandle, AppError> {
        std::fs::create_dir_all(profile_dir)?;
        restrict_directory(profile_dir)?;
        startup::prepare_profile_startup(profile_dir)?;
        let extension = self
            .bundled_runtime_dir
            .as_deref()
            .and_then(Path::parent)
            .map(|root| root.join("browser-control"))
            .ok_or_else(native_bridge::unavailable)?;
        let host = std::env::current_exe()?
            .parent()
            .ok_or_else(native_bridge::unavailable)?
            .join(if cfg!(windows) {
                "codex-account-manager-cli.exe"
            } else {
                "codex-account-manager-cli"
            });
        #[cfg(test)]
        let extension = std::env::var_os("CODEX_MANAGER_TEST_NATIVE_EXTENSION")
            .map(PathBuf::from)
            .unwrap_or(extension);
        #[cfg(test)]
        let host = std::env::var_os("CODEX_MANAGER_TEST_NATIVE_HOST")
            .map(PathBuf::from)
            .unwrap_or(host);
        if !extension.join("manifest.json").is_file() {
            return Err(native_bridge::unavailable());
        }
        let pending = native_tabs::PendingNativeSession::prepare(profile_dir, &host)?;
        let executable = self
            .managed_executable()
            .filter(|path| path.is_file())
            .or_else(system_executable)
            .ok_or_else(|| AppError::Browser("Chromium não encontrado.".into()))?;
        let mut command = Command::new(executable);
        configure_supervised_process(&mut command);
        #[cfg(target_os = "linux")]
        pending.configure(&mut command);
        #[cfg(windows)]
        pending.configure(&mut command)?;
        command
            .arg(format!("--user-data-dir={}", profile_dir.display()))
            .arg(format!("--load-extension={}", extension.display()))
            .args([
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-background-mode",
                "--disable-session-crashed-bubble",
                "--new-window",
            ])
            .arg(format!("{}bootstrap.html", native_bridge::ORIGIN))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        #[cfg(test)]
        command.arg("--password-store=basic");
        let (mut child, supervisor) = spawn_supervised(command)?;
        let stderr_tail = Arc::new(Mutex::new(VecDeque::with_capacity(24)));
        if let Some(stderr) = child.stderr.take() {
            capture_stderr(stderr, stderr_tail.clone());
        }
        let native = match pending.connect(child.id()) {
            Ok(native) => Arc::new(native),
            Err(error) => {
                supervisor.terminate(&mut child)?;
                return Err(error);
            }
        };
        Ok(BrowserHandle {
            child: Arc::new(Mutex::new(child)),
            supervisor: Arc::new(supervisor),
            stderr_tail,
            #[cfg(test)]
            redirects: Arc::new(Vec::new()),
            #[cfg(test)]
            navigation_retry: None,
            #[cfg(all(test, unix))]
            controller: None,
            #[cfg(all(test, unix))]
            owned_targets: Vec::new(),
            native: Some(native),
            native_operation: None,
        })
    }
}

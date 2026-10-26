use std::collections::VecDeque;
#[cfg(test)]
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(test)]
use std::sync::mpsc::SyncSender;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tempfile::TempDir;

mod protocol;
mod stdio;
use crate::adapters::process_launcher::spawn_supervised;
use crate::adapters::process_supervisor::{configure_supervised_process, ProcessSupervisor};
use crate::domain::AccountSnapshot;
use crate::error::AppError;
use protocol::{
    account_update_matches_login, is_login_completion, notification_matches, optional_limits,
    parse_login_completion, parse_rate_limits, response_id, response_result, session_snapshot,
};
use stdio::{read_messages, read_stderr, FrameWriter};

const MAX_BUFFERED_MESSAGES: usize = 64;
const RATE_LIMITS_TIMEOUT: Duration = Duration::from_secs(3);
const BACKGROUND_USAGE_TIMEOUT: Duration = Duration::from_secs(12);
const ACCOUNT_UPDATED_TIMEOUT: Duration = Duration::from_secs(10);

pub struct AppServerClient {
    process: Child,
    supervisor: ProcessSupervisor,
    writer: FrameWriter,
    messages: Receiver<Value>,
    buffered: VecDeque<Value>,
    stderr_tail: Arc<std::sync::Mutex<VecDeque<String>>>,
    next_id: u64,
    cancel: Arc<AtomicBool>,
}

pub struct IsolatedCodexHome {
    pub directory: TempDir,
}

impl IsolatedCodexHome {
    pub fn create() -> Result<Self, AppError> {
        let directory = tempfile::Builder::new()
            .prefix("codex-account-manager-")
            .tempdir()?;
        restrict_directory(directory.path())?;
        let config = directory.path().join("config.toml");
        std::fs::write(config, "cli_auth_credentials_store = \"file\"\n")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                directory.path().join("config.toml"),
                std::fs::Permissions::from_mode(0o600),
            )?;
        }
        Ok(Self { directory })
    }
}

impl AppServerClient {
    /// Rebind only between completed serial queries, never while an RPC is active.
    pub(crate) fn set_query_cancellation(&mut self, cancel: Arc<AtomicBool>) {
        self.cancel = cancel;
    }
    pub(crate) fn send_application_message(&mut self, value: Value) -> Result<(), AppError> {
        self.send(value)
    }
    pub(crate) fn poll_application_message(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<Value>, AppError> {
        if let Some(value) = self.buffered.pop_front() {
            return Ok(Some(value));
        }
        self.receive_new(timeout)
    }
    pub fn pid(&self) -> u32 {
        self.process.id()
    }
    #[cfg(test)]
    pub fn spawn(codex_home: &Path) -> Result<Self, AppError> {
        Self::spawn_with_options(codex_home, Arc::new(AtomicBool::new(false)), false)
    }

    pub fn spawn_cancellable(codex_home: &Path, cancel: Arc<AtomicBool>) -> Result<Self, AppError> {
        Self::spawn_with_options(codex_home, cancel, false)
    }

    pub fn spawn_read_only(codex_home: &Path, cancel: Arc<AtomicBool>) -> Result<Self, AppError> {
        Self::spawn_with_options(codex_home, cancel, true)
    }

    fn spawn_with_options(
        codex_home: &Path,
        cancel: Arc<AtomicBool>,
        experimental: bool,
    ) -> Result<Self, AppError> {
        let binary = resolve_codex_binary().ok_or_else(|| {
            AppError::AppServer(
                "executável do Codex não encontrado; defina CODEX_MANAGER_CODEX_BINARY".to_owned(),
            )
        })?;
        Self::spawn_binary(codex_home, &binary, cancel, experimental)
    }

    #[cfg(test)]
    pub(crate) fn spawn_test_binary(
        codex_home: &Path,
        binary: &Path,
        cancel: Arc<AtomicBool>,
        experimental: bool,
    ) -> Result<Self, AppError> {
        Self::spawn_binary(codex_home, binary, cancel, experimental)
    }

    fn spawn_binary(
        codex_home: &Path,
        binary: &Path,
        cancel: Arc<AtomicBool>,
        experimental: bool,
    ) -> Result<Self, AppError> {
        let mut environment = inherited_environment();
        environment.insert(
            "CODEX_HOME".to_owned(),
            codex_home.to_string_lossy().into_owned(),
        );
        let mut command = Command::new(binary);
        command
            .arg("app-server")
            .arg("-c")
            .arg("cli_auth_credentials_store=\"file\"")
            .arg("--listen")
            .arg("stdio://")
            .env_clear()
            .envs(environment)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        configure_supervised_process(&mut command);
        let (mut process, supervisor) = spawn_supervised(command)?;
        let stdin = match process.stdin.take() {
            Some(stdin) => stdin,
            None => {
                terminate_spawned_process(&supervisor, &mut process);
                return Err(AppError::AppServer(
                    "stdin do App Server indisponível".to_owned(),
                ));
            }
        };
        let stdout = match process.stdout.take() {
            Some(stdout) => stdout,
            None => {
                terminate_spawned_process(&supervisor, &mut process);
                return Err(AppError::AppServer(
                    "stdout do App Server indisponível".to_owned(),
                ));
            }
        };
        let stderr = match process.stderr.take() {
            Some(stderr) => stderr,
            None => {
                terminate_spawned_process(&supervisor, &mut process);
                return Err(AppError::AppServer(
                    "stderr do App Server indisponível".to_owned(),
                ));
            }
        };
        let (sender, messages) = mpsc::sync_channel(32);
        let stderr_tail = Arc::new(std::sync::Mutex::new(VecDeque::new()));
        let reader_tail = stderr_tail.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("codex-app-server-reader".to_owned())
            .spawn(move || read_messages(stdout, sender, reader_tail))
        {
            terminate_spawned_process(&supervisor, &mut process);
            return Err(AppError::AppServer(error.to_string()));
        }
        let stderr_tail_reader = stderr_tail.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("codex-app-server-stderr".to_owned())
            .spawn(move || read_stderr(stderr, stderr_tail_reader))
        {
            terminate_spawned_process(&supervisor, &mut process);
            return Err(AppError::AppServer(error.to_string()));
        }
        let writer = match FrameWriter::new(stdin) {
            Ok(writer) => writer,
            Err(error) => {
                terminate_spawned_process(&supervisor, &mut process);
                return Err(error);
            }
        };
        let mut client = Self {
            process,
            supervisor,
            writer,
            messages,
            buffered: VecDeque::new(),
            stderr_tail,
            next_id: 1,
            cancel,
        };
        client.request(
            "initialize",
            Some(json!({
                "clientInfo": {
                    "name": "codex-account-manager",
                    "title": "Codex Account Manager",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": { "experimentalApi": experimental }
            })),
            Duration::from_secs(15),
        )?;
        client.notify("initialized", None)?;
        Ok(client)
    }

    pub fn set_read_only_session(&mut self, document: &Value) -> Result<(), AppError> {
        self.set_access_session(&crate::adapters::auth_store::access_session(
            document, None,
        )?)
    }

    pub fn set_access_session(
        &mut self,
        session: &crate::ports::AccessSession,
    ) -> Result<(), AppError> {
        self.request(
            "account/login/start",
            Some(json!({
                "type": "chatgptAuthTokens",
                "accessToken": session.access_token,
                "chatgptAccountId": session.account_id,
            })),
            Duration::from_secs(5),
        )?;
        Ok(())
    }

    pub fn read_usage(&mut self) -> Result<AccountSnapshot, AppError> {
        let limits = self.request("account/rateLimits/read", None, BACKGROUND_USAGE_TIMEOUT)?;
        Ok(AccountSnapshot {
            email: None,
            plan_type: None,
            rate_limits: parse_rate_limits(&limits),
        })
    }

    pub fn reset_read_only_notifications(&mut self) {
        self.buffered.clear();
    }

    pub fn start_login(&mut self) -> Result<(String, String), AppError> {
        let result = self.request(
            "account/login/start",
            Some(json!({ "type": "chatgpt", "useHostedLoginSuccessPage": true, "appBrand": "codex" })),
            Duration::from_secs(20),
        )?;
        let login_id = result
            .get("loginId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| AppError::AppServer("o App Server não retornou loginId".to_owned()))?;
        let auth_url = result
            .get("authUrl")
            .and_then(Value::as_str)
            .filter(|value| value.starts_with("https://"))
            .ok_or_else(|| {
                AppError::AppServer("o App Server não retornou uma URL OAuth válida".to_owned())
            })?;
        Ok((login_id.to_owned(), auth_url.to_owned()))
    }

    pub fn wait_for_login(
        &mut self,
        login_id: &str,
        cancel: &Arc<std::sync::atomic::AtomicBool>,
        timeout: Option<Duration>,
        browser_alive: impl Fn() -> Result<bool, AppError>,
    ) -> Result<(), AppError> {
        self.wait_for_login_with_update_timeout(
            login_id,
            cancel,
            timeout,
            browser_alive,
            ACCOUNT_UPDATED_TIMEOUT,
        )
    }

    fn wait_for_login_with_update_timeout(
        &mut self,
        login_id: &str,
        cancel: &Arc<std::sync::atomic::AtomicBool>,
        timeout: Option<Duration>,
        browser_alive: impl Fn() -> Result<bool, AppError>,
        account_updated_timeout: Duration,
    ) -> Result<(), AppError> {
        // The user explicitly owns the duration of an interactive login. A
        // timeout is still supported for callers that need a bounded worker,
        // but the desktop flow passes None and waits until cancellation or
        // the browser closing.
        let deadline = timeout.map(|duration| Instant::now() + duration);
        loop {
            if cancel.load(std::sync::atomic::Ordering::Acquire) {
                let _ = self.cancel_login(login_id);
                return Err(AppError::Cancelled);
            }
            let remaining = match deadline {
                Some(deadline) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        let _ = self.cancel_login(login_id);
                        return Err(AppError::TimedOut);
                    }
                    remaining
                }
                None => Duration::from_secs(1),
            };
            if let Some(message) =
                self.take_buffered(|message| is_login_completion(message, login_id))
            {
                parse_login_completion(&message)?;
                break;
            }
            if let Some(message) = self.receive_new(remaining.min(Duration::from_secs(1)))? {
                if is_login_completion(&message, login_id) {
                    parse_login_completion(&message)?;
                    break;
                }
                self.buffer_message(message);
            }
            // Drain a matching completion before treating browser closure as
            // cancellation. A provider can close its window after a valid
            // callback; that physical event cannot undo an already received fact.
            if !browser_alive()? {
                let _ = self.cancel_login(login_id);
                return Err(AppError::Cancelled);
            }
        }
        match self.wait_notification(
            "account/updated",
            account_update_matches_login,
            cancel,
            &|| Ok(true),
            Some(account_updated_timeout),
        ) {
            Ok(()) | Err(AppError::TimedOut) => {}
            Err(error) => return Err(error),
        }
        Ok(())
    }

    fn read_account(&mut self, refresh: bool) -> Result<Value, AppError> {
        self.request(
            "account/read",
            Some(json!({ "refreshToken": refresh })),
            Duration::from_secs(15),
        )
    }

    /// The read-only preflight intentionally does not depend on quota availability.
    pub fn validate_access_identity(&mut self) -> Result<AccountSnapshot, AppError> {
        let account = self.read_account(false)?;
        session_snapshot(&account, &Value::Null)
    }

    pub fn validate_session_with_trace(
        &mut self,
        trace: &mut dyn FnMut(&'static str, Duration),
    ) -> Result<AccountSnapshot, AppError> {
        self.validate_session(false, trace)
    }

    pub fn validate_stored_session_with_trace(
        &mut self,
        trace: &mut dyn FnMut(&'static str, Duration),
    ) -> Result<AccountSnapshot, AppError> {
        self.validate_session(true, trace)
    }

    fn validate_session(
        &mut self,
        refresh: bool,
        trace: &mut dyn FnMut(&'static str, Duration),
    ) -> Result<AccountSnapshot, AppError> {
        let account_response = measure(trace, "app_server_account_read", || {
            self.read_account(refresh)
        })?;
        session_snapshot(&account_response, &Value::Null)?;
        let limits_started = Instant::now();
        let limits_response = self.request("account/rateLimits/read", None, RATE_LIMITS_TIMEOUT);
        let limits_stage = if limits_response.is_ok() {
            "app_server_rate_limits_read"
        } else {
            "app_server_rate_limits_unavailable"
        };
        trace(limits_stage, limits_started.elapsed());

        // A quota lookup is dashboard metadata, not proof of identity. Keep a
        // valid account switch usable when that independent endpoint is slow;
        // SQLite preserves the last successful quota sample when this is None.
        let limits_response = optional_limits(limits_response)?;
        measure(trace, "app_server_snapshot_parse", || {
            session_snapshot(&account_response, &limits_response)
        })
    }

    pub fn cancel_login(&mut self, login_id: &str) -> Result<(), AppError> {
        // Cancellation is best-effort because the worker is about to tear down
        // the isolated App Server. Waiting for a response here used to make the
        // Cancel button appear frozen for up to five seconds.
        let id = self.next_id;
        self.next_id += 1;
        let _ = self.send(json!({ "method": "account/login/cancel", "id": id,
            "params": { "loginId": login_id } }));
        Ok(())
    }

    fn notify(&mut self, method: &str, params: Option<Value>) -> Result<(), AppError> {
        let mut message = serde_json::Map::new();
        message.insert("method".to_owned(), Value::String(method.to_owned()));
        if let Some(params) = params {
            message.insert("params".to_owned(), params);
        }
        self.send(Value::Object(message))
    }

    fn request(
        &mut self,
        method: &str,
        params: Option<Value>,
        timeout: Duration,
    ) -> Result<Value, AppError> {
        let id = self.next_id;
        self.next_id += 1;
        let mut message = serde_json::Map::new();
        message.insert("method".to_owned(), Value::String(method.to_owned()));
        message.insert("id".to_owned(), Value::Number(id.into()));
        if let Some(params) = params {
            message.insert("params".to_owned(), params);
        }
        let deadline = Instant::now() + timeout;
        self.send(Value::Object(message))?;
        loop {
            if self.cancel.load(Ordering::Acquire) {
                return Err(AppError::Cancelled);
            }
            if let Some(message) = self.take_buffered(|message| response_id(message) == Some(id)) {
                return response_result(message);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(AppError::AppServer(format!(
                    "a chamada {method} excedeu o tempo limite"
                )));
            }
            let Some(message) = self.receive_new(remaining.min(Duration::from_millis(100)))? else {
                continue;
            };
            if message.get("method").and_then(Value::as_str)
                == Some("account/chatgptAuthTokens/refresh")
            {
                // Quota sampling must never rotate a refresh token owned by a
                // running Desktop or another interface. Only access tokens
                // enter this mode; a fresh login/switch remains their owner.
                self.send(json!({ "id": message.get("id"), "error": {
                    "code": -32000, "message": "Read-only quota client cannot refresh credentials"
                }}))?;
                continue;
            }
            if response_id(&message) == Some(id) {
                return response_result(message);
            }
            self.buffer_message(message);
        }
    }

    fn wait_notification(
        &mut self,
        method: &str,
        predicate: impl Fn(&serde_json::Map<String, Value>) -> bool,
        cancel: &Arc<std::sync::atomic::AtomicBool>,
        browser_alive: &impl Fn() -> Result<bool, AppError>,
        timeout: Option<Duration>,
    ) -> Result<(), AppError> {
        let deadline = timeout.map(|duration| Instant::now() + duration);
        loop {
            if cancel.load(std::sync::atomic::Ordering::Acquire) {
                return Err(AppError::Cancelled);
            }
            if !browser_alive()? {
                return Err(AppError::Cancelled);
            }
            if self
                .take_buffered(|message| notification_matches(message, method, &predicate))
                .is_some()
            {
                return Ok(());
            }
            let remaining = match deadline {
                Some(deadline) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(AppError::TimedOut);
                    }
                    remaining
                }
                None => Duration::from_secs(1),
            };
            let Some(message) = self.receive_new(remaining.min(Duration::from_secs(1)))? else {
                continue;
            };
            if notification_matches(&message, method, &predicate) {
                return Ok(());
            }
            self.buffer_message(message);
        }
    }

    fn receive_new(&self, timeout: Duration) -> Result<Option<Value>, AppError> {
        match self.messages.recv_timeout(timeout) {
            Ok(message) => Ok(Some(message)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => {
                Err(AppError::AppServer(self.reader_failure_message()))
            }
        }
    }

    fn reader_failure_message(&self) -> String {
        let detail = self
            .stderr_tail
            .lock()
            .ok()
            .and_then(|lines| lines.back().cloned());
        detail.map_or_else(
            || "o leitor do App Server foi encerrado".to_owned(),
            |line| format!("o App Server foi encerrado: {line}"),
        )
    }

    fn take_buffered(&mut self, predicate: impl Fn(&Value) -> bool) -> Option<Value> {
        let position = self.buffered.iter().position(predicate)?;
        self.buffered.remove(position)
    }

    fn buffer_message(&mut self, message: Value) {
        if self.buffered.len() == MAX_BUFFERED_MESSAGES {
            self.buffered.pop_front();
        }
        self.buffered.push_back(message);
    }

    fn send(&mut self, value: Value) -> Result<(), AppError> {
        self.writer.send(&value, &self.cancel)
    }
}

fn measure<T>(
    trace: &mut dyn FnMut(&'static str, Duration),
    stage: &'static str,
    operation: impl FnOnce() -> Result<T, AppError>,
) -> Result<T, AppError> {
    let started = Instant::now();
    let result = operation();
    trace(stage, started.elapsed());
    result
}

pub(crate) fn inherited_environment() -> std::collections::HashMap<String, String> {
    const ALLOWED: &[&str] = &[
        "PATH",
        "HOME",
        "USERPROFILE",
        "LOCALAPPDATA",
        "APPDATA",
        "TEMP",
        "TMP",
        "SYSTEMROOT",
        "WINDIR",
        "LANG",
        "LC_ALL",
        "XDG_RUNTIME_DIR",
        "DBUS_SESSION_BUS_ADDRESS",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "NO_PROXY",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
    ];
    ALLOWED
        .iter()
        .filter_map(|key| {
            std::env::var(key)
                .ok()
                .map(|value| ((*key).to_owned(), value))
        })
        .collect()
}

impl Drop for AppServerClient {
    fn drop(&mut self) {
        let _ = self.supervisor.terminate(&mut self.process);
        let _ = self.process.try_wait();
    }
}

fn terminate_spawned_process(supervisor: &ProcessSupervisor, process: &mut Child) {
    let _ = supervisor.terminate(process);
    let _ = process.try_wait();
}

pub(crate) fn resolve_codex_binary() -> Option<PathBuf> {
    if let Ok(value) = std::env::var("CODEX_MANAGER_CODEX_BINARY") {
        let path = PathBuf::from(value);
        if path.is_file() {
            return Some(path);
        }
    }
    which::which("codex").ok()
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
mod tests;

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use chrono::Utc;
use parking_lot::Mutex;
use serde_json::json;

use super::{
    ensure_login_active, mask_id, validate_mail_url, ManagerCore, OperationControl,
    RetainedLoginBrowser, BROWSER_INITIAL_NAVIGATION_OBSERVATION,
    BROWSER_RETRY_NAVIGATION_OBSERVATION,
};
use crate::adapters::app_server::{AppServerClient, IsolatedCodexHome};
use crate::adapters::auth_store;
use crate::adapters::browser_runtime::BrowserHandle;
use crate::adapters::logging;
use crate::domain::{
    AccountMetadata, AccountSnapshot, LoginOperation, ManagedProfile, OperationKind,
    OperationState, ProfileStatus,
};
use crate::error::AppError;

pub(super) struct LoginTask {
    pub(super) operation_id: String,
    pub(super) profile_id: String,
    requested_mail_url: Option<String>,
    application_session: bool,
    pub(super) control: Arc<OperationControl>,
}

struct PreparedLogin {
    // Drop the child before removing its temporary home.
    client: AppServerClient,
    home: IsolatedCodexHome,
    profile: ManagedProfile,
}

struct LoginBrowser {
    handle: BrowserHandle,
    login_id: String,
    had_mail_tab: bool,
    started: Instant,
}

pub(super) struct VerifiedLogin {
    pub(super) document: serde_json::Value,
    pub(super) account_id: String,
    pub(super) snapshot: AccountSnapshot,
}

struct LoginDestination {
    profile_id: String,
    provider_id: Option<String>,
}

impl ManagerCore {
    pub fn start_login(
        &self,
        profile_id: &str,
        reconnect: bool,
        mail_url: Option<&str>,
    ) -> Result<LoginOperation, AppError> {
        validate_mail_url(mail_url)?;
        let kind = if reconnect {
            OperationKind::Reconnect
        } else {
            OperationKind::Login
        };
        self.start_login_with_kind(profile_id, kind, mail_url)
    }

    pub(super) fn start_login_with_kind(
        &self,
        profile_id: &str,
        kind: OperationKind,
        mail_url: Option<&str>,
    ) -> Result<LoginOperation, AppError> {
        self.start_login_purpose(profile_id, kind, mail_url, false)
    }

    pub fn start_application_login(&self, profile_id: &str) -> Result<LoginOperation, AppError> {
        let profile = self.database.profile(profile_id)?;
        if profile.account_id.is_none() {
            return Err(AppError::Validation(
                "Associe a conta ao perfil antes de conectar para aplicações.".to_owned(),
            ));
        }
        let remote = profile
            .account_id
            .as_deref()
            .expect("associated account checked above");
        let (local, _, _, _) = self.database.account_integration_options(remote)?;
        // Preserve legacy dedicated sessions; otherwise reconnect the actual account,
        // not another OAuth chain solely because an application will use it.
        let dedicated = self.database.application_session(&local)?.is_some();
        self.start_login_purpose(profile_id, OperationKind::Reconnect, None, dedicated)
    }

    fn start_login_purpose(
        &self,
        profile_id: &str,
        kind: OperationKind,
        mail_url: Option<&str>,
        application_session: bool,
    ) -> Result<LoginOperation, AppError> {
        let task_lease = self.tasks.enter()?;
        let _admission = crate::application::operation_gate::acquire(
            &self.account_admission,
            &self.maintenance.cancel,
            Duration::from_secs(30),
        )?;
        let profile = self.database.profile(profile_id)?;
        if !matches!(profile.status, ProfileStatus::Active) {
            return Err(AppError::Validation(
                "somente perfis ativos podem ser conectados".to_owned(),
            ));
        }
        if application_session {
            if let Some(remote) = profile.account_id.as_deref() {
                let (local, _, _, _) = self.database.account_integration_options(remote)?;
                if self.executions.account_busy(&local) {
                    return Err(AppError::Busy(
                        "A sessão está gerando; aguarde ou cancele o trabalho antes de reconectar."
                            .to_owned(),
                    ));
                }
            }
        }
        self.reserve_operation(profile_id, kind)?;
        let operation = match self.database.create_typed_operation(profile_id, kind) {
            Ok(operation) => operation,
            Err(error) => {
                self.release_operation_reservation(profile_id);
                return Err(error);
            }
        };
        let control = Arc::new(OperationControl {
            profile_id: profile_id.to_owned(),
            kind: operation.kind,
            cancel: Arc::new(AtomicBool::new(false)),
            cancelable: AtomicBool::new(true),
            cancellation_gate: Mutex::new(()),
            browser: Arc::new(Mutex::new(None)),
            replacement: Mutex::new(None),
            confirmed: AtomicBool::new(false),
        });
        self.install_operation_control(&operation.id, control.clone());

        let manager = self.clone();
        let worker_operation_id = operation.id.clone();
        let worker_profile_id = profile_id.to_owned();
        let requested_mail_url = mail_url.map(str::to_owned);
        let worker = thread::Builder::new()
            .name(format!("login-operation-{}", operation.id))
            .spawn(move || {
                let _task_lease = task_lease;
                manager.run_login(LoginTask {
                    operation_id: worker_operation_id,
                    profile_id: worker_profile_id,
                    requested_mail_url,
                    application_session,
                    control,
                });
            });
        if let Err(error) = worker {
            self.operations.lock().controls.remove(&operation.id);
            let message = "Não foi possível iniciar a operação em segundo plano";
            let _ = self.finish_operation(
                &operation.id,
                profile_id,
                OperationState::Failed,
                message,
                Some("thread_spawn_failed"),
            );
            return Err(AppError::Internal(error.to_string()));
        }
        Ok(operation)
    }

    fn run_login(&self, task: LoginTask) {
        let result = self.execute_login(&task);
        let cleanup = self.close_login_browser(&task, result.as_ref().err());
        match result {
            Ok(canonical_profile_id) => {
                self.complete_login(&task, &canonical_profile_id, cleanup.as_ref());
            }
            Err(error) => self.fail_login(&task, error, cleanup.as_ref()),
        }
        self.operations.lock().controls.remove(&task.operation_id);
        self.maintenance.wake.notify_all();
    }

    fn complete_login(
        &self,
        task: &LoginTask,
        canonical_profile_id: &str,
        cleanup: Option<&AppError>,
    ) {
        if canonical_profile_id != task.profile_id && cleanup.is_none() {
            let _ = self.database.discard_unconnected_profile(&task.profile_id);
        }
        let _ = self.finish_operation(
            &task.operation_id,
            canonical_profile_id,
            OperationState::Completed,
            if cleanup.is_some() {
                "Conta conectada; o fechamento do navegador ainda não foi confirmado"
            } else {
                "Conta conectada"
            },
            cleanup.map(|_| "browser_cleanup_pending"),
        );
        self.emit_dashboard_changed();
    }

    fn fail_login(&self, task: &LoginTask, error: AppError, cleanup: Option<&AppError>) {
        let state = match error {
            AppError::Cancelled => OperationState::Cancelled,
            AppError::TimedOut => OperationState::TimedOut,
            _ => OperationState::Failed,
        };
        let mut message = logging::sanitize_error(&error.to_string());
        if cleanup.is_some() {
            message.push_str("; o fechamento do navegador ainda não foi confirmado");
        }
        if !matches!(state, OperationState::Cancelled) {
            let _ = self
                .database
                .update_profile_error(&task.profile_id, &message);
        }
        let _ = self.finish_operation(
            &task.operation_id,
            &task.profile_id,
            state,
            &message,
            Some(if cleanup.is_some() {
                "browser_cleanup_pending"
            } else {
                error.code()
            }),
        );
    }

    fn close_login_browser(
        &self,
        task: &LoginTask,
        failure: Option<&AppError>,
    ) -> Option<AppError> {
        let browser = task.control.browser.lock().as_ref().cloned()?;
        if failure.is_some_and(|error| matches!(error, AppError::AccountMismatch(_))) {
            self.retained_login_browsers.lock().insert(
                task.profile_id.clone(),
                RetainedLoginBrowser {
                    operation_id: task.operation_id.clone(),
                    handle: browser,
                },
            );
            return None;
        }
        match browser.terminate() {
            Ok(()) => None,
            Err(error) => {
                let _ = logging::record(
                    &self.database,
                    Some(&task.operation_id),
                    Some(&task.profile_id),
                    "browser_cleanup_pending",
                    json!({"errorCode":error.code(),"operationErrorCode":failure.map(AppError::code),"operationMessage":failure.map(|cause| logging::sanitize_error(&cause.to_string()))}),
                );
                self.retained_login_browsers.lock().insert(
                    task.profile_id.clone(),
                    RetainedLoginBrowser {
                        operation_id: task.operation_id.clone(),
                        handle: browser,
                    },
                );
                Some(error)
            }
        }
    }

    pub fn retry_browser_cleanup(&self, operation_id: &str) -> Result<(), AppError> {
        let operation = self.database.operation(operation_id)?;
        if !operation.state.is_terminal()
            || operation.error_code.as_deref() != Some("browser_cleanup_pending")
        {
            return Err(AppError::AccessDenied(
                "O atalho exige uma operação encerrada com fechamento pendente.".to_owned(),
            ));
        }
        let pid = operation.browser_pid.ok_or_else(|| {
            AppError::AccessDenied("A operação não identifica um navegador próprio.".to_owned())
        })?;
        self.with_profile_mutation(&operation.profile_id, || {
            let retained = self.retained_login_browsers.lock().iter().find_map(|(profile, browser)| {
                (browser.operation_id == operation.id && browser.handle.pid().ok() == Some(pid)).then(|| (profile.clone(), browser.handle.clone()))
            }).ok_or_else(|| AppError::Browser("A janela desta operação não está mais sob controle; nenhuma outra janela foi fechada.".to_owned()))?;
            retained.1.terminate()?;
            self.retained_login_browsers.lock().remove(&retained.0);
            if retained.0 != operation.profile_id { self.database.discard_unconnected_profile(&retained.0)?; }
            let step = match operation.state {
                OperationState::Completed => "Conta conectada; navegador fechado",
                OperationState::Cancelled => "Operação cancelada; navegador fechado",
                _ => "Navegador fechado; a operação original não foi concluída",
            };
            self.finish_operation(&operation.id, &operation.profile_id, operation.state, step, None)
        })
    }

    fn execute_login(&self, task: &LoginTask) -> Result<String, AppError> {
        let mut prepared = self.prepare_login_session(task)?;
        let browser = self.open_login_browser(task, &mut prepared)?;
        let mut verified = self.authenticate_login(task, &mut prepared, &browser)?;
        let destination = self.login_destination(task, &prepared.profile, &verified)?;
        if task.control.kind == OperationKind::Replace {
            self.await_replacement_confirmation(
                task,
                &prepared.profile,
                &verified,
                &browser.handle,
            )?;
            verified = self.revalidate_replacement(task, &mut prepared, &verified.account_id)?;
        }
        self.configure_authenticated_mail(task, &prepared.profile, &browser, &destination)?;
        self.persist_login(task, &browser, &destination, &verified)?;
        self.record_saved_credential(task, &destination, &verified);
        Ok(destination.profile_id)
    }

    fn prepare_login_session(&self, task: &LoginTask) -> Result<PreparedLogin, AppError> {
        self.transition_login(
            task,
            OperationState::Preparing,
            "Preparando o ambiente isolado",
        )?;
        ensure_login_active(&task.control, None)?;
        let retained = self.retained_login_browsers.lock().remove(&task.profile_id);
        if let Some(browser) = retained {
            browser.handle.terminate()?;
        }
        #[cfg(not(unix))]
        self.close_previous_browser(&task.operation_id, &task.profile_id, &task.control)?;
        ensure_login_active(&task.control, None)?;
        self.prepare_login_runtime(&task.operation_id, &task.profile_id)?;
        let profile = self.database.profile(&task.profile_id)?;
        let home = self.measure_stage(
            task,
            "create_isolated_codex_home",
            IsolatedCodexHome::create,
        )?;
        let client = self.measure_stage(task, "app_server_spawn_initialize", || {
            #[cfg(test)]
            if let Some(binary) = self.login_test_binary.as_ref() {
                return AppServerClient::spawn_test_binary(
                    home.directory.path(),
                    binary,
                    task.control.cancel.clone(),
                    false,
                );
            }
            AppServerClient::spawn_cancellable(home.directory.path(), task.control.cancel.clone())
        })?;
        logging::record(
            &self.database,
            Some(&task.operation_id),
            Some(&task.profile_id),
            "owned_app_server_started",
            json!({"pid": client.pid(), "purpose":"isolated_login"}),
        )?;
        Ok(PreparedLogin {
            client,
            home,
            profile,
        })
    }

    fn open_login_browser(
        &self,
        task: &LoginTask,
        prepared: &mut PreparedLogin,
    ) -> Result<LoginBrowser, AppError> {
        let (login_id, auth_url) = self.measure_stage(task, "app_server_start_login", || {
            prepared.client.start_login()
        })?;
        self.transition_login(
            task,
            OperationState::BrowserLaunching,
            "Abrindo OAuth e webmail",
        )?;
        let mail_enabled = prepared
            .profile
            .account_id
            .as_deref()
            .and_then(|id| self.database.account_integration_options(id).ok())
            .is_some_and(|(_, _, _, mail)| mail);
        let mail_url = if task.control.kind == OperationKind::Replace || !mail_enabled {
            None
        } else {
            task.requested_mail_url
                .clone()
                .or_else(|| self.mail_url_for_profile(&prepared.profile).ok().flatten())
        };
        let started = Instant::now();
        let handle = self.measure_stage(task, "browser_process_launch", || {
            self.browser.launch_controlled(
                PathBuf::from(&prepared.profile.data_dir).as_path(),
                &auth_url,
                mail_url.as_deref(),
            )
        })?;
        *task.control.browser.lock() = Some(handle.clone());
        self.record_launched_browser(&task.operation_id, &task.profile_id, &handle)?;
        Ok(LoginBrowser {
            handle,
            login_id,
            had_mail_tab: mail_url.is_some(),
            started,
        })
    }

    fn authenticate_login(
        &self,
        task: &LoginTask,
        prepared: &mut PreparedLogin,
        browser: &LoginBrowser,
    ) -> Result<VerifiedLogin, AppError> {
        self.transition_login(
            task,
            OperationState::AwaitingLogin,
            "Aguardando login no navegador",
        )?;
        let observation = std::cell::RefCell::new(NavigationObservation::new(browser.started));
        prepared
            .client
            .wait_for_login(&browser.login_id, &task.control.cancel, None, || {
                browser.handle.ensure_alive()?;
                observation
                    .borrow_mut()
                    .poll(&self.database, task, &browser.handle);
                Ok(true)
            })?;
        // OAuth completion is confirmed. Only explicit cancellation can now
        // stop validation/persistence; a browser window may already have closed.
        ensure_login_active(&task.control, None)?;
        self.transition_login(
            task,
            OperationState::Verifying,
            "Validando a sessão autenticada",
        )?;
        let snapshot = self.validate_authenticated_session(
            &task.operation_id,
            &task.profile_id,
            &mut prepared.client,
        )?;
        ensure_login_active(&task.control, None)?;
        let document = auth_store::read_auth(&prepared.home.directory.path().join("auth.json"))?;
        let account_id = auth_store::validate_auth_document(&document, None)?;
        if task.control.kind != OperationKind::Replace {
            validate_login_identity(&prepared.profile, &snapshot, &account_id)?;
        }
        Ok(VerifiedLogin {
            document,
            account_id,
            snapshot,
        })
    }

    fn login_destination(
        &self,
        task: &LoginTask,
        profile: &ManagedProfile,
        verified: &VerifiedLogin,
    ) -> Result<LoginDestination, AppError> {
        if task.control.kind == OperationKind::Replace {
            self.validate_replacement_identity(profile, &verified.account_id)?;
        }
        let profile_id = self.canonical_profile_id(&task.profile_id, &verified.account_id)?;
        if profile_id != task.profile_id && profile.account_id.is_some() {
            let existing = self.database.profile(&profile_id)?;
            return Err(AppError::AccountMismatch(format!(
                "O navegador entrou na conta do perfil ‘{}’. Este perfil espera outra conta. Saia da conta na janela aberta e tente novamente.", existing.name)));
        }
        let provider_id = verified
            .snapshot
            .email
            .as_deref()
            .and_then(|email| self.resolve_provider_id(email));
        Ok(LoginDestination {
            profile_id,
            provider_id,
        })
    }

    fn revalidate_replacement(
        &self,
        task: &LoginTask,
        prepared: &mut PreparedLogin,
        expected_id: &str,
    ) -> Result<VerifiedLogin, AppError> {
        ensure_login_active(&task.control, None)?;
        let snapshot = self.validate_authenticated_session(
            &task.operation_id,
            &task.profile_id,
            &mut prepared.client,
        )?;
        let document = auth_store::read_auth(&prepared.home.directory.path().join("auth.json"))?;
        let account_id = auth_store::validate_auth_document(&document, Some(expected_id))?;
        Ok(VerifiedLogin {
            document,
            account_id,
            snapshot,
        })
    }

    fn configure_authenticated_mail(
        &self,
        task: &LoginTask,
        profile: &ManagedProfile,
        browser: &LoginBrowser,
        destination: &LoginDestination,
    ) -> Result<(), AppError> {
        // A URL saved while OAuth was open wins over the pre-launch snapshot.
        let current = self.database.profile(&task.profile_id)?;
        let mail_enabled = current
            .account_id
            .as_deref()
            .and_then(|id| self.database.account_integration_options(id).ok())
            .is_some_and(|(_, _, _, mail)| mail);
        if !mail_enabled {
            return Ok(());
        }
        let url = (task.control.kind != OperationKind::Replace)
            .then_some(current.mail_url.clone())
            .flatten()
            .clone()
            .or_else(|| task.requested_mail_url.clone())
            .or_else(|| self.provider_mail_url(destination.provider_id.as_deref()));
        if destination.profile_id != task.profile_id && current.mail_url.is_some() {
            self.database
                .update_profile_mail_url(&destination.profile_id, current.mail_url.as_deref())?;
        }
        if !browser.had_mail_tab {
            if let Some(url) = url.as_deref() {
                let _ = self
                    .browser
                    .open_tab(PathBuf::from(&profile.data_dir).as_path(), url);
            }
        }
        if destination.profile_id != task.profile_id {
            self.database
                .record_browser_opened(&destination.profile_id)?;
        }
        Ok(())
    }

    fn persist_login(
        &self,
        task: &LoginTask,
        browser: &LoginBrowser,
        destination: &LoginDestination,
        verified: &VerifiedLogin,
    ) -> Result<(), AppError> {
        let _guard = crate::application::operation_gate::acquire(
            &self.switch_lock,
            &task.control.cancel,
            Duration::from_secs(30),
        )?;
        ensure_login_active(&task.control, Some(&browser.handle))?;
        self.transition_login(
            task,
            OperationState::Persisting,
            "Gravando a credencial no Keyring",
        )?;
        // Once committing, closing a browser cannot invalidate the identity
        // already verified. Cancellation was decided before this boundary.
        if task.control.kind == OperationKind::Replace {
            return self.commit_replacement(task, verified, destination.provider_id.as_deref());
        }
        if task.application_session {
            let (id, _, _, _) = self
                .database
                .account_integration_options(&verified.account_id)?;
            let session = self
                .database
                .application_session(&id)?
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            self.database.save_application_session(&id, &session)?;
            // Register ownership before the coffer write so a failed initial
            // commit has a recoverable owner, not an orphaned encrypted record.
            self.save_application_credential(&session, &verified.account_id, &verified.document)?;
            return Ok(());
        }
        let previous = self.credentials.load_optional(&verified.account_id)?;
        self.credentials.save(&verified.document)?;
        let result = self.database.persist_connected_account(
            &destination.profile_id,
            AccountMetadata {
                account_id: &verified.account_id,
                email: verified.snapshot.email.as_deref(),
                plan_type: verified.snapshot.plan_type.as_deref(),
                credential_ref: Some("native-keyring"),
                rate_limits: verified.snapshot.rate_limits.as_ref(),
                observed_at: Utc::now(),
            },
            destination.provider_id.as_deref(),
        );
        if let Err(error) = result {
            self.restore_credential(&verified.account_id, previous.as_ref())?;
            return Err(error);
        }
        Ok(())
    }

    fn record_saved_credential(
        &self,
        task: &LoginTask,
        destination: &LoginDestination,
        verified: &VerifiedLogin,
    ) {
        let _ = logging::record(
            &self.database,
            Some(&task.operation_id),
            Some(&destination.profile_id),
            "credential_persisted",
            json!({ "account": mask_id(&verified.account_id),
                "emailKnown": verified.snapshot.email.is_some(), "deduplicated": destination.profile_id != task.profile_id }),
        );
    }

    fn transition_login(
        &self,
        task: &LoginTask,
        state: OperationState,
        step: &str,
    ) -> Result<(), AppError> {
        self.transition(&task.operation_id, &task.profile_id, state, step)
    }

    fn measure_stage<T>(
        &self,
        task: &LoginTask,
        stage: &'static str,
        operation: impl FnOnce() -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        self.measure_login_stage(&task.operation_id, &task.profile_id, stage, operation)
    }

    fn prepare_login_runtime(&self, operation_id: &str, profile_id: &str) -> Result<(), AppError> {
        self.measure_login_stage(operation_id, profile_id, "browser_runtime_ready", || {
            self.ensure_managed_browser(operation_id, profile_id)
        })
    }

    #[cfg(not(unix))]
    fn close_previous_browser(
        &self,
        operation_id: &str,
        profile_id: &str,
        control: &OperationControl,
    ) -> Result<(), AppError> {
        let Some(browser) = self.take_profile_browser(profile_id) else {
            return Ok(());
        };
        *control.browser.lock() = Some(browser.clone());
        let result =
            self.measure_login_stage(operation_id, profile_id, "close_previous_browser", || {
                browser.terminate()
            });
        *control.browser.lock() = None;
        result
    }

    fn record_launched_browser(
        &self,
        operation_id: &str,
        profile_id: &str,
        browser: &BrowserHandle,
    ) -> Result<(), AppError> {
        let browser_pid = browser.pid()?;
        let runtime_kind = self.browser.status().runtime_kind;
        self.database
            .record_operation_browser(operation_id, browser_pid, &runtime_kind)?;
        self.database.record_browser_opened(profile_id)?;
        logging::record(
            &self.database,
            Some(operation_id),
            Some(profile_id),
            "browser_launched",
            json!({ "pid": browser_pid, "runtime": runtime_kind }),
        )
    }

    fn validate_authenticated_session(
        &self,
        operation_id: &str,
        profile_id: &str,
        client: &mut AppServerClient,
    ) -> Result<crate::domain::AccountSnapshot, AppError> {
        let mut stages = Vec::new();
        let started = Instant::now();
        let snapshot = client.validate_session_with_trace(&mut |stage, duration| {
            stages.push((stage, duration));
        });
        stages.push(("validate_authenticated_session", started.elapsed()));
        for (stage, duration) in stages {
            self.record_login_stage(operation_id, profile_id, stage, duration);
        }
        snapshot
    }

    fn canonical_profile_id(&self, profile_id: &str, account_id: &str) -> Result<String, AppError> {
        Ok(self
            .database
            .profile_for_account(account_id)?
            .filter(|existing| existing.id != profile_id)
            .map_or_else(|| profile_id.to_owned(), |existing| existing.id))
    }

    fn provider_mail_url(&self, provider_id: Option<&str>) -> Option<String> {
        provider_id.and_then(|id| {
            self.database
                .provider(id)
                .ok()
                .flatten()
                .map(|provider| provider.default_webmail_url)
        })
    }

    fn ensure_managed_browser(&self, operation_id: &str, profile_id: &str) -> Result<(), AppError> {
        let status = self.browser.status();
        if status.runtime_kind == "managed" && status.installed && !status.repair_required {
            return Ok(());
        }
        logging::record(
            &self.database,
            Some(operation_id),
            Some(profile_id),
            "browser_runtime_install_started",
            json!({ "expectedBytes": status.download_size_bytes }),
        )?;
        let installed = self.browser.install_managed()?;
        logging::record(
            &self.database,
            Some(operation_id),
            Some(profile_id),
            "browser_runtime_install_completed",
            json!({ "version": installed.version, "runtime": installed.runtime_kind }),
        )
    }

    fn measure_login_stage<T>(
        &self,
        operation_id: &str,
        profile_id: &str,
        stage: &'static str,
        operation: impl FnOnce() -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let started = Instant::now();
        let result = operation();
        self.record_login_stage(operation_id, profile_id, stage, started.elapsed());
        result
    }

    fn record_login_stage(
        &self,
        operation_id: &str,
        profile_id: &str,
        stage: &'static str,
        duration: Duration,
    ) {
        let _ = logging::record(
            &self.database,
            Some(operation_id),
            Some(profile_id),
            "login_stage",
            json!({
                "stage": stage,
                "durationMs": duration.as_millis(),
            }),
        );
    }
}

fn validate_login_identity(
    profile: &crate::domain::ManagedProfile,
    snapshot: &crate::domain::AccountSnapshot,
    account_id: &str,
) -> Result<(), AppError> {
    let email_mismatch = profile
        .email
        .as_deref()
        .zip(snapshot.email.as_deref())
        .is_some_and(|(expected, actual)| !expected.eq_ignore_ascii_case(actual));
    let id_mismatch = profile
        .account_id
        .as_deref()
        .is_some_and(|expected| expected != account_id);
    if email_mismatch || id_mismatch {
        let expected = profile.email.as_deref().unwrap_or(&profile.name);
        let actual = snapshot.email.as_deref().unwrap_or("outra conta");
        return Err(AppError::AccountMismatch(format!(
            "Este perfil espera {expected}, mas o navegador entrou como {actual}. Na janela aberta, saia dessa conta e entre na conta esperada antes de tentar novamente.",
        )));
    }
    Ok(())
}

enum NavigationPhase {
    Initial,
    Retry(Instant),
    Finished,
}

struct NavigationObservation {
    started: Instant,
    phase: NavigationPhase,
}

impl NavigationObservation {
    fn new(started: Instant) -> Self {
        Self {
            started,
            phase: NavigationPhase::Initial,
        }
    }

    // Polled by the login worker: no extra thread or browser owner after terminal.
    fn poll(
        &mut self,
        database: &crate::adapters::database::Database,
        task: &LoginTask,
        browser: &BrowserHandle,
    ) {
        if matches!(self.phase, NavigationPhase::Finished) {
            return;
        }
        let event = if browser.wait_for_initial_navigation(Duration::ZERO) {
            Some(if matches!(self.phase, NavigationPhase::Initial) {
                "browser_navigation_confirmed"
            } else {
                "browser_navigation_confirmed_after_retry"
            })
        } else {
            match self.phase {
                NavigationPhase::Initial
                    if self.started.elapsed() >= BROWSER_INITIAL_NAVIGATION_OBSERVATION =>
                {
                    self.begin_retry(database, task, browser);
                    None
                }
                NavigationPhase::Retry(started)
                    if started.elapsed() >= BROWSER_RETRY_NAVIGATION_OBSERVATION =>
                {
                    Some("browser_navigation_not_confirmed")
                }
                _ => None,
            }
        };
        if let Some(event) = event {
            record_navigation_event(
                database,
                &task.operation_id,
                &task.profile_id,
                event,
                self.started,
            );
            self.phase = NavigationPhase::Finished;
        }
    }

    fn begin_retry(
        &mut self,
        database: &crate::adapters::database::Database,
        task: &LoginTask,
        browser: &BrowserHandle,
    ) {
        let event = match browser.retry_initial_navigation() {
            Ok(true) => "browser_navigation_retry_started",
            Ok(false) => "browser_navigation_not_pending",
            Err(error) => {
                let _ = logging::record(
                    database,
                    Some(&task.operation_id),
                    Some(&task.profile_id),
                    "browser_navigation_retry_failed",
                    json!({"errorCode":error.code()}),
                );
                "browser_navigation_not_confirmed"
            }
        };
        record_navigation_event(
            database,
            &task.operation_id,
            &task.profile_id,
            event,
            self.started,
        );
        self.phase = NavigationPhase::Retry(Instant::now());
    }
}

fn record_navigation_event(
    database: &crate::adapters::database::Database,
    operation_id: &str,
    profile_id: &str,
    event: &str,
    started: Instant,
) {
    let _ = logging::record(
        database,
        Some(operation_id),
        Some(profile_id),
        event,
        json!({ "durationMs": started.elapsed().as_millis() }),
    );
}

#[cfg(test)]
#[cfg(unix)]
mod lifecycle_tests;
#[cfg(test)]
mod identity_tests {
    use super::validate_login_identity;
    use crate::application::test_support::profile;
    use crate::domain::AccountSnapshot;
    use crate::error::AppError;

    #[test]
    fn reconnect_accepts_original_profile_and_explains_wrong_browser_identity() {
        let profile = profile("original", chrono::Utc::now());
        let mut snapshot = AccountSnapshot {
            email: profile.email.clone(),
            plan_type: None,
            rate_limits: None,
        };
        assert!(validate_login_identity(&profile, &snapshot, "fixture-original").is_ok());
        snapshot.email = Some("another@example.test".to_owned());
        let error = validate_login_identity(&profile, &snapshot, "other-account")
            .expect_err("wrong browser session");
        assert!(matches!(error, AppError::AccountMismatch(_)));
        assert!(error.to_string().contains("original@example.test"));
        assert!(error.to_string().contains("another@example.test"));
    }

    #[test]
    fn same_email_does_not_silently_replace_a_different_account_id() {
        let profile = profile("original", chrono::Utc::now());
        let snapshot = AccountSnapshot {
            email: profile.email.clone(),
            plan_type: None,
            rate_limits: None,
        };
        assert!(validate_login_identity(&profile, &snapshot, "changed-account-id").is_err());
    }
}

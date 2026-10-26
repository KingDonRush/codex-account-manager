use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use serde_json::Value;

use super::ManagerCore;
use crate::adapters::database::Database;
use crate::domain::{AccountMetadata, OperationEvent, OperationState};
use crate::error::AppError;
use crate::ports::{CredentialStore, EventSink, StoredCredentialGeneration};

struct NoopEvents;

struct MetadataOnlyCredentials;
impl CredentialStore for MetadataOnlyCredentials {
    fn save(&self, _: &Value) -> Result<String, AppError> {
        unreachable!("snapshot never saves auth")
    }
    fn load(&self, _: &str) -> Result<Value, AppError> {
        unreachable!("snapshot never reads auth")
    }
    fn load_optional(&self, _: &str) -> Result<Option<Value>, AppError> {
        unreachable!("snapshot never reads auth")
    }
    fn delete(&self, _: &str) -> Result<(), AppError> {
        unreachable!("snapshot never deletes auth")
    }
}

struct RegisteredSessionMetadata(Vec<String>);
impl crate::ports::ApplicationSessionStore for RegisteredSessionMetadata {
    fn save(&self, _: &str, _: &Value) -> Result<(), AppError> {
        unreachable!("metadata cannot save a session")
    }
    fn load(&self, _: &str, _: &str) -> Result<Value, AppError> {
        unreachable!("metadata cannot authenticate")
    }
    fn delete(&self, _: &str) -> Result<(), AppError> {
        unreachable!("metadata cannot delete a session")
    }
    fn pending_sessions(&self) -> Result<Vec<String>, AppError> {
        Ok(self.0.clone())
    }
}

#[test]
fn integration_snapshot_preserves_registration_without_claiming_or_performing_authentication() {
    let fixture = tempfile::tempdir().expect("fixture");
    let database = Database::open(fixture.path()).expect("db");
    let profile = database
        .create_profile("Registered", None, None)
        .expect("profile");
    database
        .persist_connected_account(
            &profile.id,
            AccountMetadata {
                account_id: "registered-account",
                email: Some("fixture@example.test"),
                plan_type: Some("plus"),
                credential_ref: None,
                rate_limits: None,
                observed_at: Utc::now(),
            },
            None,
        )
        .expect("metadata");
    let local = database
        .account_integration_options("registered-account")
        .expect("ID")
        .0;
    let session = uuid::Uuid::new_v4().to_string();
    database
        .save_application_session(&local, &session)
        .expect("registration");
    let mut manager = ManagerCore::from_components(
        fixture.path().to_path_buf(),
        None,
        database.clone(),
        Arc::new(MetadataOnlyCredentials),
    )
    .expect("core");
    for pending in [false, true] {
        manager.application_credentials = Arc::new(RegisteredSessionMetadata(if pending {
            vec![session.clone()]
        } else {
            Vec::new()
        }));
        let snapshot = manager.integration_snapshot().expect("metadata only");
        assert_eq!(snapshot.accounts.len(), 1);
        let account = &snapshot.accounts[0];
        assert_eq!(account.id, local);
        assert_eq!(account.profile_id, profile.id);
        assert!(account.application_session_connected);
        assert_eq!(account.application_session_recovery_pending, pending);
        assert_eq!(account.email.as_deref(), Some("fixture@example.test"));
        let wire = serde_json::to_value(account).expect("wire metadata");
        assert_eq!(wire["applicationSessionConnected"], true);
        assert!(wire.get("accessToken").is_none());
        assert_eq!(
            database
                .application_session(&local)
                .expect("preserved")
                .as_deref(),
            Some(session.as_str())
        );
    }
    manager.cancel_all();
}

#[test]
fn cancellation_is_decided_before_commit_and_not_during_persistence() {
    use super::OperationControl;
    use crate::domain::OperationKind;
    use parking_lot::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    let fixture = tempfile::tempdir().expect("fixture");
    let database = Database::open(fixture.path()).expect("database");
    let profile = database
        .create_profile("Commit", None, None)
        .expect("profile");
    let manager = ManagerCore::from_components(
        fixture.path().to_path_buf(),
        None,
        database.clone(),
        Arc::new(DeleteFailCredentials),
    )
    .expect("core");
    for cancelled in [true, false] {
        let operation = database
            .create_operation(&profile.id, false)
            .expect("operation");
        let control = Arc::new(OperationControl {
            profile_id: profile.id.clone(),
            kind: OperationKind::Login,
            cancel: Arc::new(AtomicBool::new(false)),
            cancelable: AtomicBool::new(true),
            cancellation_gate: Mutex::new(()),
            browser: Arc::new(Mutex::new(None)),
            replacement: Mutex::new(None),
            confirmed: AtomicBool::new(false),
        });
        manager.install_operation_control(&operation.id, control.clone());
        manager
            .transition(
                &operation.id,
                &profile.id,
                OperationState::Preparing,
                "prepare",
            )
            .expect("preparing");
        manager
            .transition(
                &operation.id,
                &profile.id,
                OperationState::Verifying,
                "verify",
            )
            .expect("verifying");
        if cancelled {
            manager.cancel_operation(&operation.id).expect("cancel");
            assert!(matches!(
                manager.transition(
                    &operation.id,
                    &profile.id,
                    OperationState::Persisting,
                    "persist"
                ),
                Err(AppError::Cancelled)
            ));
            assert_eq!(
                database
                    .operation(&operation.id)
                    .expect("state")
                    .state
                    .as_str(),
                "verifying"
            );
        } else {
            manager
                .transition(
                    &operation.id,
                    &profile.id,
                    OperationState::Persisting,
                    "persist",
                )
                .expect("commit");
            assert!(manager.cancel_operation(&operation.id).is_err());
            assert!(!control.cancel.load(Ordering::Acquire));
        }
        manager.operations.lock().controls.remove(&operation.id);
    }
    manager.cancel_all();
}

#[test]
fn reconnecting_the_active_identity_requires_applying_the_new_session() {
    let now = Utc::now();
    let mut profile = crate::application::test_support::profile("active", now);
    profile.is_active_account = true;
    profile.last_browser_authenticated_at = Some(now);
    profile.last_activated_at = Some(now - chrono::Duration::hours(1));
    assert!(super::needs_activation(
        &profile,
        Some(now - chrono::Duration::hours(1))
    ));
    profile.last_activated_at = Some(now + chrono::Duration::seconds(1));
    assert!(!super::needs_activation(
        &profile,
        Some(now - chrono::Duration::hours(1))
    ));
}

impl EventSink for NoopEvents {
    fn emit(&self, _event: &OperationEvent) {}

    fn emit_dashboard_changed(&self) {}
}

struct SlowCredentials {
    started: mpsc::Sender<()>,
    completed: mpsc::Sender<()>,
}

struct ReferenceCredentials {
    generation: StoredCredentialGeneration,
}

struct DeleteFailCredentials;

impl CredentialStore for ReferenceCredentials {
    fn save(&self, _document: &Value) -> Result<String, AppError> {
        unreachable!("not used in quarantine audit")
    }

    fn load(&self, _account_id: &str) -> Result<Value, AppError> {
        Ok(serde_json::json!({}))
    }

    fn load_optional(&self, _account_id: &str) -> Result<Option<Value>, AppError> {
        Ok(Some(serde_json::json!({})))
    }

    fn delete(&self, _account_id: &str) -> Result<(), AppError> {
        Ok(())
    }

    fn generation_refs(
        &self,
        _account_id: &str,
    ) -> Result<Vec<StoredCredentialGeneration>, AppError> {
        Ok(vec![self.generation.clone()])
    }
}

impl CredentialStore for DeleteFailCredentials {
    fn save(&self, _document: &Value) -> Result<String, AppError> {
        Ok("account-delete-failure".to_owned())
    }

    fn load(&self, _account_id: &str) -> Result<Value, AppError> {
        Ok(serde_json::json!({"credential": "fixture"}))
    }

    fn load_optional(&self, _account_id: &str) -> Result<Option<Value>, AppError> {
        Ok(Some(serde_json::json!({"credential": "fixture"})))
    }

    fn delete(&self, _account_id: &str) -> Result<(), AppError> {
        Err(AppError::Keyring("falha de exclusão injetada".to_owned()))
    }

    fn generation_refs(
        &self,
        _account_id: &str,
    ) -> Result<Vec<StoredCredentialGeneration>, AppError> {
        Ok(Vec::new())
    }
}

impl CredentialStore for SlowCredentials {
    fn save(&self, _document: &Value) -> Result<String, AppError> {
        unreachable!("not used in dashboard test")
    }

    fn load(&self, _account_id: &str) -> Result<Value, AppError> {
        Err(AppError::AppServer(
            "No access token in this fixture.".to_owned(),
        ))
    }

    fn load_optional(&self, _account_id: &str) -> Result<Option<Value>, AppError> {
        Ok(None)
    }

    fn delete(&self, _account_id: &str) -> Result<(), AppError> {
        Ok(())
    }

    fn generation_refs(
        &self,
        _account_id: &str,
    ) -> Result<Vec<StoredCredentialGeneration>, AppError> {
        let _ = self.started.send(());
        std::thread::sleep(Duration::from_millis(350));
        let _ = self.completed.send(());
        Ok(Vec::new())
    }
}

#[test]
fn slow_keyring_audit_never_blocks_dashboard_snapshot() {
    let app_data = tempfile::tempdir().expect("temporary app data");
    let database = Database::open(app_data.path()).expect("database");
    let profile = database
        .create_profile("Cached account", None, None)
        .expect("profile");
    database
        .persist_connected_account(
            &profile.id,
            AccountMetadata {
                account_id: "cached-account",
                email: None,
                plan_type: None,
                credential_ref: Some("native-keyring"),
                rate_limits: None,
                observed_at: Utc::now(),
            },
            None,
        )
        .expect("account metadata");
    let (started_tx, started_rx) = mpsc::channel();
    let (completed_tx, completed_rx) = mpsc::channel();
    let credentials = Arc::new(SlowCredentials {
        started: started_tx,
        completed: completed_tx,
    });
    let manager =
        ManagerCore::from_components(app_data.path().to_path_buf(), None, database, credentials)
            .expect("manager");
    manager.set_event_sink(Arc::new(NoopEvents));
    started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("audit started");

    let started = Instant::now();
    let snapshot = manager.dashboard_snapshot().expect("cached snapshot");

    assert!(started.elapsed() < Duration::from_millis(100));
    assert_eq!(snapshot.counts.connected, 1);
    completed_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("audit completed");
    manager.cancel_all();
}

#[test]
fn audit_reports_when_every_saved_generation_is_quarantined() {
    let app_data = tempfile::tempdir().expect("temporary app data");
    let database = Database::open(app_data.path()).expect("database");
    let generation = StoredCredentialGeneration {
        account_id: "quarantined-account".to_owned(),
        generation_id: "generation-a".to_owned(),
        fingerprint: "f".repeat(64),
        created_at: Utc::now(),
        is_active: false,
    };
    database
        .upsert_credential_generation(&generation, false)
        .expect("generation metadata");
    database
        .quarantine_credential_generation(
            &generation.account_id,
            &generation.generation_id,
            Utc::now(),
        )
        .expect("quarantine");
    let credentials = Arc::new(ReferenceCredentials {
        generation: generation.clone(),
    });
    let manager =
        ManagerCore::from_components(app_data.path().to_path_buf(), None, database, credentials)
            .expect("manager");

    assert_eq!(
        manager.audit_credential(&generation.account_id),
        crate::domain::CredentialState::Quarantined
    );
    manager.cancel_all();
}

#[test]
fn keyring_delete_failure_keeps_profile_and_local_directory_intact() {
    let app_data = tempfile::tempdir().expect("temporary app data");
    let database = Database::open(app_data.path()).expect("database");
    let profile = database
        .create_profile("Protegido", None, None)
        .expect("profile");
    database
        .persist_connected_account(
            &profile.id,
            AccountMetadata {
                account_id: "account-delete-failure",
                email: None,
                plan_type: None,
                credential_ref: Some("native-keyring"),
                rate_limits: None,
                observed_at: Utc::now(),
            },
            None,
        )
        .expect("account metadata");
    let credentials = Arc::new(DeleteFailCredentials);
    let manager = ManagerCore::from_components(
        app_data.path().to_path_buf(),
        None,
        database.clone(),
        credentials,
    )
    .expect("manager");

    assert!(manager.delete_profile(&profile.id, true).is_err());
    assert!(database.profile(&profile.id).is_ok());
    assert!(std::path::Path::new(&profile.data_dir).is_dir());
    manager.cancel_all();
}

#[test]
fn saved_webmail_url_overrides_and_can_return_to_the_provider_catalog() {
    let app_data = tempfile::tempdir().expect("temporary app data");
    let database = Database::open(app_data.path()).expect("database");
    let profile = database
        .create_profile(
            "Webmail",
            Some("gmail"),
            Some("https://custom.example.test/inbox"),
        )
        .expect("profile");
    let manager = ManagerCore::from_components(
        app_data.path().to_path_buf(),
        None,
        database,
        Arc::new(DeleteFailCredentials),
    )
    .expect("manager");

    assert_eq!(
        manager
            .mail_url_for_profile(&profile)
            .expect("custom webmail")
            .as_deref(),
        Some("https://custom.example.test/inbox")
    );
    let catalog_profile = manager
        .update_profile_mail_url(&profile.id, None)
        .expect("clear override");
    assert_eq!(
        manager
            .mail_url_for_profile(&catalog_profile)
            .expect("catalog webmail")
            .as_deref(),
        Some("https://mail.google.com/mail/u/0/#inbox")
    );
    manager.cancel_all();
}

#[cfg(unix)]
#[test]
fn starting_login_never_waits_for_previous_browser_shutdown() {
    let app_data = tempfile::tempdir().expect("temporary app data");
    let database = Database::open(app_data.path()).expect("database");
    let profile = database
        .create_profile("Previous browser", None, None)
        .expect("profile");
    let manager = ManagerCore::from_components(
        app_data.path().to_path_buf(),
        None,
        database.clone(),
        Arc::new(DeleteFailCredentials),
    )
    .expect("manager");
    let previous = crate::adapters::browser_runtime::BrowserHandle::stubborn_test_process()
        .expect("stubborn browser");
    manager.remember_browser_session(profile.id.clone(), previous);

    let started = Instant::now();
    let operation = manager
        .start_login(&profile.id, false, None)
        .expect("login operation");

    assert!(
        started.elapsed() < Duration::from_millis(400),
        "start_login waited for browser shutdown"
    );
    manager
        .cancel_operation(&operation.id)
        .expect("cancel operation");
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let current = database.operation(&operation.id).expect("operation");
        if current.state.is_terminal() {
            assert!(matches!(current.state, OperationState::Cancelled));
            break;
        }
        assert!(Instant::now() < deadline, "operation did not cancel");
        std::thread::sleep(Duration::from_millis(20));
    }
    manager.cancel_all();
}

#[test]
#[ignore = "requires an installed Codex CLI, Chromium and a display"]
fn real_login_opens_chromium_and_cancels_without_blocking() {
    let app_data = tempfile::tempdir().expect("temporary app data");
    let bundled_runtime =
        std::env::var_os("CODEX_MANAGER_BUNDLED_RUNTIME").map(std::path::PathBuf::from);
    let database = Database::open(app_data.path()).expect("private db");
    let manager = ManagerCore::from_components(
        app_data.path().to_path_buf(),
        bundled_runtime,
        database,
        Arc::new(DeleteFailCredentials),
    )
    .expect("manager core without native coffer access");
    let profile = manager
        .create_profile("Integration profile", None, None)
        .expect("profile");
    manager
        .database
        .persist_connected_account(
            &profile.id,
            AccountMetadata {
                account_id: "integration-account",
                email: Some("integration@yahoo.com"),
                plan_type: None,
                credential_ref: None,
                rate_limits: None,
                observed_at: Utc::now(),
            },
            None,
        )
        .expect("connected profile fixture");

    let operation = manager
        .start_login(&profile.id, true, None)
        .expect("login operation");
    let launch_deadline = Instant::now() + Duration::from_secs(20);
    let browser_pid = loop {
        let current = manager
            .database
            .operation(&operation.id)
            .expect("operation state");
        if matches!(current.state, OperationState::AwaitingLogin) {
            break current.browser_pid.expect("browser PID was not persisted");
        }
        assert!(
            !current.state.is_terminal(),
            "login terminated before opening Chromium: {}",
            current.step
        );
        assert!(
            Instant::now() < launch_deadline,
            "Chromium did not open within 20 seconds; last step: {}",
            current.step
        );
        std::thread::sleep(Duration::from_millis(50));
    };

    #[cfg(target_os = "linux")]
    {
        let command_line =
            std::fs::read(format!("/proc/{browser_pid}/cmdline")).expect("browser command line");
        let command_line = String::from_utf8_lossy(&command_line).replace('\0', " ");
        assert!(
            command_line.contains("--remote-debugging-pipe"),
            "Linux browser must use the private CDP pipe"
        );
        assert!(
            !command_line.contains("https://"),
            "sensitive remote URL leaked into browser arguments"
        );
    }
    #[cfg(not(target_os = "linux"))]
    let _ = browser_pid;

    let cancel_started = Instant::now();
    manager
        .cancel_operation(&operation.id)
        .expect("cancellation request");
    assert!(
        cancel_started.elapsed() < Duration::from_millis(500),
        "cancellation blocked the caller"
    );

    let cancel_deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let current = manager
            .database
            .operation(&operation.id)
            .expect("cancelled operation");
        if matches!(current.state, OperationState::Cancelled) {
            break;
        }
        assert!(
            Instant::now() < cancel_deadline,
            "worker did not finish cancellation; last step: {}",
            current.step
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    manager.cancel_all();
    #[cfg(target_os = "linux")]
    assert!(!std::path::Path::new(&format!("/proc/{browser_pid}")).exists());
}

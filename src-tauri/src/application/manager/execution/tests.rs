//! Real worker/selection/lease/runner/recovery/SQL; only child service, coffer and keys are fixtures.
use super::*;
use crate::{
    adapters::{
        application_recovery::{RecoverableApplicationStore, RecoveryKeyStore},
        database::Database,
    },
    domain::{
        integrations::{AccountPolicy, Application, GrantScope, Permission},
        AccountMetadata,
    },
    ports::{ApplicationSessionStore, CredentialStore},
};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::{collections::BTreeSet, os::unix::fs::PermissionsExt, path::PathBuf, time::Instant};

struct NoDesktopCredentials;
impl CredentialStore for NoDesktopCredentials {
    fn save(&self, _: &Value) -> Result<String, AppError> {
        unreachable!("exec must not save Desktop auth")
    }
    fn load(&self, _: &str) -> Result<Value, AppError> {
        unreachable!("exec must not read Desktop coffer")
    }
    fn load_optional(&self, _: &str) -> Result<Option<Value>, AppError> {
        Ok(None)
    }
    fn delete(&self, _: &str) -> Result<(), AppError> {
        unreachable!("exec cannot delete Desktop accounts")
    }
}

struct AccountCredentials(Value);
impl CredentialStore for AccountCredentials {
    fn save(&self, _: &Value) -> Result<String, AppError> {
        panic!("external execution cannot rotate Desktop auth")
    }
    fn load(&self, expected: &str) -> Result<Value, AppError> {
        crate::adapters::auth_store::validate_auth_document(&self.0, Some(expected))?;
        Ok(self.0.clone())
    }
    fn load_optional(&self, expected: &str) -> Result<Option<Value>, AppError> {
        self.load(expected).map(Some)
    }
    fn delete(&self, _: &str) -> Result<(), AppError> {
        panic!("no deletion")
    }
}

fn document(generation: &str) -> Value {
    json!({"auth_mode":"chatgpt","tokens":{"account_id":"synthetic-remote","access_token":format!("synthetic-access-{generation}"),"refresh_token":format!("synthetic-refresh-{generation}"),"generation":generation}})
}

struct Store {
    document: Mutex<Value>,
    root: PathBuf,
    hold_commit: AtomicBool,
    release_commit: AtomicBool,
}
impl ApplicationSessionStore for Store {
    fn save(&self, _: &str, doc: &Value) -> Result<(), AppError> {
        std::fs::write(self.root.join("commit-entered"), b"synthetic")?;
        if self.root.join("coffer-unavailable").exists() {
            return Err(AppError::CredentialUnavailable(
                "synthetic outage".to_owned(),
            ));
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while self.hold_commit.load(Ordering::Acquire)
            && !self.release_commit.load(Ordering::Acquire)
        {
            if Instant::now() >= deadline {
                return Err(AppError::CredentialUnavailable(
                    "synthetic commit deadline".to_owned(),
                ));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        crate::adapters::auth_store::validate_auth_document(doc, Some("synthetic-remote"))?;
        *self.document.lock() = doc.clone();
        Ok(())
    }
    fn load(&self, _: &str, expected: &str) -> Result<Value, AppError> {
        let document = self.document.lock().clone();
        crate::adapters::auth_store::validate_auth_document(&document, Some(expected))?;
        Ok(document)
    }
    fn delete(&self, _: &str) -> Result<(), AppError> {
        unreachable!("no session deletion")
    }
}
struct Keys(PathBuf);
impl RecoveryKeyStore for Keys {
    fn load_or_create(&self, _: &str) -> Result<[u8; 32], AppError> {
        if self.0.join("coffer-unavailable").exists() {
            return Err(AppError::CredentialUnavailable(
                "synthetic locked key".to_owned(),
            ));
        }
        Ok([41; 32])
    }
    fn delete(&self, _: &str) -> Result<(), AppError> {
        unreachable!("no key deletion")
    }
}

struct Fixture {
    core: ManagerCore,
    grant: AuthorizationGrant,
    account: String,
    session: String,
    store: Arc<Store>,
    directory: tempfile::TempDir,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.store.release_commit.store(true, Ordering::Release);
        self.core.shutdown().expect("fixture workers must drain");
    }
}

impl Fixture {
    fn new(scenario: Value) -> Self {
        let directory = tempfile::tempdir().expect("private fixture");
        let root = directory.path();
        let db = Database::open(root).expect("db");
        let profile = db.create_profile("Synthetic", None, None).expect("profile");
        db.persist_connected_account(
            &profile.id,
            AccountMetadata {
                account_id: "synthetic-remote",
                email: None,
                plan_type: Some("plus"),
                credential_ref: None,
                rate_limits: None,
                observed_at: Utc::now(),
            },
            None,
        )
        .expect("account");
        let account = db
            .account_integration_options("synthetic-remote")
            .expect("account ID")
            .0;
        db.set_account_policy(&account, AccountPolicy::Shared, 1)
            .expect("policy");
        let session = uuid::Uuid::new_v4().to_string();
        if scenario["external"] != true {
            db.save_application_session(&account, &session)
                .expect("own app session");
        }
        let app = db
            .save_application(&Application {
                id: uuid::Uuid::new_v4().to_string(),
                name: "Synthetic execution".to_owned(),
                slug: "synthetic-execution".to_owned(),
                paused: false,
                revision: 0,
            })
            .expect("application");
        let grant = AuthorizationGrant {
            id: uuid::Uuid::new_v4().to_string(),
            application_id: app.id,
            verifier: "synthetic-public".to_owned(),
            peer_identity: "synthetic-peer".to_owned(),
            revoked: false,
            created_at: Utc::now(),
            scope: GrantScope {
                account_ids: BTreeSet::from([account.clone()]),
                direct_account_ids: Some(BTreeSet::from([account.clone()])),
                permissions: BTreeSet::from([Permission::Execute]),
                ..GrantScope::default()
            },
        };
        db.save_grant(&grant).expect("grant");
        let store = Arc::new(Store {
            document: Mutex::new(document("initial")),
            root: root.to_path_buf(),
            hold_commit: AtomicBool::new(false),
            release_commit: AtomicBool::new(false),
        });
        let credentials = Arc::new(RecoverableApplicationStore::new(
            root,
            store.clone(),
            Arc::new(Keys(root.to_path_buf())),
        ));
        let binary = root.join("execution.py");
        std::fs::write(&binary, include_str!("fixture.py")).expect("synthetic child");
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700))
            .expect("private executable");
        std::fs::write(root.join("scenario.json"), scenario.to_string()).expect("scenario");
        let mut core = ManagerCore::from_components(
            root.to_path_buf(),
            None,
            db,
            Arc::new(NoDesktopCredentials),
        )
        .expect("core");
        core.application_credentials = credentials;
        core.exec_test_binary = Some(binary);
        Self {
            core,
            grant,
            account,
            session,
            store,
            directory,
        }
    }
    fn start(&self) -> ExecutionStatus {
        self.core
            .start_execution(
                &self.grant,
                ExecRequest {
                    group_id: None,
                    binding_id: String::new(),
                    binding_snapshot: None,
                    account_id: Some(self.account.clone()),
                    cwd: self.directory.path().to_string_lossy().into_owned(),
                    prompt: "synthetic prompt".to_owned(),
                    arguments: Vec::new(),
                },
            )
            .expect("real submission")
    }
    fn wait_for(&self, marker: &str) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if self.directory.path().join(marker).exists() {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("fixture did not reach {marker}");
    }
    fn terminal(&self, id: &str) -> ExecutionStatus {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let status = self
                .core
                .execution_status_for(&self.grant, id)
                .expect("status");
            if status.state.terminal() {
                return status;
            }
            assert!(Instant::now() < deadline, "execution did not terminate");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn assert_resources_closed(&self) {
        let lines = std::fs::read_to_string(self.directory.path().join("processes.jsonl"))
            .expect("process identity");
        for line in lines.lines() {
            let info: Value = serde_json::from_str(line).expect("safe fixture metadata");
            let pid = info["pid"].as_u64().expect("PID");
            assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
            assert!(!std::path::Path::new(info["home"].as_str().expect("private home")).exists());
        }
        assert!(!self.core.executions.session_busy(&self.session));
        assert!(!self.core.codex_home.join("auth.json").exists());
    }
}

#[test]
fn pending_session_preservation_survives_nonzero_exit_and_cancellation_in_real_execution_status() {
    for (scenario, expected_state) in [
        (
            json!({"coffer_failure":true,"exit_code":7}),
            ExecutionState::Failed,
        ),
        (
            json!({"coffer_failure":true,"cancel":true}),
            ExecutionState::Cancelled,
        ),
    ] {
        let fixture = Fixture::new(scenario);
        let submitted = fixture.start();
        fixture.wait_for("rotated");
        if expected_state == ExecutionState::Cancelled {
            fixture
                .core
                .cancel_execution(&submitted.id)
                .expect("user cancels");
        }
        let status = fixture.terminal(&submitted.id);
        fixture.assert_resources_closed();
        assert_eq!(status.state, expected_state);
        assert_eq!(
            status.error_code.as_deref(),
            Some("credential_recovery_pending"),
            "terminal cannot hide protected session awaiting coffer"
        );
        assert_eq!(
            fixture.store.document.lock()["tokens"]["generation"],
            "initial"
        );
        assert_eq!(
            fixture
                .core
                .application_credentials
                .pending_sessions()
                .expect("protected pending")
                .len(),
            1
        );
        let again = fixture.start();
        let blocked = fixture.terminal(&again.id);
        assert_eq!(blocked.state, ExecutionState::Failed);
        assert_eq!(
            blocked.error_code.as_deref(),
            Some("credential_unavailable")
        );
        let trace = std::fs::read_to_string(fixture.directory.path().join("processes.jsonl"))
            .expect("trace");
        assert_eq!(
            trace.lines().count(),
            1,
            "new work cannot replay old auth while recovery is pending"
        );
        let record_path = fixture
            .directory
            .path()
            .join("application-recovery")
            .join(format!("{}.sealed", fixture.session));
        let encrypted = std::fs::read(&record_path).expect("sealed session");
        assert!(!encrypted
            .windows(b"synthetic-refresh-renewed".len())
            .any(|bytes| bytes == b"synthetic-refresh-renewed"));
        std::fs::remove_file(fixture.directory.path().join("coffer-unavailable"))
            .expect("restore synthetic coffer");
        let recovered = fixture
            .core
            .application_credentials
            .load(&fixture.session, "synthetic-remote")
            .expect("recover without repeating execution");
        assert_eq!(recovered["tokens"]["generation"], "renewed");
        assert!(!record_path.exists());
        assert!(!fixture.core.executions.session_busy(&fixture.session));
    }
}

#[test]
fn success_keeps_session_reserved_until_native_persistence_finishes_then_reopens_with_new_generation(
) {
    let fixture = Fixture::new(json!({}));
    fixture.store.hold_commit.store(true, Ordering::Release);
    let submitted = fixture.start();
    fixture.wait_for("commit-entered");
    assert!(fixture.core.executions.session_busy(&fixture.session));
    assert!(!fixture
        .core
        .execution_status_for(&fixture.grant, &submitted.id)
        .expect("not terminal before commit")
        .state
        .terminal());
    fixture.store.release_commit.store(true, Ordering::Release);
    let first = fixture.terminal(&submitted.id);
    assert_eq!(first.state, ExecutionState::Completed);
    assert!(first.error_code.is_none());
    fixture.assert_resources_closed();
    assert_eq!(
        fixture.store.document.lock()["tokens"]["generation"],
        "renewed"
    );
    let next = fixture.start();
    assert_eq!(fixture.terminal(&next.id).state, ExecutionState::Completed);
    let trace =
        std::fs::read_to_string(fixture.directory.path().join("processes.jsonl")).expect("trace");
    let lines: Vec<Value> = trace
        .lines()
        .map(|line| serde_json::from_str(line).expect("trace JSON"))
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[1]["generation"], "renewed");
    fixture.assert_resources_closed();
}

#[test]
fn connected_authorized_account_executes_without_another_login_or_refresh_copy() {
    let mut fixture = Fixture::new(json!({"external":true}));
    let mut saved = document("initial");
    saved["tokens"]["id_token"] = json!("synthetic-id");
    fixture.core.credentials = Arc::new(AccountCredentials(saved.clone()));
    let first = fixture.start();
    let result = fixture.terminal(&first.id);
    assert_eq!(
        result.state,
        ExecutionState::Completed,
        "{:?}",
        result.error_code
    );
    fixture.assert_resources_closed();
    assert_eq!(
        fixture.core.credentials.load("synthetic-remote").unwrap(),
        saved
    );
    assert!(fixture
        .core
        .database
        .application_session(&fixture.account)
        .unwrap()
        .is_none());
    assert!(!fixture.directory.path().join("commit-entered").exists());
    fixture
        .core
        .database
        .set_account_policy(&fixture.account, AccountPolicy::DesktopOnly, 1)
        .unwrap();
    assert!(
        fixture
            .core
            .execution_candidates(
                &fixture.grant,
                &ExecRequest {
                    cwd: fixture.directory.path().to_string_lossy().into_owned(),
                    prompt: String::new(),
                    arguments: vec![],
                    account_id: Some(fixture.account.clone()),
                    group_id: None,
                    binding_id: String::new(),
                    binding_snapshot: None,
                }
            )
            .is_err(),
        "existing credential does not grant permission"
    );
}

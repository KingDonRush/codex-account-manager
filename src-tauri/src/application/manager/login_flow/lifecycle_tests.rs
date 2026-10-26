//! Real worker, operations, CDP/JSONL process adapters and persistence, private doubles only.
use super::*;
use crate::{adapters::database::Database, ports::CredentialStore};
use parking_lot::Mutex;
use serde_json::Value;
use std::{
    collections::HashMap,
    os::unix::fs::PermissionsExt,
    sync::atomic::{AtomicUsize, Ordering},
};

#[derive(Default)]
struct Credentials {
    documents: Mutex<HashMap<String, Value>>,
    saves: AtomicUsize,
}
impl CredentialStore for Credentials {
    fn save(&self, doc: &Value) -> Result<String, AppError> {
        let id = auth_store::validate_auth_document(doc, None)?;
        self.documents.lock().insert(id.clone(), doc.clone());
        self.saves.fetch_add(1, Ordering::AcqRel);
        Ok(id)
    }
    fn load(&self, id: &str) -> Result<Value, AppError> {
        self.load_optional(id)?
            .ok_or_else(|| AppError::Keyring("missing fixture".to_owned()))
    }
    fn load_optional(&self, id: &str) -> Result<Option<Value>, AppError> {
        Ok(self.documents.lock().get(id).cloned())
    }
    fn delete(&self, id: &str) -> Result<(), AppError> {
        self.documents.lock().remove(id);
        Ok(())
    }
}
struct Fixture {
    core: ManagerCore,
    credentials: Arc<Credentials>,
    profile: String,
    directory: tempfile::TempDir,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.core
            .shutdown()
            .expect("private tasks/children drained");
    }
}
impl Fixture {
    fn new(scenario: Value) -> Self {
        let directory = tempfile::tempdir().expect("private fixture");
        let root = directory.path();
        let bundle = root.join("bundle");
        std::fs::create_dir(&bundle).expect("synthetic bundle");
        for (name, body) in [
            ("browser.py", include_str!("browser_fixture.py")),
            ("login.py", include_str!("login_fixture.py")),
        ] {
            let path = bundle.join(name);
            std::fs::write(&path, body).expect("synthetic process");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
                .expect("private executable");
        }
        std::fs::write(bundle.join("scenario.json"), scenario.to_string()).expect("scenario");
        std::fs::write(bundle.join("active.json"), json!({"version":"153.0.8010.12","executable":"browser.py","archiveSha256":"a".repeat(64)}).to_string()).expect("fake runtime marker");
        let db = Database::open(&root.join("data")).expect("db");
        let profile = db
            .create_profile("Synthetic login", Some("yahoo"), None)
            .expect("profile")
            .id;
        let credentials = Arc::new(Credentials::default());
        let mut core = ManagerCore::from_components(
            root.join("data"),
            Some(bundle.clone()),
            db,
            credentials.clone(),
        )
        .expect("core");
        core.login_test_binary = Some(bundle.join("login.py"));
        Self {
            core,
            credentials,
            profile,
            directory,
        }
    }
    fn wait(&self, id: &str, state: OperationState) -> LoginOperation {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let current = self.core.database.operation(id).expect("operation");
            if current.state.as_str() == state.as_str() {
                return current;
            }
            assert!(
                !current.state.is_terminal(),
                "unexpected terminal {}",
                current.step
            );
            assert!(
                Instant::now() < deadline,
                "operation never reached {}",
                state.as_str()
            );
            thread::sleep(Duration::from_millis(5));
        }
    }
    fn trace(&self, name: &str) -> Vec<Value> {
        std::fs::read_to_string(self.directory.path().join("bundle").join(name))
            .expect("private trace")
            .lines()
            .map(|line| serde_json::from_str(line).expect("JSON"))
            .collect()
    }
    fn assert_login_collected(&self) {
        for info in self.trace("login-processes.jsonl") {
            assert!(!std::path::Path::new(&format!(
                "/proc/{}",
                info["pid"].as_u64().expect("PID")
            ))
            .exists());
            assert!(!std::path::Path::new(info["home"].as_str().expect("private home")).exists());
        }
        assert!(!self.core.codex_home.join("auth.json").exists());
    }
}

#[test]
fn cancelling_login_owns_cleanup_before_terminal_and_repeat_starts_a_fresh_process() {
    let fixture = Fixture::new(json!({}));
    for _ in 0..2 {
        let operation = fixture
            .core
            .start_login(&fixture.profile, false, None)
            .expect("start");
        let awaiting = fixture.wait(&operation.id, OperationState::AwaitingLogin);
        let started = Instant::now();
        fixture
            .core
            .cancel_operation(&operation.id)
            .expect("cancel request");
        assert!(started.elapsed() < Duration::from_millis(100));
        fixture.wait(&operation.id, OperationState::Cancelled);
        let pid = awaiting.browser_pid.expect("owned browser PID");
        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
        fixture.assert_login_collected();
    }
    assert_eq!(fixture.credentials.saves.load(Ordering::Acquire), 0);
    assert!(fixture
        .core
        .database
        .profile(&fixture.profile)
        .expect("profile")
        .account_id
        .is_none());
    assert_eq!(
        fixture
            .trace("browser-requests.jsonl")
            .iter()
            .filter(|request| request["method"] == "Target.closeTarget")
            .count(),
        2
    );
}

#[test]
fn a_manual_window_is_not_closed_when_the_login_operation_is_cancelled() {
    let fixture = Fixture::new(json!({}));
    fixture
        .core
        .open_profile_browser(&fixture.profile)
        .expect("manual profile window");
    let operation = fixture
        .core
        .start_login(&fixture.profile, false, None)
        .expect("start in shared process");
    let awaiting = fixture.wait(&operation.id, OperationState::AwaitingLogin);
    fixture
        .core
        .cancel_operation(&operation.id)
        .expect("cancel");
    fixture.wait(&operation.id, OperationState::Cancelled);
    let pid = awaiting.browser_pid.expect("browser PID");
    assert!(std::path::Path::new(&format!("/proc/{pid}")).exists());
    let requests = fixture.trace("browser-requests.jsonl");
    let closed: Vec<_> = requests
        .iter()
        .filter(|request| request["method"] == "Target.closeTarget")
        .collect();
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0]["targetId"], "synthetic-target-2");
    fixture.assert_login_collected();
}

#[test]
fn successful_login_closes_its_owned_browser_and_saves_once_before_completed() {
    let fixture = Fixture::new(json!({"success":true}));
    let operation = fixture
        .core
        .start_login(&fixture.profile, false, None)
        .expect("start");
    let completed = fixture.wait(&operation.id, OperationState::Completed);
    assert!(completed.error_code.is_none());
    fixture.assert_login_collected();
    let pid = completed.browser_pid.expect("browser PID");
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
    assert_eq!(fixture.credentials.saves.load(Ordering::Acquire), 1);
    assert_eq!(
        fixture
            .core
            .database
            .profile(&fixture.profile)
            .expect("associated")
            .account_id
            .as_deref(),
        Some("synthetic-remote")
    );
}

#[test]
fn a_saved_login_with_unconfirmed_browser_cleanup_is_not_reported_as_full_success() {
    let fixture = Fixture::new(json!({"success":true,"refuse_close":true}));
    let operation = fixture
        .core
        .start_login(&fixture.profile, false, None)
        .expect("start");
    let completed = fixture.wait(&operation.id, OperationState::Completed);
    fixture.assert_login_collected();
    assert_eq!(fixture.credentials.saves.load(Ordering::Acquire), 1);
    assert_eq!(
        completed.error_code.as_deref(),
        Some("browser_cleanup_pending")
    );
    assert!(
        !fixture.core.retained_login_browsers.lock().is_empty(),
        "keep the exact owner for safe retry"
    );
    std::fs::write(
        fixture.directory.path().join("bundle/scenario.json"),
        json!({"success":true}).to_string(),
    )
    .expect("fixture allows close");
    fixture
        .core
        .retry_browser_cleanup(&operation.id)
        .expect("retry cleanup without OAuth");
    assert!(fixture
        .core
        .database
        .operation(&operation.id)
        .expect("updated result")
        .error_code
        .is_none());
    assert_eq!(fixture.credentials.saves.load(Ordering::Acquire), 1);
    assert_eq!(fixture.trace("login-processes.jsonl").len(), 1);
    assert!(fixture.core.retained_login_browsers.lock().is_empty());
    assert!(!std::path::Path::new(&format!(
        "/proc/{}",
        completed.browser_pid.expect("owned browser")
    ))
    .exists());
}

#[test]
fn failed_cleanup_during_cancellation_stays_actionable_without_saving_or_starting_a_new_login() {
    let fixture = Fixture::new(json!({"refuse_close":true}));
    let operation = fixture
        .core
        .start_login(&fixture.profile, false, None)
        .expect("start");
    fixture.wait(&operation.id, OperationState::AwaitingLogin);
    fixture
        .core
        .cancel_operation(&operation.id)
        .expect("cancel request");
    let cancelled = fixture.wait(&operation.id, OperationState::Cancelled);
    assert_eq!(
        cancelled.error_code.as_deref(),
        Some("browser_cleanup_pending")
    );
    assert_eq!(fixture.credentials.saves.load(Ordering::Acquire), 0);
    std::fs::write(
        fixture.directory.path().join("bundle/scenario.json"),
        json!({}).to_string(),
    )
    .expect("allow close");
    fixture
        .core
        .retry_browser_cleanup(&operation.id)
        .expect("retry only owned close");
    assert_eq!(fixture.trace("login-processes.jsonl").len(), 1);
    assert!(fixture
        .core
        .database
        .profile(&fixture.profile)
        .expect("no credential association")
        .account_id
        .is_none());
    fixture.assert_login_collected();
}

#[test]
fn cleanup_shortcut_rejects_a_different_operation_before_touching_the_browser() {
    let fixture = Fixture::new(json!({}));
    let operation = fixture
        .core
        .start_login(&fixture.profile, false, None)
        .expect("start");
    let awaiting = fixture.wait(&operation.id, OperationState::AwaitingLogin);
    assert!(matches!(
        fixture.core.retry_browser_cleanup(&operation.id),
        Err(AppError::AccessDenied(_))
    ));
    assert!(
        std::path::Path::new(&format!("/proc/{}", awaiting.browser_pid.expect("PID"))).exists()
    );
    fixture
        .core
        .cancel_operation(&operation.id)
        .expect("cancel");
    fixture.wait(&operation.id, OperationState::Cancelled);
}

#[test]
fn a_stale_cleanup_action_cannot_close_a_new_operations_targets_in_the_same_browser_process() {
    let fixture = Fixture::new(json!({"success":true,"refuse_close":true}));
    fixture
        .core
        .open_profile_browser(&fixture.profile)
        .expect("manual target keeps process");
    let old = fixture
        .core
        .start_login(&fixture.profile, false, None)
        .expect("first login");
    let old_result = fixture.wait(&old.id, OperationState::Completed);
    assert_eq!(
        old_result.error_code.as_deref(),
        Some("browser_cleanup_pending")
    );
    std::fs::write(
        fixture.directory.path().join("bundle/scenario.json"),
        json!({"success":true,"refuse_target":"synthetic-target-3"}).to_string(),
    )
    .expect("close old target; next operation has a new target");
    let next = fixture
        .core
        .start_login(&fixture.profile, true, None)
        .expect("second operation");
    let next_result = fixture.wait(&next.id, OperationState::Completed);
    assert_eq!(
        old_result.browser_pid, next_result.browser_pid,
        "same process is not same operation ownership"
    );
    assert_eq!(
        next_result.error_code.as_deref(),
        Some("browser_cleanup_pending")
    );
    let before = fixture
        .trace("browser-requests.jsonl")
        .iter()
        .filter(|request| request["method"] == "Target.closeTarget")
        .count();
    assert!(fixture.core.retry_browser_cleanup(&old.id).is_err());
    let after = fixture
        .trace("browser-requests.jsonl")
        .iter()
        .filter(|request| request["method"] == "Target.closeTarget")
        .count();
    assert_eq!(
        before, after,
        "stale action must not send a close to the replacement operation"
    );
    std::fs::write(
        fixture.directory.path().join("bundle/scenario.json"),
        json!({"success":true}).to_string(),
    )
    .expect("allow close");
    fixture
        .core
        .retry_browser_cleanup(&next.id)
        .expect("current operation may clean up its targets");
    fixture.assert_login_collected();
}

#[test]
fn failed_partial_creation_keeps_target_ownership_for_automatic_cleanup_before_next_launch() {
    let fixture =
        Fixture::new(json!({"fail_create_number":3,"refuse_target":"synthetic-target-2"}));
    fixture
        .core
        .open_profile_browser(&fixture.profile)
        .expect("manual target 1");
    let profile = fixture
        .core
        .database
        .profile(&fixture.profile)
        .expect("profile");
    let path = std::path::Path::new(&profile.data_dir);
    assert!(fixture
        .core
        .browser
        .launch_controlled(path, "about:blank#login", Some("about:blank#mail"))
        .is_err());
    let before = fixture
        .trace("browser-requests.jsonl")
        .iter()
        .filter(|request| request["method"] == "Target.createTarget")
        .count();
    assert!(fixture
        .core
        .browser
        .launch_controlled(path, "about:blank#next-login", None)
        .is_err());
    let after = fixture
        .trace("browser-requests.jsonl")
        .iter()
        .filter(|request| request["method"] == "Target.createTarget")
        .count();
    assert_eq!(
        before, after,
        "do not create another login while partial targets cannot be closed"
    );
    std::fs::write(
        fixture.directory.path().join("bundle/scenario.json"),
        json!({}).to_string(),
    )
    .expect("allow cleanup");
    let next = fixture
        .core
        .browser
        .launch_controlled(path, "about:blank#next-login", None)
        .expect("automatically clean partial targets first");
    next.terminate().expect("end next operation");
    assert!(next.is_alive().expect("manual root preserved"));
    let closed: Vec<Value> = fixture
        .trace("browser-requests.jsonl")
        .into_iter()
        .filter(|request| request["method"] == "Target.closeTarget")
        .collect();
    assert!(!closed
        .iter()
        .any(|request| request["targetId"] == "synthetic-target-1"));
}

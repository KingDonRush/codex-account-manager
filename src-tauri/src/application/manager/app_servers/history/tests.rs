use super::*;
use crate::{adapters::database::Database, ports::CredentialStore};
use serde_json::Value;
use std::sync::Arc;
struct NeverCredentials;
impl CredentialStore for NeverCredentials {
    fn save(&self, _: &Value) -> Result<String, AppError> {
        unreachable!("history must never access credentials")
    }
    fn load(&self, _: &str) -> Result<Value, AppError> {
        unreachable!("history must never access credentials")
    }
    fn load_optional(&self, _: &str) -> Result<Option<Value>, AppError> {
        unreachable!("history must never access credentials")
    }
    fn delete(&self, _: &str) -> Result<(), AppError> {
        unreachable!("history must never delete credentials")
    }
}
fn core(dir: &std::path::Path) -> ManagerCore {
    ManagerCore::from_components(
        dir.to_path_buf(),
        None,
        Database::open(dir).expect("db"),
        Arc::new(NeverCredentials),
    )
    .expect("core")
}
fn session(
    core: &ManagerCore,
    persistent: bool,
    days_old: i64,
    state: ServerState,
) -> (String, std::path::PathBuf) {
    let id = uuid::Uuid::new_v4().to_string();
    let status = ApplicationServerStatus {
        id: id.clone(),
        events_offset: 0,
        allowed_models: Default::default(),
        default_model: None,
        application_id: "application".to_owned(),
        grant_id: "grant".to_owned(),
        binding_id: "binding".to_owned(),
        binding_revision: 1,
        group_id: None,
        account_id: "local-account".to_owned(),
        state,
        background: false,
        persist_history: persistent,
        created_at: Utc::now() - chrono::Duration::days(days_old),
        last_activity: Utc::now() - chrono::Duration::days(days_old),
        error_code: None,
    };
    core.application_servers.restore(status.clone());
    core.database
        .persist_application_server(&status, "/fixture")
        .expect("metadata");
    let directory = files::session_directory(&core.app_data_dir, &id).expect("directory");
    std::fs::create_dir_all(directory.join("runtime/sessions")).expect("rollout directory");
    std::fs::write(directory.join("events.jsonl"), b"{}\n").expect("events");
    std::fs::write(directory.join("runtime/sessions/rollout.jsonl"), b"{}\n").expect("rollout");
    (id, directory)
}
#[test]
fn retention_keeps_opt_in_rollouts_and_never_removes_active_or_unregistered_data() {
    let dir = tempfile::tempdir().expect("fixture");
    let core = core(dir.path());
    let (_, persistent) = session(&core, true, 8, ServerState::Closed);
    let (_, ephemeral) = session(&core, false, 8, ServerState::Closed);
    let (_, recent) = session(&core, false, 1, ServerState::Closed);
    let (_, active) = session(&core, false, 8, ServerState::Generating);
    let unregistered = files::session_directory(dir.path(), &uuid::Uuid::new_v4().to_string())
        .expect("unregistered");
    std::fs::create_dir_all(&unregistered).expect("unregistered fixture");
    core.cleanup_application_history();
    assert!(!persistent.join("events.jsonl").exists());
    assert!(persistent.join("runtime/sessions/rollout.jsonl").exists());
    assert!(!ephemeral.exists());
    assert!(recent.join("events.jsonl").exists());
    assert!(active.join("events.jsonl").exists());
    assert!(unregistered.exists());
}
#[test]
fn clearing_is_terminal_only_and_does_not_touch_neighbor_sessions() {
    let dir = tempfile::tempdir().expect("fixture");
    let core = core(dir.path());
    let (active_id, active) = session(&core, true, 1, ServerState::Generating);
    let (closed_id, closed) = session(&core, true, 1, ServerState::Closed);
    assert!(matches!(
        core.clear_application_history(&active_id),
        Err(AppError::Busy(_))
    ));
    core.clear_application_history(&closed_id).expect("clear");
    assert!(active.exists());
    assert!(!closed.exists());
    assert!(
        !core
            .application_servers
            .status(&closed_id)
            .expect("receipt")
            .persist_history
    );
}

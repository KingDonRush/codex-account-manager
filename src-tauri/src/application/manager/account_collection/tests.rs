use super::*;
use crate::{adapters::database::Database, ports::CredentialStore};
use std::sync::Arc;
struct OldEntries;
impl CredentialStore for OldEntries {
    fn account_ids(&self) -> Result<Vec<String>, AppError> {
        Ok(["broken-first", "removed", "good", "good"]
            .map(str::to_owned)
            .to_vec())
    }
    fn save(&self, _: &Value) -> Result<String, AppError> {
        unreachable!("collection must not write credentials")
    }
    fn load(&self, id: &str) -> Result<Value, AppError> {
        if id == "broken-first" {
            Err(AppError::Keyring("missing old chunk".to_owned()))
        } else {
            Ok(
                serde_json::json!({"auth_mode":"chatgpt","tokens":{"account_id":id,"access_token":"synthetic","refresh_token":"synthetic"}}),
            )
        }
    }
    fn load_optional(&self, id: &str) -> Result<Option<Value>, AppError> {
        self.load(id).map(Some)
    }
    fn delete(&self, _: &str) -> Result<(), AppError> {
        unreachable!("collection must not delete")
    }
}
#[test]
fn collection_continues_after_a_broken_account_is_idempotent_and_preserves_removals() {
    let dir = tempfile::tempdir().expect("fixture");
    let database = Database::open(dir.path()).expect("db");
    rusqlite::Connection::open(dir.path().join("manager.sqlite3"))
        .expect("fixture connection")
        .execute(
            "INSERT INTO schema_meta VALUES('removed-account:removed','fixture')",
            [],
        )
        .expect("intentional removal");
    let core = ManagerCore::from_components(
        dir.path().to_path_buf(),
        None,
        database,
        Arc::new(OldEntries),
    )
    .expect("core");
    assert_eq!(core.collect_saved_accounts().expect("collect"), 2);
    assert_eq!(core.collect_saved_accounts().expect("repeat"), 0);
    assert!(core
        .database
        .profile_for_account("removed")
        .expect("removed")
        .is_none());
    assert_eq!(
        core.database
            .profile_for_account("broken-first")
            .expect("found")
            .expect("preserved as attention")
            .credential_state,
        crate::domain::CredentialState::Unavailable
    );
    assert!(core
        .database
        .profile_for_account("good")
        .expect("good")
        .is_some());
}

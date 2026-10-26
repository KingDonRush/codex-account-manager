use super::*;
use crate::{
    adapters::database::Database,
    domain::AccountMetadata,
    ports::{CredentialStore, StoredCredentialGeneration},
};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
#[derive(Default)]
struct Credentials {
    reads: AtomicUsize,
    locked: AtomicBool,
}
impl CredentialStore for Credentials {
    fn check_read_access(&self) -> Result<(), AppError> {
        if self.locked.load(Ordering::Acquire) {
            return Err(AppError::CredentialUnavailable(
                "synthetic locked coffer".to_owned(),
            ));
        }
        Ok(())
    }
    fn save(&self, _: &Value) -> Result<String, AppError> {
        Ok("a".to_owned())
    }
    fn load(&self, _: &str) -> Result<Value, AppError> {
        let sequence = self.reads.fetch_add(1, Ordering::AcqRel) + 1;
        Ok(
            json!({"auth_mode":"chatgpt","tokens":{"account_id":"a","access_token":format!("access-{sequence}"),"refresh_token":"never-retained-by-monitor"}}),
        )
    }
    fn load_optional(&self, id: &str) -> Result<Option<Value>, AppError> {
        self.load(id).map(Some)
    }
    fn delete(&self, _: &str) -> Result<(), AppError> {
        Ok(())
    }
}
#[test]
fn monitor_reuses_access_only_until_generation_changes_and_clears_on_coffer_lock() {
    let dir = tempfile::tempdir().expect("fixture");
    let database = Database::open(dir.path()).expect("db");
    let profile = database
        .create_profile("fixture", None, None)
        .expect("profile");
    database
        .persist_connected_account(
            &profile.id,
            AccountMetadata {
                account_id: "a",
                email: None,
                plan_type: None,
                credential_ref: None,
                rate_limits: None,
                observed_at: chrono::Utc::now(),
            },
            None,
        )
        .expect("account");
    let credentials = Arc::new(Credentials::default());
    let core = ManagerCore::from_components(
        dir.path().to_path_buf(),
        None,
        database,
        credentials.clone(),
    )
    .expect("core");
    let profile = core.database.profile(&profile.id).expect("profile");
    let first = core.usage_access_session(&profile).expect("first");
    assert_eq!(first.access_token, "access-1");
    assert_eq!(
        core.usage_access_session(&profile)
            .expect("cached")
            .access_token,
        first.access_token
    );
    assert_eq!(credentials.reads.load(Ordering::Acquire), 1);
    core.database
        .upsert_credential_generation(
            &StoredCredentialGeneration {
                account_id: "a".to_owned(),
                generation_id: "new".to_owned(),
                fingerprint: "new-generation".to_owned(),
                created_at: chrono::Utc::now(),
                is_active: true,
            },
            true,
        )
        .expect("new metadata");
    assert_eq!(
        core.usage_access_session(&profile)
            .expect("new access")
            .access_token,
        "access-2"
    );
    credentials.locked.store(true, Ordering::Release);
    assert!(core.usage_access_session(&profile).is_err());
    assert!(core.usage_access.lock().is_empty());
}

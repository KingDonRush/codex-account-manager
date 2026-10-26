use super::*;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
struct Store {
    deleted: AtomicUsize,
    refs: Mutex<Vec<StoredCredentialGeneration>>,
}
impl CredentialStore for Store {
    fn save(&self, _: &Value) -> Result<String, AppError> {
        unreachable!("no secrets written")
    }
    fn load(&self, _: &str) -> Result<Value, AppError> {
        Ok(serde_json::json!({"tokens":{"access_token":"live"}}))
    }
    fn load_optional(&self, id: &str) -> Result<Option<Value>, AppError> {
        self.load(id).map(Some)
    }
    fn delete(&self, _: &str) -> Result<(), AppError> {
        unreachable!("current session cannot be erased")
    }
    fn generation_refs(&self, _: &str) -> Result<Vec<StoredCredentialGeneration>, AppError> {
        Ok(self.refs.lock().clone())
    }
    fn delete_generation(&self, _: &str, _: &str) -> Result<(), AppError> {
        self.deleted.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
}
fn generation() -> StoredCredentialGeneration {
    StoredCredentialGeneration {
        account_id: "a".to_owned(),
        generation_id: "current".to_owned(),
        fingerprint: "fingerprint".to_owned(),
        created_at: Utc::now(),
        is_active: true,
    }
}
#[test]
fn startup_quarantine_cleanup_never_deletes_current_coffer_even_when_sql_is_stale() {
    let dir = tempfile::tempdir().expect("fixture");
    let database = Database::open(dir.path()).expect("db");
    database
        .upsert_credential_generation(&generation(), true)
        .expect("metadata");
    database
        .quarantine_credential_generation("a", "current", Utc::now() - chrono::Duration::days(8))
        .expect("old erroneous quarantine");
    let store = Arc::new(Store {
        deleted: AtomicUsize::new(0),
        refs: Mutex::new(vec![generation()]),
    });
    let vault = CredentialVault {
        database: database.clone(),
        keyring: store.clone(),
    };
    vault.cleanup_expired_generations().expect("cleanup");
    assert_eq!(store.deleted.load(Ordering::Acquire), 0);
    assert_eq!(
        database
            .credential_generations("a")
            .expect("preserved")
            .len(),
        1
    );
}
#[test]
fn real_access_confirmation_rehabilitates_only_the_unchanged_current_session() {
    let dir = tempfile::tempdir().expect("fixture");
    let database = Database::open(dir.path()).expect("db");
    database
        .upsert_credential_generation(&generation(), true)
        .expect("metadata");
    database
        .quarantine_credential_generation("a", "current", Utc::now())
        .expect("quarantine");
    let store = Arc::new(Store {
        deleted: AtomicUsize::new(0),
        refs: Mutex::new(vec![generation()]),
    });
    let vault = CredentialVault {
        database: database.clone(),
        keyring: store,
    };
    assert!(vault
        .confirm_usable_access(&crate::ports::AccessSession {
            account_id: "a".to_owned(),
            access_token: "old".to_owned()
        })
        .is_err());
    assert_eq!(
        database.credential_generations("a").expect("metadata")[0].state,
        crate::domain::CredentialGenerationState::Quarantined
    );
    vault
        .confirm_usable_access(&crate::ports::AccessSession {
            account_id: "a".to_owned(),
            access_token: "live".to_owned(),
        })
        .expect("remote confirmed");
    assert_eq!(
        database.credential_generations("a").expect("metadata")[0].state,
        crate::domain::CredentialGenerationState::Eligible
    );
}

#[test]
fn archive_keeps_native_and_sql_selection_aligned_without_marking_the_account_ready() {
    crate::adapters::native_keyring::with_test_store(
        keyring_core::mock::Store::new().expect("memory boundary"),
        || {
            let dir = tempfile::tempdir().expect("fixture");
            let database = Database::open(dir.path()).expect("db");
            let profile = database
                .create_profile("Synthetic", None, None)
                .expect("profile");
            database
                .persist_connected_account(
                    &profile.id,
                    crate::domain::AccountMetadata {
                        account_id: "a",
                        email: None,
                        plan_type: None,
                        credential_ref: None,
                        rate_limits: None,
                        observed_at: Utc::now(),
                    },
                    None,
                )
                .expect("initial metadata");
            database
                .set_credential_state("a", crate::domain::CredentialState::Unavailable)
                .expect("unconfirmed coffer state");
            let vault = CredentialVault::new(database.clone());
            let selected = serde_json::json!({"auth_mode":"chatgpt","tokens":{"account_id":"a","access_token":"synthetic-new-login","refresh_token":"synthetic-new-refresh"}});
            let primary = vault.save_generation(&selected).expect("selected");
            let mut snapshot = selected.clone();
            snapshot["tokens"]["access_token"] = serde_json::json!("synthetic-old-desktop");
            snapshot["tokens"]["refresh_token"] = serde_json::json!("synthetic-old-refresh");
            let archived = vault.archive_generation(&snapshot).expect("archive");
            assert!(!archived.is_active);
            assert_eq!(vault.load("a").expect("selected preserved"), selected);
            let metadata = database.credential_generations("a").expect("metadata");
            assert!(metadata
                .iter()
                .any(|g| g.generation_id == archived.generation_id));
            let connection = rusqlite::Connection::open_with_flags(
                dir.path().join("manager.sqlite3"),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .expect("fixture-only SQL observation");
            let active: Vec<String> = connection.prepare("SELECT generation_id FROM credential_generations WHERE account_id='a' AND is_active=1").expect("query").query_map([], |row| row.get(0)).expect("rows").collect::<Result<_, _>>().expect("active metadata");
            assert_eq!(active, vec![primary.generation_id.clone()]);
            assert_eq!(
                database
                    .profile(&profile.id)
                    .expect("profile state")
                    .credential_state,
                crate::domain::CredentialState::Unavailable
            );
            assert_eq!(
                database
                    .active_credential_signature("a")
                    .expect("signature"),
                Some(format!("{}:{}", primary.generation_id, primary.fingerprint))
            );
        },
    );
}

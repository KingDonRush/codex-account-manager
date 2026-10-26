use super::*;
use crate::adapters::database::Database;
use crate::domain::AccountSnapshot;
use crate::ports::CredentialStore;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{atomic::AtomicBool, Arc};

#[derive(Default)]
struct FakeCredentials {
    documents: Mutex<HashMap<String, Value>>,
    fail_old_delete: AtomicBool,
    fail_new_save: AtomicBool,
}

impl CredentialStore for FakeCredentials {
    fn save(&self, document: &Value) -> Result<String, AppError> {
        let id = auth_store::validate_auth_document(document, None)?;
        if id == "new" && self.fail_new_save.load(Ordering::Acquire) {
            return Err(AppError::CredentialUnavailable(
                "injected save failure".to_owned(),
            ));
        }
        self.documents.lock().insert(id.clone(), document.clone());
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
        if id == "old" && self.fail_old_delete.load(Ordering::Acquire) {
            return Err(AppError::CredentialUnavailable("injected".to_owned()));
        }
        self.documents.lock().remove(id);
        Ok(())
    }
}

fn document(id: &str) -> Value {
    json!({"auth_mode":"chatgpt","tokens":{"access_token":"synthetic-access","refresh_token":"synthetic-refresh","account_id":id}})
}

fn fixture() -> (
    tempfile::TempDir,
    ManagerCore,
    ManagedProfile,
    Arc<FakeCredentials>,
) {
    let directory = tempfile::tempdir().expect("fixture");
    let db = Database::open(directory.path()).expect("database");
    let profile = db
        .create_profile("B3", Some("yahoo"), Some("https://mail.yahoo.com/"))
        .expect("profile");
    db.persist_connected_account(
        &profile.id,
        AccountMetadata {
            account_id: "old",
            email: Some("old@yahoo.com"),
            plan_type: Some("plus"),
            credential_ref: None,
            rate_limits: None,
            observed_at: Utc::now(),
        },
        Some("yahoo"),
    )
    .expect("association");
    let credentials = Arc::new(FakeCredentials::default());
    credentials
        .save(&document("old"))
        .expect("synthetic credential");
    let manager = ManagerCore::from_components(
        directory.path().to_path_buf(),
        None,
        db,
        credentials.clone(),
    )
    .expect("core");
    let profile = manager
        .database
        .profile(&profile.id)
        .expect("connected profile");
    (directory, manager, profile, credentials)
}

fn candidate() -> VerifiedLogin {
    VerifiedLogin {
        document: document("new"),
        account_id: "new".to_owned(),
        snapshot: AccountSnapshot {
            email: Some("new@proton.me".to_owned()),
            plan_type: Some("plus".to_owned()),
            rate_limits: None,
        },
    }
}

#[test]
fn keyring_failure_preserves_the_association_and_removes_partial_candidate() {
    let (_directory, manager, profile, credentials) = fixture();
    credentials.fail_new_save.store(true, Ordering::Release);
    assert!(manager
        .commit_candidate(&profile, &candidate(), Some("proton"))
        .is_err());
    assert_eq!(
        manager
            .database
            .profile(&profile.id)
            .expect("profile")
            .account_id
            .as_deref(),
        Some("old")
    );
    assert_eq!(credentials.load("old").expect("old"), document("old"));
    assert!(credentials
        .load_optional("new")
        .expect("candidate")
        .is_none());
}

#[test]
fn database_rejection_rolls_back_both_credentials() {
    let (_directory, manager, profile, credentials) = fixture();
    let neighbor = manager
        .database
        .create_profile("Neighbor", None, None)
        .expect("neighbor");
    manager
        .database
        .persist_connected_account(
            &neighbor.id,
            AccountMetadata {
                account_id: "new",
                email: None,
                plan_type: None,
                credential_ref: None,
                rate_limits: None,
                observed_at: Utc::now(),
            },
            None,
        )
        .expect("conflicting fixture");
    assert!(manager
        .commit_candidate(&profile, &candidate(), Some("proton"))
        .is_err());
    assert_eq!(credentials.load("old").expect("old"), document("old"));
    assert!(credentials
        .load_optional("new")
        .expect("candidate")
        .is_none());
    assert_eq!(
        manager
            .database
            .profile(&profile.id)
            .expect("profile")
            .account_id
            .as_deref(),
        Some("old")
    );
    assert!(manager.database.profile(&neighbor.id).is_ok());
}

#[test]
fn successful_commit_keeps_profile_and_cleans_the_replaced_credential() {
    let (_directory, manager, profile, credentials) = fixture();
    manager
        .commit_candidate(&profile, &candidate(), Some("proton"))
        .expect("replacement");
    manager.cleanup_replaced_account("old", &profile.id);
    let current = manager.database.profile(&profile.id).expect("profile");
    assert_eq!(current.name, "B3");
    assert_eq!(current.data_dir, profile.data_dir);
    assert_eq!(current.account_id.as_deref(), Some("new"));
    assert!(current.mail_url.is_none());
    assert!(credentials.load_optional("old").expect("old").is_none());
    assert_eq!(credentials.load("new").expect("new"), document("new"));
}

#[test]
fn unavailable_old_credential_cleanup_does_not_roll_back_the_committed_account() {
    let (_directory, manager, profile, credentials) = fixture();
    credentials.fail_old_delete.store(true, Ordering::Release);
    manager
        .commit_candidate(&profile, &candidate(), Some("proton"))
        .expect("committed replacement");
    manager.cleanup_replaced_account("old", &profile.id);
    assert_eq!(
        manager
            .database
            .profile(&profile.id)
            .expect("profile")
            .account_id
            .as_deref(),
        Some("new")
    );
    assert_eq!(
        manager.database.pending_account_cleanup().expect("pending"),
        vec![("old".to_owned(), profile.id.clone())]
    );
    assert_eq!(credentials.load("old").expect("old"), document("old"));
    credentials.fail_old_delete.store(false, Ordering::Release);
    manager.cleanup_replaced_account("old", &profile.id);
    assert!(manager
        .database
        .pending_account_cleanup()
        .expect("pending")
        .is_empty());
    assert!(credentials.load_optional("old").expect("removed").is_none());
}

#[test]
fn confirmation_gate_cancels_and_times_out_without_saving_credentials() {
    let control = OperationControl {
        profile_id: "fixture".to_owned(),
        kind: OperationKind::Replace,
        cancel: Arc::new(AtomicBool::new(false)),
        cancelable: AtomicBool::new(true),
        cancellation_gate: Mutex::new(()),
        browser: Arc::new(Mutex::new(None)),
        replacement: Mutex::new(None),
        confirmed: AtomicBool::new(false),
    };
    assert!(matches!(
        wait_for_confirmation(&control, None, Duration::ZERO),
        Err(AppError::TimedOut)
    ));
    control.cancel.store(true, Ordering::Release);
    control.confirmed.store(true, Ordering::Release);
    assert!(matches!(
        wait_for_confirmation(&control, None, Duration::ZERO),
        Err(AppError::Cancelled)
    ));
    control.cancel.store(false, Ordering::Release);
    assert!(wait_for_confirmation(&control, None, Duration::ZERO).is_ok());
}

use super::*;
use crate::adapters::database::Database;
use crate::domain::execution::{ExecutionState, ExecutionStatus};
use crate::domain::integrations::GrantScope;
use crate::domain::{AccountMetadata, ManagedProfile};
use crate::ports::CredentialStore;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

#[derive(Default)]
struct Credentials {
    deletions: AtomicUsize,
}
impl CredentialStore for Credentials {
    fn save(&self, _: &Value) -> Result<String, AppError> {
        Ok("fixture".to_owned())
    }
    fn load(&self, _: &str) -> Result<Value, AppError> {
        Ok(json!({"synthetic":true}))
    }
    fn load_optional(&self, _: &str) -> Result<Option<Value>, AppError> {
        Ok(Some(json!({"synthetic":true})))
    }
    fn delete(&self, _: &str) -> Result<(), AppError> {
        self.deletions.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
}
fn fixture() -> (tempfile::TempDir, ManagerCore, Arc<Credentials>) {
    let dir = tempfile::tempdir().expect("fixture");
    let credentials = Arc::new(Credentials::default());
    let core = ManagerCore::from_components(
        dir.path().to_path_buf(),
        None,
        Database::open(dir.path()).expect("db"),
        credentials.clone(),
    )
    .expect("core");
    (dir, core, credentials)
}
fn account(core: &ManagerCore, remote: &str) -> (ManagedProfile, String) {
    let profile = core
        .database
        .create_profile(remote, None, None)
        .expect("profile");
    core.database
        .persist_connected_account(
            &profile.id,
            AccountMetadata {
                account_id: remote,
                email: None,
                plan_type: None,
                credential_ref: None,
                rate_limits: None,
                observed_at: chrono::Utc::now(),
            },
            None,
        )
        .expect("account");
    let local = core
        .database
        .account_integration_options(remote)
        .expect("local ID")
        .0;
    (core.database.profile(&profile.id).expect("profile"), local)
}
fn job() -> ExecutionStatus {
    ExecutionStatus {
        id: "queued-fixture".to_owned(),
        application_id: "application".to_owned(),
        grant_id: "grant".to_owned(),
        binding_id: "binding".to_owned(),
        binding_revision: 1,
        group_id: None,
        account_id: None,
        state: ExecutionState::Queued,
        started_at: chrono::Utc::now(),
        finished_at: None,
        error_code: None,
        exit_code: None,
        background: false,
    }
}

#[test]
fn unique_and_batch_deletion_only_remove_explicitly_confirmed_accounts() {
    let (_dir, core, credentials) = fixture();
    let (first, a) = account(&core, "a");
    let (second, b) = account(&core, "b");
    let request = core
        .request_deletion(None, &BTreeSet::from([a.clone(), b.clone()]))
        .expect("request");
    assert_eq!(credentials.deletions.load(Ordering::Acquire), 0);
    let receipt = core
        .confirm_deletion(&request.id, BTreeSet::from([b.clone()]), false)
        .expect("confirm one");
    assert_eq!(receipt.deleted_ids, [b]);
    assert_eq!(
        core.database
            .profile(&first.id)
            .expect("kept")
            .account_id
            .as_deref(),
        Some("a")
    );
    assert!(core
        .database
        .profile(&second.id)
        .expect("browser profile kept")
        .account_id
        .is_none());
    assert!(std::path::Path::new(&second.data_dir).exists());
    assert_eq!(credentials.deletions.load(Ordering::Acquire), 1);
    assert!(core
        .confirm_deletion(&request.id, BTreeSet::from([a]), false)
        .is_err());
}
#[test]
fn changed_preview_or_revoked_grant_never_deletes_credentials() {
    let (_dir, core, credentials) = fixture();
    let (profile, id) = account(&core, "a");
    let request = core
        .request_deletion(None, &BTreeSet::from([id.clone()]))
        .expect("request");
    core.rename_profile(&profile.id, "renamed").expect("rename");
    assert!(matches!(
        core.confirm_deletion(&request.id, BTreeSet::from([id.clone()]), false),
        Err(AppError::Busy(_))
    ));
    let mut scope = GrantScope::default();
    scope.permissions.insert(Permission::RequestDeletion);
    scope.account_ids.insert(id.clone());
    let grant = AuthorizationGrant {
        id: "grant".to_owned(),
        application_id: "fixture".to_owned(),
        scope,
        verifier: "unused".to_owned(),
        peer_identity: "fixture".to_owned(),
        revoked: false,
        created_at: chrono::Utc::now(),
    };
    core.database.save_grant(&grant).expect("grant");
    let request = core
        .request_deletion(Some(&grant), &BTreeSet::from([id.clone()]))
        .expect("request");
    core.revoke_authorization(&grant.id).expect("revoke");
    assert!(core
        .confirm_deletion(&request.id, BTreeSet::from([id]), false)
        .is_err());
    assert_eq!(credentials.deletions.load(Ordering::Acquire), 0);
    assert!(core
        .database
        .profile(&profile.id)
        .expect("kept")
        .account_id
        .is_some());
}
#[test]
fn reserved_generation_blocks_deletion_before_worker_updates_its_snapshot() {
    let (_dir, core, credentials) = fixture();
    let (profile, id) = account(&core, "a");
    core.executions
        .add(
            job(),
            Arc::new(AtomicBool::new(false)),
            BTreeSet::from([id.clone()]),
        )
        .expect("queue");
    let lease = core
        .executions
        .try_reserve("queued-fixture", &id, "session", 1)
        .expect("reserve");
    assert!(core.executions.account_busy(&id));
    let request = core
        .request_deletion(None, &BTreeSet::from([id.clone()]))
        .expect("request");
    assert!(matches!(
        core.confirm_deletion(&request.id, BTreeSet::from([id]), false),
        Err(AppError::Busy(_))
    ));
    assert_eq!(credentials.deletions.load(Ordering::Acquire), 0);
    assert!(core
        .database
        .profile(&profile.id)
        .expect("kept")
        .account_id
        .is_some());
    drop(lease);
}

use super::*;
use crate::{
    adapters::{
        database::Database,
        desktop_control::{DesktopLaunchOutcome, DesktopState},
        session_switch::SessionSwitcher,
    },
    domain::AccountSnapshot,
    ports::{CredentialStore, DesktopLifecycle, DestinationPreparation, SessionVerifier},
};
use parking_lot::Mutex;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Store {
    doc: Mutex<Value>,
    saves: AtomicUsize,
}
impl CredentialStore for Store {
    fn save(&self, doc: &Value) -> Result<String, AppError> {
        *self.doc.lock() = doc.clone();
        self.saves.fetch_add(1, Ordering::Relaxed);
        Ok("remote".to_owned())
    }
    fn load(&self, expected: &str) -> Result<Value, AppError> {
        let doc = self.doc.lock().clone();
        auth_store::validate_auth_document(&doc, Some(expected))?;
        Ok(doc)
    }
    fn load_optional(&self, expected: &str) -> Result<Option<Value>, AppError> {
        self.load(expected).map(Some)
    }
    fn delete(&self, _: &str) -> Result<(), AppError> {
        panic!("must not delete")
    }
}
struct NoDesktop;
impl DesktopLifecycle for NoDesktop {
    fn stop(&self) -> Result<DesktopState, AppError> {
        panic!("applications cannot stop Desktop")
    }
    fn restart(&self, _: &DesktopState) -> Result<DesktopLaunchOutcome, AppError> {
        panic!("applications cannot restart Desktop")
    }
}
struct Renew;
impl SessionVerifier for Renew {
    fn prepare_destination(
        &self,
        _: &str,
        doc: &Value,
        persist: &dyn Fn(&Value) -> Result<(), AppError>,
        _: Arc<AtomicBool>,
    ) -> Result<DestinationPreparation, AppError> {
        let mut renewed = doc.clone();
        renewed["tokens"]["access_token"] = json!("renewed-access");
        renewed["tokens"]["refresh_token"] = json!("renewed-refresh");
        persist(&renewed)?;
        Ok(DestinationPreparation::Verified)
    }
    fn verify(&self, _: &str) -> Result<AccountSnapshot, AppError> {
        panic!("not a switch")
    }
}
fn document(remote: &str, access: &str) -> Value {
    json!({"auth_mode":"chatgpt","tokens":{"account_id":remote,"access_token":access,"refresh_token":"synthetic-refresh","id_token":"synthetic-id"}})
}
fn fixture() -> (tempfile::TempDir, ManagerCore, Arc<Store>) {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    let store = Arc::new(Store {
        doc: Mutex::new(document("remote", "coffer-access")),
        saves: AtomicUsize::new(0),
    });
    let mut core =
        ManagerCore::from_components(dir.path().to_path_buf(), None, db, store.clone()).unwrap();
    core.session_switcher = Arc::new(SessionSwitcher::with_dependencies(
        dir.path(),
        core.codex_home.join("auth.json"),
        store.clone(),
        Arc::new(Renew),
        Arc::new(NoDesktop),
    ));
    (dir, core, store)
}
#[test]
fn active_account_uses_latest_owner_access_without_renewing_or_overwriting_it() {
    let (_dir, core, store) = fixture();
    let path = core.codex_home.join("auth.json");
    let source = ApplicationAuth::Account("account:local".to_owned());
    for value in ["owner-access-1", "owner-access-2"] {
        let doc = document("remote", value);
        auth_store::atomic_write_auth(&path, &doc).unwrap();
        let loaded = core
            .load_application_auth(&source, "remote", Arc::new(AtomicBool::new(false)), true)
            .unwrap();
        assert_eq!(loaded, doc);
        assert_eq!(auth_store::read_auth(&path).unwrap(), doc);
    }
    assert_eq!(store.saves.load(Ordering::Relaxed), 0);
}
#[test]
fn inactive_renewal_is_persisted_centrally_without_touching_desktop_or_creating_app_session() {
    let (_dir, core, store) = fixture();
    let path = core.codex_home.join("auth.json");
    let other = document("other", "other-access");
    auth_store::atomic_write_auth(&path, &other).unwrap();
    let source = core.application_auth("local").unwrap();
    assert!(matches!(source, ApplicationAuth::Account(_)));
    let loaded = core
        .load_application_auth(&source, "remote", Arc::new(AtomicBool::new(false)), true)
        .unwrap();
    assert_eq!(loaded["tokens"]["access_token"], "renewed-access");
    assert_eq!(store.saves.load(Ordering::Relaxed), 1);
    assert_eq!(auth_store::read_auth(&path).unwrap(), other);
    assert!(core
        .database
        .application_session("local")
        .unwrap()
        .is_none());
    assert!(core
        .load_application_auth(&source, "wrong", Arc::new(AtomicBool::new(false)), false)
        .is_err());
}
#[test]
fn cancellation_during_global_switch_lock_wait_does_not_read_or_refresh_the_chain() {
    let (_dir, core, store) = fixture();
    let _lock = FileLock::acquire(&core.codex_home.join("auth-switch.lock")).unwrap();
    let error = core.load_application_auth(
        &ApplicationAuth::Account("account:local".to_owned()),
        "remote",
        Arc::new(AtomicBool::new(true)),
        true,
    );
    assert!(matches!(error, Err(AppError::Cancelled)));
    assert_eq!(store.saves.load(Ordering::Relaxed), 0);
}

#[test]
fn a_restarted_client_with_the_same_slug_requests_an_update_but_gains_no_access_before_approval() {
    use crate::application::{authorization::ConnectionRequest, local_transport::PeerIdentity};
    use crate::domain::integrations::{Application, GrantScope};
    let (_dir, core, _store) = fixture();
    let app = core
        .database
        .save_application(&Application {
            id: uuid::Uuid::new_v4().to_string(),
            name: "Existing app".to_owned(),
            slug: "stable-app".to_owned(),
            paused: false,
            revision: 0,
        })
        .unwrap();
    let peer = PeerIdentity {
        pid: 123,
        executable: "synthetic-cli".to_owned(),
        identity: "synthetic-peer".to_owned(),
    };
    let request = core
        .request_authorization(
            ConnectionRequest {
                application_id: None,
                name: "Declared name".to_owned(),
                slug: app.slug.clone(),
                requested_scope: GrantScope::default(),
            },
            &peer,
            "synthetic-channel",
        )
        .unwrap();
    assert_eq!(
        request.request.application_id.as_deref(),
        Some(app.id.as_str())
    );
    assert!(core.database.grants().unwrap().is_empty());
    assert!(core
        .claim_authorization(&request.id, "synthetic-channel", &peer.identity)
        .unwrap()
        .is_none());
    core.deny_authorization(&request.id);
    assert_eq!(core.database.applications().unwrap().len(), 1);
    assert!(core.database.grants().unwrap().is_empty());
}

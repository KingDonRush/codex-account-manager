use super::*;
use crate::ports::ApplicationSessionStore;
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
struct FakeStore {
    documents: Mutex<HashMap<String, Value>>,
    unavailable: AtomicBool,
}
impl ApplicationSessionStore for FakeStore {
    fn save(&self, id: &str, document: &Value) -> Result<(), AppError> {
        if self.unavailable.load(Ordering::Acquire) {
            return Err(AppError::CredentialUnavailable(
                "synthetic failure".to_owned(),
            ));
        }
        self.documents
            .lock()
            .insert(id.to_owned(), document.clone());
        Ok(())
    }
    fn load(&self, id: &str, _: &str) -> Result<Value, AppError> {
        self.documents
            .lock()
            .get(id)
            .cloned()
            .ok_or_else(|| AppError::AuthenticationRequired("fixture missing".to_owned()))
    }
    fn delete(&self, id: &str) -> Result<(), AppError> {
        self.documents.lock().remove(id);
        Ok(())
    }
}
#[derive(Default)]
struct FakeKeys {
    locked: AtomicBool,
}
impl RecoveryKeyStore for FakeKeys {
    fn load_or_create(&self, _: &str) -> Result<[u8; 32], AppError> {
        if self.locked.load(Ordering::Acquire) {
            return Err(AppError::CredentialUnavailable(
                "synthetic locked key".to_owned(),
            ));
        }
        Ok([71; 32])
    }
    fn delete(&self, _: &str) -> Result<(), AppError> {
        Ok(())
    }
}
fn document(version: &str) -> Value {
    json!({"auth_mode":"chatgpt","tokens":{"account_id":"remote-fixture","access_token":format!("private-access-{version}"),"refresh_token":format!("private-refresh-{version}")}})
}
fn store(dir: &Path, inner: &Arc<FakeStore>, keys: &Arc<FakeKeys>) -> RecoverableApplicationStore {
    RecoverableApplicationStore::new(dir, inner.clone(), keys.clone())
}

#[test]
fn coffer_failure_after_rotation_preserves_only_encrypted_new_session_and_recovers_after_restart() {
    let dir = tempfile::tempdir().expect("fixture");
    let inner = Arc::new(FakeStore::default());
    let keys = Arc::new(FakeKeys::default());
    let id = uuid::Uuid::new_v4().to_string();
    inner.save(&id, &document("old")).expect("seed");
    let adapter = store(dir.path(), &inner, &keys);
    adapter
        .load(&id, "remote-fixture")
        .expect("prepare before work");
    keys.locked.store(true, Ordering::Release);
    inner.unavailable.store(true, Ordering::Release);
    assert!(adapter.save(&id, &document("new")).is_err());
    let encrypted = std::fs::read(adapter.path(&id).expect("path")).expect("durable record");
    let text = String::from_utf8(encrypted).expect("envelope");
    assert!(
        !text.contains("private-refresh")
            && !text.contains("private-access")
            && !text.contains("remote-fixture")
    );
    assert_eq!(
        adapter.pending_sessions().expect("pending").as_slice(),
        std::slice::from_ref(&id)
    );
    drop(adapter);
    let restarted = store(dir.path(), &inner, &keys);
    assert!(restarted.load(&id, "remote-fixture").is_err());
    assert_eq!(
        inner.load(&id, "remote-fixture").expect("old stored"),
        document("old")
    );
    keys.locked.store(false, Ordering::Release);
    inner.unavailable.store(false, Ordering::Release);
    assert_eq!(
        restarted.load(&id, "remote-fixture").expect("recovered"),
        document("new")
    );
    assert!(restarted.pending_sessions().expect("cleaned").is_empty());
}

#[test]
fn crash_after_coffer_commit_is_idempotent_and_journal_binding_rejects_another_session() {
    let dir = tempfile::tempdir().expect("fixture");
    let inner = Arc::new(FakeStore::default());
    let keys = Arc::new(FakeKeys::default());
    let id = uuid::Uuid::new_v4().to_string();
    let other = uuid::Uuid::new_v4().to_string();
    let adapter = store(dir.path(), &inner, &keys);
    let sealed = crypto::seal(&[71; 32], &id, &document("new")).expect("seal");
    adapter.write_pending(&id, &sealed).expect("journal");
    inner.save(&id, &document("new")).expect("coffer commit");
    adapter
        .write_pending(&other, &sealed)
        .expect("foreign envelope fixture");
    assert!(adapter.load(&other, "remote-fixture").is_err());
    assert_eq!(
        adapter
            .load(&id, "remote-fixture")
            .expect("idempotent commit"),
        document("new")
    );
    assert!(!adapter.path(&id).expect("path").exists());
    assert!(adapter.path(&other).expect("foreign kept").exists());
}

#[test]
fn corruption_wrong_identity_and_invalid_id_never_fall_back_to_old_refresh() {
    let dir = tempfile::tempdir().expect("fixture");
    let inner = Arc::new(FakeStore::default());
    let keys = Arc::new(FakeKeys::default());
    let id = uuid::Uuid::new_v4().to_string();
    let adapter = store(dir.path(), &inner, &keys);
    inner.save(&id, &document("old")).expect("seed");
    adapter
        .write_pending(&id, b"corrupt")
        .expect("corrupt fixture");
    assert!(adapter.load(&id, "remote-fixture").is_err());
    assert!(adapter.save(&id, &document("new")).is_err());
    // Only replace this synthetic corrupted file as explicit test setup.
    std::fs::remove_file(adapter.path(&id).expect("exact fixture path"))
        .expect("fixture replacement");
    let sealed = crypto::seal(&[71; 32], &id, &document("new")).expect("seal");
    adapter.write_pending(&id, &sealed).expect("journal");
    assert!(adapter.load(&id, "different-remote").is_err());
    assert_eq!(
        inner.load(&id, "remote-fixture").expect("still old"),
        document("old")
    );
    assert!(adapter.save("../outside", &document("new")).is_err());
    assert!(adapter.delete("../outside").is_err());
}

#[test]
fn prepared_journal_keys_do_not_bypass_a_locked_coffer_on_a_new_load() {
    let dir = tempfile::tempdir().expect("fixture");
    let inner = Arc::new(FakeStore::default());
    let keys = Arc::new(FakeKeys::default());
    let id = uuid::Uuid::new_v4().to_string();
    inner.save(&id, &document("old")).expect("seed");
    let adapter = store(dir.path(), &inner, &keys);
    adapter.load(&id, "remote-fixture").expect("prepared");
    keys.locked.store(true, Ordering::Release);
    assert!(adapter.load(&id, "remote-fixture").is_err());
}

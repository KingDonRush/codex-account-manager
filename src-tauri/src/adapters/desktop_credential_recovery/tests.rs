use super::*;
use parking_lot::Mutex;
use serde_json::json;
use std::{
    collections::HashMap,
    sync::atomic::{AtomicBool, Ordering},
};
#[derive(Default)]
struct Store {
    docs: Mutex<HashMap<String, Value>>,
    unavailable: AtomicBool,
}
impl CredentialStore for Store {
    fn save(&self, document: &Value) -> Result<String, AppError> {
        if self.unavailable.load(Ordering::Acquire) {
            return Err(AppError::CredentialUnavailable(
                "synthetic store failure".to_owned(),
            ));
        }
        let id = auth_store::validate_auth_document(document, None)?;
        self.docs.lock().insert(id.clone(), document.clone());
        Ok(id)
    }
    fn load(&self, id: &str) -> Result<Value, AppError> {
        self.docs
            .lock()
            .get(id)
            .cloned()
            .ok_or_else(|| AppError::Keyring("missing fixture".to_owned()))
    }
    fn load_optional(&self, id: &str) -> Result<Option<Value>, AppError> {
        Ok(self.docs.lock().get(id).cloned())
    }
    fn delete(&self, id: &str) -> Result<(), AppError> {
        self.docs.lock().remove(id);
        Ok(())
    }
}
struct Keys;
impl RecoveryKeyStore for Keys {
    fn load_or_create(&self, _: &str) -> Result<[u8; 32], AppError> {
        Ok([57; 32])
    }
    fn delete(&self, _: &str) -> Result<(), AppError> {
        Ok(())
    }
}
fn document(value: &str) -> Value {
    json!({"auth_mode":"chatgpt","tokens":{"account_id":"old-03-remote","access_token":value,"refresh_token":format!("private-{value}")}})
}
#[test]
fn upgrade_keeps_original_coffer_key_and_recovers_a_rotated_desktop_chain_after_restart() {
    let dir = tempfile::tempdir().expect("fixture");
    let inner = Arc::new(Store::default());
    inner.save(&document("original-03")).expect("old coffer");
    let adapter = RecoverableDesktopStore::new(dir.path(), inner.clone(), Arc::new(Keys));
    adapter
        .prepare_recovery("old-03-remote")
        .expect("key before renew");
    inner.unavailable.store(true, Ordering::Release);
    assert!(matches!(
        adapter.save(&document("new-05")),
        Err(AppError::CredentialRecoveryPending(_))
    ));
    assert!(matches!(
        adapter.archive_generation(&document("original-03")),
        Err(AppError::CredentialRecoveryPending(_))
    ));
    assert_eq!(
        adapter
            .recovery
            .pending_sessions()
            .expect("protected record")
            .len(),
        1
    );
    drop(adapter);
    let restarted = RecoverableDesktopStore::new(dir.path(), inner.clone(), Arc::new(Keys));
    assert!(restarted.load("old-03-remote").is_err());
    inner.unavailable.store(false, Ordering::Release);
    assert_eq!(
        restarted.load("old-03-remote").expect("recovered"),
        document("new-05")
    );
    assert_eq!(inner.docs.lock().len(), 1);
    assert!(inner.docs.lock().contains_key("old-03-remote"));
}

#[test]
fn archive_delegates_to_the_real_writer_without_enqueuing_a_primary_recovery_write() {
    crate::adapters::native_keyring::with_test_store(
        keyring_core::mock::Store::new().expect("memory boundary"),
        || {
            let dir = tempfile::tempdir().expect("fixture");
            let inner = Arc::new(crate::adapters::keyring_store::KeyringStore);
            inner
                .save(&document("selected-new-login"))
                .expect("selected");
            let selected = inner.generation_refs("old-03-remote").expect("selection")[0]
                .generation_id
                .clone();
            let adapter = RecoverableDesktopStore::new(dir.path(), inner.clone(), Arc::new(Keys));
            let archived = adapter
                .archive_generation(&document("outgoing-old-desktop"))
                .expect("archive");
            assert!(!archived.is_active);
            assert!(adapter
                .recovery
                .pending_sessions()
                .expect("no queued write")
                .is_empty());
            drop(adapter);
            let reopened = RecoverableDesktopStore::new(dir.path(), inner, Arc::new(Keys));
            assert_eq!(
                reopened.load("old-03-remote").expect("selected intact"),
                document("selected-new-login")
            );
            assert_eq!(
                reopened
                    .load_generation("old-03-remote", &selected)
                    .expect("same selection"),
                document("selected-new-login")
            );
            assert_eq!(
                reopened
                    .load_generation("old-03-remote", &archived.generation_id)
                    .expect("outgoing preserved"),
                document("outgoing-old-desktop")
            );
        },
    );
}

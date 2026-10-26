//! Recovery protects the Desktop refresh chain without renaming old coffer entries.
use crate::{
    adapters::{
        application_recovery::{RecoverableApplicationStore, RecoveryKeyStore},
        auth_store,
    },
    error::AppError,
    ports::{AccessSession, ApplicationSessionStore, CredentialStore, StoredCredentialGeneration},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc};
#[cfg(test)]
mod tests;
struct Archive(Arc<dyn CredentialStore>);
impl ApplicationSessionStore for Archive {
    fn save(&self, _: &str, document: &Value) -> Result<(), AppError> {
        self.0.save(document).map(|_| ())
    }
    fn load(&self, _: &str, expected: &str) -> Result<Value, AppError> {
        self.0.load(expected)
    }
    fn delete(&self, _: &str) -> Result<(), AppError> {
        Err(AppError::AccessDenied(
            "Use a exclusão explícita da conta.".to_owned(),
        ))
    }
}
pub struct RecoverableDesktopStore {
    inner: Arc<dyn CredentialStore>,
    recovery: RecoverableApplicationStore,
}
impl RecoverableDesktopStore {
    pub fn new(
        root: &Path,
        inner: Arc<dyn CredentialStore>,
        keys: Arc<dyn RecoveryKeyStore>,
    ) -> Self {
        Self {
            recovery: RecoverableApplicationStore::new(
                &root.join("desktop-recovery"),
                Arc::new(Archive(inner.clone())),
                keys,
            ),
            inner,
        }
    }
    fn session_id(remote: &str) -> String {
        let hash = Sha256::digest(format!("desktop-recovery:{remote}").as_bytes());
        let mut bytes = [0_u8; 16];
        bytes.copy_from_slice(&hash[..16]);
        uuid::Uuid::from_bytes(bytes).to_string()
    }
    fn pending(&self, remote: &str) -> Result<bool, AppError> {
        Ok(self
            .recovery
            .pending_sessions()?
            .contains(&Self::session_id(remote)))
    }
}
impl CredentialStore for RecoverableDesktopStore {
    fn close(&self) {
        self.recovery.close();
    }
    fn account_ids(&self) -> Result<Vec<String>, AppError> {
        self.inner.account_ids()
    }
    fn check_read_access(&self) -> Result<(), AppError> {
        self.inner.check_read_access()
    }
    fn prepare_recovery(&self, id: &str) -> Result<(), AppError> {
        self.recovery.load(&Self::session_id(id), id).map(|_| ())
    }
    fn confirm_usable_access(&self, access: &AccessSession) -> Result<(), AppError> {
        self.inner.confirm_usable_access(access)
    }
    fn save(&self, doc: &Value) -> Result<String, AppError> {
        Ok(self.save_generation(doc)?.account_id)
    }
    fn save_generation(&self, doc: &Value) -> Result<StoredCredentialGeneration, AppError> {
        let remote = auth_store::validate_auth_document(doc, None)?;
        self.recovery.save(&Self::session_id(&remote), doc)?;
        self.inner
            .generation_refs(&remote)?
            .into_iter()
            .find(|reference| reference.is_active)
            .ok_or_else(|| {
                AppError::CredentialUnavailable("Sessão salva sem referência ativa.".to_owned())
            })
    }
    fn archive_generation(&self, doc: &Value) -> Result<StoredCredentialGeneration, AppError> {
        let id = auth_store::validate_auth_document(doc, None)?;
        if self.pending(&id)? {
            return Err(AppError::CredentialRecoveryPending(
                "Conclua a recuperação protegida antes de arquivar a sessão anterior.".to_owned(),
            ));
        }
        // The caller still owns the unmodified global auth file. This operation
        // must not enqueue a recovery write that would promote it over a new login.
        self.inner.archive_generation(doc)
    }
    fn load(&self, id: &str) -> Result<Value, AppError> {
        if self.pending(id)? {
            return self.recovery.load(&Self::session_id(id), id);
        }
        self.inner.load(id)
    }
    fn load_optional(&self, id: &str) -> Result<Option<Value>, AppError> {
        if self.pending(id)? {
            return self.load(id).map(Some);
        }
        self.inner.load_optional(id)
    }
    fn generation_refs(&self, id: &str) -> Result<Vec<StoredCredentialGeneration>, AppError> {
        if self.pending(id)? {
            self.load(id)?;
        }
        self.inner.generation_refs(id)
    }
    fn load_generation(&self, id: &str, generation: &str) -> Result<Value, AppError> {
        if self.pending(id)? {
            self.load(id)?;
            return Err(AppError::Busy(
                "A sessão protegida foi recuperada; releia suas gerações antes de usar.".to_owned(),
            ));
        }
        self.inner.load_generation(id, generation)
    }
    fn delete(&self, id: &str) -> Result<(), AppError> {
        if self.pending(id)? {
            return Err(AppError::CredentialRecoveryPending(
                "Conclua a recuperação protegida antes de excluir esta conta.".to_owned(),
            ));
        }
        self.inner
            .delete(id)
            .and_then(|()| self.recovery.forget_completed(&Self::session_id(id)))
    }
    fn activate_generation(&self, id: &str, generation: &str) -> Result<(), AppError> {
        self.inner.activate_generation(id, generation)
    }
    fn delete_generation(&self, id: &str, generation: &str) -> Result<(), AppError> {
        self.inner.delete_generation(id, generation)
    }
    fn quarantine_generation(&self, id: &str, generation: &str) -> Result<(), AppError> {
        self.inner.quarantine_generation(id, generation)
    }
    fn recover_generation(&self, id: &str, generation: &str) -> Result<(), AppError> {
        self.inner.recover_generation(id, generation)
    }
    fn cleanup_expired_generations(&self) -> Result<(), AppError> {
        self.inner.cleanup_expired_generations()
    }
}

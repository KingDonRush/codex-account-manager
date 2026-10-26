use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::adapters::desktop_control::{DesktopLaunchOutcome, DesktopState};
use crate::domain::{AccountSnapshot, OperationEvent};
use crate::error::AppError;

pub trait EventSink: Send + Sync {
    fn show_authorization(&self) {}
    fn emit(&self, event: &OperationEvent);
    fn emit_dashboard_changed(&self);
}

#[derive(Debug, Clone)]
pub struct StoredCredentialGeneration {
    pub account_id: String,
    pub generation_id: String,
    pub fingerprint: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub is_active: bool,
}

pub trait CredentialStore: Send + Sync {
    fn prepare_recovery(&self, _account: &str) -> Result<(), AppError> {
        Ok(())
    }
    fn confirm_usable_access(&self, _access: &AccessSession) -> Result<(), AppError> {
        Ok(())
    }
    fn close(&self) {}
    /// Discover only this manager's main account entries, never another app's coffer.
    fn account_ids(&self) -> Result<Vec<String>, AppError> {
        Ok(Vec::new())
    }
    /// Background reads must not open an unlock prompt without a user action.
    fn check_read_access(&self) -> Result<(), AppError> {
        Ok(())
    }

    fn save(&self, document: &Value) -> Result<String, AppError>;
    fn load(&self, account_id: &str) -> Result<Value, AppError>;
    fn load_optional(&self, account_id: &str) -> Result<Option<Value>, AppError>;
    fn delete(&self, account_id: &str) -> Result<(), AppError>;

    fn save_generation(&self, document: &Value) -> Result<StoredCredentialGeneration, AppError> {
        let account_id = self.save(document)?;
        let serialized = serde_json::to_vec(document)?;
        Ok(StoredCredentialGeneration {
            account_id,
            generation_id: "primary".to_owned(),
            fingerprint: hex::encode(Sha256::digest(serialized)),
            created_at: chrono::Utc::now(),
            is_active: true,
        })
    }

    /// Preserve a snapshot without replacing the selected session. Unsupported
    /// stores must fail before the caller overwrites the original auth file.
    fn archive_generation(
        &self,
        _document: &Value,
    ) -> Result<StoredCredentialGeneration, AppError> {
        Err(AppError::CredentialUnavailable(
            "Este cofre não permite preservar a sessão anterior sem substituir a seleção."
                .to_owned(),
        ))
    }

    fn generation_refs(
        &self,
        account_id: &str,
    ) -> Result<Vec<StoredCredentialGeneration>, AppError> {
        match self.load_optional(account_id)? {
            Some(document) => {
                let serialized = serde_json::to_vec(&document)?;
                Ok(vec![StoredCredentialGeneration {
                    account_id: account_id.to_owned(),
                    generation_id: "primary".to_owned(),
                    fingerprint: hex::encode(Sha256::digest(serialized)),
                    created_at: chrono::Utc::now(),
                    is_active: true,
                }])
            }
            None => Ok(Vec::new()),
        }
    }

    fn load_generation(&self, account_id: &str, _generation_id: &str) -> Result<Value, AppError> {
        self.load(account_id)
    }

    fn activate_generation(&self, _account_id: &str, _generation_id: &str) -> Result<(), AppError> {
        Ok(())
    }

    fn delete_generation(&self, account_id: &str, _generation_id: &str) -> Result<(), AppError> {
        self.delete(account_id)
    }

    fn quarantine_generation(
        &self,
        _account_id: &str,
        _generation_id: &str,
    ) -> Result<(), AppError> {
        Ok(())
    }

    fn recover_generation(&self, account_id: &str, generation_id: &str) -> Result<(), AppError> {
        self.activate_generation(account_id, generation_id)
    }

    fn cleanup_expired_generations(&self) -> Result<(), AppError> {
        Ok(())
    }
}

/// Application chains have a distinct namespace and owner from Desktop credentials.
pub trait ApplicationSessionStore: Send + Sync {
    fn close(&self) {}
    fn save(&self, id: &str, document: &Value) -> Result<(), AppError>;
    fn load(&self, id: &str, expected_account: &str) -> Result<Value, AppError>;
    fn delete(&self, id: &str) -> Result<(), AppError>;
    fn pending_sessions(&self) -> Result<Vec<String>, AppError> {
        Ok(Vec::new())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DestinationPreparation {
    Verified,
    NeedsExclusiveRenewal,
}

pub trait SessionVerifier: Send + Sync {
    fn prepare_destination(
        &self,
        expected: &str,
        document: &Value,
        persist: &dyn Fn(&Value) -> Result<(), AppError>,
        cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<DestinationPreparation, AppError> {
        let _ = persist;
        if cancel.load(std::sync::atomic::Ordering::Acquire) {
            return Err(AppError::Cancelled);
        }
        self.preflight(expected, document)
            .map(|_| DestinationPreparation::Verified)
    }
    /// Validate the destination without materializing credentials in the Desktop home.
    fn preflight(&self, expected_account_id: &str, document: &Value) -> Result<(), AppError> {
        crate::adapters::auth_store::validate_auth_document(document, Some(expected_account_id))?;
        Ok(())
    }

    fn verify(&self, expected_account_id: &str) -> Result<AccountSnapshot, AppError>;

    fn verify_with_trace(
        &self,
        expected_account_id: &str,
        _trace: &mut dyn FnMut(&'static str, std::time::Duration),
    ) -> Result<AccountSnapshot, AppError> {
        self.verify(expected_account_id)
    }

    fn verify_current_with_trace(
        &self,
        expected_account_id: &str,
        trace: &mut dyn FnMut(&'static str, std::time::Duration),
    ) -> Result<AccountSnapshot, AppError> {
        self.verify_with_trace(expected_account_id, trace)
    }
}

/// Samples remote usage without owning or refreshing the account's session.
/// Access-only capability. Intentionally neither Debug nor serializable.
#[derive(Clone)]
pub struct AccessSession {
    pub account_id: String,
    pub access_token: String,
}

pub trait UsageReader: Send + Sync {
    fn close(&self) {}

    fn read(
        &self,
        session: &AccessSession,
        cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<AccountSnapshot, AppError>;
}

pub trait DesktopLifecycle: Send + Sync {
    fn open_or_focus(&self) -> Result<(), AppError> {
        Err(AppError::DesktopLaunch(
            "Abertura nao disponivel neste adaptador.".to_owned(),
        ))
    }

    fn select_executable(&self, _owner: isize) -> Result<bool, AppError> {
        Err(AppError::DesktopDiscovery(
            "Selecao nao disponivel neste adaptador.".to_owned(),
        ))
    }

    fn confirm_close(&self, _owner: isize) -> Result<bool, AppError> {
        Err(AppError::DesktopClosePending(
            "Confirmação nativa indisponível neste adaptador.".to_owned(),
        ))
    }

    fn stop(&self) -> Result<DesktopState, AppError>;

    fn stop_with_trace(
        &self,
        _trace: &mut dyn FnMut(&'static str, std::time::Duration),
    ) -> Result<DesktopState, AppError> {
        self.stop()
    }

    fn restart(&self, state: &DesktopState) -> Result<DesktopLaunchOutcome, AppError>;
}

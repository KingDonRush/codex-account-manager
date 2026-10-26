use super::RecoveryKeyStore;
use crate::{
    adapters::native_keyring::{self, Entry},
    error::AppError,
};

pub struct NativeRecoveryKeys;
impl RecoveryKeyStore for NativeRecoveryKeys {
    fn load_or_create(&self, id: &str) -> Result<[u8; 32], AppError> {
        native_keyring::check_unlocked()?;
        let entry = entry(id)?;
        match entry.get_password() {
            Ok(value) => hex::decode(value)
                .ok()
                .and_then(|bytes| bytes.try_into().ok())
                .ok_or_else(unavailable),
            Err(keyring_core::Error::NoEntry) => {
                let mut key = [0_u8; 32];
                getrandom::fill(&mut key).map_err(|_| unavailable())?;
                entry
                    .set_password(&hex::encode(key))
                    .map_err(|_| unavailable())?;
                Ok(key)
            }
            Err(_) => Err(unavailable()),
        }
    }
    fn delete(&self, id: &str) -> Result<(), AppError> {
        match entry(id)?.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(_) => Err(unavailable()),
        }
    }
}
fn entry(id: &str) -> Result<Entry, AppError> {
    Entry::new("com.codex.account-manager.recovery", id).map_err(|_| unavailable())
}
fn unavailable() -> AppError {
    AppError::CredentialUnavailable(
        "A chave de recuperação está no cofre; desbloqueie-o antes de iniciar trabalho.".to_owned(),
    )
}

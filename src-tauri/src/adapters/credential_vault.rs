use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use serde_json::Value;

use crate::adapters::database::Database;
use crate::adapters::keyring_store::KeyringStore;
use crate::domain::CredentialGenerationState;
use crate::error::AppError;
use crate::ports::{CredentialStore, StoredCredentialGeneration};

const MAX_ELIGIBLE_GENERATIONS: usize = 3;

pub struct CredentialVault {
    database: Database,
    keyring: Arc<dyn CredentialStore>,
}

impl CredentialVault {
    pub fn new(database: Database) -> Self {
        Self {
            database,
            keyring: Arc::new(KeyringStore),
        }
    }

    fn synchronize(&self, account_id: &str) -> Result<Vec<StoredCredentialGeneration>, AppError> {
        let keyring_generations = self.keyring.generation_refs(account_id)?;
        let available = keyring_generations
            .iter()
            .map(|generation| generation.generation_id.as_str())
            .collect::<std::collections::HashSet<_>>();
        for generation in &keyring_generations {
            self.database
                .upsert_credential_generation(generation, generation.is_active)?;
        }
        for metadata in self.database.credential_generations(account_id)? {
            if !available.contains(metadata.generation_id.as_str()) {
                self.database
                    .remove_credential_generation_metadata(account_id, &metadata.generation_id)?;
            }
        }
        self.enforce_eligible_limit(account_id)?;
        Ok(keyring_generations)
    }

    fn enforce_eligible_limit(&self, account_id: &str) -> Result<(), AppError> {
        let eligible = self
            .database
            .credential_generations(account_id)?
            .into_iter()
            .filter(|generation| matches!(generation.state, CredentialGenerationState::Eligible))
            .collect::<Vec<_>>();
        for generation in eligible.into_iter().skip(MAX_ELIGIBLE_GENERATIONS) {
            self.database.quarantine_credential_generation(
                account_id,
                &generation.generation_id,
                Utc::now(),
            )?;
        }
        Ok(())
    }
}

impl CredentialStore for CredentialVault {
    fn confirm_usable_access(&self, access: &crate::ports::AccessSession) -> Result<(), AppError> {
        let current = self.keyring.load(&access.account_id)?;
        if current
            .pointer("/tokens/access_token")
            .and_then(Value::as_str)
            != Some(&access.access_token)
        {
            return Err(AppError::Busy(
                "A sessão mudou durante a consulta.".to_owned(),
            ));
        }
        let refs = self.synchronize(&access.account_id)?;
        if let Some(active) = refs.iter().find(|reference| reference.is_active) {
            self.database
                .activate_credential_generation(&access.account_id, &active.generation_id)?;
        }
        self.database
            .set_credential_state(&access.account_id, crate::domain::CredentialState::Ready)
    }
    fn account_ids(&self) -> Result<Vec<String>, AppError> {
        self.keyring.account_ids()
    }
    fn check_read_access(&self) -> Result<(), AppError> {
        self.keyring.check_read_access()
    }

    fn save(&self, document: &Value) -> Result<String, AppError> {
        Ok(self.save_generation(document)?.account_id)
    }

    fn save_generation(&self, document: &Value) -> Result<StoredCredentialGeneration, AppError> {
        let generation = self.keyring.save_generation(document)?;
        self.database
            .upsert_credential_generation(&generation, true)?;
        self.database
            .activate_credential_generation(&generation.account_id, &generation.generation_id)?;
        self.enforce_eligible_limit(&generation.account_id)?;
        Ok(generation)
    }

    fn archive_generation(&self, document: &Value) -> Result<StoredCredentialGeneration, AppError> {
        let generation = self.keyring.archive_generation(document)?;
        self.synchronize(&generation.account_id)?;
        Ok(generation)
    }

    fn load(&self, account_id: &str) -> Result<Value, AppError> {
        self.keyring.load(account_id)
    }

    fn load_optional(&self, account_id: &str) -> Result<Option<Value>, AppError> {
        self.keyring.load_optional(account_id)
    }

    fn delete(&self, account_id: &str) -> Result<(), AppError> {
        self.keyring.delete(account_id)?;
        self.database.remove_account_credential_metadata(account_id)
    }

    fn generation_refs(
        &self,
        account_id: &str,
    ) -> Result<Vec<StoredCredentialGeneration>, AppError> {
        let keyring = self
            .synchronize(account_id)?
            .into_iter()
            .map(|generation| (generation.generation_id.clone(), generation))
            .collect::<HashMap<_, _>>();
        Ok(self
            .database
            .credential_generations(account_id)?
            .into_iter()
            .filter_map(|metadata| keyring.get(&metadata.generation_id).cloned())
            .collect())
    }

    fn load_generation(&self, account_id: &str, generation_id: &str) -> Result<Value, AppError> {
        self.keyring.load_generation(account_id, generation_id)
    }

    fn activate_generation(&self, account_id: &str, generation_id: &str) -> Result<(), AppError> {
        self.keyring
            .activate_generation(account_id, generation_id)?;
        self.database
            .activate_credential_generation(account_id, generation_id)
    }

    fn delete_generation(&self, account_id: &str, generation_id: &str) -> Result<(), AppError> {
        self.keyring.delete_generation(account_id, generation_id)?;
        self.database
            .remove_credential_generation_metadata(account_id, generation_id)
    }

    fn quarantine_generation(&self, account_id: &str, generation_id: &str) -> Result<(), AppError> {
        self.database
            .quarantine_credential_generation(account_id, generation_id, Utc::now())
    }

    fn recover_generation(&self, account_id: &str, generation_id: &str) -> Result<(), AppError> {
        self.activate_generation(account_id, generation_id)
    }

    fn cleanup_expired_generations(&self) -> Result<(), AppError> {
        for (account_id, generation_id) in
            self.database.expired_credential_generations(Utc::now())?
        {
            // A SQL quarantine may be stale. Preserve the coffer's current/last
            // session even if an older build misclassified account/read.
            let refs = self.keyring.generation_refs(&account_id)?;
            if refs
                .iter()
                .any(|reference| reference.is_active && reference.generation_id == generation_id)
            {
                continue;
            }
            self.keyring
                .delete_generation(&account_id, &generation_id)?;
            self.database
                .remove_credential_generation_metadata(&account_id, &generation_id)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;

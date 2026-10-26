use super::ManagerCore;
use crate::{adapters::auth_store, domain::ManagedProfile, error::AppError, ports::AccessSession};
#[cfg(test)]
mod tests;

pub(super) struct CachedAccess {
    signature: String,
    access: AccessSession,
}
impl ManagerCore {
    pub(super) fn usage_access_session(
        &self,
        profile: &ManagedProfile,
    ) -> Result<AccessSession, AppError> {
        let remote = profile
            .account_id
            .as_deref()
            .ok_or_else(|| AppError::Validation("Conta ausente.".to_owned()))?;
        if profile.is_active_account {
            return auth_store::access_session(
                &auth_store::read_auth(&self.codex_home.join("auth.json"))?,
                Some(remote),
            );
        }
        if let Err(error) = self.credentials.check_read_access() {
            self.usage_access.lock().clear();
            return Err(error);
        }
        let signature = self.usage_generation_signature(profile)?;
        if let Some(cached) = self
            .usage_access
            .lock()
            .get(remote)
            .filter(|cached| cached.signature == signature)
        {
            return Ok(cached.access.clone());
        }
        // The full refresh document is dropped here, not retained by the monitor.
        let access = auth_store::access_session(&self.credentials.load(remote)?, Some(remote))?;
        self.usage_access.lock().insert(
            remote.to_owned(),
            CachedAccess {
                signature,
                access: access.clone(),
            },
        );
        Ok(access)
    }
    pub(super) fn usage_generation_signature(
        &self,
        profile: &ManagedProfile,
    ) -> Result<String, AppError> {
        let remote = profile
            .account_id
            .as_deref()
            .ok_or_else(|| AppError::Validation("Conta ausente.".to_owned()))?;
        let signature = self.database.active_credential_signature(remote)?;
        Ok(format!(
            "{}:{}",
            signature.as_deref().unwrap_or("legacy"),
            profile
                .last_browser_authenticated_at
                .map_or_else(String::new, |at| at.to_rfc3339())
        ))
    }
}

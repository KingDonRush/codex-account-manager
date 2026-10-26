//! Credential ownership is independent of application authorization. Consumers never own refresh.
use super::ManagerCore;
use crate::{
    adapters::{auth_store, file_lock::FileLock},
    error::AppError,
};
use serde_json::Value;
use std::{
    sync::{atomic::AtomicBool, Arc},
    time::Duration,
};
#[cfg(test)]
mod tests;

#[derive(Clone, PartialEq, Eq)]
pub(super) enum ApplicationAuth {
    Dedicated(String),
    Account(String),
}

impl ApplicationAuth {
    pub(super) fn lease_key(&self) -> &str {
        match self {
            Self::Dedicated(id) | Self::Account(id) => id,
        }
    }
}

impl ManagerCore {
    pub(super) fn application_auth(&self, local: &str) -> Result<ApplicationAuth, AppError> {
        Ok(match self.database.application_session(local)? {
            Some(session) => ApplicationAuth::Dedicated(session),
            None => ApplicationAuth::Account(format!("account:{local}")),
        })
    }

    pub(super) fn load_application_auth(
        &self,
        source: &ApplicationAuth,
        remote: &str,
        cancel: Arc<AtomicBool>,
        renew: bool,
    ) -> Result<Value, AppError> {
        if let ApplicationAuth::Dedicated(session) = source {
            return self.application_credentials.load(session, remote);
        }
        // Same cross-process boundary as switching and the documented session helper.
        // Never hold it through inference: a child receives access only, not the refresh chain.
        let _lease = FileLock::acquire_cancellable(
            &self.codex_home.join("auth-switch.lock"),
            &cancel,
            Duration::from_secs(30),
        )?;
        let path = self.codex_home.join("auth.json");
        if path.try_exists()? {
            let current = auth_store::read_auth(&path)?;
            if auth_store::validate_auth_document(&current, None)? == remote {
                // Desktop owns this live chain. Read its newest access, never renew a copy.
                return Ok(current);
            }
        }
        let mut document = self.credentials.load(remote)?;
        auth_store::validate_auth_document(&document, Some(remote))?;
        let expiry = auth_store::token_claims(
            document
                .pointer("/tokens/access_token")
                .and_then(Value::as_str),
        )
        .and_then(|claims| claims.get("exp").and_then(Value::as_i64));
        if renew || expiry.is_some_and(|expiry| expiry <= chrono::Utc::now().timestamp() + 60) {
            // Existing verifier persists any rotation before returning, including failed validation.
            let prepared = self.session_switcher.preflight_target(remote, cancel)?;
            if prepared.needs_exclusive_renewal() {
                return Err(AppError::SessionRenewalRequired(
                    "A sessão ativa precisa concluir sua renovação antes desta execução."
                        .to_owned(),
                ));
            }
            document = self.credentials.load(remote)?;
            auth_store::validate_auth_document(&document, Some(remote))?;
        }
        Ok(document)
    }
}

use super::ManagerCore;
use crate::{adapters::auth_store, error::AppError};
use serde_json::Value;
#[cfg(test)]
mod tests;
impl ManagerCore {
    pub fn recover_saved_accounts(&self) -> Result<usize, AppError> {
        let _lease = self.tasks.enter()?;
        let _guard = crate::application::operation_gate::acquire(
            &self.switch_lock,
            &self.maintenance.cancel,
            std::time::Duration::from_secs(30),
        )?;
        let _file = crate::adapters::file_lock::FileLock::acquire_cancellable(
            &self.codex_home.join("auth-switch.lock"),
            &self.maintenance.cancel,
            std::time::Duration::from_secs(30),
        )?;
        let recovered = self.collect_saved_accounts()?;
        self.emit_dashboard_changed();
        Ok(recovered)
    }
    pub(super) fn collect_saved_accounts(&self) -> Result<usize, AppError> {
        self.credentials.check_read_access()?;
        let mut restored = 0;
        for remote in self.credentials.account_ids()? {
            if self
                .maintenance
                .cancel
                .load(std::sync::atomic::Ordering::Acquire)
            {
                return Err(AppError::Cancelled);
            }
            if self.database.profile_for_account(&remote)?.is_some()
                || self.database.account_was_removed(&remote)?
            {
                continue;
            }
            let result = self.credentials.load(&remote).and_then(|document| {
                auth_store::validate_auth_document(&document, Some(&remote)).map(|_| document)
            });
            let (email, plan) = result
                .as_ref()
                .ok()
                .map(recovered_metadata)
                .unwrap_or((None, None));
            restored += usize::from(self.database.collect_coffer_account(
                crate::adapters::database::CollectedAccount {
                    remote: &remote,
                    email: email.as_deref(),
                    plan: plan.as_deref(),
                    readable: result.is_ok(),
                },
            )?);
            if let Err(error) = result {
                let _ = crate::adapters::logging::record(
                    &self.database,
                    None,
                    None,
                    "account_collection_needs_verification",
                    serde_json::json!({"account":super::mask_id(&remote),"errorCode":error.code()}),
                );
            }
        }
        Ok(restored)
    }
}
fn recovered_metadata(document: &Value) -> (Option<String>, Option<String>) {
    // Metadata hints from local JWT claims are not authentication/entitlement.
    let Some(claims) =
        auth_store::token_claims(document.pointer("/tokens/id_token").and_then(Value::as_str))
    else {
        return (None, None);
    };
    (
        claims
            .get("email")
            .and_then(Value::as_str)
            .map(str::to_owned),
        claims
            .pointer("/https:~1~1api.openai.com~1auth/chatgpt_plan_type")
            .and_then(Value::as_str)
            .map(str::to_owned),
    )
}

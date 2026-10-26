use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use super::login_flow::{LoginTask, VerifiedLogin};
use super::{ensure_login_active, ManagerCore, OperationControl};
#[cfg(test)]
use crate::adapters::auth_store;
use crate::adapters::{browser_runtime::BrowserHandle, file_lock::FileLock};
use crate::domain::{
    AccountMetadata, LoginOperation, ManagedProfile, OperationKind, OperationState,
    ReplacementPreview,
};
use crate::error::AppError;
use chrono::Utc;

impl ManagerCore {
    pub fn start_replacement(&self, id: &str) -> Result<LoginOperation, AppError> {
        let profile = self.database.profile(id)?;
        if profile.account_id.is_none() {
            return Err(AppError::Validation(
                "Conecte uma conta antes de substituir a associação.".to_owned(),
            ));
        }
        self.ensure_profile_inactive(&profile)?;
        self.start_login_with_kind(id, OperationKind::Replace, None)
    }

    pub fn replacement_preview(&self, id: &str) -> Result<Option<ReplacementPreview>, AppError> {
        Ok(self
            .operations
            .lock()
            .controls
            .get(id)
            .and_then(|control| control.replacement.lock().clone()))
    }

    pub fn confirm_replacement(&self, id: &str) -> Result<(), AppError> {
        let operation = self.database.operation(id)?;
        if operation.kind != OperationKind::Replace
            || !matches!(operation.state, OperationState::AwaitingConfirmation)
        {
            return Err(AppError::Validation(
                "Não há uma substituição validada aguardando confirmação.".to_owned(),
            ));
        }
        let control = self
            .operations
            .lock()
            .controls
            .get(id)
            .cloned()
            .ok_or(AppError::OperationFinished)?;
        ensure_login_active(&control, None)?;
        control.confirmed.store(true, Ordering::Release);
        Ok(())
    }

    pub(super) fn validate_replacement_identity(
        &self,
        profile: &ManagedProfile,
        account_id: &str,
    ) -> Result<(), AppError> {
        if profile.account_id.as_deref() == Some(account_id) {
            return Err(AppError::AccountMismatch("Você entrou na mesma conta. Para substituir, entre na outra conta; para renovar esta sessão, use Reautenticar.".to_owned()));
        }
        if let Some(existing) = self.database.profile_for_account(account_id)? {
            return Err(AppError::AccountMismatch(format!(
                "Esta conta já está vinculada a ‘{}’. Nenhuma associação foi alterada.",
                existing.name
            )));
        }
        Ok(())
    }

    pub(super) fn await_replacement_confirmation(
        &self,
        task: &LoginTask,
        profile: &ManagedProfile,
        verified: &VerifiedLogin,
        browser: &BrowserHandle,
    ) -> Result<(), AppError> {
        if verified.snapshot.email.is_none() {
            return Err(AppError::AppServer("O servidor não confirmou o e-mail da nova conta; a associação anterior foi preservada.".to_owned()));
        }
        *task.control.replacement.lock() = Some(ReplacementPreview {
            previous_email: profile.email.clone(),
            next_email: verified.snapshot.email.clone(),
            next_plan: verified.snapshot.plan_type.clone(),
        });
        self.transition(
            &task.operation_id,
            &task.profile_id,
            OperationState::AwaitingConfirmation,
            "Confirme a substituição da conta",
        )?;
        wait_for_confirmation(&task.control, Some(browser), Duration::from_secs(600))
    }

    /// Called under the manager commit lock; no credential leaves the native worker.
    pub(super) fn commit_replacement(
        &self,
        task: &LoginTask,
        verified: &VerifiedLogin,
        provider: Option<&str>,
    ) -> Result<(), AppError> {
        let _file = FileLock::acquire(&self.codex_home.join("auth-switch.lock"))?;
        let profile = self.database.profile(&task.profile_id)?;
        self.ensure_profile_inactive(&profile)?;
        self.validate_replacement_identity(&profile, &verified.account_id)?;
        self.commit_candidate(&profile, verified, provider)
    }

    fn commit_candidate(
        &self,
        profile: &ManagedProfile,
        verified: &VerifiedLogin,
        provider: Option<&str>,
    ) -> Result<(), AppError> {
        profile.account_id.as_deref().ok_or_else(|| {
            AppError::Validation("A associação anterior deixou de existir.".to_owned())
        })?;
        let candidate_before = self.credentials.load_optional(&verified.account_id)?;
        let result = self.save_replacement(profile, verified, provider);
        if let Err(error) = result {
            let new_restore =
                self.restore_credential(&verified.account_id, candidate_before.as_ref());
            new_restore?;
            return Err(error);
        }
        // The old credential is only housekeeping after the association commit;
        // an unavailable coffer must not keep the completed login modal spinning.
        self.maintenance
            .cleanup_requested
            .store(true, Ordering::Release);
        Ok(())
    }

    fn save_replacement(
        &self,
        profile: &ManagedProfile,
        verified: &VerifiedLogin,
        provider: Option<&str>,
    ) -> Result<(), AppError> {
        self.credentials.save(&verified.document)?;
        self.database
            .persist_connected_account(
                &profile.id,
                AccountMetadata {
                    account_id: &verified.account_id,
                    email: verified.snapshot.email.as_deref(),
                    plan_type: verified.snapshot.plan_type.as_deref(),
                    credential_ref: Some("native-keyring"),
                    rate_limits: verified.snapshot.rate_limits.as_ref(),
                    observed_at: Utc::now(),
                },
                provider,
            )
            .map(|_| ())
    }
}

fn wait_for_confirmation(
    control: &OperationControl,
    browser: Option<&BrowserHandle>,
    timeout: Duration,
) -> Result<(), AppError> {
    let deadline = Instant::now() + timeout;
    loop {
        ensure_login_active(control, browser)?;
        if control.confirmed.load(Ordering::Acquire) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(AppError::TimedOut);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests;

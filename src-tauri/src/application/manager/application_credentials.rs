use super::ManagerCore;
use crate::error::AppError;
use serde_json::Value;

impl ManagerCore {
    pub(super) fn save_application_credential(
        &self,
        session: &str,
        remote: &str,
        document: &Value,
    ) -> Result<(), AppError> {
        self.usage_access.lock().remove(remote);
        self.application_credentials.save(session, document)
    }
    pub fn recover_application_credential(&self, profile_id: &str) -> Result<(), AppError> {
        self.with_profile_mutation(profile_id, || {
            let remote = self
                .database
                .profile(profile_id)?
                .account_id
                .ok_or_else(|| AppError::Validation("Conta não associada.".to_owned()))?;
            let (local, _, _, _) = self.database.account_integration_options(&remote)?;
            let session = self.database.application_session(&local)?.ok_or_else(|| {
                AppError::Validation("Sem sessão própria para aplicações.".to_owned())
            })?;
            if self.executions.session_busy(&session) {
                return Err(AppError::Busy(
                    "Aguarde o trabalho terminar antes de recuperar a sessão.".to_owned(),
                ));
            }
            self.usage_access.lock().remove(&remote);
            self.application_credentials.load(&session, &remote)?;
            self.emit_dashboard_changed();
            Ok(())
        })
    }
    pub(super) fn recover_application_credentials(&self) {
        let Ok(pending) = self.application_credentials.pending_sessions() else {
            return;
        };
        for session in pending {
            if self
                .maintenance
                .cancel
                .load(std::sync::atomic::Ordering::Acquire)
            {
                break;
            }
            let Ok(Some((local, remote))) = self.database.application_session_owner(&session)
            else {
                continue;
            };
            if self.executions.session_busy(&session) {
                continue;
            }
            let Some(_admission) = self.account_admission.try_lock() else {
                return;
            };
            if self.executions.session_busy(&session) {
                continue;
            }
            let Some(profile) = self.database.profile_for_account(&remote).ok().flatten() else {
                continue;
            };
            let operations = self.operations.lock();
            if operations.starting.contains_key(&profile.id)
                || operations
                    .controls
                    .values()
                    .any(|control| control.profile_id == profile.id)
            {
                continue;
            }
            drop(operations);
            if self.application_credentials.load(&session, &remote).is_ok() {
                self.usage_access.lock().remove(&remote);
                let _ = crate::adapters::logging::record(
                    &self.database,
                    None,
                    Some(&profile.id),
                    "application_credential_recovered",
                    serde_json::json!({"account":local}),
                );
                self.emit_dashboard_changed();
            }
        }
    }
}

use super::ManagerCore;
use crate::adapters::{auth_store, file_lock::FileLock};
use crate::domain::{ManagedProfile, OperationKind};
use crate::error::AppError;

impl ManagerCore {
    pub fn delete_profile(&self, id: &str, permanently: bool) -> Result<(), AppError> {
        self.with_profile_mutation(id, || {
            if permanently {
                self.ensure_profile_inactive(&self.database.profile(id)?)?;
            }
            self.delete_profile_data(id, permanently)?;
            self.emit_dashboard_changed();
            Ok(())
        })
    }

    /// Remove the local account and its credential, preserving the browser data.
    pub fn unlink_profile(&self, id: &str) -> Result<(), AppError> {
        self.with_profile_mutation(id, || {
            self.unlink_profile_data(id)?;
            self.emit_dashboard_changed();
            Ok(())
        })
    }

    pub(super) fn unlink_profile_data(&self, id: &str) -> Result<(), AppError> {
        let profile = self.database.profile(id)?;
        self.ensure_profile_inactive(&profile)?;
        let Some(account_id) = profile.account_id.as_deref() else {
            return Ok(());
        };
        let previous = self.credentials.load_optional(account_id)?;
        let result = self
            .credentials
            .delete(account_id)
            .and_then(|()| self.database.detach_profile(id));
        if let Err(error) = result {
            self.restore_credential(account_id, previous.as_ref())?;
            return Err(error);
        }
        Ok(())
    }

    pub fn logout_profile(&self, id: &str) -> Result<(), AppError> {
        self.with_profile_mutation(id, || {
            let profile = self.database.profile(id)?;
            let account_id = profile.account_id.as_deref().ok_or_else(|| {
                AppError::Validation("Este perfil não tem uma conta associada.".to_owned())
            })?;
            self.shared_runtime
                .prepare_global_change(&self.maintenance.cancel)?;
            self.session_switcher
                .recover(|id| self.database.switch_was_committed(id))?;
            if let Some(warning) = self.session_switcher.logout(account_id)? {
                self.database.update_profile_error(id, &warning)?;
            }
            *self.active_account_id.write() = None;
            self.emit_dashboard_changed();
            Ok(())
        })
    }

    pub(super) fn with_profile_mutation<T>(
        &self,
        id: &str,
        action: impl FnOnce() -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let _lease = self.tasks.enter()?;
        if let Some(remote) = self.database.profile(id)?.account_id {
            self.usage_access.lock().remove(&remote);
        }
        self.reserve_operation(id, OperationKind::Login)?;
        let result = (|| {
            let _guard = crate::application::operation_gate::acquire(
                &self.switch_lock,
                &self.maintenance.cancel,
                std::time::Duration::from_secs(30),
            )?;
            let _file = FileLock::acquire_cancellable(
                &self.codex_home.join("auth-switch.lock"),
                &self.maintenance.cancel,
                std::time::Duration::from_secs(30),
            )?;
            let _admission = crate::application::operation_gate::acquire(
                &self.account_admission,
                &self.maintenance.cancel,
                std::time::Duration::from_secs(30),
            )?;
            action()
        })();
        self.release_operation_reservation(id);
        result
    }

    pub(super) fn ensure_profile_inactive(&self, profile: &ManagedProfile) -> Result<(), AppError> {
        let Some(expected) = profile.account_id.as_deref() else {
            return Ok(());
        };
        let (local, _, _, _) = self.database.account_integration_options(expected)?;
        if self.executions.account_busy(&local)
            || self
                .list_executions()
                .iter()
                .any(|job| job.account_id.as_deref() == Some(&local) && !job.state.terminal())
            || self
                .list_application_servers()
                .iter()
                .any(|server| server.account_id == local && !server.state.terminal())
        {
            return Err(AppError::Busy("Há trabalhos ou sessões de aplicações nesta conta; encerre-os antes de alterar a associação.".to_owned()));
        }
        let path = self.codex_home.join("auth.json");
        let active = match auth_store::read_auth(&path) {
            Ok(document) => Some(auth_store::validate_auth_document(&document, None)?),
            Err(AppError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        ensure_not_active(expected, active.as_deref())
    }
}

fn ensure_not_active(expected: &str, active: Option<&str>) -> Result<(), AppError> {
    if active == Some(expected) {
        return Err(AppError::Busy("Esta conta está em uso no Codex. Use outra conta ou saia dela antes de alterar a associação ou excluir.".to_owned()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn active_identity_cannot_be_removed() {
        assert!(super::ensure_not_active("a", Some("a")).is_err());
        assert!(super::ensure_not_active("a", Some("b")).is_ok());
        assert!(super::ensure_not_active("a", None).is_ok());
    }
}

use super::ManagerCore;
use crate::adapters::{auth_store, file_lock::FileLock, logging};
use crate::error::AppError;
use serde_json::json;

impl ManagerCore {
    pub(super) fn cleanup_application_sessions(&self) {
        let Ok(pending) = self.database.pending_application_session_cleanup() else {
            return;
        };
        for (session, account) in pending {
            if self.executions.session_busy(&session)
                || self
                    .application_servers
                    .list()
                    .iter()
                    .any(|server| server.account_id == account && !server.state.terminal())
            {
                continue;
            }
            if self
                .database
                .application_session_referenced(&session)
                .unwrap_or(true)
            {
                continue;
            }
            if self.application_credentials.delete(&session).is_ok() {
                let _ = self.database.finish_application_session_cleanup(&session);
            }
        }
    }
    pub(super) fn cleanup_pending_accounts(&self) {
        let Some(_guard) = self.switch_lock.try_lock() else {
            return;
        };
        let Ok(_file) = FileLock::acquire(&self.codex_home.join("auth-switch.lock")) else {
            return;
        };
        if self.credentials.check_read_access().is_err() {
            return;
        }
        let Ok(pending) = self.database.pending_account_cleanup() else {
            return;
        };
        for (account_id, profile_id) in pending.into_iter().take(1) {
            self.cleanup_replaced_account(&account_id, &profile_id);
        }
    }

    /// Deletion follows the SQLite commit, so interrupted cleanup cannot erase
    /// the old session while the old profile association is still authoritative.
    pub(super) fn cleanup_replaced_account(&self, id: &str, profile_id: &str) {
        let result = self.remove_unreferenced_credential(id);
        if result.is_ok() {
            let _ = self.database.finish_account_cleanup(id);
        }
        let _ = logging::record(
            &self.database,
            None,
            Some(profile_id),
            if result.is_ok() {
                "replaced_account_cleaned"
            } else {
                "replaced_account_cleanup_pending"
            },
            json!({ "errorCode": result.as_ref().err().map(AppError::code) }),
        );
    }

    fn remove_unreferenced_credential(&self, id: &str) -> Result<(), AppError> {
        if self.database.profile_count_for_account(id)? > 0 {
            return Ok(());
        }
        let path = self.codex_home.join("auth.json");
        let active = match auth_store::read_auth(&path) {
            Ok(document) => Some(auth_store::validate_auth_document(&document, None)?),
            Err(AppError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        if active.as_deref() == Some(id) {
            return Err(AppError::Busy(
                "A sessão antiga está em uso por outra interface.".to_owned(),
            ));
        }
        self.credentials.delete(id)?;
        self.database.delete_account_metadata(id)
    }
}

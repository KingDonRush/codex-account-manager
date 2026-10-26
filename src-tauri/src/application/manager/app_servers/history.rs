use super::*;
use crate::adapters::application_history as files;
#[cfg(test)]
mod tests;

impl ManagerCore {
    pub fn clear_application_history(&self, id: &str) -> Result<(), AppError> {
        let _lease = self.tasks.enter()?;
        let _admission = crate::application::operation_gate::acquire(
            &self.account_admission,
            &self.maintenance.cancel,
            std::time::Duration::from_secs(30),
        )?;
        let mut status = self.application_servers.status(id)?;
        ensure_terminal(&status)?;
        let (_, cwd) = self.database.saved_application_server(id)?;
        files::clear(&self.app_data_dir, id)?;
        status.persist_history = false;
        status.error_code = Some("history_cleared".to_owned());
        self.update_application_server(status, &cwd)
    }
    pub fn export_application_history(&self, id: &str) -> Result<String, AppError> {
        let _lease = self.tasks.enter()?;
        let _admission = crate::application::operation_gate::acquire(
            &self.account_admission,
            &self.maintenance.cancel,
            std::time::Duration::from_secs(30),
        )?;
        ensure_terminal(&self.application_servers.status(id)?)?;
        let export_id = uuid::Uuid::new_v4().to_string();
        let path = files::export(&self.app_data_dir, id, &export_id)?;
        if let Err(error) = self.database.register_history_export(&export_id) {
            let _ = std::fs::remove_file(path);
            return Err(error);
        }
        open::that_detached(self.app_data_dir.join("history-exports"))?;
        Ok(format!("{export_id}.zip"))
    }
    pub(in crate::application::manager) fn cleanup_application_history(&self) {
        let Some(_admission) = self.account_admission.try_lock() else {
            return;
        };
        for session in self.application_servers.list() {
            if !session.state.terminal()
                || Utc::now() - session.last_activity < chrono::Duration::days(7)
            {
                continue;
            }
            let _ = if session.persist_history {
                files::remove_events(&self.app_data_dir, &session.id)
            } else {
                files::clear(&self.app_data_dir, &session.id)
            };
        }
        for id in self.database.expired_history_exports().unwrap_or_default() {
            let Ok(path) = files::export_path(&self.app_data_dir, &id) else {
                continue;
            };
            let removed = std::fs::remove_file(path);
            if removed.is_ok()
                || removed.is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
            {
                let _ = self.database.remove_history_export_metadata(&id);
            }
        }
    }
}
fn ensure_terminal(status: &ApplicationServerStatus) -> Result<(), AppError> {
    if !status.state.terminal() {
        return Err(AppError::Busy(
            "Encerre a sessão antes de exportar ou limpar seu histórico.".to_owned(),
        ));
    }
    Ok(())
}

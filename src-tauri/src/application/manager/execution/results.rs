use super::ManagerCore;
use crate::domain::execution::ExecutionStatus;
use crate::domain::integrations::{AuthorizationGrant, Permission};
use crate::error::AppError;
use chrono::Utc;
impl ManagerCore {
    pub(super) fn execution_directory(&self, id: &str) -> Result<std::path::PathBuf, AppError> {
        uuid::Uuid::parse_str(id)
            .map_err(|_| AppError::Validation("Execução inválida.".to_owned()))?;
        let directory = self.app_data_dir.join("executions").join(id);
        std::fs::create_dir_all(&directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(directory)
    }
    pub fn execution_status_for(
        &self,
        grant: &AuthorizationGrant,
        id: &str,
    ) -> Result<ExecutionStatus, AppError> {
        let status = self.executions.status(id)?;
        if !self.ui_visible.load(std::sync::atomic::Ordering::Acquire)
            && !status
                .group_id
                .as_ref()
                .is_some_and(|id| status.background && self.background_group_authorized(grant, id))
        {
            return Err(AppError::ConsentRequired(
                "Este trabalho exige a interface aberta.".to_owned(),
            ));
        }
        if status.application_id != grant.application_id
            || !grant.permits(
                Permission::Execute,
                status.account_id.as_deref(),
                Utc::now(),
            )
        {
            return Err(AppError::AccessDenied(
                "Execução não autorizada.".to_owned(),
            ));
        }
        Ok(status)
    }
    pub fn execution_events(
        &self,
        grant: &AuthorizationGrant,
        id: &str,
        offset: u64,
    ) -> Result<serde_json::Value, AppError> {
        use std::io::{Read, Seek, SeekFrom};
        let status = self.execution_status_for(grant, id)?;
        if status
            .finished_at
            .is_some_and(|at| Utc::now() - at > chrono::Duration::days(7))
        {
            return Err(AppError::OperationNotFound("Resultado expirou.".to_owned()));
        }
        let path = self
            .app_data_dir
            .join("executions")
            .join(id)
            .join("events.jsonl");
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = match options.open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(serde_json::json!({"chunk":"","nextOffset":offset,"state":status.state}))
            }
            Err(error) => return Err(error.into()),
        };
        if offset > file.metadata()?.len() {
            return Err(AppError::Validation("Offset fora da saída.".to_owned()));
        }
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = vec![0_u8; 32 * 1024];
        let read = file.read(&mut bytes)?;
        bytes.truncate(read);
        // UTF-8 boundaries are preserved; an incomplete code point is read next time.
        let valid = std::str::from_utf8(&bytes).map_or_else(|error| error.valid_up_to(), str::len);
        bytes.truncate(valid);
        let chunk = String::from_utf8(bytes)
            .map_err(|_| AppError::Internal("Saída inválida.".to_owned()))?;
        Ok(serde_json::json!({"chunk":chunk,"nextOffset":offset+valid as u64,"state":status.state}))
    }
    pub fn consumer_disconnected(&self, grant_id: &str) {
        self.application_servers
            .close_matching(|session| session.grant_id == grant_id && !session.background);
        self.executions
            .cancel_matching(|job| job.grant_id == grant_id && !job.background);
    }
    pub fn acknowledge_execution(
        &self,
        grant: &AuthorizationGrant,
        id: &str,
    ) -> Result<(), AppError> {
        let status = self.execution_status_for(grant, id)?;
        if !status.state.terminal() {
            return Err(AppError::Busy("O trabalho ainda não terminou.".to_owned()));
        }
        self.remove_execution_output(id)
    }
    pub(in crate::application::manager) fn cleanup_execution_outputs(&self) {
        for job in self.executions.list() {
            if job
                .finished_at
                .is_some_and(|at| Utc::now() - at >= chrono::Duration::days(7))
            {
                let _ = self.remove_execution_output(&job.id);
            }
        }
    }
    fn remove_execution_output(&self, id: &str) -> Result<(), AppError> {
        uuid::Uuid::parse_str(id)
            .map_err(|_| AppError::Validation("Execução inválida.".to_owned()))?;
        let directory = self.app_data_dir.join("executions").join(id);
        let metadata = match std::fs::symlink_metadata(&directory) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(AppError::Validation(
                "Diretório de resultado não é próprio.".to_owned(),
            ));
        }
        let file = directory.join("events.jsonl");
        match std::fs::symlink_metadata(&file) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                std::fs::remove_file(&file)?
            }
            Ok(_) => {
                return Err(AppError::Validation(
                    "Arquivo de resultado não é próprio.".to_owned(),
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        std::fs::remove_dir(directory)?;
        Ok(())
    }
}

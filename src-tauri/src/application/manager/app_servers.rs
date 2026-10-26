use super::ManagerCore;
mod history;
#[cfg(test)]
mod tests;
mod worker;
use crate::domain::application_server::{ApplicationServerStatus, OpenServerRequest, ServerState};
use crate::domain::execution::ExecRequest;
use crate::domain::integrations::{AuthorizationGrant, BindingTarget, Permission};
use crate::error::AppError;
use chrono::Utc;

struct ServerTask {
    status: ApplicationServerStatus,
    cwd: String,
    remote: String,
    session: super::application_auth::ApplicationAuth,
    capacity: u32,
}

impl ManagerCore {
    pub fn recover_application_server_metadata(&self) -> Result<(), AppError> {
        for status in self.database.recover_application_servers()? {
            self.application_servers.restore(status);
        }
        self.cleanup_application_history();
        Ok(())
    }
    pub fn list_application_servers(&self) -> Vec<ApplicationServerStatus> {
        self.application_servers.list()
    }
    pub fn close_application_server(&self, id: &str) -> Result<(), AppError> {
        self.application_servers.close(id)
    }
    pub fn open_application_server(
        &self,
        grant: &AuthorizationGrant,
        request: OpenServerRequest,
    ) -> Result<ApplicationServerStatus, AppError> {
        let selection = ExecRequest {
            binding_id: request.binding_id.clone(),
            binding_snapshot: None,
            account_id: request.account_id.clone(),
            group_id: request.group_id.clone(),
            cwd: request.cwd.clone(),
            prompt: String::new(),
            arguments: Vec::new(),
        };
        crate::adapters::exec_runner::validate_request(&selection)?;
        let candidate = self
            .execution_candidates(grant, &selection)?
            .into_iter()
            .next()
            .ok_or_else(|| AppError::Busy("Nenhuma conta elegível.".to_owned()))?;
        let binding = self.execution_route(grant, &selection)?;
        let status = ApplicationServerStatus {
            events_offset: 0,
            allowed_models: binding.allowed_models.clone(),
            default_model: binding.default_model.clone(),
            id: uuid::Uuid::new_v4().to_string(),
            application_id: grant.application_id.clone(),
            grant_id: grant.id.clone(),
            binding_id: binding.id,
            binding_revision: binding.revision,
            group_id: candidate.group,
            account_id: candidate.account,
            state: ServerState::Preparing,
            background: matches!(&binding.target,BindingTarget::Group(id)if grant.scope.background_group_ids.contains(id)),
            persist_history: request.persist_history,
            created_at: Utc::now(),
            last_activity: Utc::now(),
            error_code: None,
        };
        self.launch_application_server(ServerTask {
            status,
            cwd: request.cwd,
            remote: candidate.remote,
            session: candidate.session,
            capacity: candidate.capacity,
        })
    }
    fn launch_application_server(
        &self,
        mut task: ServerTask,
    ) -> Result<ApplicationServerStatus, AppError> {
        task.status.events_offset = self.application_server_event_offset(&task.status.id)?;
        let status = task.status.clone();
        let cwd = task.cwd.clone();
        let lease = self.tasks.enter()?;
        let _admission = crate::application::operation_gate::acquire(
            &self.account_admission,
            &self.maintenance.cancel,
            std::time::Duration::from_secs(30),
        )?;
        let grant = self.authorization_grant(&status.grant_id)?;
        self.validate_server_authority(&grant, &status)?;
        if self.application_auth(&status.account_id)? != task.session {
            return Err(AppError::Busy(
                "A sessão mudou antes da abertura.".to_owned(),
            ));
        }
        let (input, cancel) = self.application_servers.add(status.clone())?;
        if let Err(error) = self.database.persist_application_server(&status, &cwd) {
            let mut failed = status.clone();
            failed.state = ServerState::Failed;
            failed.error_code = Some(error.code().to_owned());
            self.application_servers.update(failed);
            return Err(error);
        }
        let core = self.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("application-app-server".to_owned())
            .spawn(move || {
                let _lease = lease;
                core.run_application_server(task, input, cancel);
            })
        {
            let mut failed = status.clone();
            failed.state = ServerState::Failed;
            failed.error_code = Some("worker_unavailable".to_owned());
            self.update_application_server(failed, &cwd)?;
            return Err(error.into());
        }
        self.emit_dashboard_changed();
        Ok(status)
    }
    pub fn application_server_for(
        &self,
        grant: &AuthorizationGrant,
        id: &str,
    ) -> Result<ApplicationServerStatus, AppError> {
        let status = self.application_servers.status(id)?;
        self.validate_server_authority(grant, &status)?;
        Ok(status)
    }
    pub fn send_application_server(
        &self,
        grant: &AuthorizationGrant,
        id: &str,
        message: serde_json::Value,
    ) -> Result<(), AppError> {
        self.application_server_for(grant, id)?;
        crate::application::application_servers::validate_harness_message(&message)?;
        self.application_servers.send(id, message)
    }
    pub fn application_server_events(
        &self,
        grant: &AuthorizationGrant,
        id: &str,
        offset: u64,
    ) -> Result<serde_json::Value, AppError> {
        use std::io::{Read, Seek, SeekFrom};
        let status = self.application_server_for(grant, id)?;
        let path = self
            .app_data_dir
            .join("application-servers")
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
            return Err(AppError::Validation("Offset inválido.".to_owned()));
        }
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = vec![0_u8; 32 * 1024];
        let read = file.read(&mut bytes)?;
        bytes.truncate(read);
        let valid = std::str::from_utf8(&bytes).map_or_else(|error| error.valid_up_to(), str::len);
        bytes.truncate(valid);
        Ok(
            serde_json::json!({"chunk":String::from_utf8(bytes).map_err(|_|AppError::Internal("UTF-8 inválido.".to_owned()))?,"nextOffset":offset+valid as u64,"state":status.state}),
        )
    }
    pub fn recover_application_server(
        &self,
        grant: &AuthorizationGrant,
        id: &str,
    ) -> Result<ApplicationServerStatus, AppError> {
        let (status, cwd) = self.database.saved_application_server(id)?;
        self.validate_server_authority(grant, &status)?;
        if status.application_id != grant.application_id
            || !grant.permits(Permission::Execute, Some(&status.account_id), Utc::now())
            || !status.persist_history
        {
            return Err(AppError::AccessDenied(
                "Histórico desta sessão não autorizado ou não persistido.".to_owned(),
            ));
        }
        if self
            .application_servers
            .status(id)
            .is_ok_and(|status| !status.state.terminal())
        {
            return Err(AppError::Busy("Sessão ainda está aberta.".to_owned()));
        }
        let account = self
            .integration_snapshot()?
            .accounts
            .into_iter()
            .find(|account| account.id == status.account_id)
            .ok_or_else(|| AppError::AccessDenied("Conta indisponível.".to_owned()))?;
        let profile = self.database.profile(&account.profile_id)?;
        let remote = profile
            .account_id
            .ok_or_else(|| AppError::AuthenticationRequired("Conta desconectada.".to_owned()))?;
        let session = self.application_auth(&account.id)?;
        let mut resumed = status;
        resumed.state = ServerState::Preparing;
        resumed.grant_id = grant.id.clone();
        resumed.background = resumed
            .group_id
            .as_ref()
            .is_some_and(|group| grant.scope.background_group_ids.contains(group));
        resumed.error_code = None;
        self.launch_application_server(ServerTask {
            status: resumed,
            cwd,
            remote,
            session,
            capacity: account.capacity,
        })
    }
    pub(super) fn update_application_server(
        &self,
        status: ApplicationServerStatus,
        cwd: &str,
    ) -> Result<(), AppError> {
        self.application_servers.update(status.clone());
        let result = self.database.persist_application_server(&status, cwd);
        self.emit_dashboard_changed();
        result
    }

    fn application_server_event_offset(&self, id: &str) -> Result<u64, AppError> {
        uuid::Uuid::parse_str(id)
            .map_err(|_| AppError::Validation("ID de sessão inválido.".to_owned()))?;
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        match options.open(
            self.app_data_dir
                .join("application-servers")
                .join(id)
                .join("events.jsonl"),
        ) {
            Ok(file) => Ok(file.metadata()?.len()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
            Err(error) => Err(error.into()),
        }
    }

    pub(super) fn validate_server_authority(
        &self,
        grant: &AuthorizationGrant,
        status: &ApplicationServerStatus,
    ) -> Result<(), AppError> {
        if status.application_id != grant.application_id
            || !grant.permits(Permission::Execute, Some(&status.account_id), Utc::now())
        {
            return Err(AppError::AccessDenied("Sessão não autorizada.".to_owned()));
        }
        let remote = self
            .database
            .integration_account_remote(&status.account_id)?;
        let (_, policy, _, _) = self.database.account_integration_options(&remote)?;
        let profile = self.database.profile_for_account(&remote)?;
        if policy == crate::domain::integrations::AccountPolicy::DesktopOnly
            || profile.is_none_or(|profile| profile.status != crate::domain::ProfileStatus::Active)
        {
            return Err(AppError::AccessDenied(
                "Conta não está disponível para aplicações.".to_owned(),
            ));
        }
        let permitted_background = self.server_group_allowed(grant, status)?;
        if !self.ui_visible.load(std::sync::atomic::Ordering::Acquire) && !permitted_background {
            return Err(AppError::ConsentRequired(
                "Esta sessão requer a interface aberta.".to_owned(),
            ));
        }
        if self
            .database
            .applications()?
            .iter()
            .all(|app| app.id != status.application_id || app.paused)
            || !status.binding_id.is_empty()
                && self
                    .database
                    .bindings()?
                    .iter()
                    .all(|binding| binding.id != status.binding_id || binding.paused)
        {
            return Err(AppError::Busy(
                "Aplicação ou vínculo pausado/removido.".to_owned(),
            ));
        }
        Ok(())
    }
    fn server_group_allowed(
        &self,
        grant: &AuthorizationGrant,
        status: &ApplicationServerStatus,
    ) -> Result<bool, AppError> {
        let Some(id) = status.group_id.as_deref() else {
            return Ok(false);
        };
        let group = self
            .database
            .groups()?
            .into_iter()
            .find(|group| group.id == id)
            .ok_or_else(|| AppError::AccessDenied("Grupo removido.".to_owned()))?;
        if group.paused
            || group.archived
            || !group.account_ids.contains(&status.account_id)
            || !grant.scope.global_including_future && !grant.scope.group_ids.contains(id)
            || group
                .exclusive_application_id
                .as_ref()
                .is_some_and(|owner| owner != &grant.application_id)
        {
            return Err(AppError::AccessDenied(
                "Grupo não autoriza mais esta sessão.".to_owned(),
            ));
        }
        Ok(grant.scope.background_group_ids.contains(id)
            && group.exclusive_application_id.as_deref() == Some(&grant.application_id))
    }
}

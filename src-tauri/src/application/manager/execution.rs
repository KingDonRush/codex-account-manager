use super::ManagerCore;
mod results;
mod selection;
use crate::domain::execution::{ExecRequest, ExecutionState, ExecutionStatus};
use crate::domain::integrations::{AuthorizationGrant, BindingTarget};
use crate::error::AppError;
use chrono::Utc;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

pub(super) struct Candidate {
    pub(super) group: Option<String>,
    pub(super) account: String,
    pub(super) remote: String,
    pub(super) session: super::application_auth::ApplicationAuth,
    pub(super) capacity: u32,
    score: f64,
}

pub(super) struct GenerationTarget<'a> {
    pub(super) account: &'a str,
    pub(super) remote: &'a str,
    pub(super) session: &'a super::application_auth::ApplicationAuth,
    pub(super) capacity: u32,
}

impl ManagerCore {
    pub fn list_executions(&self) -> Vec<ExecutionStatus> {
        self.executions.list()
    }
    pub fn cancel_execution(&self, id: &str) -> Result<(), AppError> {
        self.executions.cancel(id)
    }
    pub fn recover_executions(&self) -> Result<(), AppError> {
        for status in self.database.recover_execution_metadata()? {
            self.executions.restore(status);
        }
        self.cleanup_execution_outputs();
        Ok(())
    }
    pub fn start_execution(
        &self,
        grant: &AuthorizationGrant,
        mut request: ExecRequest,
    ) -> Result<ExecutionStatus, AppError> {
        crate::adapters::exec_runner::validate_request(&request)?;
        let accounts = self
            .execution_candidates(grant, &request)?
            .into_iter()
            .map(|candidate| candidate.account)
            .collect();
        let binding = self.execution_route(grant, &request)?;
        let selected_model = request
            .arguments
            .iter()
            .enumerate()
            .find_map(|(index, value)| {
                value
                    .strip_prefix("--model=")
                    .map(str::to_owned)
                    .or_else(|| {
                        matches!(value.as_str(), "--model" | "-m")
                            .then(|| request.arguments.get(index + 1).cloned())
                            .flatten()
                    })
            });
        if selected_model.as_ref().is_some_and(|model| {
            !binding.allowed_models.is_empty() && !binding.allowed_models.contains(model)
        }) {
            return Err(AppError::AccessDenied(
                "Modelo não está na lista autorizada deste vínculo.".to_owned(),
            ));
        }
        if selected_model.is_none() {
            if !binding.allowed_models.is_empty() && binding.default_model.is_none() {
                return Err(AppError::Validation(
                    "Escolha explicitamente um modelo autorizado neste vínculo.".to_owned(),
                ));
            }
            if let Some(model) = &binding.default_model {
                request
                    .arguments
                    .extend(["--model".to_owned(), model.clone()]);
            }
        }
        request.binding_snapshot = Some(binding.clone());
        let background = match &binding.target {
            BindingTarget::Group(id) => grant.scope.background_group_ids.contains(id),
            _ => false,
        };
        let status = ExecutionStatus {
            id: uuid::Uuid::new_v4().to_string(),
            application_id: grant.application_id.clone(),
            grant_id: grant.id.clone(),
            binding_id: binding.id,
            binding_revision: binding.revision,
            group_id: match &binding.target {
                BindingTarget::Group(id) => Some(id.clone()),
                _ => None,
            },
            account_id: None,
            state: ExecutionState::Queued,
            started_at: Utc::now(),
            finished_at: None,
            error_code: None,
            exit_code: None,
            background,
        };
        let lease = self.tasks.enter()?;
        let cancel = Arc::new(AtomicBool::new(false));
        self.executions
            .add(status.clone(), cancel.clone(), accounts)?;
        if let Err(error) = self.database.persist_execution(&status) {
            self.executions.fail_submission(&status.id, &error);
            self.emit_dashboard_changed();
            return Err(error);
        }
        let core = self.clone();
        let worker_status = status.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("authorized-exec".to_owned())
            .spawn(move || {
                let _lease = lease;
                core.run_execution(worker_status, request, cancel);
            })
        {
            let mut failed = status.clone();
            failed.state = ExecutionState::Failed;
            failed.error_code = Some("worker_unavailable".to_owned());
            self.update_execution(failed)?;
            return Err(error.into());
        }
        self.emit_dashboard_changed();
        Ok(status)
    }

    fn run_execution(
        &self,
        mut status: ExecutionStatus,
        request: ExecRequest,
        cancel: Arc<AtomicBool>,
    ) {
        let result = self.execute_job(&mut status, &request, cancel);
        status.finished_at = Some(Utc::now());
        match result {
            Ok(0) => {
                status.state = ExecutionState::Completed;
                status.exit_code = Some(0);
            }
            Ok(code) => {
                status.state = ExecutionState::Failed;
                status.exit_code = Some(code);
                status
                    .error_code
                    .get_or_insert_with(|| "exec_failed".to_owned());
            }
            Err(error) => {
                status.state = if matches!(error, AppError::Cancelled) {
                    ExecutionState::Cancelled
                } else {
                    ExecutionState::Failed
                };
                status
                    .error_code
                    .get_or_insert_with(|| error.code().to_owned());
            }
        }
        let _ = self.update_execution(status);
    }

    fn execute_job(
        &self,
        status: &mut ExecutionStatus,
        request: &ExecRequest,
        cancel: Arc<AtomicBool>,
    ) -> Result<i32, AppError> {
        let (candidate, _generation) = loop {
            if cancel.load(Ordering::Acquire) || self.maintenance.cancel.load(Ordering::Acquire) {
                return Err(AppError::Cancelled);
            }
            let grant = self.authorization_grant(&status.grant_id)?;
            let candidates = self.execution_candidates(&grant, request)?;
            let mut reserved = None;
            for candidate in candidates {
                status.group_id = candidate.group.clone();
                self.update_execution(status.clone())?;
                let target = GenerationTarget {
                    account: &candidate.account,
                    remote: &candidate.remote,
                    session: &candidate.session,
                    capacity: candidate.capacity,
                };
                if let Some(lease) = self.reserve_generation(&status.id, target, &cancel)? {
                    reserved = Some((candidate, lease));
                    break;
                }
            }
            if let Some(reserved) = reserved {
                break reserved;
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        status.account_id = Some(candidate.account.clone());
        status.state = ExecutionState::Preparing;
        self.update_execution(status.clone())?;
        let document = self.load_application_auth(
            &candidate.session,
            &candidate.remote,
            cancel.clone(),
            false,
        )?;
        let home = crate::adapters::app_server::IsolatedCodexHome::create()?;
        let auth = home.directory.path().join("auth.json");
        match &candidate.session {
            super::application_auth::ApplicationAuth::Dedicated(_) => {
                crate::adapters::auth_store::atomic_write_auth(&auth, &document)?
            }
            super::application_auth::ApplicationAuth::Account(_) => {
                crate::adapters::auth_store::write_external_auth(
                    &auth,
                    &document,
                    &candidate.remote,
                )?
            }
        }
        let output = self.execution_directory(&status.id)?;
        status.state = ExecutionState::Running;
        self.update_execution(status.clone())?;
        let result = self.run_private_exec(
            request,
            home.directory.path(),
            &output.join("events.jsonl"),
            cancel,
        );
        // Account-backed children cannot rotate a refresh chain. Do not interpret
        // their external auth document as a new managed generation.
        let super::application_auth::ApplicationAuth::Dedicated(session) = &candidate.session
        else {
            return result;
        };
        // Legacy dedicated-session ownership persists through collection.
        let refreshed =
            crate::adapters::auth_store::read_auth(&home.directory.path().join("auth.json"))?;
        crate::adapters::auth_store::validate_auth_document(&refreshed, Some(&candidate.remote))?;
        if refreshed != document {
            match self.save_application_credential(session, &candidate.remote, &refreshed) {
                Ok(()) => {}
                Err(AppError::CredentialRecoveryPending(_)) => {
                    status.error_code = Some("credential_recovery_pending".to_owned())
                }
                Err(error) => return Err(error),
            }
        }
        result
    }

    fn run_private_exec(
        &self,
        request: &ExecRequest,
        home: &std::path::Path,
        output: &std::path::Path,
        cancel: Arc<AtomicBool>,
    ) -> Result<i32, AppError> {
        #[cfg(test)]
        if let Some(binary) = self.exec_test_binary.as_ref() {
            return crate::adapters::exec_runner::run_test_binary(
                request,
                crate::adapters::exec_runner::TestExecutionContext {
                    home,
                    output,
                    binary,
                },
                cancel,
            );
        }
        crate::adapters::exec_runner::run(request, home, output, cancel)
    }

    pub(super) fn update_execution(&self, status: ExecutionStatus) -> Result<(), AppError> {
        self.executions.update(status.clone());
        let persisted = self.database.persist_execution(&status);
        self.emit_dashboard_changed();
        persisted
    }

    pub(super) fn reserve_generation(
        &self,
        job: &str,
        target: GenerationTarget<'_>,
        cancel: &AtomicBool,
    ) -> Result<Option<crate::application::execution_book::GenerationLease>, AppError> {
        let _admission = crate::application::operation_gate::acquire(
            &self.account_admission,
            cancel,
            Duration::from_secs(30),
        )?;
        let profile = self
            .database
            .profile_for_account(target.remote)?
            .ok_or_else(|| AppError::AccessDenied("Conta foi desvinculada.".to_owned()))?;
        let (local, policy, capacity, _) =
            self.database.account_integration_options(target.remote)?;
        if policy == crate::domain::integrations::AccountPolicy::DesktopOnly
            || profile.status != crate::domain::ProfileStatus::Active
        {
            return Err(AppError::AccessDenied(
                "A conta deixou de estar disponível para aplicações.".to_owned(),
            ));
        }
        if local != target.account || &self.application_auth(&local)? != target.session {
            return Err(AppError::Busy(
                "A sessão mudou antes de iniciar; envie um novo trabalho.".to_owned(),
            ));
        }
        let operations = self.operations.lock();
        if operations.starting.contains_key(&profile.id)
            || operations
                .controls
                .values()
                .any(|control| control.profile_id == profile.id)
        {
            return Ok(None);
        }
        Ok(self.executions.try_reserve(
            job,
            target.account,
            target.session.lease_key(),
            capacity.min(target.capacity),
        ))
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use chrono::Utc;
use parking_lot::Mutex;
use serde_json::json;

use super::{mask_id, ManagerCore, OperationControl};
use crate::adapters::{auth_store, logging};
use crate::domain::{
    AccountMetadata, LoginOperation, OperationEvent, OperationKind, OperationState, ProfileStatus,
    SwitchResult,
};
use crate::error::AppError;

impl ManagerCore {
    pub fn open_desktop(&self, operation_id: Option<&str>) -> Result<(), AppError> {
        let _lease = self.tasks.enter()?;
        let operation = operation_id
            .map(|id| self.database.operation(id))
            .transpose()?;
        if let Some(operation) = operation.as_ref().filter(|operation| {
            operation.kind == OperationKind::Switch
                && matches!(operation.state, OperationState::Completed)
        }) {
            let profile = self.database.profile(&operation.profile_id)?;
            if profile.account_id != auth_store::active_account_id_at(&self.codex_home) {
                return Err(AppError::Busy(
                    "A sessão ativa mudou depois desta troca; abra o Codex pela conta atual."
                        .to_owned(),
                ));
            }
        }
        self.session_switcher.open_desktop()?;
        if let Some(operation) = operation.as_ref().filter(|operation| {
            operation.error_code.as_deref() == Some("desktop_reopen_pending")
                && matches!(operation.state, OperationState::Completed)
        }) {
            self.finish_operation(
                &operation.id,
                &operation.profile_id,
                OperationState::Completed,
                "Sessão ativada e abertura do Codex confirmada",
                None,
            )?;
        }
        Ok(())
    }

    pub fn select_desktop(&self, owner: isize) -> Result<bool, AppError> {
        let _lease = self.tasks.enter()?;
        self.session_switcher.select_desktop(owner)
    }

    pub fn confirm_desktop_close(
        &self,
        operation_id: &str,
        owner: isize,
    ) -> Result<bool, AppError> {
        let _lease = self.tasks.enter()?;
        let operation = self.database.operation(operation_id)?;
        if operation.kind != OperationKind::Switch
            || !matches!(operation.state, OperationState::Failed)
            || operation.error_code.as_deref() != Some("desktop_close_pending")
        {
            return Err(AppError::AccessDenied(
                "Este atalho pertence somente a uma troca bloqueada pelo fechamento do Codex."
                    .to_owned(),
            ));
        }
        self.session_switcher.confirm_desktop_close(owner)
    }

    pub fn start_switch_account(&self, profile_id: &str) -> Result<LoginOperation, AppError> {
        let task_lease = self.tasks.enter()?;
        let profile = self.database.profile(profile_id)?;
        let account_id = profile
            .account_id
            .ok_or_else(|| AppError::Validation("conecte esta conta antes de usá-la".to_owned()))?;
        let (_, policy, _, _) = self.database.account_integration_options(&account_id)?;
        if policy == crate::domain::integrations::AccountPolicy::ApplicationsOnly {
            return Err(AppError::ConsentRequired("Esta conta está reservada para aplicações. Confirme o compartilhamento antes de usar no Desktop.".to_owned()));
        }
        if !matches!(profile.status, ProfileStatus::Active) {
            return Err(AppError::Validation(
                "restaure a conta antes de usá-la".to_owned(),
            ));
        }
        self.reserve_operation(profile_id, OperationKind::Switch)?;
        let operation = match self.database.create_switch_operation(profile_id) {
            Ok(operation) => operation,
            Err(error) => {
                self.release_operation_reservation(profile_id);
                return Err(error);
            }
        };
        let control = Arc::new(OperationControl {
            profile_id: profile_id.to_owned(),
            kind: OperationKind::Switch,
            cancel: Arc::new(AtomicBool::new(false)),
            cancelable: AtomicBool::new(true),
            cancellation_gate: Mutex::new(()),
            browser: Arc::new(Mutex::new(None)),
            replacement: Mutex::new(None),
            confirmed: AtomicBool::new(false),
        });
        self.install_operation_control(&operation.id, control.clone());

        let manager = self.clone();
        let operation_id = operation.id.clone();
        let worker_profile_id = profile_id.to_owned();
        let worker = thread::Builder::new()
            .name(format!("switch-operation-{}", operation.id))
            .spawn(move || {
                let _task_lease = task_lease;
                manager.run_switch(operation_id, worker_profile_id, account_id, control)
            });
        if let Err(error) = worker {
            self.operations.lock().controls.remove(&operation.id);
            let _ = self.finish_operation(
                &operation.id,
                profile_id,
                OperationState::Failed,
                "Não foi possível iniciar a troca em segundo plano",
                Some("thread_spawn_failed"),
            );
            return Err(AppError::Internal(error.to_string()));
        }
        Ok(operation)
    }

    fn run_switch(
        &self,
        operation_id: String,
        profile_id: String,
        account_id: String,
        control: Arc<OperationControl>,
    ) {
        let result = self.execute_switch(&operation_id, &profile_id, &account_id, &control);
        match result {
            Ok(result) => self.complete_switch_operation(&operation_id, &profile_id, result),
            Err(error) => self.fail_switch_operation(&operation_id, &profile_id, error),
        }
        self.operations.lock().controls.remove(&operation_id);
        self.emit_dashboard_changed();
    }

    fn complete_switch_operation(
        &self,
        operation_id: &str,
        profile_id: &str,
        result: SwitchResult,
    ) {
        let step = switch_completion_step(&result);
        let _ = self.finish_operation(
            operation_id,
            profile_id,
            OperationState::Completed,
            step,
            result.warning.as_ref().map(|_| "desktop_reopen_pending"),
        );
    }

    fn fail_switch_operation(&self, operation_id: &str, profile_id: &str, error: AppError) {
        let state = if matches!(error, AppError::Cancelled) {
            OperationState::Cancelled
        } else {
            OperationState::Failed
        };
        let message = if matches!(error, AppError::SessionRenewalRequired(_)) {
            "Não foi possível confirmar a sessão após a tentativa de renovação. A sessão salva foi preservada.".to_owned()
        } else if error.is_refresh_rejected() {
            "A sessão salva foi rejeitada. Entre novamente pelo navegador para usar esta conta."
                .to_owned()
        } else {
            logging::sanitize_error(&error.to_string())
        };
        if !matches!(state, OperationState::Cancelled) {
            let _ = self.database.update_profile_error(profile_id, &message);
            self.record_failed_credential_state(profile_id, &error);
        }
        let _ = self.finish_operation(
            operation_id,
            profile_id,
            state,
            &message,
            Some(if error.is_refresh_rejected() {
                "authentication_required"
            } else {
                error.code()
            }),
        );
    }

    fn record_failed_credential_state(&self, profile_id: &str, error: &AppError) {
        let Ok(profile) = self.database.profile(profile_id) else {
            return;
        };
        let Some(account_id) = profile.account_id.as_deref() else {
            return;
        };
        let state = if error.is_refresh_rejected() {
            crate::domain::CredentialState::Quarantined
        } else if matches!(error, AppError::CredentialUnavailable(_)) {
            crate::domain::CredentialState::Unavailable
        } else {
            return;
        };
        let _ = self.database.set_credential_state(account_id, state);
    }

    fn execute_switch(
        &self,
        operation_id: &str,
        profile_id: &str,
        account_id: &str,
        control: &OperationControl,
    ) -> Result<SwitchResult, AppError> {
        self.transition(
            operation_id,
            profile_id,
            OperationState::Preparing,
            "Preparando a troca de conta",
        )?;
        if control.cancel.load(Ordering::Acquire) {
            return Err(AppError::Cancelled);
        }

        let lock_started = Instant::now();
        let _guard = crate::application::operation_gate::acquire(
            &self.switch_lock,
            &control.cancel,
            Duration::from_secs(30),
        )?;
        self.record_switch_stage(
            operation_id,
            profile_id,
            "switch_lock_wait",
            lock_started.elapsed(),
        );
        if control.cancel.load(Ordering::Acquire) {
            return Err(AppError::Cancelled);
        }

        let _session_guard = crate::adapters::file_lock::FileLock::acquire_cancellable(
            &self.codex_home.join("auth-switch.lock"),
            &control.cancel,
            Duration::from_secs(30),
        )?;
        self.transition(
            operation_id,
            profile_id,
            OperationState::Preparing,
            "Verificando a conta de destino sem interromper o Codex",
        )?;
        let destination = self
            .session_switcher
            .preflight_target(account_id, control.cancel.clone())?;
        self.transition(
            operation_id,
            profile_id,
            OperationState::Preparing,
            if destination.needs_exclusive_renewal() {
                "Preparando a renovação automática; aguardando a liberação da sessão"
            } else {
                "Aguardando liberação da sessão e verificando outros consumidores"
            },
        )?;
        let runtime_started = Instant::now();
        let runtime_result = self.shared_runtime.prepare_global_change(&control.cancel);
        self.record_switch_stage(
            operation_id,
            profile_id,
            "shared_runtime_preflight",
            runtime_started.elapsed(),
        );
        runtime_result?;
        self.session_switcher
            .recover(|id| self.database.switch_was_committed(id))?;
        self.transition(
            operation_id,
            profile_id,
            OperationState::Verifying,
            "Validando a credencial e reiniciando o Codex",
        )?;
        let mut stages = Vec::new();
        let switch_result = self.session_switcher.apply_prepared_destination(
            destination,
            &mut |stage, duration| {
                stages.push((stage, duration));
                self.emit_switch_progress(operation_id, profile_id, stage, duration);
            },
        );
        for (stage, duration) in stages {
            self.record_switch_stage(operation_id, profile_id, stage, duration);
        }
        let mut outcome = switch_result?;

        let persist_started = Instant::now();
        let persist_result =
            self.persist_switch_metadata(operation_id, profile_id, account_id, &outcome);
        self.record_switch_stage(
            operation_id,
            profile_id,
            "persist_switch_metadata",
            persist_started.elapsed(),
        );
        if let Err(error) = persist_result {
            self.rollback_switch_metadata(&outcome, error)?;
        }

        *self.active_account_id.write() = Some(account_id.to_owned());
        if let Err(error) = self.session_switcher.reopen_committed_desktop(
            &mut outcome,
            |id| self.database.switch_was_committed(id),
            &mut |stage, duration| {
                self.record_switch_stage(operation_id, profile_id, stage, duration);
                self.emit_switch_progress(operation_id, profile_id, stage, duration);
            },
        ) {
            outcome.result.warning = Some(logging::sanitize_error(&error.to_string()));
        }
        if let Err(error) = self.session_switcher.acknowledge_metadata(&outcome) {
            let _ = logging::record(
                &self.database,
                Some(operation_id),
                Some(profile_id),
                "switch_journal_cleanup_pending",
                json!({"errorCode":error.code()}),
            );
        }
        let _ = logging::record(
            &self.database,
            Some(operation_id),
            Some(profile_id),
            "account_switched",
            json!({
                "previous": outcome.result.previous_account_id.as_deref().map(mask_id),
                "target": mask_id(account_id),
                "desktopRestarted": outcome.result.desktop_restarted,
            }),
        );
        Ok(outcome.result)
    }

    fn persist_switch_metadata(
        &self,
        operation_id: &str,
        profile_id: &str,
        account_id: &str,
        outcome: &crate::adapters::session_switch::SessionSwitchOutcome,
    ) -> Result<(), AppError> {
        self.transition(
            operation_id,
            profile_id,
            OperationState::Persisting,
            "Atualizando quota e histórico",
        )?;
        let profile = self.database.profile(profile_id)?;
        let email = outcome
            .snapshot
            .email
            .as_deref()
            .or(profile.email.as_deref());
        let provider_id = email
            .and_then(|value| self.resolve_provider_id(value))
            .or(profile.mail_provider_id);
        self.database.persist_switch_commit(
            profile_id,
            AccountMetadata {
                account_id,
                email,
                plan_type: outcome.snapshot.plan_type.as_deref(),
                credential_ref: Some("native-keyring"),
                rate_limits: outcome.snapshot.rate_limits.as_ref(),
                observed_at: Utc::now(),
            },
            crate::adapters::database::SwitchCommit {
                provider_id: provider_id.as_deref(),
                previous_account_id: outcome.result.previous_account_id.as_deref(),
                transaction_id: outcome.transaction_id.as_deref(),
            },
        )?;
        Ok(())
    }

    fn rollback_switch_metadata(
        &self,
        outcome: &crate::adapters::session_switch::SessionSwitchOutcome,
        error: AppError,
    ) -> Result<(), AppError> {
        if outcome.result.changed {
            if let Err(rollback_error) = self.session_switcher.rollback_materialized_switch(outcome)
            {
                *self.active_account_id.write() =
                    auth_store::active_account_id_at(&self.codex_home);
                return Err(AppError::Internal(format!(
                    "falha ao persistir a troca ({error}); rollback também falhou ({rollback_error})"
                )));
            }
        }
        *self.active_account_id.write() = auth_store::active_account_id_at(&self.codex_home);
        Err(error)
    }

    fn record_switch_stage(
        &self,
        operation_id: &str,
        profile_id: &str,
        stage: &'static str,
        duration: Duration,
    ) {
        let _ = logging::record(
            &self.database,
            Some(operation_id),
            Some(profile_id),
            "switch_stage",
            json!({
                "stage": stage,
                "durationMs": duration.as_millis(),
            }),
        );
    }

    fn emit_switch_progress(
        &self,
        operation_id: &str,
        profile_id: &str,
        stage: &'static str,
        duration: Duration,
    ) {
        let Some(step) = switch_progress_step(stage) else {
            return;
        };
        let Ok(mut operation) = self.database.operation(operation_id) else {
            return;
        };
        operation.step = step.to_owned();
        operation.last_stage_duration_ms =
            Some(i64::try_from(duration.as_millis()).unwrap_or(i64::MAX));
        if self.database.save_operation(&operation).is_err() {
            return;
        }
        self.emit(OperationEvent {
            operation_id: operation_id.to_owned(),
            profile_id: profile_id.to_owned(),
            kind: OperationKind::Switch,
            state: operation.state,
            step: step.to_owned(),
            message: step.to_owned(),
            error_code: None,
            stage_duration_ms: Some(i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)),
            cancelable: false,
        });
    }
}

fn switch_completion_step(result: &SwitchResult) -> &'static str {
    if result.warning.is_some() {
        return "Sessão ativada; a abertura do Codex não foi confirmada";
    }
    if !result.changed {
        return "Esta conta já era a sessão atual";
    }
    if !result.desktop_restarted {
        return "Sessão ativada; o Codex estava fechado";
    }
    "Conta ativada e Codex reaberto"
}

fn switch_progress_step(stage: &str) -> Option<&'static str> {
    match stage {
        "desktop_process_discovery" => Some("Solicitando o fechamento do Codex"),
        "desktop_graceful_signal" => Some("Aguardando o Codex fechar"),
        "desktop_force_signal" => Some("Finalizando o Codex com segurança"),
        "credential_generation_load" => Some("Lendo a sessão protegida"),
        "materialize_target_auth" => Some("Ativando a sessão local"),
        "app_server_spawn_initialize" => Some("Iniciando a validação da conta"),
        "app_server_account_read" => Some("Conferindo a identidade da conta"),
        "app_server_rate_limits_read" | "app_server_rate_limits_unavailable" => {
            Some("Atualizando a quota")
        }
        "persist_target_session" | "persist_rotated_target_session" => {
            Some("Atualizando a sessão protegida")
        }
        "desktop_restart" => Some("Reabrindo o Codex"),
        "desktop_restart_after_rollback" => Some("Restaurando o Codex"),
        _ => None,
    }
}

#[cfg(test)]
mod progress_tests {
    use super::switch_progress_step;

    #[test]
    fn progress_uses_human_steps_and_ignores_internal_bookkeeping() {
        assert_eq!(
            switch_progress_step("desktop_graceful_signal"),
            Some("Aguardando o Codex fechar")
        );
        assert_eq!(
            switch_progress_step("app_server_account_read"),
            Some("Conferindo a identidade da conta")
        );
        assert_eq!(
            switch_progress_step("persist_rotated_target_session"),
            Some("Atualizando a sessão protegida")
        );
        assert_eq!(switch_progress_step("write_switch_journal_prepared"), None);
    }
}

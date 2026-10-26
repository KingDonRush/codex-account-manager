use std::sync::atomic::Ordering;

use chrono::Utc;
use serde_json::json;

use super::{ManagerCore, OperationControl};
use crate::adapters::browser_runtime::BrowserHandle;
use crate::adapters::logging;
use crate::domain::{OperationEvent, OperationKind, OperationState};
use crate::error::AppError;

impl ManagerCore {
    pub fn cancel_operation(&self, operation_id: &str) -> Result<(), AppError> {
        // Live memory is authoritative while an adapter is slow; SQLite is the
        // durable diagnostic record and must not delay cancellation.
        let control = self.operations.lock().controls.get(operation_id).cloned();
        let Some(control) = control else {
            let operation = self.database.operation(operation_id)?;
            return if operation.state.is_terminal() {
                Err(AppError::OperationFinished)
            } else {
                Err(AppError::OperationNotFound(operation_id.to_owned()))
            };
        };
        let _gate = control.cancellation_gate.lock();
        if !control.cancelable.load(Ordering::Acquire) {
            return Err(AppError::Validation(
                "esta etapa não pode mais ser cancelada com segurança".to_owned(),
            ));
        }
        if control.cancel.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        // The operation worker owns browser cleanup before its terminal result.
        // Cancellation interrupts its App Server request; no detached second closer.
        Ok(())
    }

    pub(super) fn transition(
        &self,
        operation_id: &str,
        profile_id: &str,
        next: OperationState,
        step: &str,
    ) -> Result<(), AppError> {
        let mut operation = self.database.operation(operation_id)?;
        let control = self.operations.lock().controls.get(operation_id).cloned();
        let _gate = control
            .as_ref()
            .map(|control| control.cancellation_gate.lock());
        let cancelable = operation.kind.is_cancelable_in(&next);
        if control
            .as_ref()
            .is_some_and(|control| control.cancel.load(Ordering::Acquire))
        {
            return Err(AppError::Cancelled);
        }
        let previous_state = operation.state.clone();
        let transitioned_at = Utc::now();
        let stage_duration_ms = (transitioned_at - operation.stage_started_at)
            .num_milliseconds()
            .max(0);
        let state = crate::application::state_machine::transition(&operation.state, next)?;
        operation.state = state.clone();
        operation.step = step.to_owned();
        operation.stage_started_at = transitioned_at;
        operation.last_stage_duration_ms = Some(stage_duration_ms);
        operation.cancelable = cancelable;
        if let Some(control) = control.as_ref() {
            control
                .cancelable
                .store(operation.cancelable, Ordering::Release);
        }
        self.database.save_operation(&operation)?;
        logging::record(
            &self.database,
            Some(operation_id),
            Some(profile_id),
            "operation_transition",
            json!({
                "previousState": previous_state.as_str(),
                "state": state.as_str(),
                "step": step,
                "stageDurationMs": stage_duration_ms,
            }),
        )?;
        self.emit(OperationEvent {
            operation_id: operation_id.to_owned(),
            profile_id: profile_id.to_owned(),
            kind: operation.kind,
            state,
            step: step.to_owned(),
            message: step.to_owned(),
            error_code: None,
            stage_duration_ms: Some(stage_duration_ms),
            cancelable: operation.cancelable,
        });
        Ok(())
    }

    pub(super) fn finish_operation(
        &self,
        operation_id: &str,
        profile_id: &str,
        state: OperationState,
        step: &str,
        error_code: Option<&str>,
    ) -> Result<(), AppError> {
        let mut operation = self.database.operation(operation_id)?;
        let finished_at = Utc::now();
        let stage_duration_ms = (finished_at - operation.stage_started_at)
            .num_milliseconds()
            .max(0);
        if !operation.state.is_terminal() {
            operation.state =
                crate::application::state_machine::transition(&operation.state, state.clone())?;
        }
        operation.step = step.to_owned();
        let duration_ms = (finished_at - operation.started_at)
            .num_milliseconds()
            .max(0);
        operation.finished_at = Some(finished_at);
        operation.stage_started_at = finished_at;
        operation.last_stage_duration_ms = Some(stage_duration_ms);
        operation.error_code = error_code.map(str::to_owned);
        operation.error_message = error_code.map(|_| step.to_owned());
        operation.cancelable = false;
        self.database.save_operation(&operation)?;
        if let Some(control) = self.operations.lock().controls.get(operation_id) {
            control.cancelable.store(false, Ordering::Release);
        }
        self.emit(OperationEvent {
            operation_id: operation_id.to_owned(),
            profile_id: profile_id.to_owned(),
            kind: operation.kind,
            state: operation.state.clone(),
            step: step.to_owned(),
            message: step.to_owned(),
            error_code: error_code.map(str::to_owned),
            stage_duration_ms: Some(stage_duration_ms),
            cancelable: false,
        });
        logging::record(
            &self.database,
            Some(operation_id),
            Some(profile_id),
            "operation_finished",
            json!({
                "state": operation.state.as_str(),
                "errorCode": error_code,
                "durationMs": duration_ms,
                "stageDurationMs": stage_duration_ms,
            }),
        )?;
        Ok(())
    }

    pub(super) fn emit(&self, event: OperationEvent) {
        if let Some(sink) = self.event_sink.read().as_ref() {
            sink.emit(&event);
        }
    }

    pub(super) fn emit_dashboard_changed(&self) {
        self.dashboard_revision.fetch_add(1, Ordering::AcqRel);
        if let Some(sink) = self.event_sink.read().as_ref() {
            sink.emit_dashboard_changed();
        }
    }

    pub(super) fn reserve_operation(
        &self,
        profile_id: &str,
        kind: OperationKind,
    ) -> Result<(), AppError> {
        if self.maintenance.cancel.load(Ordering::Acquire) {
            return Err(AppError::Cancelled);
        }
        let mut operations = self.operations.lock();
        let profile_busy = operations
            .controls
            .values()
            .any(|control| control.profile_id == profile_id)
            || operations.starting.contains_key(profile_id);
        if profile_busy {
            return Err(AppError::Validation(
                "já existe uma operação em andamento para esta conta".to_owned(),
            ));
        }
        let switch_busy = matches!(kind, OperationKind::Switch)
            && (operations
                .controls
                .values()
                .any(|control| matches!(control.kind, OperationKind::Switch))
                || operations
                    .starting
                    .values()
                    .any(|pending| matches!(pending, OperationKind::Switch)));
        if switch_busy {
            return Err(AppError::Validation(
                "já existe uma troca de conta em andamento".to_owned(),
            ));
        }
        operations.starting.insert(profile_id.to_owned(), kind);
        Ok(())
    }

    pub(super) fn release_operation_reservation(&self, profile_id: &str) {
        self.operations.lock().starting.remove(profile_id);
    }

    pub(super) fn install_operation_control(
        &self,
        id: &str,
        control: std::sync::Arc<OperationControl>,
    ) {
        let mut operations = self.operations.lock();
        operations.starting.remove(&control.profile_id);
        operations.controls.insert(id.to_owned(), control);
    }
}

pub(super) fn ensure_login_active(
    control: &OperationControl,
    browser: Option<&BrowserHandle>,
) -> Result<(), AppError> {
    if control.cancel.load(Ordering::Acquire) {
        return Err(AppError::Cancelled);
    }
    if let Some(handle) = browser {
        handle.ensure_alive()?;
    }
    Ok(())
}

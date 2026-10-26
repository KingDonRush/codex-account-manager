use super::{
    is_definitive_credential_failure, measure, single_current_generation, verify_session,
    PreviousSessionPreservation, SessionSwitchOutcome, SessionSwitcher, SwitchJournal,
    SwitchPreparation,
};
use crate::adapters::{auth_store, desktop_control::DesktopState};
use crate::domain::{AccountSnapshot, SwitchResult};
use crate::error::AppError;
use crate::ports::StoredCredentialGeneration;
use serde_json::Value;
use std::time::Duration;

struct MaterializedSession<'a> {
    desktop: DesktopState,
    previous_id: Option<String>,
    previous_document: Option<Value>,
    target_id: &'a str,
    snapshot: AccountSnapshot,
}

impl SessionSwitcher {
    pub(super) fn preserve_previous_session(
        &self,
        previous: Option<&Value>,
        preservation: PreviousSessionPreservation,
        trace: &mut dyn FnMut(&'static str, Duration),
    ) -> Result<(), AppError> {
        let Some(document) = previous else {
            return Ok(());
        };
        measure(trace, "preserve_previous_session", || {
            match preservation {
                PreviousSessionPreservation::Select => self.credentials.save_generation(document),
                PreviousSessionPreservation::Archive => {
                    self.credentials.archive_generation(document)
                }
            }
            .map(|_| ())
        })
    }

    pub(super) fn activate_current_generation(
        &self,
        target_account_id: &str,
        mut preparation: SwitchPreparation,
        desktop: DesktopState,
        trace: &mut dyn FnMut(&'static str, Duration),
    ) -> Result<SessionSwitchOutcome, AppError> {
        let generation = single_current_generation(target_account_id, &preparation.generations)?;
        let snapshot = match self.try_generation(generation, &mut preparation.previous, trace) {
            Ok(snapshot) => snapshot,
            Err(error @ AppError::CredentialRecoveryPending(_)) => return Err(error),
            Err(error) => {
                self.restore_after_failed_switch(&desktop, preparation.previous.as_ref(), trace)?;
                return Err(error);
            }
        };
        self.materialized_outcome(MaterializedSession {
            desktop,
            previous_id: preparation.previous_id,
            previous_document: preparation.previous,
            target_id: target_account_id,
            snapshot,
        })
    }

    fn try_generation(
        &self,
        generation: &StoredCredentialGeneration,
        previous: &mut Option<Value>,
        trace: &mut dyn FnMut(&'static str, Duration),
    ) -> Result<AccountSnapshot, AppError> {
        let result = measure(trace, "credential_generation_load", || {
            self.credentials
                .load_generation(&generation.account_id, &generation.generation_id)
        })
        .and_then(|document| self.activate_generation(generation, &document, previous, trace));
        if result.as_ref().is_err_and(is_definitive_credential_failure) {
            self.credentials
                .quarantine_generation(&generation.account_id, &generation.generation_id)?;
        }
        result
    }

    fn activate_generation(
        &self,
        generation: &StoredCredentialGeneration,
        target: &Value,
        previous: &mut Option<Value>,
        trace: &mut dyn FnMut(&'static str, Duration),
    ) -> Result<AccountSnapshot, AppError> {
        let target_id = &generation.account_id;
        auth_store::validate_auth_document(target, Some(target_id))?;
        measure(trace, "materialize_target_auth", || {
            auth_store::atomic_write_auth(&self.auth_path, target)
        })?;
        measure(trace, "write_switch_journal_materialized", || {
            let previous_id = previous
                .as_ref()
                .map(|doc| auth_store::validate_auth_document(doc, None))
                .transpose()?;
            self.write_journal(previous_id, target_id, "materialized")
        })?;
        let verification = verify_session(
            self.verifier.as_ref(),
            target_id,
            "verify_target_session",
            trace,
        );
        let refreshed = self.capture_target_session(target_id, target, previous, trace)?;
        let snapshot = verification?;
        if refreshed == *target {
            measure(trace, "activate_credential_generation", || {
                self.credentials
                    .recover_generation(target_id, &generation.generation_id)
            })?;
            measure(trace, "persist_target_session", || {
                self.credentials.save_generation(&refreshed).map(|_| ())
            })?;
        }
        Ok(snapshot)
    }

    fn capture_target_session(
        &self,
        expected: &str,
        target: &Value,
        previous: &mut Option<Value>,
        trace: &mut dyn FnMut(&'static str, Duration),
    ) -> Result<Value, AppError> {
        let refreshed = measure(trace, "read_refreshed_auth", || {
            auth_store::read_auth(&self.auth_path)
        })?;
        auth_store::validate_auth_document(&refreshed, Some(expected))?;
        if refreshed != *target {
            if previous.as_ref().is_some_and(|doc| {
                doc.pointer("/tokens/account_id") == target.pointer("/tokens/account_id")
                    && doc.pointer("/tokens/refresh_token")
                        == target.pointer("/tokens/refresh_token")
            }) {
                *previous = Some(refreshed.clone());
            }
            // Rotation is an effect even when the following identity query fails.
            measure(trace, "persist_rotated_target_session", || {
                self.credentials.save_generation(&refreshed).map(|_| ())
            })?;
        }
        Ok(refreshed)
    }

    fn restore_after_failed_switch(
        &self,
        desktop: &DesktopState,
        previous: Option<&Value>,
        trace: &mut dyn FnMut(&'static str, Duration),
    ) -> Result<(), AppError> {
        measure(trace, "rollback_auth", || {
            self.rollback(&self.auth_path, previous)
        })?;
        let remove_started = std::time::Instant::now();
        let _ = std::fs::remove_file(&self.journal_path);
        trace("remove_switch_journal", remove_started.elapsed());
        measure(trace, "desktop_restart_after_rollback", || {
            self.desktop.restart(desktop)
        })
        .map(|_| ())
    }

    fn materialized_outcome(
        &self,
        session: MaterializedSession<'_>,
    ) -> Result<SessionSwitchOutcome, AppError> {
        let journal: SwitchJournal =
            serde_json::from_reader(std::fs::File::open(&self.journal_path)?)?;
        Ok(SessionSwitchOutcome {
            result: SwitchResult {
                previous_account_id: session.previous_id,
                active_account_id: session.target_id.to_owned(),
                changed: true,
                desktop_restarted: false,
                warning: None,
            },
            snapshot: session.snapshot,
            previous_document: session.previous_document,
            transaction_id: journal.transaction_id,
            desktop: Some(session.desktop),
        })
    }

    /// Reopening is a post-commit effect. A failed activation must not replay the
    /// authentication transaction or roll back underneath a newly started app.
    pub fn reopen_committed_desktop(
        &self,
        outcome: &mut SessionSwitchOutcome,
        was_committed: impl FnOnce(&str) -> Result<bool, AppError>,
        trace: &mut dyn FnMut(&'static str, Duration),
    ) -> Result<(), AppError> {
        let Some(id) = outcome.transaction_id.as_deref() else {
            return Ok(());
        };
        if !was_committed(id)? {
            return Err(AppError::Busy(
                "O Desktop não pode abrir antes do commit da troca.".to_owned(),
            ));
        }
        let Some(mut desktop) = outcome.desktop.take() else {
            return Ok(());
        };
        desktop.request_open();
        match measure(trace, "desktop_restart", || self.desktop.restart(&desktop)) {
            Ok(launch) => {
                outcome.result.desktop_restarted = launch.was_observed();
                if !launch.was_observed() {
                    trace("desktop_restart_not_required", Duration::ZERO);
                }
            }
            Err(error) => {
                outcome.result.warning =
                    Some(crate::adapters::logging::sanitize_error(&error.to_string()));
                trace("desktop_restart_failed", Duration::ZERO);
            }
        }
        Ok(())
    }
}

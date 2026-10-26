use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

use super::ManagerCore;
use crate::domain::{CredentialGenerationState, CredentialState};
use crate::error::AppError;

const MAINTENANCE_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

impl ManagerCore {
    pub(super) fn audit_credentials_in_background(&self) {
        let Ok(task_lease) = self.tasks.enter() else {
            return;
        };
        let manager = self.clone();
        let _ = thread::Builder::new()
            .name("credential-audit".to_owned())
            .spawn(move || {
                let _task_lease = task_lease;
                manager.audit_credentials();
            });
    }

    fn audit_credentials(&self) {
        if self.maintenance.cancel.load(Ordering::Acquire) {
            return;
        }
        self.cleanup_pending_accounts();
        let Ok(_guard) = crate::application::operation_gate::acquire(
            &self.switch_lock,
            &self.maintenance.cancel,
            Duration::from_secs(30),
        ) else {
            return;
        };
        let Ok(_file) = crate::adapters::file_lock::FileLock::acquire_cancellable(
            &self.codex_home.join("auth-switch.lock"),
            &self.maintenance.cancel,
            Duration::from_secs(30),
        ) else {
            return;
        };
        let Ok(account_ids) = self.database.account_ids() else {
            return;
        };
        if self.credentials.check_read_access().is_err() {
            for account_id in account_ids {
                let _ = self
                    .database
                    .set_credential_state(&account_id, CredentialState::Unavailable);
            }
            self.emit_dashboard_changed();
            return;
        }
        if let Err(error) = self.collect_saved_accounts() {
            let _ = crate::adapters::logging::record(
                &self.database,
                None,
                None,
                "account_collection_deferred",
                serde_json::json!({"errorCode":error.code()}),
            );
        }
        let Ok(account_ids) = self.database.account_ids() else {
            return;
        };
        let _ = self.credentials.cleanup_expired_generations();
        for account_id in account_ids {
            if self.maintenance.cancel.load(Ordering::Acquire) {
                break;
            }
            let state = self.audit_credential(&account_id);
            let _ = self.database.set_credential_state(&account_id, state);
        }
        self.emit_dashboard_changed();
    }

    pub(super) fn audit_credential(&self, account_id: &str) -> CredentialState {
        if self.credentials.check_read_access().is_err() {
            return CredentialState::Unavailable;
        }
        let references = match self.credentials.generation_refs(account_id) {
            Ok(references) => references,
            Err(_) => return CredentialState::Unavailable,
        };
        if references.is_empty() {
            return CredentialState::Missing;
        }
        let generations = match self.database.credential_generations(account_id) {
            Ok(generations) => generations,
            Err(_) => return CredentialState::Unavailable,
        };
        let all_quarantined = !generations.is_empty()
            && generations.iter().all(|generation| {
                matches!(generation.state, CredentialGenerationState::Quarantined)
            });
        if all_quarantined {
            return CredentialState::Quarantined;
        }
        match self.credentials.load_optional(account_id) {
            Ok(Some(_)) => CredentialState::Ready,
            Ok(None) => CredentialState::Missing,
            Err(_) => CredentialState::Unavailable,
        }
    }

    pub(super) fn start_credential_maintenance(&self) {
        let Ok(task_lease) = self.tasks.enter() else {
            return;
        };
        if self.maintenance.started.swap(true, Ordering::AcqRel) {
            return;
        }
        let manager = self.clone();
        let spawn = thread::Builder::new()
            .name("credential-maintenance".to_owned())
            .spawn(move || {
                let _task_lease = task_lease;
                manager.run_credential_maintenance();
            });
        if spawn.is_err() {
            self.maintenance.started.store(false, Ordering::Release);
        }
    }

    fn run_credential_maintenance(&self) {
        let mut schedule = crate::application::usage_schedule::UsageSchedule::default();
        let mut last_audit = std::time::Instant::now();
        let mut last_cleanup = std::time::Instant::now();
        loop {
            if self
                .maintenance
                .cleanup_requested
                .swap(false, Ordering::AcqRel)
            {
                self.cleanup_pending_accounts();
                self.cleanup_application_sessions();
                self.cleanup_execution_outputs();
                self.recover_application_credentials();
                self.cleanup_application_history();
                last_cleanup = std::time::Instant::now();
            }
            if last_cleanup.elapsed() >= Duration::from_secs(300) {
                self.cleanup_pending_accounts();
                self.cleanup_application_sessions();
                self.cleanup_execution_outputs();
                self.recover_application_credentials();
                self.cleanup_application_history();
                last_cleanup = std::time::Instant::now();
            }
            if last_audit.elapsed() >= MAINTENANCE_INTERVAL {
                self.audit_credentials();
                last_audit = std::time::Instant::now();
            }
            if self.refresh_due_usage(&mut schedule) {
                continue;
            }
            let mut stopped = self.maintenance.stopped.lock();
            if *stopped {
                self.usage_reader.close();
                return;
            }
            self.maintenance
                .wake
                .wait_for(&mut stopped, Duration::from_secs(1));
            if *stopped {
                self.usage_reader.close();
                return;
            }
            drop(stopped);
        }
    }

    pub(super) fn restore_credential(
        &self,
        account_id: &str,
        previous: Option<&serde_json::Value>,
    ) -> Result<(), AppError> {
        if let Some(document) = previous {
            self.credentials.save(document)?;
        } else {
            self.credentials.delete(account_id)?;
        }
        Ok(())
    }
}

use super::ManagerCore;
use crate::adapters::{file_lock::FileLock, logging};
use crate::domain::{RuntimeConsent, RuntimeRecoveryReport, SharedRuntimeSnapshot};
use crate::error::AppError;
use serde_json::json;
use std::time::Duration;

impl ManagerCore {
    pub fn shared_runtime_snapshot(&self) -> SharedRuntimeSnapshot {
        self.shared_runtime.snapshot(&self.maintenance.cancel)
    }

    pub fn recover_chat(
        &self,
        thread: &str,
        consent: Option<RuntimeConsent>,
    ) -> Result<RuntimeRecoveryReport, AppError> {
        let _lease = self.tasks.enter()?;
        let id = chat_id(thread)?;
        let _guard = crate::application::operation_gate::acquire(
            &self.switch_lock,
            &self.maintenance.cancel,
            Duration::from_secs(30),
        )?;
        let _file = FileLock::acquire_cancellable(
            &self.codex_home.join("auth-switch.lock"),
            &self.maintenance.cancel,
            Duration::from_secs(30),
        )?;
        let operation_id = uuid::Uuid::new_v4().to_string();
        let result =
            self.shared_runtime
                .recover_thread(&id, consent.as_ref(), &self.maintenance.cancel);
        let _ = logging::record(
            &self.database,
            Some(&operation_id),
            None,
            "runtime_chat_recovery",
            json!({
                "recovered": result.as_ref().is_ok_and(|report| report.recovered),
                "interruptionAuthorized": consent.is_some(),
                "errorCode": result.as_ref().err().map(AppError::code),
                "affectedThreads": result.as_ref().ok().map(|report| report.affected_threads),
            }),
        );
        let mut report = result?;
        if report.recovered && open_original(&id).is_err() {
            report.explanation = "O proprietário anterior foi encerrado, mas o sistema não abriu o Codex. Use o atalho Abrir chat original.".to_owned();
        }
        Ok(report)
    }

    pub fn open_original_chat(&self, thread: &str) -> Result<(), AppError> {
        open_original(&chat_id(thread)?)
    }
}

fn open_original(id: &str) -> Result<(), AppError> {
    open::that_detached(format!("codex://threads/{id}"))?;
    Ok(())
}

fn chat_id(input: &str) -> Result<String, AppError> {
    let trimmed = input.trim();
    let id = trimmed.strip_prefix("codex://threads/").unwrap_or(trimmed);
    uuid::Uuid::parse_str(id)
        .map(|id| id.to_string())
        .map_err(|_| {
            AppError::Validation(
                "Cole o endereço codex://threads/UUID ou o UUID do chat.".to_owned(),
            )
        })
}

#[cfg(test)]
mod tests {
    #[test]
    fn accepts_only_original_codex_chat_addresses() {
        let id = "01a0dabe-0cc8-7aa0-a327-dd24d42869a3";
        assert_eq!(
            super::chat_id(&format!("codex://threads/{id}")).expect("id"),
            id
        );
        assert!(super::chat_id("https://example.com").is_err());
        assert!(super::chat_id(&format!("codex://threads/{id}?command=delete")).is_err());
    }
}

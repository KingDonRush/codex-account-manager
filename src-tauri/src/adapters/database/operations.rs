use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use uuid::Uuid;

use super::rows::operation_from_row;
use super::Database;
use crate::domain::{LoginOperation, OperationKind, OperationState};
use crate::error::AppError;

impl Database {
    #[cfg(test)]
    pub fn create_operation(
        &self,
        profile_id: &str,
        reconnect: bool,
    ) -> Result<LoginOperation, AppError> {
        let kind = if reconnect {
            OperationKind::Reconnect
        } else {
            OperationKind::Login
        };
        self.create_typed_operation(profile_id, kind)
    }

    pub fn create_switch_operation(&self, profile_id: &str) -> Result<LoginOperation, AppError> {
        self.create_typed_operation(profile_id, OperationKind::Switch)
    }

    pub fn create_typed_operation(
        &self,
        profile_id: &str,
        kind: OperationKind,
    ) -> Result<LoginOperation, AppError> {
        self.ensure_profile(profile_id)?;
        let started_at = Utc::now();
        let operation = LoginOperation {
            id: format!("{}-{}", kind.as_str(), Uuid::new_v4()),
            profile_id: profile_id.to_owned(),
            kind,
            reconnect: matches!(kind, OperationKind::Reconnect),
            state: OperationState::Idle,
            step: "Aguardando início".to_owned(),
            started_at,
            stage_started_at: started_at,
            last_stage_duration_ms: None,
            finished_at: None,
            error_code: None,
            error_message: None,
            browser_pid: None,
            runtime_kind: None,
            duration_ms: None,
            cancelable: true,
        };
        self.save_operation(&operation)?;
        Ok(operation)
    }

    pub fn save_operation(&self, operation: &LoginOperation) -> Result<(), AppError> {
        let connection = self.connection()?;
        connection.execute(
            "INSERT INTO operations
             (id, profile_id, kind, reconnect, state, step, started_at, finished_at, error_code,
              error_message, browser_pid, runtime_kind, stage_started_at, last_stage_duration_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT(id) DO UPDATE SET profile_id = excluded.profile_id,
                 kind = excluded.kind, reconnect = excluded.reconnect, state = excluded.state,
                 step = excluded.step,
                 started_at = excluded.started_at, finished_at = excluded.finished_at,
                 error_code = excluded.error_code, error_message = excluded.error_message,
                 browser_pid = excluded.browser_pid, runtime_kind = excluded.runtime_kind,
                 stage_started_at = excluded.stage_started_at,
                 last_stage_duration_ms = excluded.last_stage_duration_ms",
            params![
                operation.id,
                operation.profile_id,
                operation.kind.as_str(),
                operation.reconnect,
                operation.state.as_str(),
                operation.step,
                operation.started_at.to_rfc3339(),
                operation.finished_at.map(|value| value.to_rfc3339()),
                operation.error_code,
                operation.error_message,
                operation.browser_pid.map(i64::from),
                operation.runtime_kind,
                operation.stage_started_at.to_rfc3339(),
                operation.last_stage_duration_ms,
            ],
        )?;
        Ok(())
    }

    pub fn operation(&self, operation_id: &str) -> Result<LoginOperation, AppError> {
        let connection = self.connection()?;
        connection
            .query_row(
                "SELECT id, profile_id, reconnect, state, step, started_at, finished_at,
                        error_code, error_message, browser_pid, runtime_kind, kind,
                        stage_started_at, last_stage_duration_ms
                 FROM operations WHERE id = ?1",
                [operation_id],
                operation_from_row,
            )
            .optional()?
            .ok_or_else(|| AppError::OperationNotFound(operation_id.to_owned()))
    }

    pub fn list_operations(&self) -> Result<Vec<LoginOperation>, AppError> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT id, profile_id, reconnect, state, step, started_at, finished_at,
                    error_code, error_message, browser_pid, runtime_kind, kind,
                    stage_started_at, last_stage_duration_ms
             FROM operations ORDER BY started_at DESC LIMIT 100",
        )?;
        let rows = statement.query_map([], operation_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
    }

    pub fn record_operation_browser(
        &self,
        operation_id: &str,
        browser_pid: u32,
        runtime_kind: &str,
    ) -> Result<(), AppError> {
        let connection = self.connection()?;
        let changed = connection.execute(
            "UPDATE operations SET browser_pid = ?1, runtime_kind = ?2 WHERE id = ?3",
            params![i64::from(browser_pid), runtime_kind, operation_id],
        )?;
        if changed == 0 {
            return Err(AppError::OperationNotFound(operation_id.to_owned()));
        }
        Ok(())
    }

    pub fn cancel_interrupted_operations(&self) -> Result<Vec<(String, String)>, AppError> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT id, profile_id FROM operations
             WHERE state NOT IN ('completed', 'cancelled', 'failed', 'timed_out')",
        )?;
        let interrupted = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        if interrupted.is_empty() {
            return Ok(interrupted);
        }
        connection.execute(
            "UPDATE operations SET state = 'cancelled', step = ?1, finished_at = ?2,
                    error_code = 'app_closed', error_message = ?1
             WHERE state NOT IN ('completed', 'cancelled', 'failed', 'timed_out')",
            params![
                "Cancelado porque o aplicativo foi encerrado",
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(interrupted)
    }
}

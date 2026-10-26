use chrono::Utc;
use rusqlite::params;

use super::Database;
use crate::domain::OperationStage;
use crate::error::AppError;

const TRIM_INTERVAL: i64 = 100;
const MAX_EVENTS: i64 = 2_000;
const RETENTION_DAYS: i64 = 30;

impl Database {
    pub fn save_diagnostic(
        &self,
        operation_id: Option<&str>,
        profile_id: Option<&str>,
        event_name: &str,
        payload: &serde_json::Value,
    ) -> Result<(), AppError> {
        let connection = self.connection()?;
        connection.execute(
            "INSERT INTO diagnostic_events
             (operation_id, profile_id, event_name, payload_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                operation_id,
                profile_id,
                event_name,
                serde_json::to_string(payload)?,
                Utc::now().to_rfc3339()
            ],
        )?;
        let should_trim = connection.last_insert_rowid() % TRIM_INTERVAL == 0;
        drop(connection);
        if should_trim {
            self.trim_diagnostics()?;
        }
        Ok(())
    }

    pub(super) fn trim_diagnostics(&self) -> Result<(), AppError> {
        let connection = self.connection()?;
        connection.execute(
            "DELETE FROM diagnostic_events WHERE id NOT IN
             (SELECT id FROM diagnostic_events ORDER BY id DESC LIMIT ?1)",
            [MAX_EVENTS],
        )?;
        let cutoff = (Utc::now() - chrono::Duration::days(RETENTION_DAYS)).to_rfc3339();
        connection.execute(
            "DELETE FROM diagnostic_events WHERE created_at < ?1",
            [cutoff],
        )?;
        Ok(())
    }

    pub fn diagnostic_export(&self) -> Result<Vec<serde_json::Value>, AppError> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT operation_id, profile_id, event_name, payload_json, created_at
             FROM diagnostic_events ORDER BY id ASC",
        )?;
        let rows = statement.query_map([], diagnostic_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
    }

    pub fn operation_stages(&self, operation_id: &str) -> Result<Vec<OperationStage>, AppError> {
        self.operation(operation_id)?;
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT payload_json FROM diagnostic_events
             WHERE operation_id = ?1 AND event_name IN ('login_stage', 'switch_stage')
             ORDER BY id ASC LIMIT 128",
        )?;
        let payloads = statement
            .query_map([operation_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(payloads
            .into_iter()
            .filter_map(|payload| parse_operation_stage(&payload))
            .collect())
    }
}

fn parse_operation_stage(payload: &str) -> Option<OperationStage> {
    let payload = serde_json::from_str::<serde_json::Value>(payload).ok()?;
    let name = payload.get("stage")?.as_str()?.trim();
    if name.is_empty() || name.len() > 80 {
        return None;
    }
    Some(OperationStage {
        name: name.to_owned(),
        duration_ms: payload.get("durationMs")?.as_u64()?,
    })
}

fn diagnostic_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<serde_json::Value> {
    let payload: String = row.get(3)?;
    let payload = serde_json::from_str::<serde_json::Value>(&payload).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(serde_json::json!({
        "operationId": row.get::<_, Option<String>>(0)?,
        "profileId": row.get::<_, Option<String>>(1)?,
        "event": row.get::<_, String>(2)?,
        "payload": payload,
        "createdAt": row.get::<_, String>(4)?,
    }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::Database;

    #[test]
    fn operation_timeline_contains_only_bounded_typed_stages() {
        let fixture = tempfile::tempdir().expect("fixture");
        let database = Database::open(fixture.path()).expect("database");
        let profile = database
            .create_profile("Timeline", None, None)
            .expect("profile");
        let operation = database
            .create_switch_operation(&profile.id)
            .expect("operation");
        database
            .save_diagnostic(
                Some(&operation.id),
                Some(&profile.id),
                "switch_stage",
                &json!({ "stage": "desktop_graceful_wait", "durationMs": 420 }),
            )
            .expect("stage");
        database
            .save_diagnostic(
                Some(&operation.id),
                Some(&profile.id),
                "account_switched",
                &json!({ "stage": "must_not_leak", "durationMs": 1 }),
            )
            .expect("unrelated event");

        let stages = database.operation_stages(&operation.id).expect("timeline");

        assert_eq!(stages.len(), 1);
        assert_eq!(stages[0].name, "desktop_graceful_wait");
        assert_eq!(stages[0].duration_ms, 420);
    }
}

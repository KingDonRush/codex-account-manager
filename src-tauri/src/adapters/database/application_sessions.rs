use super::Database;
use crate::error::AppError;
use rusqlite::{params, OptionalExtension};

impl Database {
    pub fn application_session_owner(
        &self,
        session: &str,
    ) -> Result<Option<(String, String)>, AppError> {
        Ok(self.connection()?.query_row("SELECT s.account_id,i.remote_id FROM application_sessions s JOIN integration_accounts i ON i.id=s.account_id WHERE s.session_id=?1", [session], |row| Ok((row.get(0)?, row.get(1)?))).optional()?)
    }
    pub fn pending_application_session_cleanup(&self) -> Result<Vec<(String, String)>, AppError> {
        let connection = self.connection()?;
        let mut query=connection.prepare("SELECT key,value FROM schema_meta WHERE key LIKE 'pending-application-session-cleanup:%'")?;
        let rows = query
            .query_map([], |row| {
                let key: String = row.get(0)?;
                Ok((
                    key.trim_start_matches("pending-application-session-cleanup:")
                        .to_owned(),
                    row.get(1)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
    pub fn application_session_referenced(&self, id: &str) -> Result<bool, AppError> {
        Ok(self.connection()?.query_row(
            "SELECT EXISTS(SELECT 1 FROM application_sessions WHERE session_id=?1)",
            [id],
            |row| row.get(0),
        )?)
    }
    pub fn finish_application_session_cleanup(&self, id: &str) -> Result<(), AppError> {
        self.connection()?.execute(
            "DELETE FROM schema_meta WHERE key=?1",
            [format!("pending-application-session-cleanup:{id}")],
        )?;
        Ok(())
    }
    pub(super) fn migrate_application_sessions(
        &self,
        connection: &rusqlite::Connection,
    ) -> Result<(), AppError> {
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS application_sessions (
            account_id TEXT PRIMARY KEY REFERENCES integration_accounts(id) ON DELETE CASCADE,
            session_id TEXT NOT NULL UNIQUE, updated_at TEXT NOT NULL);",
        )?;
        Ok(())
    }

    pub fn application_session(&self, account_id: &str) -> Result<Option<String>, AppError> {
        Ok(self
            .connection()?
            .query_row(
                "SELECT session_id FROM application_sessions WHERE account_id=?1",
                [account_id],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn save_application_session(
        &self,
        account_id: &str,
        session_id: &str,
    ) -> Result<(), AppError> {
        self.connection()?.execute("INSERT INTO application_sessions(account_id,session_id,updated_at) VALUES(?1,?2,?3)
            ON CONFLICT(account_id) DO UPDATE SET session_id=excluded.session_id,updated_at=excluded.updated_at",
            params![account_id,session_id,chrono::Utc::now().to_rfc3339()])?;
        Ok(())
    }
}

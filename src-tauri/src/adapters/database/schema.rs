use rusqlite::{params, Connection};

use super::Database;
use crate::error::AppError;

impl Database {
    pub(super) fn migrate_schema(&self, connection: &Connection) -> Result<(), AppError> {
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_meta (
                key TEXT PRIMARY KEY NOT NULL,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS profiles (
                id TEXT PRIMARY KEY NOT NULL,
                name TEXT NOT NULL,
                account_id TEXT,
                email TEXT,
                mail_provider_id TEXT,
                mail_url TEXT,
                data_dir TEXT NOT NULL,
                legacy_browser_name TEXT,
                legacy_profile_dir TEXT,
                status TEXT NOT NULL,
                last_used_at TEXT,
                last_browser_opened_at TEXT,
                last_error TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS accounts (
                id TEXT PRIMARY KEY NOT NULL,
                email TEXT,
                plan_type TEXT,
                credential_ref TEXT,
                credential_state TEXT NOT NULL DEFAULT 'not_connected',
                rate_limits_json TEXT,
                quota_observed_at TEXT,
                last_activated_at TEXT,
                last_disconnected_at TEXT,
                last_browser_authenticated_at TEXT,
                credential_checked_at TEXT,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS mail_providers (
                id TEXT PRIMARY KEY NOT NULL,
                display_name TEXT NOT NULL,
                domains_json TEXT NOT NULL,
                default_webmail_url TEXT NOT NULL,
                aliases_json TEXT NOT NULL,
                is_custom INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS credential_generations (
                account_id TEXT NOT NULL,
                generation_id TEXT NOT NULL,
                fingerprint TEXT NOT NULL,
                state TEXT NOT NULL,
                created_at TEXT NOT NULL,
                quarantined_at TEXT,
                delete_after TEXT,
                is_active INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(account_id, generation_id)
            );
            CREATE TABLE IF NOT EXISTS operations (
                id TEXT PRIMARY KEY NOT NULL,
                profile_id TEXT NOT NULL,
                kind TEXT NOT NULL DEFAULT 'login',
                reconnect INTEGER NOT NULL,
                state TEXT NOT NULL,
                step TEXT NOT NULL,
                started_at TEXT NOT NULL,
                stage_started_at TEXT,
                last_stage_duration_ms INTEGER,
                finished_at TEXT,
                error_code TEXT,
                error_message TEXT,
                browser_pid INTEGER,
                runtime_kind TEXT,
                FOREIGN KEY(profile_id) REFERENCES profiles(id)
            );
            CREATE TABLE IF NOT EXISTS diagnostic_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                operation_id TEXT,
                profile_id TEXT,
                event_name TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS operations_started_idx ON operations(started_at DESC);
            CREATE INDEX IF NOT EXISTS diagnostics_created_idx ON diagnostic_events(created_at DESC);",
        )?;
        Self::ensure_accounts_rate_limits_column(connection)?;
        Self::ensure_profile_legacy_columns(connection)?;
        Self::ensure_account_lifecycle_columns(connection)?;
        Self::ensure_profile_lifecycle_columns(connection)?;
        Self::ensure_operation_diagnostic_columns(connection)?;
        Self::ensure_operation_kind_column(connection)?;
        Self::ensure_unique_account_profile(connection)?;
        connection.execute(
            "CREATE INDEX IF NOT EXISTS credential_generations_state_idx
             ON credential_generations(account_id, state, created_at DESC)",
            [],
        )?;
        Ok(())
    }

    pub(super) fn seed_providers(&self) -> Result<(), AppError> {
        let connection = self.connection()?;
        for provider in crate::adapters::mail_catalog::built_in_providers() {
            if let Some(old_url) = previous_default_url(&provider.id) {
                connection.execute(
                    "UPDATE mail_providers SET default_webmail_url = ?1
                     WHERE id = ?2 AND (default_webmail_url = ?3 OR default_webmail_url = ?3 || '/')",
                    params![provider.default_webmail_url, provider.id, old_url],
                )?;
            }
            connection.execute(
                "INSERT OR IGNORE INTO mail_providers
                 (id, display_name, domains_json, default_webmail_url, aliases_json, is_custom)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0)",
                params![
                    provider.id,
                    provider.display_name,
                    serde_json::to_string(&provider.domains)?,
                    provider.default_webmail_url,
                    serde_json::to_string(&provider.aliases)?,
                ],
            )?;
        }
        Ok(())
    }

    fn ensure_unique_account_profile(connection: &Connection) -> Result<(), AppError> {
        connection.execute(
            "CREATE UNIQUE INDEX IF NOT EXISTS profiles_one_account_idx
             ON profiles(account_id)
             WHERE account_id IS NOT NULL AND status != 'deleted'",
            [],
        )?;
        Ok(())
    }

    fn ensure_accounts_rate_limits_column(connection: &Connection) -> Result<(), AppError> {
        if !table_columns(connection, "accounts")?
            .iter()
            .any(|column| column == "rate_limits_json")
        {
            connection.execute("ALTER TABLE accounts ADD COLUMN rate_limits_json TEXT", [])?;
        }
        Ok(())
    }

    fn ensure_profile_legacy_columns(connection: &Connection) -> Result<(), AppError> {
        Self::ensure_columns(
            connection,
            "profiles",
            &[
                ("legacy_browser_name", "TEXT"),
                ("legacy_profile_dir", "TEXT"),
            ],
        )
    }

    fn ensure_account_lifecycle_columns(connection: &Connection) -> Result<(), AppError> {
        Self::ensure_columns(
            connection,
            "accounts",
            &[
                ("credential_state", "TEXT NOT NULL DEFAULT 'not_connected'"),
                ("last_activated_at", "TEXT"),
                ("last_disconnected_at", "TEXT"),
                ("last_browser_authenticated_at", "TEXT"),
                ("credential_checked_at", "TEXT"),
                ("quota_observed_at", "TEXT"),
            ],
        )
    }

    fn ensure_profile_lifecycle_columns(connection: &Connection) -> Result<(), AppError> {
        Self::ensure_columns(
            connection,
            "profiles",
            &[("last_browser_opened_at", "TEXT")],
        )
    }

    fn ensure_operation_diagnostic_columns(connection: &Connection) -> Result<(), AppError> {
        Self::ensure_columns(
            connection,
            "operations",
            &[
                ("browser_pid", "INTEGER"),
                ("runtime_kind", "TEXT"),
                ("stage_started_at", "TEXT"),
                ("last_stage_duration_ms", "INTEGER"),
            ],
        )?;
        connection.execute(
            "UPDATE operations SET stage_started_at = started_at
             WHERE stage_started_at IS NULL",
            [],
        )?;
        Ok(())
    }

    fn ensure_operation_kind_column(connection: &Connection) -> Result<(), AppError> {
        if table_columns(connection, "operations")?
            .iter()
            .any(|column| column == "kind")
        {
            return Ok(());
        }
        connection.execute(
            "ALTER TABLE operations ADD COLUMN kind TEXT NOT NULL DEFAULT 'login'",
            [],
        )?;
        connection.execute(
            "UPDATE operations SET kind = CASE WHEN reconnect = 1 THEN 'reconnect' ELSE 'login' END",
            [],
        )?;
        Ok(())
    }

    fn ensure_columns(
        connection: &Connection,
        table: &str,
        columns: &[(&str, &str)],
    ) -> Result<(), AppError> {
        let existing = table_columns(connection, table)?;
        for (column, kind) in columns {
            if existing.iter().any(|value| value == column) {
                continue;
            }
            // Every table, column and type passed here is a source-code
            // constant. Values from users never enter this DDL statement.
            connection.execute(
                &format!("ALTER TABLE {table} ADD COLUMN {column} {kind}"),
                [],
            )?;
        }
        Ok(())
    }
}

fn previous_default_url(id: &str) -> Option<&'static str> {
    match id {
        "gmail" => Some("https://mail.google.com"),
        "yahoo" => Some("https://mail.yahoo.com"),
        "outlook" | "hotmail" => Some("https://outlook.live.com/mail"),
        "proton" => Some("https://mail.proton.me"),
        "mailbox.org" => Some("https://mailbox.org"),
        _ => None,
    }
}

fn table_columns(connection: &rusqlite::Connection, table: &str) -> Result<Vec<String>, AppError> {
    // `table` is selected only by the hardcoded migration functions above.
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(columns)
}

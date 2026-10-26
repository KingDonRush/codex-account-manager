use rusqlite::{params, OptionalExtension};

use super::rows::provider_from_row;
use super::Database;
use crate::domain::MailProvider;
use crate::error::AppError;

impl Database {
    pub fn list_providers(&self) -> Result<Vec<MailProvider>, AppError> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT id, display_name, domains_json, default_webmail_url, aliases_json, is_custom
             FROM mail_providers ORDER BY is_custom ASC, display_name COLLATE NOCASE ASC",
        )?;
        let rows = statement.query_map([], provider_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
    }

    pub fn provider(&self, provider_id: &str) -> Result<Option<MailProvider>, AppError> {
        let connection = self.connection()?;
        connection
            .query_row(
                "SELECT id, display_name, domains_json, default_webmail_url, aliases_json, is_custom
                 FROM mail_providers WHERE id = ?1",
                [provider_id],
                provider_from_row,
            )
            .optional()
            .map_err(AppError::from)
    }

    pub fn save_provider(&self, provider: &MailProvider) -> Result<MailProvider, AppError> {
        let connection = self.connection()?;
        connection.execute(
            "INSERT INTO mail_providers
             (id, display_name, domains_json, default_webmail_url, aliases_json, is_custom)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET display_name = excluded.display_name,
                 domains_json = excluded.domains_json,
                 default_webmail_url = excluded.default_webmail_url,
                 aliases_json = excluded.aliases_json,
                 is_custom = mail_providers.is_custom",
            params![
                provider.id,
                provider.display_name,
                serde_json::to_string(&provider.domains)?,
                provider.default_webmail_url,
                serde_json::to_string(&provider.aliases)?,
                provider.is_custom,
            ],
        )?;
        drop(connection);
        self.provider(&provider.id)?
            .ok_or_else(|| AppError::Internal("o provedor salvo não pôde ser relido".to_owned()))
    }
}

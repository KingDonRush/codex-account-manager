use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension};

use super::rows::credential_generation_from_row;
use super::Database;
use crate::domain::{CredentialGeneration, CredentialState};
use crate::error::AppError;
use crate::ports::StoredCredentialGeneration;

const QUARANTINE_DAYS: i64 = 7;

impl Database {
    pub fn active_credential_signature(&self, account: &str) -> Result<Option<String>, AppError> {
        let value: Option<(String, String)> = self.connection()?.query_row(
            "SELECT generation_id,fingerprint FROM credential_generations WHERE account_id=?1 AND is_active=1 AND state='eligible' LIMIT 1",
            [account], |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?;
        Ok(value.map(|(id, fingerprint)| format!("{id}:{fingerprint}")))
    }
    pub fn set_credential_state(
        &self,
        account_id: &str,
        state: CredentialState,
    ) -> Result<(), AppError> {
        let timestamp = Utc::now().to_rfc3339();
        let connection = self.connection()?;
        connection.execute(
            "UPDATE accounts SET credential_state = ?1, credential_checked_at = ?2,
                    updated_at = ?2 WHERE id = ?3",
            params![state.as_str(), timestamp, account_id],
        )?;
        Ok(())
    }

    pub fn account_ids(&self) -> Result<Vec<String>, AppError> {
        let connection = self.connection()?;
        let mut statement = connection.prepare("SELECT id FROM accounts ORDER BY id")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
    }

    pub fn upsert_credential_generation(
        &self,
        generation: &StoredCredentialGeneration,
        active: bool,
    ) -> Result<(), AppError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        if active {
            transaction.execute(
                "UPDATE credential_generations SET is_active = 0 WHERE account_id = ?1",
                [&generation.account_id],
            )?;
        }
        transaction.execute(
            "INSERT INTO credential_generations
             (account_id, generation_id, fingerprint, state, created_at, quarantined_at,
              delete_after, is_active)
             VALUES (?1, ?2, ?3, 'eligible', ?4, NULL, NULL, ?5)
             ON CONFLICT(account_id, generation_id) DO UPDATE SET
                 fingerprint = excluded.fingerprint,
                 created_at = MIN(credential_generations.created_at, excluded.created_at),
                 state = credential_generations.state,
                 quarantined_at = credential_generations.quarantined_at,
                 delete_after = credential_generations.delete_after,
                 is_active = CASE WHEN excluded.is_active = 1 THEN 1 ELSE credential_generations.is_active END",
            params![
                generation.account_id,
                generation.generation_id,
                generation.fingerprint,
                generation.created_at.to_rfc3339(),
                active,
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn credential_generations(
        &self,
        account_id: &str,
    ) -> Result<Vec<CredentialGeneration>, AppError> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT account_id, generation_id, fingerprint, state, created_at,
                    quarantined_at, delete_after, is_active
             FROM credential_generations WHERE account_id = ?1
             ORDER BY CASE state WHEN 'eligible' THEN 0 ELSE 1 END,
                      is_active DESC, created_at DESC",
        )?;
        let rows = statement.query_map([account_id], credential_generation_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
    }

    pub fn activate_credential_generation(
        &self,
        account_id: &str,
        generation_id: &str,
    ) -> Result<(), AppError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE credential_generations SET is_active = 0 WHERE account_id = ?1",
            [account_id],
        )?;
        let changed = transaction.execute(
            "UPDATE credential_generations SET is_active = 1, state = 'eligible',
                    quarantined_at = NULL, delete_after = NULL
             WHERE account_id = ?1 AND generation_id = ?2",
            params![account_id, generation_id],
        )?;
        if changed == 0 {
            return Err(AppError::Keyring(
                "metadados da geração de credencial não foram encontrados".to_owned(),
            ));
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn quarantine_credential_generation(
        &self,
        account_id: &str,
        generation_id: &str,
        now: DateTime<Utc>,
    ) -> Result<(), AppError> {
        let connection = self.connection()?;
        connection.execute(
            "UPDATE credential_generations SET state = 'quarantined', is_active = 0,
                    quarantined_at = ?1, delete_after = ?2
             WHERE account_id = ?3 AND generation_id = ?4",
            params![
                now.to_rfc3339(),
                (now + chrono::Duration::days(QUARANTINE_DAYS)).to_rfc3339(),
                account_id,
                generation_id,
            ],
        )?;
        Ok(())
    }

    pub fn remove_credential_generation_metadata(
        &self,
        account_id: &str,
        generation_id: &str,
    ) -> Result<(), AppError> {
        let connection = self.connection()?;
        connection.execute(
            "DELETE FROM credential_generations WHERE account_id = ?1 AND generation_id = ?2",
            params![account_id, generation_id],
        )?;
        Ok(())
    }

    pub fn remove_account_credential_metadata(&self, account_id: &str) -> Result<(), AppError> {
        let connection = self.connection()?;
        connection.execute(
            "DELETE FROM credential_generations WHERE account_id = ?1",
            [account_id],
        )?;
        Ok(())
    }

    pub fn expired_credential_generations(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Vec<(String, String)>, AppError> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT account_id, generation_id FROM credential_generations
             WHERE state = 'quarantined' AND is_active=0 AND delete_after IS NOT NULL AND delete_after <= ?1",
        )?;
        let rows = statement.query_map([now.to_rfc3339()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
    }

    pub fn delete_account_metadata(&self, account_id: &str) -> Result<(), AppError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "DELETE FROM credential_generations WHERE account_id = ?1",
            [account_id],
        )?;
        transaction.execute("DELETE FROM accounts WHERE id = ?1", [account_id])?;
        transaction.commit()?;
        Ok(())
    }
}

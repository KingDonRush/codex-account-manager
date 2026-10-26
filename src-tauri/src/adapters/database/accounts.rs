use chrono::Utc;
use rusqlite::params;

use super::Database;
use crate::domain::{AccountMetadata, ManagedProfile};
use crate::error::AppError;

pub struct SwitchCommit<'a> {
    pub provider_id: Option<&'a str>,
    pub previous_account_id: Option<&'a str>,
    pub transaction_id: Option<&'a str>,
}

impl Database {
    pub fn update_usage(
        &self,
        account_id: &str,
        snapshot: &crate::domain::AccountSnapshot,
    ) -> Result<(), AppError> {
        let Some(limits) = snapshot.rate_limits.as_ref() else {
            return Ok(());
        };
        self.connection()?.execute(
            "UPDATE accounts SET rate_limits_json = ?1, quota_observed_at = ?2,
             plan_type = COALESCE(?3, plan_type), updated_at = ?2 WHERE id = ?4",
            params![
                serde_json::to_string(limits)?,
                Utc::now().to_rfc3339(),
                snapshot.plan_type,
                account_id
            ],
        )?;
        Ok(())
    }

    pub fn persist_connected_account(
        &self,
        profile_id: &str,
        account: AccountMetadata<'_>,
        provider_id: Option<&str>,
    ) -> Result<ManagedProfile, AppError> {
        let previous_account = self.profile(profile_id)?.account_id;
        let rate_limits_json = account.rate_limits.map(serde_json::to_string).transpose()?;
        let quota_observed_at = account
            .rate_limits
            .map(|_| account.observed_at.to_rfc3339());
        let now = Utc::now();
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "DELETE FROM schema_meta WHERE key='removed-account:'||?1",
            [account.account_id],
        )?;
        transaction.execute(
            "INSERT INTO accounts
             (id, email, plan_type, credential_ref, credential_state, rate_limits_json,
              quota_observed_at, last_browser_authenticated_at, credential_checked_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 'ready', ?5, ?6, ?7, ?7, ?7)
             ON CONFLICT(id) DO UPDATE SET
                 email = COALESCE(excluded.email, accounts.email),
                 plan_type = COALESCE(excluded.plan_type, accounts.plan_type),
                 credential_ref = excluded.credential_ref,
                 credential_state = 'ready',
                 rate_limits_json = COALESCE(excluded.rate_limits_json, accounts.rate_limits_json),
                 quota_observed_at = COALESCE(excluded.quota_observed_at, accounts.quota_observed_at),
                 last_browser_authenticated_at = excluded.last_browser_authenticated_at,
                 credential_checked_at = excluded.credential_checked_at,
                 updated_at = excluded.updated_at",
            params![
                account.account_id,
                account.email,
                account.plan_type,
                account.credential_ref,
                rate_limits_json,
                quota_observed_at,
                account.observed_at.to_rfc3339(),
            ],
        )?;
        transaction.execute(
            "UPDATE profiles SET account_id = ?1,
                    email = CASE WHEN account_id IS NOT NULL AND account_id != ?1 THEN ?2 ELSE COALESCE(?2, email) END,
                    mail_url = CASE WHEN account_id IS NOT NULL AND account_id != ?1 THEN NULL ELSE mail_url END,
                    mail_provider_id = CASE WHEN account_id IS NOT NULL AND account_id != ?1 THEN ?3 ELSE COALESCE(?3, mail_provider_id) END,
                    status = 'active', last_error = NULL, last_used_at = ?4, updated_at = ?4
             WHERE id = ?5",
            params![
                account.account_id,
                account.email,
                provider_id,
                now.to_rfc3339(),
                profile_id,
            ],
        )?;
        if let Some(previous) = previous_account.filter(|id| id != account.account_id) {
            super::account_lifecycle::queue_account_cleanup(&transaction, &previous, profile_id)?;
        }
        transaction.commit()?;
        drop(connection);
        self.profile(profile_id)
    }

    #[cfg(test)]
    pub fn persist_switched_account(
        &self,
        profile_id: &str,
        account: AccountMetadata<'_>,
        provider_id: Option<&str>,
        previous_account_id: Option<&str>,
    ) -> Result<ManagedProfile, AppError> {
        self.persist_switch_commit(
            profile_id,
            account,
            SwitchCommit {
                provider_id,
                previous_account_id,
                transaction_id: None,
            },
        )?;
        self.profile(profile_id)
    }

    pub fn persist_switch_commit(
        &self,
        profile_id: &str,
        account: AccountMetadata<'_>,
        commit: SwitchCommit<'_>,
    ) -> Result<(), AppError> {
        self.ensure_profile(profile_id)?;
        let rate_limits_json = account.rate_limits.map(serde_json::to_string).transpose()?;
        let timestamp = account.observed_at.to_rfc3339();
        let quota_observed_at = account.rate_limits.map(|_| timestamp.clone());
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO accounts
             (id, email, plan_type, credential_ref, credential_state, rate_limits_json,
              quota_observed_at, credential_checked_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 'ready', ?5, ?6, ?7, ?7)
             ON CONFLICT(id) DO UPDATE SET
                 email = COALESCE(excluded.email, accounts.email),
                 plan_type = COALESCE(excluded.plan_type, accounts.plan_type),
                 credential_ref = COALESCE(excluded.credential_ref, accounts.credential_ref),
                 credential_state = 'ready',
                 rate_limits_json = COALESCE(excluded.rate_limits_json, accounts.rate_limits_json),
                 quota_observed_at = COALESCE(excluded.quota_observed_at, accounts.quota_observed_at),
                 credential_checked_at = excluded.credential_checked_at,
                 updated_at = excluded.updated_at",
            params![
                account.account_id,
                account.email,
                account.plan_type,
                account.credential_ref,
                rate_limits_json,
                quota_observed_at,
                timestamp,
            ],
        )?;
        transaction.execute(
            "UPDATE profiles SET account_id = ?1, email = COALESCE(?2, email),
                    mail_provider_id = COALESCE(?3, mail_provider_id), status = 'active', last_error = NULL,
                    last_used_at = ?4,
                    updated_at = ?4 WHERE id = ?5",
            params![
                account.account_id,
                account.email,
                commit.provider_id,
                timestamp,
                profile_id,
            ],
        )?;
        if let Some(previous_account_id) = commit
            .previous_account_id
            .filter(|id| *id != account.account_id)
        {
            transaction.execute(
                "UPDATE accounts SET last_disconnected_at = ?1, updated_at = ?1 WHERE id = ?2",
                params![timestamp, previous_account_id],
            )?;
        }
        transaction.execute(
            "UPDATE accounts SET last_activated_at = ?1, credential_state = 'ready',
                    credential_checked_at = ?1, updated_at = ?1 WHERE id = ?2",
            params![timestamp, account.account_id],
        )?;
        if let Some(id) = commit.transaction_id {
            transaction.execute("INSERT INTO schema_meta (key,value) VALUES ('runtime_switch_commit',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [id])?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn switch_was_committed(&self, id: &str) -> Result<bool, AppError> {
        use rusqlite::OptionalExtension;
        let committed: Option<String> = self
            .connection()?
            .query_row(
                "SELECT value FROM schema_meta WHERE key='runtime_switch_commit'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        Ok(committed.as_deref() == Some(id))
    }
}

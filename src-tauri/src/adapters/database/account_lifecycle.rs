use super::Database;
use crate::error::AppError;
use chrono::Utc;
use rusqlite::params;

impl Database {
    pub fn detach_profile(&self, id: &str) -> Result<(), AppError> {
        let profile = self.profile(id)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE profiles SET account_id = NULL, email = NULL, mail_provider_id = NULL,
            mail_url = NULL, last_error = NULL, updated_at = ?1 WHERE id = ?2",
            params![Utc::now().to_rfc3339(), id],
        )?;
        if let Some(account_id) = profile.account_id {
            remove_unreferenced_account(&transaction, &account_id)?;
        }
        transaction.commit()?;
        Ok(())
    }
}

pub(super) fn remove_unreferenced_account(
    transaction: &rusqlite::Transaction<'_>,
    id: &str,
) -> Result<(), AppError> {
    let referenced: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM profiles WHERE account_id = ?1 AND status != 'deleted')",
        [id],
        |row| row.get(0),
    )?;
    if referenced {
        return Ok(());
    }
    connection_tombstone(transaction, id)?;
    transaction.execute(
        "INSERT OR REPLACE INTO schema_meta(key,value)
        SELECT 'pending-application-session-cleanup:'||s.session_id,a.id FROM application_sessions s
        JOIN integration_accounts a ON a.id=s.account_id WHERE a.remote_id=?1",
        [id],
    )?;
    transaction.execute(
        "DELETE FROM credential_generations WHERE account_id = ?1",
        [id],
    )?;
    transaction.execute("DELETE FROM accounts WHERE id = ?1", [id])?;
    Ok(())
}

pub(super) fn queue_account_cleanup(
    connection: &rusqlite::Connection,
    id: &str,
    profile_id: &str,
) -> Result<(), AppError> {
    connection_tombstone(connection, id)?;
    connection.execute("INSERT INTO schema_meta(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", params![format!("pending-account-cleanup:{id}"), profile_id])?;
    Ok(())
}
fn connection_tombstone(connection: &rusqlite::Connection, id: &str) -> Result<(), AppError> {
    connection.execute(
        "INSERT OR REPLACE INTO schema_meta(key,value) VALUES('removed-account:'||?1,?2)",
        params![id, Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

impl Database {
    pub fn pending_account_cleanup(&self) -> Result<Vec<(String, String)>, AppError> {
        let connection = self.connection()?;
        let mut statement = connection.prepare("SELECT key, value FROM schema_meta WHERE key LIKE 'pending-account-cleanup:%' ORDER BY key")?;
        let rows = statement.query_map([], |row| {
            let key: String = row.get(0)?;
            Ok((
                key.trim_start_matches("pending-account-cleanup:")
                    .to_owned(),
                row.get(1)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
    }

    pub fn finish_account_cleanup(&self, id: &str) -> Result<(), AppError> {
        self.connection()?.execute(
            "DELETE FROM schema_meta WHERE key = ?1",
            [format!("pending-account-cleanup:{id}")],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::AccountMetadata;

    #[test]
    fn replacement_keeps_profile_identity_and_does_not_carry_old_account_data() {
        let fixture = tempfile::tempdir().expect("fixture");
        let db = Database::open(fixture.path()).expect("database");
        let profile = db
            .create_profile("B3", Some("yahoo"), Some("https://old.example.test/inbox"))
            .expect("profile");
        for (id, email, provider) in [
            ("old", "old@yahoo.com", "yahoo"),
            ("new", "new@proton.me", "proton"),
        ] {
            db.persist_connected_account(
                &profile.id,
                AccountMetadata {
                    account_id: id,
                    email: Some(email),
                    plan_type: None,
                    credential_ref: None,
                    rate_limits: None,
                    observed_at: Utc::now(),
                },
                Some(provider),
            )
            .expect("association");
        }
        let current = db.profile(&profile.id).expect("profile");
        assert_eq!(current.name, "B3");
        assert_eq!(current.data_dir, profile.data_dir);
        assert_eq!(current.account_id.as_deref(), Some("new"));
        assert_eq!(current.mail_provider_id.as_deref(), Some("proton"));
        assert!(current.mail_url.is_none() && current.rate_limits.is_none());
        assert!(db.profile_for_account("old").expect("lookup").is_none());
    }

    #[test]
    fn unlink_clears_account_fields_but_preserves_browser_and_name() {
        let fixture = tempfile::tempdir().expect("fixture");
        let db = Database::open(fixture.path()).expect("database");
        let profile = db
            .create_profile("B3", Some("yahoo"), Some("https://mail.yahoo.com/"))
            .expect("profile");
        db.persist_connected_account(
            &profile.id,
            AccountMetadata {
                account_id: "old",
                email: Some("old@yahoo.com"),
                plan_type: Some("plus"),
                credential_ref: None,
                rate_limits: None,
                observed_at: Utc::now(),
            },
            Some("yahoo"),
        )
        .expect("association");
        std::fs::write(
            std::path::Path::new(&profile.data_dir).join("fixture-cookie"),
            "fixture",
        )
        .expect("fixture");
        db.detach_profile(&profile.id).expect("detach");
        let detached = db.profile(&profile.id).expect("profile");
        assert_eq!(detached.name, "B3");
        assert!(
            detached.account_id.is_none()
                && detached.mail_url.is_none()
                && detached.plan_type.is_none()
        );
        assert!(std::path::Path::new(&profile.data_dir)
            .join("fixture-cookie")
            .exists());
        assert!(db.profile_for_account("old").expect("lookup").is_none());
    }
}

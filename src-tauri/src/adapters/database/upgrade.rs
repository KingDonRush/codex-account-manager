use super::Database;
use crate::error::AppError;
use rusqlite::{params, TransactionBehavior};

// Format versions, not the executable's semver. Rust 0.3/0.4 databases used 0.
const CURRENT_FORMAT: i64 = 5;
pub struct CollectedAccount<'a> {
    pub remote: &'a str,
    pub email: Option<&'a str>,
    pub plan: Option<&'a str>,
    pub readable: bool,
}
impl Database {
    pub(super) fn upgrade_state(&self) -> Result<(), AppError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous: i64 = transaction.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if previous > CURRENT_FORMAT {
            return Err(AppError::Validation("Estes dados foram gravados por uma versão mais nova. Não serão alterados; use essa versão ou uma atualização compatível.".to_owned()));
        }
        self.migrate_schema(&transaction)?;
        self.migrate_integrations(&transaction)?;
        self.migrate_application_sessions(&transaction)?;
        self.collect_unlisted_accounts(&transaction)?;
        transaction.pragma_update(None, "user_version", CURRENT_FORMAT)?;
        transaction.execute("INSERT INTO schema_meta(key,value) VALUES('rust_state_format',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [CURRENT_FORMAT.to_string()])?;
        transaction.commit()?;
        Ok(())
    }

    fn collect_unlisted_accounts(&self, connection: &rusqlite::Connection) -> Result<(), AppError> {
        // Recover metadata without copying cookies, rewriting tokens, or changing
        // archived/deleted associations. SQLite rollback leaves the original intact.
        connection.execute("INSERT OR IGNORE INTO accounts(id,email,credential_state,updated_at)
            SELECT account_id,email,'not_connected',updated_at FROM profiles WHERE account_id IS NOT NULL AND status<>'deleted'",[])?;
        let mut query=connection.prepare("SELECT a.id,a.email,a.updated_at FROM accounts a WHERE NOT EXISTS(SELECT 1 FROM profiles p WHERE p.account_id=a.id)
            AND NOT EXISTS(SELECT 1 FROM schema_meta m WHERE m.key='pending-account-cleanup:'||a.id OR m.key='removed-account:'||a.id)")?;
        let rows = query
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(query);
        for (remote, email, at) in rows {
            let id = uuid::Uuid::new_v4().to_string();
            let name = email.as_deref().unwrap_or("Conta recuperada");
            let directory = self.profiles_root.join(&id);
            connection.execute("INSERT INTO profiles(id,name,account_id,email,data_dir,status,last_error,created_at,updated_at)
                VALUES(?1,?2,?3,?4,?5,'active','Cadastro recuperado da instalação Rust anterior; a sessão será verificada no cofre.',?6,?6)",params![id,name,remote,email,directory.to_string_lossy(),at])?;
        }
        Ok(())
    }
    pub fn account_was_removed(&self, remote: &str) -> Result<bool, AppError> {
        Ok(self.connection()?.query_row("SELECT EXISTS(SELECT 1 FROM schema_meta WHERE key='removed-account:'||?1 OR key='pending-account-cleanup:'||?1)", [remote],|row|row.get(0))?)
    }

    pub fn collect_coffer_account(&self, input: CollectedAccount<'_>) -> Result<bool, AppError> {
        let CollectedAccount {
            remote,
            email,
            plan,
            readable,
        } = input;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ignored:bool=transaction.query_row("SELECT EXISTS(SELECT 1 FROM profiles WHERE account_id=?1) OR EXISTS(SELECT 1 FROM schema_meta WHERE key='removed-account:'||?1 OR key='pending-account-cleanup:'||?1)",[remote],|row|row.get(0))?;
        if ignored {
            return Ok(false);
        }
        let now = chrono::Utc::now().to_rfc3339();
        transaction.execute("INSERT OR IGNORE INTO accounts(id,email,plan_type,credential_ref,credential_state,updated_at) VALUES(?1,?2,?3,'native-keyring',?4,?5)",params![remote,email,plan,if readable{"ready"}else{"unavailable"},now])?;
        self.collect_unlisted_accounts(&transaction)?;
        transaction.commit()?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests;

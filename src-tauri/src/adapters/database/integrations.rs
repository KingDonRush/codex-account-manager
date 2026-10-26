use super::Database;
#[cfg(test)]
mod tests;
use crate::domain::integrations::{
    AccountGroup, AccountPolicy, Application, AuthorizationGrant, ExecutionBinding,
};
use crate::error::AppError;
use rusqlite::{params, OptionalExtension};
use serde::{de::DeserializeOwned, Serialize};

impl Database {
    pub(super) fn migrate_integrations(
        &self,
        connection: &rusqlite::Connection,
    ) -> Result<(), AppError> {
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS integration_accounts (
                id TEXT NOT NULL UNIQUE, remote_id TEXT PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
                policy TEXT NOT NULL DEFAULT 'desktop_only', capacity INTEGER NOT NULL DEFAULT 1,
                open_mail INTEGER NOT NULL DEFAULT 0 CHECK(open_mail IN (0,1)));
             INSERT OR IGNORE INTO integration_accounts(id,remote_id) SELECT lower(hex(randomblob(16))),id FROM accounts;
             CREATE TRIGGER IF NOT EXISTS integration_account_created AFTER INSERT ON accounts BEGIN
                INSERT OR IGNORE INTO integration_accounts(id,remote_id) VALUES(lower(hex(randomblob(16))),NEW.id); END;
             CREATE TABLE IF NOT EXISTS integration_objects (
                kind TEXT NOT NULL, id TEXT NOT NULL, revision INTEGER NOT NULL, body TEXT NOT NULL,
                PRIMARY KEY(kind,id));
             CREATE UNIQUE INDEX IF NOT EXISTS application_slug_unique
                ON integration_objects(json_extract(body,'$.slug')) WHERE kind='application';
             CREATE TABLE IF NOT EXISTS group_members (
                group_id TEXT NOT NULL, account_id TEXT NOT NULL REFERENCES integration_accounts(id) ON DELETE CASCADE,
                PRIMARY KEY(group_id,account_id));"
        )?;
        Ok(())
    }

    pub fn account_integration_options(
        &self,
        remote_id: &str,
    ) -> Result<(String, AccountPolicy, u32, bool), AppError> {
        let (id, policy, capacity, mail): (String, String, u32, bool) =
            self.connection()?.query_row(
                "SELECT id,policy,capacity,open_mail FROM integration_accounts WHERE remote_id=?1",
                [remote_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;
        Ok((
            id,
            serde_json::from_str(&policy).unwrap_or(match policy.as_str() {
                "applications_only" => AccountPolicy::ApplicationsOnly,
                "shared" => AccountPolicy::Shared,
                _ => AccountPolicy::DesktopOnly,
            }),
            capacity,
            mail,
        ))
    }

    pub fn integration_account_remote(&self, id: &str) -> Result<String, AppError> {
        self.connection()?
            .query_row(
                "SELECT remote_id FROM integration_accounts WHERE id=?1",
                [id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| AppError::AccessDenied("Conta foi desvinculada.".to_owned()))
    }

    pub fn set_account_policy(
        &self,
        id: &str,
        policy: AccountPolicy,
        capacity: u32,
    ) -> Result<(), AppError> {
        if !(1..=32).contains(&capacity) {
            return Err(AppError::Validation(
                "Capacidade deve estar entre 1 e 32.".to_owned(),
            ));
        }
        let value = serde_json::to_string(&policy)?;
        if self.connection()?.execute(
            "UPDATE integration_accounts SET policy=?2,capacity=?3 WHERE id=?1",
            params![id, value, capacity],
        )? != 1
        {
            return Err(AppError::Validation("Conta não encontrada.".to_owned()));
        }
        Ok(())
    }

    pub fn set_mail_preference(&self, remote_id: &str, enabled: bool) -> Result<(), AppError> {
        self.connection()?.execute(
            "UPDATE integration_accounts SET open_mail=?2 WHERE remote_id=?1",
            params![remote_id, enabled],
        )?;
        Ok(())
    }

    pub fn groups(&self) -> Result<Vec<AccountGroup>, AppError> {
        let mut groups: Vec<AccountGroup> = self.integration_objects("group")?;
        let connection = self.connection()?;
        for group in &mut groups {
            group.account_ids = connection
                .prepare(
                    "SELECT account_id FROM group_members WHERE group_id=?1 ORDER BY account_id",
                )?
                .query_map([&group.id], |row| row.get::<_, String>(0))?
                .collect::<Result<_, _>>()?;
        }
        Ok(groups)
    }

    pub fn save_group(&self, input: &AccountGroup) -> Result<AccountGroup, AppError> {
        validate_name(&input.name)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        let mut next = input.clone();
        next.revision = next_revision(input.revision)?;
        persist_object(&transaction, "group", &next.id, input.revision, &next)?;
        transaction.execute("DELETE FROM group_members WHERE group_id=?1", [&next.id])?;
        for id in &next.account_ids {
            transaction.execute(
                "INSERT INTO group_members(group_id,account_id) VALUES(?1,?2)",
                params![next.id, id],
            )?;
        }
        transaction.commit()?;
        Ok(next)
    }

    /// Check authority and commit membership/grant derivation in one SQLite transaction.
    pub fn save_authorized_group(
        &self,
        grant_id: &str,
        input: &AccountGroup,
    ) -> Result<AccountGroup, AppError> {
        use crate::domain::integrations::Permission;
        validate_name(&input.name)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        let body: String = transaction.query_row(
            "SELECT body FROM integration_objects WHERE kind='grant' AND id=?1",
            [grant_id],
            |row| row.get(0),
        )?;
        let mut grant: AuthorizationGrant = serde_json::from_str(&body)?;
        let existing: Option<String> = transaction
            .query_row(
                "SELECT body FROM integration_objects WHERE kind='group' AND id=?1",
                [&input.id],
                |row| row.get(0),
            )
            .optional()?;
        let existing = existing
            .map(|body| serde_json::from_str::<AccountGroup>(&body))
            .transpose()?;
        if !grant.permits(Permission::ManageGroups, None, chrono::Utc::now())
            || existing.is_some()
                && !grant.scope.global_including_future
                && !grant.scope.group_ids.contains(&input.id)
            || input.account_ids.iter().any(|id| {
                !grant.scope.global_including_future && !grant.scope.account_ids.contains(id)
            })
            || input.exclusive_application_id
                != existing
                    .as_ref()
                    .and_then(|group| group.exclusive_application_id.clone())
        {
            return Err(AppError::AccessDenied(
                "Grupo/contas fora do escopo aprovado.".to_owned(),
            ));
        }
        let mut next = input.clone();
        next.revision = next_revision(input.revision)?;
        persist_object(&transaction, "group", &next.id, input.revision, &next)?;
        transaction.execute("DELETE FROM group_members WHERE group_id=?1", [&next.id])?;
        for id in &next.account_ids {
            transaction.execute(
                "INSERT INTO group_members(group_id,account_id) VALUES(?1,?2)",
                params![next.id, id],
            )?;
        }
        if existing.is_none() {
            grant.scope.group_ids.insert(next.id.clone());
            transaction.execute("UPDATE integration_objects SET body=?2,revision=revision+1 WHERE kind='grant' AND id=?1",params![grant_id,serde_json::to_string(&grant)?])?;
        }
        transaction.commit()?;
        Ok(next)
    }

    pub fn delete_group(&self, id: &str, revision: u64) -> Result<(), AppError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        if transaction.execute(
            "DELETE FROM integration_objects WHERE kind='group' AND id=?1 AND revision=?2",
            params![id, sql_revision(revision)?],
        )? != 1
        {
            return Err(AppError::Busy(
                "O grupo mudou; releia antes de excluir.".to_owned(),
            ));
        }
        transaction.execute("DELETE FROM group_members WHERE group_id=?1", [id])?;
        transaction.commit()?;
        Ok(())
    }

    pub fn applications(&self) -> Result<Vec<Application>, AppError> {
        self.integration_objects("application")
    }
    pub fn bindings(&self) -> Result<Vec<ExecutionBinding>, AppError> {
        self.integration_objects("binding")
    }
    pub fn grants(&self) -> Result<Vec<AuthorizationGrant>, AppError> {
        self.integration_objects("grant")
    }

    pub fn save_application(&self, input: &Application) -> Result<Application, AppError> {
        validate_application(input)?;
        let mut next = input.clone();
        next.revision = next_revision(input.revision)?;
        let connection = self.connection()?;
        ensure_unique_slug(&connection, input)?;
        persist_object(&connection, "application", &next.id, input.revision, &next)?;
        Ok(next)
    }

    pub fn save_binding(&self, input: &ExecutionBinding) -> Result<ExecutionBinding, AppError> {
        validate_name(&input.name)?;
        let application: Option<Application> =
            self.integration_object("application", &input.application_id)?;
        if application.is_none() {
            return Err(AppError::Validation("Aplicação não encontrada.".to_owned()));
        }
        if input.default_model.as_ref().is_some_and(|model| {
            !input.allowed_models.is_empty() && !input.allowed_models.contains(model)
        }) {
            return Err(AppError::Validation(
                "Modelo padrão não está na lista permitida.".to_owned(),
            ));
        }
        let mut next = input.clone();
        next.revision = next_revision(input.revision)?;
        let connection = self.connection()?;
        persist_object(&connection, "binding", &next.id, input.revision, &next)?;
        Ok(next)
    }

    #[cfg(test)]
    pub fn save_grant(&self, grant: &AuthorizationGrant) -> Result<(), AppError> {
        self.connection()?.execute(
            "INSERT INTO integration_objects(kind,id,revision,body) VALUES('grant',?1,1,?2)",
            params![grant.id, serde_json::to_string(grant)?],
        )?;
        Ok(())
    }

    pub fn grant(&self, id: &str) -> Result<Option<AuthorizationGrant>, AppError> {
        self.integration_object("grant", id)
    }

    pub fn revoke_grant(&self, id: &str) -> Result<(), AppError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        let body: String = transaction.query_row(
            "SELECT body FROM integration_objects WHERE kind='grant' AND id=?1",
            [id],
            |row| row.get(0),
        )?;
        let mut grant: AuthorizationGrant = serde_json::from_str(&body)?;
        grant.revoked = true;
        transaction.execute("UPDATE integration_objects SET body=?2,revision=revision+1 WHERE kind='grant' AND id=?1",
            params![id,serde_json::to_string(&grant)?])?;
        transaction.commit()?;
        Ok(())
    }

    pub(super) fn integration_objects<T: DeserializeOwned>(
        &self,
        kind: &str,
    ) -> Result<Vec<T>, AppError> {
        let connection = self.connection()?;
        let mut statement =
            connection.prepare("SELECT body FROM integration_objects WHERE kind=?1 ORDER BY id")?;
        let rows = statement
            .query_map([kind], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|body| serde_json::from_str(&body).map_err(AppError::from))
            .collect()
    }

    pub(super) fn integration_object<T: DeserializeOwned>(
        &self,
        kind: &str,
        id: &str,
    ) -> Result<Option<T>, AppError> {
        let body: Option<String> = self
            .connection()?
            .query_row(
                "SELECT body FROM integration_objects WHERE kind=?1 AND id=?2",
                params![kind, id],
                |row| row.get(0),
            )
            .optional()?;
        body.map(|body| serde_json::from_str(&body).map_err(AppError::from))
            .transpose()
    }
}

pub(super) fn persist_object<T: Serialize>(
    connection: &rusqlite::Connection,
    kind: &str,
    id: &str,
    revision: u64,
    value: &T,
) -> Result<(), AppError> {
    uuid::Uuid::parse_str(id)
        .map_err(|_| AppError::Validation("Identificador inválido.".to_owned()))?;
    let body = serde_json::to_string(value)?;
    let changed = if revision == 0 {
        connection.execute(
            "INSERT INTO integration_objects(kind,id,revision,body) VALUES(?1,?2,1,?3)",
            params![kind, id, body],
        )?
    } else {
        connection.execute("UPDATE integration_objects SET revision=revision+1,body=?4 WHERE kind=?1 AND id=?2 AND revision=?3",
            params![kind,id,sql_revision(revision)?,body])?
    };
    if changed != 1 {
        return Err(AppError::Busy(
            "O objeto mudou; releia antes de salvar.".to_owned(),
        ));
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), AppError> {
    if name.trim().is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err(AppError::Validation(
            "Nome deve ter entre 1 e 80 caracteres, sem controles.".to_owned(),
        ));
    }
    Ok(())
}

fn sql_revision(revision: u64) -> Result<i64, AppError> {
    i64::try_from(revision).map_err(|_| AppError::Validation("Révision inválida.".to_owned()))
}

pub(super) fn next_revision(revision: u64) -> Result<u64, AppError> {
    sql_revision(revision)?;
    revision
        .checked_add(1)
        .ok_or_else(|| AppError::Validation("Revisão inválida.".to_owned()))
}

pub(super) fn validate_application(input: &Application) -> Result<(), AppError> {
    validate_name(&input.name)?;
    if input.slug.is_empty()
        || input.slug.len() > 64
        || !input
            .slug
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(AppError::Validation(
            "Identificador deve conter até 64 letras, números e hífens.".to_owned(),
        ));
    }
    Ok(())
}
pub(super) fn ensure_unique_slug(
    connection: &rusqlite::Connection,
    input: &Application,
) -> Result<(), AppError> {
    let taken: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM integration_objects WHERE kind='application' AND json_extract(body,'$.slug')=?1 AND id<>?2)", params![input.slug, input.id], |row| row.get(0))?;
    if taken {
        return Err(AppError::Validation(
            "Este identificador já pertence a outra aplicação.".to_owned(),
        ));
    }
    Ok(())
}

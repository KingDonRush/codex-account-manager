use super::{
    integrations::{ensure_unique_slug, next_revision, persist_object, validate_application},
    Database,
};
use crate::domain::integrations::{
    AccountGroup, Application, AuthorizationGrant, AuthorizationSetup, Permission,
};
use crate::error::AppError;
use rusqlite::{params, Connection, OptionalExtension};

impl Database {
    /// Grant publication follows this all-or-nothing metadata commit.
    #[cfg(test)]
    pub fn commit_authorization(
        &self,
        grant: &AuthorizationGrant,
        application: &Application,
        groups: &[AccountGroup],
    ) -> Result<(), AppError> {
        self.commit_authorization_setup(grant, application, groups, &AuthorizationSetup::default())
    }

    pub fn commit_authorization_setup(
        &self,
        grant: &AuthorizationGrant,
        application: &Application,
        groups: &[AccountGroup],
        setup: &AuthorizationSetup,
    ) -> Result<(), AppError> {
        validate_application(application)?;
        validate_setup(grant, application, setup)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        commit_application(&transaction, application)?;
        commit_accounts(&transaction, grant, setup)?;
        for group in groups {
            commit_group(&transaction, grant, group)?;
        }
        transaction.execute(
            "INSERT INTO integration_objects(kind,id,revision,body) VALUES('grant',?1,1,?2)",
            params![grant.id, serde_json::to_string(grant)?],
        )?;
        transaction.commit()?;
        Ok(())
    }
}

fn validate_setup(
    grant: &AuthorizationGrant,
    app: &Application,
    setup: &AuthorizationSetup,
) -> Result<(), AppError> {
    if grant.application_id != app.id
        || !setup.shared_account_ids.is_subset(&grant.scope.account_ids)
        || !setup.shared_account_ids.is_empty()
            && !grant.scope.permissions.contains(&Permission::Execute)
    {
        return Err(AppError::AccessDenied(
            "Configuração fora do consentimento de execução.".into(),
        ));
    }
    Ok(())
}

fn commit_application(connection: &Connection, application: &Application) -> Result<(), AppError> {
    if application.revision == 0 {
        ensure_unique_slug(connection, application)?;
        let mut saved = application.clone();
        saved.revision = 1;
        return persist_object(connection, "application", &saved.id, 0, &saved);
    }
    let body: Option<String> = connection
        .query_row(
            "SELECT body FROM integration_objects WHERE kind='application' AND id=?1",
            [&application.id],
            |row| row.get(0),
        )
        .optional()?;
    let current: Application = body
        .map(|body| serde_json::from_str(&body))
        .transpose()?
        .ok_or_else(|| AppError::Busy("Aplicação removida antes da aprovação.".into()))?;
    if current.revision != application.revision || current.paused {
        return Err(AppError::Busy(
            "Aplicação mudou antes da aprovação; revise o pedido.".into(),
        ));
    }
    Ok(())
}

fn commit_accounts(
    connection: &Connection,
    grant: &AuthorizationGrant,
    setup: &AuthorizationSetup,
) -> Result<(), AppError> {
    for id in &grant.scope.account_ids {
        let exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM integration_accounts WHERE id=?1)",
            [id],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(AppError::Busy("Conta mudou antes da aprovação.".into()));
        }
    }
    for id in &setup.shared_account_ids {
        // Preserve apps-only reservations, capacity, coffer and Desktop auth.
        connection.execute("UPDATE integration_accounts SET policy='shared' WHERE id=?1 AND policy IN ('desktop_only','\"desktop_only\"')",[id])?;
    }
    Ok(())
}

fn commit_group(
    connection: &Connection,
    grant: &AuthorizationGrant,
    expected: &AccountGroup,
) -> Result<(), AppError> {
    let body: Option<String> = connection
        .query_row(
            "SELECT body FROM integration_objects WHERE kind='group' AND id=?1",
            [&expected.id],
            |row| row.get(0),
        )
        .optional()?;
    let mut group: AccountGroup = body
        .map(|body| serde_json::from_str(&body))
        .transpose()?
        .ok_or_else(|| AppError::Busy("Grupo removido antes da aprovação.".to_owned()))?;
    if group.revision != expected.revision || group.archived {
        return Err(AppError::Busy(
            "Grupo mudou antes da aprovação; revise o pedido.".to_owned(),
        ));
    }
    if !grant.scope.background_group_ids.contains(&group.id) {
        return Ok(());
    }
    if group
        .exclusive_application_id
        .as_ref()
        .is_some_and(|owner| owner != &grant.application_id)
    {
        return Err(AppError::AccessDenied(
            "Grupo exclusivo de outra aplicação.".to_owned(),
        ));
    }
    let previous = group.revision;
    group.exclusive_application_id = Some(grant.application_id.clone());
    group.revision = next_revision(previous)?;
    persist_object(connection, "group", &group.id, previous, &group)
}

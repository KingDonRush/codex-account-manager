use std::collections::{BTreeMap, BTreeSet};

use super::ManagerCore;
use crate::domain::execution::ExecRequest;
use crate::domain::integrations::{AuthorizationGrant, BindingTarget, ExecutionBinding};
use crate::error::AppError;
#[cfg(test)]
mod tests;

impl ManagerCore {
    /// Direct execution uses the approved resources. Existing bindingId callers
    /// retain their pinned model/target options, without making UI users create one.
    pub(super) fn execution_route(
        &self,
        grant: &AuthorizationGrant,
        request: &ExecRequest,
    ) -> Result<ExecutionBinding, AppError> {
        let application = self
            .database
            .applications()?
            .into_iter()
            .find(|app| app.id == grant.application_id);
        if application.is_none_or(|app| app.paused) {
            return Err(AppError::Busy("Aplicação pausada ou removida.".to_owned()));
        }
        if !request.binding_id.is_empty() {
            return self.legacy_execution_route(grant, request);
        }
        let target = self.direct_target(grant, request)?;
        Ok(ExecutionBinding {
            id: String::new(),
            application_id: grant.application_id.clone(),
            name: "Recursos autorizados".into(),
            target,
            automatic: true,
            allow_projected_quota: false,
            allowed_models: BTreeSet::new(),
            default_model: None,
            paused: false,
            revision: 0,
        })
    }

    fn legacy_execution_route(
        &self,
        grant: &AuthorizationGrant,
        request: &ExecRequest,
    ) -> Result<ExecutionBinding, AppError> {
        if request.group_id.is_some() {
            return Err(AppError::Validation(
                "Use o grupo direto ou a configuração antiga, não ambos.".into(),
            ));
        }
        let current = self
            .database
            .bindings()?
            .into_iter()
            .find(|item| {
                item.id == request.binding_id && item.application_id == grant.application_id
            })
            .ok_or_else(|| AppError::AccessDenied("Configuração antiga não autorizada.".into()))?;
        if current.paused {
            return Err(AppError::Busy("Configuração antiga pausada.".into()));
        }
        Ok(request
            .binding_snapshot
            .as_ref()
            .unwrap_or(&current)
            .clone())
    }

    fn direct_target(
        &self,
        grant: &AuthorizationGrant,
        request: &ExecRequest,
    ) -> Result<BindingTarget, AppError> {
        if let Some(group) = &request.group_id {
            return Ok(BindingTarget::Group(group.clone()));
        }
        if !grant.scope.global_including_future
            && !grant.scope.group_ids.is_empty()
            && grant.scope.direct_account_ids.is_none()
        {
            // Old consents expanded group members into accountIds, losing
            // provenance. Do not reinterpret them as independent direct access.
            return Err(AppError::ConsentRequired("Esta autorização antiga contém grupos sem registrar a origem das contas. Informe um groupId aprovado ou atualize o consentimento pela aplicação. As configurações antigas continuam funcionando.".into()));
        }
        if !grant.scope.global_including_future
            && grant.scope.group_ids.len() == 1
            && grant
                .scope
                .direct_account_ids
                .as_ref()
                .is_some_and(BTreeSet::is_empty)
        {
            let group = self
                .database
                .groups()?
                .into_iter()
                .find(|group| grant.scope.group_ids.contains(&group.id));
            if let Some(group) =
                group.filter(|group| grant.scope.account_ids.is_subset(&group.account_ids))
            {
                return Ok(BindingTarget::Group(group.id));
            }
        }
        if grant.scope.group_ids.is_empty() {
            if let Some(account) = &request.account_id {
                return Ok(BindingTarget::Account(account.clone()));
            }
        }
        Ok(BindingTarget::Authorized)
    }

    pub(super) fn route_accounts(
        &self,
        grant: &AuthorizationGrant,
        target: &BindingTarget,
    ) -> Result<BTreeMap<String, Option<String>>, AppError> {
        match target {
            BindingTarget::Account(id) => Ok(BTreeMap::from([(id.clone(), None)])),
            BindingTarget::Group(id) => {
                let group = self.authorized_group(grant, id)?;
                Ok(group
                    .account_ids
                    .into_iter()
                    .map(|account| (account, Some(id.clone())))
                    .collect())
            }
            BindingTarget::Authorized => self.authorized_accounts(grant),
        }
    }

    fn authorized_group(
        &self,
        grant: &AuthorizationGrant,
        id: &str,
    ) -> Result<crate::domain::integrations::AccountGroup, AppError> {
        if !grant.scope.global_including_future && !grant.scope.group_ids.contains(id) {
            return Err(AppError::AccessDenied(
                "Grupo fora das opções aprovadas.".to_owned(),
            ));
        }
        self.database
            .groups()?
            .into_iter()
            .find(|group| group.id == id && group_is_usable(group, &grant.application_id))
            .ok_or_else(|| {
                AppError::Busy(
                    "Grupo pausado, arquivado, removido ou reservado a outra aplicação.".to_owned(),
                )
            })
    }

    fn authorized_accounts(
        &self,
        grant: &AuthorizationGrant,
    ) -> Result<BTreeMap<String, Option<String>>, AppError> {
        if grant.scope.global_including_future {
            return Ok(self
                .integration_snapshot()?
                .accounts
                .into_iter()
                .map(|account| (account.id, None))
                .collect());
        }
        let mut groups = self.database.groups()?;
        groups.retain(|group| grant.scope.group_ids.contains(&group.id));
        groups.sort_by(|left, right| left.id.cmp(&right.id));
        let direct = grant
            .scope
            .direct_account_ids
            .as_ref()
            .unwrap_or(&grant.scope.account_ids);
        let mut accounts = direct
            .iter()
            .cloned()
            .map(|id| (id, None))
            .collect::<BTreeMap<_, _>>();
        for group in groups
            .into_iter()
            .filter(|group| group_is_usable(group, &grant.application_id))
        {
            for id in group.account_ids {
                accounts.entry(id).or_insert_with(|| Some(group.id.clone()));
            }
        }
        Ok(accounts)
    }
}

fn group_is_usable(group: &crate::domain::integrations::AccountGroup, application: &str) -> bool {
    !group.archived
        && !group.paused
        && group
            .exclusive_application_id
            .as_deref()
            .is_none_or(|owner| owner == application)
}

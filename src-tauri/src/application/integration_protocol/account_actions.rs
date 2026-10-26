use super::*;
use crate::domain::integrations::AccountPolicy;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Input {
    account_id: String,
    name: Option<String>,
    policy: Option<AccountPolicy>,
    capacity: Option<u32>,
    operation_id: Option<String>,
}

pub(super) fn dispatch(
    core: &ManagerCore,
    grant: &AuthorizationGrant,
    method: &str,
    params: Value,
) -> Result<Value, AppError> {
    let input: Input = serde_json::from_value(params)?;
    let permission = if method == "accounts/activate" {
        Permission::ActivateDesktop
    } else if matches!(method, "accounts/openBrowser" | "accounts/openMail") {
        Permission::OpenBrowser
    } else {
        Permission::ManageAccounts
    };
    if !grant.permits(permission, Some(&input.account_id), chrono::Utc::now()) {
        return Err(AppError::AccessDenied(
            "Conta fora do escopo desta ação.".to_owned(),
        ));
    }
    let account = core
        .integration_snapshot()?
        .accounts
        .into_iter()
        .find(|account| account.id == input.account_id)
        .ok_or_else(|| AppError::AccessDenied("Conta indisponível.".to_owned()))?;
    match method {
        "accounts/rename" => {
            core.rename_profile(
                &account.profile_id,
                input
                    .name
                    .as_deref()
                    .ok_or_else(|| AppError::Validation("Nome ausente.".to_owned()))?,
            )?;
            Ok(json!({"accountId":account.id,"updated":true}))
        }
        "accounts/setPolicy" => {
            core.set_account_policy(
                &account.id,
                input
                    .policy
                    .ok_or_else(|| AppError::Validation("Política ausente.".to_owned()))?,
                input.capacity.unwrap_or(account.capacity),
            )?;
            Ok(json!({"updated":true}))
        }
        "accounts/archive" => {
            core.archive_profile(&account.profile_id)?;
            Ok(json!({"accountId":account.id,"updated":true}))
        }
        "accounts/restore" => {
            core.restore_profile(&account.profile_id)?;
            Ok(json!({"accountId":account.id,"updated":true}))
        }
        "accounts/reconnect" => Ok(serde_json::to_value(core.start_login(
            &account.profile_id,
            true,
            None,
        )?)?),
        "accounts/connectApplications" => Ok(serde_json::to_value(
            core.start_application_login(&account.profile_id)?,
        )?),
        "accounts/replace" => Ok(serde_json::to_value(
            core.start_replacement(&account.profile_id)?,
        )?),
        "accounts/activate" => {
            if account.policy == AccountPolicy::ApplicationsOnly {
                return Err(AppError::ConsentRequired("Esta conta está reservada; confirme compartilhamento na interface antes de ativar.".to_owned()));
            }
            Ok(serde_json::to_value(
                core.start_switch_account(&account.profile_id)?,
            )?)
        }
        "accounts/openBrowser" => {
            core.open_profile_browser(&account.profile_id)?;
            Ok(json!({"opened":true}))
        }
        "accounts/openMail" => {
            core.open_mail(&account.profile_id)?;
            Ok(json!({"opened":true}))
        }
        "accounts/unlink" => Ok(serde_json::to_value(core.request_deletion(
            Some(grant),
            &std::collections::BTreeSet::from([account.id]),
        )?)?),
        "operation/status" => {
            let id = input
                .operation_id
                .ok_or_else(|| AppError::Validation("Operação ausente.".to_owned()))?;
            let operation = core
                .list_operations()?
                .into_iter()
                .find(|operation| operation.id == id && operation.profile_id == account.profile_id)
                .ok_or_else(|| AppError::OperationNotFound(id))?;
            Ok(serde_json::to_value(operation)?)
        }
        _ => Err(AppError::Validation("Ação não suportada.".to_owned())),
    }
}

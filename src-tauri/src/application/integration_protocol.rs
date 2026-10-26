use super::authorization::{random_hex, verify_proof, ConnectionRequest};
use super::local_transport::PeerIdentity;
#[cfg(any(unix, windows))]
use super::local_transport::{read_frame_cancellable, write_frame};
use super::manager::ManagerCore;
use crate::domain::integrations::{AccountGroup, AuthorizationGrant, ExecutionBinding, Permission};
use crate::error::AppError;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::{Duration, Instant};
mod account_actions;
#[cfg(test)]
mod tests;

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Request {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub id: u64,
    pub result: Option<Value>,
    pub error: Option<ProtocolError>,
}
#[derive(Serialize, Deserialize)]
pub struct ProtocolError {
    pub code: String,
    pub message: String,
}

impl Response {
    pub fn from_result(id: u64, result: Result<Value, AppError>) -> Self {
        match result {
            Ok(value) => Self {
                id,
                result: Some(value),
                error: None,
            },
            Err(error) => Self {
                id,
                result: None,
                error: Some(ProtocolError {
                    code: error.code().to_owned(),
                    message: crate::adapters::logging::sanitize_error(&error.to_string()),
                }),
            },
        }
    }
}

pub struct Connection {
    pub peer: PeerIdentity,
    pub id: String,
    grant_id: Option<String>,
    associated_grants: std::collections::BTreeSet<String>,
    challenge: Option<(String, String, Instant)>,
}

impl Connection {
    pub fn new(peer: PeerIdentity) -> Self {
        Self {
            peer,
            id: uuid::Uuid::new_v4().to_string(),
            grant_id: None,
            associated_grants: std::collections::BTreeSet::new(),
            challenge: None,
        }
    }

    pub fn dispatch(&mut self, core: &ManagerCore, request: Request) -> Result<Value, AppError> {
        match request.method.as_str() {
            "system/capabilities" => Ok(
                json!({"apiVersion":2,"catalog":true,"groups":true,"consent":true,
                "exec":true,"appServer":true,"transport":"local","oauthTokensExported":false}),
            ),
            "catalog/read" => core.public_integration_catalog(),
            "authorization/request" => {
                let params: ConnectionRequest = serde_json::from_value(request.params)?;
                Ok(serde_json::to_value(
                    core.request_authorization(params, &self.peer, &self.id)?,
                )?)
            }
            "authorization/claim" => {
                let id = required_string(&request.params, "requestId")?;
                match core.claim_authorization(id, &self.id, &self.peer.identity)? {
                    Some(claim) => {
                        self.associated_grants.insert(claim.grant_id.clone());
                        let grant = core.authorization_grant(&claim.grant_id)?;
                        Ok(
                            json!({"grantId":claim.grant_id,"applicationId":grant.application_id,"permanent":grant.scope.permanent,"expiresAt":grant.scope.expires_at,"signingKey":hex::encode(claim.signing_key.to_bytes())}),
                        )
                    }
                    None => Ok(json!({"pending":true})),
                }
            }
            "authorization/challenge" => {
                let id = required_string(&request.params, "grantId")?;
                let grant = core.authorization_grant(id)?;
                if grant.peer_identity != self.peer.identity || grant.revoked {
                    return Err(AppError::AccessDenied(
                        "Consumidor não autorizado.".to_owned(),
                    ));
                }
                let challenge = format!("codex-accounts-v2:{}:{}:{}", self.id, id, random_hex(32)?);
                self.challenge = Some((id.to_owned(), challenge.clone(), Instant::now()));
                Ok(json!({"challenge":challenge}))
            }
            "authorization/prove" => {
                let (id, challenge, created) = self
                    .challenge
                    .take()
                    .ok_or_else(|| AppError::AccessDenied("Não há desafio pendente.".to_owned()))?;
                if created.elapsed() > Duration::from_secs(30) {
                    return Err(AppError::AccessDenied("Desafio expirado.".to_owned()));
                }
                verify_proof(
                    &core.authorization_grant(&id)?,
                    &challenge,
                    required_string(&request.params, "proof")?,
                    &self.peer.identity,
                )?;
                self.associated_grants.insert(id.clone());
                self.grant_id = Some(id);
                Ok(json!({"authenticated":true}))
            }
            _ => self.protected(core, &request.method, request.params),
        }
    }

    fn protected(
        &self,
        core: &ManagerCore,
        method: &str,
        params: Value,
    ) -> Result<Value, AppError> {
        let id = self.grant_id.as_deref().ok_or_else(|| {
            AppError::ConsentRequired("Conecte e permita na interface do gerenciador.".to_owned())
        })?;
        let grant = core.authorization_grant(id)?;
        core.require_security_context(&grant, method)?;
        let permission = match method {
            "accounts/rename"
            | "accounts/setPolicy"
            | "accounts/archive"
            | "accounts/restore"
            | "accounts/reconnect"
            | "accounts/connectApplications"
            | "accounts/replace"
            | "accounts/unlink"
            | "operation/status" => Permission::ManageAccounts,
            "accounts/activate" => Permission::ActivateDesktop,
            "accounts/openBrowser" | "accounts/openMail" => Permission::OpenBrowser,
            "accounts/requestDeletion" | "deletion/status" => Permission::RequestDeletion,
            "session/open"
            | "session/send"
            | "session/events"
            | "session/status"
            | "session/close"
            | "session/recover"
            | "session/list"
            | "session/clearHistory" => Permission::Execute,
            "execution/start" | "execution/status" | "execution/events" | "execution/cancel"
            | "execution/ack" => Permission::Execute,
            "accounts/list" | "groups/list" | "bindings/list" => Permission::ReadMetadata,
            "groups/save" => Permission::ManageGroups,
            "bindings/save" => Permission::ManageBindings,
            _ => {
                return Err(AppError::Validation(
                    "Método não suportado; consulte system/capabilities.".to_owned(),
                ))
            }
        };
        if !grant.permits(permission, None, chrono::Utc::now()) {
            return Err(AppError::AccessDenied(
                "Permissão insuficiente ou autorização expirada.".to_owned(),
            ));
        }
        if matches!(
            method,
            "accounts/rename"
                | "accounts/setPolicy"
                | "accounts/archive"
                | "accounts/restore"
                | "accounts/reconnect"
                | "accounts/connectApplications"
                | "accounts/replace"
                | "accounts/unlink"
                | "accounts/activate"
                | "accounts/openBrowser"
                | "accounts/openMail"
                | "operation/status"
        ) {
            return account_actions::dispatch(core, &grant, method, params);
        }
        if method == "accounts/requestDeletion" {
            let ids: std::collections::BTreeSet<String> = serde_json::from_value(
                params
                    .get("accountIds")
                    .cloned()
                    .ok_or_else(|| AppError::Validation("Seleção ausente.".to_owned()))?,
            )?;
            return Ok(serde_json::to_value(
                core.request_deletion(Some(&grant), &ids)?,
            )?);
        }
        if method == "deletion/status" {
            return Ok(serde_json::to_value(core.deletion_status_for(
                &grant,
                required_string(&params, "requestId")?,
            )?)?);
        }
        if method.starts_with("session/") {
            if method == "session/open" {
                return Ok(serde_json::to_value(core.open_application_server(
                    &grant,
                    serde_json::from_value(params)?,
                )?)?);
            }
            if method == "session/list" {
                return Ok(serde_json::to_value(
                    core.list_application_servers()
                        .into_iter()
                        .filter(|status| {
                            status.application_id == grant.application_id
                                && grant.permits(
                                    Permission::Execute,
                                    Some(&status.account_id),
                                    chrono::Utc::now(),
                                )
                        })
                        .collect::<Vec<_>>(),
                )?);
            }
            let id = required_string(&params, "sessionId")?;
            if method == "session/recover" {
                return Ok(serde_json::to_value(
                    core.recover_application_server(&grant, id)?,
                )?);
            }
            let status = core.application_server_for(&grant, id)?;
            return match method {
                "session/send" => {
                    core.send_application_server(
                        &grant,
                        id,
                        params
                            .get("message")
                            .cloned()
                            .ok_or_else(|| AppError::Validation("Mensagem ausente.".to_owned()))?,
                    )?;
                    Ok(json!({"accepted":true}))
                }
                "session/events" => core.application_server_events(
                    &grant,
                    id,
                    params.get("offset").and_then(Value::as_u64).unwrap_or(0),
                ),
                "session/close" => {
                    core.close_application_server(id)?;
                    Ok(json!({"closing":true}))
                }
                "session/clearHistory" => {
                    core.clear_application_history(id)?;
                    Ok(json!({"cleared":true}))
                }
                _ => Ok(serde_json::to_value(status)?),
            };
        }
        if method == "execution/start" {
            return Ok(serde_json::to_value(
                core.start_execution(&grant, serde_json::from_value(params)?)?,
            )?);
        }
        if matches!(
            method,
            "execution/status" | "execution/events" | "execution/cancel" | "execution/ack"
        ) {
            let id = required_string(&params, "executionId")?;
            let status = core.execution_status_for(&grant, id)?;
            return match method {
                "execution/ack" => {
                    core.acknowledge_execution(&grant, id)?;
                    Ok(json!({"acknowledged":true}))
                }
                "execution/events" => core.execution_events(
                    &grant,
                    id,
                    params.get("offset").and_then(Value::as_u64).unwrap_or(0),
                ),
                "execution/cancel" => {
                    core.cancel_execution(id)?;
                    Ok(json!({"cancelRequested":true}))
                }
                _ => Ok(serde_json::to_value(status)?),
            };
        }
        dispatch_protected(core, &grant, method, params)
    }

    pub fn close(&self, core: &ManagerCore) {
        for id in &self.associated_grants {
            core.consumer_disconnected(id);
            if core
                .authorization_grant(id)
                .is_ok_and(|grant| !grant.scope.permanent && grant.scope.expires_at.is_none())
            {
                let _ = core.revoke_authorization(id);
            }
        }
    }
}

fn dispatch_protected(
    core: &ManagerCore,
    grant: &AuthorizationGrant,
    method: &str,
    params: Value,
) -> Result<Value, AppError> {
    let snapshot = core.integration_snapshot()?;
    match method {
        "accounts/list" => Ok(serde_json::to_value(
            snapshot
                .accounts
                .into_iter()
                .filter(|account| {
                    grant.scope.global_including_future
                        || grant.scope.account_ids.contains(&account.id)
                })
                .collect::<Vec<_>>(),
        )?),
        "groups/list" => Ok(serde_json::to_value(
            snapshot
                .groups
                .into_iter()
                .filter(|group| {
                    grant.scope.global_including_future || grant.scope.group_ids.contains(&group.id)
                })
                .map(|mut group| {
                    group.account_ids.retain(|id| {
                        grant.scope.global_including_future || grant.scope.account_ids.contains(id)
                    });
                    group
                })
                .collect::<Vec<_>>(),
        )?),
        "bindings/list" => Ok(serde_json::to_value(
            snapshot
                .bindings
                .into_iter()
                .filter(|binding| binding.application_id == grant.application_id)
                .collect::<Vec<_>>(),
        )?),
        "groups/save" => {
            let group: AccountGroup = serde_json::from_value(params)?;
            let existing = snapshot
                .groups
                .iter()
                .find(|existing| existing.id == group.id);
            if existing.is_some()
                && !grant.scope.global_including_future
                && !grant.scope.group_ids.contains(&group.id)
                || group.account_ids.iter().any(|id| {
                    !grant.scope.global_including_future && !grant.scope.account_ids.contains(id)
                })
                || group.exclusive_application_id
                    != existing.and_then(|existing| existing.exclusive_application_id.clone())
            {
                return Err(AppError::AccessDenied(
                    "Grupo/contas fora do escopo ou tentativa de mudar exclusividade.".to_owned(),
                ));
            }
            Ok(serde_json::to_value(
                core.save_authorized_group(&grant.id, &group)?,
            )?)
        }
        "bindings/save" => {
            let binding: ExecutionBinding = serde_json::from_value(params)?;
            if binding.application_id != grant.application_id {
                return Err(AppError::AccessDenied(
                    "Vínculo pertence a outra aplicação.".to_owned(),
                ));
            }
            let allowed = match &binding.target {
                crate::domain::integrations::BindingTarget::Authorized => false,
                crate::domain::integrations::BindingTarget::Account(id) => {
                    grant.scope.global_including_future || grant.scope.account_ids.contains(id)
                }
                crate::domain::integrations::BindingTarget::Group(id) => {
                    grant.scope.global_including_future || grant.scope.group_ids.contains(id)
                }
            };
            if !allowed {
                return Err(AppError::AccessDenied("Destino fora do escopo.".to_owned()));
            }
            Ok(serde_json::to_value(core.save_binding(&binding)?)?)
        }
        _ => Err(AppError::Validation("Método indisponível.".to_owned())),
    }
}

fn required_string<'a>(value: &'a Value, field: &str) -> Result<&'a str, AppError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| value.len() <= 256)
        .ok_or_else(|| AppError::Validation(format!("Campo {field} inválido.")))
}

#[cfg(any(unix, windows))]
pub fn serve_connection(
    core: ManagerCore,
    mut stream: impl super::local_transport::LocalStream,
) -> Result<(), AppError> {
    stream.prepare()?;
    let mut connection = Connection::new(stream.peer()?);
    let result = (|| loop {
        let request: Request = read_frame_cancellable(
            &mut stream,
            Duration::from_secs(180),
            &core.shutdown_token(),
        )?;
        let id = request.id;
        write_frame(
            &mut stream,
            &Response::from_result(id, connection.dispatch(&core, request)),
        )?;
    })();
    connection.close(&core);
    result
}

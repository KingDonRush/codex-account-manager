use crate::domain::application_server::{ApplicationServerStatus, ServerState};
use crate::error::AppError;
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, SyncSender},
    Arc,
};

pub struct ServerControl {
    pub status: ApplicationServerStatus,
    pub cancel: Arc<AtomicBool>,
    pub sender: SyncSender<Value>,
}
#[derive(Default)]
pub struct ApplicationServers {
    inner: Mutex<HashMap<String, ServerControl>>,
}
impl ApplicationServers {
    pub fn restore(&self, status: ApplicationServerStatus) {
        let (sender, _receiver) = mpsc::sync_channel(1);
        self.inner.lock().insert(
            status.id.clone(),
            ServerControl {
                status,
                cancel: Arc::new(AtomicBool::new(true)),
                sender,
            },
        );
    }
    pub fn add(
        &self,
        status: ApplicationServerStatus,
    ) -> Result<(Receiver<Value>, Arc<AtomicBool>), AppError> {
        let mut sessions = self.inner.lock();
        if sessions
            .get(&status.id)
            .is_some_and(|control| !control.status.state.terminal())
        {
            return Err(AppError::Busy(
                "Esta sessão já tem um proprietário ativo.".to_owned(),
            ));
        }
        if sessions
            .values()
            .filter(|session| !session.status.state.terminal())
            .count()
            >= 16
        {
            return Err(AppError::Busy(
                "Limite de sessões abertas atingido.".to_owned(),
            ));
        }
        let (sender, receiver) = mpsc::sync_channel(16);
        let cancel = Arc::new(AtomicBool::new(false));
        sessions.insert(
            status.id.clone(),
            ServerControl {
                status,
                cancel: cancel.clone(),
                sender,
            },
        );
        Ok((receiver, cancel))
    }
    pub fn list(&self) -> Vec<ApplicationServerStatus> {
        self.inner
            .lock()
            .values()
            .map(|control| control.status.clone())
            .collect()
    }
    pub fn status(&self, id: &str) -> Result<ApplicationServerStatus, AppError> {
        self.inner
            .lock()
            .get(id)
            .map(|control| control.status.clone())
            .ok_or_else(|| AppError::OperationNotFound(id.to_owned()))
    }
    pub fn update(&self, status: ApplicationServerStatus) {
        if let Some(control) = self.inner.lock().get_mut(&status.id) {
            control.status = status;
        }
    }
    pub fn send(&self, id: &str, message: Value) -> Result<(), AppError> {
        let sessions = self.inner.lock();
        let control = sessions
            .get(id)
            .ok_or_else(|| AppError::OperationNotFound(id.to_owned()))?;
        if control.status.state.terminal() {
            return Err(AppError::OperationFinished);
        }
        control
            .sender
            .try_send(message)
            .map_err(|_| AppError::Busy("Canal da sessão está cheio ou encerrado.".to_owned()))
    }
    pub fn close(&self, id: &str) -> Result<(), AppError> {
        let mut sessions = self.inner.lock();
        let control = sessions
            .get_mut(id)
            .ok_or_else(|| AppError::OperationNotFound(id.to_owned()))?;
        if !control.status.state.terminal() {
            control.status.state = ServerState::Closing;
            control.cancel.store(true, Ordering::Release);
        }
        Ok(())
    }
    pub fn close_matching(&self, filter: impl Fn(&ApplicationServerStatus) -> bool) {
        for control in self.inner.lock().values_mut() {
            if !control.status.state.terminal() && filter(&control.status) {
                control.status.state = ServerState::Closing;
                control.cancel.store(true, Ordering::Release);
            }
        }
    }
}

/// Only account identity/lifecycle are broker-owned; the application owns its tools and sandbox.
pub fn validate_harness_message(message: &Value) -> Result<(), AppError> {
    if serde_json::to_vec(message)?.len() > 256 * 1024 {
        return Err(AppError::Validation("Mensagem excessiva.".to_owned()));
    }
    let object = message
        .as_object()
        .ok_or_else(|| AppError::Validation("Frame RPC precisa ser um objeto.".to_owned()))?;
    if object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "id" | "method" | "params" | "result" | "error" | "jsonrpc"
        )
    }) {
        return Err(AppError::Validation(
            "Campo desconhecido no frame RPC.".to_owned(),
        ));
    }
    if object
        .get("jsonrpc")
        .is_some_and(|value| value.as_str() != Some("2.0"))
    {
        return Err(AppError::Validation("Versão RPC inválida.".to_owned()));
    }
    if let Some(method) = object.get("method") {
        if !method.is_string()
            || object.get("id").and_then(Value::as_u64).is_none()
            || object.contains_key("result")
            || object.contains_key("error")
            || object
                .get("params")
                .is_some_and(|params| !params.is_null() && !params.is_object())
        {
            return Err(AppError::Validation("Request RPC inválido.".to_owned()));
        }
    }
    if let Some(method) = message.get("method").and_then(Value::as_str) {
        const ALLOWED: &[&str] = &[
            "thread/start",
            "thread/resume",
            "thread/read",
            "thread/list",
            "thread/archive",
            "thread/unarchive",
            "thread/fork",
            "thread/rollback",
            "thread/compact/start",
            "thread/inject_items",
            "turn/start",
            "turn/steer",
            "turn/interrupt",
            "model/list",
            "command/exec",
            "command/exec/write",
            "command/exec/resize",
            "command/exec/terminate",
        ];
        if !ALLOWED.contains(&method) {
            return Err(AppError::AccessDenied(
                "RPC não pertence à superfície de harness; autenticação é gerenciada pelo backend."
                    .to_owned(),
            ));
        }
    } else if message.get("id").is_none()
        || message.get("result").is_none() && message.get("error").is_none()
    {
        return Err(AppError::Validation(
            "Resposta de ferramenta inválida.".to_owned(),
        ));
    }
    let params = message.get("params").unwrap_or(&Value::Null);
    if message
        .get("method")
        .and_then(Value::as_str)
        .is_some_and(|method| method.starts_with("thread/"))
        && params.get("path").is_some()
    {
        return Err(AppError::AccessDenied(
            "Use IDs de threads da aplicação, não caminhos de histórico externo.".to_owned(),
        ));
    }
    if let Some(fields) = params.as_object() {
        for key in fields.keys() {
            reject_identity_key(key)?;
        }
    }
    if let Some(config) = params.get("config") {
        reject_identity_override(config)?;
    }
    Ok(())
}
fn reject_identity_override(value: &Value) -> Result<(), AppError> {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                reject_identity_key(key)?;
                reject_identity_override(value)?;
            }
        }
        Value::Array(items) => {
            for item in items {
                reject_identity_override(item)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn reject_identity_key(key: &str) -> Result<(), AppError> {
    let root = key.split('.').next().unwrap_or(key);
    if matches!(
        root,
        "auth"
            | "authMode"
            | "accessToken"
            | "refreshToken"
            | "chatgptAccountId"
            | "apiKey"
            | "modelProvider"
            | "model_provider"
            | "model_providers"
            | "profiles"
            | "cli_auth_credentials_store"
            | "forced_login_method"
            | "codexHome"
            | "CODEX_HOME"
    ) {
        return Err(AppError::AccessDenied(
            "O harness não pode alterar o provedor ou a identidade gerenciada.".to_owned(),
        ));
    }
    Ok(())
}

pub fn apply_model_policy(
    message: &mut Value,
    status: &ApplicationServerStatus,
) -> Result<(), AppError> {
    if !message
        .get("method")
        .and_then(Value::as_str)
        .is_some_and(|method| {
            matches!(
                method,
                "thread/start" | "thread/resume" | "thread/fork" | "turn/start"
            )
        })
    {
        return Ok(());
    }
    let requested = [
        message.pointer("/params/model"),
        message.pointer("/params/config/model"),
    ];
    if requested
        .iter()
        .flatten()
        .filter_map(|value| value.as_str())
        .any(|model| !status.allowed_models.is_empty() && !status.allowed_models.contains(model))
    {
        return Err(AppError::AccessDenied(
            "Modelo não está na lista autorizada da sessão.".to_owned(),
        ));
    }
    if requested
        .iter()
        .all(|value| value.is_none_or(Value::is_null))
    {
        if let Some(model) = &status.default_model {
            message["params"]["model"] = Value::String(model.clone());
        } else if !status.allowed_models.is_empty() {
            return Err(AppError::Validation(
                "Escolha explicitamente um modelo autorizado nesta sessão.".to_owned(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn status() -> ApplicationServerStatus {
        ApplicationServerStatus {
            events_offset: 0,
            allowed_models: std::collections::BTreeSet::from(["allowed".to_owned()]),
            default_model: Some("allowed".to_owned()),
            id: "fixture".to_owned(),
            application_id: "application".to_owned(),
            grant_id: "grant".to_owned(),
            binding_id: "binding".to_owned(),
            binding_revision: 1,
            group_id: None,
            account_id: "account".to_owned(),
            state: ServerState::Ready,
            background: false,
            persist_history: false,
            created_at: chrono::Utc::now(),
            last_activity: chrono::Utc::now(),
            error_code: None,
        }
    }
    #[test]
    fn two_recoveries_cannot_replace_a_live_server_owner() {
        let sessions = ApplicationServers::default();
        let (_receiver, _cancel) = sessions.add(status()).expect("first owner");
        assert!(sessions.add(status()).is_err());
        assert_eq!(sessions.list().len(), 1);
    }
    #[test]
    fn model_policy_is_pinned_and_cannot_be_overridden_in_config() {
        let policy = status();
        let mut default = json!({"id":1,"method":"thread/start","params":{}});
        apply_model_policy(&mut default, &policy).expect("default");
        assert_eq!(default["params"]["model"], "allowed");
        assert!(apply_model_policy(&mut json!({"id":2,"method":"turn/start","params":{"model":"allowed","config":{"model":"outside"}}}), &policy).is_err());
        let mut no_default = policy;
        no_default.default_model = None;
        assert!(apply_model_policy(
            &mut json!({"id":1,"method":"thread/start","params":{}}),
            &no_default
        )
        .is_err());
    }
    #[test]
    fn harness_data_and_tools_are_not_confused_with_identity_overrides() {
        assert!(validate_harness_message(&json!({"id":1,"method":"turn/start","params":{"threadId":"owned","outputSchema":{"type":"object","properties":{"apiKey":{"type":"string"}}},"sandboxPolicy":{"type":"readOnly"}}})).is_ok());
        assert!(validate_harness_message(
            &json!({"id":8,"result":{"accessToken":"tool-data-not-auth"}})
        )
        .is_ok());
        for config in [
            json!({"model_provider":"external"}),
            json!({"model_providers.evil.base_url":"https://example.test"}),
            json!({"nested":{"CODEX_HOME":"/outside"}}),
        ] {
            assert!(validate_harness_message(
                &json!({"id":1,"method":"thread/start","params":{"config":config}})
            )
            .is_err());
        }
        assert!(validate_harness_message(
            &json!({"id":1,"method":"thread/resume","params":{"path":"/outside/history"}})
        )
        .is_err());
        assert!(
            validate_harness_message(&json!({"id":1,"method":"account/read","params":{}})).is_err()
        );
    }
    #[test]
    fn malformed_harness_parameters_fail_before_mutating_json_or_spawning_work() {
        for message in [
            json!([]),
            json!({"id":1,"method":"thread/start","params":[]}),
            json!({"id":1,"method":false}),
            json!({"id":1,"method":"thread/start","params":"invalid"}),
            json!({"id":1,"method":"thread/start","result":{}}),
        ] {
            assert!(validate_harness_message(&message).is_err());
        }
    }
}

#[cfg(any(unix, windows))]
use super::integration_protocol::Response;
#[cfg(any(unix, windows))]
use super::local_transport::{read_frame, write_frame, LocalStream, MAX_FRAME};
use crate::error::AppError;
#[cfg(any(unix, windows))]
use ed25519_dalek::{Signer, SigningKey};
#[cfg(any(unix, windows, test))]
use serde_json::{json, Value};
#[cfg(any(unix, windows))]
use std::io::{BufRead, Read, Write};
#[cfg(any(unix, windows))]
use std::time::Duration;

#[cfg(any(unix, windows, test))]
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ConsoleRequest {
    id: u64,
    method: String,
    #[serde(default)]
    params: Value,
}
#[cfg(any(unix, windows, test))]
fn validate_console_method(method: &str) -> Result<(), AppError> {
    if method.starts_with("authorization/") {
        return Err(AppError::AccessDenied(
            "Use client/connect; a CLI não exporta a chave de integração.".to_owned(),
        ));
    }
    Ok(())
}

/// Console bridge owns its integration key; stdout contains RPC results, never OAuth.
#[cfg(any(unix, windows))]
pub fn run() -> Result<(), AppError> {
    let mut stream = super::local_transport::connect_local()?;
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    let mut bound_application = None;
    loop {
        let mut line = Vec::new();
        let count = std::io::Read::by_ref(&mut input)
            .take((MAX_FRAME + 1) as u64)
            .read_until(b'\n', &mut line)?;
        if count == 0 {
            break;
        }
        if count > MAX_FRAME {
            return Err(AppError::Validation("Entrada excessiva.".to_owned()));
        }
        let mut request: ConsoleRequest = serde_json::from_slice(&line)?;
        let id = request.id;
        let result = if matches!(
            request.method.as_str(),
            "client/connect" | "client/authorize"
        ) {
            bind_application_identity(&mut request.params, bound_application.as_deref());
            connect(
                &mut stream,
                request.params,
                request.method == "client/authorize",
            )
        } else {
            validate_console_method(&request.method)
                .and_then(|()| call(&mut stream, id, &request.method, request.params))
        };
        if let Ok(receipt) = &result {
            if matches!(
                request.method.as_str(),
                "client/connect" | "client/authorize"
            ) && receipt["authenticated"] == true
            {
                bound_application = receipt["applicationId"].as_str().map(str::to_owned);
            }
        }
        serde_json::to_writer(&mut output, &Response::from_result(id, result))?;
        output.write_all(b"\n")?;
        output.flush()?;
    }
    Ok(())
}

#[cfg(any(unix, windows, test))]
fn bind_application_identity(params: &mut Value, bound: Option<&str>) {
    if params.get("applicationId").is_none_or(Value::is_null) {
        if let (Some(object), Some(id)) = (params.as_object_mut(), bound) {
            object.insert("applicationId".to_owned(), Value::String(id.to_owned()));
        }
    }
}

#[cfg(not(any(unix, windows)))]
pub fn run() -> Result<(), AppError> {
    Err(AppError::AccessDenied(
        "Transporte local não disponível nesta plataforma.".to_owned(),
    ))
}

#[cfg(any(unix, windows))]
fn call(
    stream: &mut impl LocalStream,
    id: u64,
    method: &str,
    params: Value,
) -> Result<Value, AppError> {
    write_frame(stream, &json!({"id":id,"method":method,"params":params}))?;
    let response: Response = read_frame(stream, Duration::from_secs(180))?;
    if response.id != id {
        return Err(AppError::Internal("Correlação RPC inválida.".to_owned()));
    }
    match (response.result, response.error) {
        (Some(value), None) => Ok(value),
        (_, Some(error)) => Err(AppError::Remote {
            code: error.code,
            message: error.message,
        }),
        _ => Err(AppError::Internal("Resposta inválida.".to_owned())),
    }
}

#[cfg(any(unix, windows))]
fn connect(
    stream: &mut impl LocalStream,
    params: Value,
    fresh_consent: bool,
) -> Result<Value, AppError> {
    let input: super::authorization::ConnectionRequest = serde_json::from_value(params.clone())?;
    if fresh_consent || input.application_id.is_none() {
        return authorize_connection(stream, params);
    }
    let application_id = input
        .application_id
        .as_deref()
        .ok_or_else(|| AppError::Validation("ID da aplicação ausente.".to_owned()))?;
    let entry = integration_entry(application_id)?;
    let credential = match entry.get_password() {
        Ok(value) => serde_json::from_str::<Value>(&value)?,
        Err(keyring_core::Error::NoEntry) => return authorize_connection(stream, params),
        Err(_) => return Err(AppError::CredentialUnavailable("Não foi possível acessar a autorização; nenhuma nova conexão será aprovada automaticamente.".to_owned())),
    };
    if credential["applicationId"].as_str() != Some(application_id) {
        return Err(AppError::AccessDenied(
            "A autorização salva pertence a outra aplicação.".to_owned(),
        ));
    }
    let challenge = call(
        stream,
        3,
        "authorization/challenge",
        json!({"grantId":credential["grantId"]}),
    )?;
    prove(stream, &credential, &challenge)?;
    Ok(connection_receipt(&credential))
}

#[cfg(any(unix, windows))]
fn authorize_connection(stream: &mut impl LocalStream, params: Value) -> Result<Value, AppError> {
    let request = call(stream, 1, "authorization/request", params)?;
    eprintln!(
        "Aguardando aprovação no gerenciador. Código de conferência: {}",
        request["confirmationCode"].as_str().unwrap_or("")
    );
    loop {
        let claim = call(
            stream,
            2,
            "authorization/claim",
            json!({"requestId":request["id"]}),
        )?;
        if claim.get("pending").and_then(Value::as_bool) == Some(true) {
            std::thread::sleep(Duration::from_millis(500));
            continue;
        }
        let challenge = call(
            stream,
            3,
            "authorization/challenge",
            json!({"grantId":claim["grantId"]}),
        )?;
        prove(stream, &claim, &challenge)?;
        if claim["permanent"] == true
            || claim.get("expiresAt").is_some_and(|value| !value.is_null())
        {
            let application_id = claim["applicationId"]
                .as_str()
                .ok_or_else(|| AppError::Internal("Aprovação sem ID de aplicação.".to_owned()))?;
            integration_entry(application_id)?
                .set_password(&serde_json::to_string(&claim)?)
                .map_err(|_| {
                    AppError::CredentialUnavailable(
                        "Não foi possível guardar a autorização.".to_owned(),
                    )
                })?;
        }
        return Ok(connection_receipt(&claim));
    }
}

#[cfg(any(unix, windows, test))]
fn connection_receipt(credential: &Value) -> Value {
    json!({"authenticated":true,"grantId":credential["grantId"],"applicationId":credential["applicationId"],"permanent":credential["permanent"],"expiresAt":credential["expiresAt"]})
}

#[cfg(any(unix, windows, test))]
fn integration_entry(
    application_id: &str,
) -> Result<crate::adapters::native_keyring::Entry, AppError> {
    uuid::Uuid::parse_str(application_id)
        .map_err(|_| AppError::Validation("ID de aplicação inválido.".to_owned()))?;
    // A user-supplied display name/slug is never a credential lookup key.
    crate::adapters::native_keyring::Entry::new(
        "com.codex.account-manager.integrations",
        application_id,
    )
    .map_err(|_| AppError::CredentialUnavailable("Cofre indisponível.".to_owned()))
}

#[cfg(any(unix, windows))]
fn prove(
    stream: &mut impl LocalStream,
    credential: &Value,
    challenge: &Value,
) -> Result<(), AppError> {
    let secret: [u8; 32] = credential
        .get("signingKey")
        .and_then(Value::as_str)
        .and_then(|value| hex::decode(value).ok())
        .and_then(|value| value.try_into().ok())
        .ok_or_else(|| AppError::AccessDenied("Credencial de integração inválida.".to_owned()))?;
    let key = SigningKey::from_bytes(&secret);
    let text = challenge
        .get("challenge")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::Internal("Desafio ausente.".to_owned()))?;
    call(
        stream,
        4,
        "authorization/prove",
        json!({"proof":hex::encode(key.sign(text.as_bytes()).to_bytes())}),
    )?;
    Ok(())
}
#[cfg(test)]
mod tests {
    #[test]
    fn later_requests_use_receipt_identity_but_explicit_ids_are_preserved() {
        let mut implicit = serde_json::json!({"applicationId":null,"slug":"not-an-identity"});
        super::bind_application_identity(&mut implicit, Some("authenticated-id"));
        assert_eq!(implicit["applicationId"], "authenticated-id");
        let mut explicit = serde_json::json!({"applicationId":"another-id"});
        super::bind_application_identity(&mut explicit, Some("authenticated-id"));
        assert_eq!(explicit["applicationId"], "another-id");
        let mut first = serde_json::json!({});
        super::bind_application_identity(&mut first, None);
        assert!(first.get("applicationId").is_none());
    }
    #[test]
    fn console_never_returns_raw_authorization_secrets() {
        for method in [
            "authorization/request",
            "authorization/claim",
            "authorization/challenge",
            "authorization/prove",
        ] {
            assert!(super::validate_console_method(method).is_err());
        }
        assert!(super::validate_console_method("execution/events").is_ok());
        assert!(serde_json::from_str::<super::ConsoleRequest>(
            r#"{"id":1,"method":"catalog/read","secret":"injected"}"#
        )
        .is_err());
    }
    #[test]
    fn names_are_not_credential_lookup_identifiers_and_receipts_never_include_keys() {
        assert!(super::integration_entry("known-app-slug").is_err());
        let receipt = super::connection_receipt(
            &serde_json::json!({"grantId":"opaque-grant","applicationId":"opaque-application","permanent":true,"signingKey":"synthetic-private"}),
        );
        assert!(receipt.get("signingKey").is_none());
        assert!(!receipt.to_string().contains("synthetic-private"));
    }
}

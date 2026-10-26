use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("Codex Desktop: {0}")]
    DesktopDiscovery(String),
    #[error("fechamento do Codex: {0}")]
    DesktopClosePending(String),
    #[error("abertura do Codex: {0}")]
    DesktopLaunch(String),
    #[error("integração: {message}")]
    Remote { code: String, message: String },
    #[error("saldo: {0}")]
    QuotaExhausted(String),
    #[error("acesso: {0}")]
    AccessDenied(String),
    #[error("consentimento necessário: {0}")]
    ConsentRequired(String),
    #[error("validação: {0}")]
    Validation(String),
    #[error("perfil não encontrado: {0}")]
    ProfileNotFound(String),
    #[error("operação não encontrada: {0}")]
    OperationNotFound(String),
    #[error("operação já finalizada")]
    OperationFinished,
    #[error("operação cancelada")]
    Cancelled,
    #[error("tempo esgotado aguardando login")]
    TimedOut,
    #[error("navegador: {0}")]
    Browser(String),
    #[error("runtime do navegador: {0}")]
    Runtime(String),
    #[error("App Server: {0}")]
    AppServer(String),
    #[error("App Server ({code}): {message}")]
    Rpc { code: i64, message: String },
    #[error("runtime compartilhado: {0}")]
    RuntimeBusy(String),
    #[error("sessão: {0}")]
    AuthenticationRequired(String),
    #[error("sessão a renovar: {0}")]
    SessionRenewalRequired(String),
    #[error("conta diferente: {0}")]
    AccountMismatch(String),
    #[error("cofre indisponível: {0}")]
    CredentialUnavailable(String),
    #[error("recuperação da sessão: {0}")]
    CredentialRecoveryPending(String),
    #[error("operação em andamento: {0}")]
    Busy(String),
    #[error("contexto da consulta alterado: {0}")]
    QueryContextChanged(String),
    #[error("Keyring: {0}")]
    Keyring(String),
    #[error("atualização: {0}")]
    Update(String),
    #[error("banco local: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("pool do banco local: {0}")]
    DatabasePool(String),
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("erro interno: {0}")]
    Internal(String),
}

impl AppError {
    pub fn code(&self) -> &str {
        match self {
            Self::DesktopDiscovery(_) => "desktop_discovery_failed",
            Self::DesktopClosePending(_) => "desktop_close_pending",
            Self::DesktopLaunch(_) => "desktop_launch_failed",
            Self::Remote { code, .. } => code,
            Self::QuotaExhausted(_) => "quota_exhausted",
            Self::AccessDenied(_) => "access_denied",
            Self::ConsentRequired(_) => "consent_required",
            Self::Validation(_) => "validation_error",
            Self::ProfileNotFound(_) => "profile_not_found",
            Self::OperationNotFound(_) => "operation_not_found",
            Self::OperationFinished => "operation_finished",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
            Self::Browser(_) => "browser_error",
            Self::Runtime(_) => "runtime_error",
            Self::AppServer(_) => "app_server_error",
            Self::Rpc {
                code: -32600,
                message,
            } if message.contains("active writer") => "thread_owned_elsewhere",
            Self::Rpc { .. } => "rpc_error",
            Self::RuntimeBusy(_) => "runtime_busy",
            Self::AuthenticationRequired(_) => "authentication_required",
            Self::SessionRenewalRequired(_) => "session_renewal_required",
            Self::AccountMismatch(_) => "account_mismatch",
            Self::CredentialUnavailable(_) => "credential_unavailable",
            Self::CredentialRecoveryPending(_) => "credential_recovery_pending",
            Self::Busy(_) => "resource_busy",
            Self::QueryContextChanged(_) => "query_context_changed",
            Self::Keyring(_) => "keyring_error",
            Self::Update(_) => "update_error",
            Self::Database(_) => "database_error",
            Self::DatabasePool(_) => "database_pool_error",
            Self::Io(_) => "io_error",
            Self::Json(_) => "json_error",
            Self::Internal(_) => "internal_error",
        }
    }

    pub fn is_authentication_failure(&self) -> bool {
        match self {
            Self::AuthenticationRequired(_) => true,
            Self::SessionRenewalRequired(_) => true,
            Self::AppServer(message) => authentication_message(message),
            Self::Rpc { message, .. } => authentication_message(message),
            _ => false,
        }
    }

    pub fn is_refresh_rejected(&self) -> bool {
        // A native OS/path/coffer error is not evidence about the OAuth chain,
        // even when an unrelated message happens to contain a token marker.
        if !matches!(
            self,
            Self::AuthenticationRequired(_)
                | Self::SessionRenewalRequired(_)
                | Self::Rpc { .. }
                | Self::AppServer(_)
        ) {
            return false;
        }
        let text = self.to_string().to_ascii_lowercase();
        [
            "token_revoked",
            "refresh_token_reused",
            "refresh_token_expired",
            "refresh_token_invalidated",
            "refresh_token_invalid",
            "invalid_grant",
            "invalidated oauth token",
        ]
        .iter()
        .any(|marker| text.contains(marker))
    }
}

fn authentication_message(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "token_revoked",
        "refresh_token_reused",
        "refresh_token_expired",
        "refresh_token_invalidated",
        "invalidated oauth token",
        "invalid_grant",
        "unauthorized",
        "401 unauthorized",
    ]
    .iter()
    .any(|marker| message.contains(marker))
}

impl serde::Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut error = serializer.serialize_struct("CommandError", 3)?;
        error.serialize_field("code", self.code())?;
        error.serialize_field(
            "message",
            &crate::adapters::logging::sanitize_error(&self.to_string()),
        )?;
        let rpc_code = match self {
            Self::Rpc { code, .. } => Some(*code),
            _ => None,
        };
        error.serialize_field("rpcCode", &rpc_code)?;
        error.end()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn native_runtime_errors_cannot_quarantine_an_oauth_session_by_text_coincidence() {
        assert!(!super::AppError::DesktopLaunch(
            "invalid_grant in synthetic native message".to_owned()
        )
        .is_refresh_rejected());
        assert!(!super::AppError::Keyring(
            "refresh_token_reused in synthetic coffer message".to_owned()
        )
        .is_refresh_rejected());
        assert!(super::AppError::Rpc {
            code: 401,
            message: "refresh_token_reused".to_owned()
        }
        .is_refresh_rejected());
    }
    #[test]
    fn ipc_error_has_an_actionable_code_without_secret_parameters() {
        let error = super::AppError::Rpc {
            code: -32600,
            message: "already has an active writer https://example.test/callback?code=private"
                .to_owned(),
        };
        let value = serde_json::to_value(error).expect("error");
        assert_eq!(value["code"], "thread_owned_elsewhere");
        assert_eq!(value["rpcCode"], -32600);
        assert!(!value.to_string().contains("private"));
    }
}

use super::run_blocking;
use crate::application::authorization::AuthorizationRequest;
use crate::application::manager::ManagerCore;
use crate::domain::integrations::{AuthorizationSetup, GrantScope};
use crate::error::AppError;
use tauri::{State, WebviewWindow};

pub(super) fn trusted_ui(window: &WebviewWindow) -> Result<(), AppError> {
    if window.label() != "main" {
        return Err(AppError::AccessDenied(
            "Aprovação somente na interface principal.".to_owned(),
        ));
    }
    let url = window
        .url()
        .map_err(|error| AppError::Internal(error.to_string()))?;
    let bundled = (url.scheme() == "tauri" && url.host_str() == Some("localhost"))
        || url.scheme() == "http" && url.host_str() == Some("tauri.localhost");
    let development = cfg!(debug_assertions)
        && url.scheme() == "http"
        && url.host_str() == Some("127.0.0.1")
        && url.port() == Some(1420);
    if !bundled && !development {
        return Err(AppError::AccessDenied(
            "Origem de aprovação inválida.".to_owned(),
        ));
    }
    Ok(())
}
#[tauri::command]
pub async fn list_authorization_requests(
    window: WebviewWindow,
    state: State<'_, ManagerCore>,
) -> Result<Vec<AuthorizationRequest>, AppError> {
    trusted_ui(&window)?;
    Ok(state.authorization_requests())
}
#[tauri::command]
pub async fn approve_authorization(
    window: WebviewWindow,
    state: State<'_, ManagerCore>,
    id: String,
    scope: GrantScope,
    setup: Option<AuthorizationSetup>,
) -> Result<String, AppError> {
    trusted_ui(&window)?;
    let core = state.inner().clone();
    run_blocking(move || core.approve_authorization_setup(&id, scope, setup.unwrap_or_default()))
        .await
}
#[tauri::command]
pub async fn deny_authorization(
    window: WebviewWindow,
    state: State<'_, ManagerCore>,
    id: String,
) -> Result<(), AppError> {
    trusted_ui(&window)?;
    state.deny_authorization(&id);
    Ok(())
}
#[tauri::command]
pub async fn revoke_authorization(
    window: WebviewWindow,
    state: State<'_, ManagerCore>,
    id: String,
) -> Result<(), AppError> {
    trusted_ui(&window)?;
    let core = state.inner().clone();
    run_blocking(move || core.revoke_authorization(&id)).await
}

use super::run_blocking;

#[tauri::command]
pub async fn clear_application_history(
    window: tauri::WebviewWindow,
    state: State<'_, ManagerCore>,
    id: String,
) -> Result<(), AppError> {
    super::authorization::trusted_ui(&window)?;
    let core = state.inner().clone();
    run_blocking(move || core.clear_application_history(&id)).await
}
#[tauri::command]
pub async fn export_application_history(
    window: tauri::WebviewWindow,
    state: State<'_, ManagerCore>,
    id: String,
) -> Result<String, AppError> {
    super::authorization::trusted_ui(&window)?;
    let core = state.inner().clone();
    run_blocking(move || core.export_application_history(&id)).await
}

#[tauri::command]
pub async fn recover_application_credential(
    state: State<'_, ManagerCore>,
    profile_id: String,
) -> Result<(), AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.recover_application_credential(&profile_id)).await
}
#[tauri::command]
pub async fn list_application_servers(
    state: State<'_, ManagerCore>,
) -> Result<Vec<crate::domain::application_server::ApplicationServerStatus>, AppError> {
    Ok(state.list_application_servers())
}
#[tauri::command]
pub async fn close_application_server(
    state: State<'_, ManagerCore>,
    id: String,
) -> Result<(), AppError> {
    state.close_application_server(&id)
}
#[tauri::command]
pub async fn list_executions(
    state: State<'_, ManagerCore>,
) -> Result<Vec<crate::domain::execution::ExecutionStatus>, AppError> {
    Ok(state.list_executions())
}
#[tauri::command]
pub async fn cancel_execution(state: State<'_, ManagerCore>, id: String) -> Result<(), AppError> {
    state.cancel_execution(&id)
}
#[tauri::command]
pub async fn start_application_login(
    state: State<'_, ManagerCore>,
    profile_id: String,
) -> Result<crate::domain::LoginOperation, AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.start_application_login(&profile_id)).await
}
use crate::application::manager::ManagerCore;
use crate::domain::integrations::*;
use crate::error::AppError;
use tauri::State;

#[tauri::command]
pub async fn get_integrations(
    state: State<'_, ManagerCore>,
) -> Result<IntegrationSnapshot, AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.integration_snapshot()).await
}
#[tauri::command]
pub async fn save_account_group(
    state: State<'_, ManagerCore>,
    group: AccountGroup,
) -> Result<AccountGroup, AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.save_group(&group)).await
}
#[tauri::command]
pub async fn delete_account_group(
    state: State<'_, ManagerCore>,
    id: String,
    revision: u64,
) -> Result<(), AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.delete_group(&id, revision)).await
}
#[tauri::command]
pub async fn save_application(
    state: State<'_, ManagerCore>,
    application: Application,
) -> Result<Application, AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.save_application(&application)).await
}
#[tauri::command]
pub async fn save_execution_binding(
    state: State<'_, ManagerCore>,
    binding: ExecutionBinding,
) -> Result<ExecutionBinding, AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.save_binding(&binding)).await
}
#[tauri::command]
pub async fn set_account_policy(
    state: State<'_, ManagerCore>,
    id: String,
    policy: AccountPolicy,
    capacity: u32,
) -> Result<(), AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.set_account_policy(&id, policy, capacity)).await
}
#[tauri::command]
pub async fn save_mail_preference(
    state: State<'_, ManagerCore>,
    profile_id: String,
    enabled: bool,
    provider_id: Option<String>,
) -> Result<(), AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.save_mail_preference(&profile_id, enabled, provider_id.as_deref()))
        .await
}

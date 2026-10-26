use tauri::{AppHandle, State};
pub mod authorization;
pub mod deletion;
pub mod integrations;

use crate::application::manager::ManagerCore;
use crate::domain::ConnectionCheck;
use crate::domain::{
    DashboardSnapshot, LoginOperation, MailProvider, ManagedProfile, ManagerSettings,
    OperationStage, ProfileOptions, ReplacementPreview, RuntimeStatus, SaveMailProviderInput,
    UpdateInfo,
};
use crate::error::AppError;

#[tauri::command]
pub async fn retry_browser_cleanup(
    state: State<'_, ManagerCore>,
    operation_id: String,
) -> Result<(), AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.retry_browser_cleanup(&operation_id)).await
}

#[tauri::command]
pub async fn recover_saved_accounts(state: State<'_, ManagerCore>) -> Result<usize, AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.recover_saved_accounts()).await
}

#[tauri::command]
pub async fn focus_codex(
    state: State<'_, ManagerCore>,
    operation_id: Option<String>,
) -> Result<(), AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.open_desktop(operation_id.as_deref())).await
}

#[tauri::command]
pub async fn select_codex_desktop(
    window: tauri::WebviewWindow,
    state: State<'_, ManagerCore>,
) -> Result<bool, AppError> {
    #[cfg(windows)]
    let owner = window
        .hwnd()
        .map_err(|error| AppError::DesktopDiscovery(error.to_string()))?
        .0 as isize;
    #[cfg(not(windows))]
    let owner = {
        let _ = window;
        0
    };
    let core = state.inner().clone();
    run_blocking(move || core.select_desktop(owner)).await
}

#[tauri::command]
pub async fn confirm_codex_close(
    window: tauri::WebviewWindow,
    state: State<'_, ManagerCore>,
    operation_id: String,
) -> Result<bool, AppError> {
    #[cfg(windows)]
    let owner = window
        .hwnd()
        .map_err(|error| AppError::DesktopClosePending(error.to_string()))?
        .0 as isize;
    #[cfg(not(windows))]
    let owner = {
        let _ = window;
        0
    };
    let core = state.inner().clone();
    run_blocking(move || core.confirm_desktop_close(&operation_id, owner)).await
}

#[tauri::command]
pub async fn inspect_shared_runtime(
    state: State<'_, ManagerCore>,
) -> Result<crate::domain::SharedRuntimeSnapshot, AppError> {
    let core = state.inner().clone();
    run_blocking(move || Ok(core.shared_runtime_snapshot())).await
}

#[tauri::command]
pub async fn recover_original_chat(
    state: State<'_, ManagerCore>,
    thread: String,
    consent: Option<crate::domain::RuntimeConsent>,
) -> Result<crate::domain::RuntimeRecoveryReport, AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.recover_chat(&thread, consent)).await
}

#[tauri::command]
pub async fn open_original_chat(
    state: State<'_, ManagerCore>,
    thread: String,
) -> Result<(), AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.open_original_chat(&thread)).await
}

#[tauri::command]
pub async fn list_profiles(state: State<'_, ManagerCore>) -> Result<Vec<ManagedProfile>, AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.list_profiles()).await
}

#[tauri::command]
pub async fn get_manager_settings(
    state: State<'_, ManagerCore>,
) -> Result<ManagerSettings, AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.settings()).await
}

#[tauri::command]
pub async fn save_manager_settings(
    state: State<'_, ManagerCore>,
    settings: ManagerSettings,
) -> Result<(), AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.save_settings(&settings)).await
}

#[tauri::command]
pub async fn save_profile_options(
    state: State<'_, ManagerCore>,
    profile_id: String,
    options: ProfileOptions,
) -> Result<(), AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.save_profile_options(&profile_id, &options)).await
}

#[tauri::command]
pub async fn open_profile_browser(
    state: State<'_, ManagerCore>,
    profile_id: String,
) -> Result<(), AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.open_profile_browser(&profile_id)).await
}

#[tauri::command]
pub async fn check_profile(
    state: State<'_, ManagerCore>,
    profile_id: String,
) -> Result<ConnectionCheck, AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.check_profile(&profile_id)).await
}

#[tauri::command]
pub async fn get_dashboard_snapshot(
    state: State<'_, ManagerCore>,
) -> Result<DashboardSnapshot, AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.dashboard_snapshot()).await
}

#[tauri::command]
pub async fn list_mail_providers(
    state: State<'_, ManagerCore>,
) -> Result<Vec<MailProvider>, AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.list_providers()).await
}

#[tauri::command]
pub async fn save_mail_provider(
    state: State<'_, ManagerCore>,
    input: SaveMailProviderInput,
) -> Result<MailProvider, AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.save_mail_provider(input)).await
}

#[tauri::command]
pub async fn list_operations(
    state: State<'_, ManagerCore>,
) -> Result<Vec<LoginOperation>, AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.list_operations()).await
}

#[tauri::command]
pub async fn get_operation_stages(
    state: State<'_, ManagerCore>,
    operation_id: String,
) -> Result<Vec<OperationStage>, AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.operation_stages(&operation_id)).await
}

#[tauri::command]
pub async fn get_runtime_status(state: State<'_, ManagerCore>) -> Result<RuntimeStatus, AppError> {
    let core = state.inner().clone();
    run_blocking(move || Ok(core.runtime_status())).await
}

#[tauri::command]
pub async fn create_profile(
    state: State<'_, ManagerCore>,
    name: String,
    provider_id: Option<String>,
    mail_url: Option<String>,
) -> Result<ManagedProfile, AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.create_profile(&name, provider_id.as_deref(), mail_url.as_deref()))
        .await
}

#[tauri::command]
pub async fn rename_profile(
    state: State<'_, ManagerCore>,
    profile_id: String,
    name: String,
) -> Result<ManagedProfile, AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.rename_profile(&profile_id, &name)).await
}

#[tauri::command]
pub async fn update_profile_mail_url(
    state: State<'_, ManagerCore>,
    profile_id: String,
    mail_url: Option<String>,
) -> Result<ManagedProfile, AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.update_profile_mail_url(&profile_id, mail_url.as_deref())).await
}

#[tauri::command]
pub async fn archive_profile(
    state: State<'_, ManagerCore>,
    profile_id: String,
) -> Result<ManagedProfile, AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.archive_profile(&profile_id)).await
}

#[tauri::command]
pub async fn restore_profile(
    state: State<'_, ManagerCore>,
    profile_id: String,
) -> Result<ManagedProfile, AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.restore_profile(&profile_id)).await
}

#[tauri::command]
pub async fn delete_profile(
    state: State<'_, ManagerCore>,
    profile_id: String,
    permanently: bool,
) -> Result<(), AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.delete_profile(&profile_id, permanently)).await
}

#[tauri::command]
pub async fn open_mail(state: State<'_, ManagerCore>, profile_id: String) -> Result<(), AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.open_mail(&profile_id)).await
}

#[tauri::command]
pub async fn open_profile_folder(
    state: State<'_, ManagerCore>,
    profile_id: String,
) -> Result<(), AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.open_profile_folder(&profile_id)).await
}

#[tauri::command]
pub async fn start_login(
    state: State<'_, ManagerCore>,
    profile_id: String,
    reconnect: bool,
    mail_url: Option<String>,
) -> Result<LoginOperation, AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.start_login(&profile_id, reconnect, mail_url.as_deref())).await
}

#[tauri::command]
pub async fn cancel_operation(
    state: State<'_, ManagerCore>,
    operation_id: String,
) -> Result<(), AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.cancel_operation(&operation_id)).await
}

#[tauri::command]
pub async fn export_diagnostics(state: State<'_, ManagerCore>) -> Result<String, AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.export_diagnostics()).await
}

#[tauri::command]
pub async fn start_switch_account(
    state: State<'_, ManagerCore>,
    profile_id: String,
) -> Result<LoginOperation, AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.start_switch_account(&profile_id)).await
}

#[tauri::command]
pub async fn check_for_update(app: AppHandle) -> Result<Option<UpdateInfo>, AppError> {
    crate::adapters::update_service::check(&app).await
}

#[tauri::command]
pub async fn install_update(app: AppHandle, expected_version: String) -> Result<(), AppError> {
    crate::adapters::update_service::install(&app, &expected_version).await
}

#[tauri::command]
pub async fn unlink_profile(
    state: State<'_, ManagerCore>,
    profile_id: String,
) -> Result<(), AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.unlink_profile(&profile_id)).await
}

#[tauri::command]
pub async fn logout_profile(
    state: State<'_, ManagerCore>,
    profile_id: String,
) -> Result<(), AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.logout_profile(&profile_id)).await
}

#[tauri::command]
pub async fn start_replacement(
    state: State<'_, ManagerCore>,
    profile_id: String,
) -> Result<LoginOperation, AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.start_replacement(&profile_id)).await
}

#[tauri::command]
pub async fn get_replacement_preview(
    state: State<'_, ManagerCore>,
    operation_id: String,
) -> Result<Option<ReplacementPreview>, AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.replacement_preview(&operation_id)).await
}

#[tauri::command]
pub async fn confirm_replacement(
    state: State<'_, ManagerCore>,
    operation_id: String,
) -> Result<(), AppError> {
    let core = state.inner().clone();
    run_blocking(move || core.confirm_replacement(&operation_id)).await
}

async fn run_blocking<T, F>(operation: F) -> Result<T, AppError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, AppError> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(operation)
        .await
        .map_err(|error| AppError::Internal(format!("tarefa local interrompida: {error}")))?
}

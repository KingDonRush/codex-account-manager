use super::{authorization::trusted_ui, run_blocking};
use crate::application::manager::ManagerCore;
use crate::domain::deletion::DeletionRequest;
use crate::error::AppError;
use std::collections::BTreeSet;
use tauri::{State, WebviewWindow};
#[tauri::command]
pub async fn list_deletion_requests(
    window: WebviewWindow,
    state: State<'_, ManagerCore>,
) -> Result<Vec<DeletionRequest>, AppError> {
    trusted_ui(&window)?;
    Ok(state.pending_deletions())
}
#[tauri::command]
pub async fn request_account_deletion(
    window: WebviewWindow,
    state: State<'_, ManagerCore>,
    account_ids: BTreeSet<String>,
) -> Result<DeletionRequest, AppError> {
    trusted_ui(&window)?;
    let core = state.inner().clone();
    run_blocking(move || core.request_deletion(None, &account_ids)).await
}
#[tauri::command]
pub async fn confirm_deletion(
    window: WebviewWindow,
    state: State<'_, ManagerCore>,
    id: String,
    selected: BTreeSet<String>,
    remove_browser: bool,
) -> Result<DeletionRequest, AppError> {
    trusted_ui(&window)?;
    let core = state.inner().clone();
    run_blocking(move || core.confirm_deletion(&id, selected, remove_browser)).await
}
#[tauri::command]
pub async fn deny_deletion(
    window: WebviewWindow,
    state: State<'_, ManagerCore>,
    id: String,
) -> Result<(), AppError> {
    trusted_ui(&window)?;
    state.deny_deletion(&id)
}

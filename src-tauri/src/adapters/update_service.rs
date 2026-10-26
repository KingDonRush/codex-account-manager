use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::domain::{UpdateInfo, UpdateProgress};
use crate::error::AppError;

const COMPILED_ENDPOINT: Option<&str> = option_env!("CODEX_MANAGER_UPDATE_ENDPOINT");

pub async fn check(app: &AppHandle) -> Result<Option<UpdateInfo>, AppError> {
    let update = find_update(app).await?;
    Ok(update.map(|release| UpdateInfo {
        current_version: release.current_version,
        version: release.version,
        body: release.body,
        published_at: release.date.map(|value| value.to_string()),
    }))
}

pub async fn install(app: &AppHandle, expected_version: &str) -> Result<(), AppError> {
    let update = find_update(app).await?.ok_or_else(|| {
        AppError::Update("nenhuma atualização mais recente está disponível".to_owned())
    })?;
    if update.version != expected_version {
        return Err(AppError::Update(
            "a versão disponível mudou; verifique novamente antes de instalar".to_owned(),
        ));
    }
    download_and_install(app, update).await
}

async fn find_update(app: &AppHandle) -> Result<Option<Update>, AppError> {
    let endpoint = configured_endpoint()?;
    let updater = app
        .updater_builder()
        .endpoints(vec![endpoint])
        .map_err(update_error)?
        .build()
        .map_err(update_error)?;
    updater.check().await.map_err(update_error)
}

async fn download_and_install(app: &AppHandle, update: Update) -> Result<(), AppError> {
    let downloaded = Arc::new(AtomicU64::new(0));
    let progress_app = app.clone();
    let progress_downloaded = downloaded.clone();
    update
        .download_and_install(
            move |chunk_size, total_bytes| {
                let chunk_size = u64::try_from(chunk_size).unwrap_or(u64::MAX);
                let downloaded_bytes =
                    progress_downloaded.fetch_add(chunk_size, Ordering::AcqRel) + chunk_size;
                let _ = progress_app.emit(
                    "update-progress",
                    UpdateProgress {
                        downloaded_bytes,
                        total_bytes,
                    },
                );
            },
            || {},
        )
        .await
        .map_err(update_error)
}

fn configured_endpoint() -> Result<url::Url, AppError> {
    let value = std::env::var("CODEX_MANAGER_UPDATE_ENDPOINT")
        .ok()
        .or_else(|| COMPILED_ENDPOINT.map(str::to_owned))
        .ok_or_else(|| {
            AppError::Update("fonte de atualizações não configurada nesta distribuição".to_owned())
        })?;
    let parsed = url::Url::parse(&value)
        .map_err(|_| AppError::Update("fonte de atualizações inválida".to_owned()))?;
    if parsed.scheme() != "https" || !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(AppError::Update(
            "a fonte de atualizações deve usar HTTPS e não pode conter credenciais".to_owned(),
        ));
    }
    Ok(parsed)
}

fn update_error(error: impl std::fmt::Display) -> AppError {
    AppError::Update(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::configured_endpoint;

    #[test]
    fn updater_endpoint_never_accepts_embedded_credentials() {
        std::env::set_var(
            "CODEX_MANAGER_UPDATE_ENDPOINT",
            "https://user:password@example.com/latest.json",
        );
        let result = configured_endpoint();
        std::env::remove_var("CODEX_MANAGER_UPDATE_ENDPOINT");
        assert!(result.is_err());
    }
}

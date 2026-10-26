//! Only artifacts beneath a persisted manager session UUID are eligible.
use crate::error::AppError;
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};
use zip::{write::SimpleFileOptions, ZipWriter};
const EXPORT_LIMIT: u64 = 128 * 1024 * 1024;
struct ExportBudget {
    remaining: u64,
    entries: usize,
}

pub fn session_directory(app_data: &Path, id: &str) -> Result<PathBuf, AppError> {
    uuid::Uuid::parse_str(id)
        .map_err(|_| AppError::Validation("ID de sessão inválido.".to_owned()))?;
    let root = app_data.join("application-servers");
    if root.exists() {
        reject_link(&root)?;
    }
    let path = root.join(id);
    if path.exists() {
        reject_link(&path)?;
    }
    Ok(path)
}
fn reject_link(path: &Path) -> Result<(), AppError> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(AppError::AccessDenied(
            "Histórico não é diretório próprio.".to_owned(),
        ));
    }
    Ok(())
}
pub fn clear(app_data: &Path, id: &str) -> Result<(), AppError> {
    let directory = session_directory(app_data, id)?;
    if !directory.exists() {
        return Ok(());
    }
    // remove_dir_all does not follow descendant symlinks; the root is validated above.
    std::fs::remove_dir_all(directory)?;
    Ok(())
}
pub fn remove_events(app_data: &Path, id: &str) -> Result<(), AppError> {
    let directory = session_directory(app_data, id)?;
    match std::fs::remove_file(directory.join("events.jsonl")) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}
pub fn export(app_data: &Path, id: &str, export_id: &str) -> Result<PathBuf, AppError> {
    let directory = session_directory(app_data, id)?;
    let destination = export_path(app_data, export_id)?;
    if directory.join("runtime").exists() {
        reject_link(&directory.join("runtime"))?;
    }
    let parent = destination
        .parent()
        .ok_or_else(|| AppError::Internal("Exportação sem pasta.".to_owned()))?;
    std::fs::create_dir_all(parent)?;
    reject_link(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    let mut temp = tempfile::Builder::new()
        .prefix(".history-")
        .tempfile_in(parent)?;
    let mut archive = ZipWriter::new(temp.as_file_mut());
    let mut budget = ExportBudget {
        remaining: EXPORT_LIMIT,
        entries: 0,
    };
    add_file(
        &mut archive,
        &directory,
        &directory.join("events.jsonl"),
        &mut budget.remaining,
    )?;
    for name in ["sessions", "archived_sessions"] {
        add_rollouts(
            &mut archive,
            &directory,
            &directory.join("runtime").join(name),
            &mut budget,
        )?;
    }
    archive
        .finish()
        .map_err(|_| AppError::Internal("Não foi possível concluir a exportação.".to_owned()))?;
    temp.as_file().sync_all()?;
    temp.persist_noclobber(&destination)
        .map_err(|error| AppError::Io(error.error))?;
    Ok(destination)
}
pub fn export_path(app_data: &Path, id: &str) -> Result<PathBuf, AppError> {
    uuid::Uuid::parse_str(id)
        .map_err(|_| AppError::Validation("ID de exportação inválido.".to_owned()))?;
    Ok(app_data.join("history-exports").join(format!("{id}.zip")))
}
fn add_rollouts(
    archive: &mut ZipWriter<&mut std::fs::File>,
    root: &Path,
    folder: &Path,
    budget: &mut ExportBudget,
) -> Result<(), AppError> {
    if !folder.exists() {
        return Ok(());
    }
    reject_link(folder)?;
    if folder
        .components()
        .count()
        .saturating_sub(root.components().count())
        > 32
    {
        return Err(AppError::Validation(
            "Histórico excede profundidade permitida.".to_owned(),
        ));
    }
    for entry in std::fs::read_dir(folder)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err(AppError::AccessDenied(
                "Exportação recusou um link externo.".to_owned(),
            ));
        }
        budget.entries += 1;
        if budget.entries > 1000 {
            return Err(AppError::Validation(
                "Histórico excede limite de 1.000 arquivos/pastas.".to_owned(),
            ));
        }
        if kind.is_dir() {
            add_rollouts(archive, root, &entry.path(), budget)?;
        } else if kind.is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "jsonl")
        {
            add_file(archive, root, &entry.path(), &mut budget.remaining)?;
        }
    }
    Ok(())
}
fn add_file(
    archive: &mut ZipWriter<&mut std::fs::File>,
    root: &Path,
    path: &Path,
    remaining: &mut u64,
) -> Result<(), AppError> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if !file.metadata()?.is_file() || file.metadata()?.len() > *remaining {
        return Err(AppError::Validation(
            "Histórico excede limite de exportação.".to_owned(),
        ));
    }
    let name = path
        .strip_prefix(root)
        .map_err(|_| AppError::AccessDenied("Caminho fora da sessão.".to_owned()))?
        .to_string_lossy()
        .replace('\\', "/");
    archive
        .start_file(
            name,
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated),
        )
        .map_err(|_| AppError::Internal("Arquivo de exportação inválido.".to_owned()))?;
    let mut reader = file.take(remaining.saturating_add(1));
    let copied = std::io::copy(&mut reader, archive)?;
    if copied > *remaining {
        return Err(AppError::Validation(
            "Histórico cresceu durante a exportação.".to_owned(),
        ));
    }
    *remaining -= copied;
    archive.flush()?;
    Ok(())
}
#[cfg(test)]
mod tests;

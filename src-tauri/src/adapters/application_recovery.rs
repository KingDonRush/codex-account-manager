//! An encrypted write-ahead record bridges token rotation and native-coffer commit.
//! Its key is prepared before a model/process starts, not after the coffer fails.
mod crypto;
mod native;
use crate::{adapters::auth_store, error::AppError, ports::ApplicationSessionStore};
pub use native::NativeRecoveryKeys;
use parking_lot::Mutex;
use serde_json::Value;
use std::{
    collections::HashMap,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};
use zeroize::Zeroizing;

pub trait RecoveryKeyStore: Send + Sync {
    fn load_or_create(&self, id: &str) -> Result<[u8; 32], AppError>;
    fn delete(&self, id: &str) -> Result<(), AppError>;
}
pub struct RecoverableApplicationStore {
    root: PathBuf,
    inner: Arc<dyn ApplicationSessionStore>,
    keys: Arc<dyn RecoveryKeyStore>,
    prepared: Mutex<HashMap<String, Zeroizing<[u8; 32]>>>,
}
impl RecoverableApplicationStore {
    pub fn forget_completed(&self, id: &str) -> Result<(), AppError> {
        if self.pending(id)?.is_some() {
            return Err(AppError::CredentialRecoveryPending(
                "Recuperação pendente não pode ser descartada.".to_owned(),
            ));
        }
        self.keys.delete(id)?;
        self.prepared.lock().remove(id);
        Ok(())
    }
    pub fn new(
        app_data: &Path,
        inner: Arc<dyn ApplicationSessionStore>,
        keys: Arc<dyn RecoveryKeyStore>,
    ) -> Self {
        Self {
            root: app_data.join("application-recovery"),
            inner,
            keys,
            prepared: Mutex::new(HashMap::new()),
        }
    }
    fn path(&self, id: &str) -> Result<PathBuf, AppError> {
        uuid::Uuid::parse_str(id)
            .map_err(|_| AppError::Validation("ID de sessão inválido.".to_owned()))?;
        Ok(self.root.join(format!("{id}.sealed")))
    }
    fn pending(&self, id: &str) -> Result<Option<Vec<u8>>, AppError> {
        if self.root.exists() {
            ensure_private_directory(&self.root)?;
        }
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = match options.open(self.path(id)?) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if !file.metadata()?.is_file() {
            return Err(AppError::AccessDenied(
                "Registro de recuperação não é arquivo próprio.".to_owned(),
            ));
        }
        let mut bytes = Vec::new();
        file.take(300 * 1024).read_to_end(&mut bytes)?;
        if bytes.len() >= 300 * 1024 {
            return Err(AppError::Validation(
                "Registro de recuperação excede limite.".to_owned(),
            ));
        }
        Ok(Some(bytes))
    }
    fn commit_pending(&self, id: &str, document: &Value) -> Result<(), AppError> {
        self.inner.save(id, document).map_err(|_| AppError::CredentialRecoveryPending("A sessão renovada foi preservada de forma protegida. O cofre precisa voltar antes do próximo trabalho; não reconecte nem repita a execução por esse erro.".to_owned()))?;
        self.remove_pending(id)
    }
    fn remove_pending(&self, id: &str) -> Result<(), AppError> {
        match std::fs::remove_file(self.path(id)?) {
            Ok(()) => sync_directory(&self.root),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
    fn write_pending(&self, id: &str, bytes: &[u8]) -> Result<(), AppError> {
        std::fs::create_dir_all(&self.root)?;
        ensure_private_directory(&self.root)?;
        // Reserve the stable path first. A crash/short write leaves an explicit
        // pending record that fails closed, not an invisible temporary followed
        // by reuse of the older refresh token.
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(self.path(id)?).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                AppError::CredentialRecoveryPending(
                    "Há uma recuperação pendente; conclua-a antes de substituir a sessão."
                        .to_owned(),
                )
            } else {
                AppError::Io(error)
            }
        })?;
        file.write_all(bytes)?;
        file.sync_all()?;
        sync_directory(&self.root)
    }
}
impl ApplicationSessionStore for RecoverableApplicationStore {
    fn close(&self) {
        self.prepared.lock().clear();
    }
    fn load(&self, id: &str, expected: &str) -> Result<Value, AppError> {
        self.path(id)?;
        let mut prepared = self.prepared.lock();
        // Do not use the cached key to bypass a subsequently locked coffer.
        let key = Zeroizing::new(self.keys.load_or_create(id)?);
        if let Some(bytes) = self.pending(id)? {
            let document = crypto::open(&key, id, &bytes)?;
            auth_store::validate_auth_document(&document, Some(expected))?;
            self.commit_pending(id, &document)?;
        }
        let document = self.inner.load(id, expected)?;
        prepared.insert(id.to_owned(), key);
        Ok(document)
    }
    fn save(&self, id: &str, document: &Value) -> Result<(), AppError> {
        auth_store::validate_auth_document(document, None)?;
        self.path(id)?;
        let mut prepared = self.prepared.lock();
        if !prepared.contains_key(id) {
            prepared.insert(id.to_owned(), Zeroizing::new(self.keys.load_or_create(id)?));
        }
        let key = prepared
            .get(id)
            .ok_or_else(|| AppError::Internal("Chave de recuperação não preparada.".to_owned()))?;
        self.write_pending(id, &crypto::seal(key, id, document)?)?;
        self.commit_pending(id, document)
    }
    fn delete(&self, id: &str) -> Result<(), AppError> {
        self.path(id)?;
        let mut prepared = self.prepared.lock();
        self.inner.delete(id)?;
        self.remove_pending(id)?;
        self.keys.delete(id)?;
        prepared.remove(id);
        Ok(())
    }
    fn pending_sessions(&self) -> Result<Vec<String>, AppError> {
        if self.root.exists() {
            ensure_private_directory(&self.root)?;
        }
        let entries = match std::fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        Ok(entries
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter_map(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.strip_suffix(".sealed"))
                    .filter(|id| uuid::Uuid::parse_str(id).is_ok())
                    .map(str::to_owned)
            })
            .collect())
    }
}
fn ensure_private_directory(path: &Path) -> Result<(), AppError> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(AppError::AccessDenied(
            "Recuperação não pode usar diretório externo.".to_owned(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
fn sync_directory(path: &Path) -> Result<(), AppError> {
    #[cfg(unix)]
    std::fs::File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
#[cfg(test)]
mod tests;

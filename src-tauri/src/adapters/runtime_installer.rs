use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::AppError;

const MANIFEST_PAYLOAD: &str = include_str!("../../resources/chromium-runtime-payload.json");
const MANIFEST_SIGNATURE: &str = include_str!("../../resources/chromium-runtime.sig");
const MANIFEST_PUBLIC_KEY: &str =
    "8ac0a0963918e56b1a084ac44ac80b71ed20073d6a6c192bfa1a527a0d1523b7";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeManifest {
    schema_version: u8,
    version: String,
    artifacts: Vec<RuntimeArtifact>,
}

#[derive(Debug, Deserialize)]
struct RuntimeArtifact {
    platform: String,
    url: String,
    sha256: String,
    size: u64,
    executable: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveRuntime {
    pub version: String,
    pub executable: String,
    pub archive_sha256: String,
}

pub struct RuntimeInstaller {
    runtime_root: PathBuf,
}

impl RuntimeInstaller {
    pub fn new(app_data_dir: &Path) -> Self {
        Self {
            runtime_root: app_data_dir.join("runtime").join("chromium"),
        }
    }

    pub fn install(&self) -> Result<ActiveRuntime, AppError> {
        let manifest = verified_manifest()?;
        let artifact = select_artifact(&manifest)?;
        let downloads = self.runtime_root.join("downloads");
        std::fs::create_dir_all(&downloads)?;
        let archive_path =
            downloads.join(format!("{}-{}.zip", manifest.version, artifact.platform));
        self.ensure_archive(artifact, &archive_path)?;
        self.install_archive(&manifest, artifact, &archive_path)
    }

    pub fn active(&self) -> Result<Option<ActiveRuntime>, AppError> {
        let marker = self.runtime_root.join("active.json");
        match std::fs::read_to_string(marker) {
            Ok(content) => {
                let active: ActiveRuntime = serde_json::from_str(&content)?;
                validate_active_runtime(&active)?;
                Ok(Some(active))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(AppError::Io(error)),
        }
    }

    pub fn active_executable(&self) -> Result<Option<PathBuf>, AppError> {
        let Some(active) = self.active()? else {
            return Ok(None);
        };
        let executable = self.runtime_root.join(active.executable);
        Ok(Some(executable))
    }

    fn ensure_archive(
        &self,
        artifact: &RuntimeArtifact,
        archive_path: &Path,
    ) -> Result<(), AppError> {
        if archive_path.is_file() && verify_archive(archive_path, artifact)? {
            return Ok(());
        }
        if archive_path.exists() {
            std::fs::remove_file(archive_path)?;
        }
        let temporary = tempfile::Builder::new()
            .prefix("chromium-")
            .suffix(".download")
            .tempfile_in(
                archive_path
                    .parent()
                    .ok_or_else(|| AppError::Runtime("cache sem diretório pai".to_owned()))?,
            )?;
        download(artifact, temporary.as_file())?;
        if !verify_archive(temporary.path(), artifact)? {
            return Err(AppError::Runtime(
                "o arquivo baixado não corresponde ao hash SHA-256 fixado".to_owned(),
            ));
        }
        temporary
            .persist(archive_path)
            .map_err(|error| AppError::Io(error.error))?;
        Ok(())
    }

    fn install_archive(
        &self,
        manifest: &RuntimeManifest,
        artifact: &RuntimeArtifact,
        archive_path: &Path,
    ) -> Result<ActiveRuntime, AppError> {
        std::fs::create_dir_all(&self.runtime_root)?;
        let staging = tempfile::Builder::new()
            .prefix(".install-")
            .tempdir_in(&self.runtime_root)?;
        extract_zip(archive_path, staging.path())?;
        let staged_executable = staging.path().join(&artifact.executable);
        if !staged_executable.is_file() {
            return Err(AppError::Runtime(format!(
                "o pacote não contém {}",
                artifact.executable
            )));
        }
        make_executable(&staged_executable)?;
        let version_name = format!("runtime-{}", manifest.version);
        let destination = self.runtime_root.join(&version_name);
        let backup = self
            .runtime_root
            .join(format!(".repair-{}", uuid::Uuid::new_v4()));
        let had_destination = destination.exists();
        if had_destination {
            std::fs::rename(&destination, &backup)?;
        }
        if let Err(error) = std::fs::rename(staging.path(), &destination) {
            if had_destination {
                let _ = std::fs::rename(&backup, &destination);
            }
            return Err(AppError::Io(error));
        }
        let active = ActiveRuntime {
            version: manifest.version.clone(),
            executable: Path::new(&version_name)
                .join(&artifact.executable)
                .to_string_lossy()
                .into_owned(),
            archive_sha256: artifact.sha256.clone(),
        };
        if let Err(error) = self.write_active(&active) {
            let _ = std::fs::remove_dir_all(&destination);
            if had_destination {
                let _ = std::fs::rename(&backup, &destination);
            }
            return Err(error);
        }
        if had_destination {
            let _ = std::fs::remove_dir_all(backup);
        }
        Ok(active)
    }

    fn write_active(&self, active: &ActiveRuntime) -> Result<(), AppError> {
        let marker = self.runtime_root.join("active.json");
        let mut temporary = tempfile::Builder::new()
            .prefix(".active-")
            .suffix(".json")
            .tempfile_in(&self.runtime_root)?;
        serde_json::to_writer(&mut temporary, active)?;
        temporary.write_all(b"\n")?;
        temporary.as_file().sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            temporary
                .as_file()
                .set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        temporary
            .persist(marker)
            .map_err(|error| AppError::Io(error.error))?;
        Ok(())
    }
}

pub fn required_download_size() -> Option<u64> {
    verified_manifest().ok().and_then(|manifest| {
        select_artifact(&manifest)
            .ok()
            .map(|artifact| artifact.size)
    })
}

fn verified_manifest() -> Result<RuntimeManifest, AppError> {
    let public_key = fixed_bytes::<32>(MANIFEST_PUBLIC_KEY, "chave pública")?;
    let signature_bytes = fixed_bytes::<64>(MANIFEST_SIGNATURE.trim(), "assinatura")?;
    let verifying_key = VerifyingKey::from_bytes(&public_key)
        .map_err(|error| AppError::Runtime(format!("chave de manifesto inválida: {error}")))?;
    let signature = Signature::from_bytes(&signature_bytes);
    verifying_key
        .verify(MANIFEST_PAYLOAD.as_bytes(), &signature)
        .map_err(|_| AppError::Runtime("assinatura do manifesto Chromium inválida".to_owned()))?;
    let manifest: RuntimeManifest = serde_json::from_str(MANIFEST_PAYLOAD)?;
    if manifest.schema_version != 1 {
        return Err(AppError::Runtime(format!(
            "schema de manifesto não suportado: {}",
            manifest.schema_version
        )));
    }
    Ok(manifest)
}

fn fixed_bytes<const SIZE: usize>(value: &str, label: &str) -> Result<[u8; SIZE], AppError> {
    let decoded = hex::decode(value)
        .map_err(|_| AppError::Runtime(format!("{label} não está em hexadecimal")))?;
    decoded
        .try_into()
        .map_err(|_| AppError::Runtime(format!("{label} tem tamanho inválido")))
}

fn select_artifact(manifest: &RuntimeManifest) -> Result<&RuntimeArtifact, AppError> {
    let platform = current_platform()?;
    manifest
        .artifacts
        .iter()
        .find(|artifact| artifact.platform == platform)
        .ok_or_else(|| AppError::Runtime(format!("não há Chromium fixado para {platform}")))
}

fn current_platform() -> Result<String, AppError> {
    let os = if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(windows) {
        "windows"
    } else {
        return Err(AppError::Runtime(
            "sistema operacional ainda não suportado pelo runtime gerenciado".to_owned(),
        ));
    };
    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        return Err(AppError::Runtime(
            "arquitetura ainda não suportada pelo runtime gerenciado".to_owned(),
        ));
    };
    Ok(format!("{os}-{arch}"))
}

fn download(artifact: &RuntimeArtifact, mut destination: &File) -> Result<(), AppError> {
    let parsed = url::Url::parse(&artifact.url)
        .map_err(|_| AppError::Runtime("URL do runtime inválida".to_owned()))?;
    if parsed.scheme() != "https" {
        return Err(AppError::Runtime(
            "o runtime só pode ser baixado por HTTPS".to_owned(),
        ));
    }
    let response = ureq::get(&artifact.url)
        .call()
        .map_err(|error| AppError::Runtime(format!("download do Chromium falhou: {error}")))?;
    let reader = response.into_body().into_reader();
    let mut limited = reader.take(artifact.size.saturating_add(1));
    let bytes = std::io::copy(&mut limited, &mut destination)?;
    destination.sync_all()?;
    if bytes != artifact.size {
        return Err(AppError::Runtime(format!(
            "tamanho inesperado do pacote: esperado {}, recebido {bytes}",
            artifact.size
        )));
    }
    Ok(())
}

pub(crate) fn validate_active_runtime(active: &ActiveRuntime) -> Result<(), AppError> {
    if active.version.trim().is_empty()
        || active.archive_sha256.len() != 64
        || !active
            .archive_sha256
            .chars()
            .all(|value| value.is_ascii_hexdigit())
    {
        return Err(AppError::Runtime(
            "marcador do Chromium gerenciado está corrompido".to_owned(),
        ));
    }
    let executable = Path::new(&active.executable);
    let safe = executable.components().next().is_some()
        && executable
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)));
    if !safe {
        return Err(AppError::Runtime(
            "marcador do Chromium contém caminho inseguro".to_owned(),
        ));
    }
    Ok(())
}

fn verify_archive(path: &Path, artifact: &RuntimeArtifact) -> Result<bool, AppError> {
    let metadata = std::fs::metadata(path)?;
    if metadata.len() != artifact.size {
        return Ok(false);
    }
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 128 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(hex::encode(digest.finalize()) == artifact.sha256)
}

fn extract_zip(archive_path: &Path, destination: &Path) -> Result<(), AppError> {
    let file = File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| AppError::Runtime(format!("ZIP do Chromium inválido: {error}")))?;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| AppError::Runtime(format!("entrada ZIP inválida: {error}")))?;
        if entry.is_symlink() {
            return Err(AppError::Runtime(
                "o pacote Chromium contém link simbólico não permitido".to_owned(),
            ));
        }
        let relative = entry.enclosed_name().ok_or_else(|| {
            AppError::Runtime(
                "o pacote Chromium tentou escapar do diretório de instalação".to_owned(),
            )
        })?;
        let output = destination.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&output)?;
            continue;
        }
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = File::create(&output)?;
        std::io::copy(&mut entry, &mut file)?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&output, std::fs::Permissions::from_mode(mode & 0o777))?;
        }
    }
    Ok(())
}

fn make_executable(_path: &Path) -> Result<(), AppError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(_path)?.permissions();
        permissions.set_mode(permissions.mode() | 0o700);
        std::fs::set_permissions(_path, permissions)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{current_platform, validate_active_runtime, verified_manifest, ActiveRuntime};

    #[test]
    fn bundled_manifest_has_valid_signature() {
        let manifest = verified_manifest().expect("manifesto assinado");
        assert!(!manifest.version.is_empty());
        assert!(!manifest.artifacts.is_empty());
    }

    #[test]
    fn current_platform_is_explicit() {
        let platform = current_platform().expect("plataforma suportada no CI");
        assert!(platform.contains('-'));
    }

    #[test]
    fn active_runtime_never_accepts_a_path_outside_its_cache() {
        let active = ActiveRuntime {
            version: "1.0.0".to_owned(),
            executable: "../outside/chrome".to_owned(),
            archive_sha256: "a".repeat(64),
        };
        assert!(validate_active_runtime(&active).is_err());
    }
}

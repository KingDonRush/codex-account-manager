use std::path::{Path, PathBuf};

use crate::error::AppError;

#[derive(Debug, Clone)]
pub(super) struct DesktopInstallation {
    pub executable: PathBuf,
    pub source: &'static str,
    #[cfg(windows)]
    pub app_user_model_id: Option<String>,
}

impl DesktopInstallation {
    pub fn executable(path: PathBuf, source: &'static str) -> Self {
        Self {
            executable: path,
            source,
            #[cfg(windows)]
            app_user_model_id: None,
        }
    }
}

pub(super) fn discover_selected(
    selected: Option<PathBuf>,
) -> Result<Option<DesktopInstallation>, AppError> {
    if let Some(path) = selected {
        return configured(path).map(Some);
    }
    if let Some(value) = std::env::var_os("CODEX_DESKTOP_EXECUTABLE") {
        return configured(PathBuf::from(value)).map(Some);
    }
    #[cfg(windows)]
    {
        if let Some(installation) = super::windows::discover_packaged()? {
            return Ok(Some(installation));
        }
        let installation = traditional_candidates()
            .into_iter()
            .find(|path| path.is_file())
            .map(|path| DesktopInstallation::executable(path, "traditional"));
        installation.map(Some).ok_or_else(|| {
            AppError::DesktopDiscovery(
                "Não encontrei uma instalação válida. Detecte novamente ou selecione o executável nas opções avançadas.".to_owned(),
            )
        })
    }
    #[cfg(not(windows))]
    {
        let path = if cfg!(target_os = "macos") {
            Path::new("/Applications/Codex.app/Contents/MacOS/Codex")
        } else {
            Path::new("/usr/lib/chatgpt/ChatGPT")
        };
        Ok(path
            .is_file()
            .then(|| DesktopInstallation::executable(path.into(), "system")))
    }
}

pub(super) fn configured(path: PathBuf) -> Result<DesktopInstallation, AppError> {
    if !path.is_file() {
        return Err(AppError::DesktopDiscovery(
            "O executável configurado não existe; nenhuma sessão foi alterada.".to_owned(),
        ));
    }
    Ok(DesktopInstallation::executable(path, "configured"))
}

#[cfg(windows)]
fn traditional_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for variable in ["LOCALAPPDATA", "PROGRAMFILES", "PROGRAMFILES(X86)"] {
        let Some(root) = std::env::var_os(variable) else {
            continue;
        };
        for relative in [
            "OpenAI/ChatGPT/ChatGPT.exe",
            "ChatGPT/ChatGPT.exe",
            "OpenAI/Codex/ChatGPT.exe",
        ] {
            paths.push(PathBuf::from(&root).join(relative));
        }
    }
    paths
}

#[cfg(any(windows, test))]
pub(super) fn package_executable(root: &Path, relative: &str) -> Result<PathBuf, AppError> {
    let relative = Path::new(relative);
    if relative
        .components()
        .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err(AppError::DesktopDiscovery(
            "Caminho de aplicativo inválido no pacote.".to_owned(),
        ));
    }
    let root = root.canonicalize()?;
    let path = root.join(relative).canonicalize()?;
    if !path.starts_with(&root) || !path.is_file() {
        return Err(AppError::DesktopDiscovery(
            "O executável do pacote está fora da instalação.".to_owned(),
        ));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_installation_is_resolved_from_its_current_root_not_a_versioned_constant() {
        let fixture = tempfile::tempdir().expect("fixture");
        let executable = fixture.path().join("app/ChatGPT.exe");
        std::fs::create_dir(executable.parent().expect("parent")).expect("app directory");
        std::fs::write(&executable, "fixture").expect("exe");
        assert_eq!(
            package_executable(fixture.path(), "app/ChatGPT.exe").expect("path"),
            executable.canonicalize().expect("canonical")
        );
        assert!(package_executable(fixture.path(), "../ChatGPT.exe").is_err());
        assert!(package_executable(fixture.path(), "/ChatGPT.exe").is_err());
    }

    #[test]
    fn an_invalid_explicit_executable_is_not_a_closed_desktop() {
        assert!(matches!(
            configured(PathBuf::from("/missing/codex.exe")),
            Err(AppError::DesktopDiscovery(_))
        ));
    }
}

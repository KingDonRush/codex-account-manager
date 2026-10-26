use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::AppError;

pub fn token_claims(token: Option<&str>) -> Option<Value> {
    use base64::Engine;
    let token = token?;
    if token.len() > 64 * 1024 {
        return None;
    }
    let encoded = token.split('.').nth(1)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .ok()?;
    serde_json::from_slice(&decoded).ok()
}

pub fn access_session(
    document: &Value,
    expected: Option<&str>,
) -> Result<crate::ports::AccessSession, AppError> {
    let account_id = validate_auth_document(document, expected)?;
    let access_token = document
        .pointer("/tokens/access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::Validation("Credencial de acesso ausente.".to_owned()))?
        .to_owned();
    Ok(crate::ports::AccessSession {
        account_id,
        access_token,
    })
}

pub fn validate_auth_document(
    document: &Value,
    expected_account_id: Option<&str>,
) -> Result<String, AppError> {
    let object = document
        .as_object()
        .ok_or_else(|| AppError::Validation("auth.json não contém um objeto JSON".to_owned()))?;
    if object.get("auth_mode").and_then(Value::as_str) != Some("chatgpt") {
        return Err(AppError::Validation(
            "somente sessões ChatGPT gerenciadas são compatíveis".to_owned(),
        ));
    }
    let tokens = object
        .get("tokens")
        .and_then(Value::as_object)
        .ok_or_else(|| AppError::Validation("auth.json não contém tokens".to_owned()))?;
    for field in ["access_token", "refresh_token", "account_id"] {
        if tokens
            .get(field)
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            return Err(AppError::Validation(format!(
                "auth.json não contém {field}"
            )));
        }
    }
    let account_id = tokens
        .get("account_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if expected_account_id.is_some_and(|expected| expected != account_id) {
        return Err(AppError::Validation(
            "a credencial não pertence à conta esperada".to_owned(),
        ));
    }
    Ok(account_id)
}

pub fn read_auth(path: &Path) -> Result<Value, AppError> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    let document: Value = serde_json::from_reader(file)?;
    validate_auth_document(&document, None)?;
    Ok(document)
}

pub fn atomic_write_auth(path: &Path, document: &Value) -> Result<(), AppError> {
    validate_auth_document(document, None)?;
    write_private_auth(path, document)
}

/// The native CLI understands externally managed ChatGPT credentials. No refresh
/// capability is copied into the process that performs inference.
pub fn write_external_auth(path: &Path, document: &Value, expected: &str) -> Result<(), AppError> {
    validate_auth_document(document, Some(expected))?;
    let id_token = document
        .pointer("/tokens/id_token")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| {
            AppError::Validation(
                "A sessão salva não contém a identidade necessária para execução isolada."
                    .to_owned(),
            )
        })?;
    write_private_auth(
        path,
        &serde_json::json!({
            "auth_mode":"chatgptAuthTokens", "tokens": {
                "account_id":expected, "access_token":document["tokens"]["access_token"],
                "id_token":id_token, "refresh_token":""
            }
        }),
    )
}

fn write_private_auth(path: &Path, document: &Value) -> Result<(), AppError> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Validation("caminho de auth.json inválido".to_owned()))?;
    std::fs::create_dir_all(parent)?;
    restrict_directory(parent)?;
    let temporary = tempfile::Builder::new()
        .prefix(".auth-")
        .suffix(".tmp")
        .tempfile_in(parent)?;
    {
        use std::io::Write;
        let mut file = temporary.as_file();
        serde_json::to_writer(&mut file, document)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
    }
    temporary
        .persist(path)
        .map_err(|error| AppError::Io(error.error))?;
    restrict_file(path)?;
    Ok(())
}

pub fn codex_home() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".codex")
        })
}

pub fn active_account_id_at(home: &Path) -> Option<String> {
    let document = read_auth(&home.join("auth.json")).ok()?;
    document
        .get("tokens")?
        .get("account_id")?
        .as_str()
        .map(str::to_owned)
}

fn restrict_directory(_path: &Path) -> Result<(), AppError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(_path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn restrict_file(_path: &Path) -> Result<(), AppError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(_path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{atomic_write_auth, read_auth, validate_auth_document};
    use serde_json::json;

    fn document(account_id: &str, refresh_token: &str) -> serde_json::Value {
        json!({
            "auth_mode": "chatgpt",
            "tokens": {
                "access_token": "access",
                "refresh_token": refresh_token,
                "account_id": account_id
            }
        })
    }

    #[test]
    fn atomic_auth_replacement_keeps_a_complete_document() {
        let fixture = tempfile::tempdir().expect("fixture");
        let path = fixture.path().join("auth.json");
        atomic_write_auth(&path, &document("account-a", "generation-1")).expect("first write");
        atomic_write_auth(&path, &document("account-a", "generation-2")).expect("second write");

        let loaded = read_auth(&path).expect("read");
        assert_eq!(loaded["tokens"]["refresh_token"], "generation-2");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path)
                    .expect("metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn validation_rejects_the_wrong_account() {
        let error = validate_auth_document(&document("account-a", "refresh"), Some("account-b"));
        assert!(error.is_err());
    }

    #[test]
    fn external_auth_contains_no_refresh_and_cannot_be_saved_as_a_managed_generation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let mut managed = document("account-a", "do-not-copy-refresh");
        managed["tokens"]["id_token"] = serde_json::json!("synthetic-id");
        super::write_external_auth(&path, &managed, "account-a").unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let external: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(external["auth_mode"], "chatgptAuthTokens");
        assert_eq!(external["tokens"]["refresh_token"], "");
        assert!(!String::from_utf8_lossy(&bytes).contains("do-not-copy-refresh"));
        assert!(validate_auth_document(&external, Some("account-a")).is_err());
        assert!(super::write_external_auth(&path, &managed, "account-b").is_err());
    }

    #[test]
    #[ignore = "installed native CLI login status only; synthetic tokens, private home, no model or real coffer"]
    fn installed_cli_recognizes_access_only_external_auth_without_mutating_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let token = "e30.eyJzdWIiOiJmaXh0dXJlIiwiZW1haWwiOiJmaXh0dXJlQGV4YW1wbGUuY29tIiwiaHR0cHM6Ly9hcGkub3BlbmFpLmNvbS9hdXRoIjp7ImNoYXRncHRfYWNjb3VudF9pZCI6ImZpeHR1cmUiLCJjaGF0Z3B0X3BsYW5fdHlwZSI6InBsdXMifX0.sig";
        let doc = serde_json::json!({"auth_mode":"chatgpt","tokens":{"account_id":"fixture","access_token":token,"id_token":token,"refresh_token":"not-copied"}});
        super::write_external_auth(&path, &doc, "fixture").unwrap();
        let before = std::fs::read(&path).unwrap();
        let binary = crate::adapters::app_server::resolve_codex_binary().expect("native CLI");
        let mut command = std::process::Command::new(binary);
        command
            .args([
                "-c",
                "cli_auth_credentials_store=\"file\"",
                "login",
                "status",
            ])
            .env_clear()
            .envs(crate::adapters::app_server::inherited_environment())
            .env("CODEX_HOME", dir.path());
        crate::adapters::process_supervisor::configure_supervised_process(&mut command);
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "native CLI must parse the external mode"
        );
        assert_eq!(std::fs::read(path).unwrap(), before);
    }
}

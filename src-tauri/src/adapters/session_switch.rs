mod activation;
mod recovery;
mod verifier;
use std::path::{Path, PathBuf};
use std::time::Duration;
use verifier::{needs_owned_renewal, AppServerSessionVerifier};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::adapters::auth_store;
use crate::adapters::desktop_control::{DesktopController, DesktopState};
use crate::domain::{AccountSnapshot, SwitchResult};
use crate::error::AppError;
use crate::ports::{
    CredentialStore, DesktopLifecycle, DestinationPreparation, SessionVerifier,
    StoredCredentialGeneration,
};

pub struct SessionSwitcher {
    journal_path: PathBuf,
    auth_path: PathBuf,
    credentials: Arc<dyn CredentialStore>,
    verifier: Arc<dyn SessionVerifier>,
    desktop: Arc<dyn DesktopLifecycle>,
}

pub struct SessionSwitchOutcome {
    pub result: SwitchResult,
    pub snapshot: AccountSnapshot,
    pub previous_document: Option<Value>,
    pub transaction_id: Option<String>,
    desktop: Option<DesktopState>,
}

pub struct PreparedDestination {
    account_id: String,
    fingerprints: Vec<(String, String)>,
    checked_at: std::time::Instant,
    active_account_id: Option<String>,
    preparation: DestinationPreparation,
}

impl PreparedDestination {
    pub fn needs_exclusive_renewal(&self) -> bool {
        self.preparation == DestinationPreparation::NeedsExclusiveRenewal
    }
}

struct SwitchPreparation {
    previous: Option<Value>,
    previous_id: Option<String>,
    generations: Vec<StoredCredentialGeneration>,
    previous_preservation: PreviousSessionPreservation,
}

#[derive(Clone, Copy)]
enum PreviousSessionPreservation {
    Select,
    Archive,
}

use std::sync::Arc;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SwitchJournal {
    version: u8,
    previous_account_id: Option<String>,
    target_account_id: String,
    phase: String,
    #[serde(default)]
    transaction_id: Option<String>,
}

impl SessionSwitcher {
    pub fn new(
        app_data_dir: &Path,
        codex_home: &Path,
        credentials: Arc<dyn CredentialStore>,
        database: crate::adapters::database::Database,
    ) -> Self {
        Self {
            journal_path: app_data_dir.join("switch-journal.json"),
            auth_path: codex_home.join("auth.json"),
            credentials,
            verifier: Arc::new(AppServerSessionVerifier::new(codex_home.to_path_buf())),
            desktop: Arc::new(DesktopController::with_database(database)),
        }
    }

    pub fn open_desktop(&self) -> Result<(), AppError> {
        self.desktop.open_or_focus()
    }

    pub fn select_desktop(&self, owner: isize) -> Result<bool, AppError> {
        self.desktop.select_executable(owner)
    }

    pub fn confirm_desktop_close(&self, owner: isize) -> Result<bool, AppError> {
        self.desktop.confirm_close(owner)
    }

    #[cfg(test)]
    pub(crate) fn with_dependencies(
        app_data_dir: &Path,
        auth_path: PathBuf,
        credentials: Arc<dyn CredentialStore>,
        verifier: Arc<dyn SessionVerifier>,
        desktop: Arc<dyn DesktopLifecycle>,
    ) -> Self {
        Self {
            journal_path: app_data_dir.join("switch-journal.json"),
            auth_path,
            credentials,
            verifier,
            desktop,
        }
    }

    #[cfg(test)]
    pub fn switch(&self, target_account_id: &str) -> Result<SessionSwitchOutcome, AppError> {
        let mut noop = |_: &'static str, _: Duration| {};
        self.switch_with_trace(target_account_id, &mut noop)
    }

    #[cfg(test)]
    pub fn switch_with_trace(
        &self,
        target_account_id: &str,
        trace: &mut dyn FnMut(&'static str, Duration),
    ) -> Result<SessionSwitchOutcome, AppError> {
        self.switch_prepared(target_account_id, None, trace)
    }

    pub fn apply_prepared_destination(
        &self,
        destination: PreparedDestination,
        trace: &mut dyn FnMut(&'static str, Duration),
    ) -> Result<SessionSwitchOutcome, AppError> {
        self.switch_prepared(&destination.account_id, Some(&destination), trace)
    }

    fn switch_prepared(
        &self,
        target_account_id: &str,
        destination: Option<&PreparedDestination>,
        trace: &mut dyn FnMut(&'static str, Duration),
    ) -> Result<SessionSwitchOutcome, AppError> {
        let mut preparation = self.prepare_switch(target_account_id, trace)?;
        if let Some(outcome) =
            self.try_reuse_current_session(target_account_id, &mut preparation, trace)?
        {
            return Ok(outcome);
        }
        if !destination.is_some_and(|checked| {
            checked.checked_at.elapsed() < Duration::from_secs(10)
                && checked.fingerprints == generation_fingerprints(&preparation.generations)
                && checked.active_account_id == preparation.previous_id
        }) {
            self.preflight_generations(
                target_account_id,
                &preparation.generations,
                Arc::new(std::sync::atomic::AtomicBool::new(false)),
                trace,
            )?;
            preparation.generations = self.credentials.generation_refs(target_account_id)?;
        }
        preparation.previous_preservation =
            self.previous_preservation(target_account_id, &preparation)?;
        self.preserve_previous_session(
            preparation.previous.as_ref(),
            preparation.previous_preservation,
            trace,
        )?;
        let desktop_started = std::time::Instant::now();
        let desktop = self.desktop.stop_with_trace(trace);
        trace("desktop_stop", desktop_started.elapsed());
        let desktop = desktop?;
        if let Err(error) = self.capture_final_previous(target_account_id, &mut preparation, trace)
        {
            let _ = self.desktop.restart(&desktop);
            return Err(error);
        }
        if let Err(error) = measure(trace, "write_switch_journal_prepared", || {
            self.write_journal(
                preparation.previous_id.clone(),
                target_account_id,
                "prepared",
            )
        }) {
            let _ = self.desktop.restart(&desktop);
            return Err(error);
        }
        self.activate_current_generation(target_account_id, preparation, desktop, trace)
    }

    /// A predictable destination failure must leave the running Desktop untouched.
    pub fn preflight_target(
        &self,
        target_account_id: &str,
        cancel: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<PreparedDestination, AppError> {
        let active_account_id = current_account_id(&self.auth_path)?;
        let references = self.credentials.generation_refs(target_account_id)?;
        let preparation =
            self.preflight_generations(target_account_id, &references, cancel, &mut |_, _| {})?;
        let references = self.credentials.generation_refs(target_account_id)?;
        Ok(PreparedDestination {
            account_id: target_account_id.to_owned(),
            fingerprints: generation_fingerprints(&references),
            checked_at: std::time::Instant::now(),
            active_account_id,
            preparation,
        })
    }

    fn preflight_generations(
        &self,
        target: &str,
        references: &[StoredCredentialGeneration],
        cancel: Arc<std::sync::atomic::AtomicBool>,
        trace: &mut dyn FnMut(&'static str, Duration),
    ) -> Result<DestinationPreparation, AppError> {
        if cancel.load(std::sync::atomic::Ordering::Acquire) {
            return Err(AppError::Cancelled);
        }
        let reference = single_current_generation(target, references)?;
        self.credentials.prepare_recovery(target)?;
        let document = self
            .credentials
            .load_generation(target, &reference.generation_id)?;
        measure(trace, "preflight_destination", || {
            self.verifier.prepare_destination(
                target,
                &document,
                &|refreshed| self.credentials.save(refreshed).map(|_| ()),
                cancel,
            )
        })
    }

    /// Explicit local logout: preserve the latest session in the coffer, never revoke it remotely.
    pub fn logout(&self, expected_id: &str) -> Result<Option<String>, AppError> {
        let document = auth_store::read_auth(&self.auth_path)?;
        auth_store::validate_auth_document(&document, Some(expected_id))?;
        let desktop = self.desktop.stop()?;
        let result = (|| {
            let latest = auth_store::read_auth(&self.auth_path)?;
            auth_store::validate_auth_document(&latest, Some(expected_id))?;
            self.credentials.save(&latest)?;
            std::fs::remove_file(&self.auth_path)?;
            Ok(())
        })();
        if let Err(error) = result {
            let _ = self.desktop.restart(&desktop);
            return Err(error);
        }
        Ok(self.desktop.restart(&desktop).err().map(|_| {
            "Você saiu da sessão local, mas o Codex precisa ser reaberto manualmente.".to_owned()
        }))
    }

    fn capture_final_previous(
        &self,
        target: &str,
        preparation: &mut SwitchPreparation,
        trace: &mut dyn FnMut(&'static str, Duration),
    ) -> Result<(), AppError> {
        let final_document = measure(trace, "read_auth_after_desktop_stop", || {
            read_optional_auth(&self.auth_path)
        })?;
        let final_id = final_document
            .as_ref()
            .map(|document| auth_store::validate_auth_document(document, None))
            .transpose()?;
        if final_id != preparation.previous_id {
            return Err(AppError::Busy(
                "A sessão foi alterada por outra ferramenta durante o fechamento do Codex."
                    .to_owned(),
            ));
        }
        if final_document != preparation.previous {
            self.preserve_previous_session(
                final_document.as_ref(),
                preparation.previous_preservation,
                trace,
            )?;
            preparation.previous = final_document;
        }
        // Shutdown and legacy normalization can both change the current reference.
        // A pre-stop generation must never replay over the captured final session.
        preparation.generations = self.current_generations(target)?;
        Ok(())
    }

    fn previous_preservation(
        &self,
        target: &str,
        preparation: &SwitchPreparation,
    ) -> Result<PreviousSessionPreservation, AppError> {
        if preparation.previous_id.as_deref() != Some(target) {
            return Ok(PreviousSessionPreservation::Select);
        }
        let selected = self.credentials.load(target)?;
        auth_store::validate_auth_document(&selected, Some(target))?;
        let same_generation = preparation.previous.as_ref().is_some_and(|previous| {
            previous.pointer("/tokens/refresh_token") == selected.pointer("/tokens/refresh_token")
        });
        // Different refresh strings do not prove independent OAuth chains. Keep
        // the selected login intact; this does not authorize refreshing a copy.
        Ok(if same_generation {
            PreviousSessionPreservation::Select
        } else {
            PreviousSessionPreservation::Archive
        })
    }

    fn current_generations(
        &self,
        target: &str,
    ) -> Result<Vec<StoredCredentialGeneration>, AppError> {
        let mut generations = self.credentials.generation_refs(target)?;
        generations.retain(|generation| generation.is_active);
        single_current_generation(target, &generations)?;
        Ok(generations)
    }

    fn prepare_switch(
        &self,
        target_account_id: &str,
        trace: &mut dyn FnMut(&'static str, Duration),
    ) -> Result<SwitchPreparation, AppError> {
        let previous = measure(trace, "read_previous_auth", || {
            read_optional_auth(&self.auth_path)
        })?;
        let previous_id = previous
            .as_ref()
            .map(|document| auth_store::validate_auth_document(document, None))
            .transpose()?;
        let generations = measure(trace, "credential_generation_refs", || {
            self.current_generations(target_account_id)
        })?;
        Ok(SwitchPreparation {
            previous,
            previous_id,
            generations,
            previous_preservation: PreviousSessionPreservation::Select,
        })
    }

    fn try_reuse_current_session(
        &self,
        target_account_id: &str,
        preparation: &mut SwitchPreparation,
        trace: &mut dyn FnMut(&'static str, Duration),
    ) -> Result<Option<SessionSwitchOutcome>, AppError> {
        if preparation.previous_id.as_deref() != Some(target_account_id) {
            return Ok(None);
        }
        if let Some(active) = preparation
            .generations
            .iter()
            .find(|generation| generation.is_active)
        {
            let current = preparation
                .previous
                .as_ref()
                .map(document_fingerprint)
                .transpose()?;
            if current.as_deref() != Some(active.fingerprint.as_str()) {
                return Ok(None);
            }
        }
        let started = std::time::Instant::now();
        let current = self
            .verifier
            .verify_current_with_trace(target_account_id, trace);
        trace("verify_existing_session", started.elapsed());
        let snapshot = match current {
            Ok(snapshot) => snapshot,
            Err(error) if needs_owned_renewal(&error) => return Ok(None),
            // An expired access token does not prove its refresh token is
            // revoked. Retry under the normal stop/materialize/refresh flow.
            Err(error) => return Err(error),
        };
        let refreshed = measure(trace, "read_refreshed_auth", || {
            auth_store::read_auth(&self.auth_path)
        })?;
        measure(trace, "persist_refreshed_session", || {
            self.credentials.save_generation(&refreshed).map(|_| ())
        })?;
        Ok(Some(SessionSwitchOutcome {
            result: SwitchResult {
                previous_account_id: preparation.previous_id.clone(),
                active_account_id: target_account_id.to_owned(),
                changed: false,
                desktop_restarted: false,
                warning: None,
            },
            snapshot,
            previous_document: preparation.previous.clone(),
            transaction_id: None,
            desktop: None,
        }))
    }

    fn rollback(&self, auth_path: &Path, previous: Option<&Value>) -> Result<(), AppError> {
        if let Some(document) = previous {
            return auth_store::atomic_write_auth(auth_path, document);
        }
        match std::fs::remove_file(auth_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(AppError::Io(error)),
        }
    }

    fn write_journal(
        &self,
        previous_id: Option<String>,
        target_id: &str,
        phase: &str,
    ) -> Result<(), AppError> {
        let transaction_id = if phase == "prepared" {
            Some(uuid::Uuid::new_v4().to_string())
        } else {
            let previous: SwitchJournal =
                serde_json::from_reader(std::fs::File::open(&self.journal_path)?)?;
            previous.transaction_id
        };
        let journal = SwitchJournal {
            version: 2,
            previous_account_id: previous_id,
            target_account_id: target_id.to_owned(),
            phase: phase.to_owned(),
            transaction_id,
        };
        write_private_json(&self.journal_path, &journal)
    }
}

fn generation_fingerprints(references: &[StoredCredentialGeneration]) -> Vec<(String, String)> {
    references
        .iter()
        .filter(|reference| reference.is_active)
        .map(|value| (value.generation_id.clone(), value.fingerprint.clone()))
        .collect()
}

fn single_current_generation<'a>(
    target: &str,
    references: &'a [StoredCredentialGeneration],
) -> Result<&'a StoredCredentialGeneration, AppError> {
    let mut current = references.iter().filter(|reference| reference.is_active);
    match (current.next(), current.next()) {
        (Some(reference), None) if reference.account_id == target => Ok(reference),
        (None, _) => Err(AppError::Keyring(
            "nenhuma geração de credencial está disponível".to_owned(),
        )),
        _ => Err(AppError::Validation(
            "A seleção de sessão está ambígua; nenhuma credencial será aplicada.".to_owned(),
        )),
    }
}

fn measure<T>(
    trace: &mut dyn FnMut(&'static str, Duration),
    stage: &'static str,
    operation: impl FnOnce() -> Result<T, AppError>,
) -> Result<T, AppError> {
    let started = std::time::Instant::now();
    let result = operation();
    trace(stage, started.elapsed());
    result
}

fn verify_session(
    verifier: &dyn SessionVerifier,
    expected_account_id: &str,
    stage: &'static str,
    trace: &mut dyn FnMut(&'static str, Duration),
) -> Result<AccountSnapshot, AppError> {
    let started = std::time::Instant::now();
    let result = verifier.verify_with_trace(expected_account_id, trace);
    trace(stage, started.elapsed());
    result
}

fn read_optional_auth(path: &Path) -> Result<Option<Value>, AppError> {
    match auth_store::read_auth(path) {
        Ok(document) => Ok(Some(document)),
        Err(AppError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn current_account_id(path: &Path) -> Result<Option<String>, AppError> {
    read_optional_auth(path)?
        .as_ref()
        .map(|document| auth_store::validate_auth_document(document, None))
        .transpose()
}

fn document_fingerprint(document: &Value) -> Result<String, AppError> {
    let serialized = serde_json::to_vec(document)?;
    Ok(hex::encode(Sha256::digest(serialized)))
}

fn is_definitive_credential_failure(error: &AppError) -> bool {
    match error {
        AppError::AuthenticationRequired(_) => true,
        AppError::Validation(_) | AppError::Json(_) => true,
        AppError::Keyring(message) => contains_definitive_marker(message),
        AppError::AppServer(_) | AppError::Rpc { .. } => error.is_refresh_rejected(),
        _ => false,
    }
}

fn contains_definitive_marker(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    [
        "invalid_grant",
        "token_revoked",
        "refresh_token_reused",
        "refresh_token_expired",
        "refresh_token_invalidated",
        "invalidated oauth token",
        "token expired",
        "conta autenticada não corresponde",
        "checksum",
        "parte de credencial ausente",
        "geração de credencial ausente",
        "credencial ausente",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
}

fn write_private_json(path: &Path, value: &impl Serialize) -> Result<(), AppError> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Internal("caminho de diário inválido".to_owned()))?;
    std::fs::create_dir_all(parent)?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".switch-")
        .suffix(".tmp")
        .tempfile_in(parent)?;
    serde_json::to_writer(&mut temporary, value)?;
    use std::io::Write;
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
        .persist(path)
        .map_err(|error| AppError::Io(error.error))?;
    Ok(())
}

#[cfg(test)]
mod tests;
#[cfg(all(test, unix))]
mod verifier_tests;

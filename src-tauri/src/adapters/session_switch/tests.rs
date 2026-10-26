use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::{json, Value};

use super::SessionSwitcher;
use crate::adapters::auth_store;
use crate::adapters::desktop_control::{DesktopLaunchOutcome, DesktopState};
use crate::domain::AccountSnapshot;
use crate::error::AppError;
use crate::ports::{
    CredentialStore, DesktopLifecycle, SessionVerifier, StoredCredentialGeneration,
};

#[derive(Default)]
struct FakeCredentials {
    documents: Mutex<HashMap<String, Value>>,
    history: Mutex<HashMap<String, Value>>,
    fail_save: AtomicBool,
}

impl CredentialStore for FakeCredentials {
    fn save(&self, document: &Value) -> Result<String, AppError> {
        if self.fail_save.load(Ordering::Acquire) {
            return Err(AppError::Keyring("falha injetada".to_owned()));
        }
        let account_id = auth_store::validate_auth_document(document, None)?;
        self.documents
            .lock()
            .insert(account_id.clone(), document.clone());
        self.history
            .lock()
            .insert(super::document_fingerprint(document)?, document.clone());
        Ok(account_id)
    }

    fn load(&self, account_id: &str) -> Result<Value, AppError> {
        self.documents
            .lock()
            .get(account_id)
            .cloned()
            .ok_or_else(|| AppError::Keyring("credencial ausente".to_owned()))
    }

    fn load_optional(&self, account_id: &str) -> Result<Option<Value>, AppError> {
        Ok(self.documents.lock().get(account_id).cloned())
    }

    fn delete(&self, account_id: &str) -> Result<(), AppError> {
        self.documents.lock().remove(account_id);
        Ok(())
    }

    fn save_generation(&self, document: &Value) -> Result<StoredCredentialGeneration, AppError> {
        let id = self.save(document)?;
        Ok(self.generation_refs(&id)?.remove(0))
    }

    fn generation_refs(&self, id: &str) -> Result<Vec<StoredCredentialGeneration>, AppError> {
        let Some(document) = self.load_optional(id)? else {
            return Ok(Vec::new());
        };
        let fingerprint = super::document_fingerprint(&document)?;
        Ok(vec![StoredCredentialGeneration {
            account_id: id.to_owned(),
            generation_id: fingerprint.clone(),
            fingerprint,
            created_at: chrono::Utc::now(),
            is_active: true,
        }])
    }

    fn load_generation(&self, id: &str, generation: &str) -> Result<Value, AppError> {
        let current = self.load(id)?;
        if super::document_fingerprint(&current)? == generation {
            return Ok(current);
        }
        self.history
            .lock()
            .get(generation)
            .cloned()
            .ok_or_else(|| AppError::Keyring("missing fixture generation".to_owned()))
    }

    fn archive_generation(&self, document: &Value) -> Result<StoredCredentialGeneration, AppError> {
        if self.fail_save.load(Ordering::Acquire) {
            return Err(AppError::Keyring("archive failure".to_owned()));
        }
        let id = auth_store::validate_auth_document(document, None)?;
        let fingerprint = super::document_fingerprint(document)?;
        let current = self.load(&id)?;
        self.history
            .lock()
            .insert(fingerprint.clone(), document.clone());
        Ok(StoredCredentialGeneration {
            account_id: id,
            generation_id: fingerprint.clone(),
            is_active: super::document_fingerprint(&current)? == fingerprint,
            fingerprint,
            created_at: chrono::Utc::now(),
        })
    }
}

struct FakeVerifier {
    failure: Option<String>,
}

struct RejectedPreflight;
impl SessionVerifier for RejectedPreflight {
    fn preflight(&self, _: &str, _: &Value) -> Result<(), AppError> {
        Err(AppError::AuthenticationRequired(
            "synthetic rejection before stop".to_owned(),
        ))
    }
    fn verify(&self, _: &str) -> Result<AccountSnapshot, AppError> {
        unreachable!("rejected destination must not be materialized")
    }
}

struct RenewingPreflight;
impl SessionVerifier for RenewingPreflight {
    fn prepare_destination(
        &self,
        id: &str,
        document: &Value,
        persist: &dyn Fn(&Value) -> Result<(), AppError>,
        _: Arc<AtomicBool>,
    ) -> Result<crate::ports::DestinationPreparation, AppError> {
        assert_eq!(document["tokens"]["account_id"], id);
        let mut refreshed = document.clone();
        refreshed["tokens"]["access_token"] = json!("renewed-before-stop");
        persist(&refreshed).map(|_| crate::ports::DestinationPreparation::Verified)
    }
    fn verify(&self, _: &str) -> Result<AccountSnapshot, AppError> {
        Ok(AccountSnapshot {
            email: None,
            plan_type: None,
            rate_limits: None,
        })
    }
}
#[test]
fn destination_renewal_finishes_before_stop_and_uses_the_new_generation_without_extra_login() {
    let fixture = tempfile::tempdir().expect("fixture");
    let credentials = Arc::new(FakeCredentials::default());
    credentials.save(&document("target")).expect("old target");
    let desktop = Arc::new(FakeDesktop::default());
    let (switcher, path) = switcher(
        &fixture,
        credentials.clone(),
        Arc::new(RenewingPreflight),
        desktop.clone(),
    );
    auth_store::atomic_write_auth(&path, &document("previous")).expect("previous");
    let destination = switcher
        .preflight_target("target", Arc::new(AtomicBool::new(false)))
        .expect("renewable");
    assert_eq!(desktop.stopped.load(Ordering::Acquire), 0);
    assert_eq!(
        auth_store::read_auth(&path).expect("still previous"),
        document("previous")
    );
    switcher
        .apply_prepared_destination(destination, &mut |_, _| {})
        .expect("use renewed");
    assert_eq!(
        auth_store::read_auth(&path).expect("target")["tokens"]["access_token"],
        "renewed-before-stop"
    );
    assert_eq!(desktop.stopped.load(Ordering::Acquire), 1);
}
#[test]
fn nullable_identity_and_generic_unauthorized_do_not_quarantine_the_refresh_chain() {
    assert!(!super::is_definitive_credential_failure(
        &AppError::SessionRenewalRequired("account null".to_owned())
    ));
    assert!(!super::is_definitive_credential_failure(&AppError::Rpc {
        code: 401,
        message: "401 Unauthorized access token expired".to_owned()
    }));
    assert!(super::is_definitive_credential_failure(&AppError::Rpc {
        code: 401,
        message: "refresh_token_reused".to_owned()
    }));
}

#[test]
fn a_single_current_selection_is_required_without_promoting_old_snapshots_or_another_identity() {
    let store = FakeCredentials::default();
    store.save(&document("target")).expect("current");
    let refs = store.generation_refs("target").expect("current ref");
    assert!(super::single_current_generation("target", &refs).is_ok());
    let mut ambiguous = refs.clone();
    ambiguous.push(refs[0].clone());
    assert!(matches!(
        super::single_current_generation("target", &ambiguous),
        Err(AppError::Validation(_))
    ));
    assert!(super::single_current_generation("other", &refs).is_err());
    let mut old = refs;
    old[0].is_active = false;
    assert!(super::single_current_generation("target", &old).is_err());
}

#[test]
fn rejected_preflight_keeps_desktop_and_global_auth_untouched() {
    let fixture = tempfile::tempdir().expect("fixture");
    let credentials = Arc::new(FakeCredentials::default());
    credentials.save(&document("target")).expect("target");
    let desktop = Arc::new(FakeDesktop::default());
    let (switcher, path) = switcher(
        &fixture,
        credentials,
        Arc::new(RejectedPreflight),
        desktop.clone(),
    );
    auth_store::atomic_write_auth(&path, &document("current")).expect("active");
    assert!(matches!(
        switcher.switch("target"),
        Err(AppError::AuthenticationRequired(_))
    ));
    assert_eq!(desktop.stopped.load(Ordering::Acquire), 0);
    assert_eq!(desktop.restarted.load(Ordering::Acquire), 0);
    assert_eq!(
        auth_store::read_auth(&path).expect("preserved"),
        document("current")
    );
}

struct CyclingCredentials {
    documents: Vec<(String, Value)>,
    loaded: Mutex<Vec<String>>,
    quarantined: Mutex<HashSet<String>>,
}

impl CredentialStore for CyclingCredentials {
    fn save(&self, document: &Value) -> Result<String, AppError> {
        auth_store::validate_auth_document(document, None)
    }

    fn load(&self, account_id: &str) -> Result<Value, AppError> {
        self.documents
            .first()
            .map(|(_, document)| document.clone())
            .ok_or_else(|| AppError::Keyring(format!("credencial ausente: {account_id}")))
    }

    fn load_optional(&self, account_id: &str) -> Result<Option<Value>, AppError> {
        self.load(account_id).map(Some)
    }

    fn delete(&self, _account_id: &str) -> Result<(), AppError> {
        Ok(())
    }

    fn generation_refs(
        &self,
        account_id: &str,
    ) -> Result<Vec<StoredCredentialGeneration>, AppError> {
        let quarantined = self.quarantined.lock();
        let mut references = self
            .documents
            .iter()
            .map(|(generation_id, _)| StoredCredentialGeneration {
                account_id: account_id.to_owned(),
                generation_id: generation_id.clone(),
                fingerprint: format!("fingerprint-{generation_id}"),
                created_at: chrono::Utc::now(),
                is_active: generation_id == "working",
            })
            .collect::<Vec<_>>();
        references.sort_by_key(|reference| quarantined.contains(&reference.generation_id));
        Ok(references)
    }

    fn load_generation(&self, _account_id: &str, generation_id: &str) -> Result<Value, AppError> {
        self.loaded.lock().push(generation_id.to_owned());
        self.documents
            .iter()
            .find(|(id, _)| id == generation_id)
            .map(|(_, document)| document.clone())
            .ok_or_else(|| AppError::Keyring("geração ausente".to_owned()))
    }

    fn quarantine_generation(
        &self,
        _account_id: &str,
        generation_id: &str,
    ) -> Result<(), AppError> {
        self.quarantined.lock().insert(generation_id.to_owned());
        Ok(())
    }

    fn recover_generation(&self, _account_id: &str, generation_id: &str) -> Result<(), AppError> {
        self.quarantined.lock().remove(generation_id);
        Ok(())
    }
}

struct DocumentVerifier {
    auth_path: PathBuf,
    failures: HashMap<String, String>,
    attempted: Mutex<Vec<String>>,
}

impl SessionVerifier for DocumentVerifier {
    fn verify(&self, expected_account_id: &str) -> Result<AccountSnapshot, AppError> {
        let document = auth_store::read_auth(&self.auth_path)?;
        auth_store::validate_auth_document(&document, Some(expected_account_id))?;
        let generation = document
            .pointer("/tokens/generation")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        self.attempted.lock().push(generation.clone());
        if let Some(message) = self.failures.get(&generation) {
            return Err(AppError::AppServer(message.clone()));
        }
        Ok(AccountSnapshot {
            email: None,
            plan_type: None,
            rate_limits: None,
        })
    }
}

impl SessionVerifier for FakeVerifier {
    fn verify(&self, _expected_account_id: &str) -> Result<AccountSnapshot, AppError> {
        match self.failure.as_deref() {
            Some(message) => Err(AppError::AppServer(message.to_owned())),
            None => Ok(AccountSnapshot {
                email: None,
                plan_type: None,
                rate_limits: None,
            }),
        }
    }
}

#[derive(Default)]
struct FakeDesktop {
    stopped: AtomicUsize,
    restarted: AtomicUsize,
    stop_failed: AtomicBool,
    launch_skipped: AtomicBool,
    launch_failed: AtomicBool,
}

#[test]
fn a_materialized_switch_does_not_reopen_desktop_before_metadata_commit() {
    let fixture = tempfile::tempdir().expect("fixture");
    let credentials = Arc::new(FakeCredentials::default());
    credentials.save(&document("target")).expect("target");
    let desktop = Arc::new(FakeDesktop::default());
    let (switcher, path) = switcher(
        &fixture,
        credentials,
        Arc::new(FakeVerifier { failure: None }),
        desktop.clone(),
    );
    auth_store::atomic_write_auth(&path, &document("previous")).expect("previous");
    let outcome = switcher.switch("target").expect("materialized destination");
    assert_eq!(desktop.restarted.load(Ordering::Acquire), 0);
    assert!(!outcome.result.desktop_restarted);
}

impl DesktopLifecycle for FakeDesktop {
    fn stop(&self) -> Result<DesktopState, AppError> {
        self.stopped.fetch_add(1, Ordering::AcqRel);
        if self.stop_failed.load(Ordering::Acquire) {
            return Err(AppError::DesktopClosePending(
                "synthetic close failure".to_owned(),
            ));
        }
        Ok(DesktopState::default())
    }

    fn restart(&self, _state: &DesktopState) -> Result<DesktopLaunchOutcome, AppError> {
        self.restarted.fetch_add(1, Ordering::AcqRel);
        if self.launch_failed.load(Ordering::Acquire) {
            return Err(AppError::DesktopLaunch(
                "synthetic activation failure".to_owned(),
            ));
        }
        if self.launch_skipped.load(Ordering::Acquire) {
            return Ok(DesktopLaunchOutcome::NotRequired);
        }
        Ok(DesktopLaunchOutcome::Observed { pid: 1 })
    }
}

#[test]
fn missing_metadata_commit_blocks_reopening_and_never_claims_a_restart() {
    let fixture = tempfile::tempdir().expect("fixture");
    let credentials = Arc::new(FakeCredentials::default());
    credentials.save(&document("target")).expect("target");
    let desktop = Arc::new(FakeDesktop::default());
    let (switcher, path) = switcher(
        &fixture,
        credentials,
        Arc::new(FakeVerifier { failure: None }),
        desktop.clone(),
    );
    auth_store::atomic_write_auth(&path, &document("previous")).expect("previous");
    let mut outcome = switcher.switch("target").expect("materialized");
    assert!(switcher
        .reopen_committed_desktop(&mut outcome, |_| Ok(false), &mut |_, _| {})
        .is_err());
    assert_eq!(desktop.restarted.load(Ordering::Acquire), 0);
    assert!(!outcome.result.desktop_restarted);
}

#[test]
fn failed_post_commit_launch_preserves_the_target_and_does_not_replay_or_rollback() {
    let fixture = tempfile::tempdir().expect("fixture");
    let credentials = Arc::new(FakeCredentials::default());
    credentials.save(&document("target")).expect("target");
    let desktop = Arc::new(FakeDesktop::default());
    desktop.launch_failed.store(true, Ordering::Release);
    let (switcher, path) = switcher(
        &fixture,
        credentials,
        Arc::new(FakeVerifier { failure: None }),
        desktop.clone(),
    );
    auth_store::atomic_write_auth(&path, &document("previous")).expect("previous");
    let mut outcome = switcher.switch("target").expect("materialized");
    switcher
        .reopen_committed_desktop(&mut outcome, |_| Ok(true), &mut |_, _| {})
        .expect("committed result");
    assert!(!outcome.result.desktop_restarted);
    assert!(outcome.result.warning.is_some());
    assert_eq!(
        auth_store::read_auth(&path).expect("target"),
        document("target")
    );
    switcher
        .reopen_committed_desktop(&mut outcome, |_| Ok(true), &mut |_, _| {})
        .expect("no automatic replay");
    assert_eq!(desktop.restarted.load(Ordering::Acquire), 1);
    assert_eq!(desktop.stopped.load(Ordering::Acquire), 1);
}

#[test]
fn a_not_required_launch_is_not_reported_as_restarted() {
    let fixture = tempfile::tempdir().expect("fixture");
    let credentials = Arc::new(FakeCredentials::default());
    credentials.save(&document("target")).expect("target");
    let desktop = Arc::new(FakeDesktop::default());
    desktop.launch_skipped.store(true, Ordering::Release);
    let (switcher, path) = switcher(
        &fixture,
        credentials,
        Arc::new(FakeVerifier { failure: None }),
        desktop,
    );
    auth_store::atomic_write_auth(&path, &document("previous")).expect("previous");
    let mut outcome = switcher.switch("target").expect("materialized");
    switcher
        .reopen_committed_desktop(&mut outcome, |_| Ok(true), &mut |_, _| {})
        .expect("committed result");
    assert!(!outcome.result.desktop_restarted);
    assert!(outcome.result.warning.is_none());
}

fn document(account_id: &str) -> Value {
    json!({
        "auth_mode": "chatgpt",
        "tokens": {
            "access_token": format!("access-{account_id}"),
            "refresh_token": format!("refresh-{account_id}"),
            "account_id": account_id
        }
    })
}

#[test]
fn crash_before_metadata_commit_restores_previous_session() {
    let fixture = tempfile::tempdir().expect("fixture");
    let credentials = Arc::new(FakeCredentials::default());
    credentials.save(&document("old")).expect("old");
    credentials.save(&document("new")).expect("new");
    let (switcher, path) = switcher(
        &fixture,
        credentials.clone(),
        Arc::new(FakeVerifier { failure: None }),
        Arc::new(FakeDesktop::default()),
    );
    auth_store::atomic_write_auth(&path, &document("old")).expect("auth");
    let outcome = switcher.switch("new").expect("materialized");
    assert!(outcome.transaction_id.is_some());
    assert!(fixture.path().join("app-data/switch-journal.json").exists());
    assert_eq!(
        switcher
            .recover(|_| Ok(false))
            .expect("rollback")
            .as_deref(),
        Some("old")
    );
    assert_eq!(
        auth_store::read_auth(&path).expect("restored"),
        document("old")
    );
    assert!(!fixture.path().join("app-data/switch-journal.json").exists());
}

#[test]
fn crash_after_sqlite_commit_only_cleans_the_journal() {
    let fixture = tempfile::tempdir().expect("fixture");
    let credentials = Arc::new(FakeCredentials::default());
    credentials.save(&document("old")).expect("old");
    credentials.save(&document("new")).expect("new");
    let desktop = Arc::new(FakeDesktop::default());
    let (switcher, path) = switcher(
        &fixture,
        credentials,
        Arc::new(FakeVerifier { failure: None }),
        desktop.clone(),
    );
    auth_store::atomic_write_auth(&path, &document("old")).expect("auth");
    let outcome = switcher.switch("new").expect("materialized");
    let stopped = desktop.stopped.load(Ordering::Acquire);
    assert_eq!(
        switcher
            .recover(|id| Ok(Some(id) == outcome.transaction_id.as_deref()))
            .expect("committed")
            .as_deref(),
        Some("new")
    );
    assert_eq!(
        auth_store::read_auth(&path).expect("target"),
        document("new")
    );
    assert_eq!(desktop.stopped.load(Ordering::Acquire), stopped);
}

#[test]
fn local_logout_preserves_the_latest_credential_and_does_not_remove_history() {
    let fixture = tempfile::tempdir().expect("fixture");
    let credentials = Arc::new(FakeCredentials::default());
    let desktop = Arc::new(FakeDesktop::default());
    let (switcher, auth_path) = switcher(
        &fixture,
        credentials.clone(),
        Arc::new(FakeVerifier { failure: None }),
        desktop.clone(),
    );
    auth_store::atomic_write_auth(&auth_path, &document("account-a")).expect("auth");
    let history = auth_path.parent().expect("home").join("history.fixture");
    std::fs::write(&history, "shared fixture history").expect("history");
    assert!(switcher.logout("account-a").expect("logout").is_none());
    assert!(!auth_path.exists());
    assert!(history.exists());
    assert_eq!(
        credentials.load("account-a").expect("preserved credential"),
        document("account-a")
    );
    assert_eq!(desktop.stopped.load(Ordering::Acquire), 1);
    assert_eq!(desktop.restarted.load(Ordering::Acquire), 1);
}

#[test]
fn local_logout_coffer_failure_keeps_global_auth_intact() {
    let fixture = tempfile::tempdir().expect("fixture");
    let credentials = Arc::new(FakeCredentials::default());
    credentials.fail_save.store(true, Ordering::Release);
    let desktop = Arc::new(FakeDesktop::default());
    let (switcher, auth_path) = switcher(
        &fixture,
        credentials,
        Arc::new(FakeVerifier { failure: None }),
        desktop.clone(),
    );
    auth_store::atomic_write_auth(&auth_path, &document("account-a")).expect("auth");
    let before = std::fs::read(&auth_path).expect("before");
    assert!(switcher.logout("account-a").is_err());
    assert_eq!(std::fs::read(&auth_path).expect("after"), before);
    assert_eq!(desktop.restarted.load(Ordering::Acquire), 1);
}

fn generation_document(account_id: &str, generation: &str) -> Value {
    json!({
        "auth_mode": "chatgpt",
        "tokens": {
            "access_token": format!("access-{generation}"),
            "refresh_token": format!("refresh-{generation}"),
            "account_id": account_id,
            "generation": generation
        }
    })
}

fn switcher(
    fixture: &tempfile::TempDir,
    credentials: Arc<dyn CredentialStore>,
    verifier: Arc<dyn SessionVerifier>,
    desktop: Arc<FakeDesktop>,
) -> (SessionSwitcher, PathBuf) {
    let auth_path = fixture.path().join("codex-home/auth.json");
    let app_data = fixture.path().join("app-data");
    (
        SessionSwitcher::with_dependencies(
            &app_data,
            auth_path.clone(),
            credentials,
            verifier,
            desktop,
        ),
        auth_path,
    )
}

#[test]
fn revoked_target_is_rolled_back_after_materialization() {
    let fixture = tempfile::tempdir().expect("fixture");
    let credentials = Arc::new(FakeCredentials::default());
    credentials
        .documents
        .lock()
        .insert("target".to_owned(), document("target"));
    let desktop = Arc::new(FakeDesktop::default());
    let verifier = Arc::new(FakeVerifier {
        failure: Some("token_revoked".to_owned()),
    });
    let (switcher, auth_path) = switcher(&fixture, credentials, verifier, desktop.clone());
    auth_store::atomic_write_auth(&auth_path, &document("previous")).expect("active auth");

    let result = switcher.switch("target");

    assert!(result.is_err());
    let restored = auth_store::read_auth(&auth_path).expect("restored auth");
    assert_eq!(
        auth_store::validate_auth_document(&restored, None).expect("id"),
        "previous"
    );
    assert_eq!(desktop.stopped.load(Ordering::Acquire), 1);
    assert_eq!(desktop.restarted.load(Ordering::Acquire), 1);
    assert!(!fixture.path().join("app-data/switch-journal.json").exists());
}

#[test]
fn keyring_failure_happens_before_desktop_or_auth_changes() {
    let fixture = tempfile::tempdir().expect("fixture");
    let credentials = Arc::new(FakeCredentials::default());
    credentials
        .documents
        .lock()
        .insert("target".to_owned(), document("target"));
    credentials.fail_save.store(true, Ordering::Release);
    let desktop = Arc::new(FakeDesktop::default());
    let verifier = Arc::new(FakeVerifier { failure: None });
    let (switcher, auth_path) = switcher(&fixture, credentials, verifier, desktop.clone());
    auth_store::atomic_write_auth(&auth_path, &document("previous")).expect("active auth");

    let result = switcher.switch("target");

    assert!(matches!(result, Err(AppError::Keyring(_))));
    let unchanged = auth_store::read_auth(&auth_path).expect("unchanged auth");
    assert_eq!(
        auth_store::validate_auth_document(&unchanged, None).expect("id"),
        "previous"
    );
    assert_eq!(desktop.stopped.load(Ordering::Acquire), 0);
}

#[test]
fn a_new_saved_session_is_applied_even_when_the_account_id_is_already_active() {
    let fixture = tempfile::tempdir().expect("fixture");
    let credentials = Arc::new(FakeCredentials::default());
    credentials
        .save(&generation_document("target", "new-session"))
        .expect("new login saved before switching");
    let desktop = Arc::new(FakeDesktop::default());
    let verifier = Arc::new(FakeVerifier { failure: None });
    let (switcher, auth_path) = switcher(&fixture, credentials.clone(), verifier, desktop.clone());
    auth_store::atomic_write_auth(&auth_path, &generation_document("target", "old-session"))
        .expect("active old session");
    let outcome = switcher.switch("target").expect("apply reconnect");
    assert!(outcome.result.changed);
    assert_eq!(desktop.stopped.load(Ordering::Acquire), 1);
    assert_eq!(
        auth_store::read_auth(&auth_path).expect("active new session")["tokens"]["generation"],
        "new-session"
    );
    assert_eq!(
        credentials.load("target").expect("selected new login"),
        generation_document("target", "new-session")
    );
}

struct FinalSessionDesktop {
    auth_path: PathBuf,
    final_document: Value,
}

#[test]
fn failed_desktop_close_does_not_replace_a_new_login_with_the_previous_desktop_session() {
    let fixture = tempfile::tempdir().expect("fixture");
    let credentials = Arc::new(FakeCredentials::default());
    let selected = generation_document("target", "new-login");
    credentials.save(&selected).expect("new login");
    let desktop = Arc::new(FakeDesktop::default());
    desktop.stop_failed.store(true, Ordering::Release);
    let (switcher, path) = switcher(
        &fixture,
        credentials.clone(),
        Arc::new(FakeVerifier { failure: None }),
        desktop,
    );
    let previous = generation_document("target", "old-desktop");
    auth_store::atomic_write_auth(&path, &previous).expect("live Desktop");
    assert!(matches!(
        switcher.switch("target"),
        Err(AppError::DesktopClosePending(_))
    ));
    assert_eq!(
        credentials.load("target").expect("selected login"),
        selected
    );
    assert_eq!(
        auth_store::read_auth(&path).expect("untouched Desktop"),
        previous
    );
}

#[test]
fn archive_failure_preserves_the_selected_login_and_does_not_close_desktop() {
    let fixture = tempfile::tempdir().expect("fixture");
    let credentials = Arc::new(FakeCredentials::default());
    let selected = generation_document("target", "new-login");
    credentials.save(&selected).expect("selected");
    credentials.fail_save.store(true, Ordering::Release);
    let desktop = Arc::new(FakeDesktop::default());
    let (switcher, path) = switcher(
        &fixture,
        credentials.clone(),
        Arc::new(FakeVerifier { failure: None }),
        desktop.clone(),
    );
    let previous = generation_document("target", "old-desktop");
    auth_store::atomic_write_auth(&path, &previous).expect("live Desktop");
    assert!(matches!(
        switcher.switch("target"),
        Err(AppError::Keyring(_))
    ));
    assert_eq!(desktop.stopped.load(Ordering::Acquire), 0);
    assert_eq!(credentials.load("target").expect("selected"), selected);
    assert_eq!(auth_store::read_auth(&path).expect("global"), previous);
}

#[test]
fn real_coffer_pipeline_preserves_new_login_across_close_failure_and_retry() {
    crate::adapters::native_keyring::with_test_store(
        keyring_core::mock::Store::new().expect("memory boundary"),
        || {
            let fixture = tempfile::tempdir().expect("fixture");
            let db =
                crate::adapters::database::Database::open(&fixture.path().join("db")).expect("db");
            let credentials = Arc::new(crate::adapters::credential_vault::CredentialVault::new(
                db.clone(),
            ));
            let selected = generation_document("target", "new-login");
            let primary = credentials.save_generation(&selected).expect("new login");
            let desktop = Arc::new(FakeDesktop::default());
            desktop.stop_failed.store(true, Ordering::Release);
            let (switcher, path) = switcher(
                &fixture,
                credentials.clone(),
                Arc::new(FakeVerifier { failure: None }),
                desktop.clone(),
            );
            let previous = generation_document("target", "old-desktop");
            auth_store::atomic_write_auth(&path, &previous).expect("live Desktop");
            assert!(matches!(
                switcher.switch("target"),
                Err(AppError::DesktopClosePending(_))
            ));
            assert_eq!(
                credentials.load("target").expect("selected intact"),
                selected
            );
            assert_eq!(
                auth_store::read_auth(&path).expect("global intact"),
                previous
            );
            let refs = credentials.generation_refs("target").expect("refs");
            assert_eq!(
                refs.iter()
                    .find(|r| r.is_active)
                    .expect("selected")
                    .generation_id,
                primary.generation_id
            );
            let archived = refs
                .iter()
                .find(|r| !r.is_active)
                .expect("outgoing preserved");
            assert_eq!(
                credentials
                    .load_generation("target", &archived.generation_id)
                    .expect("snapshot"),
                previous
            );
            desktop.stop_failed.store(false, Ordering::Release);
            switcher
                .switch("target")
                .expect("retry without another login");
            assert_eq!(
                credentials.load("target").expect("selected after retry"),
                selected
            );
            assert_eq!(
                auth_store::read_auth(&path).expect("active after retry"),
                selected
            );
            let sql = rusqlite::Connection::open_with_flags(
                fixture.path().join("db/manager.sqlite3"),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .expect("fixture-only observation");
            let active: i64 = sql.query_row("SELECT COUNT(*) FROM credential_generations WHERE account_id='target' AND is_active=1", [], |row| row.get(0)).expect("active flags");
            assert_eq!(active, 1);
            assert_eq!(
                db.active_credential_signature("target").expect("signature"),
                Some(format!("{}:{}", primary.generation_id, primary.fingerprint))
            );
        },
    );
}

impl DesktopLifecycle for FinalSessionDesktop {
    fn stop(&self) -> Result<DesktopState, AppError> {
        auth_store::atomic_write_auth(&self.auth_path, &self.final_document)?;
        Ok(DesktopState::default())
    }
    fn restart(&self, _: &DesktopState) -> Result<DesktopLaunchOutcome, AppError> {
        Ok(DesktopLaunchOutcome::NotRequired)
    }
}

#[test]
fn switching_the_active_account_uses_the_refresh_captured_at_desktop_shutdown() {
    let fixture = tempfile::tempdir().expect("fixture");
    let credentials = Arc::new(FakeCredentials::default());
    let saved = generation_document("target", "before-shutdown");
    credentials.save(&saved).expect("current coffer");
    let mut active = saved;
    active["tokens"]["access_token"] = json!("desktop-newer-access");
    let final_document = generation_document("target", "after-shutdown");
    let auth_path = fixture.path().join("codex-home/auth.json");
    auth_store::atomic_write_auth(&auth_path, &active).expect("live Desktop");
    let switcher = SessionSwitcher::with_dependencies(
        &fixture.path().join("app-data"),
        auth_path.clone(),
        credentials.clone(),
        Arc::new(FakeVerifier { failure: None }),
        Arc::new(FinalSessionDesktop {
            auth_path: auth_path.clone(),
            final_document: final_document.clone(),
        }),
    );
    switcher.switch("target").expect("use final refresh");
    assert_eq!(
        auth_store::read_auth(&auth_path).expect("active"),
        final_document
    );
    assert_eq!(credentials.load("target").expect("coffer"), final_document);
}

struct RefreshingDesktop {
    auth_path: PathBuf,
}

impl DesktopLifecycle for RefreshingDesktop {
    fn stop(&self) -> Result<DesktopState, AppError> {
        auth_store::atomic_write_auth(
            &self.auth_path,
            &generation_document("previous", "final-refresh"),
        )?;
        Ok(DesktopState::default())
    }
    fn restart(&self, _state: &DesktopState) -> Result<DesktopLaunchOutcome, AppError> {
        Ok(DesktopLaunchOutcome::NotRequired)
    }
}

#[test]
fn rollback_preserves_the_refresh_written_during_desktop_shutdown() {
    let fixture = tempfile::tempdir().expect("fixture");
    let auth_path = fixture.path().join("codex-home/auth.json");
    auth_store::atomic_write_auth(&auth_path, &generation_document("previous", "old"))
        .expect("active session");
    let credentials = Arc::new(FakeCredentials::default());
    credentials
        .documents
        .lock()
        .insert("target".to_owned(), document("target"));
    let verifier = Arc::new(FakeVerifier {
        failure: Some("service unavailable".to_owned()),
    });
    let desktop = Arc::new(RefreshingDesktop {
        auth_path: auth_path.clone(),
    });
    let switcher = SessionSwitcher::with_dependencies(
        &fixture.path().join("app-data"),
        auth_path.clone(),
        credentials,
        verifier,
        desktop,
    );
    assert!(switcher.switch("target").is_err());
    let restored = auth_store::read_auth(&auth_path).expect("rollback");
    assert_eq!(restored["tokens"]["generation"], "final-refresh");
}

#[test]
fn successful_switch_reports_the_expensive_stages() {
    let fixture = tempfile::tempdir().expect("fixture");
    let credentials = Arc::new(FakeCredentials::default());
    credentials
        .documents
        .lock()
        .insert("target".to_owned(), document("target"));
    let desktop = Arc::new(FakeDesktop::default());
    let verifier = Arc::new(FakeVerifier { failure: None });
    let (switcher, auth_path) = switcher(&fixture, credentials, verifier, desktop);
    auth_store::atomic_write_auth(&auth_path, &document("previous")).expect("active auth");
    let mut stages = Vec::new();

    let mut outcome = switcher
        .switch_with_trace("target", &mut |stage, _| stages.push(stage))
        .expect("switch");
    assert!(!stages.contains(&"desktop_restart"));
    switcher
        .reopen_committed_desktop(&mut outcome, |_| Ok(true), &mut |stage, _| {
            stages.push(stage)
        })
        .expect("post-commit reopening");

    for expected in [
        "credential_generation_refs",
        "desktop_stop",
        "credential_generation_load",
        "materialize_target_auth",
        "verify_target_session",
        "persist_target_session",
        "desktop_restart",
    ] {
        assert!(stages.contains(&expected), "missing stage: {expected}");
    }
}

#[test]
fn completed_switch_can_be_compensated_when_metadata_persistence_fails() {
    let fixture = tempfile::tempdir().expect("fixture");
    let credentials = Arc::new(FakeCredentials::default());
    credentials
        .documents
        .lock()
        .insert("target".to_owned(), document("target"));
    let desktop = Arc::new(FakeDesktop::default());
    let verifier = Arc::new(FakeVerifier { failure: None });
    let (switcher, auth_path) = switcher(&fixture, credentials, verifier, desktop.clone());
    auth_store::atomic_write_auth(&auth_path, &document("previous")).expect("previous");
    let outcome = switcher.switch("target").expect("switch");

    switcher
        .rollback_materialized_switch(&outcome)
        .expect("compensating rollback");

    let restored = auth_store::read_auth(&auth_path).expect("restored auth");
    assert_eq!(
        auth_store::validate_auth_document(&restored, None).expect("account"),
        "previous"
    );
    assert_eq!(desktop.stopped.load(Ordering::Acquire), 1);
    assert_eq!(desktop.restarted.load(Ordering::Acquire), 1);
}

#[test]
fn rejected_active_generation_preserves_old_documents_without_replaying_their_refresh_tokens() {
    let fixture = tempfile::tempdir().expect("fixture");
    let auth_path = fixture.path().join("codex-home/auth.json");
    let app_data = fixture.path().join("app-data");
    let credentials = Arc::new(CyclingCredentials {
        documents: vec![
            (
                "working".to_owned(),
                generation_document("target", "working"),
            ),
            (
                "reserve-a".to_owned(),
                generation_document("target", "reserve-a"),
            ),
            (
                "reserve-b".to_owned(),
                generation_document("target", "reserve-b"),
            ),
        ],
        loaded: Mutex::new(Vec::new()),
        quarantined: Mutex::new(HashSet::from([
            "reserve-a".to_owned(),
            "reserve-b".to_owned(),
        ])),
    });
    let verifier = Arc::new(DocumentVerifier {
        auth_path: auth_path.clone(),
        failures: HashMap::from([("working".to_owned(), "token_revoked".to_owned())]),
        attempted: Mutex::new(Vec::new()),
    });
    let desktop = Arc::new(FakeDesktop::default());
    let switcher = SessionSwitcher::with_dependencies(
        &app_data,
        auth_path.clone(),
        credentials.clone(),
        verifier.clone(),
        desktop,
    );
    auth_store::atomic_write_auth(&auth_path, &document("previous")).expect("previous");

    assert!(switcher.switch("target").is_err());
    assert_eq!(
        auth_store::read_auth(&auth_path).expect("rollback"),
        document("previous")
    );
    assert_eq!(
        credentials.loaded.lock().as_slice(),
        &["working".to_owned(), "working".to_owned()]
    );
    assert_eq!(
        verifier.attempted.lock().as_slice(),
        &["working".to_owned()]
    );
    let quarantined = credentials.quarantined.lock();
    assert!(quarantined.contains("working"));
    assert!(quarantined.contains("reserve-a"));
    assert!(quarantined.contains("reserve-b"));
}

#[test]
fn transient_failure_stops_the_cycle_without_quarantining() {
    let fixture = tempfile::tempdir().expect("fixture");
    let auth_path = fixture.path().join("codex-home/auth.json");
    let credentials = Arc::new(CyclingCredentials {
        documents: vec![
            (
                "working".to_owned(),
                generation_document("target", "working"),
            ),
            (
                "reserve".to_owned(),
                generation_document("target", "reserve"),
            ),
        ],
        loaded: Mutex::new(Vec::new()),
        quarantined: Mutex::new(HashSet::new()),
    });
    let verifier = Arc::new(DocumentVerifier {
        auth_path: auth_path.clone(),
        failures: HashMap::from([("working".to_owned(), "network timeout".to_owned())]),
        attempted: Mutex::new(Vec::new()),
    });
    let desktop = Arc::new(FakeDesktop::default());
    let switcher = SessionSwitcher::with_dependencies(
        &fixture.path().join("app-data"),
        auth_path.clone(),
        credentials.clone(),
        verifier,
        desktop,
    );
    auth_store::atomic_write_auth(&auth_path, &document("previous")).expect("previous");

    assert!(switcher.switch("target").is_err());
    assert_eq!(
        credentials.loaded.lock().as_slice(),
        &["working".to_owned(), "working".to_owned()]
    );
    assert!(credentials.quarantined.lock().is_empty());
}

//! Keep the real verifier, JSONL client, coordinator and coffer writer under test.
//! Only the remote process and Desktop lifecycle are synthetic boundaries.
use super::*;
use crate::adapters::desktop_control::DesktopLaunchOutcome;
use crate::adapters::{credential_vault::CredentialVault, database::Database};
use parking_lot::Mutex;
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicUsize, Ordering};

fn document(generation: &str) -> Value {
    json!({"auth_mode":"chatgpt","tokens":{"account_id":"synthetic-account","access_token":format!("synthetic-access-{generation}"),"refresh_token":format!("synthetic-refresh-{generation}"),"generation":generation}})
}

struct FixtureDesktop {
    root: PathBuf,
    auth: PathBuf,
    final_document: Mutex<Option<Value>>,
    stops: AtomicUsize,
    restarts: AtomicUsize,
}

impl DesktopLifecycle for FixtureDesktop {
    fn stop(&self) -> Result<DesktopState, AppError> {
        self.stops.fetch_add(1, Ordering::AcqRel);
        if let Some(doc) = self.final_document.lock().as_ref() {
            auth_store::atomic_write_auth(&self.auth, doc)?;
        }
        std::fs::write(self.root.join("desktop-stopped"), b"synthetic")?;
        Ok(DesktopState::default())
    }
    fn restart(&self, _: &DesktopState) -> Result<DesktopLaunchOutcome, AppError> {
        self.restarts.fetch_add(1, Ordering::AcqRel);
        Ok(DesktopLaunchOutcome::NotRequired)
    }
}

struct Fixture {
    directory: tempfile::TempDir,
    credentials: Arc<CredentialVault>,
    desktop: Arc<FixtureDesktop>,
    switcher: SessionSwitcher,
}

impl Fixture {
    fn new(scenario: Value) -> Self {
        let directory = tempfile::tempdir().expect("private fixture");
        let root = directory.path();
        let home = root.join("home");
        auth_store::atomic_write_auth(&home.join("auth.json"), &document("initial"))
            .expect("synthetic active session");
        let binary = root.join("server.py");
        std::fs::write(&binary, include_str!("verifier_fixture.py")).expect("synthetic process");
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700))
            .expect("private executable");
        std::fs::write(root.join("scenario.json"), scenario.to_string()).expect("scenario");
        let credentials = Arc::new(CredentialVault::new(
            Database::open(&root.join("db")).expect("db"),
        ));
        credentials
            .save(&document("initial"))
            .expect("selected session");
        let desktop = Arc::new(FixtureDesktop {
            root: root.to_path_buf(),
            auth: home.join("auth.json"),
            final_document: Mutex::new(None),
            stops: AtomicUsize::new(0),
            restarts: AtomicUsize::new(0),
        });
        let switcher = SessionSwitcher::with_dependencies(
            &root.join("state"),
            home.join("auth.json"),
            credentials.clone(),
            Arc::new(AppServerSessionVerifier::with_binary(home, binary)),
            desktop.clone(),
        );
        Self {
            directory,
            credentials,
            desktop,
            switcher,
        }
    }
    fn wire(&self) -> Vec<Value> {
        std::fs::read_to_string(self.directory.path().join("requests.jsonl"))
            .expect("safe RPC trace")
            .lines()
            .map(|line| serde_json::from_str(line).expect("trace JSON"))
            .collect()
    }
    fn active(&self) -> Value {
        auth_store::read_auth(&self.switcher.auth_path).expect("synthetic active auth")
    }
}

fn with_fixture(scenario: Value, test: impl FnOnce(Fixture)) {
    crate::adapters::native_keyring::with_test_store(
        keyring_core::mock::Store::new().expect("memory boundary"),
        || test(Fixture::new(scenario)),
    );
}

#[test]
fn active_expired_session_reaches_real_managed_renewal_in_the_same_use_operation() {
    with_fixture(json!({}), |fixture| {
        let prepared = fixture
            .switcher
            .preflight_target(
                "synthetic-account",
                Arc::new(std::sync::atomic::AtomicBool::new(false)),
            )
            .expect("expiry must not stop recovery before exclusive ownership");
        assert_eq!(fixture.desktop.stops.load(Ordering::Acquire), 0);
        assert!(fixture
            .wire()
            .iter()
            .all(|rpc| rpc["external"] == true || rpc["method"] == "initialize"));
        assert!(!fixture.directory.path().join("rotated").exists());
        let outcome = fixture
            .switcher
            .apply_prepared_destination(prepared, &mut |_, _| {})
            .expect("automatic managed renewal");
        assert!(outcome.result.changed);
        assert_eq!(outcome.snapshot.plan_type.as_deref(), Some("plus"));
        assert_eq!(fixture.desktop.stops.load(Ordering::Acquire), 1);
        assert_eq!(fixture.active()["tokens"]["generation"], "renewed");
        assert_eq!(
            fixture
                .credentials
                .load("synthetic-account")
                .expect("saved"),
            fixture.active()
        );
        let wire = fixture.wire();
        let refresh: Vec<_> = wire.iter().filter(|rpc| rpc["refresh"] == true).collect();
        assert_eq!(refresh.len(), 1);
        assert!(refresh
            .iter()
            .all(|rpc| rpc["external"] == false && rpc["stopped"] == true));
    });
}

#[test]
fn real_verifier_healthy_active_session_without_published_quota_does_not_restart_desktop() {
    with_fixture(json!({"healthy":true,"quota_failure":true}), |fixture| {
        let outcome = fixture
            .switcher
            .switch("synthetic-account")
            .expect("confirmed identity without quota");
        assert!(!outcome.result.changed);
        assert!(outcome.snapshot.rate_limits.is_none());
        assert_eq!(fixture.desktop.stops.load(Ordering::Acquire), 0);
        assert!(!fixture.wire().iter().any(|rpc| rpc["refresh"] == true));
    });
}

#[test]
fn real_verifier_uses_the_final_shutdown_generation_instead_of_the_stale_preflight_reference() {
    with_fixture(json!({"expected_generation":"shutdown"}), |fixture| {
        *fixture.desktop.final_document.lock() = Some(document("shutdown"));
        fixture
            .switcher
            .switch("synthetic-account")
            .expect("captured shutdown chain");
        assert_eq!(fixture.active()["tokens"]["generation"], "renewed");
        assert_eq!(
            fixture
                .credentials
                .load("synthetic-account")
                .expect("latest"),
            fixture.active()
        );
    });
}

#[test]
fn real_verifier_network_failure_preserves_desktop_and_does_not_attempt_refresh() {
    with_fixture(json!({"network_failure":true}), |fixture| {
        assert!(matches!(
            fixture.switcher.preflight_target(
                "synthetic-account",
                Arc::new(std::sync::atomic::AtomicBool::new(false))
            ),
            Err(AppError::Rpc { .. })
        ));
        assert_eq!(fixture.active(), document("initial"));
        assert_eq!(fixture.desktop.stops.load(Ordering::Acquire), 0);
        assert!(!fixture.wire().iter().any(|rpc| rpc["refresh"] == true));
    });
}

#[test]
fn rotated_session_is_preserved_when_the_remote_identity_still_cannot_be_confirmed() {
    with_fixture(json!({"still_null":true}), |fixture| {
        assert!(matches!(
            fixture.switcher.switch("synthetic-account"),
            Err(AppError::SessionRenewalRequired(_))
        ));
        assert_eq!(
            fixture.active()["tokens"]["generation"],
            "renewed",
            "rollback cannot replay the rotated chain"
        );
        assert_eq!(
            fixture
                .credentials
                .load("synthetic-account")
                .expect("preserved rotation"),
            fixture.active()
        );
        assert_eq!(
            fixture
                .wire()
                .iter()
                .filter(|rpc| rpc["refresh"] == true)
                .count(),
            1
        );
    });
}

#[test]
fn rejected_refresh_never_replays_archived_generations_or_reports_success() {
    with_fixture(json!({"refresh_rejected":true}), |fixture| {
        fixture
            .credentials
            .archive_generation(&document("old-reserve"))
            .expect("historical snapshot");
        let error = fixture
            .switcher
            .switch("synthetic-account")
            .err()
            .expect("rejected");
        assert!(error.is_refresh_rejected());
        assert_eq!(fixture.active(), document("initial"));
        assert_eq!(
            fixture
                .credentials
                .load("synthetic-account")
                .expect("preserved"),
            document("initial")
        );
        assert_eq!(
            fixture
                .wire()
                .iter()
                .filter(|rpc| rpc["refresh"] == true)
                .count(),
            1
        );
    });
}

#[test]
fn corruption_of_global_auth_is_not_treated_as_absence_or_permission_to_refresh_a_copy() {
    with_fixture(json!({}), |fixture| {
        std::fs::write(&fixture.switcher.auth_path, b"corrupt-synthetic-auth")
            .expect("corrupt fixture");
        assert!(matches!(
            fixture.switcher.preflight_target(
                "synthetic-account",
                Arc::new(std::sync::atomic::AtomicBool::new(false))
            ),
            Err(AppError::Json(_))
        ));
        assert_eq!(
            std::fs::read(&fixture.switcher.auth_path).expect("unchanged"),
            b"corrupt-synthetic-auth"
        );
        assert!(!fixture.directory.path().join("requests.jsonl").exists());
        assert_eq!(fixture.desktop.stops.load(Ordering::Acquire), 0);
    });
}

#[test]
fn generic_access_unauthorized_recovers_but_explicit_refresh_rejection_stops_before_desktop() {
    with_fixture(json!({"access_unauthorized":true}), |fixture| {
        fixture
            .switcher
            .switch("synthetic-account")
            .expect("expired access is recoverable");
        assert_eq!(fixture.active()["tokens"]["generation"], "renewed");
    });
    with_fixture(json!({"access_refresh_rejected":true}), |fixture| {
        let error = fixture
            .switcher
            .preflight_target(
                "synthetic-account",
                Arc::new(std::sync::atomic::AtomicBool::new(false)),
            )
            .err()
            .expect("explicit rejection");
        assert!(error.is_refresh_rejected());
        assert_eq!(fixture.desktop.stops.load(Ordering::Acquire), 0);
        assert!(!fixture.wire().iter().any(|rpc| rpc["refresh"] == true));
    });
}

#[test]
fn cancellation_interrupts_the_actual_preflight_rpc_without_closing_desktop_or_losing_auth() {
    with_fixture(json!({"blocked_read":true}), |fixture| {
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let signal = cancel.clone();
        let trace = fixture.directory.path().join("requests.jsonl");
        let sender = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            while std::time::Instant::now() < deadline {
                if std::fs::read_to_string(&trace).is_ok_and(|text| text.contains("account/read")) {
                    signal.store(true, Ordering::Release);
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            signal.store(true, Ordering::Release);
            panic!("preflight never reached the blocked remote request");
        });
        let started = std::time::Instant::now();
        let result = fixture
            .switcher
            .preflight_target("synthetic-account", cancel);
        sender.join().expect("cancellation signal");
        assert!(matches!(result, Err(AppError::Cancelled)));
        assert!(started.elapsed() < Duration::from_millis(750));
        assert_eq!(fixture.desktop.stops.load(Ordering::Acquire), 0);
        assert_eq!(fixture.active(), document("initial"));
        #[cfg(target_os = "linux")]
        for rpc in fixture.wire() {
            assert!(
                !std::path::Path::new(&format!(
                    "/proc/{}",
                    rpc["pid"].as_u64().expect("fixture child PID")
                ))
                .exists(),
                "owned preflight child was not reaped"
            );
        }
    });
}

struct CofferUnavailableAfterRotation {
    inner: Arc<CredentialVault>,
    rotation_marker: PathBuf,
}
impl CredentialStore for CofferUnavailableAfterRotation {
    fn save(&self, document: &Value) -> Result<String, AppError> {
        if self.rotation_marker.exists() {
            return Err(AppError::CredentialUnavailable(
                "synthetic coffer outage after rotation".to_owned(),
            ));
        }
        self.inner.save(document)
    }
    fn load(&self, id: &str) -> Result<Value, AppError> {
        self.inner.load(id)
    }
    fn load_optional(&self, id: &str) -> Result<Option<Value>, AppError> {
        self.inner.load_optional(id)
    }
    fn delete(&self, _: &str) -> Result<(), AppError> {
        unreachable!("no account deletion in renewal")
    }
    fn generation_refs(&self, id: &str) -> Result<Vec<StoredCredentialGeneration>, AppError> {
        self.inner.generation_refs(id)
    }
    fn load_generation(&self, id: &str, generation: &str) -> Result<Value, AppError> {
        self.inner.load_generation(id, generation)
    }
    fn archive_generation(&self, doc: &Value) -> Result<StoredCredentialGeneration, AppError> {
        self.inner.archive_generation(doc)
    }
    fn recover_generation(&self, id: &str, generation: &str) -> Result<(), AppError> {
        self.inner.recover_generation(id, generation)
    }
}
struct FixtureKeys;
impl crate::adapters::application_recovery::RecoveryKeyStore for FixtureKeys {
    fn load_or_create(&self, _: &str) -> Result<[u8; 32], AppError> {
        Ok([37; 32])
    }
    fn delete(&self, _: &str) -> Result<(), AppError> {
        unreachable!("no recovery deletion")
    }
}

#[test]
fn coffer_outage_after_real_managed_rotation_preserves_protected_record_and_never_reopens_old_owner(
) {
    with_fixture(json!({}), |mut fixture| {
        let root = fixture.directory.path();
        let inner = Arc::new(CofferUnavailableAfterRotation {
            inner: fixture.credentials.clone(),
            rotation_marker: root.join("rotated"),
        });
        fixture.switcher.credentials = Arc::new(
            crate::adapters::desktop_credential_recovery::RecoverableDesktopStore::new(
                root,
                inner,
                Arc::new(FixtureKeys),
            ),
        );
        assert!(matches!(
            fixture.switcher.switch("synthetic-account"),
            Err(AppError::CredentialRecoveryPending(_))
        ));
        assert_eq!(fixture.active()["tokens"]["generation"], "renewed");
        assert_eq!(fixture.desktop.restarts.load(Ordering::Acquire), 0);
        assert!(fixture.switcher.journal_path.exists());
        let sealed_root = root.join("desktop-recovery/application-recovery");
        let records: Vec<_> = std::fs::read_dir(&sealed_root)
            .expect("protected records")
            .map(|e| e.expect("entry").path())
            .collect();
        assert_eq!(records.len(), 1);
        let bytes = std::fs::read(&records[0]).expect("sealed document");
        assert!(!bytes
            .windows(b"synthetic-refresh-renewed".len())
            .any(|w| w == b"synthetic-refresh-renewed"));
        std::fs::remove_file(root.join("rotated")).expect("restore fixture coffer availability");
        fixture
            .switcher
            .credentials
            .generation_refs("synthetic-account")
            .expect("recover protected rotation before reuse");
        assert_eq!(
            fixture
                .switcher
                .credentials
                .load("synthetic-account")
                .expect("recovered"),
            fixture.active()
        );
        assert_eq!(
            std::fs::read_dir(&sealed_root)
                .expect("completed record removed")
                .count(),
            0
        );
        fixture
            .switcher
            .recover(|_| Ok(false))
            .expect("recover interrupted transaction without old refresh");
        assert_eq!(fixture.active()["tokens"]["generation"], "renewed");
    });
}

#[test]
fn a_new_login_for_the_active_identity_renews_only_the_selected_session_after_stop() {
    with_fixture(json!({"expected_generation":"new-login"}), |fixture| {
        fixture
            .credentials
            .save(&document("new-login"))
            .expect("selected reauthentication");
        fixture
            .switcher
            .switch("synthetic-account")
            .expect("activate renewed new login");
        assert_eq!(fixture.active()["tokens"]["generation"], "renewed");
        let refs = fixture
            .credentials
            .generation_refs("synthetic-account")
            .expect("history preserved");
        assert!(refs.iter().filter(|r| !r.is_active).any(|r| fixture
            .credentials
            .load_generation("synthetic-account", &r.generation_id)
            .is_ok_and(|doc| doc == document("initial"))));
    });
}

#[test]
fn inactive_preflight_preserves_rotation_on_error_without_touching_desktop_or_global_auth() {
    with_fixture(
        json!({"requires_stop":false,"still_null":true}),
        |fixture| {
            std::fs::remove_file(&fixture.switcher.auth_path).expect("no active Desktop session");
            assert!(matches!(
                fixture.switcher.preflight_target(
                    "synthetic-account",
                    Arc::new(std::sync::atomic::AtomicBool::new(false))
                ),
                Err(AppError::SessionRenewalRequired(_))
            ));
            assert!(!fixture.switcher.auth_path.exists());
            assert_eq!(fixture.desktop.stops.load(Ordering::Acquire), 0);
            assert_eq!(
                fixture
                    .credentials
                    .load("synthetic-account")
                    .expect("rotation saved")["tokens"]["generation"],
                "renewed"
            );
        },
    );
}

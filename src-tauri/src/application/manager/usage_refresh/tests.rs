//! The real manager chooses waiting, identity, publication and cache reuse.
//! The reader controls only remote I/O; the coffer and DB are synthetic/private.
use super::*;
use crate::{
    adapters::database::Database,
    domain::{AccountMetadata, AccountSnapshot, RateLimits, UsageWindow},
    ports::{AccessSession, CredentialStore, UsageReader},
};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicUsize},
        mpsc, Arc,
    },
    thread,
    time::Duration,
};

struct Credentials {
    documents: Mutex<HashMap<String, Value>>,
    locked: AtomicBool,
}
impl CredentialStore for Credentials {
    fn check_read_access(&self) -> Result<(), AppError> {
        if self.locked.load(Ordering::Acquire) {
            return Err(AppError::CredentialUnavailable(
                "synthetic locked coffer".to_owned(),
            ));
        }
        Ok(())
    }
    fn save(&self, doc: &Value) -> Result<String, AppError> {
        let id = crate::adapters::auth_store::validate_auth_document(doc, None)?;
        self.documents.lock().insert(id.clone(), doc.clone());
        Ok(id)
    }
    fn load(&self, id: &str) -> Result<Value, AppError> {
        self.check_read_access()?;
        self.documents
            .lock()
            .get(id)
            .cloned()
            .ok_or_else(|| AppError::Keyring("missing fixture".to_owned()))
    }
    fn load_optional(&self, id: &str) -> Result<Option<Value>, AppError> {
        self.load(id).map(Some)
    }
    fn delete(&self, _: &str) -> Result<(), AppError> {
        unreachable!("quota cannot delete credentials")
    }
}

struct Reader {
    calls: AtomicUsize,
    active: AtomicUsize,
    max_active: AtomicUsize,
    entered: mpsc::SyncSender<(String, String)>,
    replies: Mutex<mpsc::Receiver<Option<f64>>>,
}
impl UsageReader for Reader {
    fn read(
        &self,
        access: &AccessSession,
        cancel: Arc<AtomicBool>,
    ) -> Result<AccountSnapshot, AppError> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        let active = self.active.fetch_add(1, Ordering::AcqRel) + 1;
        self.max_active.fetch_max(active, Ordering::AcqRel);
        let result = self.sample(access, &cancel);
        self.active.fetch_sub(1, Ordering::AcqRel);
        result
    }
}
impl Reader {
    fn sample(
        &self,
        access: &AccessSession,
        cancel: &AtomicBool,
    ) -> Result<AccountSnapshot, AppError> {
        self.entered
            .send((access.account_id.clone(), access.access_token.clone()))
            .expect("private fixture signal");
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if cancel.load(Ordering::Acquire) {
                return Err(AppError::Cancelled);
            }
            match self.replies.lock().recv_timeout(Duration::from_millis(10)) {
                Ok(used) => {
                    return Ok(AccountSnapshot {
                        email: None,
                        plan_type: None,
                        rate_limits: used.map(|percent| RateLimits {
                            primary: Some(UsageWindow {
                                used_percent: Some(percent),
                                ..UsageWindow::default()
                            }),
                            ..RateLimits::default()
                        }),
                    })
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(AppError::AppServer("fixture closed".to_owned()))
                }
                Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() < deadline => {}
                Err(_) => return Err(AppError::AppServer("fixture response deadline".to_owned())),
            }
        }
    }
}

struct Fixture {
    core: ManagerCore,
    credentials: Arc<Credentials>,
    reader: Arc<Reader>,
    entered: mpsc::Receiver<(String, String)>,
    replies: mpsc::SyncSender<Option<f64>>,
    profiles: Vec<String>,
    _directory: tempfile::TempDir,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.core.shutdown().expect("synthetic workers must finish");
        self.core.usage_reader.close();
    }
}
fn fixture() -> Fixture {
    let directory = tempfile::tempdir().expect("private fixture");
    let database = Database::open(directory.path()).expect("db");
    let credentials = Arc::new(Credentials {
        documents: Mutex::new(HashMap::new()),
        locked: AtomicBool::new(false),
    });
    let mut profiles = Vec::new();
    for (id, name) in [("a", "A poll"), ("b", "B manual")] {
        credentials.save(&json!({"auth_mode":"chatgpt","tokens":{"account_id":id,"access_token":format!("synthetic-{id}-v1"),"refresh_token":"never-passed-to-reader"}})).expect("coffer");
        let profile = database.create_profile(name, None, None).expect("profile");
        database
            .persist_connected_account(
                &profile.id,
                AccountMetadata {
                    account_id: id,
                    email: None,
                    plan_type: Some("plus"),
                    credential_ref: None,
                    rate_limits: None,
                    observed_at: Utc::now(),
                },
                None,
            )
            .expect("metadata");
        profiles.push(profile.id);
    }
    let (entered_tx, entered) = mpsc::sync_channel(8);
    let (replies, replies_rx) = mpsc::sync_channel(8);
    let reader = Arc::new(Reader {
        calls: AtomicUsize::new(0),
        active: AtomicUsize::new(0),
        max_active: AtomicUsize::new(0),
        entered: entered_tx,
        replies: Mutex::new(replies_rx),
    });
    let mut core = ManagerCore::from_components(
        directory.path().to_path_buf(),
        None,
        database,
        credentials.clone(),
    )
    .expect("core");
    core.usage_reader = reader.clone();
    Fixture {
        core,
        credentials,
        reader,
        entered,
        replies,
        profiles,
        _directory: directory,
    }
}
fn poll(core: &ManagerCore) -> thread::JoinHandle<bool> {
    let lease = core.tasks.enter().expect("maintenance ownership");
    let core = core.clone();
    thread::spawn(move || {
        let _lease = lease;
        core.refresh_due_usage(&mut UsageSchedule::default())
    })
}
fn check(
    core: &ManagerCore,
    profile: &str,
) -> (thread::JoinHandle<()>, mpsc::Receiver<ConnectionCheck>) {
    let core = core.clone();
    let profile = profile.to_owned();
    let (sender, result) = mpsc::sync_channel(1);
    (
        thread::spawn(move || {
            sender
                .send(core.check_profile(&profile).expect("check report"))
                .expect("result receiver");
        }),
        result,
    )
}

#[test]
fn manual_check_waits_for_and_reuses_the_matching_inflight_poll_instead_of_reporting_busy() {
    let fixture = fixture();
    let background = poll(&fixture.core);
    assert_eq!(
        fixture
            .entered
            .recv_timeout(Duration::from_secs(2))
            .expect("poll reached reader")
            .0,
        "a"
    );
    let (manual, result) = check(&fixture.core, &fixture.profiles[0]);
    let early = result.recv_timeout(Duration::from_millis(100));
    let returned_early = early.is_ok();
    fixture.replies.send(Some(20.0)).expect("finish poll");
    assert!(background.join().expect("poll worker"));
    let report = early.unwrap_or_else(|_| {
        result
            .recv_timeout(Duration::from_secs(2))
            .expect("joined report")
    });
    manual.join().expect("manual worker");
    assert!(
        !returned_early,
        "poll contention belongs to the coordinator, not the user"
    );
    assert!(report.confirmed);
    assert_eq!(
        fixture.reader.calls.load(Ordering::Acquire),
        1,
        "compatible in-flight read must not create a second RPC"
    );
    assert_eq!(fixture.reader.max_active.load(Ordering::Acquire), 1);
    let profile = fixture
        .core
        .database
        .profile(&fixture.profiles[0])
        .expect("published");
    assert_eq!(
        profile
            .rate_limits
            .expect("quota")
            .primary
            .expect("window")
            .used_percent,
        Some(20.0)
    );
}

#[test]
fn coffer_locked_while_waiting_prevents_reusing_the_successful_poll() {
    let fixture = fixture();
    let background = poll(&fixture.core);
    fixture
        .entered
        .recv_timeout(Duration::from_secs(2))
        .expect("poll started");
    let (manual, result) = check(&fixture.core, &fixture.profiles[0]);
    let early = result.recv_timeout(Duration::from_millis(100));
    fixture.credentials.locked.store(true, Ordering::Release);
    fixture.replies.send(Some(20.0)).expect("finish poll");
    background.join().expect("poll");
    let report =
        early.unwrap_or_else(|_| result.recv_timeout(Duration::from_secs(2)).expect("report"));
    manual.join().expect("manual");
    assert!(!report.confirmed);
    assert_eq!(report.error_code.as_deref(), Some("credential_unavailable"));
    assert!(fixture.core.usage_access.lock().is_empty());
}

#[test]
fn another_account_in_the_reader_is_not_reused_for_the_manual_target() {
    let fixture = fixture();
    let background = poll(&fixture.core);
    assert_eq!(
        fixture
            .entered
            .recv_timeout(Duration::from_secs(2))
            .expect("poll")
            .0,
        "a"
    );
    let (manual, result) = check(&fixture.core, &fixture.profiles[1]);
    let early = result.recv_timeout(Duration::from_millis(100));
    fixture.replies.send(Some(20.0)).expect("finish a");
    background.join().expect("poll");
    if early.is_err() {
        assert_eq!(
            fixture
                .entered
                .recv_timeout(Duration::from_secs(2))
                .expect("manual target")
                .0,
            "b"
        );
        fixture.replies.send(Some(65.0)).expect("finish b");
    }
    let report =
        early.unwrap_or_else(|_| result.recv_timeout(Duration::from_secs(2)).expect("report"));
    manual.join().expect("manual");
    assert!(report.confirmed);
    assert_eq!(fixture.reader.calls.load(Ordering::Acquire), 2);
    assert_eq!(fixture.reader.max_active.load(Ordering::Acquire), 1);
    let b = fixture
        .core
        .database
        .profile(&fixture.profiles[1])
        .expect("b");
    assert_eq!(
        b.rate_limits
            .expect("quota")
            .primary
            .expect("window")
            .used_percent,
        Some(65.0)
    );
}

#[test]
fn check_does_not_silently_follow_a_profile_reassigned_to_another_account_while_queued() {
    let fixture = fixture();
    let background = poll(&fixture.core);
    fixture
        .entered
        .recv_timeout(Duration::from_secs(2))
        .expect("poll");
    let (manual, result) = check(&fixture.core, &fixture.profiles[0]);
    let early = result.recv_timeout(Duration::from_millis(100));
    fixture.credentials.save(&json!({"auth_mode":"chatgpt","tokens":{"account_id":"c","access_token":"synthetic-c-v1","refresh_token":"never-passed-to-reader"}})).expect("different account");
    fixture
        .core
        .database
        .persist_connected_account(
            &fixture.profiles[0],
            AccountMetadata {
                account_id: "c",
                email: None,
                plan_type: Some("free"),
                credential_ref: None,
                rate_limits: None,
                observed_at: Utc::now(),
            },
            None,
        )
        .expect("explicit reassociation in fixture");
    fixture.replies.send(Some(20.0)).expect("finish old target");
    fixture
        .replies
        .send(Some(65.0))
        .expect("bound an incorrect new-target request too");
    assert!(!background.join().expect("old poll must be discarded"));
    let report =
        early.unwrap_or_else(|_| result.recv_timeout(Duration::from_secs(2)).expect("report"));
    manual.join().expect("manual");
    assert!(
        !report.confirmed,
        "the original request must not confirm the newly associated identity"
    );
    assert_eq!(report.title, "A conta ou sessão mudou");
    assert_eq!(report.error_code.as_deref(), Some("query_context_changed"));
    assert!(!report.can_renew);
    assert_eq!(fixture.reader.calls.load(Ordering::Acquire), 1);
    assert!(fixture
        .core
        .database
        .profile(&fixture.profiles[0])
        .expect("new association intact")
        .usage_updated_at
        .is_none());
}

#[test]
fn access_changed_in_the_active_desktop_invalidates_coalescing_even_without_sql_generation_change()
{
    let fixture = fixture();
    let auth_path = fixture.core.codex_home.join("auth.json");
    let mut document = fixture.credentials.load("a").expect("synthetic auth");
    crate::adapters::auth_store::atomic_write_auth(&auth_path, &document)
        .expect("fixture Desktop auth");
    let background = poll(&fixture.core);
    fixture
        .entered
        .recv_timeout(Duration::from_secs(2))
        .expect("poll");
    let (manual, result) = check(&fixture.core, &fixture.profiles[0]);
    let early = result.recv_timeout(Duration::from_millis(100));
    document["tokens"]["access_token"] = json!("synthetic-a-v2");
    crate::adapters::auth_store::atomic_write_auth(&auth_path, &document)
        .expect("fixture owner updates access");
    fixture.replies.send(Some(20.0)).expect("first sample");
    background.join().expect("poll");
    if early.is_err() {
        assert_eq!(
            fixture
                .entered
                .recv_timeout(Duration::from_secs(2))
                .expect("new access query"),
            ("a".to_owned(), "synthetic-a-v2".to_owned())
        );
        fixture.replies.send(Some(30.0)).expect("fresh sample");
    }
    let report =
        early.unwrap_or_else(|_| result.recv_timeout(Duration::from_secs(2)).expect("report"));
    manual.join().expect("manual");
    assert!(report.confirmed);
    assert_eq!(fixture.reader.calls.load(Ordering::Acquire), 2);
    assert_eq!(
        crate::adapters::auth_store::read_auth(&auth_path).expect("Desktop preserved"),
        document
    );
}

#[test]
fn a_sample_completed_before_the_manual_request_does_not_count_as_a_new_check() {
    let fixture = fixture();
    fixture.replies.send(Some(20.0)).expect("first sample");
    assert!(poll(&fixture.core).join().expect("poll"));
    fixture
        .entered
        .recv_timeout(Duration::from_secs(2))
        .expect("poll frame");
    let (manual, result) = check(&fixture.core, &fixture.profiles[0]);
    assert_eq!(
        fixture
            .entered
            .recv_timeout(Duration::from_secs(2))
            .expect("new check frame")
            .0,
        "a"
    );
    fixture.replies.send(Some(30.0)).expect("new sample");
    let report = result.recv_timeout(Duration::from_secs(2)).expect("report");
    manual.join().expect("manual");
    assert!(report.confirmed);
    assert_eq!(fixture.reader.calls.load(Ordering::Acquire), 2);
}

#[test]
fn generation_changed_while_waiting_discards_the_old_sample_and_uses_new_access() {
    let fixture = fixture();
    let background = poll(&fixture.core);
    fixture
        .entered
        .recv_timeout(Duration::from_secs(2))
        .expect("poll");
    let (manual, result) = check(&fixture.core, &fixture.profiles[0]);
    let early = result.recv_timeout(Duration::from_millis(100));
    let mut document = fixture.credentials.load("a").expect("old access");
    document["tokens"]["access_token"] = json!("synthetic-a-v2");
    let generation = fixture
        .credentials
        .save_generation(&document)
        .expect("fixture new generation");
    fixture
        .core
        .database
        .upsert_credential_generation(&generation, true)
        .expect("new signature");
    fixture.replies.send(Some(20.0)).expect("old response");
    assert!(!background.join().expect("stale poll discarded"));
    if early.is_err() {
        assert_eq!(
            fixture
                .entered
                .recv_timeout(Duration::from_secs(2))
                .expect("new generation query"),
            ("a".to_owned(), "synthetic-a-v2".to_owned())
        );
        fixture.replies.send(Some(30.0)).expect("new response");
    }
    let report =
        early.unwrap_or_else(|_| result.recv_timeout(Duration::from_secs(2)).expect("report"));
    manual.join().expect("manual");
    assert!(report.confirmed);
    let profile = fixture
        .core
        .database
        .profile(&fixture.profiles[0])
        .expect("published");
    assert_eq!(
        profile
            .rate_limits
            .expect("quota")
            .primary
            .expect("window")
            .used_percent,
        Some(30.0)
    );
}

#[test]
fn an_unpublished_response_preserves_the_previous_quota_and_reports_its_actual_observation_time() {
    let fixture = fixture();
    let previous = AccountSnapshot {
        email: None,
        plan_type: None,
        rate_limits: Some(RateLimits {
            primary: Some(UsageWindow {
                used_percent: Some(70.0),
                ..UsageWindow::default()
            }),
            ..RateLimits::default()
        }),
    };
    fixture
        .core
        .database
        .update_usage("a", &previous)
        .expect("last observed quota");
    let before = fixture
        .core
        .database
        .profile(&fixture.profiles[0])
        .expect("before");
    let (manual, result) = check(&fixture.core, &fixture.profiles[0]);
    fixture
        .entered
        .recv_timeout(Duration::from_secs(2))
        .expect("manual read");
    fixture
        .replies
        .send(None)
        .expect("server does not publish quota");
    let report = result.recv_timeout(Duration::from_secs(2)).expect("report");
    manual.join().expect("manual");
    assert_eq!(report.title, "Saldo não publicado pelo servidor");
    let after = fixture
        .core
        .database
        .profile(&fixture.profiles[0])
        .expect("after");
    assert_eq!(after.rate_limits, before.rate_limits);
    assert_eq!(after.usage_updated_at, before.usage_updated_at);
    let query = fixture.core.usage_lock.lock();
    assert_eq!(
        report.checked_at,
        query
            .as_ref()
            .expect("successful observation")
            .observation
            .checked_at
    );
}

#[test]
fn shutdown_cancels_a_queued_check_and_running_sample_and_drains_their_leases() {
    let fixture = fixture();
    let background = poll(&fixture.core);
    fixture
        .entered
        .recv_timeout(Duration::from_secs(2))
        .expect("poll");
    let (manual, result) = check(&fixture.core, &fixture.profiles[0]);
    let early = result.recv_timeout(Duration::from_millis(100));
    let started = Instant::now();
    fixture.core.shutdown().expect("drain without forcing exit");
    background.join().expect("poll cancelled");
    let report = early.unwrap_or_else(|_| {
        result
            .recv_timeout(Duration::from_secs(2))
            .expect("cancelled report")
    });
    manual.join().expect("manual");
    assert_eq!(report.error_code.as_deref(), Some("cancelled"));
    assert!(!report.confirmed);
    assert_eq!(fixture.reader.calls.load(Ordering::Acquire), 1);
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(fixture
        .core
        .database
        .profile(&fixture.profiles[0])
        .expect("preserved")
        .usage_updated_at
        .is_none());
}

use super::{
    digest, encode_index_pointer, fits_native_entry, index_pointer, split_for_keyring,
    validate_v3_manifest, GenerationManifest, V3Manifest, V3_PREFIX,
};
use crate::ports::CredentialStore;
use chrono::Utc;
use uuid::Uuid;

fn synthetic_document(generation: &str) -> serde_json::Value {
    serde_json::json!({"auth_mode":"chatgpt","tokens":{"account_id":"synthetic-account","access_token":format!("synthetic-access-{generation}"),"refresh_token":format!("synthetic-refresh-{generation}")}})
}

fn with_memory_coffer(test: impl FnOnce()) {
    super::super::native_keyring::with_test_store(
        keyring_core::mock::Store::new().expect("memory-only boundary"),
        test,
    );
}

struct FaultStore {
    inner: std::sync::Arc<keyring_core::mock::Store>,
    fail_main_write: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl FaultStore {
    fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            inner: keyring_core::mock::Store::new().expect("memory store"),
            fail_main_write: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }
    fn live_entries(&self) -> usize {
        use keyring_core::api::CredentialApi;
        let guard = self.inner.inner.lock().expect("memory-only state");
        let entries = guard.borrow();
        entries
            .iter()
            .filter(|entry| entry.get_secret().is_ok())
            .count()
    }
}

impl keyring_core::api::CredentialStoreApi for FaultStore {
    fn vendor(&self) -> String {
        "synthetic-memory-boundary".to_owned()
    }
    fn id(&self) -> String {
        "synthetic-write-failure".to_owned()
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn build(
        &self,
        service: &str,
        user: &str,
        modifiers: Option<&std::collections::HashMap<&str, &str>>,
    ) -> keyring_core::Result<keyring_core::Entry> {
        let inner = self.inner.build(service, user, modifiers)?;
        if user != "account:synthetic-account" {
            return Ok(inner);
        }
        Ok(keyring_core::Entry::new_with_credential(
            std::sync::Arc::new(FaultEntry {
                inner,
                fail_write: self.fail_main_write.clone(),
            }),
        ))
    }
}

struct FaultEntry {
    inner: keyring_core::Entry,
    fail_write: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl keyring_core::api::CredentialApi for FaultEntry {
    fn set_secret(&self, secret: &[u8]) -> keyring_core::Result<()> {
        if self.fail_write.load(std::sync::atomic::Ordering::Acquire) {
            return Err(keyring_core::Error::NoStorageAccess(Box::new(
                std::io::Error::other("synthetic commit failure"),
            )));
        }
        self.inner.set_secret(secret)
    }
    fn get_secret(&self) -> keyring_core::Result<Vec<u8>> {
        self.inner.get_secret()
    }
    fn delete_credential(&self) -> keyring_core::Result<()> {
        self.inner.delete_credential()
    }
    fn get_credential(
        &self,
    ) -> keyring_core::Result<Option<std::sync::Arc<keyring_core::Credential>>> {
        self.inner.get_secret()?;
        Ok(None)
    }
    fn get_specifiers(&self) -> Option<(String, String)> {
        self.inner.get_specifiers()
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[test]
fn failed_legacy_archive_commit_preserves_main_entry_and_cleans_uncommitted_chunks() {
    let boundary = FaultStore::new();
    super::super::native_keyring::with_test_store(boundary.clone(), || {
        let selected = synthetic_document("legacy");
        let main = super::main_entry("synthetic-account").expect("entry");
        main.set_password(&selected.to_string())
            .expect("old format");
        let before = boundary.live_entries();
        boundary
            .fail_main_write
            .store(true, std::sync::atomic::Ordering::Release);
        assert!(matches!(
            super::KeyringStore.archive_generation(&synthetic_document("snapshot")),
            Err(crate::error::AppError::CredentialUnavailable(_))
        ));
        assert_eq!(
            super::KeyringStore
                .load("synthetic-account")
                .expect("original"),
            selected
        );
        assert_eq!(
            boundary.live_entries(),
            before,
            "failed normalization must not orphan any chunk"
        );
        boundary
            .fail_main_write
            .store(false, std::sync::atomic::Ordering::Release);
        let archived = super::KeyringStore
            .archive_generation(&synthetic_document("snapshot"))
            .expect("retry");
        assert!(!archived.is_active);
        assert_eq!(
            super::KeyringStore
                .load("synthetic-account")
                .expect("selection"),
            selected
        );
    });
}

#[test]
fn failed_v3_and_v4_archive_commit_preserves_selection_and_releases_new_chunks() {
    for history_size in [1, 20] {
        let boundary = FaultStore::new();
        super::super::native_keyring::with_test_store(boundary.clone(), || {
            let store = super::KeyringStore;
            for index in 0..history_size {
                store
                    .save_generation(&synthetic_document(&format!("stored-{index}")))
                    .expect("history");
            }
            let selected = store.load("synthetic-account").expect("selected");
            let main = super::main_entry("synthetic-account").expect("entry");
            let original_index = main.get_password().expect("original index");
            let live_entries = boundary.live_entries();
            boundary
                .fail_main_write
                .store(true, std::sync::atomic::Ordering::Release);
            assert!(store
                .archive_generation(&synthetic_document("outgoing"))
                .is_err());
            assert_eq!(
                store.load("synthetic-account").expect("selection"),
                selected
            );
            assert_eq!(main.get_password().expect("index"), original_index);
            assert_eq!(boundary.live_entries(), live_entries);
        });
    }
}

#[test]
fn real_writer_archives_new_and_existing_snapshots_without_changing_selection() {
    with_memory_coffer(|| {
        let store = super::KeyringStore;
        let old = store
            .save_generation(&synthetic_document("old"))
            .expect("old");
        let selected = store
            .save_generation(&synthetic_document("selected"))
            .expect("selected");
        let reused = store
            .archive_generation(&synthetic_document("old"))
            .expect("archive existing");
        assert_eq!(reused.generation_id, old.generation_id);
        assert!(!reused.is_active);
        let archived = store
            .archive_generation(&synthetic_document("outgoing"))
            .expect("archive new");
        assert!(!archived.is_active);
        assert_eq!(
            store.load("synthetic-account").expect("selected intact"),
            synthetic_document("selected")
        );
        assert_eq!(
            store
                .load_generation("synthetic-account", &archived.generation_id)
                .expect("snapshot"),
            synthetic_document("outgoing")
        );
        let references = store
            .generation_refs("synthetic-account")
            .expect("references");
        assert_eq!(references.iter().filter(|r| r.is_active).count(), 1);
        assert_eq!(
            references
                .iter()
                .find(|r| r.is_active)
                .expect("active")
                .generation_id,
            selected.generation_id
        );
        let same = store
            .archive_generation(&synthetic_document("selected"))
            .expect("idempotent");
        assert!(same.is_active);
        assert_eq!(
            store
                .generation_refs("synthetic-account")
                .expect("no duplicate")
                .len(),
            3
        );
    });
}

#[test]
fn saving_an_unchanged_legacy_document_commits_the_returned_generation_reference() {
    with_memory_coffer(|| {
        let doc = synthetic_document("legacy");
        super::main_entry("synthetic-account")
            .expect("entry")
            .set_password(&doc.to_string())
            .expect("old format");
        let store = super::KeyringStore;
        let generation = store.save_generation(&doc).expect("normalize");
        assert_eq!(
            store
                .load_generation("synthetic-account", &generation.generation_id)
                .expect("returned generation must exist"),
            doc
        );
        assert_eq!(
            store.generation_refs("synthetic-account").expect("refs")[0].generation_id,
            generation.generation_id
        );
    });
}

#[test]
fn archive_preserves_legacy_and_v2_selection_while_committing_readable_references() {
    for legacy in [true, false] {
        with_memory_coffer(|| {
            let doc = synthetic_document("selected-old-format");
            let serialized = doc.to_string();
            let encoded = if legacy {
                serialized
            } else {
                let chunks = super::write_document_chunks("synthetic-account", &serialized)
                    .expect("v2 chunks");
                format!(
                    "{}{}",
                    super::V2_PREFIX,
                    serde_json::to_string(&super::V2Manifest {
                        version: 2,
                        generation: chunks.generation_id,
                        chunks: chunks.chunks,
                        sha256: chunks.sha256,
                    })
                    .expect("v2 manifest")
                )
            };
            super::main_entry("synthetic-account")
                .expect("entry")
                .set_password(&encoded)
                .expect("old coffer");
            let store = super::KeyringStore;
            let archived = store
                .archive_generation(&synthetic_document("outgoing"))
                .expect("archive");
            assert!(!archived.is_active);
            assert_eq!(
                store.load("synthetic-account").expect("selected content"),
                doc
            );
            let refs = store
                .generation_refs("synthetic-account")
                .expect("normalized refs");
            assert_eq!(refs.len(), 2);
            let selected = refs.iter().find(|r| r.is_active).expect("selection");
            assert_eq!(
                store
                    .load_generation("synthetic-account", &selected.generation_id)
                    .expect("selected reference"),
                doc
            );
        });
    }
}

#[test]
fn chunked_v4_index_preserves_its_selection_when_archiving() {
    with_memory_coffer(|| {
        let store = super::KeyringStore;
        for index in 0..20 {
            store
                .save_generation(&synthetic_document(&format!("saved-{index}")))
                .expect("history");
        }
        let entry = super::main_entry("synthetic-account").expect("entry");
        assert!(super::index_pointer(&entry.get_password().expect("main"))
            .expect("v4")
            .is_some());
        let selected = store
            .generation_refs("synthetic-account")
            .expect("refs")
            .into_iter()
            .find(|r| r.is_active)
            .expect("selection");
        let archived = store
            .archive_generation(&synthetic_document("snapshot"))
            .expect("archive");
        assert!(!archived.is_active);
        assert_eq!(
            store.load("synthetic-account").expect("current"),
            synthetic_document("saved-19")
        );
        assert_eq!(
            store
                .load_generation("synthetic-account", &selected.generation_id)
                .expect("selected ref"),
            synthetic_document("saved-19")
        );
        assert_eq!(
            store
                .load_generation("synthetic-account", &archived.generation_id)
                .expect("archive ref"),
            synthetic_document("snapshot")
        );
    });
}

#[test]
fn archive_without_selection_or_with_unknown_format_never_creates_a_replacement() {
    with_memory_coffer(|| {
        let store = super::KeyringStore;
        assert!(store
            .archive_generation(&synthetic_document("snapshot"))
            .is_err());
        assert!(store
            .load_optional("synthetic-account")
            .expect("no entry")
            .is_none());
        let future = "codex-keyring-v5\nopaque-future-format";
        let entry = super::main_entry("synthetic-account").expect("entry");
        entry.set_password(future).expect("future format");
        assert!(store
            .archive_generation(&synthetic_document("snapshot"))
            .is_err());
        assert!(store
            .save_generation(&synthetic_document("snapshot"))
            .is_err());
        assert_eq!(entry.get_password().expect("future untouched"), future);
    });
}

#[test]
fn archive_cannot_silently_replace_a_corrupt_selected_generation() {
    with_memory_coffer(|| {
        let store = super::KeyringStore;
        let doc = synthetic_document("selected");
        let selected = store.save_generation(&doc).expect("selected");
        let entry = super::main_entry("synthetic-account").expect("main");
        let before = entry.get_password().expect("manifest");
        super::chunk_entry("synthetic-account", &selected.generation_id, 0)
            .expect("chunk")
            .delete_credential()
            .expect("missing selected chunk");
        assert!(store.archive_generation(&doc).is_err());
        assert_eq!(entry.get_password().expect("manifest preserved"), before);
    });
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires a task-owned private D-Bus and XDG data/config/runtime directories; synthetic credentials only"]
fn private_native_coffer_preserves_selection_and_normalized_references() {
    let root = std::path::PathBuf::from(
        std::env::var_os("CAM_TEST_PRIVATE_COFFFER").expect("private fixture root"),
    );
    assert!(root
        .to_string_lossy()
        .starts_with("/tmp/cam-switch-ownership."));
    assert!(root.is_dir());
    for (key, child) in [
        ("XDG_DATA_HOME", "data"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_RUNTIME_DIR", "runtime"),
    ] {
        assert_eq!(
            std::path::PathBuf::from(std::env::var_os(key).expect("private XDG scope"))
                .canonicalize()
                .expect("private path"),
            root.join(child).canonicalize().expect("fixture path")
        );
    }
    crate::adapters::native_keyring::check_unlocked().expect("private coffer unlocked");
    let store = super::KeyringStore;
    let selected = synthetic_document("legacy-selected");
    super::main_entry("synthetic-account")
        .expect("private entry")
        .set_password(&selected.to_string())
        .expect("legacy input");
    let primary = store
        .save_generation(&selected)
        .expect("commit normalized reference");
    let archived = store
        .archive_generation(&synthetic_document("old-desktop"))
        .expect("archive");
    assert!(!archived.is_active);
    assert_eq!(
        store.load("synthetic-account").expect("selection"),
        selected
    );
    assert_eq!(
        store
            .load_generation("synthetic-account", &primary.generation_id)
            .expect("normalized reference"),
        selected
    );
    assert_eq!(
        store
            .load_generation("synthetic-account", &archived.generation_id)
            .expect("archive reference"),
        synthetic_document("old-desktop")
    );
    for index in 0..20 {
        store
            .save_generation(&synthetic_document(&format!("native-{index}")))
            .expect("native history");
    }
    let main = super::main_entry("synthetic-account")
        .expect("entry")
        .get_password()
        .expect("index");
    assert!(index_pointer(&main).expect("chunked index").is_some());
    store
        .archive_generation(&synthetic_document("final-outgoing"))
        .expect("v4 archive");
    assert_eq!(
        store.load("synthetic-account").expect("v4 selection"),
        synthetic_document("native-19")
    );
    store
        .delete("synthetic-account")
        .expect("remove only synthetic fixture entries");
    assert!(store
        .load_optional("synthetic-account")
        .expect("deleted")
        .is_none());
}

#[test]
fn chunks_stay_below_windows_credential_limit_and_round_trip() {
    let serialized = format!("{{\"tokens\":\"{}\"}}", "x".repeat(7_000));
    let chunks = split_for_keyring(&serialized).expect("chunks");
    assert!(chunks.len() > 2);
    assert!(chunks
        .iter()
        .all(|chunk| chunk.encode_utf16().count() * 2 <= 2_400));
    assert_eq!(chunks.concat(), serialized);
}

#[test]
fn v3_manifest_requires_a_real_active_generation() {
    let generation = GenerationManifest {
        generation_id: "550e8400-e29b-41d4-a716-446655440000".to_owned(),
        chunks: 1,
        sha256: digest("document"),
        created_at: Utc::now(),
    };
    let valid = V3Manifest {
        version: 3,
        active_generation: generation.generation_id.clone(),
        generations: vec![generation],
    };
    assert!(validate_v3_manifest(&valid).is_ok());
    assert!(validate_v3_manifest(&V3Manifest {
        active_generation: "missing".to_owned(),
        ..valid
    })
    .is_err());
}

#[test]
fn credential_index_is_chunked_too_when_its_history_exceeds_windows_limit() {
    let generations = (0..20)
        .map(|_| GenerationManifest {
            generation_id: Uuid::new_v4().simple().to_string(),
            chunks: 3,
            sha256: digest("document"),
            created_at: Utc::now(),
        })
        .collect::<Vec<_>>();
    let manifest = V3Manifest {
        version: 3,
        active_generation: generations[0].generation_id.clone(),
        generations,
    };
    let serialized = serde_json::to_string(&manifest).expect("serialize");
    assert!(!fits_native_entry(&format!("{V3_PREFIX}{serialized}")));
    let chunks = split_for_keyring(&serialized).expect("index chunks");
    assert!(chunks.iter().all(|chunk| fits_native_entry(chunk)));
    let index = GenerationManifest {
        generation_id: Uuid::new_v4().simple().to_string(),
        chunks: chunks.len(),
        sha256: digest(&serialized),
        created_at: Utc::now(),
    };
    let pointer = encode_index_pointer(&index).expect("pointer");
    assert!(fits_native_entry(&pointer));
    assert_eq!(
        index_pointer(&pointer)
            .expect("parse")
            .expect("index")
            .chunks,
        chunks.len()
    );
    let restored: V3Manifest = serde_json::from_str(&chunks.concat()).expect("restore history");
    assert_eq!(restored.generations.len(), 20);
    assert!(validate_v3_manifest(&restored).is_ok());
}

#[test]
fn a_temporary_coffer_failure_is_not_treated_as_a_corrupt_generation() {
    let unavailable = crate::error::AppError::CredentialUnavailable("fixture failure".to_owned());
    assert!(super::readable_generation(Err(unavailable)).is_err());
    let corrupt = crate::error::AppError::Keyring("checksum".to_owned());
    assert!(!super::readable_generation(Err(corrupt)).expect("replace corrupt entry"));
}
#[test]
fn account_collection_excludes_chunks_indexes_and_application_sessions() {
    for user in [
        "account:",
        "account:a:chunk:g:0",
        "account:a:index:g",
        "account:application-session:uuid",
        "integration:app",
    ] {
        assert!(super::main_account_id(user).is_none());
    }
    assert_eq!(
        super::main_account_id("account:owned-remote-id"),
        Some("owned-remote-id")
    );
}

#[test]
#[ignore = "read-only metadata audit of this user's saved accounts; never prints credentials"]
fn saved_account_metadata_probe() {
    use crate::ports::CredentialStore;
    let store = super::KeyringStore;
    store
        .check_read_access()
        .expect("coffer read access without prompt");
    let path = dirs::data_local_dir()
        .expect("app data")
        .join("com.codex.account-manager/manager.sqlite3");
    let database =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("read-only database");
    let mut query = database.prepare("SELECT name, account_id, email FROM profiles WHERE status='active' AND account_id IS NOT NULL").expect("metadata query");
    let profiles = query
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .expect("profiles");
    for profile in profiles {
        let (name, account_id, email) = profile.expect("metadata row");
        let report = match store.load(&account_id) {
            Ok(document) => {
                let mut report = credential_metadata(&document, &account_id, email.as_deref());
                if name.eq_ignore_ascii_case("Conta b3") {
                    let reader = crate::adapters::usage_reader::AppServerUsageReader::default();
                    let usage = crate::ports::UsageReader::read(
                        &reader,
                        &crate::adapters::auth_store::access_session(&document, None)
                            .expect("access only"),
                        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                    );
                    report["remoteQuotaAvailable"] = serde_json::json!(usage
                        .as_ref()
                        .is_ok_and(|snapshot| snapshot.rate_limits.is_some()));
                    report["remoteErrorCode"] =
                        serde_json::json!(usage.as_ref().err().map(crate::error::AppError::code));
                }
                report
            }
            Err(error) => serde_json::json!({ "errorCode": error.code() }),
        };
        println!(
            "{}",
            serde_json::json!({ "profile": name, "metadata": report })
        );
    }
}

fn credential_metadata(
    document: &serde_json::Value,
    account_id: &str,
    email: Option<&str>,
) -> serde_json::Value {
    let id = token_claims(document["tokens"]["id_token"].as_str());
    let access = token_claims(document["tokens"]["access_token"].as_str());
    serde_json::json!({
        "hasIdToken": document["tokens"]["id_token"].as_str().is_some_and(|value| !value.is_empty()),
        "idTokenParsed": id.is_some(),
        "idAccountMatches": id.as_ref().and_then(|value| value.pointer("/https:~1~1api.openai.com~1auth/chatgpt_account_id")).and_then(serde_json::Value::as_str).map(|value| value == account_id),
        "accessAccountMatches": access.as_ref().and_then(|value| value.pointer("/https:~1~1api.openai.com~1auth/chatgpt_account_id")).and_then(serde_json::Value::as_str).map(|value| value == account_id),
        "emailMatches": id.as_ref().and_then(|value| value.get("email")).and_then(serde_json::Value::as_str).zip(email).map(|(actual, expected)| actual.eq_ignore_ascii_case(expected)),
        "accessExpired": access.as_ref().and_then(|value| value.get("exp")).and_then(serde_json::Value::as_i64).map(|timestamp| timestamp <= Utc::now().timestamp()),
        "lastRefresh": document.get("last_refresh"),
    })
}

fn token_claims(value: Option<&str>) -> Option<serde_json::Value> {
    use base64::Engine;
    let encoded = value?.split('.').nth(1)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .ok()?;
    serde_json::from_slice(&decoded).ok()
}

#[test]
#[ignore = "explicit read-only inventory of Rust manager accounts/coffer; no refresh, network, mutation or secret output"]
fn installed_upgrade_credential_inventory_readonly() {
    use crate::ports::CredentialStore;
    use rusqlite::OpenFlags;
    let store = super::KeyringStore;
    store.check_read_access().expect("unlocked coffer");
    let path = dirs::data_local_dir()
        .expect("data directory")
        .join("com.codex.account-manager/manager.sqlite3");
    let database = rusqlite::Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("read only db");
    let active = crate::adapters::auth_store::read_auth(
        &crate::adapters::auth_store::codex_home().join("auth.json"),
    )
    .ok();
    let mut query = database
        .prepare("SELECT id,credential_state FROM accounts ORDER BY id")
        .expect("query");
    for row in query
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .expect("rows")
    {
        let (id, state) = row.expect("row");
        let report = match store.load(&id) {
            Ok(document) => {
                let references = store.generation_refs(&id).expect("references");
                let access = token_claims(
                    document
                        .pointer("/tokens/access_token")
                        .and_then(serde_json::Value::as_str),
                );
                let active_matches = active.as_ref().is_some_and(|active| {
                    active
                        .pointer("/tokens/account_id")
                        .and_then(serde_json::Value::as_str)
                        == Some(&id)
                });
                let same_refresh = active.as_ref().filter(|_| active_matches).map(|active| {
                    active.pointer("/tokens/refresh_token")
                        == document.pointer("/tokens/refresh_token")
                });
                serde_json::json!({"storedState":state,"cofferReadable":true,"generationCount":references.len(),"accessExpired":access.and_then(|claims|claims.get("exp").and_then(serde_json::Value::as_i64)).map(|exp|exp<=Utc::now().timestamp()),"isDesktopIdentity":active_matches,"cofferMatchesDesktopRefresh":same_refresh})
            }
            Err(error) => {
                serde_json::json!({"storedState":state,"cofferReadable":false,"errorCode":error.code()})
            }
        };
        println!(
            "upgrade_inventory {} {report}",
            super::digest(&id)[..8].to_owned()
        );
    }
}

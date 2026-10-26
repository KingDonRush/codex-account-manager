use super::*;
use chrono::Utc;
use rusqlite::Connection;
const REGISTERED_SESSION: &str = "24ab8196-8d13-4db8-8b2d-97bcae2f040b";
const ORIGINAL_03: &str = "CREATE TABLE profiles(id TEXT PRIMARY KEY,name TEXT NOT NULL,account_id TEXT,email TEXT,mail_provider_id TEXT,mail_url TEXT,data_dir TEXT NOT NULL,status TEXT NOT NULL,last_used_at TEXT,last_error TEXT,created_at TEXT NOT NULL,updated_at TEXT NOT NULL);
    CREATE TABLE accounts(id TEXT PRIMARY KEY,email TEXT,plan_type TEXT,credential_ref TEXT,rate_limits_json TEXT,updated_at TEXT NOT NULL);
    CREATE TABLE operations(id TEXT PRIMARY KEY,profile_id TEXT NOT NULL,reconnect INTEGER NOT NULL,state TEXT NOT NULL,step TEXT NOT NULL,started_at TEXT NOT NULL,finished_at TEXT,error_code TEXT,error_message TEXT);
    INSERT INTO accounts VALUES('remote-old','fixture@yahoo.test','pro','native-keyring','{\"primary\":null,\"secondary\":null,\"rateLimitReachedType\":null,\"resetCredits\":null}','2026-09-23T12:00:00Z');
    INSERT INTO operations VALUES('op-old','profile-old',1,'completed','connected','2026-09-23T12:00:00Z','2026-09-23T12:01:00Z',NULL,NULL);";
fn legacy(dir: &std::path::Path) {
    let connection = Connection::open(dir.join("manager.sqlite3")).expect("old db");
    connection.execute_batch(ORIGINAL_03).expect("old schema");
    connection.execute("INSERT INTO profiles VALUES('profile-old','Original','remote-old','fixture@yahoo.test','yahoo','https://mail.yahoo.test/inbox',?1,'archived','2026-09-23T12:00:00Z',NULL,'2026-09-23T12:00:00Z','2026-09-23T12:00:00Z')",[dir.join("profiles/profile-old").to_string_lossy()]).expect("original profile");
}
#[test]
fn unversioned_rust_03_and_later_additive_schemas_upgrade_without_losing_associations_or_browser_data(
) {
    for format in ["baseline-03", "later-additive-columns"] {
        let dir = tempfile::tempdir().expect("fixture");
        legacy(dir.path());
        if format == "later-additive-columns" {
            Connection::open(dir.path().join("manager.sqlite3")).expect("later db")
                .execute_batch("ALTER TABLE accounts ADD COLUMN credential_state TEXT NOT NULL DEFAULT 'ready';
                    ALTER TABLE accounts ADD COLUMN credential_checked_at TEXT;
                    ALTER TABLE profiles ADD COLUMN last_browser_opened_at TEXT;
                    UPDATE profiles SET last_browser_opened_at='2026-09-24T15:00:00Z';")
                .expect("later columns");
        }
        let folder = dir.path().join("profiles/profile-old");
        std::fs::create_dir_all(&folder).expect("browser");
        std::fs::write(folder.join("cookies-fixture"), format).expect("cookie fixture");
        let first = Database::open(dir.path()).expect("upgrade");
        let profile = first.profile("profile-old").expect("profile");
        assert_eq!(profile.name, "Original");
        assert_eq!(profile.account_id.as_deref(), Some("remote-old"));
        assert_eq!(profile.status, crate::domain::ProfileStatus::Archived);
        if format == "later-additive-columns" {
            assert_eq!(
                profile.last_browser_opened_at.map(|at| at.timestamp()),
                Some(
                    chrono::DateTime::parse_from_rfc3339("2026-09-24T15:00:00Z")
                        .expect("original time")
                        .timestamp()
                )
            );
        }
        assert_eq!(
            profile.mail_url.as_deref(),
            Some("https://mail.yahoo.test/inbox")
        );
        let opaque = first
            .account_integration_options("remote-old")
            .expect("local ID")
            .0;
        let connection = first.connection().expect("connection");
        assert_eq!(
            connection
                .query_row(
                    "SELECT credential_ref FROM accounts WHERE id='remote-old'",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .expect("coffer ref"),
            "native-keyring"
        );
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .expect("version"),
            CURRENT_FORMAT
        );
        drop(connection);
        drop(first);
        let again = Database::open(dir.path()).expect("repeated upgrade");
        assert_eq!(again.list_profiles().expect("profiles").len(), 1);
        assert_eq!(
            again
                .account_integration_options("remote-old")
                .expect("same ID")
                .0,
            opaque
        );
        assert_eq!(
            std::fs::read_to_string(folder.join("cookies-fixture")).expect("untouched cookie"),
            format
        );
        assert_eq!(
            again.operation("op-old").expect("operation").kind,
            crate::domain::OperationKind::Reconnect
        );
    }
}
#[test]
fn metadata_reconciliation_collects_orphans_but_not_archived_deleted_or_intentionally_unlinked_accounts(
) {
    let dir = tempfile::tempdir().expect("fixture");
    legacy(dir.path());
    let db = Database::open(dir.path()).expect("db");
    let connection = db.connection().expect("connection");
    for id in ["orphan", "removed", "cleanup"] {
        connection.execute("INSERT INTO accounts(id,credential_state,updated_at) VALUES(?1,'missing','2026-09-23T12:00:00Z')",[id]).expect("old orphan");
    }
    connection
        .execute(
            "INSERT INTO schema_meta VALUES('removed-account:removed','fixture')",
            [],
        )
        .expect("tombstone");
    connection
        .execute(
            "INSERT INTO schema_meta VALUES('pending-account-cleanup:cleanup','fixture')",
            [],
        )
        .expect("pending cleanup");
    drop(connection);
    drop(db);
    let repaired = Database::open(dir.path()).expect("reconcile");
    assert_eq!(repaired.list_profiles().expect("profiles").len(), 2);
    assert!(repaired
        .profile_for_account("orphan")
        .expect("orphan")
        .is_some());
    assert!(repaired
        .profile_for_account("removed")
        .expect("removed")
        .is_none());
    assert!(repaired
        .profile_for_account("cleanup")
        .expect("cleanup")
        .is_none());
}
#[test]
fn failed_migration_rolls_back_all_added_columns_and_a_future_format_is_never_downgraded() {
    let dir = tempfile::tempdir().expect("fixture");
    legacy(dir.path());
    let connection = Connection::open(dir.path().join("manager.sqlite3")).expect("db");
    connection.execute("INSERT INTO profiles SELECT 'duplicate',name,account_id,email,mail_provider_id,mail_url,data_dir,status,last_used_at,last_error,created_at,updated_at FROM profiles",[]).expect("duplicate fixture");
    drop(connection);
    assert!(Database::open(dir.path()).is_err());
    let connection = Connection::open(dir.path().join("manager.sqlite3")).expect("check");
    let columns = connection
        .prepare("PRAGMA table_info(accounts)")
        .expect("columns")
        .query_map([], |row| row.get::<_, String>(1))
        .expect("rows")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect");
    assert!(!columns.contains(&"credential_state".to_owned()));
    connection
        .execute("DELETE FROM profiles WHERE id='duplicate'", [])
        .expect("fixture repair");
    connection
        .pragma_update(None, "user_version", 99)
        .expect("future fixture");
    drop(connection);
    assert!(Database::open(dir.path()).is_err());
    let connection = Connection::open(dir.path().join("manager.sqlite3")).expect("verify");
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .expect("still future"),
        99
    );
    let preserved: (String, String, String) = connection
        .query_row(
            "SELECT name,account_id,mail_url FROM profiles WHERE id='profile-old'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("future data still present");
    assert_eq!(
        preserved,
        (
            "Original".into(),
            "remote-old".into(),
            "https://mail.yahoo.test/inbox".into()
        )
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT credential_ref FROM accounts WHERE id='remote-old'",
                [],
                |row| row.get::<_, String>(0)
            )
            .expect("credential reference intact"),
        "native-keyring"
    );
}

fn registered_state(database: &Database, profile: &str) -> serde_json::Value {
    serde_json::json!({
        "profile": database.profile(profile).expect("profile"),
        "account": database.account_integration_options("registered-remote").expect("options"),
        "preferences": database.profile_options().expect("preferences"),
        "generations": database.credential_generations("registered-remote").expect("generations")
            .into_iter().map(|generation| serde_json::json!([generation.generation_id, generation.state])).collect::<Vec<_>>(),
        "selectedGeneration": database.active_credential_signature("registered-remote").expect("selected generation"),
        "session": database.application_session_owner(REGISTERED_SESSION).expect("session owner"),
        "groups": database.groups().expect("groups"),
        "applications": database.applications().expect("applications"),
        "bindings": database.bindings().expect("bindings"),
        "grants": database.grants().expect("grants"),
        "operations": database.list_operations().expect("operations"),
    })
}

#[test]
fn reopening_registered_05_state_preserves_relationships_preferences_and_authority() {
    use crate::domain::integrations::*;
    use crate::domain::{AccountMetadata, OperationState, ProfileOptions};
    use std::collections::BTreeSet;
    let directory = tempfile::tempdir().expect("private fixture");
    let database = Database::open(directory.path()).expect("database");
    let profile = database
        .create_profile(
            "Registered",
            Some("yahoo"),
            Some("https://mail.example.test/inbox"),
        )
        .expect("profile");
    database
        .persist_connected_account(
            &profile.id,
            AccountMetadata {
                account_id: "registered-remote",
                email: Some("fixture@example.test"),
                plan_type: Some("pro"),
                credential_ref: Some("native-keyring"),
                rate_limits: None,
                observed_at: Utc::now(),
            },
            Some("yahoo"),
        )
        .expect("account");
    let opaque = database
        .account_integration_options("registered-remote")
        .expect("opaque id")
        .0;
    database
        .set_account_policy(&opaque, AccountPolicy::Shared, 3)
        .expect("policy");
    database
        .set_mail_preference("registered-remote", true)
        .expect("webmail opt in");
    database
        .save_profile_options(
            &profile.id,
            &ProfileOptions {
                rotation_enabled: false,
                preferred: true,
            },
        )
        .expect("preferences");
    database
        .upsert_credential_generation(
            &crate::ports::StoredCredentialGeneration {
                account_id: "registered-remote".into(),
                generation_id: "selected-generation".into(),
                fingerprint: "synthetic-fingerprint".into(),
                created_at: Utc::now(),
                is_active: true,
            },
            true,
        )
        .expect("generation metadata, not tokens");
    database
        .save_application_session(&opaque, REGISTERED_SESSION)
        .expect("session metadata");
    let application = database
        .save_application(&Application {
            id: uuid::Uuid::new_v4().to_string(),
            name: "Registered application".into(),
            slug: "registered-application".into(),
            paused: false,
            revision: 0,
        })
        .expect("application");
    let group = |name: &str| AccountGroup {
        id: uuid::Uuid::new_v4().to_string(),
        name: name.into(),
        account_ids: BTreeSet::from([opaque.clone()]),
        archived: false,
        paused: false,
        revision: 0,
        exclusive_application_id: None,
    };
    let first = database
        .save_group(&group("First group"))
        .expect("first group");
    let second = database
        .save_group(&group("Second group"))
        .expect("second group");
    database
        .save_binding(&ExecutionBinding {
            id: uuid::Uuid::new_v4().to_string(),
            application_id: application.id.clone(),
            name: "Existing model options".into(),
            target: BindingTarget::Group(first.id.clone()),
            automatic: false,
            allow_projected_quota: false,
            allowed_models: BTreeSet::from(["fixture-model".into()]),
            default_model: Some("fixture-model".into()),
            paused: false,
            revision: 0,
        })
        .expect("existing binding");
    database
        .save_grant(&AuthorizationGrant {
            id: uuid::Uuid::new_v4().to_string(),
            application_id: application.id,
            scope: GrantScope {
                account_ids: BTreeSet::from([opaque.clone()]),
                direct_account_ids: Some(BTreeSet::new()),
                group_ids: BTreeSet::from([first.id, second.id]),
                permissions: BTreeSet::from([Permission::ReadMetadata, Permission::Execute]),
                permanent: true,
                ..GrantScope::default()
            },
            verifier: "synthetic-public-verifier".into(),
            peer_identity: "synthetic-peer".into(),
            revoked: false,
            created_at: Utc::now(),
        })
        .expect("grant with no extra permission");
    let mut operation = database
        .create_switch_operation(&profile.id)
        .expect("operation");
    operation.state = OperationState::Completed;
    operation.finished_at = Some(Utc::now());
    database
        .save_operation(&operation)
        .expect("completed operation");
    let expected = registered_state(&database, &profile.id);
    drop(database);

    for _ in 0..2 {
        let reopened = Database::open(directory.path()).expect("repeat upgrade");
        assert_eq!(registered_state(&reopened, &profile.id), expected);
        assert_eq!(
            reopened
                .application_session(&opaque)
                .expect("same session")
                .as_deref(),
            Some(REGISTERED_SESSION)
        );
        assert_eq!(
            reopened
                .active_credential_signature("registered-remote")
                .expect("same generation")
                .as_deref(),
            Some("selected-generation:synthetic-fingerprint")
        );
    }
}

use super::*;
use crate::domain::{AccountMetadata, ProfileStatus};
use std::collections::BTreeSet;

fn connected(database: &Database, name: &str) -> String {
    let profile = database.create_profile(name, None, None).expect("profile");
    database
        .persist_connected_account(
            &profile.id,
            AccountMetadata {
                account_id: name,
                email: Some("private@example.com"),
                plan_type: Some("pro"),
                credential_ref: None,
                rate_limits: None,
                observed_at: chrono::Utc::now(),
            },
            None,
        )
        .expect("metadata");
    database
        .account_integration_options(name)
        .expect("local id")
        .0
}
#[test]
fn groups_are_many_to_many_versioned_and_never_delete_accounts() {
    let directory = tempfile::tempdir().expect("fixture");
    let database = Database::open(directory.path()).expect("db");
    let account = connected(&database, "account");
    let group = |name: &str| AccountGroup {
        id: uuid::Uuid::new_v4().to_string(),
        name: name.to_owned(),
        account_ids: BTreeSet::from([account.clone()]),
        archived: false,
        paused: false,
        revision: 0,
        exclusive_application_id: None,
    };
    let a = database.save_group(&group("A")).expect("A");
    let b = database.save_group(&group("B")).expect("B");
    let mut archived = a.clone();
    archived.archived = true;
    let archived = database.save_group(&archived).expect("archive");
    assert!(database.save_group(&a).is_err());
    database
        .delete_group(&archived.id, archived.revision)
        .expect("delete group");
    assert_eq!(database.groups().expect("groups")[0].id, b.id);
    assert_eq!(
        database.list_profiles().expect("account remains")[0].status,
        ProfileStatus::Active
    );
    let again = Database::open(directory.path()).expect("reopen");
    assert_eq!(
        again.account_integration_options("account").expect("id").0,
        account
    );
}
#[test]
fn public_catalog_contains_ids_and_plans_without_private_fields() {
    let directory = tempfile::tempdir().expect("fixture");
    let core = crate::application::manager::ManagerCore::with_bundled_runtime(
        directory.path().to_path_buf(),
        None,
    )
    .expect("core");
    let database = Database::open(directory.path()).expect("db");
    connected(&database, "private-name");
    let catalog = core.public_integration_catalog().expect("catalog");
    let encoded = serde_json::to_string(&catalog).expect("json");
    assert!(!encoded.contains("private-name") && !encoded.contains("private@example.com"));
    assert_eq!(
        catalog["accounts"][0]
            .as_object()
            .expect("object")
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["id".to_owned(), "plan".to_owned()])
    );
}

#[test]
fn authorization_commit_rolls_back_application_and_exclusivity_on_stale_group() {
    use crate::domain::integrations::GrantScope;
    let directory = tempfile::tempdir().expect("fixture");
    let database = Database::open(directory.path()).expect("db");
    let application = Application {
        id: uuid::Uuid::new_v4().to_string(),
        name: "fixture".to_owned(),
        slug: "fixture".to_owned(),
        paused: false,
        revision: 0,
    };
    let first = database
        .save_group(&AccountGroup {
            id: uuid::Uuid::new_v4().to_string(),
            name: "first".to_owned(),
            account_ids: BTreeSet::new(),
            archived: false,
            paused: false,
            revision: 0,
            exclusive_application_id: None,
        })
        .expect("group");
    let mut second = database
        .save_group(&AccountGroup {
            id: uuid::Uuid::new_v4().to_string(),
            name: "second".to_owned(),
            revision: 0,
            ..first.clone()
        })
        .expect("second group");
    let stale = second.clone();
    second.name = "changed".to_owned();
    database.save_group(&second).expect("change revision");
    let grant = AuthorizationGrant {
        id: uuid::Uuid::new_v4().to_string(),
        application_id: application.id.clone(),
        scope: GrantScope {
            group_ids: BTreeSet::from([first.id.clone(), second.id.clone()]),
            background_group_ids: BTreeSet::from([first.id.clone(), second.id.clone()]),
            permanent: true,
            ..GrantScope::default()
        },
        verifier: "public-fixture".to_owned(),
        peer_identity: "fixture".to_owned(),
        revoked: false,
        created_at: chrono::Utc::now(),
    };
    assert!(database
        .commit_authorization(&grant, &application, &[first, stale])
        .is_err());
    assert!(database.applications().expect("apps").is_empty());
    assert!(database.grants().expect("grants").is_empty());
    assert!(database
        .groups()
        .expect("groups")
        .iter()
        .all(|group| group.exclusive_application_id.is_none()));
}

#[test]
fn duplicate_application_slug_is_rejected_without_overwriting_the_original() {
    let dir = tempfile::tempdir().expect("fixture");
    let database = Database::open(dir.path()).expect("db");
    let first = Application {
        id: uuid::Uuid::new_v4().to_string(),
        name: "first".to_owned(),
        slug: "same".to_owned(),
        paused: false,
        revision: 0,
    };
    database.save_application(&first).expect("original");
    assert!(database
        .save_application(&Application {
            id: uuid::Uuid::new_v4().to_string(),
            name: "second".to_owned(),
            ..first
        })
        .is_err());
    assert_eq!(database.applications().expect("apps").len(), 1);
}

fn consent(application: &Application, account_ids: &[String]) -> AuthorizationGrant {
    AuthorizationGrant {
        id: uuid::Uuid::new_v4().to_string(),
        application_id: application.id.clone(),
        verifier: "fixture-public".into(),
        peer_identity: "fixture-peer".into(),
        revoked: false,
        created_at: chrono::Utc::now(),
        scope: crate::domain::integrations::GrantScope {
            account_ids: account_ids.iter().cloned().collect(),
            permissions: BTreeSet::from([crate::domain::integrations::Permission::Execute]),
            ..Default::default()
        },
    }
}
fn new_application() -> Application {
    Application {
        id: uuid::Uuid::new_v4().to_string(),
        name: "Fixture".into(),
        slug: "fixture".into(),
        paused: false,
        revision: 0,
    }
}

#[test]
fn consent_enables_selected_desktop_accounts_atomically_without_binding_or_credentials() {
    use crate::domain::integrations::AuthorizationSetup;
    let dir = tempfile::tempdir().expect("fixture");
    let database = Database::open(dir.path()).expect("database");
    let selected = connected(&database, "selected");
    let unselected = connected(&database, "unselected");
    let reserved = connected(&database, "reserved");
    database
        .set_account_policy(&selected, AccountPolicy::DesktopOnly, 4)
        .expect("capacity");
    database
        .set_account_policy(&reserved, AccountPolicy::ApplicationsOnly, 2)
        .expect("reservation");
    let app = new_application();
    let grant = consent(&app, &[selected.clone(), reserved.clone()]);
    let setup = AuthorizationSetup {
        shared_account_ids: BTreeSet::from([selected.clone(), reserved]),
    };
    database
        .commit_authorization_setup(&grant, &app, &[], &setup)
        .expect("approval");
    assert_eq!(
        database
            .account_integration_options("selected")
            .expect("selected")
            .1,
        AccountPolicy::Shared
    );
    assert_eq!(
        database
            .account_integration_options("selected")
            .expect("capacity")
            .2,
        4
    );
    assert_eq!(
        database
            .account_integration_options("unselected")
            .expect("unselected")
            .1,
        AccountPolicy::DesktopOnly
    );
    assert_eq!(
        database
            .account_integration_options("reserved")
            .expect("reserved")
            .1,
        AccountPolicy::ApplicationsOnly
    );
    assert_eq!(database.grants().expect("grant").len(), 1);
    assert!(database.bindings().expect("no bindings").is_empty());
    assert!(database
        .application_session(&selected)
        .expect("no credential copy")
        .is_none());
    assert!(database
        .application_session(&unselected)
        .expect("unselected")
        .is_none());
}

#[test]
fn a_read_only_or_out_of_scope_consent_cannot_enable_application_use() {
    use crate::domain::integrations::AuthorizationSetup;
    let dir = tempfile::tempdir().expect("fixture");
    let database = Database::open(dir.path()).expect("database");
    let selected = connected(&database, "selected");
    let other = connected(&database, "other");
    let app = new_application();
    let mut grant = consent(&app, std::slice::from_ref(&selected));
    let setup = AuthorizationSetup {
        shared_account_ids: BTreeSet::from([other]),
    };
    assert!(database
        .commit_authorization_setup(&grant, &app, &[], &setup)
        .is_err());
    grant.scope.permissions =
        BTreeSet::from([crate::domain::integrations::Permission::ReadMetadata]);
    let setup = AuthorizationSetup {
        shared_account_ids: BTreeSet::from([selected]),
    };
    assert!(database
        .commit_authorization_setup(&grant, &app, &[], &setup)
        .is_err());
    assert!(database.applications().expect("no app").is_empty());
    assert!(database.grants().expect("no grant").is_empty());
    assert_eq!(
        database
            .account_integration_options("selected")
            .expect("policy")
            .1,
        AccountPolicy::DesktopOnly
    );
}

#[test]
fn a_failed_grant_insert_rolls_back_account_availability_as_well_as_the_application() {
    use crate::domain::integrations::AuthorizationSetup;
    let dir = tempfile::tempdir().expect("fixture");
    let database = Database::open(dir.path()).expect("database");
    let selected = connected(&database, "selected");
    let app = new_application();
    let grant = consent(&app, std::slice::from_ref(&selected));
    database
        .save_grant(&grant)
        .expect("duplicate grant fixture");
    let setup = AuthorizationSetup {
        shared_account_ids: BTreeSet::from([selected]),
    };
    assert!(database
        .commit_authorization_setup(&grant, &app, &[], &setup)
        .is_err());
    assert!(database.applications().expect("rollback app").is_empty());
    assert_eq!(
        database
            .account_integration_options("selected")
            .expect("rollback policy")
            .1,
        AccountPolicy::DesktopOnly
    );
}

#[test]
fn application_changed_during_consent_cannot_gain_a_new_grant_or_enable_accounts() {
    use crate::domain::integrations::AuthorizationSetup;
    let dir = tempfile::tempdir().expect("fixture");
    let database = Database::open(dir.path()).expect("database");
    let id = connected(&database, "selected");
    let app = database.save_application(&new_application()).expect("app");
    let grant = consent(&app, std::slice::from_ref(&id));
    let mut changed = app.clone();
    changed.paused = true;
    database.save_application(&changed).expect("pause");
    let setup = AuthorizationSetup {
        shared_account_ids: BTreeSet::from([id]),
    };
    assert!(database
        .commit_authorization_setup(&grant, &app, &[], &setup)
        .is_err());
    assert!(database.grants().expect("no grant").is_empty());
    assert_eq!(
        database
            .account_integration_options("selected")
            .expect("unchanged policy")
            .1,
        AccountPolicy::DesktopOnly
    );
}

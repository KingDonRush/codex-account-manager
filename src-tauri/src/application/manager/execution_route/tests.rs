use super::*;
use crate::adapters::database::Database;
use crate::domain::integrations::{
    AccountGroup, AccountPolicy, Application, GrantScope, Permission,
};
use crate::domain::AccountMetadata;
use crate::ports::CredentialStore;
use chrono::Utc;
use serde_json::Value;
use std::sync::Arc;

struct NoCredentials;
impl CredentialStore for NoCredentials {
    fn save(&self, _: &Value) -> Result<String, AppError> {
        unreachable!("metadata-only test")
    }
    fn load(&self, _: &str) -> Result<Value, AppError> {
        unreachable!("metadata-only test")
    }
    fn load_optional(&self, _: &str) -> Result<Option<Value>, AppError> {
        Ok(None)
    }
    fn delete(&self, _: &str) -> Result<(), AppError> {
        unreachable!("metadata-only test")
    }
}
fn fixture() -> (tempfile::TempDir, ManagerCore, AuthorizationGrant) {
    let dir = tempfile::tempdir().expect("fixture");
    let database = Database::open(dir.path()).expect("database");
    let app = database
        .save_application(&Application {
            id: uuid::Uuid::new_v4().to_string(),
            name: "Fixture".into(),
            slug: "fixture".into(),
            paused: false,
            revision: 0,
        })
        .expect("application");
    let core = ManagerCore::from_components(
        dir.path().to_path_buf(),
        None,
        database,
        Arc::new(NoCredentials),
    )
    .expect("core");
    let grant = AuthorizationGrant {
        id: "fixture-grant".into(),
        application_id: app.id,
        verifier: "fixture-public".into(),
        peer_identity: "fixture-peer".into(),
        revoked: false,
        created_at: Utc::now(),
        scope: GrantScope {
            permissions: BTreeSet::from([Permission::Execute]),
            direct_account_ids: Some(BTreeSet::new()),
            ..Default::default()
        },
    };
    (dir, core, grant)
}
fn account(core: &ManagerCore, remote: &str, connected: bool) -> String {
    let profile = core
        .database
        .create_profile(remote, None, None)
        .expect("profile");
    core.database
        .persist_connected_account(
            &profile.id,
            AccountMetadata {
                account_id: remote,
                email: None,
                plan_type: Some("plus"),
                credential_ref: None,
                rate_limits: None,
                observed_at: Utc::now(),
            },
            None,
        )
        .expect("account");
    let id = core
        .database
        .account_integration_options(remote)
        .expect("id")
        .0;
    core.database
        .set_account_policy(&id, AccountPolicy::Shared, 1)
        .expect("policy");
    if connected {
        core.database
            .save_application_session(&id, &uuid::Uuid::new_v4().to_string())
            .expect("metadata");
    }
    id
}
fn request() -> ExecRequest {
    ExecRequest {
        binding_id: String::new(),
        binding_snapshot: None,
        account_id: None,
        group_id: None,
        cwd: "/fixture".into(),
        prompt: "fixture".into(),
        arguments: Vec::new(),
    }
}
fn group(core: &ManagerCore, ids: &[String]) -> AccountGroup {
    core.database
        .save_group(&AccountGroup {
            id: uuid::Uuid::new_v4().to_string(),
            name: "Fixture group".into(),
            account_ids: ids.iter().cloned().collect(),
            archived: false,
            paused: false,
            exclusive_application_id: None,
            revision: 0,
        })
        .expect("group")
}
fn approve_accounts(grant: &mut AuthorizationGrant, ids: &[String]) {
    grant.scope.account_ids = ids.iter().cloned().collect();
    grant.scope.direct_account_ids = Some(grant.scope.account_ids.clone());
}

#[test]
fn direct_execution_needs_only_approved_resources_and_creates_no_binding() {
    let (_dir, core, mut grant) = fixture();
    let id = account(&core, "selected", true);
    let other = account(&core, "unselected", true);
    approve_accounts(&mut grant, std::slice::from_ref(&id));
    let candidates = core
        .execution_candidates(&grant, &request())
        .expect("direct candidates");
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].account, id);
    assert!(core.database.bindings().expect("bindings").is_empty());
    let mut exact = request();
    exact.account_id = Some(other);
    assert!(matches!(
        core.execution_candidates(&grant, &exact),
        Err(AppError::AccessDenied(_))
    ));
    grant.revoked = true;
    assert!(matches!(
        core.execution_candidates(&grant, &request()),
        Err(AppError::AccessDenied(_))
    ));
}

#[test]
fn no_dedicated_login_is_required_but_a_missing_account_credential_is_not_ready() {
    let (_dir, core, mut grant) = fixture();
    let missing = account(&core, "missing", false);
    let ready = account(&core, "connected", true);
    approve_accounts(&mut grant, &[missing.clone(), ready.clone()]);
    let candidates = core
        .execution_candidates(&grant, &request())
        .expect("connected account");
    assert_eq!(
        candidates.len(),
        2,
        "both saved accounts may execute; a dedicated app session is optional"
    );
    assert!(candidates
        .iter()
        .any(|candidate| candidate.account == ready));
    core.database
        .set_credential_state("missing", crate::domain::CredentialState::Missing)
        .unwrap();
    let remaining = core.execution_candidates(&grant, &request()).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].account, ready);
    let mut exact = request();
    exact.account_id = Some(missing);
    assert!(matches!(
        core.execution_candidates(&grant, &exact),
        Err(AppError::AuthenticationRequired(_))
    ));
}

#[test]
fn direct_group_respects_the_approved_ceiling_and_current_membership() {
    let (_dir, core, mut grant) = fixture();
    let approved = account(&core, "approved", true);
    let future = account(&core, "future", true);
    let mut selected = group(&core, std::slice::from_ref(&approved));
    grant.scope.account_ids.insert(approved.clone());
    grant.scope.group_ids.insert(selected.id.clone());
    selected.account_ids.insert(future);
    selected = core
        .database
        .save_group(&selected)
        .expect("add future member");
    let mut exact = request();
    exact.group_id = Some(selected.id.clone());
    let candidates = core.execution_candidates(&grant, &exact).expect("group");
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].account, approved);
    assert_eq!(candidates[0].group.as_deref(), Some(selected.id.as_str()));
    selected.account_ids.clear();
    core.database.save_group(&selected).expect("remove members");
    assert!(core.execution_candidates(&grant, &exact).is_err());
}

#[test]
fn pausing_one_group_does_not_block_a_directly_authorized_account() {
    let (_dir, core, mut grant) = fixture();
    let id = account(&core, "selected", true);
    let mut selected = group(&core, std::slice::from_ref(&id));
    approve_accounts(&mut grant, &[id]);
    grant.scope.group_ids.insert(selected.id.clone());
    selected.paused = true;
    core.database.save_group(&selected).expect("pause");
    assert_eq!(
        core.execution_candidates(&grant, &request())
            .expect("direct resource")
            .len(),
        1
    );
    let mut exact = request();
    exact.group_id = Some(selected.id);
    assert!(matches!(
        core.execution_candidates(&grant, &exact),
        Err(AppError::Busy(_))
    ));
}

#[test]
fn group_only_access_cannot_be_bypassed_by_an_exact_account_request() {
    let (_dir, core, mut grant) = fixture();
    let id = account(&core, "group-only", true);
    let mut selected = group(&core, std::slice::from_ref(&id));
    grant.scope.account_ids.insert(id.clone());
    grant.scope.group_ids.insert(selected.id.clone());
    selected.paused = true;
    core.database.save_group(&selected).expect("pause");
    let mut exact = request();
    exact.account_id = Some(id);
    assert!(core.execution_candidates(&grant, &exact).is_err());
}

#[test]
fn old_grants_keep_direct_scope_and_old_clients_keep_binding_options() {
    let (_dir, core, mut grant) = fixture();
    let id = account(&core, "legacy", true);
    approve_accounts(&mut grant, std::slice::from_ref(&id));
    grant.scope.direct_account_ids = None;
    let binding = core
        .database
        .save_binding(&ExecutionBinding {
            id: uuid::Uuid::new_v4().to_string(),
            application_id: grant.application_id.clone(),
            name: "Legacy".into(),
            target: BindingTarget::Account(id),
            automatic: false,
            allow_projected_quota: true,
            allowed_models: BTreeSet::from(["fixture-model".into()]),
            default_model: Some("fixture-model".into()),
            paused: false,
            revision: 0,
        })
        .expect("legacy configuration");
    let mut legacy = request();
    legacy.binding_id = binding.id;
    let route = core.execution_route(&grant, &legacy).expect("legacy route");
    assert_eq!(route.default_model.as_deref(), Some("fixture-model"));
    assert!(route.allow_projected_quota);
    assert_eq!(
        core.execution_candidates(&grant, &legacy)
            .expect("legacy access")
            .len(),
        1
    );
    assert_eq!(
        core.execution_candidates(&grant, &request())
            .expect("direct old grant")
            .len(),
        1
    );
}

#[test]
fn internal_route_cannot_be_deserialized_or_saved_as_a_client_binding() {
    assert!(
        serde_json::from_value::<BindingTarget>(serde_json::json!({"kind":"authorized"})).is_err()
    );
    assert!(serde_json::to_string(&BindingTarget::Authorized).is_err());
}

#[test]
fn ambiguous_old_group_consent_is_not_promoted_to_independent_account_access() {
    let (_dir, core, mut grant) = fixture();
    let id = account(&core, "legacy-group", true);
    let selected = group(&core, std::slice::from_ref(&id));
    grant.scope.account_ids.insert(id);
    grant.scope.group_ids.insert(selected.id.clone());
    grant.scope.direct_account_ids = None;
    assert!(matches!(
        core.execution_candidates(&grant, &request()),
        Err(AppError::ConsentRequired(_))
    ));
    let mut exact = request();
    exact.group_id = Some(selected.id);
    assert_eq!(
        core.execution_candidates(&grant, &exact)
            .expect("exact old group")
            .len(),
        1
    );
}

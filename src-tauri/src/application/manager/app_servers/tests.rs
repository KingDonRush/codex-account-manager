use super::*;
use crate::{
    adapters::database::Database,
    domain::integrations::{AccountPolicy, Application, ExecutionBinding, GrantScope},
    domain::AccountMetadata,
    ports::{ApplicationSessionStore, CredentialStore},
};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    sync::Arc,
    time::{Duration, Instant},
};

struct FakeCredentials;
impl CredentialStore for FakeCredentials {
    fn save(&self, _: &Value) -> Result<String, AppError> {
        unreachable!("no Desktop credentials in this probe")
    }
    fn load(&self, _: &str) -> Result<Value, AppError> {
        unreachable!("no Desktop credentials in this probe")
    }
    fn load_optional(&self, _: &str) -> Result<Option<Value>, AppError> {
        Ok(None)
    }
    fn delete(&self, _: &str) -> Result<(), AppError> {
        unreachable!("no Desktop deletion in this probe")
    }
}
struct SyntheticSession;
struct SyntheticAccount;
impl CredentialStore for SyntheticAccount {
    fn save(&self, _: &Value) -> Result<String, AppError> {
        panic!("read-only native harness cannot rotate")
    }
    fn load(&self, expected: &str) -> Result<Value, AppError> {
        SyntheticSession.load("", expected)
    }
    fn load_optional(&self, expected: &str) -> Result<Option<Value>, AppError> {
        self.load(expected).map(Some)
    }
    fn delete(&self, _: &str) -> Result<(), AppError> {
        panic!("no deletion")
    }
}
struct OwnedCore(ManagerCore);
impl Drop for OwnedCore {
    fn drop(&mut self) {
        let _ = self.0.shutdown();
    }
}
impl ApplicationSessionStore for SyntheticSession {
    fn load(&self, _: &str, _: &str) -> Result<Value, AppError> {
        Ok(
            json!({"auth_mode":"chatgpt","tokens":{"account_id":"fixture","refresh_token":"never-use-or-send","access_token":"e30.eyJzdWIiOiJmaXh0dXJlIiwiZW1haWwiOiJmaXh0dXJlQGV4YW1wbGUuY29tIiwiaHR0cHM6Ly9hcGkub3BlbmFpLmNvbS9hdXRoIjp7ImNoYXRncHRfYWNjb3VudF9pZCI6ImZpeHR1cmUiLCJjaGF0Z3B0X3BsYW5fdHlwZSI6InBsdXMifX0.sig"}}),
        )
    }
    fn save(&self, _: &str, _: &Value) -> Result<(), AppError> {
        unreachable!("model-free probe never rotates credentials")
    }
    fn delete(&self, _: &str) -> Result<(), AppError> {
        unreachable!("no session deletion")
    }
}
fn fixture_with_source(
    dedicated: bool,
) -> (tempfile::TempDir, ManagerCore, AuthorizationGrant, String) {
    let dir = tempfile::tempdir().expect("fixture");
    let database = Database::open(dir.path()).expect("db");
    let profile = database
        .create_profile("Synthetic", None, None)
        .expect("profile");
    database
        .persist_connected_account(
            &profile.id,
            AccountMetadata {
                account_id: "fixture",
                email: None,
                plan_type: Some("plus"),
                credential_ref: None,
                rate_limits: None,
                observed_at: Utc::now(),
            },
            None,
        )
        .expect("account");
    let local = database
        .account_integration_options("fixture")
        .expect("local")
        .0;
    database
        .set_account_policy(&local, AccountPolicy::Shared, 1)
        .expect("policy");
    if dedicated {
        database
            .save_application_session(&local, &uuid::Uuid::new_v4().to_string())
            .expect("session metadata");
    }
    let application = database
        .save_application(&Application {
            id: uuid::Uuid::new_v4().to_string(),
            name: "Synthetic harness".to_owned(),
            slug: "synthetic-harness".to_owned(),
            paused: false,
            revision: 0,
        })
        .expect("application");
    let binding = database
        .save_binding(&ExecutionBinding {
            id: uuid::Uuid::new_v4().to_string(),
            application_id: application.id.clone(),
            name: "Fixture".to_owned(),
            target: BindingTarget::Account(local.clone()),
            automatic: false,
            allow_projected_quota: false,
            allowed_models: BTreeSet::new(),
            default_model: None,
            paused: false,
            revision: 0,
        })
        .expect("binding");
    let grant = AuthorizationGrant {
        id: uuid::Uuid::new_v4().to_string(),
        application_id: application.id,
        scope: GrantScope {
            account_ids: BTreeSet::from([local]),
            permissions: BTreeSet::from([Permission::Execute]),
            ..GrantScope::default()
        },
        verifier: "synthetic-public".to_owned(),
        peer_identity: "fixture".to_owned(),
        revoked: false,
        created_at: Utc::now(),
    };
    database.save_grant(&grant).expect("grant");
    let mut core = ManagerCore::from_components(
        dir.path().to_path_buf(),
        None,
        database,
        if dedicated {
            Arc::new(FakeCredentials)
        } else {
            Arc::new(SyntheticAccount)
        },
    )
    .expect("core");
    core.application_credentials = Arc::new(SyntheticSession);
    (dir, core, grant, binding.id)
}
fn wait_for(core: &ManagerCore, id: &str, desired: ServerState) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let status = core.application_servers.status(id).expect("status");
        if status.state == desired {
            return;
        }
        assert!(
            !status.state.terminal(),
            "Unexpected terminal state: {:?} {:?}",
            status.state,
            status.error_code
        );
        assert!(
            Instant::now() < deadline,
            "Session did not reach {:?}",
            desired
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
#[test]
#[ignore = "requires installed Codex; synthetic auth, private home, model/list only, no inference or real coffer"]
fn installed_harness_routes_rpc_and_closes_only_its_private_server() {
    native_harness_contract(true);
}
#[test]
#[ignore = "requires native Codex; account-backed synthetic access, model/list only, no inference or real coffer"]
fn installed_harness_uses_connected_account_without_dedicated_login() {
    native_harness_contract(false);
}
fn native_harness_contract(dedicated: bool) {
    let (_dir, core, grant, _legacy_binding_id) = fixture_with_source(dedicated);
    let _cleanup = OwnedCore(core.clone());
    let result = (|| -> Result<(), AppError> {
        let session = core.open_application_server(
            &grant,
            OpenServerRequest {
                group_id: None,
                binding_id: String::new(),
                account_id: None,
                cwd: core.app_data_dir.to_string_lossy().into_owned(),
                persist_history: false,
            },
        )?;
        wait_for(&core, &session.id, ServerState::Ready);
        assert!(core
            .send_application_server(
                &grant,
                &session.id,
                json!({"id":99,"method":"account/read","params":{}})
            )
            .is_err());
        core.send_application_server(
            &grant,
            &session.id,
            json!({"id":1,"method":"model/list","params":{}}),
        )?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let page = core.application_server_events(&grant, &session.id, 0)?;
            let found = page["chunk"]
                .as_str()
                .unwrap_or("")
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .any(|message| message["id"] == 1 && message.get("result").is_some());
            if found {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "No correlated model/list response"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(core.list_executions().is_empty());
        core.close_application_server(&session.id)?;
        wait_for(&core, &session.id, ServerState::Closed);
        assert!(!core
            .app_data_dir
            .join("codex-home-fixture/auth.json")
            .exists());
        Ok(())
    })();
    core.cancel_all();
    result.expect("model-free native harness");
}

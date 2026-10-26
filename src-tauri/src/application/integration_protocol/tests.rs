use super::*;
use crate::domain::integrations::{GrantScope, Permission};
use ed25519_dalek::{Signer, SigningKey};

fn fixture() -> (tempfile::TempDir, ManagerCore, PeerIdentity) {
    let directory = tempfile::tempdir().expect("fixture");
    let core =
        ManagerCore::with_bundled_runtime(directory.path().to_path_buf(), None).expect("core");
    let peer = PeerIdentity {
        pid: 123,
        executable: "fixture-client".to_owned(),
        identity: "fixture-user:client".to_owned(),
    };
    (directory, core, peer)
}
fn request(id: u64, method: &str, params: Value) -> Request {
    Request {
        id,
        method: method.to_owned(),
        params,
    }
}

#[test]
fn pending_or_denied_client_has_only_public_metadata() {
    let (_directory, core, peer) = fixture();
    let mut client = Connection::new(peer);
    let public = client
        .dispatch(&core, request(1, "catalog/read", json!({})))
        .expect("catalog");
    assert_eq!(public["accountCount"], 0);
    assert!(client
        .dispatch(&core, request(2, "accounts/list", json!({})))
        .is_err());
    let authorization=client.dispatch(&core,request(3,"authorization/request",json!({"applicationId":null,"name":"Test","slug":"test","requestedScope":GrantScope::default()}))).expect("request");
    assert!(core
        .integration_snapshot()
        .expect("snapshot")
        .applications
        .is_empty());
    core.deny_authorization(authorization["id"].as_str().expect("id"));
    assert!(client
        .dispatch(
            &core,
            request(
                4,
                "authorization/claim",
                json!({"requestId":authorization["id"]})
            )
        )
        .is_err());
    assert!(client
        .dispatch(&core, request(5, "groups/save", json!({})))
        .is_err());
}

#[test]
fn permanent_consent_reuses_identity_but_not_a_signature_or_challenge() {
    let (_directory, core, peer) = fixture();
    let mut first = Connection::new(peer.clone());
    let mut scope = GrantScope::default();
    scope.permissions.insert(Permission::ReadMetadata);
    scope.permanent = true;
    let authorization = first
        .dispatch(
            &core,
            request(
                1,
                "authorization/request",
                json!({"applicationId":null,"name":"Test","slug":"test","requestedScope":scope}),
            ),
        )
        .expect("request");
    let pending_id = authorization["id"].as_str().expect("id");
    assert!(first
        .dispatch(
            &core,
            request(2, "authorization/claim", json!({"requestId":pending_id}))
        )
        .expect("pending")["pending"]
        .as_bool()
        .expect("bool"));
    let grant = core
        .approve_authorization(pending_id, scope)
        .expect("approve");
    let secret = first
        .dispatch(
            &core,
            request(3, "authorization/claim", json!({"requestId":pending_id})),
        )
        .expect("claim");
    assert!(first
        .dispatch(
            &core,
            request(4, "authorization/claim", json!({"requestId":pending_id}))
        )
        .is_err());
    let bytes: [u8; 32] = hex::decode(secret["signingKey"].as_str().expect("key"))
        .expect("hex")
        .try_into()
        .expect("size");
    let key = SigningKey::from_bytes(&bytes);
    let challenge = first
        .dispatch(
            &core,
            request(5, "authorization/challenge", json!({"grantId":grant})),
        )
        .expect("challenge");
    let proof = hex::encode(
        key.sign(challenge["challenge"].as_str().expect("text").as_bytes())
            .to_bytes(),
    );
    first
        .dispatch(
            &core,
            request(6, "authorization/prove", json!({"proof":proof})),
        )
        .expect("prove");
    assert!(first
        .dispatch(
            &core,
            request(7, "authorization/prove", json!({"proof":proof}))
        )
        .is_err());
    first.close(&core);
    let mut second = Connection::new(peer);
    let _challenge = second
        .dispatch(
            &core,
            request(8, "authorization/challenge", json!({"grantId":grant})),
        )
        .expect("fresh");
    assert!(second
        .dispatch(
            &core,
            request(9, "authorization/prove", json!({"proof":proof}))
        )
        .is_err());
    let challenge = second
        .dispatch(
            &core,
            request(10, "authorization/challenge", json!({"grantId":grant})),
        )
        .expect("new challenge");
    let proof = hex::encode(
        key.sign(challenge["challenge"].as_str().expect("text").as_bytes())
            .to_bytes(),
    );
    second
        .dispatch(
            &core,
            request(11, "authorization/prove", json!({"proof":proof})),
        )
        .expect("reconnected");
    second
        .dispatch(&core, request(12, "accounts/list", json!({})))
        .expect("allowed");
    assert!(second
        .dispatch(&core, request(13, "groups/save", json!({})))
        .is_err());
    core.revoke_authorization(&grant).expect("revoke");
    assert!(second
        .dispatch(&core, request(14, "accounts/list", json!({})))
        .is_err());
}

#[test]
fn cross_channel_claim_and_unverified_peer_cannot_authenticate() {
    let (_directory, core, peer) = fixture();
    let mut first = Connection::new(peer.clone());
    let pending=first.dispatch(&core,request(1,"authorization/request",json!({"applicationId":null,"name":"Test","slug":"test","requestedScope":GrantScope::default()}))).expect("request");
    let mut other = Connection::new(peer);
    assert!(other
        .dispatch(
            &core,
            request(2, "authorization/claim", json!({"requestId":pending["id"]}))
        )
        .is_err());
    assert!(other
        .dispatch(
            &core,
            request(3, "authorization/prove", json!({"proof":"deadbeef"}))
        )
        .is_err());
}

#[test]
fn closing_a_channel_revokes_all_its_temporary_grants_not_only_the_last() {
    let (dir, core, peer) = fixture();
    let mut channel = Connection::new(peer);
    let database = crate::adapters::database::Database::open(dir.path()).expect("fixture database");
    let mut ids = Vec::new();
    for permanent in [false, false, true] {
        let grant = AuthorizationGrant {
            id: uuid::Uuid::new_v4().to_string(),
            application_id: "fixture".to_owned(),
            scope: GrantScope {
                permanent,
                ..GrantScope::default()
            },
            verifier: "public-fixture".to_owned(),
            peer_identity: "fixture".to_owned(),
            revoked: false,
            created_at: chrono::Utc::now(),
        };
        database.save_grant(&grant).expect("fixture grant");
        channel.associated_grants.insert(grant.id.clone());
        channel.grant_id = Some(grant.id.clone());
        ids.push(grant.id);
    }
    channel.close(&core);
    assert!(core.authorization_grant(&ids[0]).expect("first").revoked);
    assert!(core.authorization_grant(&ids[1]).expect("second").revoked);
    assert!(
        !core
            .authorization_grant(&ids[2])
            .expect("permanent")
            .revoked
    );
}

#[test]
fn malformed_unknown_rpc_fields_and_oversized_frames_are_rejected() {
    assert!(serde_json::from_value::<Request>(
        json!({"id":1,"method":"accounts/list","params":{},"approve":true})
    )
    .is_err());
    let bytes = (crate::application::local_transport::MAX_FRAME as u32 + 1)
        .to_be_bytes()
        .to_vec();
    assert!(crate::application::local_transport::read_frame::<Value>(
        &mut std::io::Cursor::new(bytes),
        Duration::from_secs(1)
    )
    .is_err());
}

#[test]
fn execution_and_server_requests_do_not_require_a_separate_binding() {
    let exec = serde_json::from_value::<crate::domain::execution::ExecRequest>(json!({
        "cwd":"/fixture", "prompt":"fixture", "accountId":"approved-account"
    }));
    assert!(
        exec.is_ok(),
        "the authorized account must be enough to request execution"
    );
    let server =
        serde_json::from_value::<crate::domain::application_server::OpenServerRequest>(json!({
            "cwd":"/fixture", "accountId":"approved-account"
        }));
    assert!(
        server.is_ok(),
        "the authorized account must be enough to open a harness"
    );
}

use super::*;
use crate::application::local_transport::{read_frame, write_frame};
use serde_json::{json, Value};

fn private_name() -> String {
    format!(r"\\.\pipe\cam-test-{}", uuid::Uuid::new_v4())
}

#[test]
fn exclusive_pipe_roundtrip_has_os_identity_deadline_and_releases_its_name() {
    let name = private_name();
    let mut listener = PipeListener::bind_at(name.clone()).expect("private listener");
    assert!(
        PipeListener::bind_at(name.clone()).is_err(),
        "do not replace another owner"
    );
    let expected = std::env::current_exe().expect("test executable");
    let mut client = connect_at(&name, &expected).expect("verified server");
    let mut server = listener.accept().expect("accept").expect("connected");
    assert_eq!(client.peer().expect("peer").pid, std::process::id());
    assert_eq!(server.peer().expect("peer").pid, std::process::id());
    write_frame(
        &mut client,
        &json!({"id":3,"method":"catalog/read","params":{}}),
    )
    .expect("write");
    let request: Value = read_frame(&mut server, Duration::from_secs(1)).expect("request");
    assert_eq!(request["id"], 3);
    write_frame(&mut server, &json!({"id":3,"result":{"private":true}})).expect("reply");
    let response: Value = read_frame(&mut client, Duration::from_secs(1)).expect("response");
    assert_eq!(response["result"]["private"], true);
    let started = std::time::Instant::now();
    assert!(matches!(
        read_frame::<Value>(&mut client, Duration::from_millis(60)),
        Err(AppError::TimedOut)
    ));
    assert!(started.elapsed() < Duration::from_secs(1));
    drop(server);
    drop(client);
    drop(listener);
    assert!(
        PipeListener::bind_at(name).is_ok(),
        "name must be released after owners end"
    );
}

#[test]
fn a_pipe_with_an_unexpected_server_executable_is_rejected_before_any_request() {
    let name = private_name();
    let _listener = PipeListener::bind_at(name.clone()).expect("listener");
    let other = std::env::var_os("SystemRoot")
        .map(std::path::PathBuf::from)
        .expect("Windows")
        .join("System32/notepad.exe");
    assert!(matches!(
        connect_at(&name, &other),
        Err(AppError::AccessDenied(_))
    ));
}

#[test]
fn pipe_wait_can_be_cancelled_and_oversized_frames_are_refused() {
    use crate::application::local_transport::read_frame_cancellable;
    use std::sync::atomic::AtomicBool;
    let name = private_name();
    let mut listener = PipeListener::bind_at(name.clone()).expect("listener");
    let mut client = connect_at(&name, &std::env::current_exe().expect("exe")).expect("client");
    let mut server = listener.accept().expect("accept").expect("connected");
    assert!(matches!(
        read_frame_cancellable::<Value>(
            &mut server,
            Duration::from_secs(1),
            &AtomicBool::new(true)
        ),
        Err(AppError::Cancelled)
    ));
    let size = (super::super::MAX_FRAME + 1) as u32;
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    loop {
        match client.write(&size.to_be_bytes()) {
            Ok(4) => break,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            other => panic!("unexpected header write: {other:?}"),
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(matches!(
        read_frame::<Value>(&mut server, Duration::from_secs(1)),
        Err(AppError::Validation(_))
    ));
}

#[test]
fn real_pipe_dispatch_requires_consent_new_proof_and_revokes_temporary_access_on_disconnect() {
    use crate::{
        application::{integration_protocol::Response, manager::ManagerCore},
        domain::integrations::{GrantScope, Permission},
    };
    use ed25519_dalek::{Signer, SigningKey};
    use std::{collections::BTreeSet, time::Instant};
    fn call(client: &mut PipeStream, id: u64, method: &str, params: Value) -> Response {
        write_frame(client, &json!({"id":id,"method":method,"params":params})).expect("request");
        let response: Response = read_frame(client, Duration::from_secs(2)).expect("response");
        assert_eq!(response.id, id);
        response
    }
    let directory = tempfile::tempdir().expect("metadata-only private fixture");
    let core = ManagerCore::with_bundled_runtime(directory.path().into(), None).expect("core");
    let name = private_name();
    let mut listener = PipeListener::bind_at(name.clone()).expect("listener");
    let mut client = connect_at(&name, &std::env::current_exe().expect("exe")).expect("client");
    let server = listener.accept().expect("accept").expect("connected");
    let worker = core.clone();
    let serving = std::thread::spawn(move || {
        crate::application::integration_protocol::serve_connection(worker, server)
    });
    assert_eq!(
        call(&mut client, 1, "accounts/list", json!({}))
            .error
            .expect("no access")
            .code,
        "consent_required"
    );
    let scope = GrantScope {
        permissions: BTreeSet::from([Permission::ReadMetadata]),
        ..GrantScope::default()
    };
    let request = call(&mut client, 2, "authorization/request", json!({
        "applicationId":null,"name":"Native isolated client","slug":"native-isolated-client","requestedScope":scope,
    })).result.expect("pending request");
    assert!(core
        .integration_snapshot()
        .expect("snapshot")
        .applications
        .is_empty());
    let grant_id = core
        .approve_authorization(request["id"].as_str().expect("request id"), scope)
        .expect("fixture human decision");
    let claim = call(
        &mut client,
        3,
        "authorization/claim",
        json!({"requestId":request["id"]}),
    )
    .result
    .expect("claim");
    let bytes: [u8; 32] = hex::decode(claim["signingKey"].as_str().expect("private synthetic key"))
        .expect("decode")
        .try_into()
        .expect("size");
    let key = SigningKey::from_bytes(&bytes);
    let challenge = call(
        &mut client,
        4,
        "authorization/challenge",
        json!({"grantId":grant_id}),
    )
    .result
    .expect("challenge");
    let proof = hex::encode(
        key.sign(challenge["challenge"].as_str().expect("text").as_bytes())
            .to_bytes(),
    );
    assert!(call(
        &mut client,
        5,
        "authorization/prove",
        json!({"proof":proof})
    )
    .error
    .is_none());
    assert_eq!(
        call(&mut client, 6, "accounts/list", json!({}))
            .result
            .expect("authorized read"),
        json!([])
    );
    assert_eq!(
        call(
            &mut client,
            7,
            "authorization/prove",
            json!({"proof":proof})
        )
        .error
        .expect("no replay")
        .code,
        "access_denied"
    );
    drop(client);
    let deadline = Instant::now() + Duration::from_secs(3);
    while !serving.is_finished() {
        assert!(
            Instant::now() < deadline,
            "disconnect must collect the channel"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(serving.join().expect("worker").is_err());
    assert!(core.authorization_grant(&grant_id).expect("grant").revoked);
    core.cancel_all();
}

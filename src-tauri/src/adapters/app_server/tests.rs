use super::*;

#[test]
#[ignore = "requires an installed Codex CLI"]
fn installed_app_server_starts_and_cancels_oauth() {
    let home = IsolatedCodexHome::create().expect("isolated home");
    let mut client = AppServerClient::spawn(home.directory.path()).expect("app server");

    let (login_id, auth_url) = client.start_login().expect("OAuth start");

    assert!(!login_id.is_empty());
    assert!(auth_url.starts_with("https://"));
    client.cancel_login(&login_id).expect("OAuth cancellation");
}

#[test]
#[ignore = "requires an installed Codex CLI; uses synthetic credentials only"]
fn installed_read_only_session_never_writes_auth_or_receives_refresh_token() {
    let home = IsolatedCodexHome::create().expect("isolated home");
    let mut client =
        AppServerClient::spawn_read_only(home.directory.path(), Arc::new(AtomicBool::new(false)))
            .expect("app server");
    let document = json!({ "auth_mode": "chatgpt", "tokens": {
        "account_id": "fixture", "refresh_token": "never-send-this",
        "access_token": "e30.eyJzdWIiOiJmaXh0dXJlIiwiZW1haWwiOiJmaXh0dXJlQGV4YW1wbGUuY29tIiwiaHR0cHM6Ly9hcGkub3BlbmFpLmNvbS9hdXRoIjp7ImNoYXRncHRfYWNjb3VudF9pZCI6ImZpeHR1cmUiLCJjaGF0Z3B0X3BsYW5fdHlwZSI6InBsdXMifX0.sig"
    }});
    client
        .set_read_only_session(&document)
        .expect("external token mode");
    // Synthetic tokens are deliberately rejected by workspace discovery.
    // The successful login/start above confirms protocol compatibility.
    let rejected = client
        .read_account(false)
        .expect_err("synthetic access token");
    assert!(rejected.is_authentication_failure());
    drop(client);
    assert!(!home.directory.path().join("auth.json").exists());
}

fn fake_client(responses: Vec<Value>) -> (AppServerClient, SyncSender<Value>) {
    let (sender, messages) = mpsc::sync_channel(128);
    let mut command = Command::new(if cfg!(windows) { "cmd" } else { "sh" });
    command
        .arg(if cfg!(windows) { "/C" } else { "-c" })
        .arg(if cfg!(windows) { "more" } else { "cat" })
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    configure_supervised_process(&mut command);
    let mut process = command.spawn().expect("fake server");
    let stdin = process.stdin.take().expect("stdin");
    let supervisor = ProcessSupervisor::attach(&process).expect("supervisor");
    (
        AppServerClient {
            process,
            supervisor,
            writer: FrameWriter::new(stdin).expect("writer"),
            messages,
            buffered: responses.into(),
            stderr_tail: Arc::new(std::sync::Mutex::new(VecDeque::new())),
            next_id: 1,
            cancel: Arc::new(AtomicBool::new(false)),
        },
        sender,
    )
}

#[test]
fn reusable_usage_reader_alternates_identities_without_using_notifications_as_quota() {
    use crate::ports::{AccessSession, UsageReader};
    let (mut client, sender) = fake_client(Vec::new());
    let stdout = client.process.stdout.take().expect("fake wire");
    let reader = crate::adapters::usage_reader::AppServerUsageReader::from_test_client(client);
    for (index, (account, used)) in [("a", 10), ("b", 70), ("a", 11)].into_iter().enumerate() {
        let login_id = index as u64 * 2 + 1;
        sender.send(json!({"method":"account/rateLimits/updated","params":{"rateLimits":{"primary":{"usedPercent":99}}}})).expect("stale notification");
        sender
            .send(json!({"id":login_id,"result":{}}))
            .expect("context response");
        sender.send(json!({"id":login_id+1,"result":{"rateLimits":{"primary":{"usedPercent":used,"windowDurationMins":300}}}})).expect("quota response");
        let sample = reader
            .read(
                &AccessSession {
                    account_id: account.to_owned(),
                    access_token: format!("synthetic-{account}"),
                },
                Arc::new(AtomicBool::new(false)),
            )
            .expect("sample");
        assert_eq!(
            sample
                .rate_limits
                .expect("quota")
                .primary
                .expect("window")
                .used_percent,
            Some(used as f64)
        );
    }
    let mut wire = BufReader::new(stdout);
    for account in ["a", "b", "a"] {
        let mut line = String::new();
        wire.read_line(&mut line).expect("login frame");
        let frame: Value = serde_json::from_str(&line).expect("JSON");
        assert_eq!(frame["params"]["chatgptAccountId"], account);
        assert!(frame["params"].get("refreshToken").is_none());
        assert_eq!(frame["params"].as_object().expect("params").len(), 3);
        line.clear();
        wire.read_line(&mut line).expect("quota frame");
        let frame: Value = serde_json::from_str(&line).expect("JSON");
        assert_eq!(frame["method"], "account/rateLimits/read");
    }
    reader.close();
}

#[test]
fn reused_reader_does_not_inherit_a_late_cancellation_from_the_previous_query() {
    use crate::ports::{AccessSession, UsageReader};
    let (client, sender) = fake_client(Vec::new());
    let previous_cancel = client.cancel.clone();
    let reader = crate::adapters::usage_reader::AppServerUsageReader::from_test_client(client);
    for id in [1, 3] {
        sender
            .send(json!({"id":id,"result":{}}))
            .expect("login response");
        sender
            .send(json!({"id":id+1,"result":{"rateLimits":{"primary":{"usedPercent":17}}}}))
            .expect("quota response");
    }
    let access = AccessSession {
        account_id: "fixture".to_owned(),
        access_token: "synthetic-access".to_owned(),
    };
    reader
        .read(&access, previous_cancel.clone())
        .expect("first query");
    previous_cancel.store(true, Ordering::Release);
    let second = reader.read(&access, Arc::new(AtomicBool::new(false)));
    reader.close();
    assert!(
        second.is_ok(),
        "a completed query's cancellation cannot cancel the next one"
    );
}

#[test]
fn reused_reader_honors_a_new_query_cancellation_even_when_the_initial_token_is_not_cancelled() {
    use crate::ports::{AccessSession, UsageReader};
    let (client, sender) = fake_client(Vec::new());
    let first_cancel = client.cancel.clone();
    let reader = crate::adapters::usage_reader::AppServerUsageReader::from_test_client(client);
    for id in [1, 3] {
        sender
            .send(json!({"id":id,"result":{}}))
            .expect("login response");
        sender
            .send(json!({"id":id+1,"result":{}}))
            .expect("quota response");
    }
    let access = AccessSession {
        account_id: "fixture".to_owned(),
        access_token: "synthetic-access".to_owned(),
    };
    reader.read(&access, first_cancel).expect("first query");
    let second = reader.read(&access, Arc::new(AtomicBool::new(true)));
    reader.close();
    assert!(matches!(second, Err(AppError::Cancelled)));
}

#[cfg(target_os = "linux")]
#[test]
fn a_reused_real_jsonl_process_cancels_the_current_query_and_is_reaped() {
    use crate::ports::{AccessSession, UsageReader};
    use std::os::unix::fs::PermissionsExt;
    let fixture = tempfile::tempdir().expect("private process fixture");
    let binary = fixture.path().join("reader.py");
    std::fs::write(
        &binary,
        r#"#!/usr/bin/python3
import json, os, pathlib, sys, time
root = pathlib.Path(__file__).parent
(root / 'initial-pid').write_text(str(os.getpid()))
(root / 'actual-home').write_text(os.environ['CODEX_HOME'])
count = 0
for line in sys.stdin:
    request = json.loads(line)
    if 'id' not in request:
        continue
    method = request['method']
    if method == 'account/login/start':
        assert set(request['params']) == {'type', 'accessToken', 'chatgptAccountId'}
        assert request['params']['type'] == 'chatgptAuthTokens'
    result = {}
    if method == 'account/rateLimits/read':
        count += 1
        if count == 2:
            (root / 'blocked').write_text(str(os.getpid()))
            time.sleep(30)
        result = {'rateLimits': {'primary': {'usedPercent': 25}}}
    print(json.dumps({'id': request['id'], 'result': result}), flush=True)
"#,
    )
    .expect("synthetic remote boundary");
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700))
        .expect("private executable");
    let reader = crate::adapters::usage_reader::AppServerUsageReader::from_test_binary(binary);
    let access = AccessSession {
        account_id: "fixture".to_owned(),
        access_token: "synthetic-access".to_owned(),
    };
    reader
        .read(&access, Arc::new(AtomicBool::new(false)))
        .expect("first completed query");
    let pid: u32 = std::fs::read_to_string(fixture.path().join("initial-pid"))
        .expect("owned PID")
        .parse()
        .expect("PID");
    let home = PathBuf::from(
        std::fs::read_to_string(fixture.path().join("actual-home")).expect("actual reader home"),
    );
    assert!(home.is_dir());
    assert!(!home.join("auth.json").exists());
    let current_cancel = Arc::new(AtomicBool::new(false));
    let signal = current_cancel.clone();
    let marker = fixture.path().join("blocked");
    let sender = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if marker.is_file() {
                signal.store(true, Ordering::Release);
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        signal.store(true, Ordering::Release);
        panic!("second query did not reach the blocked I/O boundary");
    });
    let started = Instant::now();
    let result = reader.read(&access, current_cancel);
    sender.join().expect("signal task");
    assert!(matches!(result, Err(AppError::Cancelled)));
    assert!(started.elapsed() < Duration::from_millis(750));
    assert!(
        !std::path::Path::new(&format!("/proc/{pid}")).exists(),
        "owned process must be reaped after cancelled RPC"
    );
    assert!(
        !home.exists(),
        "cancelled channel must release its actual private home"
    );
    let next = reader
        .read(&access, Arc::new(AtomicBool::new(false)))
        .expect("new channel after cancellation without replay");
    let replacement: u32 = std::fs::read_to_string(fixture.path().join("initial-pid"))
        .expect("new PID")
        .parse()
        .expect("PID");
    let replacement_home = PathBuf::from(
        std::fs::read_to_string(fixture.path().join("actual-home")).expect("replacement home"),
    );
    assert!(replacement_home.is_dir());
    assert!(!replacement_home.join("auth.json").exists());
    reader.close();
    assert_ne!(replacement, pid);
    assert_eq!(
        next.rate_limits
            .expect("new quota")
            .primary
            .expect("window")
            .used_percent,
        Some(25.0)
    );
    assert!(!std::path::Path::new(&format!("/proc/{replacement}")).exists());
    assert!(!replacement_home.exists());
}

#[test]
fn confirmed_oauth_completion_is_not_discarded_when_its_browser_has_closed() {
    let (mut client, _sender) = fake_client(vec![
        json!({"method":"account/login/completed","params":{"success":true,"loginId":"fixture"}}),
        json!({"method":"account/updated","params":{"authMode":"chatgpt"}}),
    ]);
    client
        .wait_for_login(
            "fixture",
            &Arc::new(AtomicBool::new(false)),
            Some(Duration::from_secs(1)),
            || Ok(false),
        )
        .expect("confirmed OAuth, even after browser close");
}

#[test]
fn closed_browser_without_its_confirmed_oauth_completion_is_still_cancelled() {
    let (mut client, _sender) = fake_client(vec![
        json!({"method":"account/login/completed","params":{"success":true,"loginId":"some-other-login"}}),
    ]);
    let result = client.wait_for_login(
        "fixture",
        &Arc::new(AtomicBool::new(false)),
        Some(Duration::from_secs(1)),
        || Ok(false),
    );
    assert!(matches!(result, Err(AppError::Cancelled)));
}

#[test]
fn queued_oauth_completion_precedes_closed_browser_and_explicit_cancel_still_wins() {
    let (mut client, sender) = fake_client(Vec::new());
    sender.send(json!({"method":"account/login/completed","params":{"success":true,"loginId":"fixture"}})).expect("wire completion");
    sender
        .send(json!({"method":"account/updated","params":{"authMode":"chatgpt"}}))
        .expect("wire identity updated");
    client
        .wait_for_login(
            "fixture",
            &Arc::new(AtomicBool::new(false)),
            Some(Duration::from_secs(1)),
            || Ok(false),
        )
        .expect("actual queue before closed browser");
    let (mut client, _sender) = fake_client(vec![
        json!({"method":"account/login/completed","params":{"success":true,"loginId":"fixture"}}),
    ]);
    assert!(matches!(
        client.wait_for_login(
            "fixture",
            &Arc::new(AtomicBool::new(true)),
            Some(Duration::from_secs(1)),
            || Ok(false)
        ),
        Err(AppError::Cancelled)
    ));
}

#[test]
fn revoked_token_after_successful_callback_is_not_accepted_as_valid() {
    let (mut client, _sender) = fake_client(vec![
        json!({"method":"account/login/completed","params":{"success":true,"loginId":"fixture"}}),
        json!({"method":"account/updated","params":{"authMode":"chatgpt"}}),
        json!({"id":1,"result":{"account":{"type":"chatgpt","email":"fixture@example.test"}}}),
        json!({"id":2,"error":{"code":-32000,"message":"401 Unauthorized: token_revoked"}}),
    ]);
    client
        .wait_for_login(
            "fixture",
            &Arc::new(AtomicBool::new(false)),
            Some(Duration::from_secs(1)),
            || Ok(true),
        )
        .expect("callback");
    let error = client
        .validate_session_with_trace(&mut |_, _| {})
        .expect_err("revoked token");
    assert!(error.is_authentication_failure());
}

#[test]
fn unavailable_usage_does_not_discard_identity_or_request_reauthentication() {
    let (mut client, _sender) = fake_client(vec![
        json!({"id":1,"result":{"account":{"type":"chatgpt","email":"fixture@example.test"}}}),
        json!({"id":2,"error":{"code":-32000,"message":"service temporarily unavailable"}}),
    ]);
    let snapshot = client
        .validate_session_with_trace(&mut |_, _| {})
        .expect("valid identity");
    assert!(snapshot.rate_limits.is_none());
}

#[test]
fn missing_identity_is_typed_and_does_not_wait_for_usage() {
    let (mut client, _sender) = fake_client(vec![json!({"id":1,"result":{"account":null}})]);
    let error = client
        .validate_session_with_trace(&mut |_, _| {})
        .expect_err("missing identity");
    assert!(matches!(error, AppError::SessionRenewalRequired(_)));
    assert!(!error.is_refresh_rejected());
    assert_eq!(client.next_id, 2);
}

#[test]
fn shutdown_interrupts_an_outstanding_server_request() {
    let (mut client, _sender) = fake_client(Vec::new());
    let cancel = client.cancel.clone();
    let signal = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(20));
        cancel.store(true, Ordering::Release);
    });
    let started = Instant::now();
    let error = client.read_account(false).expect_err("cancelled request");
    signal.join().expect("signal thread");
    assert!(matches!(error, AppError::Cancelled));
    assert!(started.elapsed() < Duration::from_millis(500));
}

#[test]
fn stored_session_validation_requests_token_renewal_before_verification() {
    let (mut client, _sender) = fake_client(vec![
        json!({"id":1,"result":{"account":{"type":"chatgpt"}}}),
        json!({"id":2,"result":{}}),
    ]);
    client
        .validate_stored_session_with_trace(&mut |_, _| {})
        .expect("renewed session");
    let stdout = client.process.stdout.as_mut().expect("fake wire");
    let mut request = String::new();
    BufReader::new(stdout)
        .read_line(&mut request)
        .expect("wire request");
    let request: Value = serde_json::from_str(&request).expect("request");
    assert_eq!(request["method"], "account/read");
    assert_eq!(request["params"]["refreshToken"], true);
}

#[test]
#[ignore = "requires a current saved account; read-only remote quota query"]
fn installed_usage_probe_preserves_the_current_auth_file() {
    let path = crate::adapters::auth_store::codex_home().join("auth.json");
    let before = std::fs::read(&path).expect("current auth");
    let document = crate::adapters::auth_store::read_auth(&path).expect("saved credential");
    let reader = crate::adapters::usage_reader::AppServerUsageReader::default();
    let snapshot = crate::ports::UsageReader::read(
        &reader,
        &crate::adapters::auth_store::access_session(&document, None).expect("access only"),
        Arc::new(AtomicBool::new(false)),
    )
    .expect("remote usage");
    assert!(snapshot.rate_limits.is_some());
    assert!(
        std::fs::read(&path).expect("auth after query") == before,
        "read-only query must preserve auth; credential contents are intentionally not printed"
    );
}

#[test]
fn buffered_lookup_skips_unrelated_messages_without_reordering_loop() {
    let (_sender, messages) = mpsc::sync_channel(128);
    let mut command = Command::new(if cfg!(windows) { "cmd" } else { "sh" });
    command
        .arg(if cfg!(windows) { "/C" } else { "-c" })
        .arg(if cfg!(windows) { "more" } else { "cat" })
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    configure_supervised_process(&mut command);
    let process = command.spawn().expect("test process");
    let mut process = process;
    let stdin = process.stdin.take().expect("stdin");
    let supervisor = ProcessSupervisor::attach(&process).expect("supervisor");
    let mut client = AppServerClient {
        process,
        supervisor,
        writer: FrameWriter::new(stdin).expect("writer"),
        messages,
        buffered: VecDeque::from([
            json!({ "method": "account/updated", "params": { "authMode": "chatgpt" } }),
            json!({ "id": 7, "result": { "ok": true } }),
        ]),
        stderr_tail: Arc::new(std::sync::Mutex::new(VecDeque::new())),
        next_id: 8,
        cancel: Arc::new(AtomicBool::new(false)),
    };

    let response = client
        .take_buffered(|message| response_id(message) == Some(7))
        .expect("matching response");

    assert_eq!(response_result(response).expect("result")["ok"], true);
    assert_eq!(client.buffered.len(), 1);
    assert_eq!(
        client.buffered[0].get("method").and_then(Value::as_str),
        Some("account/updated")
    );
}

#[test]
fn snapshot_prefers_the_codex_limit_and_clamps_percentages() {
    let account = json!({ "account": { "email": "person@example.com", "planType": "plus" } });
    let limits = json!({
        "rateLimitsByLimitId": {
            "codex": {
                "primary": { "usedPercent": 125, "windowDurationMins": 300, "resetsAt": 1800000000 },
                "secondary": { "usedPercent": 42.5, "windowDurationMins": 10080 }
            }
        },
        "rateLimitResetCredits": { "availableCount": 2 }
    });

    let snapshot = session_snapshot(&account, &limits).expect("snapshot");

    assert_eq!(snapshot.email.as_deref(), Some("person@example.com"));
    assert_eq!(snapshot.plan_type.as_deref(), Some("plus"));
    let rate_limits = snapshot.rate_limits.expect("rate limits");
    assert_eq!(
        rate_limits.primary.expect("primary").used_percent,
        Some(100.0)
    );
    assert_eq!(
        rate_limits.secondary.expect("secondary").used_percent,
        Some(42.5)
    );
    assert_eq!(
        rate_limits
            .reset_credits
            .expect("reset credits")
            .available_count,
        Some(2)
    );
}

#[test]
fn snapshot_keeps_identity_when_rate_limits_are_unavailable() {
    let account = json!({
        "account": { "email": "person@example.com", "planType": "plus" }
    });

    let snapshot = session_snapshot(&account, &Value::Null).expect("snapshot");

    assert_eq!(snapshot.email.as_deref(), Some("person@example.com"));
    assert_eq!(snapshot.plan_type.as_deref(), Some("plus"));
    assert!(snapshot.rate_limits.is_none());
}

#[test]
fn login_notifications_accept_optional_correlation_fields() {
    let completion_without_id = json!({
        "method": "account/login/completed",
        "params": { "success": true, "loginId": null }
    });
    let update_without_mode = serde_json::Map::new();

    assert!(is_login_completion(&completion_without_id, "expected-id"));
    assert!(account_update_matches_login(&update_without_mode));
    assert!(!is_login_completion(
        &json!({
            "method": "account/login/completed",
            "params": { "success": true, "loginId": "another-id" }
        }),
        "expected-id"
    ));
}

#[test]
fn missing_account_update_does_not_keep_login_waiting_forever() {
    let (_sender, messages) = mpsc::sync_channel(128);
    let mut command = Command::new(if cfg!(windows) { "cmd" } else { "sh" });
    command
        .arg(if cfg!(windows) { "/C" } else { "-c" })
        .arg(if cfg!(windows) { "more" } else { "cat" })
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    configure_supervised_process(&mut command);
    let mut process = command.spawn().expect("test process");
    let stdin = process.stdin.take().expect("stdin");
    let supervisor = ProcessSupervisor::attach(&process).expect("supervisor");
    let mut client = AppServerClient {
        process,
        supervisor,
        writer: FrameWriter::new(stdin).expect("writer"),
        messages,
        buffered: VecDeque::from([json!({
            "method": "account/login/completed",
            "params": { "success": true, "loginId": "expected-id" }
        })]),
        stderr_tail: Arc::new(std::sync::Mutex::new(VecDeque::new())),
        next_id: 1,
        cancel: Arc::new(AtomicBool::new(false)),
    };
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));

    client
        .wait_for_login_with_update_timeout(
            "expected-id",
            &cancel,
            Some(Duration::from_secs(1)),
            || Ok(true),
            Duration::ZERO,
        )
        .expect("login completion remains sufficient");
}

#[test]
fn stderr_tail_is_bounded_and_sanitized() {
    let tail = Arc::new(std::sync::Mutex::new(VecDeque::new()));
    let input = (0..25)
        .map(|index| {
            format!("line {index} https://login.example/callback?code=secret&state=private\n")
        })
        .collect::<String>();

    read_stderr(input.as_bytes(), tail.clone());

    let lines = tail.lock().expect("tail");
    assert_eq!(lines.len(), 20);
    assert!(lines
        .front()
        .is_some_and(|line| line.starts_with("line 5 ")));
    assert!(lines.iter().all(|line| !line.contains("secret")));
    assert!(lines.iter().all(|line| !line.contains('?')));
}

#[test]
fn stderr_discards_a_whole_oversized_record_and_keeps_draining() {
    use std::io::Read;
    let tail = Arc::new(std::sync::Mutex::new(VecDeque::new()));
    let oversized = std::io::repeat(b'x').take(3 * 1024 * 1024);
    let input = oversized.chain(b" secret-fragment\nnext safe diagnostic\n".as_slice());

    read_stderr(input, tail.clone());

    let lines = tail.lock().expect("tail");
    assert_eq!(lines.len(), 2);
    assert!(lines.front().expect("omission").contains("omitido"));
    assert_eq!(lines.back().expect("next record"), "next safe diagnostic");
    assert!(lines.iter().all(|line| !line.contains("secret-fragment")));
    assert!(lines.iter().all(|line| line.len() <= 1600));
}

#[cfg(target_os = "linux")]
#[test]
fn real_pipes_route_interleaved_rpc_and_notifications_without_prefilled_results() {
    let mut command = Command::new("sh");
    command.args(["-c", r#"
        IFS= read -r request || exit 2
        case "$request" in *'"method":"account/read"'*'"refreshToken":true'*) ;; *) exit 3 ;; esac
        printf '%s\n' '{"method":"account/updated","params":{"authMode":"chatgpt"}}'
        printf '%s\n' '{"id":1,"result":{"account":{"type":"chatgpt","email":"fixture@example.test"}}}'
        IFS= read -r request || exit 4
        case "$request" in *'"method":"account/rateLimits/read"'*) ;; *) exit 5 ;; esac
        printf '%s\n' '{"id":2,"result":{"rateLimits":{"primary":{"usedPercent":17}}}}'
    "#]).stdin(Stdio::piped()).stdout(Stdio::piped());
    let (mut process, supervisor) = spawn_supervised(command).expect("owned fixture");
    let stdin = process.stdin.take().expect("stdin");
    let stdout = process.stdout.take().expect("stdout");
    let tail = Arc::new(std::sync::Mutex::new(VecDeque::new()));
    let (sender, messages) = mpsc::sync_channel(32);
    let reader_tail = tail.clone();
    let reader = std::thread::spawn(move || read_messages(stdout, sender, reader_tail));
    let mut client = AppServerClient {
        process,
        supervisor,
        writer: FrameWriter::new(stdin).expect("writer"),
        messages,
        buffered: VecDeque::new(),
        stderr_tail: tail,
        next_id: 1,
        cancel: Arc::new(AtomicBool::new(false)),
    };
    let outcome = client.validate_stored_session_with_trace(&mut |_, _| {});
    let notification = client.take_buffered(|message| {
        notification_matches(message, "account/updated", &account_update_matches_login)
    });
    drop(client);
    reader.join().expect("fixture reader collected");
    let snapshot = outcome.expect("correlated response from real pipes");
    assert_eq!(snapshot.email.as_deref(), Some("fixture@example.test"));
    assert_eq!(
        snapshot
            .rate_limits
            .expect("limits")
            .primary
            .expect("window")
            .used_percent,
        Some(17.0)
    );
    assert!(notification.is_some());
}

use super::*;
#[test]
fn harness_options_are_supported_but_auth_overrides_are_refused() {
    let directory = tempfile::tempdir().expect("workspace");
    let mut request = ExecRequest {
        group_id: None,
        binding_id: "binding".to_owned(),
        binding_snapshot: None,
        account_id: None,
        cwd: directory.path().to_string_lossy().into_owned(),
        prompt: "test".to_owned(),
        arguments: vec![
            "--sandbox".to_owned(),
            "workspace-write".to_owned(),
            "--disable".to_owned(),
            "hooks".to_owned(),
        ],
    };
    validate_request(&request).expect("harness options");
    request.arguments = vec![
        "--config".to_owned(),
        "cli_auth_credentials_store=keyring".to_owned(),
    ];
    assert!(validate_request(&request).is_err());
}
#[test]
fn output_is_private_bounded_and_keeps_codex_jsonl() {
    let directory = tempfile::tempdir().expect("output");
    let path = directory.path().join("events.jsonl");
    let bytes = b"{\"type\":\"item.completed\"}\n{\"type\":\"turn.completed\"}\n";
    capture(std::io::Cursor::new(bytes), &path).expect("capture");
    assert_eq!(std::fs::read(&path).expect("read"), bytes);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[cfg(target_os = "linux")]
fn fixture_command(directory: &Path, body: &str) -> Command {
    use std::os::unix::fs::PermissionsExt;
    let binary = directory.join("exec-fixture.py");
    std::fs::write(&binary, format!("#!/usr/bin/python3\nimport json, os, pathlib, sys, time\nroot = pathlib.Path(__file__).parent\n(root / 'started').write_text(str(os.getpid()))\n{body}\n")).expect("synthetic executable");
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700))
        .expect("private executable");
    let mut command = Command::new(binary);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

#[cfg(target_os = "linux")]
fn cancel_when_started(directory: &Path, cancel: Arc<AtomicBool>) -> std::thread::JoinHandle<()> {
    let marker = directory.join("started");
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            if marker.is_file() {
                cancel.store(true, Ordering::Release);
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        cancel.store(true, Ordering::Release);
        panic!("fixture did not start");
    })
}

#[cfg(target_os = "linux")]
fn assert_owned_child_reaped(directory: &Path) {
    let pid: u32 = std::fs::read_to_string(directory.join("started"))
        .expect("PID")
        .parse()
        .expect("PID number");
    assert!(
        !Path::new(&format!("/proc/{pid}")).exists(),
        "exec must collect its owned child before returning"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn cancelling_exec_with_blocked_prompt_does_not_turn_cleanup_broken_pipe_into_failure() {
    let dir = tempfile::tempdir().expect("private exec fixture");
    let command = fixture_command(dir.path(), "time.sleep(30)");
    let cancel = Arc::new(AtomicBool::new(false));
    let signal = cancel_when_started(dir.path(), cancel.clone());
    let started = std::time::Instant::now();
    let result = execute(
        command,
        &"x".repeat(192 * 1024),
        &dir.path().join("events.jsonl"),
        cancel,
    );
    signal.join().expect("signal owner");
    assert_owned_child_reaped(dir.path());
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(
        matches!(result, Err(AppError::Cancelled)),
        "cancel must not be replaced by I/O fallout: {}",
        result
            .as_ref()
            .err()
            .map_or("unexpected success", AppError::code)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn invalid_exec_jsonl_keeps_the_capture_error_when_prompt_writer_is_also_blocked() {
    let dir = tempfile::tempdir().expect("private exec fixture");
    let command = fixture_command(
        dir.path(),
        "print('synthetic-not-json', flush=True)\ntime.sleep(30)",
    );
    let result = execute(
        command,
        &"x".repeat(192 * 1024),
        &dir.path().join("events.jsonl"),
        Arc::new(AtomicBool::new(false)),
    );
    assert_owned_child_reaped(dir.path());
    assert!(
        matches!(result, Err(AppError::Json(_))),
        "capture failure must not be replaced by I/O fallout"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn successful_exec_preserves_production_arguments_private_home_and_complete_output() {
    let dir = tempfile::tempdir().expect("private fixture");
    let _ = fixture_command(
        dir.path(),
        r#"
assert sys.argv[1] == 'exec'
for expected in ['--json', '--ephemeral', '--ignore-user-config', '--strict-config', 'cli_auth_credentials_store="file"', 'forced_login_method="chatgpt"', 'model_provider="openai"']:
    assert expected in sys.argv
assert sys.argv[-1] == '-'
home = pathlib.Path(os.environ['CODEX_HOME'])
assert home == root / 'private-home'
prompt = sys.stdin.read()
assert prompt == 'synthetic prompt with spaces and quotes'
sys.stderr.write('synthetic diagnostic\n' * 10000)
print(json.dumps({'type': 'item.completed', 'text': prompt}), flush=True)
doc = json.loads((home / 'auth.json').read_text())
doc['tokens']['refresh_token'] = 'synthetic-renewed'
(home / 'auth.json').write_text(json.dumps(doc))
print(json.dumps({'type': 'turn.completed'}), flush=True)
"#,
    );
    let home = dir.path().join("private-home");
    let document = serde_json::json!({"auth_mode":"chatgpt","tokens":{"account_id":"synthetic-account","access_token":"synthetic-access","refresh_token":"synthetic-before"}});
    crate::adapters::auth_store::atomic_write_auth(&home.join("auth.json"), &document)
        .expect("synthetic auth");
    let request = ExecRequest {
        group_id: None,
        binding_id: String::new(),
        binding_snapshot: None,
        account_id: None,
        cwd: dir.path().to_string_lossy().into_owned(),
        prompt: "synthetic prompt with spaces and quotes".to_owned(),
        arguments: vec!["--sandbox".to_owned(), "read-only".to_owned()],
    };
    validate_request(&request).expect("request");
    let output = dir.path().join("events.jsonl");
    let command = command_for(&request, &home, &dir.path().join("exec-fixture.py"));
    let status = execute(
        command,
        &request.prompt,
        &output,
        Arc::new(AtomicBool::new(false)),
    )
    .expect("complete execution");
    assert_eq!(status, 0);
    assert_owned_child_reaped(dir.path());
    let bytes = std::fs::read_to_string(&output).expect("complete JSONL");
    let events: Vec<serde_json::Value> = bytes
        .lines()
        .map(|line| serde_json::from_str(line).expect("JSON event"))
        .collect();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["text"], request.prompt);
    assert_eq!(events[1]["type"], "turn.completed");
    assert_eq!(
        crate::adapters::auth_store::read_auth(&home.join("auth.json"))
            .expect("auth ready for caller capture")["tokens"]["refresh_token"],
        "synthetic-renewed"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn exec_exit_code_is_preserved_without_claiming_success() {
    let dir = tempfile::tempdir().expect("private fixture");
    let command = fixture_command(dir.path(), "sys.stdin.read()\nprint(json.dumps({'type':'error','message':'synthetic rejection'}), flush=True)\nsys.exit(7)");
    let result = execute(
        command,
        "synthetic prompt",
        &dir.path().join("events.jsonl"),
        Arc::new(AtomicBool::new(false)),
    );
    assert_owned_child_reaped(dir.path());
    assert_eq!(result.expect("nonzero exit reported"), 7);
}

#[cfg(target_os = "linux")]
#[test]
fn rejected_capture_size_is_not_reported_as_user_cancellation_or_writer_error() {
    let dir = tempfile::tempdir().expect("private fixture");
    let command = fixture_command(
        dir.path(),
        "print('x' * (256 * 1024 + 1), flush=True)\ntime.sleep(30)",
    );
    let result = execute(
        command,
        &"x".repeat(192 * 1024),
        &dir.path().join("events.jsonl"),
        Arc::new(AtomicBool::new(false)),
    );
    assert_owned_child_reaped(dir.path());
    assert!(matches!(result, Err(AppError::Validation(_))));
}

#[cfg(target_os = "linux")]
#[test]
fn cancelling_before_exec_admission_never_starts_a_process_or_creates_output() {
    let dir = tempfile::tempdir().expect("private fixture");
    let command = fixture_command(dir.path(), "time.sleep(30)");
    let output = dir.path().join("events.jsonl");
    let result = execute(
        command,
        "synthetic",
        &output,
        Arc::new(AtomicBool::new(true)),
    );
    assert!(matches!(result, Err(AppError::Cancelled)));
    assert!(!dir.path().join("started").exists());
    assert!(!output.exists());
}

#[cfg(target_os = "linux")]
#[test]
fn missing_pipe_failure_closes_the_owned_child_before_returning() {
    let dir = tempfile::tempdir().expect("private fixture");
    let mut command = fixture_command(dir.path(), "time.sleep(30)");
    command.stderr(Stdio::null());
    let result = execute(
        command,
        "synthetic",
        &dir.path().join("events.jsonl"),
        Arc::new(AtomicBool::new(false)),
    );
    assert!(matches!(result, Err(AppError::Internal(_))));
    // When Python has not yet started, no marker exists. If it did, its PID
    // must already be collected by the early-return scope guard.
    if dir.path().join("started").exists() {
        assert_owned_child_reaped(dir.path());
    }
}

#[cfg(target_os = "linux")]
#[test]
fn an_exited_exec_leader_does_not_release_the_runner_while_a_descendant_keeps_pipes_open() {
    let dir = tempfile::tempdir().expect("private tree fixture");
    let command = fixture_command(
        dir.path(),
        r#"
import signal
pid = os.fork()
if pid == 0:
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    (root / 'descendant-ready').write_text(str(os.getpid()))
    time.sleep(30)
else:
    while not (root / 'descendant-ready').is_file():
        time.sleep(0.005)
    print(json.dumps({'type':'turn.completed'}), flush=True)
    sys.exit(0)
"#,
    );
    let started = std::time::Instant::now();
    let result = execute(
        command,
        "synthetic",
        &dir.path().join("events.jsonl"),
        Arc::new(AtomicBool::new(false)),
    );
    assert_owned_child_reaped(dir.path());
    assert_eq!(result.expect("complete cleanup"), 0);
    let descendant =
        std::fs::read_to_string(dir.path().join("descendant-ready")).expect("descendant identity");
    let stat = std::fs::read_to_string(format!("/proc/{descendant}/stat"));
    assert!(
        stat.is_err()
            || stat.as_ref().is_ok_and(|value| value
                .rsplit_once(") ")
                .is_some_and(|(_, fields)| fields.starts_with("Z ") || fields.starts_with("X "))),
        "no descendant can retain a live pipe after return"
    );
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn worker_panic_signals_stop_and_can_be_collected_as_a_typed_failure() {
    let failed = AtomicBool::new(false);
    let result = std::thread::scope(|scope| {
        let worker = spawn_worker(
            scope,
            "synthetic-panic",
            &failed,
            || -> Result<(), AppError> {
                panic!("synthetic worker failure");
            },
        )
        .expect("spawn");
        collect_worker(worker)
    });
    assert!(failed.load(Ordering::Acquire));
    assert!(matches!(result, Err(AppError::Internal(_))));
}

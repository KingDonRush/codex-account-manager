use super::*;
use crate::adapters::{
    process_launcher::spawn_supervised, process_supervisor::configure_supervised_process,
};

fn synthetic_pipe(
    serve: impl FnOnce(std::os::unix::net::UnixStream) + Send + 'static,
) -> (CdpPipe, std::thread::JoinHandle<()>) {
    let (parent, child) = std::os::unix::net::UnixStream::pair().expect("private CDP channel");
    parent
        .set_read_timeout(Some(Duration::from_secs(1)))
        .expect("deadline");
    let worker = std::thread::spawn(move || serve(child));
    (
        CdpPipe {
            channel: Mutex::new((BufReader::new(parent), 0)),
        },
        worker,
    )
}

fn read_request(reader: &mut BufReader<std::os::unix::net::UnixStream>) -> Value {
    let mut frame = Vec::new();
    reader.read_until(0, &mut frame).expect("request bytes");
    assert_eq!(frame.pop(), Some(0));
    serde_json::from_slice(&frame).expect("CDP request")
}
fn reply(reader: &mut BufReader<std::os::unix::net::UnixStream>, id: &Value, result: Value) {
    let mut frame = serde_json::to_vec(&json!({"id":id,"result":result})).expect("response");
    frame.push(0);
    reader.get_mut().write_all(&frame).expect("response bytes");
}

#[test]
fn missing_target_inventory_is_not_proof_that_login_or_manual_pages_are_closed() {
    for check_alive in [true, false] {
        let (pipe, server) = synthetic_pipe(move |wire| {
            let mut reader = BufReader::new(wire);
            loop {
                let mut frame = Vec::new();
                if reader.read_until(0, &mut frame).expect("request") == 0 {
                    break;
                }
                frame.pop();
                let request: Value = serde_json::from_slice(&frame).expect("request JSON");
                let result = if request["method"] == "Target.closeTarget" {
                    json!({"success":true})
                } else {
                    json!({})
                };
                reply(&mut reader, &request["id"], result);
            }
        });
        let targets = ["owned-login".to_owned()];
        let result = if check_alive {
            pipe.owns_alive_target(&targets)
        } else {
            pipe.close_targets(&targets)
        };
        drop(pipe);
        server.join().expect("wire owner");
        assert!(
            result.is_err(),
            "unknown inventory must not certify a closure"
        );
    }
}

#[test]
fn a_page_that_failed_to_close_is_not_reported_as_a_successful_cleanup_with_manual_pages_remaining()
{
    let (pipe, server) = synthetic_pipe(|wire| {
        let mut reader = BufReader::new(wire);
        loop {
            let mut frame = Vec::new();
            if reader.read_until(0, &mut frame).expect("request") == 0 {
                break;
            }
            frame.pop();
            let request: Value = serde_json::from_slice(&frame).expect("request JSON");
            let result = if request["method"] == "Target.closeTarget" {
                json!({"success":false})
            } else {
                json!({"targetInfos":[{"targetId":"owned-login","type":"page"}]})
            };
            reply(&mut reader, &request["id"], result);
        }
    });
    let result = pipe.close_targets(&["owned-login".to_owned()]);
    drop(pipe);
    server.join().expect("wire owner");
    assert!(
        result.is_err(),
        "owned page remaining is not a manual page to preserve"
    );
}

#[test]
fn closing_targets_that_are_already_absent_is_idempotent_and_preserves_manual_pages() {
    let (pipe, server) = synthetic_pipe(|wire| {
        let mut reader = BufReader::new(wire);
        loop {
            let mut frame = Vec::new();
            if reader.read_until(0, &mut frame).expect("request") == 0 {
                break;
            }
            frame.pop();
            let request: Value = serde_json::from_slice(&frame).expect("request JSON");
            if request["method"] == "Target.closeTarget" {
                let mut response = serde_json::to_vec(&json!({"id":request["id"],"error":{"code":-32602,"message":"No target with given id found"}})).expect("error response");
                response.push(0);
                reader.get_mut().write_all(&response).expect("response");
            } else {
                reply(
                    &mut reader,
                    &request["id"],
                    json!({"targetInfos":[{"targetId":"manual","type":"page"}]}),
                );
            }
        }
    });
    let result = pipe.close_targets(&["already-closed".to_owned()]);
    drop(pipe);
    server.join().expect("wire owner");
    assert!(
        matches!(result, Ok(false)),
        "missing owned target is a completed cleanup, not a reason to kill manual pages"
    );
}

#[test]
fn opening_mail_failure_removes_the_partial_login_target_without_closing_a_manual_page() {
    let (pipe, server) = synthetic_pipe(|wire| {
        let mut reader = BufReader::new(wire);
        let first = read_request(&mut reader);
        assert_eq!(first["method"], "Target.createTarget");
        reply(
            &mut reader,
            &first["id"],
            json!({"targetId":"partial-login"}),
        );
        let mail = read_request(&mut reader);
        assert_eq!(mail["method"], "Target.createTarget");
        let mut failure = serde_json::to_vec(
            &json!({"id":mail["id"],"error":{"code":-32000,"message":"synthetic refused target"}}),
        )
        .expect("response");
        failure.push(0);
        reader.get_mut().write_all(&failure).expect("failure reply");
        let inventory = read_request(&mut reader);
        assert_eq!(inventory["method"], "Target.getTargets");
        reply(
            &mut reader,
            &inventory["id"],
            json!({"targetInfos":[{"targetId":"partial-login","type":"page"},{"targetId":"manual","type":"page"}]}),
        );
        let close = read_request(&mut reader);
        assert_eq!(close["method"], "Target.closeTarget");
        assert_eq!(close["params"]["targetId"], "partial-login");
        reply(&mut reader, &close["id"], json!({"success":true}));
        let final_inventory = read_request(&mut reader);
        assert_eq!(final_inventory["method"], "Target.getTargets");
        reply(
            &mut reader,
            &final_inventory["id"],
            json!({"targetInfos":[{"targetId":"manual","type":"page"}]}),
        );
    });
    let result = pipe.create_operation_targets("about:blank#login", Some("about:blank#mail"));
    drop(pipe);
    server.join().expect("fixture boundary");
    assert!(matches!(result, Err(AppError::Browser(_))));
}

#[test]
fn a_target_inventory_entry_without_an_identity_or_kind_is_not_usable() {
    for target in [
        json!({"type":"page"}),
        json!({"targetId":"manual"}),
        json!({"targetId":"","type":"page"}),
    ] {
        let (pipe, server) = synthetic_pipe(move |wire| {
            let mut reader = BufReader::new(wire);
            let request = read_request(&mut reader);
            reply(&mut reader, &request["id"], json!({"targetInfos":[target]}));
        });
        let result = pipe.owns_alive_target(&["login".to_owned()]);
        drop(pipe);
        server.join().expect("wire owner");
        assert!(result.is_err());
    }
}

fn chromium() -> std::path::PathBuf {
    std::env::var_os("CODEX_MANAGER_TEST_CHROMIUM")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            which::which("google-chrome")
                .or_else(|_| which::which("chromium"))
                .expect("Chromium")
        })
}

#[test]
#[ignore = "requires an installed Chromium; uses only a disposable headless profile, no authentication"]
fn real_pipe_controls_only_owned_targets_without_localhost_bridge() {
    let home = tempfile::tempdir().expect("profile");
    let executable = chromium();
    let mut command = Command::new(executable);
    configure_supervised_process(&mut command);
    command
        .arg(format!("--user-data-dir={}", home.path().display()))
        .args([
            "--headless=new",
            "--no-first-run",
            "--no-default-browser-check",
            "--remote-debugging-pipe",
            "--no-startup-window",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let pipe = CdpPipe::prepare(&mut command).expect("pipe");
    let (mut child, supervisor) = spawn_supervised(command).expect("browser");
    let result = (|| {
        let manual = pipe.create_target("about:blank")?;
        let login = pipe.create_target("https://example.com/")?;
        if !pipe.owns_alive_target(std::slice::from_ref(&login))? {
            return Err(AppError::Browser("Login target missing.".to_owned()));
        }
        if pipe.close_targets(std::slice::from_ref(&login))? {
            return Err(AppError::Browser("Manual target was closed.".to_owned()));
        }
        if !pipe.owns_alive_target(std::slice::from_ref(&manual))? {
            return Err(AppError::Browser("Manual target missing.".to_owned()));
        }
        Ok::<_, AppError>(())
    })();
    supervisor.terminate(&mut child).expect("cleanup");
    let _ = child.wait();
    result.expect("owned targets controlled");
}

fn headless(
    profile: &std::path::Path,
) -> (
    CdpPipe,
    std::process::Child,
    crate::adapters::process_supervisor::ProcessSupervisor,
) {
    let executable = chromium();
    let mut command = Command::new(executable);
    configure_supervised_process(&mut command);
    command
        .arg(format!("--user-data-dir={}", profile.display()))
        .args([
            "--headless=new",
            "--no-first-run",
            "--no-default-browser-check",
            "--remote-debugging-pipe",
            "--no-startup-window",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let pipe = CdpPipe::prepare(&mut command).expect("pipe");
    let (child, supervisor) = spawn_supervised(command).expect("owned browser");
    (pipe, child, supervisor)
}

fn browser_handle(
    pipe: std::sync::Arc<CdpPipe>,
    child: std::process::Child,
    supervisor: crate::adapters::process_supervisor::ProcessSupervisor,
    targets: Vec<String>,
) -> super::super::BrowserHandle {
    use std::sync::{Arc, Mutex};
    super::super::BrowserHandle {
        child: Arc::new(Mutex::new(child)),
        supervisor: Arc::new(supervisor),
        stderr_tail: Arc::new(Mutex::new(std::collections::VecDeque::new())),
        redirects: Arc::new(Vec::new()),
        navigation_retry: None,
        controller: Some(pipe),
        owned_targets: targets,
        #[cfg(target_os = "linux")]
        native: None,
        #[cfg(target_os = "linux")]
        native_operation: None,
    }
}

#[test]
#[ignore = "requires installed Chromium; actual operation handle/idempotent closure, private headless profile only"]
fn real_operation_handle_closes_only_its_targets_and_supports_repeated_cleanup() {
    let home = tempfile::tempdir().expect("private profile");
    let (pipe, child, supervisor) = headless(home.path());
    let pipe = std::sync::Arc::new(pipe);
    let manual = pipe
        .create_target("about:blank#manual")
        .expect("manual page");
    let targets = pipe
        .create_operation_targets("about:blank#login", Some("about:blank#mail"))
        .expect("operation pages");
    let handle = browser_handle(pipe.clone(), child, supervisor, targets);
    handle.terminate().expect("cancel operation targets");
    handle.terminate().expect("repeat is idempotent");
    assert!(handle.is_alive().expect("manual browser remains"));
    assert!(pipe
        .owns_alive_target(&[manual])
        .expect("manual target intact"));
    assert!(
        matches!(handle.ensure_alive(), Err(AppError::Cancelled)),
        "remaining manual page is not a live login operation"
    );
    drop(handle);
}

#[test]
#[ignore = "requires installed Chromium; private headless process closes when its only operation ends"]
fn real_operation_handle_collects_its_process_when_no_manual_pages_remain() {
    let home = tempfile::tempdir().expect("private profile");
    let (pipe, child, supervisor) = headless(home.path());
    let pipe = std::sync::Arc::new(pipe);
    let targets = pipe
        .create_operation_targets("about:blank#login", Some("about:blank#mail"))
        .expect("operation pages");
    let handle = browser_handle(pipe, child, supervisor, targets);
    let pid = handle.pid().expect("owned browser");
    handle.terminate().expect("complete cleanup");
    handle.terminate().expect("repeat cleanup");
    assert!(!handle.is_alive().expect("child collected"));
    #[cfg(target_os = "linux")]
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
}
fn wait_for_exit(child: &mut std::process::Child) -> Result<(), AppError> {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while child.try_wait()?.is_none() {
        if std::time::Instant::now() > deadline {
            return Err(AppError::TimedOut);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

#[test]
#[ignore = "requires installed Chromium; disposable profile with synthetic restore preferences, no login"]
fn reopening_a_restore_enabled_profile_does_not_reopen_the_previous_login_tabs() {
    let home = tempfile::tempdir().expect("profile");
    std::fs::create_dir(home.path().join("Default")).expect("profile directory");
    std::fs::write(
        home.path().join("Default/Preferences"),
        br#"{"session":{"restore_on_startup":1},"test_marker":"preserve-me"}"#,
    )
    .expect("fixture preferences");
    let (pipe, mut child, supervisor) = headless(home.path());
    let seeded = (|| -> Result<(), AppError> {
        pipe.create_target("about:blank#previous-login")?;
        pipe.create_target("about:blank#previous-mail")?;
        // Synthetic cookie only, on a reserved test domain; no real browser data.
        pipe.request("Storage.setCookies",json!({"cookies":[{"name":"fixture-session","value":"not-a-login","domain":"example.test","path":"/"}]}))?;
        pipe.request("Browser.close", json!({}))?;
        wait_for_exit(&mut child)
    })();
    let _ = supervisor.terminate(&mut child);
    let _ = child.wait();
    seeded.expect("seed last session");
    super::super::startup::prepare_profile_startup(home.path()).expect("prepare managed startup");
    let (pipe, mut child, supervisor) = headless(home.path());
    let result = (|| -> Result<(), AppError> {
        pipe.create_target("about:blank#current-login")?;
        pipe.create_target("about:blank#current-mail")?;
        let targets = pipe.request("Target.getTargets", json!({}))?;
        let infos = targets["targetInfos"].as_array().expect("targets");
        assert!(
            !infos.iter().any(|info| info["url"]
                .as_str()
                .is_some_and(|url| url.contains("previous-"))),
            "previous login/mail tabs were restored: {:?}",
            infos
                .iter()
                .filter_map(|info| info["url"].as_str())
                .collect::<Vec<_>>()
        );
        let cookies = pipe.request("Storage.getCookies", json!({}))?;
        assert!(
            cookies["cookies"]
                .as_array()
                .expect("cookies")
                .iter()
                .any(|cookie| cookie["name"] == "fixture-session"),
            "synthetic session cookie was lost while preventing tab restoration"
        );
        Ok(())
    })();
    let _ = supervisor.terminate(&mut child);
    let _ = child.wait();
    result.expect("fresh operation targets");
}

#[test]
#[ignore = "requires installed Chromium; verifies live disposable profile is not edited"]
fn live_manual_profile_is_never_rewritten_to_change_startup_behavior() {
    let home = tempfile::tempdir().expect("profile");
    super::super::startup::prepare_profile_startup(home.path()).expect("initial startup");
    let (pipe, mut child, supervisor) = headless(home.path());
    let result = (|| -> Result<(), AppError> {
        pipe.create_target("about:blank#manual")?;
        let preferences = home.path().join("Default/Preferences");
        let before = std::fs::read(&preferences)?;
        let mut inventory = sysinfo::System::new();
        inventory.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::All,
            true,
            sysinfo::ProcessRefreshKind::nothing().with_cmd(sysinfo::UpdateKind::Always),
        );
        let process = inventory
            .process(sysinfo::Pid::from_u32(child.id()))
            .expect("live child");
        let matching = process
            .cmd()
            .iter()
            .filter(|argument| argument.to_string_lossy().contains("user-data-dir"))
            .count();
        let guarded = super::super::startup::prepare_profile_startup(home.path());
        assert!(matches!(guarded,Err(AppError::Busy(_))),"live profile guard failed; child argument count {}, profile flag count {}, outcome {:?}",process.cmd().len(),matching,guarded.as_ref().err().map(AppError::code));
        assert_eq!(std::fs::read(preferences)?, before);
        Ok(())
    })();
    let _ = supervisor.terminate(&mut child);
    let _ = child.wait();
    result.expect("live profile preserved");
}

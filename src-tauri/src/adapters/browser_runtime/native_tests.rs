use super::*;
use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

#[test]
#[ignore = "requires native graphical Chromium and the built CLI host; private profiles/local synthetic pages only, no OAuth/account"]
fn normal_browser_keeps_private_url_cookies_manual_tabs_and_repeated_operation_ownership() {
    let directory = tempfile::tempdir().expect("disposable native fixture");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("fixture endpoint");
    let port = listener.local_addr().expect("endpoint").port();
    listener.set_nonblocking(true).expect("bound");
    let observations = Arc::new(ParkingMutex::new(Vec::new()));
    let observed = observations.clone();
    let stopped = Arc::new(AtomicBool::new(false));
    let worker_stop = stopped.clone();
    let server = thread::spawn(move || {
        while !worker_stop.load(Ordering::Acquire) {
            let (mut stream, _) = match listener.accept() {
                Ok(stream) => stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(error) => panic!("fixture endpoint: {error}"),
            };
            // Winsock inherits the listener's nonblocking mode on accept;
            // Linux doesn't. This test server needs bounded blocking HTTP reads.
            stream
                .set_nonblocking(false)
                .expect("blocking fixture connection");
            stream
                .set_read_timeout(Some(Duration::from_secs(1)))
                .expect("read timeout");
            let mut reader = BufReader::new(stream.try_clone().expect("fixture stream"));
            let mut request = String::new();
            if reader.read_line(&mut request).unwrap_or(0) == 0 {
                continue; // Speculative browser preconnect, not an HTTP request.
            }
            let mut cookie = false;
            for _ in 0..32 {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                };
                if line.starts_with("Cookie:") && line.contains("cam_native_cookie=synthetic") {
                    cookie = true
                }
            }
            if request.starts_with("GET /observed?") {
                observed
                    .lock()
                    .push((request.contains("webdriver=false"), cookie));
            }
            let body = if request.starts_with("GET /seed?") || request.starts_with("GET /fixture?")
            {
                "<!doctype html><title>CAM native fixture</title><script>fetch('/observed?webdriver='+navigator.webdriver);</script>"
            } else {
                "<!doctype html><title>CAM quiet native fixture</title>Disposable fixture"
            };
            // Seed only once, not on the observation/read/reopen responses.
            // Otherwise every navigation would replace a lost session cookie
            // before the assertion could observe that it had been discarded.
            let set_cookie = if request.starts_with("GET /seed?") {
                "Set-Cookie: cam_native_cookie=synthetic; Path=/; SameSite=Lax\r\n"
            } else {
                ""
            };
            let _=write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n{set_cookie}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
        }
    });
    let _server_cleanup =
        scopeguard::guard((stopped.clone(), Some(server)), |(stopped, server)| {
            stopped.store(true, Ordering::Release);
            if let Some(server) = server {
                let joined = server.join();
                if !thread::panicking() {
                    joined.expect("collect fixture endpoint");
                }
            }
        });
    let bundle = std::env::var_os("CODEX_MANAGER_TEST_BUNDLE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from("/usr/lib/Gerenciador de contas do Codex/runtime/chromium")
        });
    let runtime = BrowserRuntime::with_bundled_runtime(directory.path(), Some(bundle));
    let _browser_cleanup = scopeguard::guard(runtime.clone(), |runtime| {
        let _ = runtime.shutdown_controlled();
    });
    let profile = directory.path().join("profile");
    let wait_observed = |minimum| {
        let deadline = Instant::now() + Duration::from_secs(6);
        while observations.lock().len() < minimum {
            if Instant::now() >= deadline {
                return Err(AppError::Browser(
                    "Synthetic browser response absent.".into(),
                ));
            }
            thread::sleep(Duration::from_millis(20));
        }
        Ok::<_, AppError>(())
    };
    let result = (|| {
        let seed = format!("http://127.0.0.1:{port}/seed?oauth=synthetic-private-parameter");
        let target = format!("http://127.0.0.1:{port}/fixture?oauth=synthetic-private-parameter");
        let first = runtime.launch_controlled(&profile, &seed, None)?;
        wait_observed(1)?;
        let argv = browser_command_line(first.pid()?)?;
        if argv.contains("synthetic-private-parameter") || argv.contains("remote-debugging") {
            return Err(AppError::Browser(
                "Browser startup exposed OAuth or debugging flags.".into(),
            ));
        }
        if !observations.lock().iter().all(|(normal, _)| *normal) {
            return Err(AppError::Browser(
                "Normal browser unexpectedly advertises WebDriver.".into(),
            ));
        }
        first.ensure_alive()?;
        runtime.open_tab(&profile, &format!("http://127.0.0.1:{port}/manual"))?;
        thread::sleep(Duration::from_millis(300));
        first.terminate()?;
        if !first.is_alive()? {
            return Err(AppError::Browser("Manual browser was closed.".into()));
        }
        if !matches!(first.ensure_alive(), Err(AppError::Cancelled)) {
            return Err(AppError::Browser(
                "Manual tab mistaken for live login.".into(),
            ));
        }
        let second = runtime.launch_controlled(
            &profile,
            &target,
            Some(&format!("http://127.0.0.1:{port}/mail")),
        )?;
        wait_observed(2)?;
        if !observations
            .lock()
            .iter()
            .all(|(normal, cookie)| *normal && *cookie)
        {
            return Err(AppError::Browser(
                "Normal identity or session cookie was lost.".into(),
            ));
        }
        first.terminate()?;
        second.ensure_alive()?;
        if !matches!(first.ensure_alive(), Err(AppError::Cancelled)) {
            return Err(AppError::Browser(
                "Closed operation was reactivated by a new tab.".into(),
            ));
        }
        second.terminate()?;
        if !second.is_alive()? {
            return Err(AppError::Browser(
                "Manual tab lost after second login.".into(),
            ));
        }
        let only = runtime.launch_controlled(
            &directory.path().join("only-operation"),
            &format!("http://127.0.0.1:{port}/quiet"),
            None,
        )?;
        only.terminate()?;
        if only.is_alive()? {
            return Err(AppError::Browser(
                "Only-operation browser was not collected.".into(),
            ));
        }
        runtime.shutdown_controlled()?;
        let before_reopen = observations.lock().len();
        let reopened = runtime.launch_controlled(&profile, &target, None)?;
        wait_observed(before_reopen + 1)?;
        if !observations.lock()[before_reopen..]
            .iter()
            .all(|(normal, cookie)| *normal && *cookie)
        {
            return Err(AppError::Browser(
                "Session cookie did not survive full browser restart.".into(),
            ));
        }
        reopened.terminate()?;
        Ok::<_, AppError>(())
    })();
    let cleanup = runtime.shutdown_controlled();
    result.expect("native non-automated browser lifecycle");
    cleanup.expect("collect own browser only");
}

fn browser_command_line(pid: u32) -> Result<String, AppError> {
    #[cfg(target_os = "linux")]
    return Ok(
        String::from_utf8_lossy(&std::fs::read(format!("/proc/{pid}/cmdline"))?).into_owned(),
    );
    #[cfg(windows)]
    {
        let mut system = sysinfo::System::new();
        system.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::All,
            true,
            sysinfo::ProcessRefreshKind::nothing().with_cmd(sysinfo::UpdateKind::Always),
        );
        let process = system
            .process(sysinfo::Pid::from_u32(pid))
            .ok_or_else(native_bridge::unavailable)?;
        Ok(process
            .cmd()
            .iter()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" "))
    }
}

#[test]
#[ignore = "requires native Chromium/CLI; delayed synthetic redirect and visibility only, no real OAuth/account"]
fn login_redirect_is_loaded_before_activation_and_cancellation_remains_owned() {
    let directory = tempfile::tempdir().expect("private browser presentation fixture");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("local fixture endpoint");
    let port = listener.local_addr().expect("local address").port();
    listener.set_nonblocking(true).expect("bound endpoint");
    let observed = Arc::new(ParkingMutex::new(Vec::<String>::new()));
    let events = observed.clone();
    let stopped = Arc::new(AtomicBool::new(false));
    let worker_stop = stopped.clone();
    let server = thread::spawn(move || {
        while !worker_stop.load(Ordering::Acquire) {
            let (mut stream, _) = match listener.accept() {
                Ok(pair) => pair,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(error) => panic!("fixture endpoint: {error}"),
            };
            stream
                .set_nonblocking(false)
                .expect("bounded blocking fixture connection");
            stream
                .set_read_timeout(Some(Duration::from_secs(1)))
                .expect("HTTP read deadline");
            let mut reader = BufReader::new(stream.try_clone().expect("fixture stream"));
            let mut request = String::new();
            if reader.read_line(&mut request).unwrap_or(0) == 0 {
                continue;
            }
            for _ in 0..32 {
                let mut header = String::new();
                if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
                    break;
                }
            }
            if request.starts_with("GET /observed?") {
                events.lock().push(request.clone());
            }
            if request.starts_with("GET /authorize?") {
                thread::sleep(Duration::from_millis(400));
                let _ = write!(stream,"HTTP/1.1 302 Found\r\nLocation: /login\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                continue;
            }
            let body = if request.starts_with("GET /login ") {
                "<!doctype html><title>Synthetic login</title><script>fetch('/observed?phase=arrival&visible='+(!document.hidden)+'&webdriver='+navigator.webdriver);document.addEventListener('visibilitychange',()=>{if(!document.hidden)fetch('/observed?phase=activated&visible=true');});</script>Local synthetic login"
            } else {
                "<!doctype html><title>Quiet synthetic fixture</title>Local fixture"
            };
            let _ = write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
        }
    });
    let _endpoint_cleanup = scopeguard::guard((stopped, Some(server)), |(stop, server)| {
        stop.store(true, Ordering::Release);
        if let Some(server) = server {
            let result = server.join();
            if !thread::panicking() {
                result.expect("collect own HTTP fixture");
            }
        }
    });
    let bundle = std::env::var_os("CODEX_MANAGER_TEST_BUNDLE")
        .map(PathBuf::from)
        .expect("explicit private native runtime fixture");
    let runtime = BrowserRuntime::with_bundled_runtime(directory.path(), Some(bundle));
    let _browser_cleanup = scopeguard::guard(runtime.clone(), |runtime| {
        let _ = runtime.shutdown_controlled();
    });
    let url = format!("http://127.0.0.1:{port}/authorize?private=synthetic-initial-link");
    let operation = runtime
        .launch_controlled(&directory.path().join("profile"), &url, None)
        .expect("private native operation");
    let deadline = Instant::now() + Duration::from_secs(6);
    loop {
        let events = observed.lock();
        if events.iter().any(|event| event.contains("phase=arrival")) {
            assert!(events.iter().any(|event|event.contains("phase=arrival&visible=false&webdriver=false")),
                "the initial redirect must load in a background tab, not expose its pending URL on the active tab");
        }
        if events
            .iter()
            .any(|event| event.contains("phase=activated&visible=true"))
        {
            break;
        }
        drop(events);
        assert!(
            Instant::now() < deadline,
            "resolved login did not become visible"
        );
        thread::sleep(Duration::from_millis(20));
    }
    operation
        .ensure_alive()
        .expect("same live operation after presentation");
    operation.terminate().expect("cancel only owned login");
    assert!(!operation.is_alive().expect("root collected"));
    runtime.shutdown_controlled().expect("collect own roots");
}

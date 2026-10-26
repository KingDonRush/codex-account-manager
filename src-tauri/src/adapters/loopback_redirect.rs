use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::error::AppError;

const REDIRECT_LIFETIME: Duration = Duration::from_secs(180);
const REDIRECT_DELIVERED_GRACE: Duration = Duration::from_secs(12);
const REDIRECT_READ_TIMEOUT: Duration = Duration::from_secs(10);
const REDIRECT_WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_REQUEST_LINE_BYTES: usize = 8 * 1024;
const MAX_CONNECTIONS: usize = 8;

const REDIRECT_ACTIVE: u8 = 0;
const REDIRECT_CLAIMED: u8 = 1;
const REDIRECT_DELIVERED: u8 = 2;
const REDIRECT_STOPPED: u8 = 3;

#[derive(Clone)]
pub(super) struct LoopbackRedirect {
    state: Arc<AtomicU8>,
}

impl LoopbackRedirect {
    pub(super) fn start(target: &str) -> Result<(String, Self), AppError> {
        let target = validated_target(target)?;
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let expected_path = format!("/open/{}", uuid::Uuid::new_v4().simple());
        let launch_url = format!("http://{address}{expected_path}");
        let state = Arc::new(AtomicU8::new(REDIRECT_ACTIVE));
        spawn_listener(
            listener,
            expected_path,
            target,
            launch_url.clone(),
            state.clone(),
        )?;

        Ok((launch_url, Self { state }))
    }

    pub(super) fn stop(&self) {
        self.state.store(REDIRECT_STOPPED, Ordering::Release);
    }

    pub(super) fn wait_until_delivered(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            match self.state.load(Ordering::Acquire) {
                REDIRECT_DELIVERED => return true,
                REDIRECT_STOPPED => return false,
                _ if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                _ => return false,
            }
        }
    }

    pub(super) fn is_delivered(&self) -> bool {
        self.state.load(Ordering::Acquire) == REDIRECT_DELIVERED
    }

    pub(super) fn can_retry(&self) -> bool {
        matches!(
            self.state.load(Ordering::Acquire),
            REDIRECT_ACTIVE | REDIRECT_CLAIMED
        )
    }
}

fn spawn_listener(
    listener: TcpListener,
    expected_path: String,
    target: String,
    retry_url: String,
    state: Arc<AtomicU8>,
) -> Result<(), AppError> {
    thread::Builder::new()
        .name("browser-url-bridge".to_owned())
        .spawn(move || serve(listener, expected_path, target, retry_url, state))
        .map(|_| ())
        .map_err(|error| AppError::Internal(format!("ponte local do navegador: {error}")))
}

fn validated_target(value: &str) -> Result<String, AppError> {
    if value.chars().any(char::is_control) {
        return Err(insecure_url_error());
    }
    let parsed = url::Url::parse(value)
        .map_err(|_| AppError::Browser("o navegador recebeu uma URL inválida".to_owned()))?;
    if !matches!(parsed.scheme(), "http" | "https")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(insecure_url_error());
    }
    Ok(parsed.to_string())
}

fn insecure_url_error() -> AppError {
    AppError::Browser("o navegador recebeu uma URL HTTP(S) insegura".to_owned())
}

fn serve(
    listener: TcpListener,
    expected_path: String,
    target: String,
    retry_url: String,
    state: Arc<AtomicU8>,
) {
    let deadline = Instant::now() + REDIRECT_LIFETIME;
    let connections = Arc::new(AtomicUsize::new(0));
    let mut delivered_deadline = None;
    while bridge_is_open(&state, deadline, &mut delivered_deadline) {
        match listener.accept() {
            Ok((stream, _)) => spawn_connection(
                stream,
                expected_path.clone(),
                target.clone(),
                retry_url.clone(),
                state.clone(),
                connections.clone(),
            ),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}

fn bridge_is_open(
    state: &AtomicU8,
    deadline: Instant,
    delivered_deadline: &mut Option<Instant>,
) -> bool {
    match state.load(Ordering::Acquire) {
        REDIRECT_STOPPED => false,
        REDIRECT_DELIVERED => {
            let grace_deadline =
                delivered_deadline.get_or_insert_with(|| Instant::now() + REDIRECT_DELIVERED_GRACE);
            Instant::now() < *grace_deadline
        }
        _ => Instant::now() < deadline,
    }
}

fn spawn_connection(
    stream: TcpStream,
    expected_path: String,
    target: String,
    retry_url: String,
    state: Arc<AtomicU8>,
    connections: Arc<AtomicUsize>,
) {
    if connections
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
            (count < MAX_CONNECTIONS).then_some(count + 1)
        })
        .is_err()
    {
        return;
    }
    let permit = ConnectionPermit(connections);
    let _ = thread::Builder::new()
        .name("browser-url-bridge-connection".to_owned())
        .spawn(move || {
            let _permit = permit;
            serve_connection(stream, &expected_path, &target, &retry_url, &state);
        });
}

struct ConnectionPermit(Arc<AtomicUsize>);
impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn serve_connection(
    mut stream: TcpStream,
    expected_path: &str,
    target: &str,
    retry_url: &str,
    state: &AtomicU8,
) {
    if !wait_until_available(state) {
        return;
    }
    if request_matches(&mut stream, expected_path, state) {
        if state.load(Ordering::Acquire) == REDIRECT_DELIVERED {
            let _ = write_redirect(&mut stream, target);
        } else {
            deliver_once(state, || write_redirect(&mut stream, target));
        }
        return;
    }
    if state.load(Ordering::Acquire) == REDIRECT_ACTIVE {
        let _ = write_redirect(&mut stream, retry_url);
    }
}

fn wait_until_available(state: &AtomicU8) -> bool {
    let deadline = Instant::now() + REDIRECT_WRITE_TIMEOUT;
    loop {
        match state.load(Ordering::Acquire) {
            REDIRECT_ACTIVE | REDIRECT_DELIVERED => return true,
            REDIRECT_STOPPED => return false,
            REDIRECT_CLAIMED if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(5));
            }
            _ => return false,
        }
    }
}

fn deliver_once(state: &AtomicU8, deliver: impl FnOnce() -> bool) {
    if state
        .compare_exchange(
            REDIRECT_ACTIVE,
            REDIRECT_CLAIMED,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_err()
    {
        return;
    }
    let next = if deliver() {
        REDIRECT_DELIVERED
    } else {
        // A speculative socket may close after sending its request line. The
        // real navigation still needs a chance to consume the bridge.
        REDIRECT_ACTIVE
    };
    // Cancellation may change CLAIMED to STOPPED while the socket write is in
    // progress. A compare-exchange keeps that terminal cancellation from
    // being overwritten by a late writer.
    let _ = state.compare_exchange(REDIRECT_CLAIMED, next, Ordering::AcqRel, Ordering::Acquire);
}

fn request_matches(stream: &mut TcpStream, expected_path: &str, state: &AtomicU8) -> bool {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
    let Some(request_target) = read_request_target(stream, state) else {
        return false;
    };
    normalize_request_target(&request_target).as_deref() == Some(expected_path)
}

fn read_request_target(stream: &mut TcpStream, state: &AtomicU8) -> Option<String> {
    // Reading only the request line prevents a speculative Chromium socket
    // from blocking the real navigation while still supporting TCP splits.
    let mut request_line = Vec::with_capacity(256);
    let deadline = Instant::now() + REDIRECT_READ_TIMEOUT;
    let mut chunk = [0_u8; 512];
    while state.load(Ordering::Acquire) != REDIRECT_STOPPED && Instant::now() < deadline {
        let count = match stream.read(&mut chunk) {
            Ok(0) => return None,
            Ok(count) => count,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) =>
            {
                continue
            }
            Err(_) => return None,
        };
        let end = chunk[..count]
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|end| end + 1);
        request_line.extend_from_slice(&chunk[..end.unwrap_or(count)]);
        if request_line.len() > MAX_REQUEST_LINE_BYTES {
            return None;
        }
        if end.is_some() {
            break;
        }
    }
    if !request_line.ends_with(b"\n") {
        return None;
    }
    let request = std::str::from_utf8(&request_line).ok()?;
    let mut fields = request.lines().next()?.split_whitespace();
    if fields.next()? != "GET" {
        return None;
    }
    Some(fields.next()?.to_owned())
}

fn normalize_request_target(target: &str) -> Option<String> {
    if let Ok(parsed) = url::Url::parse(target) {
        if !matches!(parsed.scheme(), "http" | "https") {
            return None;
        }
        return Some(normalize_path(parsed.path()));
    }
    Some(normalize_path(target.split('?').next()?))
}

fn normalize_path(path: &str) -> String {
    let normalized = path.trim_end_matches('/');
    if normalized.is_empty() {
        "/".to_owned()
    } else {
        normalized.to_owned()
    }
}

fn write_redirect(stream: &mut TcpStream, target: &str) -> bool {
    let _ = stream.set_write_timeout(Some(REDIRECT_WRITE_TIMEOUT));
    let response = format!(
        "HTTP/1.1 302 Found\r\nLocation: {target}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(response.as_bytes()).is_ok() && stream.flush().is_ok()
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};

    use super::{LoopbackRedirect, REDIRECT_ACTIVE, REDIRECT_DELIVERED};

    #[test]
    fn cancelling_interrupts_an_incomplete_http_request() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("listener");
        let _peer =
            std::net::TcpStream::connect(listener.local_addr().expect("address")).expect("peer");
        let (mut stream, _) = listener.accept().expect("accepted");
        stream
            .set_read_timeout(Some(std::time::Duration::from_millis(30)))
            .expect("timeout");
        let state = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(REDIRECT_ACTIVE));
        let worker_state = state.clone();
        let reader =
            std::thread::spawn(move || super::read_request_target(&mut stream, &worker_state));
        std::thread::sleep(std::time::Duration::from_millis(20));
        let started = std::time::Instant::now();
        state.store(
            super::REDIRECT_STOPPED,
            std::sync::atomic::Ordering::Release,
        );
        assert!(reader.join().expect("reader").is_none());
        assert!(started.elapsed() < std::time::Duration::from_millis(150));
    }

    #[test]
    fn split_request_delivers_target_without_exposing_it_in_launch_url() {
        let target = "https://login.example.test/oauth?code=secret&state=private";
        let (launch_url, redirect) = LoopbackRedirect::start(target).expect("redirect");
        assert!(launch_url.starts_with("http://127.0.0.1:"));
        assert!(!launch_url.contains("secret"));

        let parsed = url::Url::parse(&launch_url).expect("launch URL");
        let address = format!("127.0.0.1:{}", parsed.port().expect("port"));
        let mut stream = std::net::TcpStream::connect(address).expect("loopback connection");
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .expect("read timeout");
        write!(stream, "GET {}", parsed.path()).expect("request prefix");
        std::thread::sleep(std::time::Duration::from_millis(25));
        stream
            .write_all(b" HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
            .expect("request suffix");
        let response = read_response(&mut stream);

        assert!(response.starts_with("HTTP/1.1 302 Found"));
        assert!(response.contains(&format!("Location: {target}\r\n")));
        redirect.stop();
    }

    #[test]
    fn speculative_connection_does_not_delay_real_navigation() {
        let target = "https://login.example.test/oauth?state=private";
        let (launch_url, redirect) = LoopbackRedirect::start(target).expect("redirect");
        let parsed = url::Url::parse(&launch_url).expect("launch URL");
        let address = format!("127.0.0.1:{}", parsed.port().expect("port"));
        let _speculative = std::net::TcpStream::connect(&address).expect("speculative connection");
        std::thread::sleep(std::time::Duration::from_millis(25));

        let started = std::time::Instant::now();
        let response = request(&address, parsed.path());

        assert!(response.starts_with("HTTP/1.1 302 Found"));
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        redirect.stop();
    }

    #[test]
    fn failed_delivery_does_not_consume_bridge() {
        let state = std::sync::atomic::AtomicU8::new(REDIRECT_ACTIVE);

        super::deliver_once(&state, || false);
        assert_eq!(
            state.load(std::sync::atomic::Ordering::Acquire),
            REDIRECT_ACTIVE
        );

        super::deliver_once(&state, || true);
        assert_eq!(
            state.load(std::sync::atomic::Ordering::Acquire),
            REDIRECT_DELIVERED
        );
    }

    #[test]
    fn cancellation_wins_over_an_in_flight_delivery() {
        let state = std::sync::atomic::AtomicU8::new(REDIRECT_ACTIVE);

        super::deliver_once(&state, || {
            state.store(
                super::REDIRECT_STOPPED,
                std::sync::atomic::Ordering::Release,
            );
            true
        });

        assert_eq!(
            state.load(std::sync::atomic::Ordering::Acquire),
            super::REDIRECT_STOPPED
        );
    }

    #[test]
    fn malformed_probe_retries_canonical_bridge_path() {
        let target = "https://login.example.test/oauth?state=private";
        let (launch_url, redirect) = LoopbackRedirect::start(target).expect("redirect");
        let parsed = url::Url::parse(&launch_url).expect("launch URL");
        let address = format!("127.0.0.1:{}", parsed.port().expect("port"));

        let probe_response = request(&address, "/favicon.ico");
        assert!(probe_response.starts_with("HTTP/1.1 302 Found"));
        assert!(probe_response.contains(&format!("Location: {launch_url}\r\n")));

        let navigation_response = request(&address, parsed.path());
        assert!(navigation_response.starts_with("HTTP/1.1 302 Found"));
        assert!(navigation_response.contains(&format!("Location: {target}\r\n")));
        redirect.stop();
    }

    #[test]
    fn delivered_bridge_replays_target_for_a_short_startup_grace() {
        let target = "https://login.example.test/oauth?state=private";
        let (launch_url, redirect) = LoopbackRedirect::start(target).expect("redirect");
        let parsed = url::Url::parse(&launch_url).expect("launch URL");
        let address = format!("127.0.0.1:{}", parsed.port().expect("port"));

        let first = request(&address, parsed.path());
        assert!(first.contains(&format!("Location: {target}\r\n")));
        let second = request(&address, parsed.path());

        assert!(second.starts_with("HTTP/1.1 302 Found"));
        assert!(second.contains(&format!("Location: {target}\r\n")));
        redirect.stop();
        assert_eq!(
            redirect.state.load(std::sync::atomic::Ordering::Acquire),
            super::REDIRECT_STOPPED
        );
    }

    #[test]
    #[ignore = "requires a Chromium executable in CODEX_MANAGER_CHROMIUM"]
    fn installed_chromium_follows_bridge() {
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};

        let executable = std::env::var_os("CODEX_MANAGER_CHROMIUM")
            .map(std::path::PathBuf::from)
            .filter(|path| path.is_file())
            .expect("CODEX_MANAGER_CHROMIUM");
        let target_listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("target");
        target_listener
            .set_nonblocking(true)
            .expect("nonblocking target");
        let target_url = format!(
            "http://{}/result",
            target_listener.local_addr().expect("addr")
        );
        let (arrived_sender, arrived) = std::sync::mpsc::sync_channel(1);
        let target_worker = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            while Instant::now() < deadline {
                match target_listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                        let mut request = [0_u8; 2048];
                        let _ = stream.read(&mut request);
                        let body = b"<html><body>bridge-arrived</body></html>";
                        write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .expect("target headers");
                        stream.write_all(body).expect("target body");
                        arrived_sender.send(()).expect("arrival signal");
                        return;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("target accept: {error}"),
                }
            }
            panic!("Chromium did not reach the redirect target");
        });
        let (launch_url, redirect) = LoopbackRedirect::start(&target_url).expect("redirect");
        let profile = tempfile::tempdir().expect("browser profile");
        let mut command = Command::new(executable);
        crate::adapters::process_supervisor::configure_supervised_process(&mut command);
        command
            .arg("--headless=new")
            .arg("--no-sandbox")
            .arg("--disable-gpu")
            .arg("--disable-background-networking")
            .arg("--no-first-run")
            .arg(format!("--user-data-dir={}", profile.path().display()))
            .arg(&launch_url)
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = command.spawn().expect("Chromium");
        let supervisor = crate::adapters::process_supervisor::ProcessSupervisor::attach(&child)
            .expect("browser supervisor");
        let deadline = Instant::now() + Duration::from_secs(15);
        let reached_target = loop {
            if arrived.try_recv().is_ok() {
                break true;
            }
            if child.try_wait().expect("browser status").is_some() {
                break false;
            }
            if Instant::now() >= deadline {
                break false;
            }
            std::thread::sleep(Duration::from_millis(25));
        };
        let _ = supervisor.terminate(&mut child);
        let output = child.wait_with_output().expect("browser output");
        target_worker.join().expect("target worker");
        redirect.stop();

        if !reached_target {
            let stderr = String::from_utf8_lossy(&output.stderr);
            panic!(
                "Chromium did not reach the loopback target: {}",
                stderr.chars().take(2_000).collect::<String>()
            );
        }
    }

    fn request(address: &str, path: &str) -> String {
        let mut stream = std::net::TcpStream::connect(address).expect("connection");
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .expect("read timeout");
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
        )
        .expect("request");
        read_response(&mut stream)
    }

    fn read_response(stream: &mut std::net::TcpStream) -> String {
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("response");
        response
    }
}

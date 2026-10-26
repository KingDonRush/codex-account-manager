use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use windows::core::w;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, PostMessageW, RegisterClassW,
    ShowWindow, MSG, SW_HIDE, WINDOW_EX_STYLE, WM_CLOSE, WM_ENDSESSION, WM_QUERYENDSESSION,
    WNDCLASSW, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};

use super::*;

struct Fixture(Child);

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn window_fixture(mode: &str) -> (Fixture, HWND) {
    use std::os::windows::process::CommandExt;
    let mut child = Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            "adapters::desktop_control::restart_manager::tests::owned_window_fixture",
            "--ignored",
            "--nocapture",
        ])
        .env("CAM_QUIT_FIXTURE", mode)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000)
        .spawn()
        .expect("own fixture");
    let output = child.stdout.take().expect("output");
    let mut fixture = Fixture(child);
    let (ready, receive) = mpsc::sync_channel(1);
    let reader = thread::spawn(move || {
        for line in BufReader::new(output).lines().map_while(Result::ok) {
            if let Some(value) = line.strip_prefix("fixture_window:") {
                let _ = ready.send(value.parse::<usize>().expect("own handle"));
                return;
            }
        }
    });
    let handle = receive.recv_timeout(Duration::from_secs(5));
    if handle.is_err() {
        let _ = fixture.0.kill();
    }
    reader.join().expect("reader");
    (fixture, HWND(handle.expect("fixture ready") as *mut _))
}

fn identity(child: &Child) -> RM_UNIQUE_PROCESS {
    target(
        child.id(),
        Some(native::created_ticks(child.id()).expect("own creation")),
    )
    .expect("target")
}

#[test]
fn quit_exits_a_tray_style_app_while_a_window_close_does_not() {
    let (mut fixture, window) = window_fixture("accept");
    let (mut neighbor, _) = window_fixture("accept");
    // SAFETY: the fixture publishes only its own handle; no user windows.
    unsafe { PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0)) }.expect("close");
    thread::sleep(Duration::from_millis(100));
    assert!(
        fixture.0.try_wait().expect("status").is_none(),
        "WM_CLOSE only hides this app"
    );
    request_shutdown(&[identity(&fixture.0)]).expect("application quit");
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = fixture.0.try_wait().expect("status") {
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < until, "graceful app exit was not observed");
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        neighbor.0.try_wait().expect("neighbor").is_none(),
        "another app must remain alive"
    );
}

#[test]
fn rejecting_normal_quit_is_not_forced_or_reported_as_success() {
    let (mut fixture, _) = window_fixture("reject");
    assert!(request_shutdown(&[identity(&fixture.0)]).is_err());
    assert!(fixture.0.try_wait().expect("status").is_none());
}

#[test]
fn recycled_process_and_additional_targets_are_not_authorized() {
    let (mut fixture, _) = window_fixture("accept");
    let mut stale = identity(&fixture.0);
    stale.ProcessStartTime.dwLowDateTime += 1;
    assert!(request_shutdown(&[stale]).is_err());
    assert!(fixture.0.try_wait().expect("status").is_none());
    let target = RM_UNIQUE_PROCESS {
        dwProcessId: 12,
        ProcessStartTime: FILETIME {
            dwLowDateTime: 7,
            dwHighDateTime: 0,
        },
    };
    let other = RM_UNIQUE_PROCESS {
        dwProcessId: 13,
        ..target
    };
    assert!(ensure_expected(&[target], &[target, other]).is_err());
}

#[test]
#[ignore = "child fixture, invoked only by the native shutdown tests with an exact private mode"]
fn owned_window_fixture() {
    assert!(matches!(
        std::env::var("CAM_QUIT_FIXTURE").as_deref(),
        Ok("accept" | "reject")
    ));
    // SAFETY: test process owns this class/window and its message loop. The
    // fixture never opens credentials, files, network, or user applications.
    unsafe {
        let module = GetModuleHandleW(None).expect("module");
        let class = WNDCLASSW {
            lpfnWndProc: Some(fixture_window),
            hInstance: HINSTANCE(module.0),
            lpszClassName: w!("CodexManagerQuitFixture"),
            ..Default::default()
        };
        assert_ne!(RegisterClassW(&class), 0);
        let window = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class.lpszClassName,
            w!("Isolated shutdown test"),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            0,
            0,
            240,
            120,
            None,
            None,
            Some(class.hInstance),
            None,
        )
        .expect("window");
        println!("fixture_window:{}", window.0 as usize);
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).0 > 0 {
            DispatchMessageW(&message);
        }
    }
}

unsafe extern "system" fn fixture_window(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_CLOSE => {
            let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
            LRESULT(0)
        }
        WM_QUERYENDSESSION => LRESULT(isize::from(
            std::env::var("CAM_QUIT_FIXTURE").as_deref() == Ok("accept"),
        )),
        WM_ENDSESSION if wparam.0 != 0 => std::process::exit(0),
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

use crate::{adapters::logging, error::AppError};
use serde_json::Value;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::process::ChildStdin;
#[cfg(test)]
use std::sync::mpsc;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{SyncSender, TrySendError},
    Arc, Mutex,
};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;

const MAX_FRAME_BYTES: usize = 256 * 1024;
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
type Tail = Arc<Mutex<VecDeque<String>>>;
/// The client serializes writes; Tokio owns pipe I/O, not a per-client worker.
/// Cancellation/timeout may leave a partial frame, so callers discard the client.
pub(super) struct FrameWriter {
    stdin: tokio::process::ChildStdin,
}

impl FrameWriter {
    pub(super) fn new(stdin: ChildStdin) -> Result<Self, AppError> {
        let stdin =
            tauri::async_runtime::block_on(async { tokio::process::ChildStdin::from_std(stdin) })?;
        Ok(Self { stdin })
    }

    pub(super) fn send(&mut self, value: &Value, cancel: &AtomicBool) -> Result<(), AppError> {
        if cancel.load(Ordering::Acquire) {
            return Err(AppError::Cancelled);
        }
        let mut bytes = serde_json::to_vec(value)?;
        if bytes.len() > MAX_FRAME_BYTES {
            return Err(AppError::AppServer(
                "Mensagem excedeu o limite local de transporte.".to_owned(),
            ));
        }
        bytes.push(b'\n');
        tauri::async_runtime::block_on(write_cancellable(&mut self.stdin, &bytes, cancel))
    }
}

async fn write_cancellable(
    stdin: &mut tokio::process::ChildStdin,
    bytes: &[u8],
    cancel: &AtomicBool,
) -> Result<(), AppError> {
    let deadline = Instant::now() + WRITE_TIMEOUT;
    let mut write = std::pin::pin!(async {
        stdin.write_all(bytes).await?;
        stdin.flush().await
    });
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(AppError::Cancelled);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(AppError::AppServer(
                "O App Server não aceitou a mensagem no prazo.".to_owned(),
            ));
        }
        if let Ok(result) =
            tokio::time::timeout(remaining.min(Duration::from_millis(25)), write.as_mut()).await
        {
            return result.map_err(AppError::Io);
        }
    }
}

fn frame(reader: &mut impl BufRead) -> std::io::Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    if reader
        .take((MAX_FRAME_BYTES + 1) as u64)
        .read_until(b'\n', &mut bytes)?
        == 0
    {
        return Ok(None);
    }
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(std::io::Error::other("Frame excedeu o limite local."));
    }
    Ok(Some(bytes))
}

pub(super) fn read_messages(stdout: impl Read, sender: SyncSender<Value>, tail: Tail) {
    let mut reader = BufReader::new(stdout);
    while let Ok(Some(bytes)) = frame(&mut reader) {
        if bytes.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let Ok(value) = serde_json::from_slice(&bytes) else {
            note(&tail, "Resposta JSONL inválida; transporte encerrado.");
            return;
        };
        match sender.try_send(value) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                note(
                    &tail,
                    "Fila de respostas cheia; transporte encerrado para preservar memória.",
                );
                return;
            }
            Err(TrySendError::Disconnected(_)) => return,
        }
    }
    note(&tail, "Leitor encerrou ou recebeu um frame excessivo.");
}

pub(super) fn read_stderr(stderr: impl Read, tail: Tail) {
    logging::read_stderr_tail(stderr, tail, 20, logging::sanitize_error);
}

fn note(tail: &Tail, message: &str) {
    if let Ok(mut lines) = tail.lock() {
        if lines.len() == 20 {
            lines.pop_front();
        }
        lines.push_back(message.chars().take(1600).collect());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oversized_and_invalid_frames_fail_closed() {
        let mut reader = BufReader::new(std::io::Cursor::new(vec![b'x'; MAX_FRAME_BYTES + 1]));
        assert!(frame(&mut reader).is_err());
        let (sender, receiver) = mpsc::sync_channel(2);
        let tail = Arc::new(Mutex::new(VecDeque::new()));
        read_messages("not-json secret-input\n".as_bytes(), sender, tail.clone());
        assert!(receiver.recv().is_err());
        assert!(!tail
            .lock()
            .expect("tail")
            .back()
            .expect("error")
            .contains("secret-input"));
    }
    #[test]
    fn response_queue_has_a_hard_bound() {
        let (sender, receiver) = mpsc::sync_channel(2);
        let tail = Arc::new(Mutex::new(VecDeque::new()));
        read_messages("{}\n{}\n{}\n".as_bytes(), sender, tail.clone());
        assert_eq!(receiver.try_iter().count(), 2);
        assert!(tail
            .lock()
            .expect("tail")
            .back()
            .expect("error")
            .contains("cheia"));
    }

    #[test]
    fn frame_limit_counts_the_delimiter_and_accepts_a_final_frame_without_it() {
        let mut exact = vec![b' '; MAX_FRAME_BYTES - 3];
        exact.extend_from_slice(b"{}\n");
        let mut reader = BufReader::new(std::io::Cursor::new(exact.clone()));
        assert_eq!(frame(&mut reader).expect("exact frame"), Some(exact));
        assert!(frame(&mut reader).expect("EOF").is_none());

        let mut oversized = vec![b' '; MAX_FRAME_BYTES - 2];
        oversized.extend_from_slice(b"{}\n");
        assert!(frame(&mut BufReader::new(std::io::Cursor::new(oversized))).is_err());

        let (sender, receiver) = mpsc::sync_channel(2);
        let tail = Arc::new(Mutex::new(VecDeque::new()));
        read_messages(b"{}\n{\"id\":2}".as_slice(), sender, tail);
        assert_eq!(
            receiver.try_iter().collect::<Vec<_>>(),
            vec![serde_json::json!({}), serde_json::json!({"id":2}),]
        );
    }

    #[test]
    fn invalid_input_stops_the_reader_instead_of_resuming_after_the_bad_frame() {
        let (sender, receiver) = mpsc::sync_channel(4);
        let tail = Arc::new(Mutex::new(VecDeque::new()));
        read_messages(
            b"{}\ninvalid\n{\"id\":2}\n".as_slice(),
            sender,
            tail.clone(),
        );
        assert_eq!(
            receiver.try_iter().collect::<Vec<_>>(),
            vec![serde_json::json!({})]
        );
        let lines = tail.lock().expect("tail");
        assert!(lines.back().expect("failure").contains("JSONL inválida"));
        assert!(!lines.back().expect("failure").contains("invalid\n"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn blocked_stdin_remains_cancelable() {
        let mut command = std::process::Command::new("sh");
        crate::adapters::process_supervisor::configure_supervised_process(&mut command);
        command
            .args(["-c", "sleep 30 & wait"])
            .stdin(std::process::Stdio::piped());
        let mut child = command.spawn().expect("owned fixture");
        let supervisor = crate::adapters::process_supervisor::ProcessSupervisor::attach(&child)
            .expect("supervisor");
        let mut writer = FrameWriter::new(child.stdin.take().expect("stdin")).expect("writer");
        let cancel = Arc::new(AtomicBool::new(false));
        let other = cancel.clone();
        let cancellation = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(80));
            other.store(true, Ordering::Release);
        });
        let started = Instant::now();
        let result = writer.send(
            &serde_json::json!({"payload":"x".repeat(128*1024)}),
            &cancel,
        );
        supervisor.terminate(&mut child).expect("cleanup");
        let _ = child.wait();
        cancellation.join().expect("cancellation");
        assert!(matches!(result, Err(AppError::Cancelled)));
        assert!(started.elapsed() < Duration::from_millis(600));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_slow_pipe_does_not_restart_partial_writes_at_each_cancellation_slice() {
        let mut command = std::process::Command::new("sh");
        command
            .args([
                "-c",
                "sleep 0.15; IFS= read -r frame; printf '%s' \"$frame\" | wc -c",
            ])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped());
        let (mut child, supervisor) =
            crate::adapters::process_launcher::spawn_supervised(command).expect("owned fixture");
        let stdout = child.stdout.take().expect("stdout");
        let mut writer = FrameWriter::new(child.stdin.take().expect("stdin")).expect("writer");
        let value = serde_json::json!({"payload":"x".repeat(128*1024)});
        let expected = serde_json::to_vec(&value).expect("JSON").len();
        let result = writer.send(&value, &AtomicBool::new(false));
        let mut count = String::new();
        let received = if result.is_ok() {
            BufReader::new(stdout).read_line(&mut count)
        } else {
            Ok(0)
        };
        supervisor.terminate(&mut child).expect("cleanup");
        let _ = child.wait();
        result.expect("one complete frame after backpressure");
        received.expect("fixture output");
        assert_eq!(count.trim().parse::<usize>().expect("byte count"), expected);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_blocked_pipe_has_a_deadline_even_without_a_cancellation_signal() {
        let mut command = std::process::Command::new("sh");
        command
            .args(["-c", "sleep 30 & wait"])
            .stdin(std::process::Stdio::piped());
        let (mut child, supervisor) =
            crate::adapters::process_launcher::spawn_supervised(command).expect("owned fixture");
        let mut writer = FrameWriter::new(child.stdin.take().expect("stdin")).expect("writer");
        let started = Instant::now();
        let result = writer.send(
            &serde_json::json!({"payload":"x".repeat(128*1024)}),
            &AtomicBool::new(false),
        );
        supervisor.terminate(&mut child).expect("cleanup");
        let _ = child.wait();
        assert!(matches!(result, Err(AppError::AppServer(_))));
        assert!(started.elapsed() >= WRITE_TIMEOUT);
        assert!(started.elapsed() < WRITE_TIMEOUT + Duration::from_secs(1));
    }
}

use crate::error::AppError;
use parking_lot::{Mutex, MutexGuard};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Serialize commits without blocking a cancelled operation behind another worker.
pub fn acquire<'a, T>(
    gate: &'a Mutex<T>,
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<MutexGuard<'a, T>, AppError> {
    let deadline = Instant::now() + timeout;
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(AppError::Cancelled);
        }
        if let Some(lease) = gate.try_lock_for(Duration::from_millis(25)) {
            if cancel.load(Ordering::Acquire) {
                return Err(AppError::Cancelled);
            }
            return Ok(lease);
        }
        if Instant::now() >= deadline {
            return Err(AppError::Busy(
                "Outra operação ainda está finalizando. Sua conta não foi alterada.".to_owned(),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    #[test]
    fn cancellation_is_checked_while_a_commit_is_busy() {
        let gate = Mutex::new(());
        let _owner = gate.lock();
        let cancel = AtomicBool::new(true);
        let started = Instant::now();
        assert!(matches!(
            acquire(&gate, &cancel, Duration::from_secs(30)),
            Err(AppError::Cancelled)
        ));
        assert!(started.elapsed() < Duration::from_millis(100));
    }
    #[test]
    fn waiting_for_another_commit_has_a_deadline() {
        let gate = Mutex::new(());
        let _owner = gate.lock();
        assert!(matches!(
            acquire(&gate, &AtomicBool::new(false), Duration::from_millis(25)),
            Err(AppError::Busy(_))
        ));
    }

    #[test]
    fn cancellation_while_lock_is_waiting_wins_over_the_newly_available_lease() {
        let gate = Arc::new(Mutex::new(()));
        let owner = gate.lock();
        let cancel = Arc::new(AtomicBool::new(false));
        let next_gate = gate.clone();
        let next_cancel = cancel.clone();
        let (entered, started) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            entered.send(()).expect("waiter signal");
            let result = acquire(&next_gate, &next_cancel, Duration::from_secs(1));
            matches!(result, Err(AppError::Cancelled))
        });
        started.recv().expect("waiter admitted");
        std::thread::sleep(Duration::from_millis(10));
        cancel.store(true, Ordering::Release);
        drop(owner);
        assert!(
            worker.join().expect("worker"),
            "a cancelled waiter cannot enter the next operation"
        );
    }
}

use crate::error::AppError;
use parking_lot::{Condvar, Mutex};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Default)]
struct State {
    closing: bool,
    running: usize,
}

/// Counts owned work until its cleanup is complete, including unwinding.
#[derive(Default)]
pub(super) struct TaskTracker {
    state: Mutex<State>,
    drained: Condvar,
}

pub(super) struct TaskLease(Arc<TaskTracker>);

impl TaskTracker {
    pub(super) fn enter(self: &Arc<Self>) -> Result<TaskLease, AppError> {
        let mut state = self.state.lock();
        if state.closing {
            return Err(AppError::Cancelled);
        }
        state.running += 1;
        Ok(TaskLease(self.clone()))
    }

    pub(super) fn close(&self) {
        self.state.lock().closing = true;
    }

    pub(super) fn wait_drained(&self, timeout: Duration) -> Result<(), AppError> {
        let deadline = Instant::now() + timeout;
        let mut state = self.state.lock();
        while state.running != 0 {
            if Instant::now() >= deadline {
                return Err(AppError::Busy("O aplicativo ainda está concluindo uma operação. Ele permanecerá aberto para preservar a sessão; tente fechar novamente em instantes.".to_owned()));
            }
            self.drained.wait_for(
                &mut state,
                deadline.saturating_duration_since(Instant::now()),
            );
        }
        Ok(())
    }
}

impl Drop for TaskLease {
    fn drop(&mut self) {
        self.0.state.lock().running -= 1;
        self.0.drained.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_rejects_new_work_but_waits_for_existing_commit() {
        let tracker = Arc::new(TaskTracker::default());
        let commit = tracker.enter().expect("commit");
        tracker.close();
        assert!(tracker.enter().is_err());
        assert!(tracker.wait_drained(Duration::from_millis(1)).is_err());
        drop(commit);
        assert!(tracker.wait_drained(Duration::from_millis(1)).is_ok());
    }

    #[test]
    fn unwinding_releases_owned_work() {
        let tracker = Arc::new(TaskTracker::default());
        let other = tracker.clone();
        let _ = std::thread::spawn(move || {
            let _lease = other.enter().expect("lease");
            panic!("fixture");
        })
        .join();
        tracker.close();
        assert!(tracker.wait_drained(Duration::from_millis(1)).is_ok());
    }
}

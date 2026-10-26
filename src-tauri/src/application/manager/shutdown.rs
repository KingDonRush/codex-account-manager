use super::ManagerCore;
use crate::error::AppError;
use std::sync::atomic::Ordering;
use std::time::Duration;

impl ManagerCore {
    pub fn claim_shutdown(&self) -> bool {
        !self.shutdown_running.swap(true, Ordering::AcqRel)
    }

    pub fn is_exit_ready(&self) -> bool {
        self.exit_ready.load(Ordering::Acquire)
    }

    /// Cancel preparatory work; committing workers retain ownership until their
    /// compensation/cleanup finishes. A deadline never authorizes forced exit.
    pub fn shutdown(&self) -> Result<(), AppError> {
        self.tasks.close();
        *self.maintenance.stopped.lock() = true;
        self.maintenance.cancel.store(true, Ordering::Release);
        self.maintenance.wake.notify_all();
        self.usage_access.lock().clear();
        self.executions.cancel_matching(|_| true);
        self.application_servers.close_matching(|_| true);
        self.cancel_preparatory_operations();
        let result = self
            .tasks
            .wait_drained(Duration::from_secs(45))
            .and_then(|()| self.close_owned_browsers());
        if result.is_ok() {
            self.application_credentials.close();
            self.credentials.close();
        }
        self.exit_ready.store(result.is_ok(), Ordering::Release);
        self.shutdown_running.store(false, Ordering::Release);
        result
    }

    fn cancel_preparatory_operations(&self) {
        let controls = self
            .operations
            .lock()
            .controls
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for control in controls {
            let _gate = control.cancellation_gate.lock();
            if control.cancelable.load(Ordering::Acquire) {
                control.cancel.store(true, Ordering::Release);
            }
        }
    }

    fn close_owned_browsers(&self) -> Result<(), AppError> {
        // App exit owns the managed processes, unlike operation cancellation,
        // which only owns its targets. Stop controlled roots before CDP cleanup.
        self.browser.shutdown_controlled()?;
        let retained = self
            .retained_login_browsers
            .lock()
            .values()
            .map(|browser| browser.handle.clone())
            .collect::<Vec<_>>();
        for browser in retained {
            browser.terminate()?;
        }
        let browsers = self
            .browser_sessions
            .lock()
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for browser in browsers {
            browser.terminate()?;
        }
        self.retained_login_browsers.lock().clear();
        self.browser_sessions.lock().clear();
        Ok(())
    }

    #[cfg(test)]
    pub fn cancel_all(&self) {
        self.shutdown().expect("owned workers must finish");
    }
}

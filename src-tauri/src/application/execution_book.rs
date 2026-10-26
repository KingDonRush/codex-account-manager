use crate::domain::execution::{ExecutionState, ExecutionStatus};
use crate::error::AppError;
#[cfg(test)]
mod tests;
use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

pub struct ExecutionControl {
    pub status: ExecutionStatus,
    pub cancel: Arc<AtomicBool>,
    accounts: std::collections::BTreeSet<String>,
}

#[derive(Default)]
pub struct ExecutionBook {
    inner: Mutex<Book>,
}
#[derive(Default)]
struct Book {
    jobs: HashMap<String, ExecutionControl>,
    queue: VecDeque<String>,
    account_usage: HashMap<String, u32>,
    sessions: HashMap<String, String>,
    last_application: HashMap<String, String>,
}
pub struct GenerationLease {
    book: Arc<ExecutionBook>,
    account: String,
    session: String,
}

impl ExecutionBook {
    pub fn restore(&self, status: ExecutionStatus) {
        self.inner.lock().jobs.insert(
            status.id.clone(),
            ExecutionControl {
                status,
                cancel: Arc::new(AtomicBool::new(false)),
                accounts: std::collections::BTreeSet::new(),
            },
        );
    }
    pub fn add(
        &self,
        status: ExecutionStatus,
        cancel: Arc<AtomicBool>,
        accounts: std::collections::BTreeSet<String>,
    ) -> Result<(), AppError> {
        let mut book = self.inner.lock();
        if book
            .jobs
            .values()
            .filter(|job| !job.status.state.terminal())
            .count()
            >= 64
        {
            return Err(AppError::Busy("Fila de trabalhos cheia.".to_owned()));
        }
        book.queue.push_back(status.id.clone());
        book.jobs.insert(
            status.id.clone(),
            ExecutionControl {
                status,
                cancel,
                accounts,
            },
        );
        Ok(())
    }
    pub fn list(&self) -> Vec<ExecutionStatus> {
        self.inner
            .lock()
            .jobs
            .values()
            .map(|job| job.status.clone())
            .collect()
    }
    pub fn status(&self, id: &str) -> Result<ExecutionStatus, AppError> {
        self.inner
            .lock()
            .jobs
            .get(id)
            .map(|job| job.status.clone())
            .ok_or_else(|| AppError::OperationNotFound(id.to_owned()))
    }
    pub fn update(&self, status: ExecutionStatus) {
        let mut book = self.inner.lock();
        if status.state.terminal() {
            book.queue.retain(|id| id != &status.id);
        }
        if let Some(job) = book.jobs.get_mut(&status.id) {
            job.status = status;
        }
    }
    /// A failed admission must not leave a queue entry without a worker owner.
    pub fn fail_submission(&self, id: &str, error: &AppError) {
        let mut book = self.inner.lock();
        if let Some(job) = book.jobs.get_mut(id) {
            if job.status.state != ExecutionState::Queued {
                return;
            }
            job.status.state = ExecutionState::Failed;
            job.status.finished_at = Some(chrono::Utc::now());
            job.status.error_code = Some(error.code().to_owned());
        }
        book.queue.retain(|queued| queued != id);
    }
    pub fn cancel(&self, id: &str) -> Result<(), AppError> {
        let mut book = self.inner.lock();
        let job = book
            .jobs
            .get_mut(id)
            .ok_or_else(|| AppError::OperationNotFound(id.to_owned()))?;
        if !job.status.state.terminal() {
            job.cancel.store(true, Ordering::Release);
            job.status.state = ExecutionState::Cancelling;
        }
        Ok(())
    }
    pub fn cancel_matching(&self, matches: impl Fn(&ExecutionStatus) -> bool) {
        for job in self.inner.lock().jobs.values_mut() {
            if matches(&job.status) && !job.status.state.terminal() {
                job.cancel.store(true, Ordering::Release);
                job.status.state = ExecutionState::Cancelling;
            }
        }
    }
    pub fn session_busy(&self, session: &str) -> bool {
        self.inner.lock().sessions.contains_key(session)
    }
    pub fn account_busy(&self, account: &str) -> bool {
        self.inner
            .lock()
            .account_usage
            .get(account)
            .copied()
            .unwrap_or(0)
            > 0
    }
    pub fn try_reserve(
        self: &Arc<Self>,
        job_id: &str,
        account: &str,
        session: &str,
        capacity: u32,
    ) -> Option<GenerationLease> {
        let mut book = self.inner.lock();
        let control = book.jobs.get(job_id)?;
        if control.cancel.load(Ordering::Acquire) || control.status.state != ExecutionState::Queued
        {
            return None;
        }
        let current = control.status.application_id.clone();
        let competing = book
            .queue
            .iter()
            .filter_map(|id| book.jobs.get(id))
            .filter(|job| {
                job.accounts.contains(account)
                    && job.status.state == ExecutionState::Queued
                    && !job.cancel.load(Ordering::Acquire)
            })
            .collect::<Vec<_>>();
        let first_other = competing
            .iter()
            .copied()
            .find(|job| Some(&job.status.application_id) != book.last_application.get(account));
        let next_app = first_other
            .or_else(|| competing.first().copied())?
            .status
            .application_id
            .clone();
        if current != next_app
            || competing
                .iter()
                .copied()
                .find(|job| job.status.application_id == current)?
                .status
                .id
                != job_id
            || book.account_usage.get(account).copied().unwrap_or(0) >= capacity
            || book.sessions.contains_key(session)
        {
            return None;
        }
        *book.account_usage.entry(account.to_owned()).or_default() += 1;
        book.sessions.insert(session.to_owned(), job_id.to_owned());
        book.queue.retain(|id| id != job_id);
        book.last_application.insert(account.to_owned(), current);
        if let Some(job) = book.jobs.get_mut(job_id) {
            job.status.account_id = Some(account.to_owned());
            job.status.state = ExecutionState::Preparing;
        }
        Some(GenerationLease {
            book: self.clone(),
            account: account.to_owned(),
            session: session.to_owned(),
        })
    }
}
impl Drop for GenerationLease {
    fn drop(&mut self) {
        let mut book = self.book.inner.lock();
        book.sessions.remove(&self.session);
        if let Some(usage) = book.account_usage.get_mut(&self.account) {
            *usage = usage.saturating_sub(1);
        }
    }
}

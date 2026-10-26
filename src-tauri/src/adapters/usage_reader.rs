use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use parking_lot::Mutex;

use super::app_server::{AppServerClient, IsolatedCodexHome};
use crate::{
    domain::AccountSnapshot,
    error::AppError,
    ports::{AccessSession, UsageReader},
};

#[derive(Default)]
pub struct AppServerUsageReader {
    session: Mutex<Option<ReadSession>>,
    #[cfg(test)]
    test_binary: Option<std::path::PathBuf>,
}

// Drop the child before its private home. One reader is reused, not one per account.
struct ReadSession {
    client: AppServerClient,
    _home: IsolatedCodexHome,
}

impl ReadSession {
    fn open(cancel: Arc<AtomicBool>) -> Result<Self, AppError> {
        let home = IsolatedCodexHome::create()?;
        let client = AppServerClient::spawn_read_only(home.directory.path(), cancel)?;
        Ok(Self {
            client,
            _home: home,
        })
    }

    fn sample(
        &mut self,
        session: &AccessSession,
        cancel: Arc<AtomicBool>,
    ) -> Result<AccountSnapshot, AppError> {
        self.client.set_query_cancellation(cancel);
        self.client.reset_read_only_notifications();
        self.client.set_access_session(session)?;
        self.client.read_usage()
    }
}

impl UsageReader for AppServerUsageReader {
    fn close(&self) {
        self.session.lock().take();
    }

    fn read(
        &self,
        access: &AccessSession,
        cancel: Arc<AtomicBool>,
    ) -> Result<AccountSnapshot, AppError> {
        if cancel.load(Ordering::Acquire) {
            return Err(AppError::Cancelled);
        }
        let mut session = self.session.lock();
        if session.is_none() {
            *session = Some(self.open_session(cancel.clone())?);
        }
        let result = match session.as_mut() {
            Some(reader) => reader.sample(access, cancel),
            None => Err(AppError::Internal("Leitor não inicializado".to_owned())),
        };
        // Never let a timed-out channel apply a late response to another identity.
        if result.is_err() {
            session.take();
        }
        result
    }
}

impl AppServerUsageReader {
    fn open_session(&self, cancel: Arc<AtomicBool>) -> Result<ReadSession, AppError> {
        #[cfg(test)]
        if let Some(binary) = self.test_binary.as_ref() {
            let home = IsolatedCodexHome::create()?;
            let client =
                AppServerClient::spawn_test_binary(home.directory.path(), binary, cancel, true)?;
            return Ok(ReadSession {
                client,
                _home: home,
            });
        }
        ReadSession::open(cancel)
    }
}

#[cfg(test)]
impl AppServerUsageReader {
    pub(crate) fn from_test_client(client: AppServerClient) -> Self {
        Self {
            session: Mutex::new(Some(ReadSession {
                client,
                _home: IsolatedCodexHome::create().expect("fixture home"),
            })),
            test_binary: None,
        }
    }

    #[cfg(unix)]
    pub(crate) fn from_test_binary(binary: std::path::PathBuf) -> Self {
        Self {
            session: Mutex::new(None),
            test_binary: Some(binary),
        }
    }
}

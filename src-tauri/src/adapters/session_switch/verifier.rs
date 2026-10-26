//! Protocol ownership is separate from the Desktop authentication transaction.
use super::{current_account_id, measure};
use crate::{
    adapters::{app_server::AppServerClient, auth_store},
    domain::AccountSnapshot,
    error::AppError,
    ports::{DestinationPreparation, SessionVerifier},
};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::{atomic::AtomicBool, Arc},
    time::Duration,
};

pub(super) struct AppServerSessionVerifier {
    codex_home: PathBuf,
    #[cfg(test)]
    test_binary: Option<PathBuf>,
}

impl AppServerSessionVerifier {
    pub(super) fn new(codex_home: PathBuf) -> Self {
        Self {
            codex_home,
            #[cfg(test)]
            test_binary: None,
        }
    }

    #[cfg(all(test, unix))]
    pub(super) fn with_binary(codex_home: PathBuf, binary: PathBuf) -> Self {
        Self {
            codex_home,
            test_binary: Some(binary),
        }
    }

    fn open_client(
        &self,
        home: &Path,
        cancel: Arc<AtomicBool>,
        read_only: bool,
    ) -> Result<AppServerClient, AppError> {
        #[cfg(test)]
        if let Some(binary) = self.test_binary.as_ref() {
            return AppServerClient::spawn_test_binary(home, binary, cancel, read_only);
        }
        if read_only {
            AppServerClient::spawn_read_only(home, cancel)
        } else {
            AppServerClient::spawn_cancellable(home, cancel)
        }
    }

    fn preflight_access(&self, document: &Value, cancel: Arc<AtomicBool>) -> Result<(), AppError> {
        let home = crate::adapters::app_server::IsolatedCodexHome::create()?;
        let mut client = self.open_client(home.directory.path(), cancel, true)?;
        client.set_read_only_session(document)?;
        client.validate_access_identity().map(|_| ())
    }
}

impl SessionVerifier for AppServerSessionVerifier {
    fn prepare_destination(
        &self,
        expected: &str,
        document: &Value,
        persist: &dyn Fn(&Value) -> Result<(), AppError>,
        cancel: Arc<AtomicBool>,
    ) -> Result<DestinationPreparation, AppError> {
        auth_store::validate_auth_document(document, Some(expected))?;
        if current_account_id(&self.codex_home.join("auth.json"))?.as_deref() == Some(expected) {
            return match self.preflight_access(document, cancel) {
                Ok(()) => Ok(DestinationPreparation::Verified),
                Err(error) if needs_owned_renewal(&error) => {
                    Ok(DestinationPreparation::NeedsExclusiveRenewal)
                }
                Err(error) => Err(error),
            };
        }
        let home = crate::adapters::app_server::IsolatedCodexHome::create()?;
        auth_store::atomic_write_auth(&home.directory.path().join("auth.json"), document)?;
        let mut client = self.open_client(home.directory.path(), cancel, false)?;
        let result = validate_managed_session(&mut client, &mut |_, _| {});
        drop(client);
        let refreshed = auth_store::read_auth(&home.directory.path().join("auth.json"))?;
        auth_store::validate_auth_document(&refreshed, Some(expected))?;
        if refreshed != *document {
            persist(&refreshed)?;
        }
        result.map(|_| DestinationPreparation::Verified)
    }
    fn preflight(&self, expected_account_id: &str, document: &Value) -> Result<(), AppError> {
        auth_store::validate_auth_document(document, Some(expected_account_id))?;
        self.preflight_access(document, Arc::new(AtomicBool::new(false)))
    }

    fn verify(&self, expected_account_id: &str) -> Result<AccountSnapshot, AppError> {
        let mut noop = |_: &'static str, _: Duration| {};
        self.verify_with_trace(expected_account_id, &mut noop)
    }

    fn verify_with_trace(
        &self,
        expected_account_id: &str,
        trace: &mut dyn FnMut(&'static str, Duration),
    ) -> Result<AccountSnapshot, AppError> {
        let mut client = measure(trace, "app_server_spawn_initialize", || {
            self.open_client(&self.codex_home, Arc::new(AtomicBool::new(false)), false)
        })?;
        let snapshot = validate_managed_session(&mut client, trace)?;
        let document = measure(trace, "read_verified_auth", || {
            auth_store::read_auth(&self.codex_home.join("auth.json"))
        })?;
        measure(trace, "validate_verified_auth", || {
            auth_store::validate_auth_document(&document, Some(expected_account_id)).map(|_| ())
        })?;
        Ok(snapshot)
    }

    fn verify_current_with_trace(
        &self,
        expected_account_id: &str,
        trace: &mut dyn FnMut(&'static str, Duration),
    ) -> Result<AccountSnapshot, AppError> {
        let document = auth_store::read_auth(&self.codex_home.join("auth.json"))?;
        auth_store::validate_auth_document(&document, Some(expected_account_id))?;
        let home = crate::adapters::app_server::IsolatedCodexHome::create()?;
        let mut client = self.open_client(
            home.directory.path(),
            Arc::new(AtomicBool::new(false)),
            true,
        )?;
        client.set_read_only_session(&document)?;
        client.validate_session_with_trace(trace)
    }
}

pub(super) fn needs_owned_renewal(error: &AppError) -> bool {
    error.is_authentication_failure() && !error.is_refresh_rejected()
}

fn validate_managed_session(
    client: &mut AppServerClient,
    trace: &mut dyn FnMut(&'static str, Duration),
) -> Result<AccountSnapshot, AppError> {
    match client.validate_session_with_trace(trace) {
        Err(error) if needs_owned_renewal(&error) => {
            client.validate_stored_session_with_trace(trace)
        }
        result => result,
    }
}

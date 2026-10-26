use std::sync::{Arc, LazyLock};

use keyring_core::{api::CredentialStore, Error, Result};
use parking_lot::Mutex;

// The connection belongs to this adapter, so a daemon restart does not leave
// entries permanently attached to keyring's one-time default-store session.
static STORE: LazyLock<Mutex<Option<Arc<CredentialStore>>>> = LazyLock::new(|| Mutex::new(None));

pub struct Entry {
    service: String,
    user: String,
}

impl Entry {
    pub fn new(service: &str, user: &str) -> Result<Self> {
        Ok(Self {
            service: service.to_owned(),
            user: user.to_owned(),
        })
    }

    pub fn get_password(&self) -> Result<String> {
        self.perform(keyring_core::Entry::get_password)
    }

    pub fn set_password(&self, value: &str) -> Result<()> {
        self.perform(|entry| entry.set_password(value))
    }

    pub fn delete_credential(&self) -> Result<()> {
        self.perform(keyring_core::Entry::delete_credential)
    }

    fn perform<T>(&self, operation: impl Fn(&keyring_core::Entry) -> Result<T>) -> Result<T> {
        #[cfg(test)]
        if let Some(store) = TEST_STORE.with(|slot| slot.borrow().clone()) {
            return operation(&store.build(&self.service, &self.user, None)?);
        }
        let mut cached = STORE.lock();
        retry_stale_session(|reconnect| {
            if reconnect {
                *cached = None;
            }
            let result = perform(&mut cached, &self.service, &self.user, &operation);
            if reconnect && result.is_err() {
                *cached = None;
            }
            result
        })
    }
}

// Test isolation is per-thread, never a replacement of the native process-wide
// cache. The real KeyringStore writer runs against keyring-core's memory store.
#[cfg(test)]
thread_local! {
    static TEST_STORE: std::cell::RefCell<Option<Arc<CredentialStore>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(super) fn with_test_store<T>(store: Arc<CredentialStore>, operation: impl FnOnce() -> T) -> T {
    struct Reset(Option<Arc<CredentialStore>>);
    impl Drop for Reset {
        fn drop(&mut self) {
            TEST_STORE.with(|slot| *slot.borrow_mut() = self.0.take());
        }
    }
    let _reset = Reset(TEST_STORE.with(|slot| slot.replace(Some(store))));
    operation()
}

fn retry_stale_session<T>(mut operation: impl FnMut(bool) -> Result<T>) -> Result<T> {
    let result = operation(false);
    if result.as_ref().is_err_and(session_is_stale) {
        return operation(true);
    }
    result
}

fn perform<T>(
    cached: &mut Option<Arc<CredentialStore>>,
    service: &str,
    user: &str,
    operation: &impl Fn(&keyring_core::Entry) -> Result<T>,
) -> Result<T> {
    let store = match cached {
        Some(store) => store.clone(),
        None => {
            let store = connect_store()?;
            *cached = Some(store.clone());
            store
        }
    };
    let entry = store.build(service, user, None)?;
    operation(&entry)
}

fn connect_store() -> Result<Arc<CredentialStore>> {
    #[cfg(target_os = "linux")]
    {
        Ok(zbus_secret_service_keyring_store::Store::new()?)
    }
    #[cfg(target_os = "windows")]
    {
        Ok(windows_native_keyring_store::Store::new()?)
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        Err(Error::NoDefaultStore)
    }
}

pub fn check_unlocked() -> std::result::Result<(), crate::error::AppError> {
    #[cfg(target_os = "linux")]
    {
        use secret_service::{blocking::SecretService, EncryptionType};
        let check = || {
            // This session only reads a lock flag; it never reads a secret.
            let service = SecretService::connect(EncryptionType::Plain)?;
            let collection = service.get_default_collection()?;
            collection.is_locked()
        };
        match check() {
            Ok(false) => Ok(()),
            Ok(true) => Err(crate::error::AppError::CredentialUnavailable(
                "Desbloqueie o cofre do sistema para consultar as sessões salvas.".to_owned(),
            )),
            Err(_) => Err(crate::error::AppError::CredentialUnavailable(
                "O cofre do sistema não respondeu. A consulta automática será retomada depois."
                    .to_owned(),
            )),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(())
    }
}

pub fn account_user_names() -> std::result::Result<Vec<String>, crate::error::AppError> {
    use crate::error::AppError;
    #[cfg(target_os = "linux")]
    {
        use secret_service::{blocking::SecretService, EncryptionType};
        let query = || -> std::result::Result<Vec<String>, secret_service::Error> {
            let service = SecretService::connect(EncryptionType::Plain)?;
            let items = service.search_items(std::collections::HashMap::from([(
                "service",
                "com.codex.account-manager",
            )]))?;
            if !items.locked.is_empty() {
                return Err(secret_service::Error::Locked);
            }
            items
                .unlocked
                .iter()
                .map(|item| {
                    item.get_attributes()
                        .map(|attributes| attributes.get("username").cloned().unwrap_or_default())
                })
                .collect()
        };
        query().map_err(|_|AppError::CredentialUnavailable("Não foi possível enumerar os cadastros antigos no cofre, sem desbloqueio automático. Nenhuma conta será removida.".to_owned()))
    }
    #[cfg(target_os = "windows")]
    {
        use keyring_core::api::CredentialStoreApi;
        let store = windows_native_keyring_store::Store::new()
            .map_err(|_| AppError::CredentialUnavailable("Cofre indisponível.".to_owned()))?;
        let entries = store
            .search(&std::collections::HashMap::from([(
                "pattern",
                r"^account:.*\.com\.codex\.account-manager$",
            )]))
            .map_err(|_| {
                AppError::CredentialUnavailable(
                    "Não foi possível enumerar os cadastros antigos no cofre.".to_owned(),
                )
            })?;
        Ok(entries
            .iter()
            .filter_map(|entry| entry.get_specifiers())
            .filter(|(service, _)| service == "com.codex.account-manager")
            .map(|(_, user)| user)
            .collect())
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        Err(AppError::CredentialUnavailable(
            "Enumeração de cadastros não disponível nesta plataforma.".to_owned(),
        ))
    }
}

fn session_is_stale(error: &Error) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    [
        "org.freedesktop.secret.error.nosession",
        "serviceunknown",
        "namehasnoowner",
        "connection is closed",
        "broken pipe",
        "disconnected",
    ]
    .iter()
    .any(|marker| message.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::{retry_stale_session, session_is_stale};
    use keyring_core::Error;

    #[test]
    fn lost_dbus_session_is_recoverable_but_missing_secret_is_not() {
        let stale = Error::PlatformFailure(Box::new(std::io::Error::other(
            "org.freedesktop.Secret.Error.NoSession: The session does not exist",
        )));
        assert!(session_is_stale(&stale));
        assert!(!session_is_stale(&Error::NoEntry));
    }

    fn stale_session() -> Error {
        Error::PlatformFailure(Box::new(std::io::Error::other(
            "org.freedesktop.Secret.Error.NoSession",
        )))
    }

    #[test]
    fn daemon_restart_reconnects_once_and_repeats_the_original_operation() {
        let mut calls = 0;
        let value = retry_stale_session(|reconnect| {
            calls += 1;
            if reconnect {
                Ok("saved credential")
            } else {
                Err(stale_session())
            }
        })
        .expect("recovered session");
        assert_eq!(calls, 2);
        assert_eq!(value, "saved credential");
    }

    #[test]
    fn persistent_failure_is_bounded_and_missing_entry_is_not_retried() {
        let mut calls = 0;
        let result: Result<(), Error> = retry_stale_session(|_| {
            calls += 1;
            Err(stale_session())
        });
        assert!(result.is_err());
        assert_eq!(calls, 2);
        calls = 0;
        let missing: Result<(), Error> = retry_stale_session(|_| {
            calls += 1;
            Err(Error::NoEntry)
        });
        assert!(matches!(missing, Err(Error::NoEntry)));
        assert_eq!(calls, 1);
    }
}

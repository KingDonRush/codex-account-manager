use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::adapters::browser_runtime::BrowserHandle;
use crate::adapters::mail_catalog::resolve_provider;
use crate::domain::{MailProvider, ManagedProfile, ProfileStatus, SaveMailProviderInput};
use crate::error::AppError;

use super::ManagerCore;

impl ManagerCore {
    pub fn list_providers(&self) -> Result<Vec<MailProvider>, AppError> {
        self.database.list_providers()
    }

    pub fn save_mail_provider(
        &self,
        input: SaveMailProviderInput,
    ) -> Result<MailProvider, AppError> {
        let display_name = input.display_name.trim();
        if display_name.is_empty() || display_name.chars().count() > 80 {
            return Err(AppError::Validation(
                "o nome do provedor deve ter entre 1 e 80 caracteres".to_owned(),
            ));
        }
        validate_mail_url(Some(&input.default_webmail_url))?;
        let domains = normalize_domains(input.domains)?;
        let aliases = input
            .aliases
            .into_iter()
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty())
            .collect();
        let existing = input
            .id
            .as_deref()
            .and_then(|id| self.database.provider(id).ok().flatten());
        let provider = MailProvider {
            id: input
                .id
                .unwrap_or_else(|| format!("custom-{}", uuid::Uuid::new_v4())),
            display_name: display_name.to_owned(),
            domains,
            default_webmail_url: input.default_webmail_url,
            aliases,
            is_custom: existing.as_ref().is_none_or(|provider| provider.is_custom),
        };
        self.database.save_provider(&provider)
    }

    pub fn create_profile(
        &self,
        name: &str,
        provider_id: Option<&str>,
        mail_url: Option<&str>,
    ) -> Result<ManagedProfile, AppError> {
        validate_mail_url(mail_url)?;
        if let Some(provider_id) = provider_id {
            if self.database.provider(provider_id)?.is_none() {
                return Err(AppError::Validation(format!(
                    "provedor desconhecido: {provider_id}"
                )));
            }
        }
        self.database.create_profile(name, provider_id, mail_url)
    }

    pub fn rename_profile(&self, profile_id: &str, name: &str) -> Result<ManagedProfile, AppError> {
        self.database.rename_profile(profile_id, name)
    }

    pub fn update_profile_mail_url(
        &self,
        profile_id: &str,
        mail_url: Option<&str>,
    ) -> Result<ManagedProfile, AppError> {
        let mail_url = mail_url.map(str::trim).filter(|value| !value.is_empty());
        validate_mail_url(mail_url)?;
        self.database.update_profile_mail_url(profile_id, mail_url)
    }

    pub fn archive_profile(&self, profile_id: &str) -> Result<ManagedProfile, AppError> {
        self.with_profile_mutation(profile_id, || {
            self.close_profile_browser(profile_id);
            self.database
                .set_profile_status(profile_id, ProfileStatus::Archived)
        })
    }

    pub fn restore_profile(&self, profile_id: &str) -> Result<ManagedProfile, AppError> {
        self.with_profile_mutation(profile_id, || {
            self.database
                .set_profile_status(profile_id, ProfileStatus::Active)
        })
    }

    pub(super) fn delete_profile_data(
        &self,
        profile_id: &str,
        permanently: bool,
    ) -> Result<(), AppError> {
        self.close_profile_browser(profile_id);
        let profile = self.database.profile(profile_id)?;
        if !permanently {
            return self.database.delete_profile(profile_id, false);
        }

        let Some(account_id) = profile.account_id.as_deref() else {
            return self.database.delete_profile(profile_id, true);
        };
        let remove_account_data = self.database.profile_count_for_account(account_id)? == 1;
        let previous_credential = if remove_account_data {
            self.credentials.load_optional(account_id)?
        } else {
            None
        };
        if remove_account_data {
            if let Err(error) = self.credentials.delete(account_id) {
                self.restore_credential(account_id, previous_credential.as_ref())
                    .map_err(|restore_error| {
                        AppError::Internal(format!(
                            "falha ao remover a credencial ({error}); restauração também falhou ({restore_error})"
                        ))
                    })?;
                return Err(error);
            }
        }

        if let Err(error) = self.database.delete_profile(profile_id, true) {
            if remove_account_data {
                self.restore_credential(account_id, previous_credential.as_ref())
                    .map_err(|restore_error| {
                        AppError::Internal(format!(
                            "falha ao excluir o perfil ({error}); restauração da credencial também falhou ({restore_error})"
                        ))
                    })?;
            }
            return Err(error);
        }
        if remove_account_data {
            if let Err(error) = self.database.delete_account_metadata(account_id) {
                self.restore_credential(account_id, previous_credential.as_ref())
                    .map_err(|restore_error| {
                        AppError::Internal(format!(
                            "perfil excluído, mas a limpeza dos metadados falhou ({error}); restauração da credencial também falhou ({restore_error})"
                        ))
                    })?;
                return Err(error);
            }
        }
        Ok(())
    }

    pub fn open_mail(&self, profile_id: &str) -> Result<(), AppError> {
        let _lease = self.tasks.enter()?;
        let profile = self.database.profile(profile_id)?;
        let url = self.mail_url_for_profile(&profile)?.ok_or_else(|| {
            AppError::Validation(
                "este perfil ainda não tem e-mail nem URL de webmail; conecte-o ou salve uma URL"
                    .to_owned(),
            )
        })?;
        validate_mail_url(Some(&url))?;
        let profile_dir = PathBuf::from(&profile.data_dir);
        if self.profile_browser_is_alive(profile_id) {
            return self.browser.open_tab(&profile_dir, &url);
        }
        let handle = self.browser.launch_controlled(&profile_dir, &url, None)?;
        self.remember_browser_session(profile_id.to_owned(), handle);
        Ok(())
    }

    pub fn open_profile_browser(&self, profile_id: &str) -> Result<(), AppError> {
        let _lease = self.tasks.enter()?;
        let profile = self.database.profile(profile_id)?;
        let directory = PathBuf::from(&profile.data_dir);
        let url = "https://chatgpt.com/";
        if self.profile_browser_is_alive(profile_id) {
            return self.browser.open_tab(&directory, url);
        }
        let browser = self.browser.launch_controlled(&directory, url, None)?;
        if let Err(error) = self.database.record_browser_opened(profile_id) {
            let _ = browser.terminate();
            return Err(error);
        }
        self.remember_browser_session(profile_id.to_owned(), browser);
        Ok(())
    }

    pub fn open_profile_folder(&self, profile_id: &str) -> Result<(), AppError> {
        let profile = self.database.profile(profile_id)?;
        let path = PathBuf::from(profile.data_dir);
        if !path.is_dir() {
            return Err(AppError::Validation(
                "o diretório deste perfil não existe".to_owned(),
            ));
        }
        open::that_detached(path).map_err(AppError::Io)
    }

    pub(super) fn mail_url_for_profile(
        &self,
        profile: &ManagedProfile,
    ) -> Result<Option<String>, AppError> {
        if let Some(url) = profile.mail_url.as_deref() {
            return Ok(Some(url.to_owned()));
        }
        if let Some(provider_id) = profile.mail_provider_id.as_deref() {
            if let Some(provider) = self.database.provider(provider_id)? {
                return Ok(Some(provider.default_webmail_url));
            }
        }
        let Some(email) = profile.email.as_deref() else {
            return Ok(None);
        };
        let providers = self.database.list_providers()?;
        let Some(provider) = resolve_provider(email, &providers) else {
            return Ok(None);
        };
        Ok(Some(provider.default_webmail_url.clone()))
    }

    pub(super) fn resolve_provider_id(&self, email: &str) -> Option<String> {
        let providers = self.database.list_providers().ok()?;
        resolve_provider(email, &providers).map(|provider| provider.id.clone())
    }

    fn profile_browser_is_alive(&self, profile_id: &str) -> bool {
        let pending = self
            .operations
            .lock()
            .controls
            .values()
            .find(|control| control.profile_id == profile_id)
            .cloned();
        if pending
            .and_then(|control| control.browser.lock().clone())
            .is_some_and(|browser| browser.is_alive().unwrap_or(false))
        {
            return true;
        }
        let browser = self.browser_sessions.lock().get(profile_id).cloned();
        if browser.and_then(|handle| handle.is_alive().ok()) == Some(true) {
            return true;
        }
        self.browser_sessions.lock().remove(profile_id);
        false
    }

    pub(super) fn close_profile_browser(&self, profile_id: &str) {
        if let Some(browser) = self.browser_sessions.lock().remove(profile_id) {
            let _ = browser.terminate();
        }
    }

    #[cfg(not(unix))]
    pub(super) fn take_profile_browser(&self, profile_id: &str) -> Option<BrowserHandle> {
        self.browser_sessions.lock().remove(profile_id)
    }

    pub(super) fn remember_browser_session(&self, profile_id: String, browser: BrowserHandle) {
        self.browser_sessions
            .lock()
            .insert(profile_id.clone(), browser.clone());
        let sessions = Arc::downgrade(&self.browser_sessions);
        let watched_browser = browser.clone();
        let short_profile_id = profile_id.chars().take(8).collect::<String>();
        let thread_name = format!("browser-session-{short_profile_id}");
        let _ = thread::Builder::new()
            .name(thread_name)
            .spawn(move || loop {
                thread::sleep(Duration::from_millis(500));
                if watched_browser.is_alive().unwrap_or(false) {
                    continue;
                }
                let Some(sessions) = sessions.upgrade() else {
                    break;
                };
                let mut sessions = sessions.lock();
                let is_current = sessions
                    .get(&profile_id)
                    .is_some_and(|current| current.same_process(&watched_browser));
                if is_current {
                    sessions.remove(&profile_id);
                }
                break;
            });
    }
}

pub(super) fn validate_mail_url(value: Option<&str>) -> Result<(), AppError> {
    let Some(value) = value else {
        return Ok(());
    };
    let parsed = url::Url::parse(value)
        .map_err(|_| AppError::Validation("URL de webmail inválida".to_owned()))?;
    if !matches!(parsed.scheme(), "https" | "http")
        || parsed.username() != ""
        || parsed.password().is_some()
    {
        return Err(AppError::Validation(
            "a URL de webmail deve usar HTTP(S) e não pode conter credenciais".to_owned(),
        ));
    }
    Ok(())
}

fn normalize_domains(values: Vec<String>) -> Result<Vec<String>, AppError> {
    let mut domains = Vec::new();
    for value in values {
        let domain = value.trim().trim_start_matches('@').to_ascii_lowercase();
        if domain.is_empty() {
            continue;
        }
        if !domain.contains('.') || domain.contains('/') || domain.contains(char::is_whitespace) {
            return Err(AppError::Validation(format!(
                "domínio de e-mail inválido: {domain}"
            )));
        }
        if !domains.contains(&domain) {
            domains.push(domain);
        }
    }
    if domains.is_empty() {
        return Err(AppError::Validation(
            "informe ao menos um domínio de e-mail".to_owned(),
        ));
    }
    Ok(domains)
}

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use chrono::Utc;
use serde_json::json;
use sha2::{Digest, Sha256};

use super::ManagerCore;
use crate::adapters::logging;
use crate::application::usage_schedule::UsageSchedule;
use crate::domain::{ConnectionCheck, CredentialState, ManagedProfile};
use crate::error::AppError;

#[derive(Clone, Copy)]
enum SampleAvailability {
    Observed,
    NotPublished,
}

#[derive(Clone, Copy)]
struct UsageObservation {
    availability: SampleAvailability,
    checked_at: chrono::DateTime<Utc>,
}

// One completed successful sample, not a queue or a cache of credentials/errors.
pub(super) struct CompletedUsage {
    profile_id: String,
    account_id: String,
    signature: String,
    access_fingerprint: String,
    completed_at: Instant,
    observation: UsageObservation,
}

impl CompletedUsage {
    fn observation_since(
        &self,
        profile: &ManagedProfile,
        signature: &str,
        access_fingerprint: &str,
        requested_at: Instant,
    ) -> Option<UsageObservation> {
        let same_scope = self.profile_id == profile.id
            && profile.account_id.as_deref() == Some(&self.account_id)
            && self.signature == signature
            && self.access_fingerprint == access_fingerprint;
        (same_scope && self.completed_at >= requested_at).then_some(self.observation)
    }
}

#[derive(Clone, Copy)]
enum QueryMode {
    Background,
    Manual(Instant),
}

impl ManagerCore {
    pub fn check_profile(&self, profile_id: &str) -> Result<ConnectionCheck, AppError> {
        let _lease = self.tasks.enter()?;
        if self
            .operations
            .lock()
            .controls
            .values()
            .any(|control| control.profile_id == profile_id)
        {
            return Err(AppError::Busy(
                "Aguarde a operação atual antes de verificar o saldo.".to_owned(),
            ));
        }
        let profile = self
            .list_profiles()?
            .into_iter()
            .find(|profile| profile.id == profile_id)
            .ok_or_else(|| AppError::ProfileNotFound(profile_id.to_owned()))?;
        let started = Instant::now();
        let result = self.refresh_profile_usage(&profile, QueryMode::Manual(started));
        if matches!(result, Err(AppError::CredentialUnavailable(_))) {
            if let Some(id) = profile.account_id.as_deref() {
                self.database
                    .set_credential_state(id, CredentialState::Unavailable)?;
            }
        }
        let _ = logging::record(
            &self.database,
            None,
            Some(profile_id),
            "manual_connection_check",
            json!({
                "durationMs": started.elapsed().as_millis(), "errorCode": result.as_ref().err().map(AppError::code),
            }),
        );
        self.emit_dashboard_changed();
        Ok(check_report(result))
    }

    pub(super) fn refresh_due_usage(&self, schedule: &mut UsageSchedule) -> bool {
        if self.maintenance.cancel.load(Ordering::Acquire) {
            return false;
        }
        let Ok(mut profiles) = self.list_profiles() else {
            return false;
        };
        self.usage_access.lock().retain(|id, _| {
            profiles
                .iter()
                .any(|profile| profile.account_id.as_deref() == Some(id))
        });
        let busy = self
            .operations
            .lock()
            .controls
            .values()
            .map(|control| control.profile_id.clone())
            .collect::<std::collections::HashSet<_>>();
        profiles.retain(|profile| !busy.contains(&profile.id));
        let Some(profile) = schedule.next(&profiles, Utc::now()) else {
            return false;
        };
        let started = Instant::now();
        let result = self.refresh_profile_usage(profile, QueryMode::Background);
        if matches!(
            result,
            Err(AppError::Busy(_) | AppError::QueryContextChanged(_))
        ) {
            return false;
        }
        if matches!(result, Err(AppError::CredentialUnavailable(_))) {
            if let Some(account_id) = profile.account_id.as_deref() {
                let _ = self
                    .database
                    .set_credential_state(account_id, CredentialState::Unavailable);
            }
            self.emit_dashboard_changed();
        }
        schedule.record(profile, Utc::now(), result.is_ok());
        if let Err(error) = &result {
            let _ = logging::record(
                &self.database,
                None,
                Some(&profile.id),
                "usage_refresh_failed",
                json!({"durationMs":started.elapsed().as_millis(),"errorCode":error.code()}),
            );
        }
        if result.is_ok() {
            self.emit_dashboard_changed();
        }
        true
    }

    fn refresh_profile_usage(
        &self,
        profile: &ManagedProfile,
        mode: QueryMode,
    ) -> Result<UsageObservation, AppError> {
        let mut query = self.acquire_usage_query(mode)?;
        let updated = if matches!(mode, QueryMode::Manual(_)) {
            Some(self.current_query_profile(profile)?)
        } else {
            None
        };
        let profile = updated.as_ref().unwrap_or(profile);
        let account_id = profile
            .account_id
            .as_deref()
            .ok_or_else(|| AppError::Validation("conta ausente".to_owned()))?;
        let signature = self.usage_generation_signature(profile)?;
        let access = self.usage_access_session(profile)?;
        let access_fingerprint = hex::encode(Sha256::digest(access.access_token.as_bytes()));
        if let QueryMode::Manual(requested_at) = mode {
            if let Some(observation) = query.as_ref().and_then(|sample| {
                sample.observation_since(profile, &signature, &access_fingerprint, requested_at)
            }) {
                return Ok(observation);
            }
        }
        *query = None;
        let observation = self.sample_profile_usage(profile, &signature, &access)?;
        *query = Some(CompletedUsage {
            profile_id: profile.id.clone(),
            account_id: account_id.to_owned(),
            signature,
            access_fingerprint,
            completed_at: Instant::now(),
            observation,
        });
        Ok(observation)
    }

    fn acquire_usage_query(
        &self,
        mode: QueryMode,
    ) -> Result<parking_lot::MutexGuard<'_, Option<CompletedUsage>>, AppError> {
        match mode {
            QueryMode::Background => self.usage_lock.try_lock().ok_or_else(|| {
                AppError::Busy("Já existe uma consulta de saldo em andamento.".to_owned())
            }),
            QueryMode::Manual(_) => crate::application::operation_gate::acquire(
                &self.usage_lock,
                &self.maintenance.cancel,
                Duration::from_secs(30),
            ),
        }
    }

    fn current_query_profile(&self, expected: &ManagedProfile) -> Result<ManagedProfile, AppError> {
        // A queued check must not use a profile/identity captured before waiting.
        let current = self
            .list_profiles()?
            .into_iter()
            .find(|current| current.id == expected.id)
            .ok_or_else(|| AppError::ProfileNotFound(expected.id.clone()))?;
        if current.account_id != expected.account_id {
            return Err(AppError::QueryContextChanged(
                "A conta deste perfil mudou durante a espera; a consulta original não foi aplicada.".to_owned(),
            ));
        }
        Ok(current)
    }

    fn sample_profile_usage(
        &self,
        profile: &ManagedProfile,
        signature: &str,
        access: &crate::ports::AccessSession,
    ) -> Result<UsageObservation, AppError> {
        let account_id = &access.account_id;
        let snapshot = self
            .usage_reader
            .read(access, self.maintenance.cancel.clone())
            .inspect_err(|_| {
                self.usage_access.lock().remove(account_id);
            })?;
        let current = self.database.profile(&profile.id)?;
        if current.account_id.as_deref() != Some(account_id)
            || self.usage_generation_signature(&current)? != signature
        {
            return Err(AppError::QueryContextChanged(
                "A sessão mudou durante a consulta; uma nova amostra será solicitada.".to_owned(),
            ));
        }
        let availability = if snapshot.rate_limits.is_some() {
            if profile.credential_state != CredentialState::Ready && !profile.is_active_account {
                self.credentials.confirm_usable_access(access)?;
            }
            SampleAvailability::Observed
        } else {
            SampleAvailability::NotPublished
        };
        self.database.update_usage(account_id, &snapshot)?;
        Ok(UsageObservation {
            availability,
            checked_at: Utc::now(),
        })
    }
}

fn check_report(result: Result<UsageObservation, AppError>) -> ConnectionCheck {
    let error_code = result.as_ref().err().map(|error| error.code().to_owned());
    let (title, guidance) = match result.as_ref().map(|observation| observation.availability) {
        Ok(SampleAvailability::Observed) => ("Conexão e saldo confirmados", "A consulta usou um ambiente isolado. A conta ativa e o histórico do Codex não foram alterados."),
        Ok(SampleAvailability::NotPublished) => ("Saldo não publicado pelo servidor", "A consulta respondeu sem quota. Isso não significa saldo zero, ilimitado ou uma conta desconectada."),
        Err(AppError::Cancelled) => ("Consulta cancelada", "A consulta foi interrompida; a amostra anterior foi preservada."),
        Err(AppError::QueryContextChanged(_)) => ("A conta ou sessão mudou", "A amostra anterior foi preservada. Confira a associação atual do perfil antes de verificar novamente; nenhuma conta diferente foi confirmada por esta consulta."),
        Err(AppError::CredentialUnavailable(_)) => ("Cofre indisponível", "A sessão salva não foi invalidada. Desbloqueie o cofre do sistema e tente verificar novamente; não é necessário refazer o login por esse erro."),
        Err(error) if error.is_authentication_failure() => ("Consulta não autenticada", "A sessão precisa ser renovada ou validada. Tente usar esta conta para renovar a credencial; refaça o login no navegador apenas se a renovação falhar."),
        Err(AppError::Keyring(_)) => ("Sessão salva inacessível", "Não foi possível ler a credencial. Verifique o cofre antes de decidir reautenticar."),
        Err(AppError::Busy(_)) => ("Consulta em andamento", "Aguarde a consulta atual terminar antes de solicitar outra. Isso evita abrir vários App Servers em paralelo."),
        Err(_) => ("Não foi possível confirmar agora", "Pode ser uma falha de transporte ou do servidor. O saldo anterior foi mantido; tente novamente sem trocar a conta ativa."),
    };
    ConnectionCheck {
        confirmed: result.is_ok(),
        can_renew: result
            .as_ref()
            .err()
            .is_some_and(AppError::is_authentication_failure),
        title: title.to_owned(),
        guidance: guidance.to_owned(),
        error_code,
        checked_at: result
            .as_ref()
            .map_or_else(|_| Utc::now(), |observation| observation.checked_at),
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod report_tests {
    use super::*;
    #[test]
    fn coffer_and_transport_failures_do_not_demand_a_new_oauth_login() {
        let coffer = check_report(Err(AppError::CredentialUnavailable("NoSession".to_owned())));
        assert!(!coffer.confirmed);
        assert_eq!(coffer.title, "Cofre indisponível");
        assert!(coffer.guidance.contains("não é necessário refazer"));
        let transport = check_report(Err(AppError::AppServer("offline".to_owned())));
        assert_eq!(transport.title, "Não foi possível confirmar agora");
        assert!(transport.guidance.contains("saldo anterior"));
        let auth = check_report(Err(AppError::AuthenticationRequired(
            "expired access".to_owned(),
        )));
        assert!(auth.can_renew);
        assert!(!coffer.can_renew);
    }

    #[test]
    fn concurrent_usage_query_is_reported_without_running_another_server() {
        let report = check_report(Err(AppError::Busy("query busy".to_owned())));
        assert_eq!(report.title, "Consulta em andamento");
        assert!(!report.confirmed && !report.can_renew);
    }
    #[test]
    fn quota_not_published_is_not_a_disconnected_or_exhausted_account() {
        let report = check_report(Ok(UsageObservation {
            availability: SampleAvailability::NotPublished,
            checked_at: Utc::now(),
        }));
        assert!(report.confirmed);
        assert!(!report.can_renew);
        assert!(report.error_code.is_none());
        assert_eq!(report.title, "Saldo não publicado pelo servidor");
    }
}

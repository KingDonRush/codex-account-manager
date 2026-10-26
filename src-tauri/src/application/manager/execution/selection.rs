use super::{Candidate, ManagerCore};
use crate::domain::execution::ExecRequest;
use crate::domain::integrations::{AccountPolicy, AuthorizationGrant, BindingTarget, Permission};
use crate::domain::ProfileStatus;
use crate::error::AppError;
use chrono::Utc;
impl ManagerCore {
    pub(in crate::application::manager) fn execution_candidates(
        &self,
        grant: &AuthorizationGrant,
        request: &ExecRequest,
    ) -> Result<Vec<Candidate>, AppError> {
        if !grant.permits(Permission::Execute, None, Utc::now()) {
            return Err(AppError::AccessDenied(
                "Execução não autorizada.".to_owned(),
            ));
        }
        let binding = self.execution_route(grant, request)?;
        if !self.background_binding_allowed(grant, &binding.target) {
            return Err(AppError::ConsentRequired(
                "Este acesso requer o gerenciador aberto.".to_owned(),
            ));
        }
        let sources = self.route_accounts(grant, &binding.target)?;
        let ids = sources
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        if request
            .account_id
            .as_ref()
            .is_some_and(|id| !ids.contains(id))
            || (!binding.automatic
                && request.account_id.is_none()
                && matches!(binding.target, BindingTarget::Group(_)))
        {
            return Err(AppError::AccessDenied(
                "Selecione uma conta exata entre os recursos autorizados.".to_owned(),
            ));
        }
        let cards = self.dashboard_snapshot()?.accounts;
        let mut candidates = Vec::new();
        let mut usable = 0;
        let mut missing_credential = false;
        for card in cards {
            let Some(remote) = card.profile.account_id.as_deref() else {
                continue;
            };
            let (id, policy, capacity, _) = self.database.account_integration_options(remote)?;
            if !ids.contains(&id)
                || request
                    .account_id
                    .as_ref()
                    .is_some_and(|selected| selected != &id)
                || !grant.permits(Permission::Execute, Some(&id), Utc::now())
                || policy == AccountPolicy::DesktopOnly
                || card.profile.status != ProfileStatus::Active
            {
                continue;
            }
            usable += 1;
            if card.effective_remaining == Some(0.0)
                || card.projected_reset && !binding.allow_projected_quota
            {
                continue;
            }
            let session = self.application_auth(&id)?;
            if matches!(
                session,
                super::super::application_auth::ApplicationAuth::Account(_)
            ) && matches!(
                card.profile.credential_state,
                crate::domain::CredentialState::Missing
                    | crate::domain::CredentialState::NotConnected
            ) && !card.profile.is_active_account
            {
                missing_credential = true;
                continue;
            }
            candidates.push(Candidate {
                group: sources.get(&id).cloned().flatten(),
                account: id,
                remote: remote.to_owned(),
                session,
                capacity,
                score: card.effective_remaining.unwrap_or(-1.0),
            });
        }
        if candidates.is_empty() {
            return Err(if missing_credential {
                AppError::AuthenticationRequired("A sessão salva da conta não está disponível. Reconecte a conta no gerenciador; aplicações não exigem um login separado.".to_owned())
            } else if usable > 0 {
                AppError::QuotaExhausted(
                    "Nenhuma conta autorizada tem saldo confirmado/permitido disponível."
                        .to_owned(),
                )
            } else {
                AppError::AccessDenied(
                    "Nenhuma conta autorizada está disponível para aplicações. Abra a conta em Aplicações e habilite Codex e aplicações.".to_owned(),
                )
            });
        }
        candidates.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.account.cmp(&b.account))
        });
        Ok(candidates)
    }
}

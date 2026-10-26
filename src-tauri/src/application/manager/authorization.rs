use super::ManagerCore;
use crate::application::authorization::{AuthorizationRequest, Claim, ConnectionRequest};
use crate::application::local_transport::PeerIdentity;
use crate::domain::integrations::*;
use crate::error::AppError;

impl ManagerCore {
    pub fn authorization_requests(&self) -> Vec<AuthorizationRequest> {
        self.authorization.pending()
    }
    pub fn request_authorization(
        &self,
        mut request: ConnectionRequest,
        peer: &PeerIdentity,
        channel: &str,
    ) -> Result<AuthorizationRequest, AppError> {
        // A declared slug is a routing hint for HUMAN consent, never a credential
        // lookup or proof. Old clients may omit the receipt ID on a new process.
        if request.application_id.is_none() {
            request.application_id = self
                .database
                .applications()?
                .into_iter()
                .find(|app| app.slug == request.slug)
                .map(|app| app.id);
        }
        self.mark_ui_visible();
        let pending = self.authorization.request(request, peer, channel)?;
        self.emit_dashboard_changed();
        if let Some(sink) = self.event_sink.read().as_ref() {
            sink.show_authorization();
        }
        Ok(pending)
    }
    pub fn claim_authorization(
        &self,
        id: &str,
        channel: &str,
        peer: &str,
    ) -> Result<Option<Claim>, AppError> {
        self.authorization.claim(id, channel, peer)
    }
    /// This entry point is intentionally absent from external dispatch.
    #[cfg(test)]
    pub fn approve_authorization(&self, id: &str, scope: GrantScope) -> Result<String, AppError> {
        self.approve_authorization_setup(id, scope, AuthorizationSetup::default())
    }

    pub fn approve_authorization_setup(
        &self,
        id: &str,
        scope: GrantScope,
        setup: AuthorizationSetup,
    ) -> Result<String, AppError> {
        let _lease = self.tasks.enter()?;
        let _admission = crate::application::operation_gate::acquire(
            &self.account_admission,
            &self.maintenance.cancel,
            std::time::Duration::from_secs(30),
        )?;
        let request = self
            .authorization
            .pending()
            .into_iter()
            .find(|request| request.id == id)
            .ok_or_else(|| {
                AppError::ConsentRequired("Pedido não está mais disponível.".to_owned())
            })?;
        let snapshot = self.integration_snapshot()?;
        if scope
            .account_ids
            .iter()
            .any(|id| !snapshot.accounts.iter().any(|account| &account.id == id))
            || scope
                .direct_account_ids
                .as_ref()
                .is_some_and(|ids| !ids.is_subset(&scope.account_ids))
            || !setup.shared_account_ids.is_subset(&scope.account_ids)
            || !setup.shared_account_ids.is_empty()
                && !scope.permissions.contains(&Permission::Execute)
            || scope.group_ids.iter().any(|id| {
                !snapshot
                    .groups
                    .iter()
                    .any(|group| &group.id == id && !group.archived)
            })
            || !scope.background_group_ids.is_subset(&scope.group_ids)
            || !scope.background_group_ids.is_empty()
                && !scope.permanent
                && scope.expires_at.is_none()
        {
            return Err(AppError::Validation(
                "O escopo mudou ou contém recursos desconhecidos.".to_owned(),
            ));
        }
        let application = match request.request.application_id.as_deref() {
            Some(id) => snapshot
                .applications
                .into_iter()
                .find(|app| app.id == id)
                .ok_or_else(|| AppError::Validation("Aplicação não encontrada.".to_owned()))?,
            None => Application {
                id: uuid::Uuid::new_v4().to_string(),
                name: request.request.name,
                slug: request.request.slug,
                paused: false,
                revision: 0,
            },
        };
        for id in &scope.background_group_ids {
            let group = snapshot
                .groups
                .iter()
                .find(|group| &group.id == id)
                .ok_or_else(|| AppError::Validation("Grupo não encontrado.".to_owned()))?;
            if group
                .exclusive_application_id
                .as_ref()
                .is_some_and(|owner| owner != &application.id)
            {
                return Err(AppError::AccessDenied(
                    "Grupo já é exclusivo de outra aplicação.".to_owned(),
                ));
            }
        }
        let (grant, key) = self.authorization.approve(id, &application.id, scope)?;
        let selected = snapshot
            .groups
            .into_iter()
            .filter(|group| grant.scope.group_ids.contains(&group.id))
            .collect::<Vec<_>>();
        if let Err(error) =
            self.database
                .commit_authorization_setup(&grant, &application, &selected, &setup)
        {
            self.authorization.deny(id);
            return Err(error);
        }
        if let Err(error) = self.authorization.publish(id, grant.id.clone(), key) {
            self.database.revoke_grant(&grant.id)?;
            return Err(error);
        }
        self.emit_dashboard_changed();
        Ok(grant.id)
    }
    pub fn deny_authorization(&self, id: &str) {
        self.authorization.deny(id);
        self.emit_dashboard_changed();
    }
    pub fn revoke_authorization(&self, id: &str) -> Result<(), AppError> {
        self.database.revoke_grant(id)?;
        self.application_servers
            .close_matching(|session| session.grant_id == id);
        self.executions.cancel_matching(|job| job.grant_id == id);
        self.emit_dashboard_changed();
        Ok(())
    }
    pub fn authorization_grant(&self, id: &str) -> Result<AuthorizationGrant, AppError> {
        self.database
            .grant(id)?
            .ok_or_else(|| AppError::AccessDenied("Autorização não encontrada.".to_owned()))
    }
}

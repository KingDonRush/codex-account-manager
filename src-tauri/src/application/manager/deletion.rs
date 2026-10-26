use super::ManagerCore;
use crate::domain::deletion::{DeletionItem, DeletionRequest, DeletionState};
use crate::domain::integrations::{AuthorizationGrant, Permission};
use crate::error::AppError;
use std::collections::BTreeSet;
#[cfg(test)]
mod tests;

impl ManagerCore {
    pub fn pending_deletions(&self) -> Vec<DeletionRequest> {
        self.deletions.pending()
    }
    pub fn request_deletion(
        &self,
        grant: Option<&AuthorizationGrant>,
        ids: &BTreeSet<String>,
    ) -> Result<DeletionRequest, AppError> {
        if ids.is_empty() || ids.len() > 128 {
            return Err(AppError::Validation(
                "Selecione entre 1 e 128 contas.".to_owned(),
            ));
        }
        let snapshot = self.integration_snapshot()?;
        let mut items = Vec::new();
        for id in ids {
            if grant.is_some_and(|grant| {
                !grant.permits(Permission::RequestDeletion, Some(id), chrono::Utc::now())
            }) {
                return Err(AppError::AccessDenied(
                    "Exclusão não autorizada neste escopo.".to_owned(),
                ));
            }
            let account = snapshot
                .accounts
                .iter()
                .find(|account| &account.id == id)
                .ok_or_else(|| AppError::Validation("Conta não encontrada.".to_owned()))?;
            let profile = self.database.profile(&account.profile_id)?;
            items.push(DeletionItem {
                account_id: id.clone(),
                profile_id: profile.id,
                name: profile.name,
                email: profile.email,
                revision: profile.updated_at.to_rfc3339(),
            });
        }
        let requester = grant
            .and_then(|grant| {
                snapshot
                    .applications
                    .iter()
                    .find(|app| app.id == grant.application_id)
            })
            .map(|app| app.name.clone())
            .unwrap_or_else(|| "Você, na interface".to_owned());
        let request = DeletionRequest {
            id: uuid::Uuid::new_v4().to_string(),
            requester,
            grant_id: grant.map(|grant| grant.id.clone()),
            items,
            state: DeletionState::AwaitingConfirmation,
            deleted_ids: Vec::new(),
            error_code: None,
        };
        self.deletions.add(request.clone())?;
        self.mark_ui_visible();
        if let Some(sink) = self.event_sink.read().as_ref() {
            sink.show_authorization();
        }
        self.emit_dashboard_changed();
        Ok(request)
    }
    pub fn deletion_status_for(
        &self,
        grant: &AuthorizationGrant,
        id: &str,
    ) -> Result<DeletionRequest, AppError> {
        let request = self.deletions.get(id)?;
        if request.grant_id.as_deref() != Some(&grant.id) {
            return Err(AppError::AccessDenied(
                "Pedido pertence a outra integração.".to_owned(),
            ));
        }
        Ok(request)
    }
    /// Called only from the trusted UI; the external API cannot confirm destruction.
    pub fn confirm_deletion(
        &self,
        id: &str,
        selected: BTreeSet<String>,
        remove_browser: bool,
    ) -> Result<DeletionRequest, AppError> {
        let mut request = self.deletions.begin(id)?;
        let result = self.commit_deletions(&mut request, &selected, remove_browser);
        request.state = if result.is_ok() {
            DeletionState::Completed
        } else {
            DeletionState::Failed
        };
        request.error_code = result.as_ref().err().map(|error| error.code().to_owned());
        self.deletions.save(request.clone());
        self.emit_dashboard_changed();
        result.map(|_| request)
    }
    fn commit_deletions(
        &self,
        request: &mut DeletionRequest,
        selected: &BTreeSet<String>,
        remove_browser: bool,
    ) -> Result<(), AppError> {
        if selected.is_empty()
            || selected
                .iter()
                .any(|id| !request.items.iter().any(|item| &item.account_id == id))
        {
            return Err(AppError::Validation(
                "Seleção difere do pedido confirmado.".to_owned(),
            ));
        }
        for item in request
            .items
            .iter()
            .filter(|item| selected.contains(&item.account_id))
        {
            self.validate_deletion_item(item, request.grant_id.as_deref())?;
            self.ensure_profile_inactive(&self.database.profile(&item.profile_id)?)?;
        }
        for item in request
            .items
            .iter()
            .filter(|item| selected.contains(&item.account_id))
        {
            self.with_profile_mutation(&item.profile_id, || {
                self.validate_deletion_item(item, request.grant_id.as_deref())?;
                self.ensure_profile_inactive(&self.database.profile(&item.profile_id)?)?;
                if remove_browser {
                    self.delete_profile_data(&item.profile_id, true)
                } else {
                    self.unlink_profile_data(&item.profile_id)
                }
            })?;
            request.deleted_ids.push(item.account_id.clone());
            self.deletions.save(request.clone());
        }
        Ok(())
    }
    fn validate_deletion_item(
        &self,
        item: &DeletionItem,
        grant_id: Option<&str>,
    ) -> Result<(), AppError> {
        let profile = self.database.profile(&item.profile_id)?;
        let local = profile
            .account_id
            .as_deref()
            .map(|remote| self.database.account_integration_options(remote))
            .transpose()?
            .map(|options| options.0);
        if local.as_deref() != Some(&item.account_id)
            || profile.updated_at.to_rfc3339() != item.revision
        {
            return Err(AppError::Busy(
                "Conta mudou após a prévia; solicite outra confirmação.".to_owned(),
            ));
        }
        if let Some(id) = grant_id {
            if !self.authorization_grant(id)?.permits(
                Permission::RequestDeletion,
                Some(&item.account_id),
                chrono::Utc::now(),
            ) {
                return Err(AppError::AccessDenied(
                    "Autorização mudou antes da exclusão.".to_owned(),
                ));
            }
        }
        Ok(())
    }
    pub fn deny_deletion(&self, id: &str) -> Result<(), AppError> {
        self.deletions.deny(id)?;
        self.emit_dashboard_changed();
        Ok(())
    }
}

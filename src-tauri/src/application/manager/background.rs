use super::ManagerCore;
use crate::domain::integrations::AuthorizationGrant;
use crate::error::AppError;
use std::sync::atomic::Ordering;
impl ManagerCore {
    pub fn keep_authorized_background(&self) -> bool {
        let Ok(grants) = self.database.grants() else {
            return false;
        };
        let active = grants.iter().any(|grant| {
            !grant.revoked
                && grant
                    .scope
                    .expires_at
                    .is_none_or(|at| chrono::Utc::now() < at)
                && !grant.scope.background_group_ids.is_empty()
        });
        if active {
            self.ui_visible.store(false, Ordering::Release);
            self.executions.cancel_matching(|job| !job.background);
            self.application_servers
                .close_matching(|session| !session.background);
        }
        active
    }
    pub fn mark_ui_visible(&self) {
        self.ui_visible.store(true, Ordering::Release);
    }
    pub fn require_security_context(
        &self,
        grant: &AuthorizationGrant,
        method: &str,
    ) -> Result<(), AppError> {
        if self.ui_visible.load(Ordering::Acquire) {
            return Ok(());
        }
        if !grant.scope.background_group_ids.is_empty()
            && matches!(
                method,
                "execution/start"
                    | "execution/status"
                    | "execution/events"
                    | "execution/ack"
                    | "execution/cancel"
                    | "session/open"
                    | "session/list"
                    | "session/send"
                    | "session/events"
                    | "session/status"
                    | "session/close"
                    | "session/recover"
            )
        {
            return Ok(());
        }
        Err(AppError::ConsentRequired("Abra o gerenciador para esta ação; somente grupos autorizados continuam em segundo plano.".to_owned()))
    }
    pub(super) fn background_binding_allowed(
        &self,
        grant: &AuthorizationGrant,
        target: &crate::domain::integrations::BindingTarget,
    ) -> bool {
        self.ui_visible.load(Ordering::Acquire)
            || matches!(target,crate::domain::integrations::BindingTarget::Group(id)if self.background_group_authorized(grant,id))
    }
    pub(super) fn background_group_authorized(&self, grant: &AuthorizationGrant, id: &str) -> bool {
        grant.scope.background_group_ids.contains(id)
            && self.database.groups().is_ok_and(|groups| {
                groups.iter().any(|group| {
                    group.id == id
                        && !group.archived
                        && !group.paused
                        && group.exclusive_application_id.as_deref() == Some(&grant.application_id)
                })
            })
    }
}

use super::ManagerCore;
use crate::domain::integrations::*;
use crate::error::AppError;

impl ManagerCore {
    pub fn public_integration_catalog(&self) -> Result<serde_json::Value, AppError> {
        let mut accounts = Vec::new();
        for profile in self.database.list_profiles()? {
            if let Some(remote) = profile.account_id.as_deref() {
                let (id, _, _, _) = self.database.account_integration_options(remote)?;
                accounts.push(serde_json::json!({"id":id,"plan":profile.plan_type}));
            }
        }
        let groups = self.database.groups()?;
        Ok(
            serde_json::json!({"apiVersion":2,"accountCount":accounts.len(),"groupCount":groups.len(),"accounts":accounts,
            "groups":groups.into_iter().map(|group|serde_json::json!({"id":group.id,"name":group.name,"accountIds":group.account_ids})).collect::<Vec<_>>()}),
        )
    }
    pub fn integration_snapshot(&self) -> Result<IntegrationSnapshot, AppError> {
        let pending = self.application_credentials.pending_sessions()?;
        let mut accounts = Vec::new();
        for profile in self.list_profiles()? {
            let Some(remote_id) = profile.account_id.as_deref() else {
                continue;
            };
            let (id, policy, capacity, _) = self.database.account_integration_options(remote_id)?;
            let session = self.database.application_session(&id)?;
            accounts.push(IntegrationAccount {
                account_credential_state: profile.credential_state,
                application_session_recovery_pending: session
                    .as_ref()
                    .is_some_and(|session| pending.contains(session)),
                application_session_connected: session.is_some(),
                usage: AccountUsage {
                    observed_at: profile.usage_updated_at,
                    rate_limits: profile.rate_limits,
                },
                id,
                policy,
                capacity,
                profile_id: profile.id,
                name: profile.name,
                email: profile.email,
                plan: profile.plan_type,
            });
        }
        Ok(IntegrationSnapshot {
            accounts,
            groups: self.database.groups()?,
            applications: self.database.applications()?,
            bindings: self.database.bindings()?,
            grants: self.database.grants()?,
        })
    }

    pub fn save_group(&self, group: &AccountGroup) -> Result<AccountGroup, AppError> {
        let saved = self.database.save_group(group)?;
        if saved.paused {
            self.executions
                .cancel_matching(|job| job.group_id.as_deref() == Some(&saved.id));
            self.application_servers
                .close_matching(|server| server.group_id.as_deref() == Some(&saved.id));
        }
        self.emit_dashboard_changed();
        Ok(saved)
    }
    pub fn save_authorized_group(
        &self,
        grant_id: &str,
        group: &AccountGroup,
    ) -> Result<AccountGroup, AppError> {
        let group = self.database.save_authorized_group(grant_id, group)?;
        if group.paused {
            let application = self.authorization_grant(grant_id)?.application_id;
            self.executions.cancel_matching(|job| {
                job.application_id == application && job.group_id.as_deref() == Some(&group.id)
            });
            self.application_servers.close_matching(|server| {
                server.application_id == application
                    && server.group_id.as_deref() == Some(&group.id)
            });
        }
        self.emit_dashboard_changed();
        Ok(group)
    }

    pub fn delete_group(&self, id: &str, revision: u64) -> Result<(), AppError> {
        self.database.delete_group(id, revision)?;
        self.emit_dashboard_changed();
        Ok(())
    }

    pub fn save_application(&self, application: &Application) -> Result<Application, AppError> {
        let saved = self.database.save_application(application)?;
        if saved.paused {
            self.executions
                .cancel_matching(|job| job.application_id == saved.id);
            self.application_servers
                .close_matching(|server| server.application_id == saved.id);
        }
        self.emit_dashboard_changed();
        Ok(saved)
    }

    pub fn save_binding(&self, binding: &ExecutionBinding) -> Result<ExecutionBinding, AppError> {
        let saved = self.database.save_binding(binding)?;
        if saved.paused {
            self.executions
                .cancel_matching(|job| job.binding_id == saved.id);
            self.application_servers
                .close_matching(|server| server.binding_id == saved.id);
        }
        self.emit_dashboard_changed();
        Ok(saved)
    }

    pub fn set_account_policy(
        &self,
        id: &str,
        policy: AccountPolicy,
        capacity: u32,
    ) -> Result<(), AppError> {
        self.database.set_account_policy(id, policy, capacity)?;
        self.emit_dashboard_changed();
        Ok(())
    }

    pub fn save_mail_preference(
        &self,
        profile_id: &str,
        enabled: bool,
        provider_id: Option<&str>,
    ) -> Result<(), AppError> {
        let profile = self.database.profile(profile_id)?;
        let account_id = profile
            .account_id
            .ok_or_else(|| AppError::Validation("Conecte a conta primeiro.".to_owned()))?;
        if let Some(id) = provider_id {
            let provider = self
                .database
                .provider(id)?
                .ok_or_else(|| AppError::Validation("Provedor não encontrado.".to_owned()))?;
            self.database
                .update_profile_mail_url(profile_id, Some(&provider.default_webmail_url))?;
        }
        self.database.set_mail_preference(&account_id, enabled)?;
        self.emit_dashboard_changed();
        Ok(())
    }
}

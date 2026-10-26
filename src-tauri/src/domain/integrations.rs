use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountPolicy {
    #[default]
    DesktopOnly,
    ApplicationsOnly,
    Shared,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationAccount {
    pub account_credential_state: super::CredentialState,
    pub application_session_recovery_pending: bool,
    pub usage: AccountUsage,
    pub id: String,
    pub profile_id: String,
    pub name: String,
    pub email: Option<String>,
    pub plan: Option<String>,
    pub policy: AccountPolicy,
    pub capacity: u32,
    /// Historical wire field: registered session ID, not proof of usable remote auth.
    pub application_session_connected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountUsage {
    pub observed_at: Option<DateTime<Utc>>,
    pub rate_limits: Option<super::RateLimits>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountGroup {
    pub id: String,
    pub name: String,
    pub account_ids: BTreeSet<String>,
    pub archived: bool,
    #[serde(default)]
    pub paused: bool,
    pub revision: u64,
    pub exclusive_application_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Application {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub paused: bool,
    pub revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum BindingTarget {
    Account(String),
    Group(String),
    // Internal route only: neither clients nor persisted legacy bindings may
    // introduce it as another source of authority.
    #[serde(skip)]
    Authorized,
}

/// Local UI setup belongs to the same consent, not to the client's authority.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthorizationSetup {
    #[serde(default)]
    pub shared_account_ids: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionBinding {
    pub id: String,
    pub application_id: String,
    pub name: String,
    pub target: BindingTarget,
    pub automatic: bool,
    pub allow_projected_quota: bool,
    pub allowed_models: BTreeSet<String>,
    pub default_model: Option<String>,
    pub paused: bool,
    pub revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    ReadMetadata,
    Execute,
    ManageGroups,
    ManageAccounts,
    ActivateDesktop,
    ManageBindings,
    OpenBrowser,
    RequestDeletion,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GrantScope {
    pub account_ids: BTreeSet<String>,
    /// New UI consents distinguish accounts chosen directly from the ceiling
    /// approved through groups. Older grants keep their original direct scope.
    #[serde(default)]
    pub direct_account_ids: Option<BTreeSet<String>>,
    pub group_ids: BTreeSet<String>,
    pub permissions: BTreeSet<Permission>,
    pub global_including_future: bool,
    pub background_group_ids: BTreeSet<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub permanent: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorizationGrant {
    pub id: String,
    pub application_id: String,
    pub scope: GrantScope,
    pub verifier: String,
    pub peer_identity: String,
    pub revoked: bool,
    pub created_at: DateTime<Utc>,
}

impl AuthorizationGrant {
    pub fn permits(
        &self,
        permission: Permission,
        account: Option<&str>,
        now: DateTime<Utc>,
    ) -> bool {
        !self.revoked
            && self.scope.expires_at.is_none_or(|expires| now < expires)
            && self.scope.permissions.contains(&permission)
            && account.is_none_or(|id| {
                self.scope.global_including_future || self.scope.account_ids.contains(id)
            })
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationSnapshot {
    pub accounts: Vec<IntegrationAccount>,
    pub groups: Vec<AccountGroup>,
    pub applications: Vec<Application>,
    pub bindings: Vec<ExecutionBinding>,
    pub grants: Vec<AuthorizationGrant>,
}

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
pub mod application_server;
mod connection_check;
pub mod deletion;
pub mod execution;
pub mod integrations;
mod quota;
mod replacement;
mod runtime;
pub use connection_check::ConnectionCheck;
pub use quota::{CreditBalance, RateLimitBucket, RateLimits, ResetCredits, UsageWindow};
pub use replacement::ReplacementPreview;
#[cfg(any(unix, test))]
pub use runtime::RuntimeThread;
pub use runtime::{
    RuntimeConsent, RuntimeRecoveryReport, SharedRuntimeSnapshot, SharedRuntimeState,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProfileStatus {
    Active,
    Archived,
    Deleted,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialState {
    #[default]
    NotConnected,
    Ready,
    Missing,
    Unavailable,
    Quarantined,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialGenerationState {
    Eligible,
    Quarantined,
}

impl TryFrom<&str> for CredentialGenerationState {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "eligible" => Ok(Self::Eligible),
            "quarantined" => Ok(Self::Quarantined),
            other => Err(format!("estado de geração desconhecido: {other}")),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CredentialGeneration {
    pub generation_id: String,
    pub state: CredentialGenerationState,
}

impl CredentialState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotConnected => "not_connected",
            Self::Ready => "ready",
            Self::Missing => "missing",
            Self::Unavailable => "unavailable",
            Self::Quarantined => "quarantined",
        }
    }
}

impl TryFrom<&str> for CredentialState {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "not_connected" => Ok(Self::NotConnected),
            "ready" => Ok(Self::Ready),
            "missing" => Ok(Self::Missing),
            "unavailable" => Ok(Self::Unavailable),
            "quarantined" => Ok(Self::Quarantined),
            other => Err(format!("estado de credencial desconhecido: {other}")),
        }
    }
}

impl ProfileStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Archived => "archived",
            Self::Deleted => "deleted",
        }
    }
}

impl TryFrom<&str> for ProfileStatus {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "active" => Ok(Self::Active),
            "archived" => Ok(Self::Archived),
            "deleted" => Ok(Self::Deleted),
            other => Err(format!("status de perfil desconhecido: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedProfile {
    pub id: String,
    pub name: String,
    pub account_id: Option<String>,
    pub email: Option<String>,
    pub mail_provider_id: Option<String>,
    pub mail_url: Option<String>,
    #[serde(default)]
    pub open_mail_with_login: bool,
    #[serde(skip)]
    pub data_dir: String,
    pub status: ProfileStatus,
    pub last_used_at: Option<DateTime<Utc>>,
    pub last_activated_at: Option<DateTime<Utc>>,
    pub last_disconnected_at: Option<DateTime<Utc>>,
    pub last_browser_opened_at: Option<DateTime<Utc>>,
    pub last_browser_authenticated_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub plan_type: Option<String>,
    pub rate_limits: Option<RateLimits>,
    pub usage_updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub is_active_account: bool,
    #[serde(default)]
    pub needs_activation: bool,
    #[serde(default)]
    pub credential_state: CredentialState,
    #[serde(default = "default_rotation_enabled")]
    pub rotation_enabled: bool,
    #[serde(default)]
    pub preferred: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn default_rotation_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RotationStrategy {
    #[default]
    BeforeReset,
    LargestBalance,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ManagerSettings {
    pub rotation_strategy: RotationStrategy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileOptions {
    pub rotation_enabled: bool,
    pub preferred: bool,
}

impl Default for ProfileOptions {
    fn default() -> Self {
        Self {
            rotation_enabled: true,
            preferred: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AccountMetadata<'a> {
    pub account_id: &'a str,
    pub email: Option<&'a str>,
    pub plan_type: Option<&'a str>,
    pub credential_ref: Option<&'a str>,
    pub rate_limits: Option<&'a RateLimits>,
    pub observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccountHealth {
    Connected,
    Attention,
    Archived,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Recommendation {
    pub label: String,
    pub effective_remaining: f64,
    pub projected_reset: bool,
    pub reason: String,
    pub next_reset_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaPresentation {
    pub remaining: Option<f64>,
    pub projected: bool,
    pub window_duration_mins: Option<u64>,
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountCard {
    pub profile: ManagedProfile,
    pub health: AccountHealth,
    pub attention_reason: Option<String>,
    pub recommendation: Option<Recommendation>,
    pub effective_remaining: Option<f64>,
    pub projected_reset: bool,
    pub primary_quota: Option<QuotaPresentation>,
    pub secondary_quota: Option<QuotaPresentation>,
    pub next_reset_at: Option<DateTime<Utc>>,
    pub available_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardCounts {
    pub connected: usize,
    pub attention: usize,
    pub archived: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardSnapshot {
    #[serde(default)]
    pub revision: u64,
    pub accounts: Vec<AccountCard>,
    pub counts: DashboardCounts,
    pub generated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct AccountSnapshot {
    pub email: Option<String>,
    pub plan_type: Option<String>,
    pub rate_limits: Option<RateLimits>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailProvider {
    pub id: String,
    pub display_name: String,
    pub domains: Vec<String>,
    pub default_webmail_url: String,
    pub aliases: Vec<String>,
    pub is_custom: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveMailProviderInput {
    pub id: Option<String>,
    pub display_name: String,
    pub domains: Vec<String>,
    pub default_webmail_url: String,
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    Idle,
    Preparing,
    BrowserLaunching,
    AwaitingLogin,
    AwaitingConfirmation,
    Verifying,
    Persisting,
    Completed,
    Cancelled,
    Failed,
    TimedOut,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    #[default]
    Login,
    Reconnect,
    Switch,
    Replace,
}

impl OperationKind {
    pub fn is_cancelable_in(self, state: &OperationState) -> bool {
        if state.is_terminal() || matches!(state, OperationState::Persisting) {
            return false;
        }
        self != Self::Switch || matches!(state, OperationState::Idle | OperationState::Preparing)
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Login => "login",
            Self::Reconnect => "reconnect",
            Self::Switch => "switch",
            Self::Replace => "replace",
        }
    }
}

impl TryFrom<&str> for OperationKind {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "login" => Ok(Self::Login),
            "reconnect" => Ok(Self::Reconnect),
            "switch" => Ok(Self::Switch),
            "replace" => Ok(Self::Replace),
            other => Err(format!("tipo de operação desconhecido: {other}")),
        }
    }
}

impl OperationState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Preparing => "preparing",
            Self::BrowserLaunching => "browser_launching",
            Self::AwaitingLogin => "awaiting_login",
            Self::AwaitingConfirmation => "awaiting_confirmation",
            Self::Verifying => "verifying",
            Self::Persisting => "persisting",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
            Self::TimedOut => "timed_out",
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Cancelled | Self::Failed | Self::TimedOut
        )
    }
}

impl TryFrom<&str> for OperationState {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "idle" => Ok(Self::Idle),
            "preparing" => Ok(Self::Preparing),
            "browser_launching" => Ok(Self::BrowserLaunching),
            "awaiting_login" => Ok(Self::AwaitingLogin),
            "awaiting_confirmation" => Ok(Self::AwaitingConfirmation),
            "verifying" => Ok(Self::Verifying),
            "persisting" => Ok(Self::Persisting),
            "completed" => Ok(Self::Completed),
            "cancelled" => Ok(Self::Cancelled),
            "failed" => Ok(Self::Failed),
            "timed_out" => Ok(Self::TimedOut),
            other => Err(format!("estado de operação desconhecido: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginOperation {
    pub id: String,
    pub profile_id: String,
    pub kind: OperationKind,
    pub reconnect: bool,
    pub state: OperationState,
    pub step: String,
    pub started_at: DateTime<Utc>,
    pub stage_started_at: DateTime<Utc>,
    pub last_stage_duration_ms: Option<i64>,
    pub finished_at: Option<DateTime<Utc>>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub browser_pid: Option<u32>,
    pub runtime_kind: Option<String>,
    pub duration_ms: Option<i64>,
    pub cancelable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationEvent {
    pub operation_id: String,
    pub profile_id: String,
    pub kind: OperationKind,
    pub state: OperationState,
    pub step: String,
    pub message: String,
    pub error_code: Option<String>,
    pub stage_duration_ms: Option<i64>,
    pub cancelable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OperationStage {
    pub name: String,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub runtime_kind: String,
    pub version: Option<String>,
    pub installed: bool,
    pub repair_required: bool,
    pub download_size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchResult {
    pub previous_account_id: Option<String>,
    pub active_account_id: String,
    pub changed: bool,
    pub desktop_restarted: bool,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub current_version: String,
    pub version: String,
    pub body: Option<String>,
    pub published_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProgress {
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::{CredentialState, ManagedProfile, ProfileStatus};

    #[test]
    fn frontend_models_do_not_expose_local_filesystem_paths() {
        let now = Utc::now();
        let profile = ManagedProfile {
            id: "profile-a".to_owned(),
            name: "Conta".to_owned(),
            account_id: None,
            email: None,
            mail_provider_id: None,
            mail_url: None,
            open_mail_with_login: false,
            data_dir: "/private/profile-a".to_owned(),
            status: ProfileStatus::Active,
            last_used_at: None,
            last_activated_at: None,
            last_disconnected_at: None,
            last_browser_opened_at: None,
            last_browser_authenticated_at: None,
            last_error: None,
            plan_type: None,
            rate_limits: None,
            usage_updated_at: None,
            is_active_account: false,
            needs_activation: false,
            credential_state: CredentialState::NotConnected,
            rotation_enabled: true,
            preferred: false,
            created_at: now,
            updated_at: now,
        };
        let profile_json = serde_json::to_value(profile).expect("profile JSON");
        assert!(profile_json.get("dataDir").is_none());
    }
}

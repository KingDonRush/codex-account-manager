use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenServerRequest {
    #[serde(default)]
    pub binding_id: String,
    pub account_id: Option<String>,
    pub group_id: Option<String>,
    pub cwd: String,
    #[serde(default)]
    pub persist_history: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationServerStatus {
    #[serde(default)]
    pub events_offset: u64,
    #[serde(default)]
    pub allowed_models: std::collections::BTreeSet<String>,
    #[serde(default)]
    pub default_model: Option<String>,
    pub id: String,
    pub application_id: String,
    pub grant_id: String,
    pub binding_id: String,
    pub binding_revision: u64,
    #[serde(default)]
    pub group_id: Option<String>,
    pub account_id: String,
    pub state: ServerState,
    pub background: bool,
    pub persist_history: bool,
    pub created_at: DateTime<Utc>,
    pub last_activity: DateTime<Utc>,
    pub error_code: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerState {
    Preparing,
    Ready,
    Generating,
    Closing,
    Closed,
    Failed,
    Interrupted,
}

impl ServerState {
    pub fn terminal(self) -> bool {
        matches!(self, Self::Closed | Self::Failed | Self::Interrupted)
    }
}

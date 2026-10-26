use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecRequest {
    #[serde(default)]
    pub binding_id: String,
    #[serde(skip)]
    pub binding_snapshot: Option<crate::domain::integrations::ExecutionBinding>,
    pub account_id: Option<String>,
    pub group_id: Option<String>,
    pub cwd: String,
    pub prompt: String,
    #[serde(default)]
    pub arguments: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionState {
    Queued,
    Preparing,
    Running,
    Cancelling,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

impl ExecutionState {
    pub fn terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionStatus {
    pub id: String,
    pub application_id: String,
    pub grant_id: String,
    pub binding_id: String,
    pub binding_revision: u64,
    #[serde(default)]
    pub group_id: Option<String>,
    pub account_id: Option<String>,
    pub state: ExecutionState,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub error_code: Option<String>,
    pub exit_code: Option<i32>,
    pub background: bool,
}

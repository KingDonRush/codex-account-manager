use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeletionState {
    AwaitingConfirmation,
    Confirming,
    Completed,
    Denied,
    Failed,
}
impl DeletionState {
    pub fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Denied | Self::Failed)
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletionItem {
    pub account_id: String,
    pub profile_id: String,
    pub name: String,
    pub email: Option<String>,
    pub revision: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletionRequest {
    pub id: String,
    pub requester: String,
    pub grant_id: Option<String>,
    pub items: Vec<DeletionItem>,
    pub state: DeletionState,
    pub deleted_ids: Vec<String>,
    pub error_code: Option<String>,
}

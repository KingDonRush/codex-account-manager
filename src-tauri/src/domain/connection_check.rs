use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionCheck {
    pub confirmed: bool,
    pub can_renew: bool,
    pub title: String,
    pub guidance: String,
    pub error_code: Option<String>,
    pub checked_at: DateTime<Utc>,
}

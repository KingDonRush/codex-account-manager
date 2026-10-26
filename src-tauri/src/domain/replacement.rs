use serde::Serialize;

/// Identity metadata only: the candidate credential stays in the login worker.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplacementPreview {
    pub previous_email: Option<String>,
    pub next_email: Option<String>,
    pub next_plan: Option<String>,
}

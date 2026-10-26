use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SharedRuntimeState {
    Absent,
    Idle,
    Busy,
    Unavailable,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeThread {
    pub id: String,
    pub name: Option<String>,
    pub state: String,
    pub active_goal: bool,
    pub background_terminals: usize,
}

impl RuntimeThread {
    #[cfg(unix)]
    pub fn is_idle(&self) -> bool {
        self.state == "idle" && !self.active_goal && self.background_terminals == 0
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedRuntimeSnapshot {
    pub state: SharedRuntimeState,
    pub pid: Option<u32>,
    pub threads: Vec<RuntimeThread>,
    pub external_calls: Vec<u32>,
    pub external_inspection_supported: bool,
    pub explanation: String,
}

impl SharedRuntimeSnapshot {
    pub fn unavailable(explanation: impl Into<String>) -> Self {
        Self {
            state: SharedRuntimeState::Unavailable,
            pid: None,
            threads: Vec::new(),
            external_calls: Vec::new(),
            external_inspection_supported: cfg!(target_os = "linux"),
            explanation: explanation.into(),
        }
    }
    pub fn absent() -> Self {
        Self {
            state: SharedRuntimeState::Absent,
            pid: None,
            threads: Vec::new(),
            external_calls: Vec::new(),
            external_inspection_supported: cfg!(target_os = "linux"),
            explanation: "Nenhum daemon compartilhado em execução.".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeRecoveryReport {
    pub recovered: bool,
    pub needs_confirmation: bool,
    pub explanation: String,
    pub affected_threads: usize,
    pub confirmation: Option<RuntimeConsent>,
}

/// Consent is bound to a specific process incarnation and complete thread set.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeConsent {
    pub pid: u32,
    pub started_ticks: u64,
    pub thread_ids: Vec<String>,
}

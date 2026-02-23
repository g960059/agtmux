use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionState {
    None,
    InformationalCompleted,
    ActionRequiredInput,
    ActionRequiredApproval,
    ActionRequiredError,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttentionResult {
    pub state: AttentionState,
    pub reason: String,
    pub since: Option<DateTime<Utc>>,
}

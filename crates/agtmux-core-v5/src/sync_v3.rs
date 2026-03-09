use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::{FreshnessState, PaneInstanceId, Provider, SourceKind};

pub const UI_BOOTSTRAP_V3_VERSION: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresenceV3 {
    Managed,
    Unmanaged,
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentLifecycleV3 {
    Unknown,
    PendingInit,
    Running,
    Completed,
    Errored,
    Shutdown,
    NotFound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadLifecycleV3 {
    NotLoaded,
    Active,
    Idle,
    Interrupted,
    Errored,
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadBlockingV3 {
    None,
    WaitingUserInput,
    WaitingApproval,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadExecutionV3 {
    None,
    Thinking,
    Streaming,
    ToolRunning,
    Compacting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnOutcomeV3 {
    None,
    Completed,
    Aborted,
    Errored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingRequestKindV3 {
    Approval,
    UserInput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingRequestStatusV3 {
    Pending,
    Resolved,
    Dismissed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionKindV3 {
    Question,
    Approval,
    Error,
    Completion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionPriorityV3 {
    None,
    Question,
    Approval,
    Error,
    Completion,
}

impl AttentionPriorityV3 {
    pub fn from_active_kinds(kinds: &[AttentionKindV3]) -> Self {
        if kinds
            .iter()
            .any(|kind| matches!(kind, AttentionKindV3::Error))
        {
            Self::Error
        } else if kinds
            .iter()
            .any(|kind| matches!(kind, AttentionKindV3::Approval))
        {
            Self::Approval
        } else if kinds
            .iter()
            .any(|kind| matches!(kind, AttentionKindV3::Question))
        {
            Self::Question
        } else if kinds
            .iter()
            .any(|kind| matches!(kind, AttentionKindV3::Completion))
        {
            Self::Completion
        } else {
            Self::None
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentStateV3 {
    pub lifecycle: AgentLifecycleV3,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadFlagsV3 {
    pub review_mode: bool,
    pub subagent_active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnStateV3 {
    pub outcome: TurnOutcomeV3,
    pub sequence: Option<u64>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadStateV3 {
    pub lifecycle: ThreadLifecycleV3,
    pub blocking: ThreadBlockingV3,
    pub execution: ThreadExecutionV3,
    pub flags: ThreadFlagsV3,
    pub turn: TurnStateV3,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingRequestSourceV3 {
    pub provider: Provider,
    pub source_kind: SourceKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingRequestV3 {
    pub request_id: String,
    pub kind: PendingRequestKindV3,
    pub title: Option<String>,
    pub detail: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub status: PendingRequestStatusV3,
    pub source: PendingRequestSourceV3,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttentionSummaryV3 {
    pub active_kinds: Vec<AttentionKindV3>,
    pub highest_priority: AttentionPriorityV3,
    pub unresolved_count: u32,
    pub generation: u64,
    pub latest_at: Option<DateTime<Utc>>,
}

impl AttentionSummaryV3 {
    pub fn none() -> Self {
        Self {
            active_kinds: Vec::new(),
            highest_priority: AttentionPriorityV3::None,
            unresolved_count: 0,
            generation: 0,
            latest_at: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreshnessSummaryV3 {
    pub snapshot: FreshnessState,
    pub blocking: FreshnessState,
    pub execution: FreshnessState,
}

impl FreshnessSummaryV3 {
    pub fn fresh() -> Self {
        Self {
            snapshot: FreshnessState::Fresh,
            blocking: FreshnessState::Fresh,
            execution: FreshnessState::Fresh,
        }
    }

    pub fn is_degraded(&self) -> bool {
        !matches!(
            (self.snapshot, self.blocking, self.execution),
            (
                FreshnessState::Fresh,
                FreshnessState::Fresh,
                FreshnessState::Fresh
            )
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProviderRawEnvelopeV3 {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codex: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gemini: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copilot: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncV3PaneSnapshot {
    pub session_name: String,
    pub window_id: String,
    pub session_key: String,
    pub pane_id: String,
    pub pane_instance_id: PaneInstanceId,
    pub provider: Option<Provider>,
    pub presence: PresenceV3,
    pub agent: AgentStateV3,
    pub thread: ThreadStateV3,
    pub pending_requests: Vec<PendingRequestV3>,
    pub attention: AttentionSummaryV3,
    pub freshness: FreshnessSummaryV3,
    pub provider_raw: ProviderRawEnvelopeV3,
    pub updated_at: DateTime<Utc>,
}

impl SyncV3PaneSnapshot {
    pub fn validate(&self) -> Result<(), String> {
        for (field_name, value) in [
            ("session_name", self.session_name.as_str()),
            ("window_id", self.window_id.as_str()),
            ("session_key", self.session_key.as_str()),
            ("pane_id", self.pane_id.as_str()),
        ] {
            if value.is_empty() {
                return Err(format!(
                    "required identity field {field_name} must not be empty"
                ));
            }
        }

        if self.pane_instance_id.pane_id != self.pane_id {
            return Err("pane_instance_id.pane_id must match pane_id".to_string());
        }

        if self
            .pending_requests
            .iter()
            .any(|request| request.request_id.is_empty())
        {
            return Err("pending request_id must not be empty".to_string());
        }

        if self
            .pending_requests
            .iter()
            .any(|request| !matches!(request.status, PendingRequestStatusV3::Pending))
        {
            return Err("bootstrap pending_requests must contain only status=pending".to_string());
        }

        let approval_count = self
            .pending_requests
            .iter()
            .filter(|request| matches!(request.kind, PendingRequestKindV3::Approval))
            .count();
        let question_count = self
            .pending_requests
            .iter()
            .filter(|request| matches!(request.kind, PendingRequestKindV3::UserInput))
            .count();

        match self.thread.blocking {
            ThreadBlockingV3::WaitingApproval if approval_count == 0 => {
                return Err("waiting_approval requires at least one approval request".to_string());
            }
            ThreadBlockingV3::WaitingUserInput if question_count == 0 => {
                return Err(
                    "waiting_user_input requires at least one user_input request".to_string(),
                );
            }
            _ => {}
        }

        if self.attention.unresolved_count != self.pending_requests.len() as u32 {
            return Err("attention.unresolved_count must equal pending request count".to_string());
        }

        let expected_priority =
            AttentionPriorityV3::from_active_kinds(&self.attention.active_kinds);
        if expected_priority != self.attention.highest_priority {
            return Err("attention.highest_priority must match active_kinds".to_string());
        }

        if approval_count > 0
            && !self
                .attention
                .active_kinds
                .iter()
                .any(|kind| matches!(kind, AttentionKindV3::Approval))
        {
            return Err("approval request requires attention kind=approval".to_string());
        }

        if question_count > 0
            && !self
                .attention
                .active_kinds
                .iter()
                .any(|kind| matches!(kind, AttentionKindV3::Question))
        {
            return Err("user_input request requires attention kind=question".to_string());
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiBootstrapV3 {
    pub version: u32,
    pub generated_at: DateTime<Utc>,
    pub panes: Vec<SyncV3PaneSnapshot>,
}

impl UiBootstrapV3 {
    pub fn new(generated_at: DateTime<Utc>, panes: Vec<SyncV3PaneSnapshot>) -> Self {
        Self {
            version: UI_BOOTSTRAP_V3_VERSION,
            generated_at,
            panes,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.version != UI_BOOTSTRAP_V3_VERSION {
            return Err(format!(
                "unexpected ui.bootstrap.v3 version: {}",
                self.version
            ));
        }

        for pane in &self.panes {
            pane.validate()?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn fixture_path(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/sync-v3")
            .join(name)
    }

    fn parse_fixture(name: &str) -> UiBootstrapV3 {
        let body = fs::read_to_string(fixture_path(name)).expect("fixture must be readable");
        let payload: UiBootstrapV3 = serde_json::from_str(&body).expect("fixture must parse");
        payload.validate().expect("fixture must validate");
        payload
    }

    #[test]
    fn bootstrap_version_is_frozen() {
        let payload = parse_fixture("codex-running.json");
        assert_eq!(payload.version, UI_BOOTSTRAP_V3_VERSION);
        assert_eq!(payload.panes.len(), 1);
    }

    #[test]
    fn all_sync_v3_fixtures_parse_and_validate() {
        for name in [
            "codex-running.json",
            "codex-waiting-approval.json",
            "codex-completed-idle.json",
            "claude-approval.json",
            "claude-stop-idle.json",
            "unmanaged-demotion.json",
            "error.json",
            "freshness-degraded.json",
        ] {
            parse_fixture(name);
        }
    }

    #[test]
    fn missing_required_identity_field_is_rejected() {
        let mut value =
            serde_json::to_value(parse_fixture("codex-running.json")).expect("serialize fixture");
        value["panes"][0]
            .as_object_mut()
            .expect("pane object")
            .remove("pane_id");

        let err = serde_json::from_value::<UiBootstrapV3>(value).expect_err("missing pane_id");
        assert!(err.to_string().contains("pane_id"));
    }

    #[test]
    fn unknown_enum_value_is_rejected() {
        let mut value =
            serde_json::to_value(parse_fixture("codex-running.json")).expect("serialize fixture");
        value["panes"][0]["thread"]["blocking"] = serde_json::json!("waiting_review");

        let err =
            serde_json::from_value::<UiBootstrapV3>(value).expect_err("unknown enum must fail");
        assert!(err.to_string().contains("waiting_review"));
    }

    #[test]
    fn attention_priority_prefers_error_then_approval_then_question_then_completion() {
        assert_eq!(
            AttentionPriorityV3::from_active_kinds(&[
                AttentionKindV3::Completion,
                AttentionKindV3::Question,
                AttentionKindV3::Approval,
            ]),
            AttentionPriorityV3::Approval
        );
        assert_eq!(
            AttentionPriorityV3::from_active_kinds(&[
                AttentionKindV3::Completion,
                AttentionKindV3::Error,
            ]),
            AttentionPriorityV3::Error
        );
    }

    #[test]
    fn waiting_approval_requires_pending_approval_request() {
        let mut payload = parse_fixture("codex-waiting-approval.json");
        payload.panes[0].pending_requests.clear();
        let err = payload
            .validate()
            .expect_err("blocking must match requests");
        assert!(err.contains("waiting_approval"));
    }
}

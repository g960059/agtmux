use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

// ─── Provider & Source ────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Provider {
    Claude,
    Codex,
    Gemini,
    Copilot,
}

impl Provider {
    pub const ALL: [Self; 4] = [Self::Claude, Self::Codex, Self::Gemini, Self::Copilot];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Gemini => "gemini",
            Self::Copilot => "copilot",
        }
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Provider {
    type Err = AgtmuxError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "claude" => Ok(Self::Claude),
            "codex" => Ok(Self::Codex),
            "gemini" => Ok(Self::Gemini),
            "copilot" => Ok(Self::Copilot),
            _ => Err(AgtmuxError::InvalidSourceEvent(format!(
                "unknown provider: {s}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SourceKind {
    CodexAppserver,
    ClaudeHooks,
    ClaudeJsonl,
    Poller,
}

impl SourceKind {
    /// Map source kind to its evidence tier.
    pub fn tier(self) -> EvidenceTier {
        match self {
            Self::CodexAppserver | Self::ClaudeHooks | Self::ClaudeJsonl => {
                EvidenceTier::Deterministic
            }
            Self::Poller => EvidenceTier::Heuristic,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::CodexAppserver => "codex_appserver",
            Self::ClaudeHooks => "claude_hooks",
            Self::ClaudeJsonl => "claude_jsonl",
            Self::Poller => "poller",
        }
    }
}

impl fmt::Display for SourceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ─── Evidence & Tier ──────────────────────────────────────────────

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EvidenceTier {
    Deterministic,
    #[default]
    Heuristic,
}

// ─── Presence & Mode ──────────────────────────────────────────────

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PanePresence {
    Managed,
    #[default]
    Unmanaged,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EvidenceMode {
    Deterministic,
    Heuristic,
    #[default]
    None,
}

// ─── Activity ─────────────────────────────────────────────────────

#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[repr(u8)]
#[non_exhaustive]
pub enum ActivityState {
    #[default]
    Unknown = 0,
    Idle = 1,
    Running = 2,
    WaitingInput = 3,
    WaitingApproval = 4,
    Error = 5,
}

impl ActivityState {
    /// Precedence order (descending): higher-priority states take precedence.
    pub const PRECEDENCE_DESC: [Self; 6] = [
        Self::Error,
        Self::WaitingApproval,
        Self::WaitingInput,
        Self::Running,
        Self::Idle,
        Self::Unknown,
    ];
}

// ─── Pane Signature ───────────────────────────────────────────────

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PaneSignatureClass {
    Deterministic,
    Heuristic,
    #[default]
    None,
}

// ─── Source Health ─────────────────────────────────────────────────

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceHealthStatus {
    Healthy,
    Degraded,
    #[default]
    Down,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FreshnessState {
    Fresh,
    Stale,
    #[default]
    Down,
}

// ─── Event ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceEventV2 {
    pub event_id: String,
    pub provider: Provider,
    pub source_kind: SourceKind,
    pub tier: EvidenceTier,
    pub observed_at: DateTime<Utc>,
    pub session_key: String,
    pub pane_id: Option<String>,
    pub pane_generation: Option<u64>,
    pub pane_birth_ts: Option<DateTime<Utc>>,
    pub source_event_id: Option<String>,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub confidence: f64,
}

// ─── Identity ─────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PaneInstanceId {
    pub pane_id: String,
    pub generation: u64,
    pub birth_ts: DateTime<Utc>,
}

// ─── Runtime State ────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRuntimeState {
    pub session_key: String,
    pub presence: PanePresence,
    pub evidence_mode: EvidenceMode,
    pub deterministic_last_seen: Option<DateTime<Utc>>,
    pub winner_tier: EvidenceTier,
    pub activity_state: ActivityState,
    pub activity_source: SourceKind,
    pub representative_pane_instance_id: Option<PaneInstanceId>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneRuntimeState {
    pub pane_instance_id: PaneInstanceId,
    pub presence: PanePresence,
    pub evidence_mode: EvidenceMode,
    pub signature_class: PaneSignatureClass,
    pub signature_reason: String,
    pub signature_confidence: f64,
    pub no_agent_streak: u32,
    /// Compact representation of the signals that produced the current signature.
    /// Exposed in the client API for observability.
    pub signature_inputs: SignatureInputsCompact,
    /// Per-pane activity state (Running/Idle/WaitingApproval etc.).
    pub activity_state: ActivityState,
    /// Detected provider for this pane (None if unmanaged or not yet determined).
    pub provider: Option<Provider>,
    /// Session key that owns this pane (for title resolution and session lookup).
    pub session_key: String,
    pub updated_at: DateTime<Utc>,
}

/// Client-visible compact form of signature inputs (heuristic signal booleans only).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureInputsCompact {
    pub provider_hint: bool,
    pub cmd_match: bool,
    pub poller_match: bool,
    pub title_match: bool,
}

// ─── Cursor ───────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceCursorState {
    pub source_kind: SourceKind,
    pub committed_cursor: Option<String>,
    pub checkpoint_ts: DateTime<Utc>,
}

// ─── Source Health Report ─────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceHealthReport {
    pub status: SourceHealthStatus,
    pub checked_at: DateTime<Utc>,
}

// ─── Protocol: Source <-> Gateway ─────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullEventsRequest {
    pub cursor: Option<String>,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PullEventsResponse {
    pub events: Vec<SourceEventV2>,
    pub next_cursor: Option<String>,
    pub heartbeat_ts: DateTime<Utc>,
    pub source_health: SourceHealthReport,
}

// ─── Protocol: Gateway <-> Daemon ─────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayPullRequest {
    pub cursor: Option<String>,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GatewayPullResponse {
    pub events: Vec<SourceEventV2>,
    pub next_cursor: Option<String>,
}

// ─── Error ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgtmuxError {
    InvalidSourceEvent(String),
    MissingEventTime,
    SourceInadmissible(String),
    SourceRankSuppressed {
        source_kind: SourceKind,
        suppressed_by: SourceKind,
    },
    LateEvent {
        event_id: String,
    },
    BindingConflict {
        pane_id: String,
    },
    SignatureInconclusive,
    SignatureGuardRejected(String),
}

impl fmt::Display for AgtmuxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSourceEvent(msg) => write!(f, "invalid source event: {msg}"),
            Self::MissingEventTime => write!(f, "missing event time"),
            Self::SourceInadmissible(msg) => write!(f, "source inadmissible: {msg}"),
            Self::SourceRankSuppressed {
                source_kind,
                suppressed_by,
            } => {
                write!(f, "source {source_kind} suppressed by {suppressed_by}")
            }
            Self::LateEvent { event_id } => write!(f, "late event: {event_id}"),
            Self::BindingConflict { pane_id } => write!(f, "binding conflict: pane {pane_id}"),
            Self::SignatureInconclusive => write!(f, "signature inconclusive"),
            Self::SignatureGuardRejected(msg) => write!(f, "signature guard rejected: {msg}"),
        }
    }
}

impl std::error::Error for AgtmuxError {}

// ─── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_serde_roundtrip() {
        for p in Provider::ALL {
            let json = serde_json::to_string(&p).expect("serialize");
            let back: Provider = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(p, back);
        }
    }

    #[test]
    fn source_kind_tier_mapping() {
        assert_eq!(
            SourceKind::CodexAppserver.tier(),
            EvidenceTier::Deterministic
        );
        assert_eq!(SourceKind::ClaudeHooks.tier(), EvidenceTier::Deterministic);
        assert_eq!(SourceKind::Poller.tier(), EvidenceTier::Heuristic);
    }

    #[test]
    fn provider_display_and_parse() {
        for p in Provider::ALL {
            let s = p.to_string();
            let parsed = s.parse::<Provider>().expect("parse");
            assert_eq!(p, parsed);
        }
    }

    #[test]
    fn evidence_mode_default_is_none() {
        assert_eq!(EvidenceMode::default(), EvidenceMode::None);
    }

    #[test]
    fn pane_presence_default_is_unmanaged() {
        assert_eq!(PanePresence::default(), PanePresence::Unmanaged);
    }

    #[test]
    fn activity_state_precedence_order() {
        let prec = ActivityState::PRECEDENCE_DESC;
        assert_eq!(prec[0], ActivityState::Error);
        assert_eq!(prec[5], ActivityState::Unknown);
    }

    #[test]
    fn source_event_serde_roundtrip() {
        let event = SourceEventV2 {
            event_id: "evt-001".into(),
            provider: Provider::Codex,
            source_kind: SourceKind::CodexAppserver,
            tier: EvidenceTier::Deterministic,
            observed_at: Utc::now(),
            session_key: "sess-001".into(),
            pane_id: Some("%1".into()),
            pane_generation: Some(1),
            pane_birth_ts: Some(Utc::now()),
            source_event_id: Some("src-evt-001".into()),
            event_type: "lifecycle.running".into(),
            payload: serde_json::json!({"status": "running"}),
            confidence: 1.0,
        };
        let json = serde_json::to_string(&event).expect("serialize");
        let back: SourceEventV2 = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event.event_id, back.event_id);
        assert_eq!(event.provider, back.provider);
        assert_eq!(event.source_kind, back.source_kind);
    }

    #[test]
    fn error_display() {
        let err = AgtmuxError::SourceRankSuppressed {
            source_kind: SourceKind::Poller,
            suppressed_by: SourceKind::CodexAppserver,
        };
        let msg = err.to_string();
        assert!(msg.contains("poller"));
        assert!(msg.contains("codex_appserver"));
    }
}

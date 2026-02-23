pub mod loader;
pub mod providers;

use crate::types::{Evidence, PaneMeta, Provider};
use chrono::{DateTime, Utc};

/// Detect whether a pane belongs to this provider.
pub trait ProviderDetector: Send + Sync {
    fn id(&self) -> Provider;
    fn detect(&self, meta: &PaneMeta) -> Option<f64>;
}

/// Build evidence from PaneMeta (evaluation-time).
pub trait EvidenceBuilder: Send + Sync {
    fn provider(&self) -> Provider;
    fn build_evidence(&self, meta: &PaneMeta, now: DateTime<Utc>) -> Vec<Evidence>;
}

/// Normalize raw signals from hooks/API into canonical events.
/// Only providers with hook/API support implement this.
pub trait EventNormalizer: Send + Sync {
    fn provider(&self) -> Provider;
    fn normalize(&self, signal: &RawSignal) -> Option<NormalizedState>;
}

/// Raw signal from a source (hook, API, etc.)
#[derive(Debug, Clone)]
pub struct RawSignal {
    pub event_type: String,
    pub payload: String,
    pub pane_id: String,
}

/// Normalized state output from EventNormalizer.
#[derive(Debug, Clone)]
pub struct NormalizedState {
    pub state: crate::types::ActivityState,
    pub reason_code: String,
    pub confidence: String,
}

/// Detect provider by matching agent_type, current_cmd, or pane_title.
pub fn detect_by_agent_or_cmd(
    meta: &PaneMeta,
    agent_type_match: &str,
    cmd_tokens: &[&str],
) -> Option<f64> {
    let agent_lower = meta.agent_type.to_lowercase();
    if agent_lower == agent_type_match {
        return Some(1.0);
    }

    let cmd_lower = meta.current_cmd.to_lowercase();
    if cmd_tokens.iter().any(|t| cmd_lower.contains(t)) {
        return Some(0.86);
    }

    let title_lower = meta.pane_title.to_lowercase();
    let label_lower = meta.session_label.to_lowercase();
    if title_lower.contains(agent_type_match) || label_lower.contains(agent_type_match) {
        return Some(0.66);
    }

    None
}

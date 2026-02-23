pub mod loader;
pub mod providers;

use crate::types::{Evidence, EvidenceKind, PaneMeta, Provider, SourceType};
use chrono::{DateTime, Utc};
use std::time::Duration;

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
    pub provider: Provider,
    pub state: crate::types::ActivityState,
    pub reason_code: String,
    pub confidence: f64,
    pub weight: f64,
    pub ttl: Duration,
}

impl NormalizedState {
    pub fn to_evidence(&self, source: SourceType, now: DateTime<Utc>) -> Evidence {
        let kind = match source {
            SourceType::Hook => EvidenceKind::HookEvent(self.reason_code.clone()),
            SourceType::Api => EvidenceKind::ApiNotification(self.reason_code.clone()),
            SourceType::File => EvidenceKind::FileChange(self.reason_code.clone()),
            _ => EvidenceKind::PollerMatch(self.reason_code.clone()),
        };
        Evidence {
            provider: self.provider,
            kind,
            signal: self.state,
            weight: self.weight,
            confidence: self.confidence,
            timestamp: now,
            ttl: self.ttl,
            source,
            reason_code: self.reason_code.clone(),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ActivityState;

    #[test]
    fn to_evidence_hook_source_produces_hook_event_kind() {
        let ns = NormalizedState {
            provider: Provider::Claude,
            state: ActivityState::Running,
            reason_code: "hook:running".into(),
            confidence: 0.94,
            weight: 0.90,
            ttl: Duration::from_secs(90),
        };
        let ev = ns.to_evidence(SourceType::Hook, Utc::now());
        assert_eq!(ev.provider, Provider::Claude);
        assert_eq!(ev.signal, ActivityState::Running);
        assert_eq!(ev.source, SourceType::Hook);
        assert!(matches!(ev.kind, EvidenceKind::HookEvent(_)));
        assert!((ev.confidence - 0.94).abs() < f64::EPSILON);
        assert!((ev.weight - 0.90).abs() < f64::EPSILON);
        assert_eq!(ev.ttl, Duration::from_secs(90));
    }

    #[test]
    fn to_evidence_api_source_produces_api_notification_kind() {
        let ns = NormalizedState {
            provider: Provider::Codex,
            state: ActivityState::WaitingApproval,
            reason_code: "api:approval".into(),
            confidence: 0.97,
            weight: 0.98,
            ttl: Duration::from_secs(180),
        };
        let ev = ns.to_evidence(SourceType::Api, Utc::now());
        assert_eq!(ev.source, SourceType::Api);
        assert!(matches!(ev.kind, EvidenceKind::ApiNotification(_)));
    }

    #[test]
    fn to_evidence_file_source_produces_file_change_kind() {
        let ns = NormalizedState {
            provider: Provider::Claude,
            state: ActivityState::Idle,
            reason_code: "file:idle".into(),
            confidence: 0.90,
            weight: 0.85,
            ttl: Duration::from_secs(90),
        };
        let ev = ns.to_evidence(SourceType::File, Utc::now());
        assert_eq!(ev.source, SourceType::File);
        assert!(matches!(ev.kind, EvidenceKind::FileChange(_)));
    }
}

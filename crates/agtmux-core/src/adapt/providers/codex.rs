use crate::adapt::{
    detect_by_agent_or_cmd, EventNormalizer, EvidenceBuilder, NormalizedState, ProviderDetector,
    RawSignal,
};
use crate::types::{ActivityState, Evidence, EvidenceKind, PaneMeta, Provider, SourceType};
use chrono::{DateTime, Utc};
use std::time::Duration;

pub struct CodexDetector;

impl ProviderDetector for CodexDetector {
    fn id(&self) -> Provider {
        Provider::Codex
    }

    fn detect(&self, meta: &PaneMeta) -> Option<f64> {
        detect_by_agent_or_cmd(meta, "codex", &["codex"])
    }
}

pub struct CodexEvidenceBuilder;

impl EvidenceBuilder for CodexEvidenceBuilder {
    fn provider(&self) -> Provider {
        Provider::Codex
    }

    fn build_evidence(&self, meta: &PaneMeta, now: DateTime<Utc>) -> Vec<Evidence> {
        let combined = format!(
            "{} {} {}",
            meta.raw_state, meta.raw_reason_code, meta.last_event_type
        )
        .to_lowercase();

        let mut evidence = Vec::new();
        let has = |tokens: &[&str]| tokens.iter().any(|t| combined.contains(t));

        if has(&["approval-requested"]) {
            evidence.push(Evidence {
                provider: Provider::Codex,
                kind: EvidenceKind::PollerMatch("approval".into()),
                signal: ActivityState::WaitingApproval,
                weight: 0.97,
                confidence: 0.95,
                timestamp: now,
                ttl: Duration::from_secs(180),
                source: SourceType::Poller,
                reason_code: meta.raw_reason_code.clone(),
            });
        }

        if has(&["input-requested"]) {
            evidence.push(Evidence {
                provider: Provider::Codex,
                kind: EvidenceKind::PollerMatch("input".into()),
                signal: ActivityState::WaitingInput,
                weight: 0.94,
                confidence: 0.92,
                timestamp: now,
                ttl: Duration::from_secs(180),
                source: SourceType::Poller,
                reason_code: meta.raw_reason_code.clone(),
            });
        }

        if has(&["error", "failed", "panic", "exception"]) {
            evidence.push(Evidence {
                provider: Provider::Codex,
                kind: EvidenceKind::PollerMatch("error".into()),
                signal: ActivityState::Error,
                weight: 1.00,
                confidence: 0.95,
                timestamp: now,
                ttl: Duration::from_secs(180),
                source: SourceType::Poller,
                reason_code: meta.raw_reason_code.clone(),
            });
        }

        if has(&["running", "wrapper_start"]) {
            evidence.push(Evidence {
                provider: Provider::Codex,
                kind: EvidenceKind::PollerMatch("running".into()),
                signal: ActivityState::Running,
                weight: 0.92,
                confidence: 0.88,
                timestamp: now,
                ttl: Duration::from_secs(90),
                source: SourceType::Poller,
                reason_code: meta.raw_reason_code.clone(),
            });
        }

        if has(&["idle", "completed", "done"]) {
            evidence.push(Evidence {
                provider: Provider::Codex,
                kind: EvidenceKind::PollerMatch("idle".into()),
                signal: ActivityState::Idle,
                weight: 0.88,
                confidence: 0.86,
                timestamp: now,
                ttl: Duration::from_secs(90),
                source: SourceType::Poller,
                reason_code: meta.raw_reason_code.clone(),
            });
        }

        evidence
    }
}

pub struct CodexNormalizer;

impl EventNormalizer for CodexNormalizer {
    fn provider(&self) -> Provider {
        Provider::Codex
    }

    fn normalize(&self, signal: &RawSignal) -> Option<NormalizedState> {
        let event = signal.event_type.to_lowercase();
        let payload = signal.payload.to_lowercase();

        if event.contains("approval-requested") || payload.contains("approval-requested") {
            return Some(NormalizedState {
                provider: Provider::Codex,
                state: ActivityState::WaitingApproval,
                reason_code: "api:approval".into(),
                confidence: 0.97,
            });
        }

        if event.contains("input-requested") || payload.contains("input-requested") {
            return Some(NormalizedState {
                provider: Provider::Codex,
                state: ActivityState::WaitingInput,
                reason_code: "api:input".into(),
                confidence: 0.96,
            });
        }

        if event.contains("error") || event.contains("failed") {
            return Some(NormalizedState {
                provider: Provider::Codex,
                state: ActivityState::Error,
                reason_code: "api:error".into(),
                confidence: 0.98,
            });
        }

        if event.contains("running") || event.contains("wrapper_start") {
            return Some(NormalizedState {
                provider: Provider::Codex,
                state: ActivityState::Running,
                reason_code: "api:running".into(),
                confidence: 0.94,
            });
        }

        if event.contains("idle") || event.contains("completed") || event.contains("done") {
            return Some(NormalizedState {
                provider: Provider::Codex,
                state: ActivityState::Idle,
                reason_code: "api:idle".into(),
                confidence: 0.92,
            });
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_signal(event_type: &str, payload: &str) -> RawSignal {
        RawSignal {
            event_type: event_type.into(),
            payload: payload.into(),
            pane_id: "%1".into(),
        }
    }

    #[test]
    fn normalizes_approval_requested() {
        let n = CodexNormalizer;
        let result = n.normalize(&make_signal("approval-requested", "{}")).unwrap();
        assert_eq!(result.state, ActivityState::WaitingApproval);
        assert_eq!(result.provider, Provider::Codex);
        assert!(result.confidence > 0.9);
    }

    #[test]
    fn normalizes_input_requested() {
        let n = CodexNormalizer;
        let result = n.normalize(&make_signal("input-requested", "{}")).unwrap();
        assert_eq!(result.state, ActivityState::WaitingInput);
    }

    #[test]
    fn normalizes_error() {
        let n = CodexNormalizer;
        let result = n.normalize(&make_signal("error", "{}")).unwrap();
        assert_eq!(result.state, ActivityState::Error);
    }

    #[test]
    fn normalizes_running() {
        let n = CodexNormalizer;
        let result = n.normalize(&make_signal("running", "{}")).unwrap();
        assert_eq!(result.state, ActivityState::Running);
    }

    #[test]
    fn normalizes_idle() {
        let n = CodexNormalizer;
        let result = n.normalize(&make_signal("idle", "{}")).unwrap();
        assert_eq!(result.state, ActivityState::Idle);
    }

    #[test]
    fn returns_none_for_unknown_event() {
        let n = CodexNormalizer;
        assert!(n.normalize(&make_signal("unknown_xyz", "{}")).is_none());
    }

    #[test]
    fn approval_in_payload_triggers_normalization() {
        let n = CodexNormalizer;
        let result = n.normalize(&make_signal("some_event", "approval-requested")).unwrap();
        assert_eq!(result.state, ActivityState::WaitingApproval);
    }
}

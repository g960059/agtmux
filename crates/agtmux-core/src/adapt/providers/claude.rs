use crate::adapt::{
    detect_by_agent_or_cmd, EventNormalizer, EvidenceBuilder, NormalizedState, ProviderDetector,
    RawSignal,
};
use crate::types::{ActivityState, Evidence, EvidenceKind, PaneMeta, Provider, SourceType};
use chrono::{DateTime, Utc};
use std::time::Duration;

pub struct ClaudeDetector;

impl ProviderDetector for ClaudeDetector {
    fn id(&self) -> Provider {
        Provider::Claude
    }

    fn detect(&self, meta: &PaneMeta) -> Option<f64> {
        detect_by_agent_or_cmd(meta, "claude", &["claude", "claude-code", "cc"])
    }
}

pub struct ClaudeEvidenceBuilder;

impl EvidenceBuilder for ClaudeEvidenceBuilder {
    fn provider(&self) -> Provider {
        Provider::Claude
    }

    fn build_evidence(&self, meta: &PaneMeta, now: DateTime<Utc>) -> Vec<Evidence> {
        let combined = format!(
            "{} {} {}",
            meta.raw_state, meta.raw_reason_code, meta.last_event_type
        )
        .to_lowercase();

        let mut evidence = Vec::new();

        let has = |tokens: &[&str]| tokens.iter().any(|t| combined.contains(t));

        if has(&["approval", "waiting_approval", "needs_approval", "permission"]) {
            evidence.push(Evidence {
                provider: Provider::Claude,
                kind: EvidenceKind::PollerMatch("approval".into()),
                signal: ActivityState::WaitingApproval,
                weight: 0.95,
                confidence: 0.92,
                timestamp: now,
                ttl: Duration::from_secs(180),
                source: SourceType::Poller,
                reason_code: meta.raw_reason_code.clone(),
            });
        }

        if has(&["waiting_input", "input_required", "await_user", "prompt"]) {
            evidence.push(Evidence {
                provider: Provider::Claude,
                kind: EvidenceKind::PollerMatch("waiting_input".into()),
                signal: ActivityState::WaitingInput,
                weight: 0.90,
                confidence: 0.88,
                timestamp: now,
                ttl: Duration::from_secs(180),
                source: SourceType::Poller,
                reason_code: meta.raw_reason_code.clone(),
            });
        }

        if has(&["error", "failed", "panic", "exception"]) {
            evidence.push(Evidence {
                provider: Provider::Claude,
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

        if has(&[
            "working",
            "running",
            "in_progress",
            "streaming",
            "task_started",
            "agent_turn_started",
            "pretooluse",
        ]) {
            evidence.push(Evidence {
                provider: Provider::Claude,
                kind: EvidenceKind::PollerMatch("running".into()),
                signal: ActivityState::Running,
                weight: 0.90,
                confidence: 0.86,
                timestamp: now,
                ttl: Duration::from_secs(90),
                source: SourceType::Poller,
                reason_code: meta.raw_reason_code.clone(),
            });
        }

        if has(&["idle", "completed", "done", "stop", "session_end"]) {
            evidence.push(Evidence {
                provider: Provider::Claude,
                kind: EvidenceKind::PollerMatch("idle".into()),
                signal: ActivityState::Idle,
                weight: 0.88,
                confidence: 0.88,
                timestamp: now,
                ttl: Duration::from_secs(90),
                source: SourceType::Poller,
                reason_code: meta.raw_reason_code.clone(),
            });
        }

        evidence
    }
}

pub struct ClaudeNormalizer;

impl EventNormalizer for ClaudeNormalizer {
    fn provider(&self) -> Provider {
        Provider::Claude
    }

    fn normalize(&self, signal: &RawSignal) -> Option<NormalizedState> {
        let event = signal.event_type.to_lowercase();
        let payload = signal.payload.to_lowercase();

        if event.contains("approval") || payload.contains("needs_approval") {
            return Some(NormalizedState {
                provider: Provider::Claude,
                state: ActivityState::WaitingApproval,
                reason_code: "hook:approval".into(),
                confidence: 0.96,
                weight: 0.98,
                ttl: Duration::from_secs(180),
            });
        }

        if event.contains("waiting_input") || event.contains("input_required") {
            return Some(NormalizedState {
                provider: Provider::Claude,
                state: ActivityState::WaitingInput,
                reason_code: "hook:waiting_input".into(),
                confidence: 0.94,
                weight: 0.92,
                ttl: Duration::from_secs(120),
            });
        }

        if event.contains("error") || event.contains("failed") {
            return Some(NormalizedState {
                provider: Provider::Claude,
                state: ActivityState::Error,
                reason_code: "hook:error".into(),
                confidence: 0.98,
                weight: 1.0,
                ttl: Duration::from_secs(180),
            });
        }

        if event.contains("running")
            || event.contains("agent_turn_started")
            || event.contains("tool-execution")
            || event.contains("streaming")
        {
            return Some(NormalizedState {
                provider: Provider::Claude,
                state: ActivityState::Running,
                reason_code: "hook:running".into(),
                confidence: 0.94,
                weight: 0.90,
                ttl: Duration::from_secs(90),
            });
        }

        if event.contains("idle") || event.contains("session_end") || event.contains("completed") {
            return Some(NormalizedState {
                provider: Provider::Claude,
                state: ActivityState::Idle,
                reason_code: "hook:idle".into(),
                confidence: 0.92,
                weight: 0.85,
                ttl: Duration::from_secs(90),
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
    fn normalizes_approval_event() {
        let n = ClaudeNormalizer;
        let result = n.normalize(&make_signal("approval", "{}")).unwrap();
        assert_eq!(result.state, ActivityState::WaitingApproval);
        assert_eq!(result.provider, Provider::Claude);
        assert!(result.confidence > 0.9);
    }

    #[test]
    fn normalizes_waiting_input_event() {
        let n = ClaudeNormalizer;
        let result = n.normalize(&make_signal("waiting_input", "{}")).unwrap();
        assert_eq!(result.state, ActivityState::WaitingInput);
    }

    #[test]
    fn normalizes_error_event() {
        let n = ClaudeNormalizer;
        let result = n.normalize(&make_signal("error", "{}")).unwrap();
        assert_eq!(result.state, ActivityState::Error);
        assert!(result.confidence > 0.95);
    }

    #[test]
    fn normalizes_running_event() {
        let n = ClaudeNormalizer;
        let result = n.normalize(&make_signal("running", "{}")).unwrap();
        assert_eq!(result.state, ActivityState::Running);
    }

    #[test]
    fn normalizes_tool_execution_event() {
        let n = ClaudeNormalizer;
        let result = n.normalize(&make_signal("tool-execution", "{}")).unwrap();
        assert_eq!(result.state, ActivityState::Running);
    }

    #[test]
    fn normalizes_idle_event() {
        let n = ClaudeNormalizer;
        let result = n.normalize(&make_signal("idle", "{}")).unwrap();
        assert_eq!(result.state, ActivityState::Idle);
    }

    #[test]
    fn normalizes_session_end_event() {
        let n = ClaudeNormalizer;
        let result = n.normalize(&make_signal("session_end", "{}")).unwrap();
        assert_eq!(result.state, ActivityState::Idle);
    }

    #[test]
    fn returns_none_for_unknown_event() {
        let n = ClaudeNormalizer;
        assert!(n.normalize(&make_signal("unknown_xyz", "{}")).is_none());
    }

    #[test]
    fn approval_in_payload_triggers_normalization() {
        let n = ClaudeNormalizer;
        let result = n.normalize(&make_signal("some_event", "needs_approval")).unwrap();
        assert_eq!(result.state, ActivityState::WaitingApproval);
    }
}

use crate::adapt::{detect_by_agent_or_cmd, EvidenceBuilder, ProviderDetector};
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

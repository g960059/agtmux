use crate::adapt::{detect_by_agent_or_cmd, EvidenceBuilder, ProviderDetector};
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

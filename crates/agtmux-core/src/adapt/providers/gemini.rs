use crate::adapt::{detect_by_agent_or_cmd, EvidenceBuilder, ProviderDetector};
use crate::types::{ActivityState, Evidence, EvidenceKind, PaneMeta, Provider, SourceType};
use chrono::{DateTime, Utc};
use std::time::Duration;

pub struct GeminiDetector;

impl ProviderDetector for GeminiDetector {
    fn id(&self) -> Provider { Provider::Gemini }

    fn detect(&self, meta: &PaneMeta) -> Option<f64> {
        detect_by_agent_or_cmd(meta, "gemini", &["gemini"])
    }
}

pub struct GeminiEvidenceBuilder;

impl EvidenceBuilder for GeminiEvidenceBuilder {
    fn provider(&self) -> Provider { Provider::Gemini }

    fn build_evidence(&self, meta: &PaneMeta, now: DateTime<Utc>) -> Vec<Evidence> {
        let combined = format!("{} {} {}", meta.raw_state, meta.raw_reason_code, meta.last_event_type).to_lowercase();
        let mut evidence = Vec::new();

        if combined.contains("running") {
            evidence.push(Evidence {
                provider: Provider::Gemini,
                kind: EvidenceKind::PollerMatch("running".into()),
                signal: ActivityState::Running,
                weight: 0.78,
                confidence: 0.80,
                timestamp: now,
                ttl: Duration::from_secs(90),
                source: SourceType::Poller,
                reason_code: meta.raw_reason_code.clone(),
            });
        }

        evidence
    }
}

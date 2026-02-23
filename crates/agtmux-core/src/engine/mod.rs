use crate::types::{ActivityState, Evidence, SourceType};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const EPSILON: f64 = 1e-9;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineConfig {
    pub min_score: f64,
    pub running_enter_score: f64,
    pub min_stable_duration_ms: u64,
    pub default_evidence_ttl_secs: u64,
    pub high_confidence_ttl_secs: u64,
    pub low_confidence_ttl_secs: u64,
    pub strong_source_bonus: f64,
    pub weak_source_multiplier: f64,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            min_score: 0.35,
            running_enter_score: 0.62,
            min_stable_duration_ms: 1500,
            default_evidence_ttl_secs: 90,
            high_confidence_ttl_secs: 180,
            low_confidence_ttl_secs: 30,
            strong_source_bonus: 0.15,
            weak_source_multiplier: 0.75,
        }
    }
}

/// Output of Engine::resolve().
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedActivity {
    pub state: ActivityState,
    pub confidence: f64,
    pub source: SourceType,
    pub reason_code: String,
}

/// Pure state engine: takes evidence, outputs resolved activity.
/// No knowledge of PaneMeta, providers, or terminal backends.
pub struct Engine {
    pub config: EngineConfig,
}

impl Engine {
    pub fn new(config: EngineConfig) -> Self {
        Self { config }
    }

    /// Resolve the winning activity from a set of evidence.
    pub fn resolve(&self, evidence: &[Evidence], now: DateTime<Utc>) -> ResolvedActivity {
        use std::collections::HashMap;

        let default_ttl = Duration::from_secs(self.config.default_evidence_ttl_secs);

        // 1. Filter expired evidence, accumulate scores per activity
        let mut scores: HashMap<ActivityState, (f64, SourceType, String)> = HashMap::new();

        for ev in evidence {
            let age = now.signed_duration_since(ev.timestamp);
            if age.num_milliseconds() < 0 {
                continue; // future evidence, skip
            }
            let ttl_ms = ev.ttl.as_millis() as i64;
            if ttl_ms > 0 && age.num_milliseconds() > ttl_ms {
                continue; // expired
            }
            if default_ttl.as_millis() > 0
                && ev.ttl.as_millis() == 0
                && age.num_milliseconds() > default_ttl.as_millis() as i64
            {
                continue; // expired with default TTL
            }

            // 2. Score = weight * confidence, with source bonus/penalty
            let mut score = ev.weight * ev.confidence;
            match ev.source {
                SourceType::Hook | SourceType::Api => {
                    score += self.config.strong_source_bonus;
                }
                SourceType::Poller => {
                    score *= self.config.weak_source_multiplier;
                }
                _ => {}
            }

            let entry = scores
                .entry(ev.signal)
                .or_insert((0.0, ev.source, ev.reason_code.clone()));
            entry.0 += score;
            // Keep the strongest source
            if matches!(ev.source, SourceType::Hook | SourceType::Api) {
                entry.1 = ev.source;
            }
        }

        if scores.is_empty() {
            return ResolvedActivity {
                state: ActivityState::Unknown,
                confidence: 0.0,
                source: SourceType::Poller,
                reason_code: String::new(),
            };
        }

        // 3. Sort by precedence DESC, score DESC
        let mut candidates: Vec<_> = scores.into_iter().collect();
        candidates.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| b.1 .0.partial_cmp(&a.1 .0).unwrap_or(std::cmp::Ordering::Equal))
        });

        let (state, (score, source, reason_code)) = candidates.into_iter().next().unwrap();

        // 4. Threshold checks
        if score < self.config.min_score - EPSILON {
            return ResolvedActivity {
                state: ActivityState::Unknown,
                confidence: score,
                source,
                reason_code,
            };
        }

        if state == ActivityState::Running && score < self.config.running_enter_score - EPSILON {
            return ResolvedActivity {
                state: ActivityState::Unknown,
                confidence: score,
                source,
                reason_code,
            };
        }

        ResolvedActivity {
            state,
            confidence: score,
            source,
            reason_code,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{EvidenceKind, Provider};

    fn make_evidence(
        signal: ActivityState,
        weight: f64,
        confidence: f64,
        source: SourceType,
    ) -> Evidence {
        Evidence {
            provider: Provider::Claude,
            kind: EvidenceKind::PollerMatch("test".into()),
            signal,
            weight,
            confidence,
            timestamp: Utc::now(),
            ttl: Duration::from_secs(90),
            source,
            reason_code: String::new(),
        }
    }

    #[test]
    fn empty_evidence_yields_unknown() {
        let engine = Engine::new(EngineConfig::default());
        let result = engine.resolve(&[], Utc::now());
        assert_eq!(result.state, ActivityState::Unknown);
    }

    #[test]
    fn error_wins_over_running() {
        let engine = Engine::new(EngineConfig::default());
        let evidence = vec![
            make_evidence(ActivityState::Running, 0.90, 0.86, SourceType::Hook),
            make_evidence(ActivityState::Error, 1.00, 0.95, SourceType::Hook),
        ];
        let result = engine.resolve(&evidence, Utc::now());
        assert_eq!(result.state, ActivityState::Error);
    }

    #[test]
    fn expired_evidence_is_ignored() {
        let engine = Engine::new(EngineConfig::default());
        let mut ev = make_evidence(ActivityState::Running, 0.90, 0.86, SourceType::Hook);
        ev.timestamp = Utc::now() - chrono::Duration::seconds(200);
        ev.ttl = Duration::from_secs(90);

        let result = engine.resolve(&[ev], Utc::now());
        assert_eq!(result.state, ActivityState::Unknown);
    }

    #[test]
    fn hook_source_gets_bonus() {
        let engine = Engine::new(EngineConfig::default());
        let ev = make_evidence(ActivityState::Running, 0.90, 0.86, SourceType::Hook);
        let result = engine.resolve(&[ev], Utc::now());
        // score = 0.90 * 0.86 + 0.15 (bonus) = 0.924
        assert_eq!(result.state, ActivityState::Running);
        assert!(result.confidence > 0.9);
    }

    #[test]
    fn poller_running_below_threshold() {
        let engine = Engine::new(EngineConfig::default());
        // score = 0.78 * 0.80 * 0.75 (weak multiplier) = 0.468 < 0.62
        let ev = make_evidence(ActivityState::Running, 0.78, 0.80, SourceType::Poller);
        let result = engine.resolve(&[ev], Utc::now());
        assert_eq!(result.state, ActivityState::Unknown);
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use crate::types::{EvidenceKind, Provider};
    use proptest::prelude::*;

    fn arb_activity() -> impl Strategy<Value = ActivityState> {
        prop_oneof![
            Just(ActivityState::Unknown),
            Just(ActivityState::Idle),
            Just(ActivityState::Running),
            Just(ActivityState::WaitingInput),
            Just(ActivityState::WaitingApproval),
            Just(ActivityState::Error),
        ]
    }

    fn arb_provider() -> impl Strategy<Value = Provider> {
        prop_oneof![
            Just(Provider::Claude),
            Just(Provider::Codex),
            Just(Provider::Gemini),
            Just(Provider::Copilot),
        ]
    }

    fn arb_source() -> impl Strategy<Value = SourceType> {
        prop_oneof![
            Just(SourceType::Hook),
            Just(SourceType::Api),
            Just(SourceType::File),
            Just(SourceType::Poller),
        ]
    }

    fn arb_evidence() -> impl Strategy<Value = Evidence> {
        (
            arb_provider(),
            arb_activity(),
            0.0f64..=1.0,
            0.0f64..=1.0,
            arb_source(),
            1u64..200,
        )
            .prop_map(|(provider, signal, weight, confidence, source, ttl_secs)| Evidence {
                provider,
                kind: EvidenceKind::PollerMatch("proptest".into()),
                signal,
                weight,
                confidence,
                timestamp: Utc::now(),
                ttl: Duration::from_secs(ttl_secs),
                source,
                reason_code: "proptest".into(),
            })
    }

    proptest! {
        /// Invariant 1: Empty evidence always yields Unknown.
        #[test]
        fn empty_evidence_is_unknown(_ in 0u32..100) {
            let engine = Engine::new(EngineConfig::default());
            let result = engine.resolve(&[], Utc::now());
            prop_assert_eq!(result.state, ActivityState::Unknown);
        }

        /// Invariant 2: All evidence expired → Unknown.
        #[test]
        fn expired_evidence_is_unknown(
            evidence in proptest::collection::vec(arb_evidence(), 1..10),
            delay_secs in 300u64..1000,
        ) {
            let engine = Engine::new(EngineConfig::default());
            let now = Utc::now() + chrono::Duration::seconds(delay_secs as i64);
            let result = engine.resolve(&evidence, now);
            prop_assert_eq!(result.state, ActivityState::Unknown);
        }

        /// Invariant 3: Error evidence (high weight, from Hook) always wins.
        #[test]
        fn error_always_wins(
            mut evidence in proptest::collection::vec(arb_evidence(), 0..5),
        ) {
            let engine = Engine::new(EngineConfig::default());
            let now = Utc::now();
            evidence.push(Evidence {
                provider: Provider::Claude,
                kind: EvidenceKind::HookEvent("error".into()),
                signal: ActivityState::Error,
                weight: 1.0,
                confidence: 0.95,
                timestamp: now,
                ttl: Duration::from_secs(180),
                source: SourceType::Hook,
                reason_code: "error".into(),
            });
            let result = engine.resolve(&evidence, now);
            prop_assert_eq!(result.state, ActivityState::Error);
        }

        /// Invariant 4: Confidence/score is always non-negative.
        #[test]
        fn confidence_non_negative(
            evidence in proptest::collection::vec(arb_evidence(), 0..20),
        ) {
            let engine = Engine::new(EngineConfig::default());
            let now = Utc::now();
            let result = engine.resolve(&evidence, now);
            prop_assert!(result.confidence >= 0.0,
                "confidence was {}", result.confidence);
        }
    }
}

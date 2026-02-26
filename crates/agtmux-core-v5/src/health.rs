//! Source health finite state machine.
//!
//! Extracted from v4 `agtmux-daemon/src/source_probe.rs` as a pure,
//! side-effect-free module.  The [`transition_health`] function is the
//! single entry point for all state changes.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default probe interval (seconds).
pub const DEFAULT_PROBE_INTERVAL_SECS: u64 = 5;

/// Default probe timeout (milliseconds).
pub const DEFAULT_PROBE_TIMEOUT_MS: u64 = 250;

/// Grace period for freshness calculation (milliseconds).
pub const FRESHNESS_GRACE_MS: u64 = 250;

/// Calculate the freshness window: probe_interval + probe_timeout + grace.
#[must_use]
pub fn freshness_window_secs() -> f64 {
    #[expect(clippy::cast_precision_loss)]
    let interval = DEFAULT_PROBE_INTERVAL_SECS as f64;
    #[expect(clippy::cast_precision_loss)]
    let timeout_secs = DEFAULT_PROBE_TIMEOUT_MS as f64 / 1000.0;
    #[expect(clippy::cast_precision_loss)]
    let grace_secs = FRESHNESS_GRACE_MS as f64 / 1000.0;
    interval + timeout_secs + grace_secs
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Source health states (6-state FSM).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceHealthState {
    #[default]
    Unknown,
    Healthy,
    Degraded,
    Unhealthy,
    Recovering,
    Disabled,
}

/// Probe signal input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeSignal {
    Success,
    Timeout,
    Error,
    Panic,
    Disabled,
}

/// Health tracking state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceHealth {
    pub state: SourceHealthState,
    pub reason: String,
    pub checked_at: DateTime<Utc>,
    pub consecutive_failures: u32,
    pub consecutive_successes: u32,
}

impl SourceHealth {
    /// Create an initial `Unknown` health record.
    #[must_use]
    pub fn unknown(now: DateTime<Utc>) -> Self {
        Self {
            state: SourceHealthState::Unknown,
            reason: "not probed yet".to_string(),
            checked_at: now,
            consecutive_failures: 0,
            consecutive_successes: 0,
        }
    }

    /// Returns `true` only when the source is [`SourceHealthState::Healthy`].
    #[must_use]
    pub fn is_admissible(&self) -> bool {
        self.state == SourceHealthState::Healthy
    }
}

/// Configurable thresholds for health transitions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthPolicy {
    /// Number of consecutive failures before transitioning to Unhealthy.
    pub failure_threshold: u32,
    /// Number of consecutive successes before recovering to Healthy.
    pub recovery_threshold: u32,
}

impl Default for HealthPolicy {
    fn default() -> Self {
        Self {
            failure_threshold: 2,
            recovery_threshold: 2,
        }
    }
}

// ---------------------------------------------------------------------------
// Transition function
// ---------------------------------------------------------------------------

/// Pure state machine: transition source health based on probe signal.
///
/// When `previous` is `None`, the source is treated as being in the
/// [`SourceHealthState::Unknown`] state.
#[must_use]
pub fn transition_health(
    previous: Option<&SourceHealth>,
    signal: ProbeSignal,
    policy: &HealthPolicy,
    now: DateTime<Utc>,
) -> SourceHealth {
    let previous = previous
        .cloned()
        .unwrap_or_else(|| SourceHealth::unknown(now));

    // Disabled is terminal: once disabled, always disabled.
    if previous.state == SourceHealthState::Disabled || signal == ProbeSignal::Disabled {
        return SourceHealth {
            state: SourceHealthState::Disabled,
            reason: "source disabled".to_string(),
            checked_at: now,
            consecutive_failures: 0,
            consecutive_successes: 0,
        };
    }

    // Ensure thresholds are at least 1 to avoid immediate promotion.
    let failure_threshold = policy.failure_threshold.max(1);
    let recovery_threshold = policy.recovery_threshold.max(1);

    match signal {
        ProbeSignal::Success => {
            let consecutive_successes = previous.consecutive_successes.saturating_add(1);
            let state = match previous.state {
                SourceHealthState::Unhealthy | SourceHealthState::Recovering => {
                    if consecutive_successes >= recovery_threshold {
                        SourceHealthState::Healthy
                    } else {
                        SourceHealthState::Recovering
                    }
                }
                _ => SourceHealthState::Healthy,
            };
            let reason = if state == SourceHealthState::Recovering {
                format!("probe recovering ({consecutive_successes}/{recovery_threshold})")
            } else {
                "probe succeeded".to_string()
            };

            SourceHealth {
                state,
                reason,
                checked_at: now,
                consecutive_failures: 0,
                consecutive_successes,
            }
        }
        ProbeSignal::Timeout | ProbeSignal::Error | ProbeSignal::Panic => {
            let consecutive_failures = previous.consecutive_failures.saturating_add(1);
            let state = if consecutive_failures >= failure_threshold {
                SourceHealthState::Unhealthy
            } else {
                SourceHealthState::Degraded
            };

            SourceHealth {
                state,
                reason: failure_reason(signal).to_string(),
                checked_at: now,
                consecutive_failures,
                consecutive_successes: 0,
            }
        }
        ProbeSignal::Disabled => unreachable!("disabled is handled above"),
    }
}

/// Map a failure signal to a human-readable reason string.
fn failure_reason(signal: ProbeSignal) -> &'static str {
    match signal {
        ProbeSignal::Timeout => "probe timeout",
        ProbeSignal::Error => "probe error",
        ProbeSignal::Panic => "probe panic",
        _ => "probe failure",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::DateTime;

    /// Helper: parse an RFC 3339 timestamp.
    fn ts(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .expect("valid RFC3339 timestamp")
            .with_timezone(&Utc)
    }

    fn now() -> DateTime<Utc> {
        ts("2026-02-25T00:00:00Z")
    }

    // -- Unknown transitions --

    #[test]
    fn unknown_plus_success_becomes_healthy() {
        let h = transition_health(None, ProbeSignal::Success, &HealthPolicy::default(), now());
        assert_eq!(h.state, SourceHealthState::Healthy);
        assert_eq!(h.consecutive_successes, 1);
    }

    #[test]
    fn unknown_plus_timeout_becomes_degraded() {
        let h = transition_health(None, ProbeSignal::Timeout, &HealthPolicy::default(), now());
        assert_eq!(h.state, SourceHealthState::Degraded);
        assert_eq!(h.consecutive_failures, 1);
    }

    #[test]
    fn unknown_plus_error_becomes_degraded() {
        let h = transition_health(None, ProbeSignal::Error, &HealthPolicy::default(), now());
        assert_eq!(h.state, SourceHealthState::Degraded);
    }

    #[test]
    fn unknown_plus_panic_becomes_degraded() {
        let h = transition_health(None, ProbeSignal::Panic, &HealthPolicy::default(), now());
        assert_eq!(h.state, SourceHealthState::Degraded);
    }

    #[test]
    fn unknown_plus_disabled_becomes_disabled() {
        let h = transition_health(None, ProbeSignal::Disabled, &HealthPolicy::default(), now());
        assert_eq!(h.state, SourceHealthState::Disabled);
    }

    // -- Healthy transitions --

    #[test]
    fn healthy_plus_success_stays_healthy() {
        let prev = SourceHealth {
            state: SourceHealthState::Healthy,
            reason: "probe succeeded".to_string(),
            checked_at: now(),
            consecutive_failures: 0,
            consecutive_successes: 5,
        };
        let h = transition_health(
            Some(&prev),
            ProbeSignal::Success,
            &HealthPolicy::default(),
            now(),
        );
        assert_eq!(h.state, SourceHealthState::Healthy);
        assert_eq!(h.consecutive_successes, 6);
    }

    #[test]
    fn healthy_plus_error_becomes_degraded_resets_successes() {
        let prev = SourceHealth {
            state: SourceHealthState::Healthy,
            reason: "probe succeeded".to_string(),
            checked_at: now(),
            consecutive_failures: 0,
            consecutive_successes: 3,
        };
        let h = transition_health(
            Some(&prev),
            ProbeSignal::Error,
            &HealthPolicy::default(),
            now(),
        );
        assert_eq!(h.state, SourceHealthState::Degraded);
        assert_eq!(h.consecutive_failures, 1);
        assert_eq!(h.consecutive_successes, 0);
    }

    #[test]
    fn healthy_plus_timeout_becomes_degraded() {
        let prev = SourceHealth {
            state: SourceHealthState::Healthy,
            reason: "probe succeeded".to_string(),
            checked_at: now(),
            consecutive_failures: 0,
            consecutive_successes: 1,
        };
        let h = transition_health(
            Some(&prev),
            ProbeSignal::Timeout,
            &HealthPolicy::default(),
            now(),
        );
        assert_eq!(h.state, SourceHealthState::Degraded);
        assert_eq!(h.consecutive_failures, 1);
    }

    #[test]
    fn healthy_plus_panic_becomes_degraded() {
        let prev = SourceHealth {
            state: SourceHealthState::Healthy,
            reason: "probe succeeded".to_string(),
            checked_at: now(),
            consecutive_failures: 0,
            consecutive_successes: 1,
        };
        let h = transition_health(
            Some(&prev),
            ProbeSignal::Panic,
            &HealthPolicy::default(),
            now(),
        );
        assert_eq!(h.state, SourceHealthState::Degraded);
    }

    // -- Degraded transitions --

    #[test]
    fn degraded_plus_error_below_threshold_stays_degraded() {
        let prev = SourceHealth {
            state: SourceHealthState::Degraded,
            reason: "probe error".to_string(),
            checked_at: now(),
            consecutive_failures: 0,
            consecutive_successes: 0,
        };
        let policy = HealthPolicy {
            failure_threshold: 3,
            recovery_threshold: 2,
        };
        let h = transition_health(Some(&prev), ProbeSignal::Error, &policy, now());
        assert_eq!(h.state, SourceHealthState::Degraded);
        assert_eq!(h.consecutive_failures, 1);
    }

    #[test]
    fn degraded_plus_success_becomes_healthy() {
        let prev = SourceHealth {
            state: SourceHealthState::Degraded,
            reason: "probe error".to_string(),
            checked_at: now(),
            consecutive_failures: 1,
            consecutive_successes: 0,
        };
        let h = transition_health(
            Some(&prev),
            ProbeSignal::Success,
            &HealthPolicy::default(),
            now(),
        );
        assert_eq!(h.state, SourceHealthState::Healthy);
    }

    // -- Failure threshold counting --

    #[test]
    fn two_failures_reaches_unhealthy() {
        let policy = HealthPolicy::default();
        let first = transition_health(None, ProbeSignal::Error, &policy, now());
        assert_eq!(first.state, SourceHealthState::Degraded);
        assert_eq!(first.consecutive_failures, 1);

        let second = transition_health(Some(&first), ProbeSignal::Error, &policy, now());
        assert_eq!(second.state, SourceHealthState::Unhealthy);
        assert_eq!(second.consecutive_failures, 2);
    }

    #[test]
    fn three_failures_with_threshold_3() {
        let policy = HealthPolicy {
            failure_threshold: 3,
            recovery_threshold: 2,
        };
        let first = transition_health(None, ProbeSignal::Error, &policy, now());
        assert_eq!(first.state, SourceHealthState::Degraded);

        let second = transition_health(Some(&first), ProbeSignal::Error, &policy, now());
        assert_eq!(second.state, SourceHealthState::Degraded);

        let third = transition_health(Some(&second), ProbeSignal::Error, &policy, now());
        assert_eq!(third.state, SourceHealthState::Unhealthy);
    }

    // -- Unhealthy transitions --

    #[test]
    fn unhealthy_plus_success_becomes_recovering() {
        let prev = SourceHealth {
            state: SourceHealthState::Unhealthy,
            reason: "probe error".to_string(),
            checked_at: now(),
            consecutive_failures: 3,
            consecutive_successes: 0,
        };
        let h = transition_health(
            Some(&prev),
            ProbeSignal::Success,
            &HealthPolicy::default(),
            now(),
        );
        assert_eq!(h.state, SourceHealthState::Recovering);
        assert_eq!(h.consecutive_failures, 0);
        assert_eq!(h.consecutive_successes, 1);
    }

    #[test]
    fn unhealthy_plus_error_stays_unhealthy() {
        let prev = SourceHealth {
            state: SourceHealthState::Unhealthy,
            reason: "probe error".to_string(),
            checked_at: now(),
            consecutive_failures: 3,
            consecutive_successes: 0,
        };
        let h = transition_health(
            Some(&prev),
            ProbeSignal::Error,
            &HealthPolicy::default(),
            now(),
        );
        assert_eq!(h.state, SourceHealthState::Unhealthy);
    }

    #[test]
    fn unhealthy_plus_timeout_stays_unhealthy() {
        let prev = SourceHealth {
            state: SourceHealthState::Unhealthy,
            reason: "probe error".to_string(),
            checked_at: now(),
            consecutive_failures: 3,
            consecutive_successes: 0,
        };
        let h = transition_health(
            Some(&prev),
            ProbeSignal::Timeout,
            &HealthPolicy::default(),
            now(),
        );
        assert_eq!(h.state, SourceHealthState::Unhealthy);
    }

    #[test]
    fn unhealthy_plus_panic_stays_unhealthy() {
        let prev = SourceHealth {
            state: SourceHealthState::Unhealthy,
            reason: "probe error".to_string(),
            checked_at: now(),
            consecutive_failures: 3,
            consecutive_successes: 0,
        };
        let h = transition_health(
            Some(&prev),
            ProbeSignal::Panic,
            &HealthPolicy::default(),
            now(),
        );
        assert_eq!(h.state, SourceHealthState::Unhealthy);
    }

    // -- Recovering transitions --

    #[test]
    fn recovering_plus_success_below_threshold_stays_recovering() {
        let prev = SourceHealth {
            state: SourceHealthState::Recovering,
            reason: "probe recovering (1/3)".to_string(),
            checked_at: now(),
            consecutive_failures: 0,
            consecutive_successes: 1,
        };
        let policy = HealthPolicy {
            failure_threshold: 2,
            recovery_threshold: 3,
        };
        let h = transition_health(Some(&prev), ProbeSignal::Success, &policy, now());
        assert_eq!(h.state, SourceHealthState::Recovering);
        assert_eq!(h.consecutive_successes, 2);
    }

    #[test]
    fn recovering_plus_success_at_threshold_becomes_healthy() {
        let prev = SourceHealth {
            state: SourceHealthState::Recovering,
            reason: "probe recovering (1/2)".to_string(),
            checked_at: now(),
            consecutive_failures: 0,
            consecutive_successes: 1,
        };
        let h = transition_health(
            Some(&prev),
            ProbeSignal::Success,
            &HealthPolicy::default(),
            now(),
        );
        assert_eq!(h.state, SourceHealthState::Healthy);
        assert_eq!(h.consecutive_successes, 2);
    }

    #[test]
    fn recovering_plus_error_becomes_unhealthy_resets_successes() {
        let prev = SourceHealth {
            state: SourceHealthState::Recovering,
            reason: "probe recovering (1/2)".to_string(),
            checked_at: now(),
            consecutive_failures: 0,
            consecutive_successes: 1,
        };
        let h = transition_health(
            Some(&prev),
            ProbeSignal::Error,
            &HealthPolicy::default(),
            now(),
        );
        assert_eq!(h.state, SourceHealthState::Degraded);
        assert_eq!(h.consecutive_successes, 0);
        assert_eq!(h.consecutive_failures, 1);
    }

    #[test]
    fn recovering_plus_timeout_becomes_degraded() {
        let prev = SourceHealth {
            state: SourceHealthState::Recovering,
            reason: "probe recovering (1/2)".to_string(),
            checked_at: now(),
            consecutive_failures: 0,
            consecutive_successes: 1,
        };
        let h = transition_health(
            Some(&prev),
            ProbeSignal::Timeout,
            &HealthPolicy::default(),
            now(),
        );
        // With default threshold=2, 1 failure -> Degraded
        assert_eq!(h.state, SourceHealthState::Degraded);
    }

    #[test]
    fn recovering_plus_panic_becomes_degraded() {
        let prev = SourceHealth {
            state: SourceHealthState::Recovering,
            reason: "probe recovering (1/2)".to_string(),
            checked_at: now(),
            consecutive_failures: 0,
            consecutive_successes: 1,
        };
        let h = transition_health(
            Some(&prev),
            ProbeSignal::Panic,
            &HealthPolicy::default(),
            now(),
        );
        assert_eq!(h.state, SourceHealthState::Degraded);
    }

    // -- Recovery threshold counting (full cycle) --

    #[test]
    fn full_recovery_cycle_with_threshold_3() {
        let policy = HealthPolicy {
            failure_threshold: 2,
            recovery_threshold: 3,
        };

        // Start unhealthy
        let unhealthy = SourceHealth {
            state: SourceHealthState::Unhealthy,
            reason: "probe error".to_string(),
            checked_at: now(),
            consecutive_failures: 5,
            consecutive_successes: 0,
        };

        // 1st success -> Recovering
        let r1 = transition_health(Some(&unhealthy), ProbeSignal::Success, &policy, now());
        assert_eq!(r1.state, SourceHealthState::Recovering);
        assert_eq!(r1.consecutive_successes, 1);

        // 2nd success -> still Recovering
        let r2 = transition_health(Some(&r1), ProbeSignal::Success, &policy, now());
        assert_eq!(r2.state, SourceHealthState::Recovering);
        assert_eq!(r2.consecutive_successes, 2);

        // 3rd success -> Healthy
        let r3 = transition_health(Some(&r2), ProbeSignal::Success, &policy, now());
        assert_eq!(r3.state, SourceHealthState::Healthy);
        assert_eq!(r3.consecutive_successes, 3);
    }

    // -- Disabled is terminal --

    #[test]
    fn disabled_always_stays_disabled() {
        let prev = SourceHealth {
            state: SourceHealthState::Disabled,
            reason: "source disabled".to_string(),
            checked_at: now(),
            consecutive_failures: 2,
            consecutive_successes: 0,
        };

        for signal in [
            ProbeSignal::Success,
            ProbeSignal::Timeout,
            ProbeSignal::Error,
            ProbeSignal::Panic,
            ProbeSignal::Disabled,
        ] {
            let h = transition_health(Some(&prev), signal, &HealthPolicy::default(), now());
            assert_eq!(h.state, SourceHealthState::Disabled);
        }
    }

    #[test]
    fn any_state_plus_disabled_signal_becomes_disabled() {
        for state in [
            SourceHealthState::Unknown,
            SourceHealthState::Healthy,
            SourceHealthState::Degraded,
            SourceHealthState::Unhealthy,
            SourceHealthState::Recovering,
        ] {
            let prev = SourceHealth {
                state,
                reason: "test".to_string(),
                checked_at: now(),
                consecutive_failures: 0,
                consecutive_successes: 0,
            };
            let h = transition_health(
                Some(&prev),
                ProbeSignal::Disabled,
                &HealthPolicy::default(),
                now(),
            );
            assert_eq!(
                h.state,
                SourceHealthState::Disabled,
                "expected Disabled from {state:?} + Disabled signal"
            );
        }
    }

    // -- Default policy values --

    #[test]
    fn default_policy_values() {
        let p = HealthPolicy::default();
        assert_eq!(p.failure_threshold, 2);
        assert_eq!(p.recovery_threshold, 2);
    }

    // -- Unknown factory --

    #[test]
    fn unknown_factory() {
        let t = now();
        let h = SourceHealth::unknown(t);
        assert_eq!(h.state, SourceHealthState::Unknown);
        assert_eq!(h.reason, "not probed yet");
        assert_eq!(h.checked_at, t);
        assert_eq!(h.consecutive_failures, 0);
        assert_eq!(h.consecutive_successes, 0);
    }

    // -- Admissibility --

    #[test]
    fn only_healthy_is_admissible() {
        let t = now();

        let healthy = SourceHealth {
            state: SourceHealthState::Healthy,
            reason: "ok".to_string(),
            checked_at: t,
            consecutive_failures: 0,
            consecutive_successes: 1,
        };
        assert!(healthy.is_admissible());

        for state in [
            SourceHealthState::Unknown,
            SourceHealthState::Degraded,
            SourceHealthState::Unhealthy,
            SourceHealthState::Recovering,
            SourceHealthState::Disabled,
        ] {
            let h = SourceHealth {
                state,
                reason: "test".to_string(),
                checked_at: t,
                consecutive_failures: 0,
                consecutive_successes: 0,
            };
            assert!(
                !h.is_admissible(),
                "expected {state:?} to NOT be admissible"
            );
        }
    }

    // -- Freshness --

    #[test]
    fn freshness_window_is_correct() {
        // 5.0 + 0.25 + 0.25 = 5.5
        let w = freshness_window_secs();
        assert!((w - 5.5).abs() < f64::EPSILON);
    }

    // -- Serde round-trip --

    #[test]
    fn serde_round_trip_state() {
        let json = serde_json::to_string(&SourceHealthState::Recovering)
            .expect("serialize SourceHealthState");
        assert_eq!(json, r#""recovering""#);
        let back: SourceHealthState =
            serde_json::from_str(&json).expect("deserialize SourceHealthState");
        assert_eq!(back, SourceHealthState::Recovering);
    }

    #[test]
    fn default_state_is_unknown() {
        assert_eq!(SourceHealthState::default(), SourceHealthState::Unknown);
    }
}

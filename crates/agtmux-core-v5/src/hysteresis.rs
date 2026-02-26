//! Hysteresis state machine for activity state transitions.
//!
//! Implements temporal stabilization to prevent flapping:
//!
//! - **Idle stability**: Requires idle observation for `max(4s, 2*poll_interval)`
//!   before confirming an idle transition.
//! - **Running promotion**: Running hint must persist with
//!   `last_interaction <= 8s` to promote to running.
//! - **Running demotion**: Hint must disappear with
//!   `last_interaction > 45s` to demote from running.
//! - **No-agent streak**: Tracked externally via `SignatureInputs::no_agent_streak`,
//!   applied in `signature::classify`.
//!
//! Task ref: T-045

use chrono::{DateTime, TimeDelta, Utc};

use crate::signature::{
    HYSTERESIS_IDLE_MIN_SECS, HYSTERESIS_RUNNING_DEMOTE_SECS, HYSTERESIS_RUNNING_PROMOTE_SECS,
};
use crate::types::ActivityState;

/// Default poll interval for hysteresis calculation (seconds).
pub const DEFAULT_POLL_INTERVAL_SECS: u64 = 5;

/// Confirmed activity state with hysteresis tracking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HysteresisState {
    /// The last confirmed (stable) activity state.
    pub confirmed: ActivityState,
    /// When the confirmed state was established.
    pub confirmed_at: DateTime<Utc>,
    /// The most recent raw observation.
    pub observed: ActivityState,
    /// When the current observed state was first seen (for debounce window).
    pub observed_since: DateTime<Utc>,
    /// Last interaction timestamp (used for running promote/demote).
    pub last_interaction: Option<DateTime<Utc>>,
    /// No-agent streak counter (consecutive observations with no agent signal).
    pub no_agent_streak: u32,
}

impl HysteresisState {
    /// Create an initial hysteresis state.
    pub fn new(initial: ActivityState, now: DateTime<Utc>) -> Self {
        Self {
            confirmed: initial,
            confirmed_at: now,
            observed: initial,
            observed_since: now,
            last_interaction: None,
            no_agent_streak: 0,
        }
    }

    /// Create a default `Unknown` hysteresis state.
    pub fn unknown(now: DateTime<Utc>) -> Self {
        Self::new(ActivityState::Unknown, now)
    }
}

/// Output of a hysteresis update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HysteresisOutput {
    /// The confirmed (stable) activity state after this update.
    pub confirmed: ActivityState,
    /// Whether the confirmed state changed in this update.
    pub changed: bool,
    /// Whether this observation was suppressed by the hysteresis window.
    pub suppressed: bool,
    /// Updated no-agent streak.
    pub no_agent_streak: u32,
}

/// Update the hysteresis state with a new activity observation.
///
/// # Arguments
///
/// * `state` - Current hysteresis state.
/// * `observed` - New raw activity observation.
/// * `now` - Current wall-clock time.
/// * `has_agent_signal` - Whether any agent signal was detected (false increments no-agent streak).
/// * `poll_interval_secs` - Current poll interval for idle window calculation.
///
/// Returns the updated state and output.
pub fn update(
    state: &HysteresisState,
    observed: ActivityState,
    now: DateTime<Utc>,
    has_agent_signal: bool,
    poll_interval_secs: u64,
) -> (HysteresisState, HysteresisOutput) {
    // Track no-agent streak
    let no_agent_streak = if has_agent_signal {
        0
    } else {
        state.no_agent_streak.saturating_add(1)
    };

    // Determine if the observed state is the same as the last observation
    let observed_since = if observed == state.observed {
        state.observed_since
    } else {
        now
    };

    // Use original last_interaction for hysteresis decisions.
    // The promotion/demotion check must reflect the state *before* this observation.
    let last_interaction_for_check = state.last_interaction;

    // Track last interaction for the *next* cycle's promote/demote check.
    let last_interaction = if is_interactive(observed) {
        Some(now)
    } else {
        state.last_interaction
    };

    // Apply hysteresis rules to determine if the confirmed state should change
    let (new_confirmed, changed, suppressed) = apply_hysteresis(
        state,
        observed,
        observed_since,
        now,
        last_interaction_for_check,
        poll_interval_secs,
    );

    let confirmed_at = if changed { now } else { state.confirmed_at };

    let next_state = HysteresisState {
        confirmed: new_confirmed,
        confirmed_at,
        observed,
        observed_since,
        last_interaction,
        no_agent_streak,
    };

    let output = HysteresisOutput {
        confirmed: new_confirmed,
        changed,
        suppressed,
        no_agent_streak,
    };

    (next_state, output)
}

/// Apply hysteresis rules to determine the confirmed state.
///
/// Returns `(new_confirmed, changed, suppressed)`.
fn apply_hysteresis(
    state: &HysteresisState,
    observed: ActivityState,
    observed_since: DateTime<Utc>,
    now: DateTime<Utc>,
    last_interaction: Option<DateTime<Utc>>,
    poll_interval_secs: u64,
) -> (ActivityState, bool, bool) {
    let current = state.confirmed;

    // Same state → no change needed
    if observed == current {
        return (current, false, false);
    }

    match (current, observed) {
        // ── High-priority states: Error, WaitingApproval, WaitingInput ──
        // These transition immediately (no hysteresis) — must be checked first
        // since they override idle/running stability rules.
        (
            _,
            ActivityState::Error | ActivityState::WaitingApproval | ActivityState::WaitingInput,
        ) => (observed, true, false),

        // ── Transition TO Idle ─────────────────────────────────────
        // Idle stability: require idle for max(4s, 2*poll_interval)
        (_, ActivityState::Idle) => {
            let idle_window_secs = idle_window(poll_interval_secs);
            let elapsed = now.signed_duration_since(observed_since);
            if elapsed >= TimeDelta::seconds(idle_window_secs) {
                (ActivityState::Idle, true, false)
            } else {
                // Suppress: not yet stable
                (current, false, true)
            }
        }

        // ── Transition TO Running (promotion) ─────────────────────
        // Running promotion: running hint + last_interaction <= 8s
        (_, ActivityState::Running) => {
            let promote_ok = last_interaction.is_some_and(|li| {
                let elapsed = now.signed_duration_since(li);
                elapsed <= TimeDelta::seconds(HYSTERESIS_RUNNING_PROMOTE_SECS as i64)
            });

            if promote_ok {
                (ActivityState::Running, true, false)
            } else {
                // Suppress: interaction too old or missing
                (current, false, true)
            }
        }

        // ── Transition FROM Running (demotion) ────────────────────
        // Running demotion: hint disappeared + last_interaction > 45s
        (ActivityState::Running, _) => {
            let demote_ok = match last_interaction {
                Some(li) => {
                    let elapsed = now.signed_duration_since(li);
                    elapsed > TimeDelta::seconds(HYSTERESIS_RUNNING_DEMOTE_SECS as i64)
                }
                None => true, // No interaction record → allow demotion
            };

            if demote_ok {
                (observed, true, false)
            } else {
                // Suppress: interaction too recent for demotion
                (current, false, true)
            }
        }

        // ── Default: transition immediately ───────────────────────
        (_, _) => (observed, true, false),
    }
}

/// Calculate idle stability window: max(4s, 2 * poll_interval).
fn idle_window(poll_interval_secs: u64) -> i64 {
    let min = HYSTERESIS_IDLE_MIN_SECS as i64;
    let double_interval = (poll_interval_secs as i64).saturating_mul(2);
    std::cmp::max(min, double_interval)
}

/// Determine if an activity state represents interactive behavior.
fn is_interactive(state: ActivityState) -> bool {
    matches!(
        state,
        ActivityState::Running | ActivityState::WaitingInput | ActivityState::WaitingApproval
    )
}

// ─── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeDelta;

    fn ts(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s)
            .expect("valid RFC3339")
            .with_timezone(&Utc)
    }

    fn t0() -> DateTime<Utc> {
        ts("2026-02-25T12:00:00Z")
    }

    const POLL: u64 = DEFAULT_POLL_INTERVAL_SECS;

    // ── 1. Idle stability: suppressed within window ─────────────────

    #[test]
    fn idle_suppressed_within_window() {
        let state = HysteresisState::new(ActivityState::Running, t0());
        let now = t0() + TimeDelta::seconds(1);

        let (next, output) = update(&state, ActivityState::Idle, now, true, POLL);

        assert_eq!(output.confirmed, ActivityState::Running);
        assert!(!output.changed);
        assert!(output.suppressed);
        assert_eq!(next.observed, ActivityState::Idle);
        assert_eq!(next.observed_since, now);
    }

    // ── 2. Idle stability: confirmed after window ───────────────────

    #[test]
    fn idle_confirmed_after_window() {
        let t = t0();
        let state = HysteresisState {
            confirmed: ActivityState::Running,
            confirmed_at: t,
            observed: ActivityState::Idle,
            observed_since: t, // idle observed since t0
            last_interaction: Some(t),
            no_agent_streak: 0,
        };

        // After max(4s, 2*5s) = 10s
        let now = t + TimeDelta::seconds(10);
        let (next, output) = update(&state, ActivityState::Idle, now, true, POLL);

        assert_eq!(output.confirmed, ActivityState::Idle);
        assert!(output.changed);
        assert!(!output.suppressed);
        assert_eq!(next.confirmed, ActivityState::Idle);
    }

    // ── 3. Idle window: max(4s, 2*interval) ─────────────────────────

    #[test]
    fn idle_window_respects_poll_interval() {
        // With 1s poll interval: max(4, 2*1) = 4s
        assert_eq!(idle_window(1), 4);
        // With 3s poll interval: max(4, 2*3) = 6s
        assert_eq!(idle_window(3), 6);
        // With 5s poll interval: max(4, 2*5) = 10s
        assert_eq!(idle_window(5), 10);
        // With 0s poll interval: max(4, 0) = 4s
        assert_eq!(idle_window(0), 4);
    }

    // ── 4. Running promotion: with recent interaction ───────────────

    #[test]
    fn running_promotion_with_recent_interaction() {
        let t = t0();
        let state = HysteresisState {
            confirmed: ActivityState::Idle,
            confirmed_at: t,
            observed: ActivityState::Running,
            observed_since: t,
            last_interaction: Some(t), // recent interaction
            no_agent_streak: 0,
        };

        let now = t + TimeDelta::seconds(5); // within 8s window
        let (next, output) = update(&state, ActivityState::Running, now, true, POLL);

        assert_eq!(output.confirmed, ActivityState::Running);
        assert!(output.changed);
        assert!(!output.suppressed);
        assert_eq!(next.confirmed, ActivityState::Running);
    }

    // ── 5. Running promotion: suppressed without recent interaction ──

    #[test]
    fn running_promotion_suppressed_without_recent_interaction() {
        let t = t0();
        let state = HysteresisState {
            confirmed: ActivityState::Idle,
            confirmed_at: t,
            observed: ActivityState::Idle,
            observed_since: t,
            last_interaction: None, // no interaction
            no_agent_streak: 0,
        };

        let now = t + TimeDelta::seconds(5);
        let (_, output) = update(&state, ActivityState::Running, now, true, POLL);

        assert_eq!(output.confirmed, ActivityState::Idle);
        assert!(!output.changed);
        assert!(output.suppressed);
    }

    // ── 6. Running promotion: suppressed with old interaction ───────

    #[test]
    fn running_promotion_suppressed_with_old_interaction() {
        let t = t0();
        let state = HysteresisState {
            confirmed: ActivityState::Idle,
            confirmed_at: t,
            observed: ActivityState::Idle,
            observed_since: t,
            last_interaction: Some(t - TimeDelta::seconds(20)), // old interaction
            no_agent_streak: 0,
        };

        let now = t + TimeDelta::seconds(5);
        let (_, output) = update(&state, ActivityState::Running, now, true, POLL);

        assert_eq!(output.confirmed, ActivityState::Idle);
        assert!(output.suppressed);
    }

    // ── 7. Running demotion: suppressed with recent interaction ─────

    #[test]
    fn running_demotion_suppressed_with_recent_interaction() {
        let t = t0();
        let state = HysteresisState {
            confirmed: ActivityState::Running,
            confirmed_at: t,
            observed: ActivityState::Running,
            observed_since: t,
            last_interaction: Some(t), // very recent
            no_agent_streak: 0,
        };

        let now = t + TimeDelta::seconds(10); // only 10s since interaction, < 45s
        let (_, output) = update(&state, ActivityState::Idle, now, true, POLL);

        // Idle observation just started (observed changes from Running→Idle),
        // so observed_since = now, elapsed = 0 < idle_window → suppressed.
        // Confirmed stays Running.
        assert_eq!(output.confirmed, ActivityState::Running);
        assert!(output.suppressed);
    }

    // ── 8. Running demotion: to unknown after old interaction ───────

    #[test]
    fn running_demotion_to_unknown_after_old_interaction() {
        let t = t0();
        let state = HysteresisState {
            confirmed: ActivityState::Running,
            confirmed_at: t,
            observed: ActivityState::Running,
            observed_since: t,
            last_interaction: Some(t),
            no_agent_streak: 0,
        };

        let now = t + TimeDelta::seconds(50); // 50s > 45s threshold
        let (next, output) = update(&state, ActivityState::Unknown, now, false, POLL);

        assert_eq!(output.confirmed, ActivityState::Unknown);
        assert!(output.changed);
        assert_eq!(next.confirmed, ActivityState::Unknown);
    }

    // ── 9. Running demotion: to unknown suppressed with recent interaction ──

    #[test]
    fn running_demotion_to_unknown_suppressed_recent_interaction() {
        let t = t0();
        let state = HysteresisState {
            confirmed: ActivityState::Running,
            confirmed_at: t,
            observed: ActivityState::Running,
            observed_since: t,
            last_interaction: Some(t + TimeDelta::seconds(30)),
            no_agent_streak: 0,
        };

        let now = t + TimeDelta::seconds(40); // only 10s since last_interaction
        let (_, output) = update(&state, ActivityState::Unknown, now, false, POLL);

        assert_eq!(output.confirmed, ActivityState::Running);
        assert!(output.suppressed);
    }

    // ── 10. Error transitions immediately ───────────────────────────

    #[test]
    fn error_transitions_immediately() {
        let t = t0();
        let state = HysteresisState::new(ActivityState::Running, t);

        let now = t + TimeDelta::seconds(1);
        let (next, output) = update(&state, ActivityState::Error, now, true, POLL);

        assert_eq!(output.confirmed, ActivityState::Error);
        assert!(output.changed);
        assert!(!output.suppressed);
        assert_eq!(next.confirmed, ActivityState::Error);
    }

    // ── 11. WaitingApproval transitions immediately ─────────────────

    #[test]
    fn waiting_approval_transitions_immediately() {
        let t = t0();
        let state = HysteresisState::new(ActivityState::Running, t);

        let now = t + TimeDelta::seconds(1);
        let (_, output) = update(&state, ActivityState::WaitingApproval, now, true, POLL);

        assert_eq!(output.confirmed, ActivityState::WaitingApproval);
        assert!(output.changed);
    }

    // ── 12. WaitingInput transitions immediately ────────────────────

    #[test]
    fn waiting_input_transitions_immediately() {
        let t = t0();
        let state = HysteresisState::new(ActivityState::Idle, t);

        let now = t + TimeDelta::seconds(1);
        let (_, output) = update(&state, ActivityState::WaitingInput, now, true, POLL);

        assert_eq!(output.confirmed, ActivityState::WaitingInput);
        assert!(output.changed);
    }

    // ── 13. No-agent streak increments on no signal ─────────────────

    #[test]
    fn no_agent_streak_increments() {
        let t = t0();
        let state = HysteresisState::new(ActivityState::Unknown, t);

        let (next1, out1) = update(
            &state,
            ActivityState::Unknown,
            t + TimeDelta::seconds(1),
            false,
            POLL,
        );
        assert_eq!(out1.no_agent_streak, 1);

        let (next2, out2) = update(
            &next1,
            ActivityState::Unknown,
            t + TimeDelta::seconds(2),
            false,
            POLL,
        );
        assert_eq!(out2.no_agent_streak, 2);

        // Reset on agent signal
        let (_, out3) = update(
            &next2,
            ActivityState::Running,
            t + TimeDelta::seconds(3),
            true,
            POLL,
        );
        assert_eq!(out3.no_agent_streak, 0);
    }

    // ── 14. Same state returns no change ────────────────────────────

    #[test]
    fn same_state_no_change() {
        let t = t0();
        let state = HysteresisState::new(ActivityState::Running, t);

        let (_, output) = update(
            &state,
            ActivityState::Running,
            t + TimeDelta::seconds(1),
            true,
            POLL,
        );

        assert_eq!(output.confirmed, ActivityState::Running);
        assert!(!output.changed);
        assert!(!output.suppressed);
    }

    // ── 15. Flap suppression: rapid idle/running toggles ────────────

    #[test]
    fn flap_suppression_rapid_toggles() {
        let t = t0();
        let state = HysteresisState::new(ActivityState::Running, t);

        // Rapid toggles: running → idle (1s later) → running (2s later)
        let (s1, o1) = update(
            &state,
            ActivityState::Idle,
            t + TimeDelta::seconds(1),
            true,
            POLL,
        );
        assert!(!o1.changed, "idle should be suppressed within window");
        assert_eq!(o1.confirmed, ActivityState::Running);

        let (s2, o2) = update(
            &s1,
            ActivityState::Running,
            t + TimeDelta::seconds(2),
            true,
            POLL,
        );
        assert!(!o2.changed, "back to running, same as confirmed");
        assert_eq!(o2.confirmed, ActivityState::Running);

        // Stable idle for full window
        let (s3, _) = update(
            &s2,
            ActivityState::Idle,
            t + TimeDelta::seconds(3),
            true,
            POLL,
        );
        let (_, o4) = update(
            &s3,
            ActivityState::Idle,
            t + TimeDelta::seconds(13),
            true,
            POLL,
        );
        assert!(o4.changed, "idle stable for 10s, should be confirmed");
        assert_eq!(o4.confirmed, ActivityState::Idle);
    }

    // ── 16. Running promotion at boundary ───────────────────────────

    #[test]
    fn running_promotion_at_8s_boundary() {
        let t = t0();
        let state = HysteresisState {
            confirmed: ActivityState::Idle,
            confirmed_at: t,
            observed: ActivityState::Idle,
            observed_since: t,
            last_interaction: Some(t),
            no_agent_streak: 0,
        };

        // Exactly at 8s boundary: last_interaction was at t, now = t+8
        let now = t + TimeDelta::seconds(8);
        let (_, output) = update(&state, ActivityState::Running, now, true, POLL);

        // <= 8s includes the boundary
        assert_eq!(output.confirmed, ActivityState::Running);
        assert!(output.changed);
    }

    // ── 17. Running promotion just past boundary ────────────────────

    #[test]
    fn running_promotion_past_8s_boundary() {
        let t = t0();
        let state = HysteresisState {
            confirmed: ActivityState::Idle,
            confirmed_at: t,
            observed: ActivityState::Idle,
            observed_since: t,
            last_interaction: Some(t),
            no_agent_streak: 0,
        };

        let now = t + TimeDelta::seconds(9); // > 8s
        let (_, output) = update(&state, ActivityState::Running, now, true, POLL);

        assert_eq!(output.confirmed, ActivityState::Idle);
        assert!(output.suppressed);
    }

    // ── 18. Running demotion at 45s boundary ────────────────────────

    #[test]
    fn running_demotion_at_45s_boundary() {
        let t = t0();
        let state = HysteresisState {
            confirmed: ActivityState::Running,
            confirmed_at: t,
            observed: ActivityState::Running,
            observed_since: t,
            last_interaction: Some(t),
            no_agent_streak: 0,
        };

        // At exactly 45s: not > 45s, so demotion should be suppressed
        let now = t + TimeDelta::seconds(45);
        let (_, output) = update(&state, ActivityState::Unknown, now, false, POLL);

        assert_eq!(output.confirmed, ActivityState::Running);
        assert!(output.suppressed);
    }

    // ── 19. Running demotion just past 45s ──────────────────────────

    #[test]
    fn running_demotion_past_45s_boundary() {
        let t = t0();
        let state = HysteresisState {
            confirmed: ActivityState::Running,
            confirmed_at: t,
            observed: ActivityState::Running,
            observed_since: t,
            last_interaction: Some(t),
            no_agent_streak: 0,
        };

        let now = t + TimeDelta::seconds(46);
        let (_, output) = update(&state, ActivityState::Unknown, now, false, POLL);

        assert_eq!(output.confirmed, ActivityState::Unknown);
        assert!(output.changed);
    }

    // ── 20. Unknown initial state factory ───────────────────────────

    #[test]
    fn unknown_factory() {
        let t = t0();
        let state = HysteresisState::unknown(t);
        assert_eq!(state.confirmed, ActivityState::Unknown);
        assert_eq!(state.observed, ActivityState::Unknown);
        assert!(state.last_interaction.is_none());
        assert_eq!(state.no_agent_streak, 0);
    }

    // ── 21. Last interaction updated on interactive states ──────────

    #[test]
    fn last_interaction_updated_on_interactive() {
        let t = t0();
        let state = HysteresisState::unknown(t);

        // Running is interactive
        let now = t + TimeDelta::seconds(1);
        let (next, _) = update(&state, ActivityState::Running, now, true, POLL);
        assert_eq!(next.last_interaction, Some(now));

        // WaitingApproval is interactive
        let now2 = now + TimeDelta::seconds(1);
        let (next2, _) = update(&next, ActivityState::WaitingApproval, now2, true, POLL);
        assert_eq!(next2.last_interaction, Some(now2));
    }

    // ── 22. Last interaction NOT updated on non-interactive ─────────

    #[test]
    fn last_interaction_not_updated_on_idle() {
        let t = t0();
        let state = HysteresisState {
            confirmed: ActivityState::Running,
            confirmed_at: t,
            observed: ActivityState::Running,
            observed_since: t,
            last_interaction: Some(t),
            no_agent_streak: 0,
        };

        let now = t + TimeDelta::seconds(20);
        let (next, _) = update(&state, ActivityState::Idle, now, true, POLL);

        // last_interaction should NOT be updated (Idle is not interactive)
        assert_eq!(next.last_interaction, Some(t));
    }

    // ── 23. Idle window with short poll interval ────────────────────

    #[test]
    fn idle_confirmed_with_short_poll_interval() {
        let t = t0();
        let state = HysteresisState {
            confirmed: ActivityState::Running,
            confirmed_at: t,
            observed: ActivityState::Idle,
            observed_since: t,
            last_interaction: Some(t),
            no_agent_streak: 0,
        };

        // With 1s poll interval: idle window = max(4, 2) = 4s
        let now = t + TimeDelta::seconds(4);
        let (_, output) = update(&state, ActivityState::Idle, now, true, 1);

        assert_eq!(output.confirmed, ActivityState::Idle);
        assert!(output.changed);
    }

    // ── 24. Deterministic priority: error overrides running ─────────

    #[test]
    fn error_overrides_running_immediately() {
        let t = t0();
        let state = HysteresisState {
            confirmed: ActivityState::Running,
            confirmed_at: t,
            observed: ActivityState::Running,
            observed_since: t,
            last_interaction: Some(t),
            no_agent_streak: 0,
        };

        // Error should override even with very recent interaction
        let now = t + TimeDelta::seconds(1);
        let (_, output) = update(&state, ActivityState::Error, now, true, POLL);

        assert_eq!(output.confirmed, ActivityState::Error);
        assert!(output.changed);
    }

    // ── 25. Full lifecycle: Unknown → Running → Idle → Unknown ─────

    #[test]
    fn full_lifecycle() {
        let t = t0();
        let s0 = HysteresisState::unknown(t);

        // Observe Running with interaction
        let t1 = t + TimeDelta::seconds(1);
        let (s1, o1) = update(&s0, ActivityState::Running, t1, true, POLL);
        // No last_interaction in s0, so running promotion is suppressed
        assert!(!o1.changed);
        // But last_interaction is now set
        assert_eq!(s1.last_interaction, Some(t1));

        // Observe Running again (now has recent interaction)
        let t2 = t1 + TimeDelta::seconds(2);
        let (s2, o2) = update(&s1, ActivityState::Running, t2, true, POLL);
        assert!(o2.changed);
        assert_eq!(o2.confirmed, ActivityState::Running);
        assert_eq!(s2.confirmed, ActivityState::Running);

        // Observe Idle (suppressed initially by idle window)
        let t3 = t2 + TimeDelta::seconds(1);
        let (s3, o3) = update(&s2, ActivityState::Idle, t3, true, POLL);
        assert!(!o3.changed);
        assert_eq!(o3.confirmed, ActivityState::Running);

        // Idle stable for window
        let t4 = t3 + TimeDelta::seconds(10);
        let (s4, o4) = update(&s3, ActivityState::Idle, t4, true, POLL);
        assert!(o4.changed);
        assert_eq!(o4.confirmed, ActivityState::Idle);

        // No agent signals, observe Unknown
        let t5 = t4 + TimeDelta::seconds(1);
        let (_, o5) = update(&s4, ActivityState::Unknown, t5, false, POLL);
        assert!(o5.changed);
        assert_eq!(o5.confirmed, ActivityState::Unknown);
        assert_eq!(o5.no_agent_streak, 1);
    }
}

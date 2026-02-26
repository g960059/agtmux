//! Pane-first binding state machine (MVP).
//!
//! Tracks the lifecycle of a pane's binding to an agent session:
//!
//! - **Entity key**: `PaneInstanceId` (`pane_id`, `generation`, `birth_ts`)
//! - **Link target**: `session_key`
//!
//! ## States
//!
//! - `Unmanaged` — no agent evidence for this pane
//! - `ManagedHeuristic` — bound via heuristic signature
//! - `ManagedDeterministicFresh` — bound via deterministic handshake, evidence is fresh
//! - `ManagedDeterministicStale` — deterministic evidence has gone stale
//!
//! ## Key transitions
//!
//! - heuristic signature: `Unmanaged -> ManagedHeuristic`
//! - deterministic handshake: `{Unmanaged, ManagedHeuristic} -> ManagedDeterministicFresh`
//! - freshness exceeded: `ManagedDeterministicFresh -> ManagedDeterministicStale`
//! - deterministic recovery: `ManagedDeterministicStale -> ManagedDeterministicFresh`
//! - heuristic no-agent x2: `ManagedHeuristic -> Unmanaged`
//!
//! ## Pane reuse guard
//!
//! When a `pane_id` is reused, `generation` is incremented.
//! A tombstone grace window (120s) prevents false binding.
//!
//! Task ref: T-042

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::signature::NO_AGENT_DEMOTION_STREAK;
use crate::types::PaneInstanceId;

// ─── Constants ───────────────────────────────────────────────────────

/// Grace window (seconds) for tombstone after pane reuse.
/// During this window the old binding is kept as a tombstone to prevent
/// false binding from stale events arriving for the previous generation.
pub const TOMBSTONE_GRACE_SECS: u64 = 120;

// ─── Binding State ───────────────────────────────────────────────────

/// Binding lifecycle state for a pane instance.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingState {
    /// No agent evidence for this pane.
    #[default]
    Unmanaged,
    /// Bound via heuristic signature only.
    ManagedHeuristic,
    /// Bound via deterministic handshake; evidence is fresh.
    ManagedDeterministicFresh,
    /// Deterministic evidence has gone stale (freshness exceeded).
    ManagedDeterministicStale,
}

// ─── Pane Binding ────────────────────────────────────────────────────

/// Full binding record for a pane instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneBinding {
    /// Pane identifier (e.g. `%1`).
    pub pane_id: String,
    /// Monotonically increasing generation counter for pane reuse detection.
    pub generation: u64,
    /// Timestamp when this pane instance was born (created/reused).
    pub birth_ts: DateTime<Utc>,
    /// Current binding lifecycle state.
    pub binding_state: BindingState,
    /// The agent session this pane is linked to (if any).
    pub session_key: Option<String>,
    /// When the binding was established.
    pub bound_at: Option<DateTime<Utc>>,
    /// Most recent activity observation on this pane.
    pub last_activity_at: Option<DateTime<Utc>>,
    /// Most recent deterministic handshake timestamp.
    pub last_deterministic_at: Option<DateTime<Utc>>,
    /// Consecutive observations with no agent signal.
    pub no_agent_streak: u32,
    /// If set, this binding is tombstoned until this time.
    /// During grace window, new bindings for the same `pane_id` are blocked.
    pub tombstone_until: Option<DateTime<Utc>>,
}

impl PaneBinding {
    /// Create a new unmanaged pane binding.
    pub fn new(pane_id: String, generation: u64, birth_ts: DateTime<Utc>) -> Self {
        Self {
            pane_id,
            generation,
            birth_ts,
            binding_state: BindingState::Unmanaged,
            session_key: None,
            bound_at: None,
            last_activity_at: None,
            last_deterministic_at: None,
            no_agent_streak: 0,
            tombstone_until: None,
        }
    }

    /// Returns the `PaneInstanceId` for this binding.
    pub fn instance_id(&self) -> PaneInstanceId {
        PaneInstanceId {
            pane_id: self.pane_id.clone(),
            generation: self.generation,
            birth_ts: self.birth_ts,
        }
    }

    /// Returns `true` if this binding is currently tombstoned at the given time.
    pub fn is_tombstoned(&self, now: DateTime<Utc>) -> bool {
        self.tombstone_until.is_some_and(|until| now < until)
    }
}

// ─── Binding Events ──────────────────────────────────────────────────

/// Input events that drive binding state transitions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BindingEvent {
    /// A heuristic signature was detected for this pane.
    HeuristicDetected {
        session_key: String,
        confidence: f64,
        at: DateTime<Utc>,
    },
    /// A deterministic handshake was completed.
    DeterministicHandshake {
        session_key: String,
        at: DateTime<Utc>,
    },
    /// Deterministic freshness has expired (stale threshold exceeded).
    FreshnessExpired { at: DateTime<Utc> },
    /// Deterministic evidence has recovered (fresh again).
    DeterministicRecovered { at: DateTime<Utc> },
    /// No agent signal was observed in this poll cycle.
    NoAgentObserved { at: DateTime<Utc> },
    /// An agent signal was observed (resets no-agent streak).
    AgentObserved { at: DateTime<Utc> },
    /// The pane was reused (new process in same pane_id).
    PaneReused {
        birth_ts: DateTime<Utc>,
        at: DateTime<Utc>,
    },
}

// ─── State Machine ───────────────────────────────────────────────────

/// Pure function: apply a binding event to produce a new binding state.
///
/// This is the core state machine for pane binding. It returns a new
/// `PaneBinding` reflecting the transition (if any). The input is not
/// mutated.
pub fn apply_binding_event(state: &PaneBinding, event: &BindingEvent) -> PaneBinding {
    let mut next = state.clone();

    match event {
        // ── Heuristic detected ───────────────────────────────────
        BindingEvent::HeuristicDetected {
            session_key,
            confidence: _,
            at,
        } => {
            match state.binding_state {
                BindingState::Unmanaged => {
                    // Guard: if tombstoned, do not bind
                    if state.is_tombstoned(*at) {
                        return next;
                    }
                    next.binding_state = BindingState::ManagedHeuristic;
                    next.session_key = Some(session_key.clone());
                    next.bound_at = Some(*at);
                    next.last_activity_at = Some(*at);
                    next.no_agent_streak = 0;
                }
                // Already managed: update activity, keep state
                BindingState::ManagedHeuristic
                | BindingState::ManagedDeterministicFresh
                | BindingState::ManagedDeterministicStale => {
                    next.last_activity_at = Some(*at);
                }
            }
        }

        // ── Deterministic handshake ──────────────────────────────
        BindingEvent::DeterministicHandshake { session_key, at } => {
            match state.binding_state {
                BindingState::Unmanaged => {
                    // Guard: if tombstoned, do not bind
                    if state.is_tombstoned(*at) {
                        return next;
                    }
                    next.binding_state = BindingState::ManagedDeterministicFresh;
                    next.session_key = Some(session_key.clone());
                    next.bound_at = Some(*at);
                    next.last_activity_at = Some(*at);
                    next.last_deterministic_at = Some(*at);
                    next.no_agent_streak = 0;
                }
                BindingState::ManagedHeuristic | BindingState::ManagedDeterministicStale => {
                    next.binding_state = BindingState::ManagedDeterministicFresh;
                    next.session_key = Some(session_key.clone());
                    next.last_activity_at = Some(*at);
                    next.last_deterministic_at = Some(*at);
                    next.no_agent_streak = 0;
                }
                BindingState::ManagedDeterministicFresh => {
                    // Already fresh-deterministic: update timestamps
                    next.last_activity_at = Some(*at);
                    next.last_deterministic_at = Some(*at);
                    next.no_agent_streak = 0;
                }
            }
        }

        // ── Freshness expired ────────────────────────────────────
        BindingEvent::FreshnessExpired { at } => {
            if state.binding_state == BindingState::ManagedDeterministicFresh {
                next.binding_state = BindingState::ManagedDeterministicStale;
                next.last_activity_at = Some(*at);
            }
            // Other states: no-op
        }

        // ── Deterministic recovered ──────────────────────────────
        BindingEvent::DeterministicRecovered { at } => {
            if state.binding_state == BindingState::ManagedDeterministicStale {
                next.binding_state = BindingState::ManagedDeterministicFresh;
                next.last_deterministic_at = Some(*at);
                next.last_activity_at = Some(*at);
            }
            // Other states: no-op
        }

        // ── No agent observed ────────────────────────────────────
        BindingEvent::NoAgentObserved { at } => {
            next.no_agent_streak = state.no_agent_streak.saturating_add(1);
            next.last_activity_at = Some(*at);

            // Demotion: ManagedHeuristic -> Unmanaged after streak threshold
            if state.binding_state == BindingState::ManagedHeuristic
                && next.no_agent_streak >= NO_AGENT_DEMOTION_STREAK
            {
                next.binding_state = BindingState::Unmanaged;
                next.session_key = None;
                next.bound_at = None;
            }
        }

        // ── Agent observed ───────────────────────────────────────
        BindingEvent::AgentObserved { at } => {
            next.no_agent_streak = 0;
            next.last_activity_at = Some(*at);
        }

        // ── Pane reused ──────────────────────────────────────────
        BindingEvent::PaneReused { birth_ts, at } => {
            let grace_end = *at + chrono::TimeDelta::seconds(TOMBSTONE_GRACE_SECS as i64);
            next.tombstone_until = Some(grace_end);
            next.generation = state.generation.saturating_add(1);
            next.birth_ts = *birth_ts;
            next.binding_state = BindingState::Unmanaged;
            next.session_key = None;
            next.bound_at = None;
            next.last_activity_at = None;
            next.last_deterministic_at = None;
            next.no_agent_streak = 0;
        }
    }

    next
}

// ─── Representative Pane Selection ───────────────────────────────────

/// Select the representative pane from a list of bindings for a given session.
///
/// Selection criteria (in priority order):
/// 1. Latest deterministic handshake time
/// 2. Tie-break: latest activity
/// 3. Tie-break: `pane_id` lexical order (ascending, so smallest wins)
///
/// Returns `None` if the input slice is empty or no bindings match the
/// given `session_key`.
pub fn select_representative<'a>(
    bindings: &'a [PaneBinding],
    session_key: &str,
) -> Option<&'a PaneBinding> {
    bindings
        .iter()
        .filter(|b| b.session_key.as_deref() == Some(session_key))
        .max_by(|a, b| {
            // 1. Latest deterministic handshake time (None < Some)
            let det_cmp = a.last_deterministic_at.cmp(&b.last_deterministic_at);
            if det_cmp != std::cmp::Ordering::Equal {
                return det_cmp;
            }

            // 2. Tie-break: latest activity (None < Some)
            let act_cmp = a.last_activity_at.cmp(&b.last_activity_at);
            if act_cmp != std::cmp::Ordering::Equal {
                return act_cmp;
            }

            // 3. Tie-break: pane_id lexical order (ascending, so smallest wins)
            //    We want the smallest pane_id to win, so reverse the comparison:
            //    if a < b lexically, a should be "greater" in max_by terms.
            b.pane_id.cmp(&a.pane_id)
        })
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

    fn make_binding(pane_id: &str, generation: u64) -> PaneBinding {
        PaneBinding::new(pane_id.to_string(), generation, t0())
    }

    // ── 1. Unmanaged -> ManagedHeuristic on heuristic detection ───────

    #[test]
    fn unmanaged_to_managed_heuristic() {
        let b = make_binding("%1", 0);
        let at = t0() + TimeDelta::seconds(1);
        let event = BindingEvent::HeuristicDetected {
            session_key: "sess-001".into(),
            confidence: 0.86,
            at,
        };

        let next = apply_binding_event(&b, &event);

        assert_eq!(next.binding_state, BindingState::ManagedHeuristic);
        assert_eq!(next.session_key.as_deref(), Some("sess-001"));
        assert_eq!(next.bound_at, Some(at));
        assert_eq!(next.last_activity_at, Some(at));
        assert_eq!(next.no_agent_streak, 0);
    }

    // ── 2. ManagedHeuristic -> ManagedDeterministicFresh on handshake ─

    #[test]
    fn managed_heuristic_to_deterministic_fresh() {
        let mut b = make_binding("%1", 0);
        b.binding_state = BindingState::ManagedHeuristic;
        b.session_key = Some("sess-001".into());
        b.bound_at = Some(t0());

        let at = t0() + TimeDelta::seconds(5);
        let event = BindingEvent::DeterministicHandshake {
            session_key: "sess-001".into(),
            at,
        };

        let next = apply_binding_event(&b, &event);

        assert_eq!(next.binding_state, BindingState::ManagedDeterministicFresh);
        assert_eq!(next.last_deterministic_at, Some(at));
        assert_eq!(next.last_activity_at, Some(at));
    }

    // ── 3. Unmanaged -> ManagedDeterministicFresh on handshake ────────

    #[test]
    fn unmanaged_to_deterministic_fresh() {
        let b = make_binding("%1", 0);
        let at = t0() + TimeDelta::seconds(1);
        let event = BindingEvent::DeterministicHandshake {
            session_key: "sess-001".into(),
            at,
        };

        let next = apply_binding_event(&b, &event);

        assert_eq!(next.binding_state, BindingState::ManagedDeterministicFresh);
        assert_eq!(next.session_key.as_deref(), Some("sess-001"));
        assert_eq!(next.bound_at, Some(at));
        assert_eq!(next.last_deterministic_at, Some(at));
    }

    // ── 4. ManagedDeterministicFresh -> ManagedDeterministicStale ─────

    #[test]
    fn deterministic_fresh_to_stale_on_freshness_expired() {
        let mut b = make_binding("%1", 0);
        b.binding_state = BindingState::ManagedDeterministicFresh;
        b.session_key = Some("sess-001".into());
        b.last_deterministic_at = Some(t0());

        let at = t0() + TimeDelta::seconds(4);
        let event = BindingEvent::FreshnessExpired { at };

        let next = apply_binding_event(&b, &event);

        assert_eq!(next.binding_state, BindingState::ManagedDeterministicStale);
        assert_eq!(next.last_activity_at, Some(at));
        // session_key preserved
        assert_eq!(next.session_key.as_deref(), Some("sess-001"));
    }

    // ── 5. ManagedDeterministicStale -> ManagedDeterministicFresh ─────

    #[test]
    fn deterministic_stale_to_fresh_on_recovery() {
        let mut b = make_binding("%1", 0);
        b.binding_state = BindingState::ManagedDeterministicStale;
        b.session_key = Some("sess-001".into());
        b.last_deterministic_at = Some(t0());

        let at = t0() + TimeDelta::seconds(10);
        let event = BindingEvent::DeterministicRecovered { at };

        let next = apply_binding_event(&b, &event);

        assert_eq!(next.binding_state, BindingState::ManagedDeterministicFresh);
        assert_eq!(next.last_deterministic_at, Some(at));
        assert_eq!(next.last_activity_at, Some(at));
    }

    // ── 6. ManagedHeuristic -> Unmanaged on no-agent streak=2 ────────

    #[test]
    fn heuristic_demoted_to_unmanaged_on_no_agent_streak() {
        let mut b = make_binding("%1", 0);
        b.binding_state = BindingState::ManagedHeuristic;
        b.session_key = Some("sess-001".into());
        b.bound_at = Some(t0());
        b.no_agent_streak = 0;

        let at1 = t0() + TimeDelta::seconds(1);
        let event1 = BindingEvent::NoAgentObserved { at: at1 };
        let b1 = apply_binding_event(&b, &event1);

        assert_eq!(b1.binding_state, BindingState::ManagedHeuristic);
        assert_eq!(b1.no_agent_streak, 1);
        assert_eq!(b1.session_key.as_deref(), Some("sess-001"));

        let at2 = t0() + TimeDelta::seconds(2);
        let event2 = BindingEvent::NoAgentObserved { at: at2 };
        let b2 = apply_binding_event(&b1, &event2);

        assert_eq!(b2.binding_state, BindingState::Unmanaged);
        assert_eq!(b2.no_agent_streak, NO_AGENT_DEMOTION_STREAK);
        assert_eq!(b2.session_key, None);
        assert_eq!(b2.bound_at, None);
    }

    // ── 7. No-agent streak reset on agent observation ────────────────

    #[test]
    fn no_agent_streak_reset_on_agent_observed() {
        let mut b = make_binding("%1", 0);
        b.binding_state = BindingState::ManagedHeuristic;
        b.session_key = Some("sess-001".into());
        b.no_agent_streak = 1;

        let at = t0() + TimeDelta::seconds(3);
        let event = BindingEvent::AgentObserved { at };

        let next = apply_binding_event(&b, &event);

        assert_eq!(next.no_agent_streak, 0);
        assert_eq!(next.binding_state, BindingState::ManagedHeuristic);
        assert_eq!(next.last_activity_at, Some(at));
    }

    // ── 8. Pane reuse increments generation + sets tombstone ─────────

    #[test]
    fn pane_reuse_increments_generation_and_tombstones() {
        let mut b = make_binding("%1", 0);
        b.binding_state = BindingState::ManagedDeterministicFresh;
        b.session_key = Some("sess-001".into());
        b.last_deterministic_at = Some(t0());

        let at = t0() + TimeDelta::seconds(60);
        let new_birth = at;
        let event = BindingEvent::PaneReused {
            birth_ts: new_birth,
            at,
        };

        let next = apply_binding_event(&b, &event);

        assert_eq!(next.generation, 1);
        assert_eq!(next.birth_ts, new_birth);
        assert_eq!(next.binding_state, BindingState::Unmanaged);
        assert_eq!(next.session_key, None);
        assert_eq!(next.bound_at, None);
        assert_eq!(next.last_activity_at, None);
        assert_eq!(next.last_deterministic_at, None);
        assert_eq!(next.no_agent_streak, 0);

        let expected_grace_end = at + TimeDelta::seconds(TOMBSTONE_GRACE_SECS as i64);
        assert_eq!(next.tombstone_until, Some(expected_grace_end));
    }

    // ── 9. Tombstone blocks heuristic binding during grace window ────

    #[test]
    fn tombstone_blocks_heuristic_during_grace() {
        let mut b = make_binding("%1", 1);
        b.binding_state = BindingState::Unmanaged;
        let grace_end = t0() + TimeDelta::seconds(TOMBSTONE_GRACE_SECS as i64);
        b.tombstone_until = Some(grace_end);

        // Try to bind during grace window
        let at = t0() + TimeDelta::seconds(10); // well within 120s grace
        let event = BindingEvent::HeuristicDetected {
            session_key: "sess-002".into(),
            confidence: 0.86,
            at,
        };

        let next = apply_binding_event(&b, &event);

        // Should remain unmanaged
        assert_eq!(next.binding_state, BindingState::Unmanaged);
        assert_eq!(next.session_key, None);
    }

    // ── 10. Tombstone blocks deterministic binding during grace ──────

    #[test]
    fn tombstone_blocks_deterministic_during_grace() {
        let mut b = make_binding("%1", 1);
        b.binding_state = BindingState::Unmanaged;
        let grace_end = t0() + TimeDelta::seconds(TOMBSTONE_GRACE_SECS as i64);
        b.tombstone_until = Some(grace_end);

        let at = t0() + TimeDelta::seconds(10);
        let event = BindingEvent::DeterministicHandshake {
            session_key: "sess-002".into(),
            at,
        };

        let next = apply_binding_event(&b, &event);

        assert_eq!(next.binding_state, BindingState::Unmanaged);
        assert_eq!(next.session_key, None);
    }

    // ── 11. Tombstone expires after grace window ─────────────────────

    #[test]
    fn tombstone_expires_after_grace_window() {
        let mut b = make_binding("%1", 1);
        b.binding_state = BindingState::Unmanaged;
        let grace_end = t0() + TimeDelta::seconds(TOMBSTONE_GRACE_SECS as i64);
        b.tombstone_until = Some(grace_end);

        // After grace window: should allow binding
        let at = grace_end + TimeDelta::seconds(1);
        let event = BindingEvent::HeuristicDetected {
            session_key: "sess-002".into(),
            confidence: 0.86,
            at,
        };

        let next = apply_binding_event(&b, &event);

        assert_eq!(next.binding_state, BindingState::ManagedHeuristic);
        assert_eq!(next.session_key.as_deref(), Some("sess-002"));
    }

    // ── 12. Tombstone exactly at boundary allows binding ─────────────

    #[test]
    fn tombstone_at_exact_boundary_allows_binding() {
        let mut b = make_binding("%1", 1);
        b.binding_state = BindingState::Unmanaged;
        let grace_end = t0() + TimeDelta::seconds(TOMBSTONE_GRACE_SECS as i64);
        b.tombstone_until = Some(grace_end);

        // Exactly at grace_end: `now < until` is false, so binding is allowed
        let event = BindingEvent::HeuristicDetected {
            session_key: "sess-002".into(),
            confidence: 0.86,
            at: grace_end,
        };

        let next = apply_binding_event(&b, &event);

        assert_eq!(next.binding_state, BindingState::ManagedHeuristic);
        assert_eq!(next.session_key.as_deref(), Some("sess-002"));
    }

    // ── 13. FreshnessExpired is no-op for non-fresh states ───────────

    #[test]
    fn freshness_expired_noop_on_unmanaged() {
        let b = make_binding("%1", 0);
        let at = t0() + TimeDelta::seconds(5);
        let event = BindingEvent::FreshnessExpired { at };

        let next = apply_binding_event(&b, &event);

        assert_eq!(next.binding_state, BindingState::Unmanaged);
        assert_eq!(next.last_activity_at, None);
    }

    #[test]
    fn freshness_expired_noop_on_heuristic() {
        let mut b = make_binding("%1", 0);
        b.binding_state = BindingState::ManagedHeuristic;
        b.session_key = Some("sess-001".into());

        let at = t0() + TimeDelta::seconds(5);
        let event = BindingEvent::FreshnessExpired { at };

        let next = apply_binding_event(&b, &event);

        assert_eq!(next.binding_state, BindingState::ManagedHeuristic);
    }

    // ── 14. DeterministicRecovered is no-op for non-stale states ─────

    #[test]
    fn deterministic_recovered_noop_on_fresh() {
        let mut b = make_binding("%1", 0);
        b.binding_state = BindingState::ManagedDeterministicFresh;
        b.session_key = Some("sess-001".into());
        b.last_deterministic_at = Some(t0());

        let at = t0() + TimeDelta::seconds(5);
        let event = BindingEvent::DeterministicRecovered { at };

        let next = apply_binding_event(&b, &event);

        // Should remain fresh, timestamps unchanged
        assert_eq!(next.binding_state, BindingState::ManagedDeterministicFresh);
        assert_eq!(next.last_deterministic_at, Some(t0()));
    }

    #[test]
    fn deterministic_recovered_noop_on_unmanaged() {
        let b = make_binding("%1", 0);
        let at = t0() + TimeDelta::seconds(5);
        let event = BindingEvent::DeterministicRecovered { at };

        let next = apply_binding_event(&b, &event);

        assert_eq!(next.binding_state, BindingState::Unmanaged);
    }

    // ── 15. NoAgent on non-heuristic states: streak increments but no demotion ─

    #[test]
    fn no_agent_on_deterministic_fresh_no_demotion() {
        let mut b = make_binding("%1", 0);
        b.binding_state = BindingState::ManagedDeterministicFresh;
        b.session_key = Some("sess-001".into());
        b.no_agent_streak = 0;

        let at1 = t0() + TimeDelta::seconds(1);
        let b1 = apply_binding_event(&b, &BindingEvent::NoAgentObserved { at: at1 });
        assert_eq!(b1.no_agent_streak, 1);
        assert_eq!(b1.binding_state, BindingState::ManagedDeterministicFresh);

        let at2 = t0() + TimeDelta::seconds(2);
        let b2 = apply_binding_event(&b1, &BindingEvent::NoAgentObserved { at: at2 });
        assert_eq!(b2.no_agent_streak, 2);
        assert_eq!(
            b2.binding_state,
            BindingState::ManagedDeterministicFresh,
            "deterministic bindings must not be demoted by no-agent streak"
        );
    }

    // ── 16. No-agent streak exactly at threshold ─────────────────────

    #[test]
    fn no_agent_streak_at_exact_threshold_demotes() {
        let mut b = make_binding("%1", 0);
        b.binding_state = BindingState::ManagedHeuristic;
        b.session_key = Some("sess-001".into());
        b.no_agent_streak = NO_AGENT_DEMOTION_STREAK - 1;

        let at = t0() + TimeDelta::seconds(1);
        let next = apply_binding_event(&b, &BindingEvent::NoAgentObserved { at });

        assert_eq!(next.no_agent_streak, NO_AGENT_DEMOTION_STREAK);
        assert_eq!(next.binding_state, BindingState::Unmanaged);
    }

    // ── 17. Representative pane: deterministic handshake wins ────────

    #[test]
    fn representative_deterministic_handshake_wins() {
        let at1 = t0() + TimeDelta::seconds(1);
        let at2 = t0() + TimeDelta::seconds(5);

        let b1 = PaneBinding {
            last_deterministic_at: Some(at1),
            last_activity_at: Some(at2), // more recent activity
            session_key: Some("sess-001".into()),
            ..make_binding("%1", 0)
        };

        let b2 = PaneBinding {
            last_deterministic_at: Some(at2), // more recent deterministic
            last_activity_at: Some(at1),
            session_key: Some("sess-001".into()),
            ..make_binding("%2", 0)
        };

        let bindings = [b1, b2];
        let rep = select_representative(&bindings, "sess-001").expect("should find representative");

        assert_eq!(
            rep.pane_id, "%2",
            "pane with latest deterministic handshake should win"
        );
    }

    // ── 18. Representative pane: activity tie-break ──────────────────

    #[test]
    fn representative_activity_tiebreak() {
        let det_at = t0() + TimeDelta::seconds(1);
        let act1 = t0() + TimeDelta::seconds(10);
        let act2 = t0() + TimeDelta::seconds(20);

        let b1 = PaneBinding {
            last_deterministic_at: Some(det_at),
            last_activity_at: Some(act1),
            session_key: Some("sess-001".into()),
            ..make_binding("%1", 0)
        };

        let b2 = PaneBinding {
            last_deterministic_at: Some(det_at), // same deterministic
            last_activity_at: Some(act2),        // more recent activity
            session_key: Some("sess-001".into()),
            ..make_binding("%2", 0)
        };

        let bindings = [b1, b2];
        let rep = select_representative(&bindings, "sess-001").expect("should find representative");

        assert_eq!(
            rep.pane_id, "%2",
            "pane with latest activity should win on tie"
        );
    }

    // ── 19. Representative pane: lexical order tie-break ─────────────

    #[test]
    fn representative_lexical_tiebreak() {
        let det_at = t0() + TimeDelta::seconds(1);
        let act_at = t0() + TimeDelta::seconds(10);

        let b1 = PaneBinding {
            last_deterministic_at: Some(det_at),
            last_activity_at: Some(act_at),
            session_key: Some("sess-001".into()),
            ..make_binding("%1", 0)
        };

        let b2 = PaneBinding {
            last_deterministic_at: Some(det_at),
            last_activity_at: Some(act_at),
            session_key: Some("sess-001".into()),
            ..make_binding("%2", 0)
        };

        let bindings = [b1, b2];
        let rep = select_representative(&bindings, "sess-001").expect("should find representative");

        assert_eq!(
            rep.pane_id, "%1",
            "smallest pane_id should win on lexical tie-break"
        );
    }

    // ── 20. Representative pane: filters by session_key ──────────────

    #[test]
    fn representative_filters_by_session_key() {
        let det_at = t0() + TimeDelta::seconds(10);

        let b1 = PaneBinding {
            last_deterministic_at: Some(det_at),
            last_activity_at: Some(det_at),
            session_key: Some("sess-001".into()),
            ..make_binding("%1", 0)
        };

        let b2 = PaneBinding {
            last_deterministic_at: Some(det_at),
            last_activity_at: Some(det_at),
            session_key: Some("sess-002".into()), // different session
            ..make_binding("%2", 0)
        };

        let bindings = [b1, b2];
        let rep = select_representative(&bindings, "sess-001").expect("should find representative");
        assert_eq!(rep.pane_id, "%1");

        let rep2 =
            select_representative(&bindings, "sess-002").expect("should find representative");
        assert_eq!(rep2.pane_id, "%2");
    }

    // ── 21. Representative pane: empty input returns None ────────────

    #[test]
    fn representative_empty_returns_none() {
        let bindings: Vec<PaneBinding> = vec![];
        let rep = select_representative(&bindings, "sess-001");
        assert!(rep.is_none());
    }

    // ── 22. Representative pane: no matching session returns None ────

    #[test]
    fn representative_no_matching_session_returns_none() {
        let b = PaneBinding {
            session_key: Some("sess-999".into()),
            ..make_binding("%1", 0)
        };
        let bindings = [b];
        let rep = select_representative(&bindings, "sess-001");
        assert!(rep.is_none());
    }

    // ── 23. Heuristic on already-managed-heuristic updates activity ──

    #[test]
    fn heuristic_on_managed_heuristic_updates_activity() {
        let mut b = make_binding("%1", 0);
        b.binding_state = BindingState::ManagedHeuristic;
        b.session_key = Some("sess-001".into());
        b.last_activity_at = Some(t0());

        let at = t0() + TimeDelta::seconds(10);
        let event = BindingEvent::HeuristicDetected {
            session_key: "sess-001".into(),
            confidence: 0.9,
            at,
        };

        let next = apply_binding_event(&b, &event);

        assert_eq!(next.binding_state, BindingState::ManagedHeuristic);
        assert_eq!(next.last_activity_at, Some(at));
        // session_key unchanged
        assert_eq!(next.session_key.as_deref(), Some("sess-001"));
    }

    // ── 24. ManagedDeterministicStale -> ManagedDeterministicFresh via handshake ─

    #[test]
    fn deterministic_stale_to_fresh_via_handshake() {
        let mut b = make_binding("%1", 0);
        b.binding_state = BindingState::ManagedDeterministicStale;
        b.session_key = Some("sess-001".into());
        b.last_deterministic_at = Some(t0());

        let at = t0() + TimeDelta::seconds(20);
        let event = BindingEvent::DeterministicHandshake {
            session_key: "sess-001".into(),
            at,
        };

        let next = apply_binding_event(&b, &event);

        assert_eq!(next.binding_state, BindingState::ManagedDeterministicFresh);
        assert_eq!(next.last_deterministic_at, Some(at));
    }

    // ── 25. Full lifecycle: Unmanaged -> Heuristic -> Deterministic -> Stale -> Recovery ─

    #[test]
    fn full_binding_lifecycle() {
        let b0 = make_binding("%1", 0);
        assert_eq!(b0.binding_state, BindingState::Unmanaged);

        // Step 1: heuristic detection
        let t1 = t0() + TimeDelta::seconds(1);
        let b1 = apply_binding_event(
            &b0,
            &BindingEvent::HeuristicDetected {
                session_key: "sess-001".into(),
                confidence: 0.86,
                at: t1,
            },
        );
        assert_eq!(b1.binding_state, BindingState::ManagedHeuristic);

        // Step 2: deterministic handshake
        let t2 = t0() + TimeDelta::seconds(5);
        let b2 = apply_binding_event(
            &b1,
            &BindingEvent::DeterministicHandshake {
                session_key: "sess-001".into(),
                at: t2,
            },
        );
        assert_eq!(b2.binding_state, BindingState::ManagedDeterministicFresh);
        assert_eq!(b2.last_deterministic_at, Some(t2));

        // Step 3: freshness expired
        let t3 = t0() + TimeDelta::seconds(10);
        let b3 = apply_binding_event(&b2, &BindingEvent::FreshnessExpired { at: t3 });
        assert_eq!(b3.binding_state, BindingState::ManagedDeterministicStale);

        // Step 4: deterministic recovered
        let t4 = t0() + TimeDelta::seconds(15);
        let b4 = apply_binding_event(&b3, &BindingEvent::DeterministicRecovered { at: t4 });
        assert_eq!(b4.binding_state, BindingState::ManagedDeterministicFresh);
        assert_eq!(b4.last_deterministic_at, Some(t4));

        // Session key preserved throughout
        assert_eq!(b4.session_key.as_deref(), Some("sess-001"));
    }

    // ── 26. Pane reuse full cycle: reuse then rebind after grace ─────

    #[test]
    fn pane_reuse_full_cycle() {
        let mut b = make_binding("%1", 0);
        b.binding_state = BindingState::ManagedDeterministicFresh;
        b.session_key = Some("sess-001".into());

        // Reuse
        let reuse_at = t0() + TimeDelta::seconds(60);
        let b1 = apply_binding_event(
            &b,
            &BindingEvent::PaneReused {
                birth_ts: reuse_at,
                at: reuse_at,
            },
        );
        assert_eq!(b1.generation, 1);
        assert_eq!(b1.binding_state, BindingState::Unmanaged);
        assert!(b1.is_tombstoned(reuse_at + TimeDelta::seconds(1)));

        // Attempt bind during grace: blocked
        let during_grace = reuse_at + TimeDelta::seconds(10);
        let b2 = apply_binding_event(
            &b1,
            &BindingEvent::HeuristicDetected {
                session_key: "sess-002".into(),
                confidence: 0.86,
                at: during_grace,
            },
        );
        assert_eq!(b2.binding_state, BindingState::Unmanaged);

        // After grace: allowed
        let after_grace = reuse_at + TimeDelta::seconds(TOMBSTONE_GRACE_SECS as i64 + 1);
        let b3 = apply_binding_event(
            &b2,
            &BindingEvent::HeuristicDetected {
                session_key: "sess-002".into(),
                confidence: 0.86,
                at: after_grace,
            },
        );
        assert_eq!(b3.binding_state, BindingState::ManagedHeuristic);
        assert_eq!(b3.session_key.as_deref(), Some("sess-002"));
    }

    // ── 27. instance_id returns correct PaneInstanceId ───────────────

    #[test]
    fn instance_id_returns_correct_values() {
        let b = make_binding("%5", 3);
        let id = b.instance_id();
        assert_eq!(id.pane_id, "%5");
        assert_eq!(id.generation, 3);
        assert_eq!(id.birth_ts, t0());
    }

    // ── 28. Default BindingState is Unmanaged ────────────────────────

    #[test]
    fn binding_state_default_is_unmanaged() {
        assert_eq!(BindingState::default(), BindingState::Unmanaged);
    }

    // ── 29. Serde roundtrip for PaneBinding ──────────────────────────

    #[test]
    fn pane_binding_serde_roundtrip() {
        let mut b = make_binding("%1", 2);
        b.binding_state = BindingState::ManagedDeterministicFresh;
        b.session_key = Some("sess-001".into());
        b.bound_at = Some(t0());
        b.last_activity_at = Some(t0());
        b.last_deterministic_at = Some(t0());
        b.no_agent_streak = 1;
        b.tombstone_until = Some(t0() + TimeDelta::seconds(120));

        let json = serde_json::to_string(&b).expect("serialize");
        let back: PaneBinding = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(b, back);
    }

    // ── 30. Serde roundtrip for BindingState ─────────────────────────

    #[test]
    fn binding_state_serde_roundtrip() {
        let states = [
            BindingState::Unmanaged,
            BindingState::ManagedHeuristic,
            BindingState::ManagedDeterministicFresh,
            BindingState::ManagedDeterministicStale,
        ];
        for state in states {
            let json = serde_json::to_string(&state).expect("serialize");
            let back: BindingState = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(state, back);
        }
    }

    // ── 31. Serde roundtrip for BindingEvent ─────────────────────────

    #[test]
    fn binding_event_serde_roundtrip() {
        let event = BindingEvent::DeterministicHandshake {
            session_key: "sess-001".into(),
            at: t0(),
        };
        let json = serde_json::to_string(&event).expect("serialize");
        let back: BindingEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event, back);
    }

    // ── 32. Representative: heuristic-only panes (no deterministic_at) ─

    #[test]
    fn representative_heuristic_only_panes() {
        let act1 = t0() + TimeDelta::seconds(5);
        let act2 = t0() + TimeDelta::seconds(10);

        let b1 = PaneBinding {
            binding_state: BindingState::ManagedHeuristic,
            last_deterministic_at: None,
            last_activity_at: Some(act1),
            session_key: Some("sess-001".into()),
            ..make_binding("%1", 0)
        };

        let b2 = PaneBinding {
            binding_state: BindingState::ManagedHeuristic,
            last_deterministic_at: None,
            last_activity_at: Some(act2), // more recent
            session_key: Some("sess-001".into()),
            ..make_binding("%2", 0)
        };

        let bindings = [b1, b2];
        let rep = select_representative(&bindings, "sess-001").expect("should find representative");

        assert_eq!(
            rep.pane_id, "%2",
            "with no deterministic handshake, latest activity should win"
        );
    }

    // ── 33. is_tombstoned helper ─────────────────────────────────────

    #[test]
    fn is_tombstoned_helper() {
        let mut b = make_binding("%1", 0);

        // No tombstone
        assert!(!b.is_tombstoned(t0()));

        // Set tombstone
        let grace_end = t0() + TimeDelta::seconds(120);
        b.tombstone_until = Some(grace_end);

        // Before grace end: tombstoned
        assert!(b.is_tombstoned(t0() + TimeDelta::seconds(60)));

        // At grace end: not tombstoned (now < until is false when now == until)
        assert!(!b.is_tombstoned(grace_end));

        // After grace end: not tombstoned
        assert!(!b.is_tombstoned(grace_end + TimeDelta::seconds(1)));
    }

    // ── 34. Agent observed on unmanaged: just resets streak ──────────

    #[test]
    fn agent_observed_on_unmanaged() {
        let mut b = make_binding("%1", 0);
        b.no_agent_streak = 5;

        let at = t0() + TimeDelta::seconds(1);
        let next = apply_binding_event(&b, &BindingEvent::AgentObserved { at });

        assert_eq!(next.no_agent_streak, 0);
        assert_eq!(next.binding_state, BindingState::Unmanaged);
        assert_eq!(next.last_activity_at, Some(at));
    }
}

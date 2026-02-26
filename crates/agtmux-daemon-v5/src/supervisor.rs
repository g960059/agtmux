//! Supervisor restart policy, startup order, and UI label semantics.
//!
//! Pure, testable state machines with no IO or async dependencies.
//!
//! Task ref: T-060

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use agtmux_core_v5::types::{EvidenceMode, PanePresence};

// ─── Restart Policy ──────────────────────────────────────────────

/// Configuration for the supervisor restart policy.
///
/// Exponential backoff with a failure budget and hold-down escalation.
/// Jitter (`jitter_pct`) is declared here but MUST be applied by the
/// runtime caller — the pure state machine returns pre-jitter delays.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RestartPolicy {
    /// Initial backoff delay in milliseconds (default 1000).
    pub initial_backoff_ms: u64,
    /// Backoff multiplier per attempt (default 2.0).
    pub multiplier: f64,
    /// Maximum backoff delay in milliseconds (default 30000).
    pub max_backoff_ms: u64,
    /// Jitter percentage applied by the runtime layer (default 0.20 = +/-20%).
    pub jitter_pct: f64,
    /// Maximum number of failures within `budget_window_ms` before hold-down (default 5).
    pub failure_budget: u32,
    /// Sliding window for failure budget counting in milliseconds (default 600_000 = 10min).
    pub budget_window_ms: u64,
    /// Hold-down duration in milliseconds when budget is exhausted (default 300_000 = 5min).
    pub holddown_ms: u64,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self {
            initial_backoff_ms: 1_000,
            multiplier: 2.0,
            max_backoff_ms: 30_000,
            jitter_pct: 0.20,
            failure_budget: 5,
            budget_window_ms: 600_000,
            holddown_ms: 300_000,
        }
    }
}

// ─── Supervisor State Machine ────────────────────────────────────

/// Current state of the supervisor state machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SupervisorState {
    /// Normal operation — process is running or can be started immediately.
    Ready,
    /// In exponential backoff after a failure.
    Restarting {
        /// Zero-based attempt counter (0 = first failure just occurred).
        attempt: u32,
        /// Scheduled restart time in epoch milliseconds.
        next_restart_ms: u64,
    },
    /// Failure budget exhausted; escalation required.
    HoldDown {
        /// Epoch millisecond at which hold-down expires.
        until_ms: u64,
    },
}

/// Decision returned by the supervisor after recording a failure or success.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestartDecision {
    /// Schedule a restart after `after_ms` milliseconds (before jitter).
    Restart { after_ms: u64 },
    /// Failure budget exhausted; enter hold-down for `duration_ms`.
    HoldDown { duration_ms: u64 },
    /// Process recovered — back to normal operation.
    Ready,
}

/// Tracks supervisor restart state for a single supervised process.
///
/// Pure, deterministic state machine. All time values are passed in as
/// parameters (no system clock access).
#[derive(Debug, Clone)]
pub struct SupervisorTracker {
    policy: RestartPolicy,
    state: SupervisorState,
    failure_timestamps: Vec<u64>,
}

impl SupervisorTracker {
    /// Create a new tracker with the given restart policy.
    pub fn new(policy: RestartPolicy) -> Self {
        Self {
            policy,
            state: SupervisorState::Ready,
            failure_timestamps: Vec::new(),
        }
    }

    /// Record a process failure at `now_ms` (epoch milliseconds).
    ///
    /// Returns the restart decision:
    /// - `Restart` with the computed backoff delay (before jitter).
    /// - `HoldDown` if the failure budget is exhausted.
    ///
    /// The caller is responsible for applying jitter (`policy.jitter_pct`)
    /// to the returned delay before actually scheduling the restart.
    pub fn record_failure(&mut self, now_ms: u64) -> RestartDecision {
        // 0. If currently in hold-down, check if it has expired.
        //    If still active, return HoldDown. If expired, reset and continue.
        if let SupervisorState::HoldDown { until_ms } = self.state {
            if now_ms < until_ms {
                return RestartDecision::HoldDown {
                    duration_ms: until_ms.saturating_sub(now_ms),
                };
            }
            // Hold-down expired: reset state and clear old failure history
            self.state = SupervisorState::Ready;
            self.failure_timestamps.clear();
        }

        // 1. Record this failure timestamp.
        self.failure_timestamps.push(now_ms);

        // 2. Prune failures outside the budget window.
        let window_start = now_ms.saturating_sub(self.policy.budget_window_ms);
        self.failure_timestamps.retain(|&ts| ts >= window_start);

        // 3. Check failure budget (skip if budget is 0 — treat as "no budget limit").
        if self.policy.failure_budget > 0
            && self.failure_timestamps.len() >= self.policy.failure_budget as usize
        {
            let until_ms = now_ms.saturating_add(self.policy.holddown_ms);
            self.state = SupervisorState::HoldDown { until_ms };
            return RestartDecision::HoldDown {
                duration_ms: self.policy.holddown_ms,
            };
        }

        // 4. Compute exponential backoff.
        let attempt = match &self.state {
            SupervisorState::Restarting { attempt, .. } => *attempt + 1,
            _ => 0,
        };

        let backoff_raw =
            (self.policy.initial_backoff_ms as f64) * self.policy.multiplier.powi(attempt as i32);
        let backoff = (backoff_raw as u64).min(self.policy.max_backoff_ms);

        let next_restart_ms = now_ms.saturating_add(backoff);
        self.state = SupervisorState::Restarting {
            attempt,
            next_restart_ms,
        };

        RestartDecision::Restart { after_ms: backoff }
    }

    /// Record a successful process start / recovery.
    ///
    /// Resets the state machine to `Ready` and clears failure history.
    pub fn record_success(&mut self) -> RestartDecision {
        self.state = SupervisorState::Ready;
        self.failure_timestamps.clear();
        RestartDecision::Ready
    }

    /// Current supervisor state.
    pub fn state(&self) -> &SupervisorState {
        &self.state
    }

    /// Whether the supervisor is in hold-down (budget exhausted).
    pub fn is_hold_down(&self) -> bool {
        matches!(self.state, SupervisorState::HoldDown { .. })
    }
}

// ─── Startup Order ───────────────────────────────────────────────

/// Logical startup stage for system components.
///
/// Components within the same stage may start concurrently;
/// stages are ordered by dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ComponentStage {
    /// Data sources: codex appserver, claude hooks, poller.
    Sources,
    /// Aggregation / gateway layer.
    Gateway,
    /// Resolver + projection daemon.
    Daemon,
    /// CLI / TUI / GUI clients.
    Ui,
}

/// Returns the canonical startup order for system components.
///
/// Sources must be available before the gateway can aggregate,
/// the gateway before the daemon can resolve, and the daemon
/// before UI clients can connect.
pub fn startup_order() -> &'static [ComponentStage] {
    &[
        ComponentStage::Sources,
        ComponentStage::Gateway,
        ComponentStage::Daemon,
        ComponentStage::Ui,
    ]
}

// ─── UI Label Formatting ─────────────────────────────────────────

/// Format a pane's display label for the UI.
///
/// Returns e.g. `"agents (deterministic)"`, `"agents (heuristic)"`, `"unmanaged"`.
///
/// The word "agents" is always English (project requirement).
pub fn format_pane_label(presence: PanePresence, evidence_mode: EvidenceMode) -> String {
    match presence {
        PanePresence::Managed => match evidence_mode {
            EvidenceMode::Deterministic => "agents (deterministic)".to_owned(),
            EvidenceMode::Heuristic => "agents (heuristic)".to_owned(),
            EvidenceMode::None => "agents".to_owned(),
        },
        PanePresence::Unmanaged => "unmanaged".to_owned(),
    }
}

/// Whether to show the unmanaged badge on this pane.
///
/// Badge shown ONLY on unmanaged panes (FR-031).
pub fn show_unmanaged_badge(presence: PanePresence) -> bool {
    presence == PanePresence::Unmanaged
}

// ─── Dependency Readiness Gate ────────────────────────────────────

/// Tracks readiness of upstream dependencies.
///
/// Each dependency starts as not-ready and must be explicitly marked ready.
/// The gate is considered open only when all dependencies are ready.
///
/// Task ref: T-052
#[derive(Debug, Clone)]
pub struct DependencyGate {
    /// Map of dependency name -> ready status.
    deps: HashMap<String, bool>,
}

impl DependencyGate {
    /// Create a new gate with the given dependency names. All start as not-ready.
    pub fn new(dependency_names: &[&str]) -> Self {
        let deps = dependency_names
            .iter()
            .map(|&name| (name.to_owned(), false))
            .collect();
        Self { deps }
    }

    /// Mark a dependency as ready. Returns `false` if the name is unknown.
    pub fn mark_ready(&mut self, name: &str) -> bool {
        match self.deps.get_mut(name) {
            Some(ready) => {
                *ready = true;
                true
            }
            None => false,
        }
    }

    /// Mark a dependency as not ready. Returns `false` if the name is unknown.
    pub fn mark_unready(&mut self, name: &str) -> bool {
        match self.deps.get_mut(name) {
            Some(ready) => {
                *ready = false;
                true
            }
            None => false,
        }
    }

    /// Check if all dependencies are ready.
    pub fn all_ready(&self) -> bool {
        self.deps.values().all(|&ready| ready)
    }

    /// List dependencies that are not ready.
    pub fn unready_deps(&self) -> Vec<&str> {
        let mut unready: Vec<&str> = self
            .deps
            .iter()
            .filter(|&(_, &ready)| !ready)
            .map(|(name, _)| name.as_str())
            .collect();
        unready.sort();
        unready
    }

    /// Total dependency count.
    pub fn count(&self) -> usize {
        self.deps.len()
    }
}

// ─── Failure Budget Tracker ──────────────────────────────────────

/// Standalone failure budget tracker using a sliding time window.
///
/// Records failure timestamps and determines whether the configured
/// budget has been exhausted within the sliding window. Can be reused
/// independently of `SupervisorTracker`.
///
/// Task ref: T-052
#[derive(Debug, Clone)]
pub struct FailureBudget {
    timestamps: Vec<u64>,
    budget: u32,
    window_ms: u64,
}

impl FailureBudget {
    /// Create a new budget allowing `budget` failures within `window_ms` milliseconds.
    pub fn new(budget: u32, window_ms: u64) -> Self {
        Self {
            timestamps: Vec::new(),
            budget,
            window_ms,
        }
    }

    /// Record a failure at `now_ms`. Returns `true` if the budget is now exhausted.
    ///
    /// A budget of 0 means "no limit" — always returns `false`.
    pub fn record(&mut self, now_ms: u64) -> bool {
        self.timestamps.push(now_ms);
        self.prune(now_ms);
        self.budget > 0 && self.timestamps.len() >= self.budget as usize
    }

    /// Reset the budget, clearing all recorded failures.
    pub fn reset(&mut self) {
        self.timestamps.clear();
    }

    /// Number of remaining failures before exhaustion at time `now_ms`.
    pub fn remaining(&self, now_ms: u64) -> u32 {
        let window_start = now_ms.saturating_sub(self.window_ms);
        let active = self
            .timestamps
            .iter()
            .filter(|&&ts| ts >= window_start)
            .count() as u32;
        self.budget.saturating_sub(active)
    }

    /// Is the budget exhausted right now (at time `now_ms`)?
    ///
    /// A budget of 0 means "no limit" — always returns `false`.
    pub fn is_exhausted(&self, now_ms: u64) -> bool {
        if self.budget == 0 {
            return false;
        }
        let window_start = now_ms.saturating_sub(self.window_ms);
        let active = self
            .timestamps
            .iter()
            .filter(|&&ts| ts >= window_start)
            .count();
        active >= self.budget as usize
    }

    /// Prune timestamps outside the sliding window.
    fn prune(&mut self, now_ms: u64) {
        let window_start = now_ms.saturating_sub(self.window_ms);
        self.timestamps.retain(|&ts| ts >= window_start);
    }
}

// ─── Hold-Down Timer ─────────────────────────────────────────────

/// A timer that enforces a hold-down (cool-off) period.
///
/// Once started, the timer is active for `duration_ms` milliseconds.
/// During this period, the caller should suppress restarts or other
/// actions.
///
/// Not thread-safe: callers must serialize access (e.g., wrap in a `Mutex`)
/// or confine a single instance to one thread. Cloning copies the current
/// `until_ms`, so clones do not share runtime state.
///
/// Task ref: T-052
#[derive(Debug, Clone)]
pub struct HoldDownTimer {
    until_ms: Option<u64>,
    duration_ms: u64,
}

impl HoldDownTimer {
    /// Create a new hold-down timer with the given duration. Not active initially.
    pub fn new(duration_ms: u64) -> Self {
        Self {
            until_ms: None,
            duration_ms,
        }
    }

    /// Start the hold-down period from `now_ms`.
    pub fn start(&mut self, now_ms: u64) {
        self.until_ms = Some(now_ms.saturating_add(self.duration_ms));
    }

    /// Is hold-down currently active at time `now_ms`?
    pub fn is_active(&self, now_ms: u64) -> bool {
        match self.until_ms {
            Some(until) => now_ms < until,
            None => false,
        }
    }

    /// Clear the hold-down, making it inactive immediately.
    pub fn clear(&mut self) {
        self.until_ms = None;
    }

    /// Remaining time in hold-down at `now_ms` (0 if not active).
    pub fn remaining_ms(&self, now_ms: u64) -> u64 {
        match self.until_ms {
            Some(until) if now_ms < until => until.saturating_sub(now_ms),
            _ => 0,
        }
    }
}

// ─── Tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Restart policy defaults ─────────────────────────────────

    #[test]
    fn default_policy_values() {
        let p = RestartPolicy::default();
        assert_eq!(p.initial_backoff_ms, 1_000);
        assert!((p.multiplier - 2.0).abs() < f64::EPSILON);
        assert_eq!(p.max_backoff_ms, 30_000);
        assert!((p.jitter_pct - 0.20).abs() < f64::EPSILON);
        assert_eq!(p.failure_budget, 5);
        assert_eq!(p.budget_window_ms, 600_000);
        assert_eq!(p.holddown_ms, 300_000);
    }

    // ── Restart state machine ───────────────────────────────────

    #[test]
    fn first_failure_returns_restart() {
        let mut tracker = SupervisorTracker::new(RestartPolicy::default());
        let decision = tracker.record_failure(1_000);
        // attempt=0 → backoff = initial * 2^0 = 1000
        assert_eq!(decision, RestartDecision::Restart { after_ms: 1_000 });
    }

    #[test]
    fn second_failure_doubles_backoff() {
        let mut tracker = SupervisorTracker::new(RestartPolicy::default());
        tracker.record_failure(1_000);
        let decision = tracker.record_failure(3_000);
        // attempt=1 → backoff = 1000 * 2^1 = 2000
        assert_eq!(decision, RestartDecision::Restart { after_ms: 2_000 });
    }

    #[test]
    fn backoff_capped_at_max() {
        let policy = RestartPolicy {
            max_backoff_ms: 30_000,
            ..Default::default()
        };
        let mut tracker = SupervisorTracker::new(policy);
        // Drive up the attempt counter with spaced-out failures
        // so we don't hit the budget (budget=5, window=600s).
        for i in 0..4 {
            tracker.record_failure(i * 10_000);
        }
        // attempt=3 → backoff raw = 1000 * 2^3 = 8000 (still under max)
        // attempt=4 would be 16000, still under. Let's use a policy with lower max.
        let policy2 = RestartPolicy {
            initial_backoff_ms: 1_000,
            multiplier: 2.0,
            max_backoff_ms: 5_000,
            failure_budget: 20, // high budget so we don't hit holddown
            ..Default::default()
        };
        let mut tracker2 = SupervisorTracker::new(policy2);
        // attempt 0: 1000, 1: 2000, 2: 4000, 3: 8000 → capped at 5000
        tracker2.record_failure(0);
        tracker2.record_failure(1_000);
        tracker2.record_failure(3_000);
        let decision = tracker2.record_failure(7_000);
        assert_eq!(decision, RestartDecision::Restart { after_ms: 5_000 });
    }

    #[test]
    fn budget_exhaustion_triggers_holddown() {
        let mut tracker = SupervisorTracker::new(RestartPolicy::default());
        // 5 failures within the 10-minute window → holddown
        for i in 0..4 {
            let decision = tracker.record_failure(i * 1_000);
            assert!(
                matches!(decision, RestartDecision::Restart { .. }),
                "failure {i} should be Restart"
            );
        }
        let decision = tracker.record_failure(4_000);
        assert_eq!(
            decision,
            RestartDecision::HoldDown {
                duration_ms: 300_000
            }
        );
    }

    #[test]
    fn old_failures_pruned_from_window() {
        let policy = RestartPolicy {
            budget_window_ms: 10_000, // 10s window for easy testing
            failure_budget: 3,
            ..Default::default()
        };
        let mut tracker = SupervisorTracker::new(policy);
        // Two failures at t=0 and t=1000
        tracker.record_failure(0);
        tracker.record_failure(1_000);
        // Third failure at t=20_000 — the first two are outside the 10s window
        // so only 1 failure in window; should NOT trigger holddown.
        let decision = tracker.record_failure(20_000);
        assert!(
            matches!(decision, RestartDecision::Restart { .. }),
            "old failures should be pruned; expected Restart, got {decision:?}"
        );
    }

    #[test]
    fn success_resets_state() {
        let mut tracker = SupervisorTracker::new(RestartPolicy::default());
        tracker.record_failure(1_000);
        tracker.record_failure(2_000);
        let decision = tracker.record_success();
        assert_eq!(decision, RestartDecision::Ready);
        assert_eq!(*tracker.state(), SupervisorState::Ready);
    }

    #[test]
    fn holddown_state_check() {
        let policy = RestartPolicy {
            failure_budget: 2,
            ..Default::default()
        };
        let mut tracker = SupervisorTracker::new(policy);
        tracker.record_failure(0);
        tracker.record_failure(1_000);
        assert!(tracker.is_hold_down());
    }

    #[test]
    fn ready_state_after_construction() {
        let tracker = SupervisorTracker::new(RestartPolicy::default());
        assert_eq!(*tracker.state(), SupervisorState::Ready);
        assert!(!tracker.is_hold_down());
    }

    #[test]
    fn multiple_failure_recovery_cycle() {
        let mut tracker = SupervisorTracker::new(RestartPolicy::default());

        // First failure → Restart
        let d1 = tracker.record_failure(1_000);
        assert!(matches!(d1, RestartDecision::Restart { .. }));

        // Second failure → Restart (higher backoff)
        let d2 = tracker.record_failure(3_000);
        assert!(matches!(d2, RestartDecision::Restart { .. }));

        // Success → Ready
        let d3 = tracker.record_success();
        assert_eq!(d3, RestartDecision::Ready);
        assert_eq!(*tracker.state(), SupervisorState::Ready);

        // Another failure after recovery → Restart (attempt resets)
        let d4 = tracker.record_failure(10_000);
        assert_eq!(d4, RestartDecision::Restart { after_ms: 1_000 });
    }

    // ── Startup order ───────────────────────────────────────────

    #[test]
    fn startup_order_length() {
        assert_eq!(startup_order().len(), 4);
    }

    #[test]
    fn startup_order_sources_first() {
        assert_eq!(startup_order()[0], ComponentStage::Sources);
    }

    #[test]
    fn startup_order_ui_last() {
        assert_eq!(startup_order()[3], ComponentStage::Ui);
    }

    // ── UI label formatting ─────────────────────────────────────

    #[test]
    fn label_managed_deterministic() {
        assert_eq!(
            format_pane_label(PanePresence::Managed, EvidenceMode::Deterministic),
            "agents (deterministic)"
        );
    }

    #[test]
    fn label_managed_heuristic() {
        assert_eq!(
            format_pane_label(PanePresence::Managed, EvidenceMode::Heuristic),
            "agents (heuristic)"
        );
    }

    #[test]
    fn label_managed_none() {
        assert_eq!(
            format_pane_label(PanePresence::Managed, EvidenceMode::None),
            "agents"
        );
    }

    #[test]
    fn label_unmanaged_any() {
        // Unmanaged always produces "unmanaged" regardless of evidence mode.
        assert_eq!(
            format_pane_label(PanePresence::Unmanaged, EvidenceMode::Deterministic),
            "unmanaged"
        );
        assert_eq!(
            format_pane_label(PanePresence::Unmanaged, EvidenceMode::Heuristic),
            "unmanaged"
        );
        assert_eq!(
            format_pane_label(PanePresence::Unmanaged, EvidenceMode::None),
            "unmanaged"
        );
    }

    #[test]
    fn badge_unmanaged_true() {
        assert!(show_unmanaged_badge(PanePresence::Unmanaged));
    }

    #[test]
    fn badge_managed_false() {
        assert!(!show_unmanaged_badge(PanePresence::Managed));
    }

    // ── HoldDown recovery ───────────────────────────────────────

    #[test]
    fn holddown_still_active_returns_holddown() {
        let policy = RestartPolicy {
            failure_budget: 2,
            holddown_ms: 300_000,
            ..Default::default()
        };
        let mut tracker = SupervisorTracker::new(policy);
        tracker.record_failure(0);
        tracker.record_failure(1_000); // → HoldDown until 301_000
        assert!(tracker.is_hold_down());

        // Failure during hold-down returns remaining duration
        let decision = tracker.record_failure(100_000);
        assert!(matches!(decision, RestartDecision::HoldDown { .. }));
        assert!(tracker.is_hold_down());
    }

    #[test]
    fn holddown_expired_resets_and_restarts() {
        let policy = RestartPolicy {
            failure_budget: 2,
            holddown_ms: 10_000,
            ..Default::default()
        };
        let mut tracker = SupervisorTracker::new(policy);
        tracker.record_failure(0);
        tracker.record_failure(1_000); // → HoldDown until 11_000
        assert!(tracker.is_hold_down());

        // Failure AFTER hold-down expires → fresh Restart (not another HoldDown)
        let decision = tracker.record_failure(20_000);
        assert_eq!(decision, RestartDecision::Restart { after_ms: 1_000 });
        assert!(!tracker.is_hold_down());
    }

    #[test]
    fn failure_budget_zero_allows_restart() {
        let policy = RestartPolicy {
            failure_budget: 0,
            ..Default::default()
        };
        let mut tracker = SupervisorTracker::new(policy);
        let decision = tracker.record_failure(1_000);
        // budget=0 means "no budget limit" — should always Restart, never HoldDown
        assert_eq!(decision, RestartDecision::Restart { after_ms: 1_000 });
        assert!(!tracker.is_hold_down());
    }

    // ── DependencyGate (T-052) ─────────────────────────────────

    #[test]
    fn dep_gate_all_unready_initially() {
        let gate = DependencyGate::new(&["db", "cache", "queue"]);
        assert!(!gate.all_ready());
        assert_eq!(gate.count(), 3);
    }

    #[test]
    fn dep_gate_mark_ready() {
        let mut gate = DependencyGate::new(&["db", "cache"]);
        assert!(gate.mark_ready("db"));
        assert!(!gate.all_ready()); // cache still unready
    }

    #[test]
    fn dep_gate_all_ready_when_all_marked() {
        let mut gate = DependencyGate::new(&["db", "cache"]);
        gate.mark_ready("db");
        gate.mark_ready("cache");
        assert!(gate.all_ready());
    }

    #[test]
    fn dep_gate_unready_deps_lists_missing() {
        let mut gate = DependencyGate::new(&["db", "cache", "queue"]);
        gate.mark_ready("cache");
        let unready = gate.unready_deps();
        assert_eq!(unready.len(), 2);
        assert!(unready.contains(&"db"));
        assert!(unready.contains(&"queue"));
    }

    #[test]
    fn dep_gate_mark_unknown_returns_false() {
        let mut gate = DependencyGate::new(&["db"]);
        assert!(!gate.mark_ready("nonexistent"));
        assert!(!gate.mark_unready("nonexistent"));
    }

    #[test]
    fn dep_gate_mark_unready_reverts() {
        let mut gate = DependencyGate::new(&["db"]);
        gate.mark_ready("db");
        assert!(gate.all_ready());
        gate.mark_unready("db");
        assert!(!gate.all_ready());
    }

    #[test]
    fn dep_gate_unready_deps_empty_when_all_ready_then_reappears() {
        let mut gate = DependencyGate::new(&["db", "cache"]);
        gate.mark_ready("db");
        gate.mark_ready("cache");
        assert!(gate.unready_deps().is_empty());

        gate.mark_unready("cache");
        assert_eq!(gate.unready_deps(), vec!["cache"]);
    }

    // ── FailureBudget (T-052) ──────────────────────────────────

    #[test]
    fn budget_not_exhausted_initially() {
        let budget = FailureBudget::new(3, 10_000);
        assert!(!budget.is_exhausted(0));
    }

    #[test]
    fn budget_exhausted_at_limit() {
        let mut budget = FailureBudget::new(3, 10_000);
        assert!(!budget.record(1_000)); // 1 of 3
        assert!(!budget.record(2_000)); // 2 of 3
        assert!(budget.record(3_000)); // 3 of 3 → exhausted
        assert!(budget.is_exhausted(3_000));
    }

    #[test]
    fn budget_old_failures_pruned() {
        let mut budget = FailureBudget::new(3, 10_000);
        budget.record(0);
        budget.record(1_000);
        // At t=20_000, both old failures are outside the 10s window
        assert!(!budget.record(20_000)); // only 1 in window
        assert!(!budget.is_exhausted(20_000));
    }

    #[test]
    fn budget_remaining_count() {
        let mut budget = FailureBudget::new(5, 10_000);
        assert_eq!(budget.remaining(0), 5);
        budget.record(1_000);
        budget.record(2_000);
        assert_eq!(budget.remaining(3_000), 3);
    }

    #[test]
    fn budget_reset_clears() {
        let mut budget = FailureBudget::new(3, 10_000);
        budget.record(0);
        budget.record(1_000);
        budget.reset();
        assert!(!budget.is_exhausted(2_000));
        assert_eq!(budget.remaining(2_000), 3);
    }

    // ── HoldDownTimer (T-052) ──────────────────────────────────

    #[test]
    fn holddown_not_active_initially() {
        let timer = HoldDownTimer::new(5_000);
        assert!(!timer.is_active(0));
    }

    #[test]
    fn holddown_active_after_start() {
        let mut timer = HoldDownTimer::new(5_000);
        timer.start(1_000);
        assert!(timer.is_active(1_000));
        assert!(timer.is_active(5_999));
    }

    #[test]
    fn holddown_expires_after_duration() {
        let mut timer = HoldDownTimer::new(5_000);
        timer.start(1_000);
        // Active at 5_999 (1_000 + 5_000 - 1)
        assert!(timer.is_active(5_999));
        // Not active at 6_000 (1_000 + 5_000)
        assert!(!timer.is_active(6_000));
    }

    #[test]
    fn holddown_remaining_ms() {
        let mut timer = HoldDownTimer::new(5_000);
        assert_eq!(timer.remaining_ms(0), 0); // not started
        timer.start(1_000);
        assert_eq!(timer.remaining_ms(1_000), 5_000);
        assert_eq!(timer.remaining_ms(3_000), 3_000);
        assert_eq!(timer.remaining_ms(6_000), 0); // expired
    }

    #[test]
    fn holddown_clear_resets() {
        let mut timer = HoldDownTimer::new(5_000);
        timer.start(1_000);
        assert!(timer.is_active(2_000));
        timer.clear();
        assert!(!timer.is_active(2_000));
        assert_eq!(timer.remaining_ms(2_000), 0);
    }
}

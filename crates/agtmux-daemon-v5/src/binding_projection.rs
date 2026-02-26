//! Binding projection with CAS concurrency control.
//!
//! Single-writer projection that manages pane bindings, keyed by `pane_id`.
//! Each binding is versioned; callers may use [`BindingProjection::apply_event_cas`]
//! for compare-and-swap semantics to detect concurrent modification.
//!
//! Task ref: T-053

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use agtmux_core_v5::binding::{BindingEvent, BindingState, PaneBinding, apply_binding_event};

// ─── Types ──────────────────────────────────────────────────────────

/// A pane binding together with its CAS version.
#[derive(Debug, Clone)]
pub struct VersionedBinding {
    /// The underlying pane binding state.
    pub binding: PaneBinding,
    /// CAS version — incremented on every successful mutation.
    pub version: u64,
}

/// Result of applying a binding event through the projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingApplyResult {
    /// The pane that was affected.
    pub pane_id: String,
    /// Binding state before the event was applied.
    pub previous_state: BindingState,
    /// Binding state after the event was applied.
    pub new_state: BindingState,
    /// Whether the binding state actually changed.
    pub changed: bool,
    /// The new CAS version for this pane after the event.
    pub version: u64,
}

/// Error returned when a CAS check fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CasConflict {
    /// The version the caller expected.
    pub expected: u64,
    /// The actual current version of the binding.
    pub actual: u64,
}

impl std::fmt::Display for CasConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "CAS conflict: expected version {}, actual version {}",
            self.expected, self.actual
        )
    }
}

impl std::error::Error for CasConflict {}

// ─── Projection ─────────────────────────────────────────────────────

/// Single-writer binding projection with CAS concurrency control.
///
/// Manages per-pane bindings keyed by `pane_id`. Each mutation increments
/// a monotonic `state_version`, and every per-pane entry carries its own
/// CAS version for optimistic concurrency.
#[derive(Debug, Clone)]
pub struct BindingProjection {
    /// Per-pane binding state, keyed by pane_id.
    bindings: HashMap<String, VersionedBinding>,
    /// Monotonic version counter — incremented on every successful apply.
    state_version: u64,
}

impl BindingProjection {
    /// Create an empty projection with no bindings and version 0.
    pub fn new() -> Self {
        Self {
            bindings: HashMap::new(),
            state_version: 0,
        }
    }

    /// Apply a binding event to the given pane without CAS checking.
    ///
    /// If the pane does not exist, a default `PaneBinding` is created.
    /// The projection's `state_version` is always incremented.
    pub fn apply_event(
        &mut self,
        pane_id: &str,
        event: BindingEvent,
        now: DateTime<Utc>,
    ) -> BindingApplyResult {
        self.state_version += 1;
        let new_version = self.state_version;

        let current_binding = self
            .bindings
            .get(pane_id)
            .map(|vb| &vb.binding)
            .cloned()
            .unwrap_or_else(|| PaneBinding::new(pane_id.to_string(), 0, now));

        let previous_state = current_binding.binding_state;
        let next_binding = apply_binding_event(&current_binding, &event);
        let new_state = next_binding.binding_state;
        let changed = previous_state != new_state;

        self.bindings.insert(
            pane_id.to_string(),
            VersionedBinding {
                binding: next_binding,
                version: new_version,
            },
        );

        BindingApplyResult {
            pane_id: pane_id.to_string(),
            previous_state,
            new_state,
            changed,
            version: new_version,
        }
    }

    /// Apply a binding event with compare-and-swap concurrency control.
    ///
    /// The caller provides the `expected_version` they last observed for
    /// this pane. If the current version does not match, `CasConflict` is
    /// returned and no mutation occurs.
    ///
    /// Special case: `expected_version == 0` means "expect the pane to be
    /// new (not yet in the projection)".
    pub fn apply_event_cas(
        &mut self,
        pane_id: &str,
        event: BindingEvent,
        now: DateTime<Utc>,
        expected_version: u64,
    ) -> Result<BindingApplyResult, CasConflict> {
        let actual_version = self.bindings.get(pane_id).map_or(0, |vb| vb.version);

        if actual_version != expected_version {
            return Err(CasConflict {
                expected: expected_version,
                actual: actual_version,
            });
        }

        Ok(self.apply_event(pane_id, event, now))
    }

    /// Look up the versioned binding for a pane.
    pub fn get_binding(&self, pane_id: &str) -> Option<&VersionedBinding> {
        self.bindings.get(pane_id)
    }

    /// List all bindings, sorted by pane_id.
    pub fn list_bindings(&self) -> Vec<(&str, &VersionedBinding)> {
        let mut entries: Vec<(&str, &VersionedBinding)> =
            self.bindings.iter().map(|(k, v)| (k.as_str(), v)).collect();
        entries.sort_by_key(|(pane_id, _)| *pane_id);
        entries
    }

    /// Current monotonic state version.
    pub fn state_version(&self) -> u64 {
        self.state_version
    }
}

impl Default for BindingProjection {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeDelta;

    fn t0() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-02-25T12:00:00Z")
            .expect("valid RFC3339")
            .with_timezone(&Utc)
    }

    fn agent_observed_event(at: DateTime<Utc>) -> BindingEvent {
        BindingEvent::AgentObserved { at }
    }

    fn heuristic_event(session_key: &str, at: DateTime<Utc>) -> BindingEvent {
        BindingEvent::HeuristicDetected {
            session_key: session_key.to_string(),
            confidence: 0.86,
            at,
        }
    }

    // ── 1. empty_projection ─────────────────────────────────────────

    #[test]
    fn empty_projection() {
        let proj = BindingProjection::new();
        assert_eq!(proj.state_version(), 0);
        assert!(proj.list_bindings().is_empty());
    }

    // ── 2. apply_event_creates_binding ──────────────────────────────

    #[test]
    fn apply_event_creates_binding() {
        let mut proj = BindingProjection::new();
        let now = t0();
        let event = agent_observed_event(now);

        let result = proj.apply_event("%1", event, now);

        assert_eq!(result.pane_id, "%1");
        let vb = proj.get_binding("%1").expect("binding should exist");
        assert_eq!(vb.binding.pane_id, "%1");
        assert_eq!(vb.version, 1);
    }

    // ── 3. apply_event_increments_version ───────────────────────────

    #[test]
    fn apply_event_increments_version() {
        let mut proj = BindingProjection::new();
        let now = t0();

        proj.apply_event("%1", agent_observed_event(now), now);
        assert_eq!(proj.state_version(), 1);

        proj.apply_event("%1", agent_observed_event(now + TimeDelta::seconds(1)), now);
        assert_eq!(proj.state_version(), 2);

        proj.apply_event("%2", agent_observed_event(now + TimeDelta::seconds(2)), now);
        assert_eq!(proj.state_version(), 3);
    }

    // ── 4. apply_event_state_transition ─────────────────────────────

    #[test]
    fn apply_event_state_transition() {
        let mut proj = BindingProjection::new();
        let now = t0();
        let at = now + TimeDelta::seconds(1);

        // First create the pane with AgentObserved (stays Unmanaged)
        proj.apply_event("%1", agent_observed_event(now), now);

        // Then apply HeuristicDetected which should transition Unmanaged -> ManagedHeuristic
        let result = proj.apply_event("%1", heuristic_event("sess-001", at), now);

        assert_eq!(result.previous_state, BindingState::Unmanaged);
        assert_eq!(result.new_state, BindingState::ManagedHeuristic);
        assert!(result.changed);
    }

    // ── 5. apply_event_no_state_change ──────────────────────────────

    #[test]
    fn apply_event_no_state_change() {
        let mut proj = BindingProjection::new();
        let now = t0();

        // AgentObserved on a new (Unmanaged) pane — state stays Unmanaged
        let result = proj.apply_event("%1", agent_observed_event(now), now);
        assert_eq!(result.previous_state, BindingState::Unmanaged);
        assert_eq!(result.new_state, BindingState::Unmanaged);
        assert!(!result.changed);

        // Version still incremented
        assert_eq!(result.version, 1);
    }

    // ── 6. cas_success ──────────────────────────────────────────────

    #[test]
    fn cas_success() {
        let mut proj = BindingProjection::new();
        let now = t0();

        // Create the pane (version becomes 1)
        proj.apply_event("%1", agent_observed_event(now), now);

        // CAS with correct version
        let at = now + TimeDelta::seconds(1);
        let result = proj
            .apply_event_cas("%1", heuristic_event("sess-001", at), now, 1)
            .expect("CAS should succeed");

        assert_eq!(result.new_state, BindingState::ManagedHeuristic);
        assert_eq!(result.version, 2);
    }

    // ── 7. cas_conflict_wrong_version ───────────────────────────────

    #[test]
    fn cas_conflict_wrong_version() {
        let mut proj = BindingProjection::new();
        let now = t0();

        // Create the pane (version becomes 1)
        proj.apply_event("%1", agent_observed_event(now), now);

        // CAS with wrong version
        let at = now + TimeDelta::seconds(1);
        let err = proj
            .apply_event_cas("%1", heuristic_event("sess-001", at), now, 999)
            .expect_err("CAS should fail");

        assert_eq!(err.expected, 999);
        assert_eq!(err.actual, 1);
    }

    // ── 8. cas_conflict_returns_actual_version ──────────────────────

    #[test]
    fn cas_conflict_returns_actual_version() {
        let mut proj = BindingProjection::new();
        let now = t0();

        // Create the pane (version becomes 1), then update (version becomes 2)
        proj.apply_event("%1", agent_observed_event(now), now);
        proj.apply_event("%1", agent_observed_event(now + TimeDelta::seconds(1)), now);

        // CAS with stale version 1
        let at = now + TimeDelta::seconds(2);
        let err = proj
            .apply_event_cas("%1", heuristic_event("sess-001", at), now, 1)
            .expect_err("CAS should fail");

        assert_eq!(
            err.actual, 2,
            "CasConflict must contain the actual current version for retry"
        );
    }

    // ── 9. cas_new_pane_version_zero ────────────────────────────────

    #[test]
    fn cas_new_pane_version_zero() {
        let mut proj = BindingProjection::new();
        let now = t0();
        let at = now + TimeDelta::seconds(1);

        // expected_version=0 on a new pane should succeed
        let result = proj
            .apply_event_cas("%1", heuristic_event("sess-001", at), now, 0)
            .expect("CAS with version 0 on new pane should succeed");

        assert_eq!(result.pane_id, "%1");
        assert_eq!(result.version, 1);
    }

    // ── 10. cas_existing_pane_version_zero_fails ────────────────────

    #[test]
    fn cas_existing_pane_version_zero_fails() {
        let mut proj = BindingProjection::new();
        let now = t0();

        // Create the pane first
        proj.apply_event("%1", agent_observed_event(now), now);

        // CAS with expected_version=0 on existing pane should fail
        let at = now + TimeDelta::seconds(1);
        let err = proj
            .apply_event_cas("%1", heuristic_event("sess-001", at), now, 0)
            .expect_err("CAS with version 0 on existing pane should fail");

        assert_eq!(err.expected, 0);
        assert_eq!(err.actual, 1);
    }

    // ── 11. concurrent_event_simulation ─────────────────────────────

    #[test]
    fn concurrent_event_simulation() {
        let mut proj = BindingProjection::new();
        let now = t0();

        // Create pane — both "callers" observe version 1
        proj.apply_event("%1", agent_observed_event(now), now);
        let observed_version = proj.get_binding("%1").expect("exists").version;
        assert_eq!(observed_version, 1);

        // Caller A succeeds with CAS
        let at_a = now + TimeDelta::seconds(1);
        let result_a = proj
            .apply_event_cas("%1", heuristic_event("sess-A", at_a), now, observed_version)
            .expect("Caller A should succeed");
        assert_eq!(result_a.version, 2);

        // Caller B tries with same stale version — fails
        let at_b = now + TimeDelta::seconds(2);
        let err_b = proj
            .apply_event_cas("%1", heuristic_event("sess-B", at_b), now, observed_version)
            .expect_err("Caller B should fail (stale version)");
        assert_eq!(err_b.expected, 1);
        assert_eq!(err_b.actual, 2);

        // Caller B retries with the actual version from the conflict
        let result_b = proj
            .apply_event_cas("%1", heuristic_event("sess-B", at_b), now, err_b.actual)
            .expect("Caller B retry should succeed");
        assert_eq!(result_b.version, 3);
    }

    // ── 12. list_bindings_sorted ────────────────────────────────────

    #[test]
    fn list_bindings_sorted() {
        let mut proj = BindingProjection::new();
        let now = t0();

        // Insert in non-alphabetical order
        proj.apply_event("%3", agent_observed_event(now), now);
        proj.apply_event("%1", agent_observed_event(now), now);
        proj.apply_event("%2", agent_observed_event(now), now);

        let list = proj.list_bindings();
        let pane_ids: Vec<&str> = list.iter().map(|(id, _)| *id).collect();
        assert_eq!(pane_ids, vec!["%1", "%2", "%3"]);
    }

    // ── 13. get_binding_existing ────────────────────────────────────

    #[test]
    fn get_binding_existing() {
        let mut proj = BindingProjection::new();
        let now = t0();

        proj.apply_event("%1", agent_observed_event(now), now);

        let vb = proj.get_binding("%1");
        assert!(vb.is_some());
        assert_eq!(vb.expect("exists").binding.pane_id, "%1");
    }

    // ── 14. get_binding_missing ─────────────────────────────────────

    #[test]
    fn get_binding_missing() {
        let proj = BindingProjection::new();
        assert!(proj.get_binding("%99").is_none());
    }

    // ── 15. rollback_prevention ─────────────────────────────────────

    #[test]
    fn rollback_prevention() {
        let mut proj = BindingProjection::new();
        let now = t0();
        let at = now + TimeDelta::seconds(1);

        // Create pane in ManagedHeuristic state
        proj.apply_event("%1", heuristic_event("sess-001", at), now);
        let version_before = proj.state_version();
        let state_before = proj
            .get_binding("%1")
            .expect("exists")
            .binding
            .binding_state;

        // Attempt CAS with wrong version
        let at2 = now + TimeDelta::seconds(2);
        let err = proj.apply_event_cas(
            "%1",
            BindingEvent::DeterministicHandshake {
                session_key: "sess-001".into(),
                at: at2,
            },
            now,
            999,
        );
        assert!(err.is_err());

        // Verify state is unchanged after failed CAS
        assert_eq!(
            proj.state_version(),
            version_before,
            "state_version must not change after failed CAS"
        );
        let state_after = proj
            .get_binding("%1")
            .expect("exists")
            .binding
            .binding_state;
        assert_eq!(
            state_before, state_after,
            "binding state must not change after failed CAS"
        );
    }
}

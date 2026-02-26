//! Daemon V5 projection: event-driven read model for pane/session state.
//!
//! Processes gateway event batches through the tier resolver,
//! projects per-session and per-pane runtime state, and provides
//! the client query API (`list_panes`, `list_sessions`, change notifications).
//!
//! Push semantics (`state_changed`, `summary_changed`) are modeled via
//! version-based change tracking: callers poll `changes_since(version)`.
//!
//! Task ref: T-050

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};

use agtmux_core_v5::resolver::{self, ResolverState, SourceRank};
use agtmux_core_v5::signature::{self, SignatureInputs};
use agtmux_core_v5::types::{
    ActivityState, EvidenceMode, EvidenceTier, PaneInstanceId, PanePresence, PaneRuntimeState,
    PaneSignatureClass, SessionRuntimeState, SignatureInputsCompact, SourceEventV2,
};

/// Monotonic version counter for change tracking.
pub type StateVersion = u64;

/// Change notification for a pane or session state update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateChange {
    pub version: StateVersion,
    pub session_key: String,
    pub pane_id: Option<String>,
    pub timestamp: DateTime<Utc>,
}

/// Result of applying a batch of events.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApplyResult {
    pub sessions_changed: usize,
    pub panes_changed: usize,
    pub events_accepted: usize,
    pub events_suppressed: usize,
    pub duplicates_dropped: usize,
}

/// In-memory daemon projection (read model).
///
/// Single-threaded, deterministic. No IO or async.
/// Receives event batches, runs the tier resolver per-session,
/// and maintains projected pane/session runtime state.
#[derive(Debug)]
pub struct DaemonProjection {
    /// Per-session resolver state (carried across resolve calls).
    resolver_states: HashMap<String, ResolverState>,
    /// Per-session runtime state.
    sessions: HashMap<String, SessionRuntimeState>,
    /// Per-pane runtime state, keyed by `pane_id`.
    panes: HashMap<String, PaneRuntimeState>,
    /// Monotonic version counter for change tracking.
    version: StateVersion,
    /// Change log for client polling.
    changes: Vec<StateChange>,
    /// Source rank policy.
    source_ranks: Vec<SourceRank>,
}

impl Default for DaemonProjection {
    fn default() -> Self {
        Self::new()
    }
}

impl DaemonProjection {
    /// Create a new empty projection with default source rank policy.
    pub fn new() -> Self {
        Self {
            resolver_states: HashMap::new(),
            sessions: HashMap::new(),
            panes: HashMap::new(),
            version: 0,
            changes: Vec::new(),
            source_ranks: resolver::default_source_ranks(),
        }
    }

    /// Apply a batch of events from the gateway.
    ///
    /// Events are grouped by `session_key`, resolved per-session through
    /// the tier resolver, and projected into the read model. Returns
    /// statistics about what was accepted/changed.
    pub fn apply_events(&mut self, events: Vec<SourceEventV2>, now: DateTime<Utc>) -> ApplyResult {
        if events.is_empty() {
            return ApplyResult::default();
        }

        // Group events by session_key
        let mut by_session: HashMap<String, Vec<SourceEventV2>> = HashMap::new();
        for event in events {
            by_session
                .entry(event.session_key.clone())
                .or_default()
                .push(event);
        }

        let mut result = ApplyResult::default();

        // Process sorted for determinism in tests
        let mut session_keys: Vec<_> = by_session.keys().cloned().collect();
        session_keys.sort();

        for session_key in session_keys {
            let session_events = by_session.remove(&session_key).unwrap_or_default();
            let prev_state = self.resolver_states.get(&session_key);

            let output = resolver::resolve(session_events, now, prev_state, &self.source_ranks);

            // Always update resolver state (tracks deterministic_last_seen)
            self.resolver_states
                .insert(session_key.clone(), output.next_state.clone());

            result.events_accepted += output.accepted_events.len();
            result.events_suppressed += output.suppressed_events.len();
            result.duplicates_dropped += output.duplicates_dropped;

            // Only project when there are accepted events
            if output.accepted_events.is_empty() {
                continue;
            }

            // Update session runtime state
            if self.project_session(&session_key, &output, now) {
                result.sessions_changed += 1;
            }

            // Update pane states from accepted events (dedup same pane_id)
            let mut panes_counted: HashSet<&str> = HashSet::new();
            for event in &output.accepted_events {
                if let Some(pane_id) = &event.pane_id
                    && self.project_pane(pane_id, event, &output, now)
                    && panes_counted.insert(pane_id.as_str())
                {
                    result.panes_changed += 1;
                }
            }
        }

        result
    }

    /// Project session state from resolver output.
    /// Returns true if the state changed.
    fn project_session(
        &mut self,
        session_key: &str,
        output: &resolver::ResolverOutput,
        now: DateTime<Utc>,
    ) -> bool {
        // Determine activity state from the latest accepted event.
        // Tie-break on event_id for determinism when timestamps are equal.
        let latest_event = output.accepted_events.iter().max_by(|a, b| {
            a.observed_at
                .cmp(&b.observed_at)
                .then_with(|| a.event_id.cmp(&b.event_id))
        });

        let (activity_state, activity_source) = match latest_event {
            Some(event) => (parse_activity_state(&event.event_type), event.source_kind),
            None => return false,
        };

        let evidence_mode = tier_to_evidence_mode(output.result.winner_tier);

        let new_state = SessionRuntimeState {
            session_key: session_key.to_owned(),
            presence: PanePresence::Managed,
            evidence_mode,
            deterministic_last_seen: output.next_state.deterministic_last_seen,
            winner_tier: output.result.winner_tier,
            activity_state,
            activity_source,
            representative_pane_instance_id: None, // T-042
            updated_at: now,
        };

        let changed = self.sessions.get(session_key).is_none_or(|existing| {
            existing.activity_state != new_state.activity_state
                || existing.evidence_mode != new_state.evidence_mode
                || existing.winner_tier != new_state.winner_tier
                || existing.activity_source != new_state.activity_source
        });

        if changed {
            self.version += 1;
            self.changes.push(StateChange {
                version: self.version,
                session_key: session_key.to_owned(),
                pane_id: None,
                timestamp: now,
            });
        }

        self.sessions.insert(session_key.to_owned(), new_state);
        changed
    }

    /// Project pane state from an accepted event.
    /// Returns true if the state changed.
    fn project_pane(
        &mut self,
        pane_id: &str,
        event: &SourceEventV2,
        output: &resolver::ResolverOutput,
        now: DateTime<Utc>,
    ) -> bool {
        // Reuse existing birth_ts for stability when events lack pane_birth_ts
        let birth_ts = event.pane_birth_ts.unwrap_or_else(|| {
            self.panes
                .get(pane_id)
                .map(|p| p.pane_instance_id.birth_ts)
                .unwrap_or(now)
        });

        let pane_instance_id = PaneInstanceId {
            pane_id: pane_id.to_owned(),
            generation: event.pane_generation.unwrap_or(0),
            birth_ts,
        };

        let sig_inputs_compact = extract_signature_inputs(&event.payload);
        let evidence_mode = tier_to_evidence_mode(output.result.winner_tier);

        // Carry forward no_agent_streak from existing pane state (or 0 if new).
        let prev_no_agent_streak = self
            .panes
            .get(pane_id)
            .map(|p| p.no_agent_streak)
            .unwrap_or(0);

        // Check whether the previous pane was deterministic (for deterministic_expected).
        let deterministic_expected = self
            .panes
            .get(pane_id)
            .is_some_and(|p| p.signature_class == PaneSignatureClass::Deterministic);

        // Check if deterministic evidence is fresh for this session.
        let deterministic_fresh_active = {
            let resolver_state = self.resolver_states.get(&event.session_key);
            let det_last_seen = resolver_state.and_then(|s| s.deterministic_last_seen);
            matches!(
                resolver::classify_freshness(det_last_seen, now),
                resolver::Freshness::Fresh
            )
        };

        let has_any_signal = sig_inputs_compact.provider_hint
            || sig_inputs_compact.cmd_match
            || sig_inputs_compact.poller_match
            || sig_inputs_compact.title_match;

        let is_wrapper_cmd = event
            .payload
            .get("is_wrapper_cmd")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Compute no_agent_streak: increment if heuristic with no signals, else reset.
        let no_agent_streak = if event.tier == EvidenceTier::Heuristic && !has_any_signal {
            prev_no_agent_streak + 1
        } else {
            0
        };

        // Build full SignatureInputs for the classifier.
        let classifier_inputs = SignatureInputs {
            provider_hint: sig_inputs_compact.provider_hint,
            cmd_match: sig_inputs_compact.cmd_match,
            poller_match: sig_inputs_compact.poller_match,
            title_match: sig_inputs_compact.title_match,
            has_deterministic_fields: event.tier == EvidenceTier::Deterministic,
            is_wrapper_cmd,
            no_agent_streak,
            deterministic_expected,
            deterministic_fresh_active,
        };

        // Run the signature classifier.
        let (sig_class, sig_reason, sig_confidence) = match signature::classify(&classifier_inputs)
        {
            Ok(result) => (result.class, result.reason, result.confidence),
            Err(agtmux_core_v5::types::AgtmuxError::SignatureInconclusive) => {
                (PaneSignatureClass::None, "inconclusive".to_owned(), 0.0)
            }
            Err(agtmux_core_v5::types::AgtmuxError::SignatureGuardRejected(msg)) => {
                (PaneSignatureClass::None, msg, 0.0)
            }
            Err(_) => (PaneSignatureClass::None, "unknown_error".to_owned(), 0.0),
        };

        let pane_activity_state = parse_activity_state(&event.event_type);
        let pane_provider = Some(event.provider);

        let new_state = PaneRuntimeState {
            pane_instance_id,
            presence: PanePresence::Managed,
            evidence_mode,
            signature_class: sig_class,
            signature_reason: sig_reason,
            signature_confidence: sig_confidence,
            no_agent_streak,
            signature_inputs: sig_inputs_compact,
            activity_state: pane_activity_state,
            provider: pane_provider,
            updated_at: now,
        };

        let changed = self.panes.get(pane_id).is_none_or(|existing| {
            existing.signature_class != new_state.signature_class
                || existing.evidence_mode != new_state.evidence_mode
                || (existing.signature_confidence - new_state.signature_confidence).abs()
                    > f64::EPSILON
                || existing.activity_state != new_state.activity_state
                || existing.provider != new_state.provider
        });

        if changed {
            self.version += 1;
            self.changes.push(StateChange {
                version: self.version,
                session_key: event.session_key.clone(),
                pane_id: Some(pane_id.to_owned()),
                timestamp: now,
            });
        }

        self.panes.insert(pane_id.to_owned(), new_state);
        changed
    }

    // ── Client API ─────────────────────────────────────────────────

    /// List all pane runtime states, sorted by `pane_id`.
    pub fn list_panes(&self) -> Vec<&PaneRuntimeState> {
        let mut panes: Vec<_> = self.panes.values().collect();
        panes.sort_by(|a, b| a.pane_instance_id.pane_id.cmp(&b.pane_instance_id.pane_id));
        panes
    }

    /// List all session runtime states, sorted by `session_key`.
    pub fn list_sessions(&self) -> Vec<&SessionRuntimeState> {
        let mut sessions: Vec<_> = self.sessions.values().collect();
        sessions.sort_by(|a, b| a.session_key.cmp(&b.session_key));
        sessions
    }

    /// Get changes since a given version (for `state_changed` / `summary_changed`).
    ///
    /// Returns notification references only. Clients should use `get_pane()`
    /// or `get_session()` to retrieve the full runtime state for each change.
    pub fn changes_since(&self, since_version: StateVersion) -> Vec<&StateChange> {
        let start = self.changes.partition_point(|c| c.version <= since_version);
        self.changes[start..].iter().collect()
    }

    /// Remove change entries with version <= `before_version`.
    ///
    /// Call periodically once all clients have acknowledged past the given
    /// version, to prevent unbounded growth of the change log.
    pub fn trim_changes_before(&mut self, before_version: StateVersion) {
        self.changes.retain(|c| c.version > before_version);
    }

    /// Current projection version (for change tracking).
    pub fn version(&self) -> StateVersion {
        self.version
    }

    /// Get a specific session state.
    pub fn get_session(&self, session_key: &str) -> Option<&SessionRuntimeState> {
        self.sessions.get(session_key)
    }

    /// Get a specific pane state.
    pub fn get_pane(&self, pane_id: &str) -> Option<&PaneRuntimeState> {
        self.panes.get(pane_id)
    }

    /// Number of tracked sessions.
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Number of tracked panes.
    pub fn pane_count(&self) -> usize {
        self.panes.len()
    }
}

// ─── Helpers ─────────────────────────────────────────────────────

/// Parse an `ActivityState` from an `event_type` string.
fn parse_activity_state(event_type: &str) -> ActivityState {
    match event_type {
        "activity.running" | "lifecycle.running" | "activity.start" | "lifecycle.start" => {
            ActivityState::Running
        }
        "activity.idle" | "lifecycle.idle" | "activity.end" | "activity.stop" | "lifecycle.end"
        | "lifecycle.stop" => ActivityState::Idle,
        "activity.waiting_input" | "lifecycle.waiting_input" => ActivityState::WaitingInput,
        "activity.waiting_approval" | "lifecycle.waiting_approval" => {
            ActivityState::WaitingApproval
        }
        "activity.error" | "lifecycle.error" => ActivityState::Error,
        _ => ActivityState::Unknown,
    }
}

/// Map `EvidenceTier` to `EvidenceMode`.
fn tier_to_evidence_mode(tier: EvidenceTier) -> EvidenceMode {
    match tier {
        EvidenceTier::Deterministic => EvidenceMode::Deterministic,
        EvidenceTier::Heuristic => EvidenceMode::Heuristic,
    }
}

/// Extract compact signature inputs from event payload JSON.
///
/// `poller_match` has a fallback: if the explicit bool field is absent,
/// the presence of a `matched_pattern` string (set by poller events) is used.
fn extract_signature_inputs(payload: &serde_json::Value) -> SignatureInputsCompact {
    let explicit_poller = payload
        .get("poller_match")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let capture_match = payload
        .get("capture_match")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let inferred_poller = payload
        .get("matched_pattern")
        .and_then(|v| v.as_str())
        .is_some();

    SignatureInputsCompact {
        provider_hint: payload
            .get("provider_hint")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        cmd_match: payload
            .get("cmd_match")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        poller_match: explicit_poller || capture_match || inferred_poller,
        title_match: payload
            .get("title_match")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    }
}

// ─── Tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use agtmux_core_v5::types::SourceKind;
    use chrono::TimeDelta;

    fn t0() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-02-25T12:00:00Z")
            .expect("valid")
            .with_timezone(&Utc)
    }

    fn make_event(
        event_id: &str,
        provider: agtmux_core_v5::types::Provider,
        source_kind: SourceKind,
        session_key: &str,
        pane_id: Option<&str>,
        event_type: &str,
        observed_at: DateTime<Utc>,
    ) -> SourceEventV2 {
        SourceEventV2 {
            event_id: event_id.to_owned(),
            provider,
            source_kind,
            tier: source_kind.tier(),
            observed_at,
            session_key: session_key.to_owned(),
            pane_id: pane_id.map(str::to_owned),
            pane_generation: None,
            pane_birth_ts: None,
            source_event_id: None,
            event_type: event_type.to_owned(),
            payload: serde_json::json!({}),
            confidence: 0.86,
        }
    }

    fn det_event(
        id: &str,
        session: &str,
        pane: &str,
        event_type: &str,
        at: DateTime<Utc>,
    ) -> SourceEventV2 {
        make_event(
            id,
            agtmux_core_v5::types::Provider::Codex,
            SourceKind::CodexAppserver,
            session,
            Some(pane),
            event_type,
            at,
        )
    }

    fn heur_event(
        id: &str,
        session: &str,
        pane: &str,
        event_type: &str,
        at: DateTime<Utc>,
    ) -> SourceEventV2 {
        let mut e = make_event(
            id,
            agtmux_core_v5::types::Provider::Codex,
            SourceKind::Poller,
            session,
            Some(pane),
            event_type,
            at,
        );
        e.payload = serde_json::json!({
            "provider_hint": true,
            "cmd_match": true,
        });
        e
    }

    // ── 1. Empty projection ─────────────────────────────────────────

    #[test]
    fn empty_projection() {
        let proj = DaemonProjection::new();
        assert!(proj.list_panes().is_empty());
        assert!(proj.list_sessions().is_empty());
        assert_eq!(proj.version(), 0);
        assert_eq!(proj.session_count(), 0);
        assert_eq!(proj.pane_count(), 0);
    }

    // ── 2. Single deterministic event creates session + pane ────────

    #[test]
    fn single_deterministic_event() {
        let mut proj = DaemonProjection::new();
        let now = t0();
        let event = det_event("e1", "sess-1", "%1", "activity.running", now);

        let result = proj.apply_events(vec![event], now);

        assert_eq!(result.events_accepted, 1);
        assert_eq!(result.sessions_changed, 1);
        assert_eq!(result.panes_changed, 1);

        let session = proj.get_session("sess-1").expect("session exists");
        assert_eq!(session.activity_state, ActivityState::Running);
        assert_eq!(session.evidence_mode, EvidenceMode::Deterministic);
        assert_eq!(session.winner_tier, EvidenceTier::Deterministic);
        assert_eq!(session.presence, PanePresence::Managed);
        assert_eq!(session.deterministic_last_seen, Some(now));

        let pane = proj.get_pane("%1").expect("pane exists");
        assert_eq!(pane.signature_class, PaneSignatureClass::Deterministic);
        assert_eq!(pane.signature_confidence, 1.0);
        assert_eq!(pane.presence, PanePresence::Managed);
    }

    // ── 3. Single heuristic event ──────────────────────────────────

    #[test]
    fn single_heuristic_event() {
        let mut proj = DaemonProjection::new();
        let now = t0();
        let event = heur_event("e1", "sess-1", "%1", "activity.running", now);

        let result = proj.apply_events(vec![event], now);

        assert_eq!(result.events_accepted, 1);
        let session = proj.get_session("sess-1").expect("session");
        assert_eq!(session.evidence_mode, EvidenceMode::Heuristic);
        assert_eq!(session.winner_tier, EvidenceTier::Heuristic);

        let pane = proj.get_pane("%1").expect("pane");
        assert_eq!(pane.signature_class, PaneSignatureClass::Heuristic);
        // Classifier uses max weight: provider_hint (1.0) > cmd_match (0.86)
        assert!(
            (pane.signature_confidence - 1.0).abs() < f64::EPSILON,
            "expected confidence 1.0 (WEIGHT_PROCESS_HINT), got {}",
            pane.signature_confidence,
        );
        assert!(pane.signature_inputs.provider_hint);
        assert!(pane.signature_inputs.cmd_match);
    }

    // ── 4. Activity state parsing ──────────────────────────────────

    #[test]
    fn activity_state_parsing() {
        assert_eq!(
            parse_activity_state("activity.running"),
            ActivityState::Running
        );
        assert_eq!(
            parse_activity_state("lifecycle.running"),
            ActivityState::Running
        );
        assert_eq!(parse_activity_state("activity.idle"), ActivityState::Idle);
        assert_eq!(parse_activity_state("lifecycle.idle"), ActivityState::Idle);
        assert_eq!(
            parse_activity_state("activity.waiting_input"),
            ActivityState::WaitingInput
        );
        assert_eq!(
            parse_activity_state("activity.waiting_approval"),
            ActivityState::WaitingApproval
        );
        assert_eq!(parse_activity_state("activity.error"), ActivityState::Error);
        assert_eq!(
            parse_activity_state("lifecycle.start"),
            ActivityState::Running
        );
        assert_eq!(
            parse_activity_state("activity.start"),
            ActivityState::Running
        );
        assert_eq!(parse_activity_state("lifecycle.end"), ActivityState::Idle);
        assert_eq!(parse_activity_state("lifecycle.stop"), ActivityState::Idle);
        assert_eq!(parse_activity_state("activity.end"), ActivityState::Idle);
        assert_eq!(parse_activity_state("activity.stop"), ActivityState::Idle);
        assert_eq!(
            parse_activity_state("lifecycle.waiting_input"),
            ActivityState::WaitingInput
        );
        assert_eq!(
            parse_activity_state("lifecycle.waiting_approval"),
            ActivityState::WaitingApproval
        );
        assert_eq!(
            parse_activity_state("lifecycle.error"),
            ActivityState::Error
        );
        assert_eq!(parse_activity_state("unknown.type"), ActivityState::Unknown);
    }

    // ── 5. Empty batch returns default result ──────────────────────

    #[test]
    fn empty_batch() {
        let mut proj = DaemonProjection::new();
        let result = proj.apply_events(vec![], t0());
        assert_eq!(result, ApplyResult::default());
    }

    // ── 6. Change tracking: version increments ─────────────────────

    #[test]
    fn change_tracking_version() {
        let mut proj = DaemonProjection::new();
        let now = t0();
        assert_eq!(proj.version(), 0);

        proj.apply_events(
            vec![det_event("e1", "s1", "%1", "activity.running", now)],
            now,
        );

        // session + pane = 2 version increments
        assert_eq!(proj.version(), 2);

        let changes = proj.changes_since(0);
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].session_key, "s1");
        assert!(changes[0].pane_id.is_none()); // session change
        assert_eq!(changes[1].session_key, "s1");
        assert_eq!(changes[1].pane_id, Some("%1".to_owned())); // pane change
    }

    // ── 7. No change on same state re-application ──────────────────

    #[test]
    fn no_change_on_same_state() {
        let mut proj = DaemonProjection::new();
        let t = t0();

        // First application
        proj.apply_events(vec![det_event("e1", "s1", "%1", "activity.running", t)], t);
        let v1 = proj.version();

        // Second application with same state (different event_id to avoid dedup)
        let t2 = t + TimeDelta::seconds(1);
        let result = proj.apply_events(
            vec![det_event("e2", "s1", "%1", "activity.running", t2)],
            t2,
        );

        // Events accepted but state didn't change
        assert_eq!(result.events_accepted, 1);
        assert_eq!(result.sessions_changed, 0);
        assert_eq!(result.panes_changed, 0);
        assert_eq!(proj.version(), v1);
    }

    // ── 8. State change detection ──────────────────────────────────

    #[test]
    fn state_change_detected() {
        let mut proj = DaemonProjection::new();
        let t = t0();

        proj.apply_events(vec![det_event("e1", "s1", "%1", "activity.running", t)], t);
        let v1 = proj.version();

        // Change activity state
        let t2 = t + TimeDelta::seconds(1);
        let result = proj.apply_events(vec![det_event("e2", "s1", "%1", "activity.idle", t2)], t2);

        assert_eq!(result.sessions_changed, 1);
        let session = proj.get_session("s1").expect("session");
        assert_eq!(session.activity_state, ActivityState::Idle);

        let new_changes = proj.changes_since(v1);
        assert!(!new_changes.is_empty());
    }

    // ── 9. Multiple sessions are isolated ──────────────────────────

    #[test]
    fn multiple_sessions_isolated() {
        let mut proj = DaemonProjection::new();
        let now = t0();

        let events = vec![
            det_event("e1", "sess-a", "%1", "activity.running", now),
            det_event("e2", "sess-b", "%2", "activity.idle", now),
        ];
        let result = proj.apply_events(events, now);

        assert_eq!(result.sessions_changed, 2);
        assert_eq!(result.panes_changed, 2);
        assert_eq!(proj.session_count(), 2);
        assert_eq!(proj.pane_count(), 2);

        let sa = proj.get_session("sess-a").expect("a");
        assert_eq!(sa.activity_state, ActivityState::Running);

        let sb = proj.get_session("sess-b").expect("b");
        assert_eq!(sb.activity_state, ActivityState::Idle);
    }

    // ── 10. list_panes sorted by pane_id ───────────────────────────

    #[test]
    fn list_panes_sorted() {
        let mut proj = DaemonProjection::new();
        let now = t0();

        proj.apply_events(
            vec![
                det_event("e1", "s1", "%3", "activity.running", now),
                det_event("e2", "s1", "%1", "activity.idle", now),
                det_event("e3", "s2", "%2", "activity.running", now),
            ],
            now,
        );

        let panes = proj.list_panes();
        assert_eq!(panes.len(), 3);
        assert_eq!(panes[0].pane_instance_id.pane_id, "%1");
        assert_eq!(panes[1].pane_instance_id.pane_id, "%2");
        assert_eq!(panes[2].pane_instance_id.pane_id, "%3");
    }

    // ── 11. list_sessions sorted by session_key ────────────────────

    #[test]
    fn list_sessions_sorted() {
        let mut proj = DaemonProjection::new();
        let now = t0();

        proj.apply_events(
            vec![
                det_event("e1", "sess-c", "%1", "activity.running", now),
                det_event("e2", "sess-a", "%2", "activity.idle", now),
                det_event("e3", "sess-b", "%3", "activity.running", now),
            ],
            now,
        );

        let sessions = proj.list_sessions();
        assert_eq!(sessions.len(), 3);
        assert_eq!(sessions[0].session_key, "sess-a");
        assert_eq!(sessions[1].session_key, "sess-b");
        assert_eq!(sessions[2].session_key, "sess-c");
    }

    // ── 12. Duplicate events are dropped ───────────────────────────

    #[test]
    fn duplicate_events_dropped() {
        let mut proj = DaemonProjection::new();
        let now = t0();

        let event = det_event("e1", "s1", "%1", "activity.running", now);
        let result = proj.apply_events(vec![event.clone(), event], now);

        assert_eq!(result.duplicates_dropped, 1);
        assert_eq!(result.events_accepted, 1);
    }

    // ── 13. Evidence mode tracks tier transitions ──────────────────

    #[test]
    fn evidence_mode_tracks_tier() {
        let mut proj = DaemonProjection::new();
        let t = t0();

        // Start with deterministic
        proj.apply_events(vec![det_event("e1", "s1", "%1", "activity.running", t)], t);
        let session = proj.get_session("s1").expect("session");
        assert_eq!(session.evidence_mode, EvidenceMode::Deterministic);

        // Deterministic goes stale (> 3s), heuristic takes over
        let t2 = t + TimeDelta::seconds(5);
        proj.apply_events(
            vec![heur_event("e2", "s1", "%1", "activity.running", t2)],
            t2,
        );
        let session = proj.get_session("s1").expect("session");
        assert_eq!(session.evidence_mode, EvidenceMode::Heuristic);
    }

    // ── 14. changes_since filters by version ───────────────────────

    #[test]
    fn changes_since_filters() {
        let mut proj = DaemonProjection::new();
        let t = t0();

        proj.apply_events(vec![det_event("e1", "s1", "%1", "activity.running", t)], t);
        let v1 = proj.version();

        let t2 = t + TimeDelta::seconds(1);
        proj.apply_events(vec![det_event("e2", "s2", "%2", "activity.idle", t2)], t2);

        let all_changes = proj.changes_since(0);
        let new_changes = proj.changes_since(v1);

        assert!(new_changes.len() < all_changes.len());
        assert!(new_changes.iter().all(|c| c.version > v1));
    }

    // ── 15. Event without pane_id still updates session ────────────

    #[test]
    fn event_without_pane_id() {
        let mut proj = DaemonProjection::new();
        let now = t0();

        let mut event = det_event("e1", "s1", "%1", "activity.running", now);
        event.pane_id = None;

        let result = proj.apply_events(vec![event], now);

        assert_eq!(result.sessions_changed, 1);
        assert_eq!(result.panes_changed, 0);
        assert!(proj.get_session("s1").is_some());
        assert_eq!(proj.pane_count(), 0);
    }

    // ── 16. Signature inputs extracted from payload ─────────────────

    #[test]
    fn signature_inputs_from_payload() {
        let mut proj = DaemonProjection::new();
        let now = t0();

        let event = heur_event("e1", "s1", "%1", "activity.running", now);
        proj.apply_events(vec![event], now);

        let pane = proj.get_pane("%1").expect("pane");
        assert!(pane.signature_inputs.provider_hint);
        assert!(pane.signature_inputs.cmd_match);
        assert!(!pane.signature_inputs.poller_match);
        assert!(!pane.signature_inputs.title_match);
    }

    // ── 17. Default projection is Default ──────────────────────────

    #[test]
    fn default_trait() {
        let proj = DaemonProjection::default();
        assert_eq!(proj.version(), 0);
    }

    // ── 18. Latest event determines activity state ──────────────────

    #[test]
    fn latest_event_wins_activity() {
        let mut proj = DaemonProjection::new();
        let t = t0();

        let events = vec![
            det_event("e1", "s1", "%1", "activity.idle", t),
            det_event(
                "e2",
                "s1",
                "%1",
                "activity.running",
                t + TimeDelta::seconds(1),
            ),
        ];
        proj.apply_events(events, t + TimeDelta::seconds(1));

        let session = proj.get_session("s1").expect("session");
        assert_eq!(session.activity_state, ActivityState::Running);
    }

    // ── 19. Pane updated_at reflects application time ──────────────

    #[test]
    fn updated_at_set_to_now() {
        let mut proj = DaemonProjection::new();
        // Event observed 1s ago, now is application time
        let event_time = t0();
        let now = t0() + TimeDelta::seconds(1);

        proj.apply_events(
            vec![det_event("e1", "s1", "%1", "activity.running", event_time)],
            now,
        );

        let session = proj.get_session("s1").expect("session");
        assert_eq!(session.updated_at, now);

        let pane = proj.get_pane("%1").expect("pane");
        assert_eq!(pane.updated_at, now);
    }

    // ── 20. Source rank suppression ─────────────────────────────────

    #[test]
    fn source_rank_suppression() {
        let mut proj = DaemonProjection::new();
        let now = t0();

        // Both appserver and poller events for Codex
        // Appserver should win (rank 0 vs rank 1)
        let events = vec![
            det_event("e1", "s1", "%1", "activity.running", now),
            heur_event("e2", "s1", "%1", "activity.idle", now),
        ];
        let result = proj.apply_events(events, now);

        assert_eq!(result.events_suppressed, 1); // poller suppressed
        let session = proj.get_session("s1").expect("session");
        assert_eq!(session.activity_state, ActivityState::Running);
        assert_eq!(session.activity_source, SourceKind::CodexAppserver);
    }

    // ── 21. Extract signature inputs edge cases ────────────────────

    #[test]
    fn extract_signature_inputs_edge_cases() {
        // Empty payload
        let empty = extract_signature_inputs(&serde_json::json!({}));
        assert!(!empty.provider_hint);
        assert!(!empty.cmd_match);

        // Full payload
        let full = extract_signature_inputs(&serde_json::json!({
            "provider_hint": true,
            "cmd_match": true,
            "poller_match": true,
            "title_match": true,
        }));
        assert!(full.provider_hint);
        assert!(full.cmd_match);
        assert!(full.poller_match);
        assert!(full.title_match);

        // Non-bool values
        let mixed = extract_signature_inputs(&serde_json::json!({
            "provider_hint": "yes",
            "cmd_match": 1,
        }));
        assert!(!mixed.provider_hint); // "yes" is not bool
        assert!(!mixed.cmd_match); // 1 is not bool
    }

    // ── 22. Claude hooks event (different provider) ────────────────

    #[test]
    fn claude_hooks_event() {
        let mut proj = DaemonProjection::new();
        let now = t0();

        let event = make_event(
            "claude-hooks-1",
            agtmux_core_v5::types::Provider::Claude,
            SourceKind::ClaudeHooks,
            "claude-sess-1",
            Some("%5"),
            "lifecycle.start",
            now,
        );
        proj.apply_events(vec![event], now);

        let session = proj.get_session("claude-sess-1").expect("session");
        assert_eq!(session.activity_state, ActivityState::Running); // lifecycle.start → Running
        assert_eq!(session.activity_source, SourceKind::ClaudeHooks);
        assert_eq!(session.evidence_mode, EvidenceMode::Deterministic);
    }

    // ── 23. Tier_to_evidence_mode mapping ──────────────────────────

    #[test]
    fn tier_to_evidence_mode_mapping() {
        assert_eq!(
            tier_to_evidence_mode(EvidenceTier::Deterministic),
            EvidenceMode::Deterministic
        );
        assert_eq!(
            tier_to_evidence_mode(EvidenceTier::Heuristic),
            EvidenceMode::Heuristic
        );
    }

    // ── 24. Multi-batch accumulation ───────────────────────────────

    #[test]
    fn multi_batch_accumulation() {
        let mut proj = DaemonProjection::new();
        let t = t0();

        // Batch 1: two sessions
        proj.apply_events(
            vec![
                det_event("e1", "s1", "%1", "activity.running", t),
                det_event("e2", "s2", "%2", "activity.idle", t),
            ],
            t,
        );
        assert_eq!(proj.session_count(), 2);
        assert_eq!(proj.pane_count(), 2);

        // Batch 2: new pane for existing session
        let t2 = t + TimeDelta::seconds(1);
        proj.apply_events(
            vec![det_event("e3", "s1", "%3", "activity.running", t2)],
            t2,
        );
        assert_eq!(proj.session_count(), 2);
        assert_eq!(proj.pane_count(), 3);
    }

    // ── 25. Re-promotion from heuristic back to deterministic ──────

    #[test]
    fn re_promotion() {
        let mut proj = DaemonProjection::new();
        let t = t0();

        // Start deterministic
        proj.apply_events(vec![det_event("e1", "s1", "%1", "activity.running", t)], t);

        // Go stale, heuristic takes over
        let t2 = t + TimeDelta::seconds(5);
        proj.apply_events(
            vec![heur_event("e2", "s1", "%1", "activity.running", t2)],
            t2,
        );
        assert_eq!(
            proj.get_session("s1").expect("s").evidence_mode,
            EvidenceMode::Heuristic
        );

        // Fresh deterministic arrives → re-promotion
        let t3 = t2 + TimeDelta::seconds(1);
        proj.apply_events(
            vec![det_event("e3", "s1", "%1", "activity.running", t3)],
            t3,
        );
        let session = proj.get_session("s1").expect("s");
        assert_eq!(session.evidence_mode, EvidenceMode::Deterministic);
        assert_eq!(session.winner_tier, EvidenceTier::Deterministic);
    }

    // ── 26. Signature classifier integration: deterministic ──────

    #[test]
    fn signature_classifier_integration() {
        let mut proj = DaemonProjection::new();
        let now = t0();
        let event = det_event("e1", "s1", "%1", "activity.running", now);

        proj.apply_events(vec![event], now);

        let pane = proj.get_pane("%1").expect("pane");
        assert_eq!(pane.signature_class, PaneSignatureClass::Deterministic);
        assert!(
            (pane.signature_confidence - 1.0).abs() < f64::EPSILON,
            "deterministic confidence must be 1.0, got {}",
            pane.signature_confidence,
        );
        assert!(
            pane.signature_reason.contains("deterministic"),
            "reason should contain 'deterministic', got: {}",
            pane.signature_reason,
        );
    }

    // ── 27. Signature heuristic with signals ─────────────────────

    #[test]
    fn signature_heuristic_with_signals() {
        let mut proj = DaemonProjection::new();
        let now = t0();

        let mut event = make_event(
            "e1",
            agtmux_core_v5::types::Provider::Codex,
            SourceKind::Poller,
            "s1",
            Some("%1"),
            "activity.running",
            now,
        );
        event.payload = serde_json::json!({ "provider_hint": true });

        proj.apply_events(vec![event], now);

        let pane = proj.get_pane("%1").expect("pane");
        assert_eq!(pane.signature_class, PaneSignatureClass::Heuristic);
        assert!(
            (pane.signature_confidence - 1.0).abs() < f64::EPSILON,
            "provider_hint weight is WEIGHT_PROCESS_HINT (1.0), got {}",
            pane.signature_confidence,
        );
        assert!(
            pane.signature_reason.contains("provider_hint"),
            "reason should contain 'provider_hint', got: {}",
            pane.signature_reason,
        );
    }

    // ── 28. Signature no signals returns None ────────────────────

    #[test]
    fn signature_no_signals_returns_none() {
        let mut proj = DaemonProjection::new();
        let now = t0();

        // Heuristic event with empty payload (no signals)
        let event = make_event(
            "e1",
            agtmux_core_v5::types::Provider::Codex,
            SourceKind::Poller,
            "s1",
            Some("%1"),
            "activity.running",
            now,
        );

        proj.apply_events(vec![event], now);

        let pane = proj.get_pane("%1").expect("pane");
        assert_eq!(pane.signature_class, PaneSignatureClass::None);
        assert!(
            (pane.signature_confidence - 0.0).abs() < f64::EPSILON,
            "no-signal confidence must be 0.0, got {}",
            pane.signature_confidence,
        );
    }

    // ── 29. No-agent streak demotion ─────────────────────────────

    #[test]
    fn signature_no_agent_streak_demotion() {
        let mut proj = DaemonProjection::new();
        let t = t0();

        // First heuristic event with signals → Heuristic, streak resets to 0
        let mut e1 = make_event(
            "e1",
            agtmux_core_v5::types::Provider::Codex,
            SourceKind::Poller,
            "s1",
            Some("%1"),
            "activity.running",
            t,
        );
        e1.payload = serde_json::json!({ "provider_hint": true });
        proj.apply_events(vec![e1], t);
        let pane = proj.get_pane("%1").expect("pane");
        assert_eq!(pane.no_agent_streak, 0);
        assert_eq!(pane.signature_class, PaneSignatureClass::Heuristic);

        // Second heuristic event with NO signals → streak = 1
        let t2 = t + TimeDelta::seconds(1);
        let e2 = make_event(
            "e2",
            agtmux_core_v5::types::Provider::Codex,
            SourceKind::Poller,
            "s1",
            Some("%1"),
            "activity.running",
            t2,
        );
        proj.apply_events(vec![e2], t2);
        let pane = proj.get_pane("%1").expect("pane");
        assert_eq!(pane.no_agent_streak, 1);
        assert_eq!(pane.signature_class, PaneSignatureClass::None);

        // Third heuristic event with NO signals → streak = 2 (≥ threshold)
        let t3 = t + TimeDelta::seconds(2);
        let e3 = make_event(
            "e3",
            agtmux_core_v5::types::Provider::Codex,
            SourceKind::Poller,
            "s1",
            Some("%1"),
            "activity.running",
            t3,
        );
        proj.apply_events(vec![e3], t3);
        let pane = proj.get_pane("%1").expect("pane");
        assert_eq!(pane.no_agent_streak, 2);
        assert_eq!(
            pane.signature_class,
            PaneSignatureClass::None,
            "streak >= threshold should demote to None"
        );
    }

    // ── 30. Guardrail: wrapper_cmd + title_only → rejected ───────

    #[test]
    fn signature_guardrail_wrapper_cmd_title_only() {
        let mut proj = DaemonProjection::new();
        let now = t0();

        let mut event = make_event(
            "e1",
            agtmux_core_v5::types::Provider::Codex,
            SourceKind::Poller,
            "s1",
            Some("%1"),
            "activity.running",
            now,
        );
        event.payload = serde_json::json!({
            "title_match": true,
            "is_wrapper_cmd": true,
        });

        proj.apply_events(vec![event], now);

        let pane = proj.get_pane("%1").expect("pane");
        assert_eq!(
            pane.signature_class,
            PaneSignatureClass::None,
            "wrapper + title-only should be rejected (guard)"
        );
        assert!(
            pane.signature_reason.contains("wrapper"),
            "reason should mention wrapper, got: {}",
            pane.signature_reason,
        );
    }

    // ── 31. Signature fields present in list_panes ───────────────

    #[test]
    fn signature_fields_in_list_panes() {
        let mut proj = DaemonProjection::new();
        let now = t0();

        let event = det_event("e1", "s1", "%1", "activity.running", now);
        proj.apply_events(vec![event], now);

        let panes = proj.list_panes();
        assert_eq!(panes.len(), 1);

        let pane = panes[0];
        assert_eq!(pane.signature_class, PaneSignatureClass::Deterministic);
        assert!(
            (pane.signature_confidence - 1.0).abs() < f64::EPSILON,
            "confidence should be 1.0"
        );
        assert!(
            !pane.signature_reason.is_empty(),
            "reason should not be empty"
        );
        // no_agent_streak should be present and zero for deterministic
        assert_eq!(pane.no_agent_streak, 0);
        // signature_inputs should be present (all false for det event with empty payload)
        assert!(!pane.signature_inputs.provider_hint);
        assert!(!pane.signature_inputs.cmd_match);
        assert!(!pane.signature_inputs.poller_match);
        assert!(!pane.signature_inputs.title_match);
    }

    // ── 32. Snapshot: deterministic pane ──────────────────────────

    #[test]
    fn signature_snapshot_deterministic() {
        let mut proj = DaemonProjection::new();
        let now = t0();

        let event = det_event("e1", "s1", "%1", "activity.running", now);
        proj.apply_events(vec![event], now);

        let pane = proj.get_pane("%1").expect("pane");
        assert_eq!(pane.pane_instance_id.pane_id, "%1");
        assert_eq!(pane.presence, PanePresence::Managed);
        assert_eq!(pane.evidence_mode, EvidenceMode::Deterministic);
        assert_eq!(pane.signature_class, PaneSignatureClass::Deterministic);
        assert!(
            pane.signature_reason.contains("deterministic"),
            "reason: {}",
            pane.signature_reason
        );
        assert!((pane.signature_confidence - 1.0).abs() < f64::EPSILON);
        assert_eq!(pane.no_agent_streak, 0);
        assert_eq!(pane.updated_at, now);
    }

    // ── 33. Snapshot: heuristic pane ─────────────────────────────

    #[test]
    fn signature_snapshot_heuristic() {
        let mut proj = DaemonProjection::new();
        let now = t0();

        let event = heur_event("e1", "s1", "%1", "activity.running", now);
        proj.apply_events(vec![event], now);

        let pane = proj.get_pane("%1").expect("pane");
        assert_eq!(pane.pane_instance_id.pane_id, "%1");
        assert_eq!(pane.presence, PanePresence::Managed);
        assert_eq!(pane.evidence_mode, EvidenceMode::Heuristic);
        assert_eq!(pane.signature_class, PaneSignatureClass::Heuristic);
        assert!(
            pane.signature_reason.contains("heuristic"),
            "reason: {}",
            pane.signature_reason
        );
        // provider_hint (1.0) is the max weight
        assert!((pane.signature_confidence - 1.0).abs() < f64::EPSILON);
        assert_eq!(pane.no_agent_streak, 0);
        assert!(pane.signature_inputs.provider_hint);
        assert!(pane.signature_inputs.cmd_match);
        assert_eq!(pane.updated_at, now);
    }

    // ── 34. Snapshot: none pane ──────────────────────────────────

    #[test]
    fn signature_snapshot_none() {
        let mut proj = DaemonProjection::new();
        let now = t0();

        // Heuristic event with no signals
        let event = make_event(
            "e1",
            agtmux_core_v5::types::Provider::Codex,
            SourceKind::Poller,
            "s1",
            Some("%1"),
            "activity.running",
            now,
        );
        proj.apply_events(vec![event], now);

        let pane = proj.get_pane("%1").expect("pane");
        assert_eq!(pane.pane_instance_id.pane_id, "%1");
        assert_eq!(pane.presence, PanePresence::Managed);
        assert_eq!(pane.evidence_mode, EvidenceMode::Heuristic);
        assert_eq!(pane.signature_class, PaneSignatureClass::None);
        assert!(
            pane.signature_reason.contains("no heuristic signals"),
            "reason: {}",
            pane.signature_reason
        );
        assert!((pane.signature_confidence - 0.0).abs() < f64::EPSILON);
        assert_eq!(pane.no_agent_streak, 1);
        assert!(!pane.signature_inputs.provider_hint);
        assert!(!pane.signature_inputs.cmd_match);
        assert!(!pane.signature_inputs.poller_match);
        assert!(!pane.signature_inputs.title_match);
        assert_eq!(pane.updated_at, now);
    }

    // ── 35. SignatureInconclusive regression: det→heur empty ─────

    #[test]
    fn signature_inconclusive_after_deterministic() {
        let mut proj = DaemonProjection::new();
        let t = t0();

        // Step 1: deterministic event establishes the pane
        proj.apply_events(vec![det_event("e1", "s1", "%1", "activity.running", t)], t);
        let pane = proj.get_pane("%1").expect("pane");
        assert_eq!(pane.signature_class, PaneSignatureClass::Deterministic);

        // Step 2: deterministic goes stale (>3s), heuristic event with NO signals
        // deterministic_expected=true because pane was previously deterministic
        let t2 = t + TimeDelta::seconds(5);
        let empty_heur = make_event(
            "e2",
            agtmux_core_v5::types::Provider::Codex,
            SourceKind::Poller,
            "s1",
            Some("%1"),
            "activity.running",
            t2,
        );
        proj.apply_events(vec![empty_heur], t2);

        let pane = proj.get_pane("%1").expect("pane");
        // deterministic_expected=true + no signals → SignatureInconclusive → None
        assert_eq!(pane.signature_class, PaneSignatureClass::None);
        assert!(
            pane.signature_reason.contains("inconclusive"),
            "reason should contain 'inconclusive', got: {}",
            pane.signature_reason,
        );
        assert!((pane.signature_confidence - 0.0).abs() < f64::EPSILON);
        assert_eq!(pane.no_agent_streak, 1);
    }

    // ── 36. Poller match inferred from matched_pattern ──────────

    #[test]
    fn poller_match_inferred_from_matched_pattern() {
        let mut proj = DaemonProjection::new();
        let now = t0();

        let mut event = make_event(
            "e1",
            agtmux_core_v5::types::Provider::Codex,
            SourceKind::Poller,
            "s1",
            Some("%1"),
            "activity.running",
            now,
        );
        // Poller events set matched_pattern, not poller_match
        event.payload = serde_json::json!({
            "matched_pattern": "codex_running",
        });
        proj.apply_events(vec![event], now);

        let pane = proj.get_pane("%1").expect("pane");
        assert!(
            pane.signature_inputs.poller_match,
            "poller_match should be inferred from matched_pattern"
        );
        assert_eq!(pane.signature_class, PaneSignatureClass::Heuristic);
    }
}

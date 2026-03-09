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
use agtmux_core_v5::sync_v2_compat;
use agtmux_core_v5::types::{
    EvidenceMode, EvidenceTier, PaneInstanceId, PanePresence, PaneRuntimeState, PaneSignatureClass,
    Provider, SessionRuntimeState, SignatureInputsCompact, SourceEventV2,
};

/// Monotonic version counter for change tracking.
pub type StateVersion = u64;

/// Replay cursor for UI change feed polling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplayCursor {
    pub epoch: u64,
    pub seq: StateVersion,
}

/// Explicit resync response when replay continuity cannot be guaranteed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResyncRequired {
    pub current_epoch: u64,
    pub latest_snapshot_seq: StateVersion,
    pub reason: &'static str,
}

/// Strictly validated replay batch for `ui.changes.v2`.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplayBatch<'a> {
    pub epoch: u64,
    pub from_seq: StateVersion,
    pub to_seq: StateVersion,
    pub next_cursor: ReplayCursor,
    pub changes: Vec<&'a StateChange>,
}

/// Replay observability snapshot for `ui.health.v1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayHealthSnapshot {
    pub current_epoch: u64,
    pub cursor_seq: StateVersion,
    pub head_seq: StateVersion,
    pub lag: StateVersion,
    pub last_resync_reason: Option<&'static str>,
    pub last_resync_at: Option<DateTime<Utc>>,
}

/// Change notification for a pane or session state update.
#[derive(Debug, Clone, PartialEq)]
pub struct StateChange {
    pub version: StateVersion,
    pub session_key: String,
    pub pane_id: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub session_state: Option<SessionRuntimeState>,
    pub pane_state: Option<PaneRuntimeState>,
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
    /// Best-known session -> pane mapping (used when events omit pane_id).
    session_to_pane: HashMap<String, String>,
    /// Monotonic version counter for change tracking.
    version: StateVersion,
    /// Replay epoch for strict UI change-feed continuity.
    epoch: u64,
    /// Legacy change log for `state_changed` / `summary_changed`.
    changes: Vec<StateChange>,
    /// Sync-v2 replay log with its own retention policy.
    replay_log: Vec<StateChange>,
    /// Highest sync-v2 cursor acknowledged by the client.
    replay_acked_cursor: ReplayCursor,
    /// Last explicit resync reason emitted for sync-v2 replay.
    replay_last_resync_reason: Option<&'static str>,
    /// Timestamp of the last explicit resync emitted for sync-v2 replay.
    replay_last_resync_at: Option<DateTime<Utc>>,
    /// Source rank policy.
    source_ranks: Vec<SourceRank>,
    /// Per-pane, per-provider last non-heartbeat deterministic event timestamp.
    /// Used for cross-provider conflict resolution (T-123).
    last_real_activity: HashMap<String, HashMap<Provider, DateTime<Utc>>>,
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
            session_to_pane: HashMap::new(),
            version: 0,
            epoch: 1,
            changes: Vec::new(),
            replay_log: Vec::new(),
            replay_acked_cursor: ReplayCursor { epoch: 1, seq: 0 },
            replay_last_resync_reason: None,
            replay_last_resync_at: None,
            source_ranks: resolver::default_source_ranks(),
            last_real_activity: HashMap::new(),
        }
    }

    /// Apply a batch of events from the gateway.
    ///
    /// Events are grouped by `pane_id` (pane-first grouping), resolved
    /// per-group through the tier resolver, and projected into the read model.
    /// This ensures all source events for the same pane enter the same resolver
    /// batch, so cross-source tier suppression works correctly.
    ///
    /// Fallback when `pane_id` is absent: `session_to_pane` lookup, then `session_key`.
    pub fn apply_events(&mut self, events: Vec<SourceEventV2>, now: DateTime<Utc>) -> ApplyResult {
        if events.is_empty() {
            return ApplyResult::default();
        }

        // (a) Group events by pane_id (fallback: session_to_pane → session_key).
        // Invariant: all source events for the same pane enter the same resolver batch.
        let mut by_group: HashMap<String, Vec<SourceEventV2>> = HashMap::new();
        for event in events {
            let group_key = event
                .pane_id
                .clone()
                .or_else(|| self.session_to_pane.get(&event.session_key).cloned())
                .unwrap_or_else(|| event.session_key.clone());
            by_group.entry(group_key).or_default().push(event);
        }

        let mut result = ApplyResult::default();

        // Process sorted for determinism in tests
        let mut group_keys: Vec<_> = by_group.keys().cloned().collect();
        group_keys.sort();

        for group_key in group_keys {
            let group_events = by_group.remove(&group_key).unwrap_or_default();

            // (b) resolver_states keyed by group_key (= pane_id or fallback).
            // deterministic_last_seen is tracked per-pane across all sources.
            let prev_state = self.resolver_states.get(&group_key);

            let output = resolver::resolve(group_events, now, prev_state, &self.source_ranks);

            // Always update resolver state (tracks deterministic_last_seen)
            self.resolver_states
                .insert(group_key.clone(), output.next_state.clone());

            result.events_accepted += output.accepted_events.len();
            result.events_suppressed += output.suppressed_events.len();
            result.duplicates_dropped += output.duplicates_dropped;

            // Only project when there are accepted events
            if output.accepted_events.is_empty() {
                continue;
            }

            // (c) One group may contain multiple session_keys (different sources).
            // Project each unique session_key independently.
            let mut session_keys_in_group: Vec<String> = output
                .accepted_events
                .iter()
                .map(|e| e.session_key.clone())
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
            session_keys_in_group.sort();
            for sk in &session_keys_in_group {
                if self.project_session(sk, &output, now) {
                    result.sessions_changed += 1;
                }
            }

            // B2: Update last_real_activity for non-heartbeat deterministic events.
            for event in &output.accepted_events {
                if !event.is_heartbeat
                    && event.tier == EvidenceTier::Deterministic
                    && let Some(pane_id) = &event.pane_id
                {
                    let entry = self
                        .last_real_activity
                        .entry(pane_id.clone())
                        .or_default()
                        .entry(event.provider)
                        .or_insert(event.observed_at);
                    if event.observed_at > *entry {
                        *entry = event.observed_at;
                    }
                }
            }

            // B3: Determine winning provider for this group (cross-provider arbitration).
            let winning_provider =
                self.select_winning_provider(&group_key, &output.accepted_events);

            // Update pane states from accepted events (dedup same pane_id).
            // Only project events from the winning provider to avoid nondeterministic overwrite.
            let mut panes_counted: HashSet<String> = HashSet::new();
            for event in &output.accepted_events {
                let pane_id = event
                    .pane_id
                    .clone()
                    .or_else(|| self.session_to_pane.get(&event.session_key).cloned());
                if let Some(pane_id) = pane_id {
                    // Provider arbitration: skip events from losing providers.
                    if let Some(wp) = winning_provider
                        && event.provider != wp
                    {
                        continue;
                    }
                    if self.project_pane(&pane_id, event, &output, now)
                        && panes_counted.insert(pane_id)
                    {
                        result.panes_changed += 1;
                    }
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
        // Determine activity state from the latest accepted event for this session.
        // Filter by session_key so events from other sessions in the same group
        // (different sources for the same pane) don't cross-contaminate.
        // Tie-break on event_id for determinism when timestamps are equal.
        let latest_event = output
            .accepted_events
            .iter()
            .filter(|e| e.session_key == session_key)
            .max_by(|a, b| {
                a.observed_at
                    .cmp(&b.observed_at)
                    .then_with(|| a.event_id.cmp(&b.event_id))
            });

        // Prefer the latest *non-heartbeat* event for activity_state.
        // Periodic idle_heartbeats (is_heartbeat=true) have Utc::now() as observed_at
        // and can be newer than real hooks events; using them for activity_state would
        // flip Running→Idle between back-to-back tool calls.
        // Fall back to latest_event only if all events are heartbeats (e.g. initial bootstrap).
        let state_event = output
            .accepted_events
            .iter()
            .filter(|e| e.session_key == session_key && !e.is_heartbeat)
            .max_by(|a, b| {
                a.observed_at
                    .cmp(&b.observed_at)
                    .then_with(|| a.event_id.cmp(&b.event_id))
            })
            .or(latest_event);

        let (activity_state, activity_source) = match state_event {
            Some(event) => (
                sync_v2_compat::parse_activity_state(&event.event_type),
                event.source_kind,
            ),
            None => return false,
        };

        let evidence_mode = tier_to_evidence_mode(output.result.winner_tier);

        // updated_at semantics (mirrors project_pane):
        // - Real events: use actual_activity_at (bootstrap) or observed_at, capped at now.
        // - Heartbeats: preserve existing session updated_at; fall back to actual_activity_at
        //   (ambiguous bootstrap) then observed_at when session is new.
        let event_ts = match latest_event {
            None => now,
            Some(e) if e.is_heartbeat => self
                .sessions
                .get(session_key)
                .map(|s| s.updated_at)
                .or(e.actual_activity_at)
                .unwrap_or_else(|| e.observed_at.min(now)),
            Some(e) => e.actual_activity_at.unwrap_or(e.observed_at).min(now),
        };

        let new_state = SessionRuntimeState {
            session_key: session_key.to_owned(),
            presence: PanePresence::Managed,
            evidence_mode,
            deterministic_last_seen: output.next_state.deterministic_last_seen,
            winner_tier: output.result.winner_tier,
            activity_state,
            activity_source,
            representative_pane_instance_id: None, // T-042
            updated_at: event_ts,
        };

        let changed = self.sessions.get(session_key).is_none_or(|existing| {
            existing.activity_state != new_state.activity_state
                || existing.evidence_mode != new_state.evidence_mode
                || existing.winner_tier != new_state.winner_tier
                || existing.activity_source != new_state.activity_source
        });

        if changed {
            self.version += 1;
            self.record_change(StateChange {
                version: self.version,
                session_key: session_key.to_owned(),
                pane_id: None,
                timestamp: now,
                session_state: Some(new_state.clone()),
                pane_state: None,
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
            generation: event.pane_generation.unwrap_or_else(|| {
                self.panes
                    .get(pane_id)
                    .map(|p| p.pane_instance_id.generation)
                    .unwrap_or(0)
            }),
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

        // (d) Check if deterministic evidence is fresh for this pane.
        // Use the pane's group_key (pane_id) to look up the resolver state,
        // not event.session_key — ensures cross-source freshness is tracked.
        let deterministic_fresh_active = {
            let pane_resolver_key = event.pane_id.as_deref().unwrap_or(&event.session_key);
            let resolver_state = self.resolver_states.get(pane_resolver_key);
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

        // Heartbeats (is_heartbeat=true) preserve existing activity_state to prevent
        // periodic idle_heartbeats from flipping Running→Idle between real tool events.
        // If the pane has no prior state (first event is a heartbeat), fall back to
        // the heartbeat's own event_type so initial bootstraps still work.
        // Legacy sync-v2 projection still derives its collapsed activity enum from
        // the compat `event_type` string. The sync-v3 truth path is updated
        // separately from provider-native payloads and must not depend on this parser.
        let pane_activity_state = if event.is_heartbeat {
            self.panes
                .get(pane_id)
                .map(|p| p.activity_state)
                .unwrap_or_else(|| sync_v2_compat::parse_activity_state(&event.event_type))
        } else {
            sync_v2_compat::parse_activity_state(&event.event_type)
        };
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
            session_key: event.session_key.clone(),
            // updated_at semantics:
            // - Real events (is_heartbeat=false): use actual_activity_at (bootstrap carries
            //   last JSONL line ts) or observed_at, capped at now.
            // - Heartbeats (is_heartbeat=true): preserve existing updated_at; fall back to
            //   actual_activity_at (ambiguous bootstrap) then observed_at when pane is new.
            updated_at: if event.is_heartbeat {
                self.panes
                    .get(pane_id)
                    .map(|p| p.updated_at)
                    .or(event.actual_activity_at)
                    .unwrap_or_else(|| event.observed_at.min(now))
            } else {
                event
                    .actual_activity_at
                    .unwrap_or(event.observed_at)
                    .min(now)
            },
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
            self.record_change(StateChange {
                version: self.version,
                session_key: event.session_key.clone(),
                pane_id: Some(pane_id.to_owned()),
                timestamp: now,
                session_state: None,
                pane_state: Some(new_state.clone()),
            });
        }

        self.session_to_pane
            .insert(event.session_key.clone(), pane_id.to_owned());
        self.panes.insert(pane_id.to_owned(), new_state);
        changed
    }

    // ── Provider Arbitration (T-123) ───────────────────────────────

    /// Determine the winning provider for a pane when multiple deterministic
    /// sources are active simultaneously (e.g. Codex heartbeat + Claude JSONL).
    ///
    /// Rules:
    /// 1. If ≤1 Det provider in accepted events → no conflict, return that provider.
    /// 2. Conflict: winner = provider with most recent non-heartbeat Det activity.
    /// 3. No history: keep current pane provider, or fall back to latest-event provider.
    fn select_winning_provider(
        &self,
        pane_id: &str,
        accepted_events: &[SourceEventV2],
    ) -> Option<Provider> {
        let det_providers: HashSet<Provider> = accepted_events
            .iter()
            .filter(|e| e.tier == EvidenceTier::Deterministic)
            .map(|e| e.provider)
            .collect();

        // No conflict: 0 or 1 Det provider.
        if det_providers.len() <= 1 {
            return accepted_events.iter().map(|e| e.provider).next();
        }

        // Conflict: winner = provider with most recent real activity.
        if let Some(map) = self.last_real_activity.get(pane_id) {
            let winner = map
                .iter()
                .filter(|(p, _)| det_providers.contains(p))
                .max_by_key(|(_, t)| *t)
                .map(|(p, _)| *p);
            if winner.is_some() {
                return winner;
            }
        }

        // No activity history yet: keep current pane provider, or latest-event provider.
        self.panes
            .get(pane_id)
            .and_then(|p| p.provider)
            .or_else(|| {
                accepted_events
                    .iter()
                    .max_by_key(|e| e.observed_at)
                    .map(|e| e.provider)
            })
    }

    // ── Client API ─────────────────────────────────────────────────

    /// Current replay epoch for `ui.bootstrap.v2` / `ui.changes.v2`.
    pub fn replay_epoch(&self) -> u64 {
        self.epoch
    }

    /// Current replay cursor at the head of the change log.
    pub fn replay_cursor(&self) -> ReplayCursor {
        ReplayCursor {
            epoch: self.epoch,
            seq: self.version,
        }
    }

    /// Strict replay for `ui.changes.v2`.
    ///
    /// The cursor is interpreted as "the client has applied all changes up to `seq`".
    /// Returned changes therefore start at `seq + 1`.
    pub fn replay_changes(
        &self,
        cursor: ReplayCursor,
        limit: usize,
    ) -> Result<ReplayBatch<'_>, ResyncRequired> {
        if cursor.epoch != self.epoch {
            return Err(self.resync_required("epoch_mismatch"));
        }

        if cursor.seq > self.version {
            return Err(self.resync_required("unknown_cursor"));
        }

        if self.replay_log.is_empty() {
            return if cursor.seq == self.version {
                Ok(ReplayBatch {
                    epoch: self.epoch,
                    from_seq: cursor.seq,
                    to_seq: cursor.seq,
                    next_cursor: cursor,
                    changes: Vec::new(),
                })
            } else {
                Err(self.resync_required("trimmed_cursor"))
            };
        }

        let first_retained = self.replay_log[0].version;
        let expected_next = cursor.seq.saturating_add(1);
        if expected_next < first_retained {
            return Err(self.resync_required("trimmed_cursor"));
        }

        let start = self.replay_log.partition_point(|c| c.version <= cursor.seq);
        if start < self.replay_log.len() {
            if cursor.seq < self.version && self.replay_log[start].version != expected_next {
                return Err(self.resync_required("replay_miss"));
            }
        } else if cursor.seq < self.version {
            return Err(self.resync_required("replay_miss"));
        }

        let end = start.saturating_add(limit).min(self.replay_log.len());
        let selected: Vec<&StateChange> = self.replay_log[start..end].iter().collect();

        let mut prev_seq = cursor.seq;
        for change in &selected {
            if change.version != prev_seq.saturating_add(1) {
                return Err(self.resync_required("replay_miss"));
            }
            prev_seq = change.version;
        }

        if let Some(last_change) = selected.last() {
            Ok(ReplayBatch {
                epoch: self.epoch,
                from_seq: selected[0].version,
                to_seq: last_change.version,
                next_cursor: ReplayCursor {
                    epoch: self.epoch,
                    seq: last_change.version,
                },
                changes: selected,
            })
        } else {
            Ok(ReplayBatch {
                epoch: self.epoch,
                from_seq: cursor.seq,
                to_seq: cursor.seq,
                next_cursor: cursor,
                changes: selected,
            })
        }
    }

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

    /// Remove sync-v2 replay entries with version <= `before_version`.
    pub fn trim_replay_before(&mut self, before_version: StateVersion) {
        self.replay_log.retain(|c| c.version > before_version);
    }

    /// Record the highest sync-v2 cursor the client has acknowledged.
    ///
    /// Acknowledged replay entries are compacted immediately from the sync-v2
    /// replay log. The legacy `changes` log remains untouched.
    pub fn acknowledge_replay_cursor(&mut self, cursor: ReplayCursor) {
        if cursor.epoch != self.epoch || cursor.seq > self.version {
            return;
        }
        if cursor.seq <= self.replay_acked_cursor.seq {
            return;
        }

        self.replay_acked_cursor = cursor;
        self.trim_replay_before(cursor.seq);

        if cursor.seq == self.version {
            self.replay_last_resync_reason = None;
            self.replay_last_resync_at = None;
        }
    }

    /// Record that sync-v2 required an explicit resync.
    pub fn record_replay_resync(&mut self, reason: &'static str, at: DateTime<Utc>) {
        self.replay_last_resync_reason = Some(reason);
        self.replay_last_resync_at = Some(at);
    }

    /// Snapshot replay observability counters for `ui.health.v1`.
    pub fn replay_health_snapshot(&self) -> ReplayHealthSnapshot {
        let cursor_seq = if self.replay_acked_cursor.epoch == self.epoch {
            self.replay_acked_cursor.seq.min(self.version)
        } else {
            0
        };
        ReplayHealthSnapshot {
            current_epoch: self.epoch,
            cursor_seq,
            head_seq: self.version,
            lag: self.version.saturating_sub(cursor_seq),
            last_resync_reason: self.replay_last_resync_reason,
            last_resync_at: self.replay_last_resync_at,
        }
    }

    /// Current number of retained sync-v2 replay entries.
    pub fn replay_len(&self) -> usize {
        self.replay_log.len()
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

    /// Demote exact pane rows back to unmanaged truth by removing managed state.
    ///
    /// Used when live tmux inventory proves that a pane has already returned to a
    /// plain shell. This exact-row shell truth must override any stale managed
    /// heuristic/deterministic state still retained in the projection.
    ///
    /// Returns the number of change-log entries recorded.
    pub fn demote_panes_to_unmanaged(&mut self, pane_ids: &[String], now: DateTime<Utc>) -> usize {
        let mut change_count = 0usize;

        for pane_id in pane_ids {
            let Some(pane) = self.panes.remove(pane_id) else {
                continue;
            };

            self.resolver_states.remove(pane_id);
            self.last_real_activity.remove(pane_id);

            let mut affected_sessions: Vec<String> = self
                .session_to_pane
                .iter()
                .filter(|(_, mapped_pane_id)| *mapped_pane_id == pane_id)
                .map(|(session_key, _)| session_key.clone())
                .collect();
            self.session_to_pane
                .retain(|_, mapped_pane_id| mapped_pane_id != pane_id);

            if !affected_sessions
                .iter()
                .any(|session_key| session_key == &pane.session_key)
            {
                affected_sessions.push(pane.session_key.clone());
            }
            affected_sessions.sort();
            affected_sessions.dedup();

            self.version += 1;
            self.record_change(StateChange {
                version: self.version,
                session_key: pane.session_key.clone(),
                pane_id: Some(pane_id.clone()),
                timestamp: now,
                session_state: None,
                pane_state: None,
            });
            change_count += 1;

            for session_key in affected_sessions {
                if self.sessions.remove(&session_key).is_some() {
                    self.version += 1;
                    self.record_change(StateChange {
                        version: self.version,
                        session_key,
                        pane_id: None,
                        timestamp: now,
                        session_state: None,
                        pane_state: None,
                    });
                    change_count += 1;
                }
            }
        }

        change_count
    }

    fn resync_required(&self, reason: &'static str) -> ResyncRequired {
        ResyncRequired {
            current_epoch: self.epoch,
            latest_snapshot_seq: self.version,
            reason,
        }
    }

    /// Number of tracked sessions.
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Number of tracked panes.
    pub fn pane_count(&self) -> usize {
        self.panes.len()
    }

    /// Evaluate deterministic freshness for all tracked panes.
    ///
    /// Panes whose deterministic evidence has gone stale (no new events
    /// within `DOWN_THRESHOLD_SECS`) are downgraded to heuristic evidence
    /// mode. This prevents panes from being permanently frozen as
    /// deterministic when their source stops emitting events.
    ///
    /// Returns the number of panes whose evidence mode changed.
    pub fn tick_freshness(&mut self, now: DateTime<Utc>) -> usize {
        let mut changed = 0;

        // Find panes whose resolver state is Deterministic but freshness is not Fresh
        let stale_keys: Vec<String> = self
            .resolver_states
            .iter()
            .filter(|(_, state)| {
                state.current_tier == EvidenceTier::Deterministic
                    && resolver::classify_freshness(state.deterministic_last_seen, now)
                        != resolver::Freshness::Fresh
            })
            .map(|(k, _)| k.clone())
            .collect();

        for key in stale_keys {
            // Run resolver with empty events to trigger tier fallback
            let prev_state = self.resolver_states.get(&key);
            let output = resolver::resolve(vec![], now, prev_state, &self.source_ranks);
            self.resolver_states
                .insert(key.clone(), output.next_state.clone());

            // Update pane evidence_mode if it changed
            let pane_change = if let Some(pane) = self.panes.get_mut(&key) {
                let new_mode = tier_to_evidence_mode(output.result.winner_tier);
                if pane.evidence_mode != new_mode {
                    pane.evidence_mode = new_mode;
                    pane.updated_at = now;
                    self.version += 1;
                    Some(StateChange {
                        version: self.version,
                        session_key: pane.session_key.clone(),
                        pane_id: Some(key.clone()),
                        timestamp: now,
                        session_state: None,
                        pane_state: Some(pane.clone()),
                    })
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(change) = pane_change {
                self.record_change(change);
                changed += 1;
            }

            // Update session evidence_mode if it changed.
            let affected_sessions: Vec<String> = self
                .session_to_pane
                .iter()
                .filter(|(_, pane_id)| *pane_id == &key)
                .map(|(session_key, _)| session_key.clone())
                .collect();
            for session_key in affected_sessions {
                let session_change = if let Some(session) = self.sessions.get_mut(&session_key) {
                    let new_mode = tier_to_evidence_mode(output.result.winner_tier);
                    if session.evidence_mode != new_mode {
                        session.evidence_mode = new_mode;
                        session.updated_at = now;
                        self.version += 1;
                        Some(StateChange {
                            version: self.version,
                            session_key: session_key.clone(),
                            pane_id: None,
                            timestamp: now,
                            session_state: Some(session.clone()),
                            pane_state: None,
                        })
                    } else {
                        None
                    }
                } else {
                    None
                };
                if let Some(change) = session_change {
                    self.record_change(change);
                }
            }

            // T-123: clear provider activity history for this stale pane.
            self.last_real_activity.remove(&key);
        }

        changed
    }

    fn record_change(&mut self, change: StateChange) {
        self.replay_log.push(change.clone());
        self.changes.push(change);
    }
}

// ─── Helpers ─────────────────────────────────────────────────────

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
    use agtmux_core_v5::types::{ActivityState, SourceKind};
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
            is_heartbeat: false,
            actual_activity_at: None,
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
    fn sync_v2_compat_activity_state_parsing() {
        assert_eq!(
            sync_v2_compat::parse_activity_state("activity.running"),
            ActivityState::Running
        );
        assert_eq!(
            sync_v2_compat::parse_activity_state("lifecycle.running"),
            ActivityState::Running
        );
        assert_eq!(
            sync_v2_compat::parse_activity_state("activity.idle"),
            ActivityState::Idle
        );
        assert_eq!(
            sync_v2_compat::parse_activity_state("lifecycle.idle"),
            ActivityState::Idle
        );
        assert_eq!(
            sync_v2_compat::parse_activity_state("activity.waiting_input"),
            ActivityState::WaitingInput
        );
        assert_eq!(
            sync_v2_compat::parse_activity_state("activity.waiting_approval"),
            ActivityState::WaitingApproval
        );
        assert_eq!(
            sync_v2_compat::parse_activity_state("activity.error"),
            ActivityState::Error
        );
        assert_eq!(
            sync_v2_compat::parse_activity_state("lifecycle.start"),
            ActivityState::Running
        );
        assert_eq!(
            sync_v2_compat::parse_activity_state("activity.start"),
            ActivityState::Running
        );
        assert_eq!(
            sync_v2_compat::parse_activity_state("lifecycle.end"),
            ActivityState::Idle
        );
        assert_eq!(
            sync_v2_compat::parse_activity_state("lifecycle.stop"),
            ActivityState::Idle
        );
        assert_eq!(
            sync_v2_compat::parse_activity_state("activity.end"),
            ActivityState::Idle
        );
        assert_eq!(
            sync_v2_compat::parse_activity_state("activity.stop"),
            ActivityState::Idle
        );
        assert_eq!(
            sync_v2_compat::parse_activity_state("lifecycle.waiting_input"),
            ActivityState::WaitingInput
        );
        assert_eq!(
            sync_v2_compat::parse_activity_state("lifecycle.waiting_approval"),
            ActivityState::WaitingApproval
        );
        assert_eq!(
            sync_v2_compat::parse_activity_state("lifecycle.error"),
            ActivityState::Error
        );
        assert_eq!(
            sync_v2_compat::parse_activity_state("unknown.type"),
            ActivityState::Unknown
        );

        // Claude JSONL namespace
        assert_eq!(
            sync_v2_compat::parse_activity_state("activity.user_input"),
            ActivityState::Running
        );
        assert_eq!(
            sync_v2_compat::parse_activity_state("activity.tool_complete"),
            ActivityState::Running
        );

        // Codex App Server namespace
        assert_eq!(
            sync_v2_compat::parse_activity_state("thread.active"),
            ActivityState::Running
        );
        assert_eq!(
            sync_v2_compat::parse_activity_state("thread.idle"),
            ActivityState::Idle
        );
        assert_eq!(
            sync_v2_compat::parse_activity_state("thread.error"),
            ActivityState::Error
        );
        assert_eq!(
            sync_v2_compat::parse_activity_state("thread.systemError"),
            ActivityState::Error
        );
        assert_eq!(
            sync_v2_compat::parse_activity_state("turn.started"),
            ActivityState::Running
        );
        assert_eq!(
            sync_v2_compat::parse_activity_state("turn.inProgress"),
            ActivityState::Running
        );
        assert_eq!(
            sync_v2_compat::parse_activity_state("turn.completed"),
            ActivityState::Idle
        );
        assert_eq!(
            sync_v2_compat::parse_activity_state("turn.interrupted"),
            ActivityState::Idle
        );
        assert_eq!(
            sync_v2_compat::parse_activity_state("turn.failed"),
            ActivityState::Error
        );
        // notLoaded threads map to Idle (defensive — primary filter is in codex_poller)
        assert_eq!(
            sync_v2_compat::parse_activity_state("thread.not_loaded"),
            ActivityState::Idle
        );
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

    #[test]
    fn demote_panes_to_unmanaged_removes_exact_row_and_session_state() {
        let mut proj = DaemonProjection::new();
        let t = t0();

        proj.apply_events(vec![det_event("e1", "s1", "%1", "activity.running", t)], t);
        let before = proj.version();

        let changed =
            proj.demote_panes_to_unmanaged(&["%1".to_string()], t + TimeDelta::seconds(1));

        assert_eq!(
            changed, 2,
            "pane removal + session removal should be recorded"
        );
        assert!(
            proj.get_pane("%1").is_none(),
            "managed pane must be removed"
        );
        assert!(
            proj.get_session("s1").is_none(),
            "session state must not survive exact-row shell demotion"
        );
        assert!(
            !proj.session_to_pane.contains_key("s1"),
            "session-to-pane link must be cleared"
        );
        assert!(
            !proj.resolver_states.contains_key("%1"),
            "resolver state must be cleared so stale freshness cannot resurrect the row"
        );

        let changes = proj.changes_since(before);
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].pane_id.as_deref(), Some("%1"));
        assert!(changes[0].pane_state.is_none(), "pane removal change");
        assert!(changes[1].pane_id.is_none(), "session removal change");
        assert!(changes[1].session_state.is_none(), "session removal change");
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

    #[test]
    fn replay_changes_same_epoch_ordered() {
        let mut proj = DaemonProjection::new();
        let t = t0();

        proj.apply_events(vec![det_event("e1", "s1", "%1", "activity.running", t)], t);
        proj.apply_events(
            vec![det_event(
                "e2",
                "s1",
                "%1",
                "activity.waiting_input",
                t + TimeDelta::seconds(1),
            )],
            t + TimeDelta::seconds(1),
        );

        let batch = proj
            .replay_changes(ReplayCursor { epoch: 1, seq: 0 }, 10)
            .expect("valid replay");

        assert_eq!(batch.epoch, 1);
        assert_eq!(batch.from_seq, 1);
        assert_eq!(batch.to_seq, 4);
        assert_eq!(batch.next_cursor, ReplayCursor { epoch: 1, seq: 4 });
        assert_eq!(batch.changes.len(), 4);
        assert_eq!(
            batch.changes.iter().map(|c| c.version).collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
    }

    #[test]
    fn replay_changes_resyncs_on_epoch_mismatch() {
        let mut proj = DaemonProjection::new();
        let t = t0();
        proj.apply_events(vec![det_event("e1", "s1", "%1", "activity.running", t)], t);

        let err = proj
            .replay_changes(ReplayCursor { epoch: 99, seq: 0 }, 10)
            .expect_err("epoch mismatch");
        assert_eq!(err.reason, "epoch_mismatch");
        assert_eq!(err.current_epoch, 1);
        assert_eq!(err.latest_snapshot_seq, proj.version());
    }

    #[test]
    fn replay_changes_resyncs_on_trimmed_cursor() {
        let mut proj = DaemonProjection::new();
        let t = t0();

        proj.apply_events(vec![det_event("e1", "s1", "%1", "activity.running", t)], t);
        proj.apply_events(
            vec![det_event(
                "e2",
                "s1",
                "%1",
                "activity.waiting_input",
                t + TimeDelta::seconds(1),
            )],
            t + TimeDelta::seconds(1),
        );
        proj.trim_replay_before(2);

        let err = proj
            .replay_changes(ReplayCursor { epoch: 1, seq: 0 }, 10)
            .expect_err("trimmed cursor");
        assert_eq!(err.reason, "trimmed_cursor");
        assert_eq!(err.latest_snapshot_seq, proj.version());
    }

    #[test]
    fn replay_changes_resyncs_on_unknown_cursor() {
        let mut proj = DaemonProjection::new();
        let t = t0();
        proj.apply_events(vec![det_event("e1", "s1", "%1", "activity.running", t)], t);

        let err = proj
            .replay_changes(
                ReplayCursor {
                    epoch: 1,
                    seq: proj.version() + 1,
                },
                10,
            )
            .expect_err("unknown cursor");
        assert_eq!(err.reason, "unknown_cursor");
    }

    #[test]
    fn replay_changes_resyncs_on_replay_gap() {
        let mut proj = DaemonProjection::new();
        proj.version = 3;
        proj.replay_log = vec![
            StateChange {
                version: 1,
                session_key: "s1".to_owned(),
                pane_id: None,
                timestamp: t0(),
                session_state: None,
                pane_state: None,
            },
            StateChange {
                version: 3,
                session_key: "s1".to_owned(),
                pane_id: Some("%1".to_owned()),
                timestamp: t0() + TimeDelta::seconds(1),
                session_state: None,
                pane_state: None,
            },
        ];

        let err = proj
            .replay_changes(ReplayCursor { epoch: 1, seq: 0 }, 10)
            .expect_err("gap should resync");
        assert_eq!(err.reason, "replay_miss");
    }

    #[test]
    fn acknowledge_replay_cursor_compacts_only_sync_v2_log() {
        let mut proj = DaemonProjection::new();
        let t = t0();

        proj.apply_events(vec![det_event("e1", "s1", "%1", "activity.running", t)], t);
        proj.apply_events(
            vec![det_event(
                "e2",
                "s1",
                "%1",
                "activity.waiting_input",
                t + TimeDelta::seconds(1),
            )],
            t + TimeDelta::seconds(1),
        );

        let legacy_len = proj.changes_since(0).len();
        assert_eq!(proj.replay_len(), legacy_len);

        let head = proj.replay_cursor();
        proj.acknowledge_replay_cursor(head);

        assert_eq!(proj.replay_len(), 0, "sync-v2 replay log should compact");
        assert_eq!(
            proj.changes_since(0).len(),
            legacy_len,
            "legacy change log must remain intact"
        );

        let batch = proj.replay_changes(head, 10).expect("caught up cursor");
        assert!(batch.changes.is_empty());
    }

    #[test]
    fn replay_health_snapshot_tracks_lag_and_resync() {
        let mut proj = DaemonProjection::new();
        let t = t0();

        proj.apply_events(vec![det_event("e1", "s1", "%1", "activity.running", t)], t);
        proj.record_replay_resync("trimmed_cursor", t + TimeDelta::seconds(1));

        let initial = proj.replay_health_snapshot();
        assert_eq!(initial.current_epoch, 1);
        assert_eq!(initial.cursor_seq, 0);
        assert_eq!(initial.head_seq, proj.version());
        assert_eq!(initial.lag, proj.version());
        assert_eq!(initial.last_resync_reason, Some("trimmed_cursor"));
        assert_eq!(initial.last_resync_at, Some(t + TimeDelta::seconds(1)));

        proj.acknowledge_replay_cursor(proj.replay_cursor());

        let recovered = proj.replay_health_snapshot();
        assert_eq!(recovered.lag, 0);
        assert_eq!(recovered.last_resync_reason, None);
        assert_eq!(recovered.last_resync_at, None);
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

    #[test]
    fn event_without_pane_id_updates_known_pane() {
        let mut proj = DaemonProjection::new();
        let t = t0();

        // First deterministic event establishes session->pane mapping.
        proj.apply_events(vec![det_event("e1", "thr-1", "%1", "thread.active", t)], t);
        let pane = proj.get_pane("%1").expect("pane");
        assert_eq!(pane.activity_state, ActivityState::Running);

        // Follow-up event for the same session has no pane_id (notification/global path).
        let t2 = t + TimeDelta::seconds(1);
        let event = make_event(
            "e2",
            agtmux_core_v5::types::Provider::Codex,
            SourceKind::CodexAppserver,
            "thr-1",
            None,
            "turn.completed",
            t2,
        );
        let result = proj.apply_events(vec![event], t2);

        assert_eq!(result.sessions_changed, 1);
        assert_eq!(result.panes_changed, 1);
        let pane = proj.get_pane("%1").expect("pane");
        assert_eq!(pane.activity_state, ActivityState::Idle);
        assert_eq!(pane.session_key, "thr-1");
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

    // ── 19. Pane updated_at reflects event observed_at, not processing time ──────

    #[test]
    fn updated_at_reflects_event_observed_at() {
        let mut proj = DaemonProjection::new();
        // Event observed 1s before processing — updated_at should use event_time, not now.
        let event_time = t0();
        let now = t0() + TimeDelta::seconds(1);

        proj.apply_events(
            vec![det_event("e1", "s1", "%1", "activity.running", event_time)],
            now,
        );

        let session = proj.get_session("s1").expect("session");
        assert_eq!(session.updated_at, event_time);

        let pane = proj.get_pane("%1").expect("pane");
        assert_eq!(pane.updated_at, event_time);
    }

    #[test]
    fn tick_freshness_records_pane_and_session_changes() {
        let mut proj = DaemonProjection::new();
        let t = t0();

        proj.apply_events(vec![det_event("e1", "s1", "%1", "activity.running", t)], t);
        let before = proj.version();

        let changed = proj.tick_freshness(t + TimeDelta::seconds(5));
        assert_eq!(changed, 1);

        let freshness_changes = proj.changes_since(before);
        assert_eq!(freshness_changes.len(), 2);

        let pane_change = freshness_changes
            .iter()
            .find(|change| change.pane_id == Some("%1".to_owned()))
            .expect("pane change");
        assert_eq!(pane_change.session_key, "s1");
        assert!(pane_change.pane_state.is_some());

        let session_change = freshness_changes
            .iter()
            .find(|change| change.pane_id.is_none())
            .expect("session change");
        assert_eq!(session_change.session_key, "s1");
        assert!(session_change.session_state.is_some());
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

    // ═══════════════════════════════════════════════════════════════
    // Cross-session_key evidence downgrade tests
    //
    // These tests reproduce the bug where different sources generate
    // different session_keys for the same pane, causing heuristic
    // evidence to overwrite deterministic evidence.
    // ═══════════════════════════════════════════════════════════════

    // Helper: Codex AppServer deterministic event (session_key = thread_id)
    fn codex_det_event(id: &str, thread_id: &str, pane: &str, at: DateTime<Utc>) -> SourceEventV2 {
        make_event(
            id,
            agtmux_core_v5::types::Provider::Codex,
            SourceKind::CodexAppserver,
            thread_id, // e.g. "thr_abc"
            Some(pane),
            "thread.active",
            at,
        )
    }

    // Helper: Poller heuristic event for Codex (session_key = "poller-{pane_id}")
    fn codex_poller_event(
        id: &str,
        pane: &str,
        event_type: &str,
        at: DateTime<Utc>,
    ) -> SourceEventV2 {
        let session_key = format!("poller-{pane}");
        let mut e = make_event(
            id,
            agtmux_core_v5::types::Provider::Codex,
            SourceKind::Poller,
            &session_key,
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

    // Helper: Claude Hooks deterministic event
    fn claude_det_event(
        id: &str,
        session_id: &str,
        pane: &str,
        at: DateTime<Utc>,
    ) -> SourceEventV2 {
        make_event(
            id,
            agtmux_core_v5::types::Provider::Claude,
            SourceKind::ClaudeHooks,
            session_id, // e.g. "claude-sess-xyz"
            Some(pane),
            "lifecycle.running",
            at,
        )
    }

    // Helper: Poller heuristic event for Claude (session_key = "poller-{pane_id}")
    fn claude_poller_event(
        id: &str,
        pane: &str,
        event_type: &str,
        at: DateTime<Utc>,
    ) -> SourceEventV2 {
        let session_key = format!("poller-{pane}");
        let mut e = make_event(
            id,
            agtmux_core_v5::types::Provider::Claude,
            SourceKind::Poller,
            &session_key,
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

    // ── 37. BUG REPRO: Deterministic overwritten by heuristic (same batch) ──

    #[test]
    fn cross_session_det_overwritten_by_heur_same_batch() {
        let mut proj = DaemonProjection::new();
        let now = Utc::now();

        // Codex AppServer deterministic event (session_key = "thr_abc")
        // AND Poller heuristic event (session_key = "poller-%1")
        // for the SAME pane %1, in the same batch.
        let events = vec![
            codex_det_event("e1", "thr_abc", "%1", now - TimeDelta::milliseconds(500)),
            codex_poller_event(
                "e2",
                "%1",
                "activity.running",
                now - TimeDelta::milliseconds(500),
            ),
        ];
        proj.apply_events(events, now);

        let pane = proj.get_pane("%1").expect("pane should exist");

        // BUG: With session_key-based grouping, the poller's "poller-%1" session
        // is resolved independently (no deterministic evidence in its batch),
        // so it sets evidence_mode=Heuristic. Since "poller-%1" sorts after
        // "thr_abc", it overwrites the deterministic result.
        //
        // EXPECTED after fix: evidence_mode should be Deterministic because
        // Codex AppServer's deterministic evidence should take priority.
        assert_eq!(
            pane.evidence_mode,
            EvidenceMode::Deterministic,
            "deterministic evidence should NOT be overwritten by heuristic \
             from a different session_key targeting the same pane"
        );
    }

    // ── 38. BUG REPRO: Deterministic overwritten by heuristic (sequential ticks) ──

    #[test]
    fn cross_session_det_overwritten_by_heur_sequential_ticks() {
        let mut proj = DaemonProjection::new();
        let now = Utc::now();

        // Tick 1: Codex AppServer deterministic event
        proj.apply_events(vec![codex_det_event("e1", "thr_abc", "%1", now)], now);

        let pane = proj.get_pane("%1").expect("pane after tick 1");
        assert_eq!(pane.evidence_mode, EvidenceMode::Deterministic);

        // Tick 2 (1s later): ONLY poller event arrives for same pane
        // (different session_key "poller-%1")
        let now2 = now + TimeDelta::seconds(1);
        proj.apply_events(
            vec![codex_poller_event("e2", "%1", "activity.running", now2)],
            now2,
        );

        let pane = proj.get_pane("%1").expect("pane after tick 2");

        // BUG: Poller's session "poller-%1" has no deterministic history,
        // so winner_tier=Heuristic, overwriting the pane's Deterministic state.
        //
        // EXPECTED after fix: Deterministic should be maintained because
        // the pane's det_last_seen (from tick 1) is still fresh (1s < 3s).
        assert_eq!(
            pane.evidence_mode,
            EvidenceMode::Deterministic,
            "fresh deterministic evidence (1s old) should not be overwritten by heuristic"
        );
    }

    // ── 39. Heuristic correctly takes over when deterministic goes stale ──

    #[test]
    fn cross_session_heur_takes_over_when_det_stale() {
        let mut proj = DaemonProjection::new();
        let now = Utc::now();

        // Tick 1: Deterministic
        proj.apply_events(vec![codex_det_event("e1", "thr_abc", "%1", now)], now);
        assert_eq!(
            proj.get_pane("%1").expect("pane %1").evidence_mode,
            EvidenceMode::Deterministic,
        );

        // Tick 2 (4s later): Det is stale (>3s), only poller arrives
        let now2 = now + TimeDelta::seconds(4);
        proj.apply_events(
            vec![codex_poller_event("e2", "%1", "activity.idle", now2)],
            now2,
        );

        let pane = proj.get_pane("%1").expect("pane after tick 2");
        assert_eq!(
            pane.evidence_mode,
            EvidenceMode::Heuristic,
            "when deterministic is stale (>3s), heuristic should take over"
        );
    }

    // ── 40. Deterministic recovery after stale fallback ──

    #[test]
    fn cross_session_det_recovery_after_stale() {
        let mut proj = DaemonProjection::new();
        let now = Utc::now();

        // Tick 1: Deterministic
        proj.apply_events(vec![codex_det_event("e1", "thr_abc", "%1", now)], now);
        assert_eq!(
            proj.get_pane("%1").expect("pane %1").evidence_mode,
            EvidenceMode::Deterministic,
        );

        // Tick 2 (4s): Stale → Heuristic
        let now2 = now + TimeDelta::seconds(4);
        proj.apply_events(
            vec![codex_poller_event("e2", "%1", "activity.idle", now2)],
            now2,
        );
        assert_eq!(
            proj.get_pane("%1").expect("pane %1").evidence_mode,
            EvidenceMode::Heuristic,
        );

        // Tick 3 (5s): Deterministic recovers
        let now3 = now + TimeDelta::seconds(5);
        proj.apply_events(vec![codex_det_event("e3", "thr_abc", "%1", now3)], now3);
        assert_eq!(
            proj.get_pane("%1").expect("pane %1").evidence_mode,
            EvidenceMode::Deterministic,
            "deterministic should re-promote after recovery"
        );
    }

    // ── 41. Provider switch: Codex (Det) → Claude (Heur, no hooks) ──

    #[test]
    fn cross_session_provider_switch_codex_to_claude() {
        let mut proj = DaemonProjection::new();
        let now = Utc::now();

        // Tick 1: Codex with deterministic AppServer evidence
        proj.apply_events(
            vec![
                codex_det_event("e1", "thr_abc", "%1", now),
                codex_poller_event("e2", "%1", "activity.running", now),
            ],
            now,
        );
        let pane = proj.get_pane("%1").expect("pane %1");
        assert_eq!(pane.evidence_mode, EvidenceMode::Deterministic);
        assert_eq!(pane.provider, Some(agtmux_core_v5::types::Provider::Codex),);

        // [User switches pane from Codex to Claude]
        // Tick 2 (4s later): Only Claude poller events, Codex AppServer stopped
        let now2 = now + TimeDelta::seconds(4);
        proj.apply_events(
            vec![claude_poller_event("e3", "%1", "activity.idle", now2)],
            now2,
        );
        let pane = proj.get_pane("%1").expect("pane %1");
        assert_eq!(
            pane.evidence_mode,
            EvidenceMode::Heuristic,
            "after Codex stops (det stale >3s), Claude heuristic should take over"
        );
        assert_eq!(
            pane.provider,
            Some(agtmux_core_v5::types::Provider::Claude),
            "provider should switch to Claude"
        );
    }

    // ── 42. Claude with hooks (Det) + Claude poller (Heur) same pane ──

    #[test]
    fn cross_session_claude_det_plus_poller_heur() {
        let mut proj = DaemonProjection::new();
        let now = Utc::now();

        // Claude Hooks (deterministic, session="claude-sess-xyz")
        // + Poller (heuristic, session="poller-%2")
        // for pane %2
        let events = vec![
            claude_det_event("e1", "claude-sess-xyz", "%2", now),
            claude_poller_event("e2", "%2", "activity.running", now),
        ];
        proj.apply_events(events, now);

        let pane = proj.get_pane("%2").expect("pane");
        assert_eq!(
            pane.evidence_mode,
            EvidenceMode::Deterministic,
            "Claude hooks deterministic should win over Claude poller heuristic"
        );
    }

    // ── 43. Three sources: Codex Det + Claude Det + Poller Heur ──

    #[test]
    fn cross_session_three_sources_same_pane() {
        let mut proj = DaemonProjection::new();
        let now = Utc::now();

        // Unlikely but possible: all three sources target pane %1
        let events = vec![
            codex_det_event("e1", "thr_abc", "%1", now),
            claude_det_event("e2", "claude-sess", "%1", now),
            codex_poller_event("e3", "%1", "activity.running", now),
        ];
        proj.apply_events(events, now);

        let pane = proj.get_pane("%1").expect("pane");
        assert_eq!(
            pane.evidence_mode,
            EvidenceMode::Deterministic,
            "deterministic should win when multiple sources target same pane"
        );
    }

    // ── 44. pane_id=None events use session_key grouping (regression guard) ──

    #[test]
    fn session_only_events_use_session_key_grouping() {
        let mut proj = DaemonProjection::new();
        let now = Utc::now();

        // Event with pane_id=None (session-level event)
        let event = make_event(
            "e1",
            agtmux_core_v5::types::Provider::Codex,
            SourceKind::CodexAppserver,
            "thr_abc",
            None, // no pane_id
            "thread.active",
            now,
        );
        let result = proj.apply_events(vec![event], now);

        // Should process without panic, update session state
        assert_eq!(result.events_accepted, 1);
        assert_eq!(result.sessions_changed, 1);
        // No pane projection (no pane_id)
        assert_eq!(result.panes_changed, 0);
    }

    // ── 45. deterministic_fresh_active uses pane-level freshness ──

    #[test]
    fn deterministic_fresh_active_cross_session() {
        let mut proj = DaemonProjection::new();
        let now = Utc::now();

        // Tick 1: Codex AppServer sets deterministic for pane %1
        proj.apply_events(vec![codex_det_event("e1", "thr_abc", "%1", now)], now);
        let pane = proj.get_pane("%1").expect("pane %1");
        assert_eq!(pane.signature_class, PaneSignatureClass::Deterministic);

        // Tick 2 (1s later): Poller event for same pane (different session_key).
        // The pane has deterministic evidence from tick 1 (still fresh).
        // After fix: deterministic_fresh_active should be true for this pane,
        // so no-agent demotion is blocked.
        let now2 = now + TimeDelta::seconds(1);
        let mut poller_evt = codex_poller_event("e2", "%1", "activity.running", now2);
        // Remove signals to test no-agent streak guard
        poller_evt.payload = serde_json::json!({});
        proj.apply_events(vec![poller_evt], now2);

        let pane = proj.get_pane("%1").expect("pane %1");
        // With pane-first grouping, the resolver state for "%1" knows about
        // det_last_seen from tick 1. deterministic_fresh_active should be true.
        // This means no-agent demotion is blocked even from a heuristic session.
        assert_ne!(
            pane.signature_class,
            PaneSignatureClass::None,
            "deterministic_fresh_active should prevent no-agent demotion \
             when deterministic evidence is still fresh from another session"
        );
    }

    // ── tick_freshness tests ───────────────────────────────────────

    #[test]
    fn tick_freshness_downgrades_stale_pane() {
        let now = Utc::now();
        let mut proj = DaemonProjection::new();

        // Apply a deterministic event at T0
        let det_event = make_event(
            "evt-det",
            agtmux_core_v5::types::Provider::Codex,
            SourceKind::CodexAppserver,
            "sess-1",
            Some("%1"),
            "thread.idle",
            now,
        );
        proj.apply_events(vec![det_event], now);

        let pane = proj.get_pane("%1").expect("pane %1");
        assert_eq!(pane.evidence_mode, EvidenceMode::Deterministic);

        // Advance time past DOWN_THRESHOLD (15s + margin)
        let later = now + TimeDelta::seconds(20);
        let changed = proj.tick_freshness(later);

        assert!(changed > 0, "should have downgraded at least one pane");
        let pane_after = proj.get_pane("%1").expect("pane %1");
        assert_eq!(
            pane_after.evidence_mode,
            EvidenceMode::Heuristic,
            "stale deterministic pane should fall back to heuristic"
        );
    }

    #[test]
    fn tick_freshness_keeps_fresh_pane() {
        let now = Utc::now();
        let mut proj = DaemonProjection::new();

        let det_event = make_event(
            "evt-det",
            agtmux_core_v5::types::Provider::Codex,
            SourceKind::CodexAppserver,
            "sess-1",
            Some("%1"),
            "thread.idle",
            now,
        );
        proj.apply_events(vec![det_event], now);

        // Only 1 second later — still fresh
        let soon = now + TimeDelta::seconds(1);
        let changed = proj.tick_freshness(soon);

        assert_eq!(
            changed, 0,
            "fresh deterministic pane should not be downgraded"
        );
        let pane = proj.get_pane("%1").expect("pane %1");
        assert_eq!(pane.evidence_mode, EvidenceMode::Deterministic);
    }

    // ═══════════════════════════════════════════════════════════════
    // T-123: Provider Switching / Cross-Provider Arbitration Tests
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn codex_to_claude_switch_via_real_activity() {
        let mut proj = DaemonProjection::new();
        let t = t0();
        let t1 = t + TimeDelta::seconds(3);

        // Step 1: Codex is the established active provider
        let codex_real = codex_det_event("c1", "codex-sess", "%1", t);
        proj.apply_events(vec![codex_real], t);
        assert_eq!(
            proj.get_pane("%1").expect("pane %1 must exist").provider,
            Some(agtmux_core_v5::types::Provider::Codex),
        );

        // Step 2: Codex heartbeat + Claude real event arrive in same tick
        let mut codex_hb = codex_det_event("c2", "codex-sess", "%1", t1);
        codex_hb.is_heartbeat = true;
        let claude_real = claude_det_event("cl1", "claude-sess", "%1", t1);

        proj.apply_events(vec![codex_hb, claude_real], t1);

        // Claude real activity is most recent → pane switches to Claude
        assert_eq!(
            proj.get_pane("%1").expect("pane %1 must exist").provider,
            Some(agtmux_core_v5::types::Provider::Claude),
            "pane provider should switch to Claude after Claude real activity"
        );
    }

    #[test]
    fn claude_to_codex_switch_via_real_activity() {
        let mut proj = DaemonProjection::new();
        let t = t0();
        let t1 = t + TimeDelta::seconds(3);

        // Step 1: Claude is the established active provider
        let claude_real = claude_det_event("cl1", "claude-sess", "%1", t);
        proj.apply_events(vec![claude_real], t);
        assert_eq!(
            proj.get_pane("%1").expect("pane %1 must exist").provider,
            Some(agtmux_core_v5::types::Provider::Claude),
        );

        // Step 2: Codex real event + Claude heartbeat arrive in same tick
        let codex_real = codex_det_event("c1", "codex-sess", "%1", t1);
        let mut claude_hb = claude_det_event("cl2", "claude-sess", "%1", t1);
        claude_hb.is_heartbeat = true;

        proj.apply_events(vec![codex_real, claude_hb], t1);

        // Codex real activity is most recent → pane switches to Codex
        assert_eq!(
            proj.get_pane("%1").expect("pane %1 must exist").provider,
            Some(agtmux_core_v5::types::Provider::Codex),
            "pane provider should switch to Codex after Codex real activity"
        );
    }

    #[test]
    fn both_have_real_activity_recency_wins() {
        let mut proj = DaemonProjection::new();
        let t = t0();
        let t1 = t + TimeDelta::seconds(2);
        let t2 = t + TimeDelta::seconds(4);

        // Codex real at t1, Claude real at t2 — both in same tick
        let codex_real = codex_det_event("c1", "codex-sess", "%1", t1);
        let claude_real = claude_det_event("cl1", "claude-sess", "%1", t2);

        proj.apply_events(vec![codex_real, claude_real], t2);

        // Claude has more recent real activity → Claude wins
        assert_eq!(
            proj.get_pane("%1").expect("pane %1 must exist").provider,
            Some(agtmux_core_v5::types::Provider::Claude),
            "provider with more recent real activity should win"
        );
    }

    #[test]
    fn heartbeat_only_no_provider_switch() {
        let mut proj = DaemonProjection::new();
        let t = t0();
        let t1 = t + TimeDelta::seconds(2);

        // Establish Codex as pane owner
        let codex_real = codex_det_event("c1", "codex-sess", "%1", t);
        proj.apply_events(vec![codex_real], t);
        assert_eq!(
            proj.get_pane("%1").expect("pane %1 must exist").provider,
            Some(agtmux_core_v5::types::Provider::Codex),
        );

        // Both Codex and Claude send only heartbeats — no real activity
        let mut codex_hb = codex_det_event("c2", "codex-sess", "%1", t1);
        codex_hb.is_heartbeat = true;
        let mut claude_hb = claude_det_event("cl1", "claude-sess", "%1", t1);
        claude_hb.is_heartbeat = true;

        proj.apply_events(vec![codex_hb, claude_hb], t1);

        // No new real activity: keep Codex (established via last_real_activity from Step 1)
        assert_eq!(
            proj.get_pane("%1").expect("pane %1 must exist").provider,
            Some(agtmux_core_v5::types::Provider::Codex),
            "heartbeat-only tick should not switch provider"
        );
    }

    #[test]
    fn single_provider_no_conflict() {
        let mut proj = DaemonProjection::new();
        let t = t0();

        // Only Codex events — no conflict resolution needed
        let codex_real = codex_det_event("c1", "codex-sess", "%1", t);
        proj.apply_events(vec![codex_real], t);

        let pane = proj.get_pane("%1").expect("pane");
        assert_eq!(
            pane.provider,
            Some(agtmux_core_v5::types::Provider::Codex),
            "single provider should be selected without conflict"
        );
        assert_eq!(pane.evidence_mode, EvidenceMode::Deterministic);
    }

    #[test]
    fn provider_switch_cleanup_on_freshness_timeout() {
        let mut proj = DaemonProjection::new();
        let t = t0();

        // Establish pane with Codex real activity
        let codex_real = codex_det_event("c1", "codex-sess", "%1", t);
        proj.apply_events(vec![codex_real], t);

        // last_real_activity should have Codex entry
        assert!(
            proj.last_real_activity
                .get("%1")
                .and_then(|m| m.get(&agtmux_core_v5::types::Provider::Codex))
                .is_some(),
            "last_real_activity should contain Codex entry after real event"
        );

        // Advance time well past DOWN_THRESHOLD (15s) — pane goes stale
        let stale_time = t + TimeDelta::seconds(20);
        proj.tick_freshness(stale_time);

        // last_real_activity should be cleared for this pane
        assert!(
            proj.last_real_activity.get("%1").is_none(),
            "tick_freshness should clear last_real_activity for stale pane"
        );
    }

    #[test]
    fn heartbeat_events_do_not_update_last_real_activity() {
        let mut proj = DaemonProjection::new();
        let t = t0();
        let t1 = t + TimeDelta::seconds(2);

        // First, establish via real event
        let real = codex_det_event("c1", "codex-sess", "%1", t);
        proj.apply_events(vec![real], t);

        let ts_after_real = *proj
            .last_real_activity
            .get("%1")
            .and_then(|m| m.get(&agtmux_core_v5::types::Provider::Codex))
            .expect("should have Codex entry");

        // Now send a heartbeat — should NOT update last_real_activity
        let mut hb = codex_det_event("c2", "codex-sess", "%1", t1);
        hb.is_heartbeat = true;
        proj.apply_events(vec![hb], t1);

        let ts_after_heartbeat = *proj
            .last_real_activity
            .get("%1")
            .and_then(|m| m.get(&agtmux_core_v5::types::Provider::Codex))
            .expect("Codex entry should still exist");

        assert_eq!(
            ts_after_real, ts_after_heartbeat,
            "heartbeat should not advance last_real_activity timestamp"
        );
    }

    // ── Heartbeat idle-flap tests ────────────────────────────────────

    /// Running→Idle flap at pane level: idle_heartbeat must NOT overwrite Running
    /// from a real (is_heartbeat=false) event received in the same tick.
    #[test]
    fn heartbeat_does_not_flip_pane_running_to_idle() {
        let mut proj = DaemonProjection::new();
        let t = t0();
        let t_hb = t + TimeDelta::milliseconds(200); // heartbeat is newer

        // Real hooks Running event (older but is_heartbeat=false)
        let real = claude_det_event("real-1", "claude-sess", "%1", t);

        // JSONL idle_heartbeat (newer observed_at, is_heartbeat=true, event_type=Idle)
        let mut hb = make_event(
            "hb-1",
            agtmux_core_v5::types::Provider::Claude,
            SourceKind::ClaudeJsonl,
            "claude-sess",
            Some("%1"),
            "activity.idle",
            t_hb,
        );
        hb.is_heartbeat = true;

        // Both events arrive in the same tick (as happens in poll_loop)
        proj.apply_events(vec![real, hb], t_hb);

        let pane = proj.get_pane("%1").expect("pane must exist");
        assert_eq!(
            pane.activity_state,
            ActivityState::Running,
            "idle_heartbeat must not flip Running→Idle at pane level"
        );
    }

    /// Running→Idle flap at session level: idle_heartbeat must NOT overwrite Running.
    #[test]
    fn heartbeat_does_not_flip_session_running_to_idle() {
        let mut proj = DaemonProjection::new();
        let t = t0();
        let t_hb = t + TimeDelta::milliseconds(200);

        let real = claude_det_event("real-1", "claude-sess", "%1", t);
        let mut hb = make_event(
            "hb-1",
            agtmux_core_v5::types::Provider::Claude,
            SourceKind::ClaudeJsonl,
            "claude-sess",
            Some("%1"),
            "activity.idle",
            t_hb,
        );
        hb.is_heartbeat = true;

        proj.apply_events(vec![real, hb], t_hb);

        let session = proj.get_session("claude-sess").expect("session must exist");
        assert_eq!(
            session.activity_state,
            ActivityState::Running,
            "idle_heartbeat must not flip Running→Idle at session level"
        );
    }

    /// A heartbeat on a NEW pane (no prior state) should still set the initial activity.
    #[test]
    fn heartbeat_on_new_pane_sets_initial_activity() {
        let mut proj = DaemonProjection::new();
        let t = t0();

        let mut hb = make_event(
            "hb-1",
            agtmux_core_v5::types::Provider::Codex,
            SourceKind::CodexAppserver,
            "codex-sess",
            Some("%1"),
            "thread.idle",
            t,
        );
        hb.is_heartbeat = true;

        proj.apply_events(vec![hb], t);

        let pane = proj.get_pane("%1").expect("pane must exist");
        assert_eq!(
            pane.activity_state,
            ActivityState::Idle,
            "heartbeat on a new pane should initialize activity_state from event_type"
        );
    }

    /// Real Stop/end events (is_heartbeat=false) must still transition to Idle.
    #[test]
    fn real_stop_event_correctly_sets_idle() {
        let mut proj = DaemonProjection::new();
        let t = t0();
        let t1 = t + TimeDelta::seconds(1);

        // Establish Running
        let running = claude_det_event("r1", "claude-sess", "%1", t);
        proj.apply_events(vec![running], t);
        assert_eq!(
            proj.get_pane("%1").unwrap().activity_state,
            ActivityState::Running
        );

        // Real Stop event (hooks SessionEnd, is_heartbeat=false)
        let stop = make_event(
            "stop-1",
            agtmux_core_v5::types::Provider::Claude,
            SourceKind::ClaudeHooks,
            "claude-sess",
            Some("%1"),
            "lifecycle.stop",
            t1,
        );
        proj.apply_events(vec![stop], t1);

        assert_eq!(
            proj.get_pane("%1").unwrap().activity_state,
            ActivityState::Idle,
            "real stop event (is_heartbeat=false) must transition Running→Idle"
        );
    }

    #[test]
    fn real_events_update_last_real_activity() {
        let mut proj = DaemonProjection::new();
        let t = t0();
        let t1 = t + TimeDelta::seconds(5);

        // First real event at t
        let e1 = codex_det_event("c1", "codex-sess", "%1", t);
        proj.apply_events(vec![e1], t);

        let ts_t0 = *proj
            .last_real_activity
            .get("%1")
            .and_then(|m| m.get(&agtmux_core_v5::types::Provider::Codex))
            .expect("should have Codex entry");
        assert_eq!(ts_t0, t);

        // Second real event at t1
        let e2 = codex_det_event("c2", "codex-sess", "%1", t1);
        proj.apply_events(vec![e2], t1);

        let ts_t1 = *proj
            .last_real_activity
            .get("%1")
            .and_then(|m| m.get(&agtmux_core_v5::types::Provider::Codex))
            .expect("should have Codex entry");
        assert_eq!(ts_t1, t1, "real event should advance last_real_activity");
    }
}

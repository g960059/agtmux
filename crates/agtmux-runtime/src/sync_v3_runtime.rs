use std::collections::{BTreeMap, HashMap, HashSet};

use agtmux_core_v5::resolver::{Freshness as ResolverFreshness, classify_freshness};
use agtmux_core_v5::sync_v3::{
    AgentLifecycleV3, AgentStateV3, AttentionSummaryV3, FreshnessSummaryV3, PendingRequestStatusV3,
    PresenceV3, ProviderRawEnvelopeV3, SyncV3ChangeKindV3, SyncV3CursorV3, SyncV3FieldGroupV3,
    SyncV3PaneChangeV3, SyncV3PaneSnapshot, ThreadBlockingV3, ThreadExecutionV3, ThreadFlagsV3,
    ThreadLifecycleV3, ThreadStateV3, TurnOutcomeV3, TurnStateV3, UiBootstrapV3, UiChangesV3,
};
use agtmux_core_v5::types::{
    FreshnessState, PaneInstanceId, PaneRuntimeState, Provider, SourceEventV2,
};
use agtmux_daemon_v5::claude_v3::apply_claude_source_event;
use agtmux_daemon_v5::codex_v3::apply_codex_source_event;
use agtmux_daemon_v5::sync_v3::{SyncV3Reducer, build_freshness_summary};
use agtmux_tmux_v5::{PaneGenerationTracker, TmuxPaneInfo};
use chrono::{DateTime, Utc};

#[derive(Debug, Default)]
pub struct SyncV3LiveState {
    reducers: BTreeMap<String, SyncV3Reducer>,
    rows: BTreeMap<SyncV3RowKey, SyncV3PaneSnapshot>,
    replay_log: Vec<SyncV3PaneChangeV3>,
    head_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SyncV3RowKey {
    session_name: String,
    window_id: String,
    pane_id: String,
}

impl SyncV3RowKey {
    fn from_tmux_pane(pane: &TmuxPaneInfo) -> Self {
        Self {
            session_name: pane.session_name.clone(),
            window_id: pane.window_id.clone(),
            pane_id: pane.pane_id.clone(),
        }
    }
}

impl SyncV3LiveState {
    pub fn retain_inventory(&mut self, tmux_panes: &[TmuxPaneInfo]) {
        let live_ids = tmux_panes
            .iter()
            .map(|pane| pane.pane_id.as_str())
            .collect::<HashSet<_>>();
        self.reducers
            .retain(|pane_id, _| live_ids.contains(pane_id.as_str()));
    }

    pub fn apply_events(
        &mut self,
        events: &[SourceEventV2],
        tmux_panes: &[TmuxPaneInfo],
        generation_tracker: &PaneGenerationTracker,
    ) {
        let tmux_by_id = tmux_panes
            .iter()
            .map(|pane| (pane.pane_id.as_str(), pane))
            .collect::<HashMap<_, _>>();

        for event in events {
            let Some(pane_id) = event.pane_id.as_deref() else {
                continue;
            };
            let Some(tmux_pane) = tmux_by_id.get(pane_id).copied() else {
                continue;
            };
            let Some(pane_instance_id) = resolve_pane_instance_id(event, generation_tracker) else {
                continue;
            };
            let updated_at = event.actual_activity_at.unwrap_or(event.observed_at);

            if let Some(existing) = self.reducers.get_mut(pane_id)
                && existing.snapshot().provider == Some(event.provider)
            {
                apply_supported_semantics(existing, event);
                continue;
            }

            let mut candidate = SyncV3Reducer::new(new_managed_snapshot(
                tmux_pane,
                pane_instance_id,
                event.provider,
                updated_at,
            ));
            if apply_supported_semantics(&mut candidate, event) {
                self.reducers.insert(pane_id.to_string(), candidate);
            }
        }
    }

    pub fn demote_panes_to_unmanaged(&mut self, pane_ids: &[String]) {
        for pane_id in pane_ids {
            self.reducers.remove(pane_id);
        }
    }

    pub fn reconcile(
        &mut self,
        managed_fallbacks: &[&PaneRuntimeState],
        tmux_panes: &[TmuxPaneInfo],
        generation_tracker: &PaneGenerationTracker,
        now: DateTime<Utc>,
    ) {
        let next_rows = compose_rows(
            managed_fallbacks,
            &self.reducers,
            tmux_panes,
            generation_tracker,
            now,
        );
        let mut changes = Vec::new();

        for (pane_id, previous) in &self.rows {
            if next_rows.contains_key(pane_id) {
                continue;
            }
            self.head_seq += 1;
            changes.push(remove_change(self.head_seq, now, previous));
        }

        for (pane_id, pane) in &next_rows {
            match self.rows.get(pane_id) {
                None => {
                    self.head_seq += 1;
                    changes.push(upsert_change(self.head_seq, now, pane, all_field_groups()));
                }
                Some(previous) if previous != pane => {
                    let field_groups = diff_field_groups(previous, pane);
                    if !field_groups.is_empty() {
                        if requires_row_replacement(&field_groups) {
                            self.head_seq += 1;
                            changes.push(remove_change(self.head_seq, now, previous));
                            self.head_seq += 1;
                            changes.push(upsert_change(
                                self.head_seq,
                                now,
                                pane,
                                all_field_groups(),
                            ));
                        } else {
                            self.head_seq += 1;
                            changes.push(upsert_change(self.head_seq, now, pane, field_groups));
                        }
                    }
                }
                Some(_) => {}
            }
        }

        self.rows = next_rows;
        self.replay_log.extend(changes);
    }

    pub fn current_cursor(&self) -> SyncV3CursorV3 {
        SyncV3CursorV3 { seq: self.head_seq }
    }

    pub fn build_bootstrap(&self, now: DateTime<Utc>) -> UiBootstrapV3 {
        let mut payload = UiBootstrapV3::new(now, self.rows.values().cloned().collect());
        payload.replay_cursor = Some(self.current_cursor());
        payload
    }

    pub fn build_changes(&self, cursor: Option<SyncV3CursorV3>, limit: usize) -> UiChangesV3 {
        let Some(cursor) = cursor else {
            return UiChangesV3::resync_required(self.head_seq, "invalid_cursor");
        };

        if cursor.seq > self.head_seq {
            return UiChangesV3::resync_required(self.head_seq, "ahead_of_head");
        }

        let from_seq = cursor.seq.saturating_add(1);
        let changes = self
            .replay_log
            .iter()
            .filter(|change| change.seq > cursor.seq)
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        let to_seq = changes
            .last()
            .map(|change| change.seq)
            .unwrap_or(cursor.seq);

        UiChangesV3::batch(from_seq, to_seq, SyncV3CursorV3 { seq: to_seq }, changes)
    }
}

fn compose_rows(
    managed_fallbacks: &[&PaneRuntimeState],
    reducers: &BTreeMap<String, SyncV3Reducer>,
    tmux_panes: &[TmuxPaneInfo],
    generation_tracker: &PaneGenerationTracker,
    now: DateTime<Utc>,
) -> BTreeMap<SyncV3RowKey, SyncV3PaneSnapshot> {
    let managed_by_id = managed_fallbacks
        .iter()
        .map(|pane| (pane.pane_instance_id.pane_id.as_str(), *pane))
        .collect::<HashMap<_, _>>();
    let mut inventory = tmux_panes.iter().collect::<Vec<_>>();
    inventory.sort_by(|left, right| {
        (&left.session_name, &left.window_id, &left.pane_id).cmp(&(
            &right.session_name,
            &right.window_id,
            &right.pane_id,
        ))
    });

    let mut rows = BTreeMap::new();
    for tmux_pane in inventory {
        let Some(pane_instance_id) =
            resolve_inventory_pane_instance_id(tmux_pane, generation_tracker)
        else {
            continue;
        };

        let snapshot = if let Some(reducer) = reducers.get(&tmux_pane.pane_id) {
            build_managed_snapshot(reducer.snapshot(), tmux_pane, pane_instance_id, now)
        } else if let Some(managed) = managed_by_id.get(tmux_pane.pane_id.as_str()) {
            build_managed_fallback_snapshot(tmux_pane, pane_instance_id, managed, now)
        } else {
            build_unmanaged_snapshot(tmux_pane, pane_instance_id)
        };

        match snapshot.validate() {
            Ok(()) => {
                rows.insert(SyncV3RowKey::from_tmux_pane(tmux_pane), snapshot);
            }
            Err(err) => {
                tracing::warn!(
                    session_name = %tmux_pane.session_name,
                    window_id = %tmux_pane.window_id,
                    pane_id = %tmux_pane.pane_id,
                    "dropping invalid sync-v3 pane row: {err}"
                );
            }
        }
    }

    rows
}

fn upsert_change(
    seq: u64,
    at: DateTime<Utc>,
    pane: &SyncV3PaneSnapshot,
    field_groups: Vec<SyncV3FieldGroupV3>,
) -> SyncV3PaneChangeV3 {
    SyncV3PaneChangeV3 {
        seq,
        at,
        kind: SyncV3ChangeKindV3::Upsert,
        pane_id: pane.pane_id.clone(),
        session_name: pane.session_name.clone(),
        window_id: pane.window_id.clone(),
        session_key: pane.session_key.clone(),
        pane_instance_id: pane.pane_instance_id.clone(),
        field_groups,
        pane: Some(pane.clone()),
    }
}

fn remove_change(seq: u64, at: DateTime<Utc>, pane: &SyncV3PaneSnapshot) -> SyncV3PaneChangeV3 {
    SyncV3PaneChangeV3 {
        seq,
        at,
        kind: SyncV3ChangeKindV3::Remove,
        pane_id: pane.pane_id.clone(),
        session_name: pane.session_name.clone(),
        window_id: pane.window_id.clone(),
        session_key: pane.session_key.clone(),
        pane_instance_id: pane.pane_instance_id.clone(),
        field_groups: vec![SyncV3FieldGroupV3::Identity],
        pane: None,
    }
}

fn all_field_groups() -> Vec<SyncV3FieldGroupV3> {
    vec![
        SyncV3FieldGroupV3::Identity,
        SyncV3FieldGroupV3::Presence,
        SyncV3FieldGroupV3::Provider,
        SyncV3FieldGroupV3::Agent,
        SyncV3FieldGroupV3::Thread,
        SyncV3FieldGroupV3::PendingRequests,
        SyncV3FieldGroupV3::Attention,
        SyncV3FieldGroupV3::Freshness,
        SyncV3FieldGroupV3::ProviderRaw,
    ]
}

fn requires_row_replacement(field_groups: &[SyncV3FieldGroupV3]) -> bool {
    field_groups
        .iter()
        .any(|group| matches!(group, SyncV3FieldGroupV3::Identity))
}

fn diff_field_groups(
    previous: &SyncV3PaneSnapshot,
    next: &SyncV3PaneSnapshot,
) -> Vec<SyncV3FieldGroupV3> {
    let mut groups = Vec::new();

    if previous.session_name != next.session_name
        || previous.window_id != next.window_id
        || previous.session_key != next.session_key
        || previous.pane_id != next.pane_id
        || previous.pane_instance_id != next.pane_instance_id
    {
        groups.push(SyncV3FieldGroupV3::Identity);
    }
    if previous.presence != next.presence {
        groups.push(SyncV3FieldGroupV3::Presence);
    }
    if previous.provider != next.provider {
        groups.push(SyncV3FieldGroupV3::Provider);
    }
    if previous.agent != next.agent {
        groups.push(SyncV3FieldGroupV3::Agent);
    }
    if previous.thread != next.thread {
        groups.push(SyncV3FieldGroupV3::Thread);
    }
    if previous.pending_requests != next.pending_requests {
        groups.push(SyncV3FieldGroupV3::PendingRequests);
    }
    if previous.attention != next.attention {
        groups.push(SyncV3FieldGroupV3::Attention);
    }
    if previous.freshness != next.freshness || previous.updated_at != next.updated_at {
        groups.push(SyncV3FieldGroupV3::Freshness);
    }
    if previous.provider_raw != next.provider_raw {
        groups.push(SyncV3FieldGroupV3::ProviderRaw);
    }

    groups
}

fn apply_supported_semantics(reducer: &mut SyncV3Reducer, event: &SourceEventV2) -> bool {
    match event.provider {
        Provider::Codex => apply_codex_source_event(reducer, event),
        Provider::Claude => apply_claude_source_event(reducer, event),
        _ => false,
    }
}

fn resolve_pane_instance_id(
    event: &SourceEventV2,
    generation_tracker: &PaneGenerationTracker,
) -> Option<PaneInstanceId> {
    let pane_id = event.pane_id.as_ref()?;
    let (generation, birth_ts) = match (event.pane_generation, event.pane_birth_ts) {
        (Some(generation), Some(birth_ts)) => (generation, birth_ts),
        _ => generation_tracker.get(pane_id)?,
    };

    Some(PaneInstanceId {
        pane_id: pane_id.clone(),
        generation,
        birth_ts,
    })
}

fn resolve_inventory_pane_instance_id(
    tmux_pane: &TmuxPaneInfo,
    generation_tracker: &PaneGenerationTracker,
) -> Option<PaneInstanceId> {
    if tmux_pane.pane_id.is_empty()
        || tmux_pane.session_name.is_empty()
        || tmux_pane.window_id.is_empty()
    {
        return None;
    }

    let (generation, birth_ts) = generation_tracker.get(&tmux_pane.pane_id)?;
    Some(PaneInstanceId {
        pane_id: tmux_pane.pane_id.clone(),
        generation,
        birth_ts,
    })
}

fn new_managed_snapshot(
    tmux_pane: &TmuxPaneInfo,
    pane_instance_id: PaneInstanceId,
    provider: Provider,
    updated_at: DateTime<Utc>,
) -> SyncV3PaneSnapshot {
    SyncV3PaneSnapshot {
        session_name: tmux_pane.session_name.clone(),
        window_id: tmux_pane.window_id.clone(),
        session_key: managed_session_key(provider, &tmux_pane.pane_id),
        pane_id: tmux_pane.pane_id.clone(),
        pane_instance_id,
        provider: Some(provider),
        presence: PresenceV3::Managed,
        agent: AgentStateV3 {
            lifecycle: AgentLifecycleV3::Unknown,
        },
        thread: ThreadStateV3 {
            lifecycle: ThreadLifecycleV3::NotLoaded,
            blocking: ThreadBlockingV3::None,
            execution: ThreadExecutionV3::None,
            flags: ThreadFlagsV3::default(),
            turn: empty_turn(),
        },
        pending_requests: Vec::new(),
        attention: AttentionSummaryV3::none(),
        freshness: FreshnessSummaryV3::fresh(),
        provider_raw: ProviderRawEnvelopeV3::default(),
        updated_at,
    }
}

fn build_managed_snapshot(
    snapshot: &SyncV3PaneSnapshot,
    tmux_pane: &TmuxPaneInfo,
    pane_instance_id: PaneInstanceId,
    now: DateTime<Utc>,
) -> SyncV3PaneSnapshot {
    let mut pane = snapshot.clone();
    pane.session_name = tmux_pane.session_name.clone();
    pane.window_id = tmux_pane.window_id.clone();
    pane.session_key = snapshot
        .provider
        .map(|provider| managed_session_key(provider, &tmux_pane.pane_id))
        .unwrap_or_else(|| format!("managed:{}", tmux_pane.pane_id));
    pane.pane_id = tmux_pane.pane_id.clone();
    pane.pane_instance_id = pane_instance_id;
    pane.presence = PresenceV3::Managed;
    pane.freshness = freshness_for_managed_snapshot(&pane, now);
    pane.pending_requests
        .retain(|request| matches!(request.status, PendingRequestStatusV3::Pending));
    pane
}

fn build_managed_fallback_snapshot(
    tmux_pane: &TmuxPaneInfo,
    pane_instance_id: PaneInstanceId,
    managed: &PaneRuntimeState,
    now: DateTime<Utc>,
) -> SyncV3PaneSnapshot {
    SyncV3PaneSnapshot {
        session_name: tmux_pane.session_name.clone(),
        window_id: tmux_pane.window_id.clone(),
        session_key: managed
            .provider
            .map(|provider| managed_session_key(provider, &tmux_pane.pane_id))
            .unwrap_or_else(|| format!("managed:{}", tmux_pane.pane_id)),
        pane_id: tmux_pane.pane_id.clone(),
        pane_instance_id,
        provider: managed.provider,
        presence: PresenceV3::Managed,
        agent: AgentStateV3 {
            lifecycle: AgentLifecycleV3::Unknown,
        },
        thread: ThreadStateV3 {
            lifecycle: ThreadLifecycleV3::NotLoaded,
            blocking: ThreadBlockingV3::None,
            execution: ThreadExecutionV3::None,
            flags: ThreadFlagsV3::default(),
            turn: empty_turn(),
        },
        pending_requests: Vec::new(),
        attention: AttentionSummaryV3::none(),
        freshness: freshness_from_updated_at(managed.updated_at, now),
        provider_raw: ProviderRawEnvelopeV3::default(),
        updated_at: managed.updated_at,
    }
}

fn build_unmanaged_snapshot(
    tmux_pane: &TmuxPaneInfo,
    pane_instance_id: PaneInstanceId,
) -> SyncV3PaneSnapshot {
    let updated_at = pane_instance_id.birth_ts;

    SyncV3PaneSnapshot {
        session_name: tmux_pane.session_name.clone(),
        window_id: tmux_pane.window_id.clone(),
        session_key: unmanaged_session_key(&tmux_pane.pane_id),
        pane_id: tmux_pane.pane_id.clone(),
        pane_instance_id,
        provider: None,
        presence: PresenceV3::Unmanaged,
        agent: AgentStateV3 {
            lifecycle: AgentLifecycleV3::Unknown,
        },
        thread: ThreadStateV3 {
            lifecycle: ThreadLifecycleV3::Idle,
            blocking: ThreadBlockingV3::None,
            execution: ThreadExecutionV3::None,
            flags: ThreadFlagsV3::default(),
            turn: empty_turn(),
        },
        pending_requests: Vec::new(),
        attention: AttentionSummaryV3::none(),
        freshness: build_freshness_summary(FreshnessState::Down, FreshnessState::Down, None),
        provider_raw: ProviderRawEnvelopeV3::default(),
        updated_at,
    }
}

fn empty_turn() -> TurnStateV3 {
    TurnStateV3 {
        outcome: TurnOutcomeV3::None,
        sequence: None,
        started_at: None,
        completed_at: None,
    }
}

fn managed_session_key(provider: Provider, pane_id: &str) -> String {
    format!("{}:{pane_id}", provider.as_str())
}

fn unmanaged_session_key(pane_id: &str) -> String {
    format!("shell:{pane_id}")
}

fn freshness_from_updated_at(updated_at: DateTime<Utc>, now: DateTime<Utc>) -> FreshnessSummaryV3 {
    let freshness = match classify_freshness(Some(updated_at), now) {
        ResolverFreshness::Fresh => FreshnessState::Fresh,
        ResolverFreshness::Stale => FreshnessState::Stale,
        ResolverFreshness::Down => FreshnessState::Down,
    };
    build_freshness_summary(freshness, freshness, None)
}

fn freshness_for_managed_snapshot(
    snapshot: &SyncV3PaneSnapshot,
    now: DateTime<Utc>,
) -> FreshnessSummaryV3 {
    if matches!(
        snapshot.thread.lifecycle,
        ThreadLifecycleV3::Idle | ThreadLifecycleV3::Interrupted | ThreadLifecycleV3::Errored
    ) {
        snapshot.freshness.clone()
    } else {
        freshness_from_updated_at(snapshot.updated_at, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agtmux_core_v5::sync_v3::{
        SyncV3ChangeKindV3, SyncV3FieldGroupV3, ThreadBlockingV3, ThreadLifecycleV3, TurnOutcomeV3,
    };
    use agtmux_core_v5::types::{
        ActivityState, EvidenceTier, PanePresence, PaneSignatureClass, SignatureInputsCompact,
        SourceKind,
    };
    use chrono::TimeZone;

    fn ts(second: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 3, 9, 23, 0, second)
            .single()
            .expect("valid timestamp")
    }

    fn tmux_pane(
        pane_id: &str,
        session_name: &str,
        window_id: &str,
        current_cmd: &str,
    ) -> TmuxPaneInfo {
        TmuxPaneInfo {
            pane_id: pane_id.to_string(),
            session_name: session_name.to_string(),
            window_id: window_id.to_string(),
            window_name: "main".to_string(),
            current_cmd: current_cmd.to_string(),
            ..Default::default()
        }
    }

    fn codex_event(inner_type: &str, pane_id: &str, observed_at: DateTime<Utc>) -> SourceEventV2 {
        codex_event_with_activity_state(ActivityState::Unknown, inner_type, pane_id, observed_at)
    }

    fn codex_event_with_activity_state(
        activity_state: ActivityState,
        inner_type: &str,
        pane_id: &str,
        observed_at: DateTime<Utc>,
    ) -> SourceEventV2 {
        SourceEventV2 {
            event_id: format!("codex-{inner_type}-{}", observed_at.timestamp()),
            provider: Provider::Codex,
            source_kind: SourceKind::CodexJsonl,
            tier: EvidenceTier::Deterministic,
            observed_at,
            session_key: "thr-codex-1".to_string(),
            pane_id: Some(pane_id.to_string()),
            pane_generation: None,
            pane_birth_ts: None,
            source_event_id: None,
            activity_state,
            payload: serde_json::json!({
                "codex_jsonl": {
                    "top_type": "event_msg",
                    "inner_type": inner_type,
                    "bootstrap": false
                }
            }),
            confidence: 1.0,
            is_heartbeat: false,
            actual_activity_at: Some(observed_at),
        }
    }

    #[test]
    fn build_bootstrap_uses_codex_v3_truth_instead_of_waiting_input_collapse() {
        let mut live = SyncV3LiveState::default();
        let pane = tmux_pane("%12", "workbench", "@5", "codex");
        let panes = vec![pane];
        let mut tracker = PaneGenerationTracker::new();
        tracker.update(&["%12"], ts(0));

        live.apply_events(
            &[codex_event("task_complete", "%12", ts(10))],
            &panes,
            &tracker,
        );
        live.reconcile(&[], &panes, &tracker, ts(11));

        let payload = live.build_bootstrap(ts(11));
        payload.validate().expect("payload should validate");

        assert_eq!(payload.version, 3);
        assert_eq!(payload.replay_cursor, Some(SyncV3CursorV3 { seq: 1 }));
        assert_eq!(payload.panes.len(), 1);
        let pane = &payload.panes[0];
        assert_eq!(pane.session_key, "codex:%12");
        assert_eq!(pane.presence, PresenceV3::Managed);
        assert_eq!(pane.thread.lifecycle, ThreadLifecycleV3::Idle);
        assert_eq!(pane.thread.blocking, ThreadBlockingV3::None);
        assert_eq!(pane.thread.turn.outcome, TurnOutcomeV3::Completed);
    }

    #[test]
    fn build_bootstrap_ignores_legacy_event_type_when_codex_payload_has_v3_truth() {
        let mut live = SyncV3LiveState::default();
        let pane = tmux_pane("%18", "workbench", "@8", "codex");
        let panes = vec![pane];
        let mut tracker = PaneGenerationTracker::new();
        tracker.update(&["%18"], ts(0));

        let event =
            codex_event_with_activity_state(ActivityState::Running, "task_complete", "%18", ts(10));

        live.apply_events(&[event], &panes, &tracker);
        live.reconcile(&[], &panes, &tracker, ts(11));

        let payload = live.build_bootstrap(ts(11));
        payload.validate().expect("payload should validate");

        let pane = &payload.panes[0];
        assert_eq!(pane.thread.lifecycle, ThreadLifecycleV3::Idle);
        assert_eq!(pane.thread.execution, ThreadExecutionV3::None);
        assert_eq!(pane.thread.turn.outcome, TurnOutcomeV3::Completed);
    }

    #[test]
    fn build_bootstrap_emits_unmanaged_row_with_strict_identity_when_no_v3_truth_exists() {
        let mut live = SyncV3LiveState::default();
        let pane = tmux_pane("%45", "shells", "@9", "zsh");
        let panes = vec![pane];
        let mut tracker = PaneGenerationTracker::new();
        tracker.update(&["%45"], ts(0));
        live.reconcile(&[], &panes, &tracker, ts(12));

        let payload = live.build_bootstrap(ts(12));
        payload.validate().expect("payload should validate");

        assert_eq!(payload.panes.len(), 1);
        let pane = &payload.panes[0];
        assert_eq!(pane.session_key, "shell:%45");
        assert_eq!(pane.presence, PresenceV3::Unmanaged);
        assert!(pane.provider.is_none());
        assert_eq!(pane.thread.lifecycle, ThreadLifecycleV3::Idle);
        assert_eq!(pane.freshness.snapshot, FreshnessState::Down);
        assert_eq!(pane.updated_at, ts(0));
    }

    #[test]
    fn build_bootstrap_preserves_linked_session_locations_for_same_pane_id() {
        let mut live = SyncV3LiveState::default();
        let panes = vec![
            tmux_pane("%52", "linked", "@9", "node"),
            tmux_pane("%52", "primary", "@9", "node"),
        ];
        let mut tracker = PaneGenerationTracker::new();
        tracker.update(&["%52"], ts(0));

        live.apply_events(
            &[codex_event("task_started", "%52", ts(10))],
            &panes,
            &tracker,
        );
        live.reconcile(&[], &panes, &tracker, ts(11));

        let payload = live.build_bootstrap(ts(11));
        payload.validate().expect("payload should validate");

        assert_eq!(payload.panes.len(), 2);
        let primary = payload
            .panes
            .iter()
            .find(|pane| pane.session_name == "primary")
            .expect("primary linked row");
        let linked = payload
            .panes
            .iter()
            .find(|pane| pane.session_name == "linked")
            .expect("linked row");

        for pane in [primary, linked] {
            assert_eq!(pane.window_id, "@9");
            assert_eq!(pane.pane_id, "%52");
            assert_eq!(pane.pane_instance_id.pane_id, "%52");
            assert_eq!(pane.session_key, "codex:%52");
            assert_eq!(pane.presence, PresenceV3::Managed);
            assert_eq!(pane.provider, Some(Provider::Codex));
            assert_eq!(pane.thread.lifecycle, ThreadLifecycleV3::Active);
        }
    }

    #[test]
    fn reconcile_removes_only_missing_linked_session_location() {
        let mut live = SyncV3LiveState::default();
        let panes = vec![
            tmux_pane("%52", "linked", "@9", "node"),
            tmux_pane("%52", "primary", "@9", "node"),
        ];
        let mut tracker = PaneGenerationTracker::new();
        tracker.update(&["%52"], ts(0));

        live.apply_events(
            &[codex_event("task_started", "%52", ts(10))],
            &panes,
            &tracker,
        );
        live.reconcile(&[], &panes, &tracker, ts(11));

        let remaining = vec![tmux_pane("%52", "primary", "@9", "node")];
        live.reconcile(&[], &remaining, &tracker, ts(12));

        let payload = live.build_changes(Some(SyncV3CursorV3 { seq: 2 }), 10);
        payload.validate().expect("changes should validate");

        assert_eq!(payload.changes.len(), 1);
        let change = &payload.changes[0];
        assert_eq!(change.kind, SyncV3ChangeKindV3::Remove);
        assert_eq!(change.pane_id, "%52");
        assert_eq!(change.session_name, "linked");
        assert_eq!(change.window_id, "@9");
        assert_eq!(change.session_key, "codex:%52");
    }

    #[test]
    fn managed_fallback_does_not_reuse_collapsed_v2_activity_state() {
        let mut live = SyncV3LiveState::default();
        let pane = tmux_pane("%34", "review", "@7", "claude");
        let panes = vec![pane];
        let mut tracker = PaneGenerationTracker::new();
        tracker.update(&["%34"], ts(0));

        let managed = PaneRuntimeState {
            pane_instance_id: PaneInstanceId {
                pane_id: "%34".to_string(),
                generation: 2,
                birth_ts: ts(0),
            },
            presence: PanePresence::Managed,
            evidence_mode: agtmux_core_v5::types::EvidenceMode::Deterministic,
            signature_class: PaneSignatureClass::Deterministic,
            signature_reason: "deterministic".to_string(),
            signature_confidence: 1.0,
            no_agent_streak: 0,
            signature_inputs: SignatureInputsCompact::default(),
            activity_state: agtmux_core_v5::types::ActivityState::WaitingInput,
            provider: Some(Provider::Claude),
            session_key: "poller-%34".to_string(),
            updated_at: ts(8),
        };

        live.reconcile(&[&managed], &panes, &tracker, ts(12));

        let payload = live.build_bootstrap(ts(12));
        payload.validate().expect("payload should validate");

        let pane = &payload.panes[0];
        assert_eq!(pane.session_key, "claude:%34");
        assert_eq!(pane.presence, PresenceV3::Managed);
        assert_eq!(pane.agent.lifecycle, AgentLifecycleV3::Unknown);
        assert_eq!(pane.thread.lifecycle, ThreadLifecycleV3::NotLoaded);
        assert_eq!(pane.thread.blocking, ThreadBlockingV3::None);
        assert_eq!(pane.freshness.snapshot, FreshnessState::Stale);
        assert_eq!(pane.freshness.blocking, FreshnessState::Stale);
        assert_eq!(pane.freshness.execution, FreshnessState::Stale);
    }

    #[test]
    fn managed_fallback_freshness_tracks_projection_updated_at() {
        let mut live = SyncV3LiveState::default();
        let pane = tmux_pane("%77", "codex", "@3", "node");
        let panes = vec![pane];
        let mut tracker = PaneGenerationTracker::new();
        tracker.update(&["%77"], ts(0));

        let managed = PaneRuntimeState {
            pane_instance_id: PaneInstanceId {
                pane_id: "%77".to_string(),
                generation: 2,
                birth_ts: ts(0),
            },
            presence: PanePresence::Managed,
            evidence_mode: agtmux_core_v5::types::EvidenceMode::Deterministic,
            signature_class: PaneSignatureClass::Deterministic,
            signature_reason: "deterministic".to_string(),
            signature_confidence: 1.0,
            no_agent_streak: 0,
            signature_inputs: SignatureInputsCompact::default(),
            activity_state: agtmux_core_v5::types::ActivityState::Running,
            provider: Some(Provider::Codex),
            session_key: "poller-%77".to_string(),
            updated_at: ts(10),
        };

        live.reconcile(&[&managed], &panes, &tracker, ts(11));
        let fresh_payload = live.build_bootstrap(ts(11));
        let fresh_pane = &fresh_payload.panes[0];
        assert_eq!(fresh_pane.thread.lifecycle, ThreadLifecycleV3::NotLoaded);
        assert_eq!(fresh_pane.freshness.snapshot, FreshnessState::Fresh);

        live.reconcile(&[&managed], &panes, &tracker, ts(26));
        let down_payload = live.build_bootstrap(ts(26));
        let down_pane = &down_payload.panes[0];
        assert_eq!(down_pane.thread.lifecycle, ThreadLifecycleV3::NotLoaded);
        assert_eq!(down_pane.freshness.snapshot, FreshnessState::Down);
    }

    #[test]
    fn settled_managed_rows_preserve_freshness_after_fifteen_seconds() {
        let pane = tmux_pane("%88", "workbench", "@8", "codex");
        let pane_instance_id = PaneInstanceId {
            pane_id: "%88".to_string(),
            generation: 3,
            birth_ts: ts(0),
        };

        for lifecycle in [
            ThreadLifecycleV3::Idle,
            ThreadLifecycleV3::Interrupted,
            ThreadLifecycleV3::Errored,
        ] {
            let snapshot = SyncV3PaneSnapshot {
                session_name: "workbench".to_string(),
                window_id: "@8".to_string(),
                session_key: "codex:%88".to_string(),
                pane_id: "%88".to_string(),
                pane_instance_id: pane_instance_id.clone(),
                provider: Some(Provider::Codex),
                presence: PresenceV3::Managed,
                agent: AgentStateV3 {
                    lifecycle: AgentLifecycleV3::Completed,
                },
                thread: ThreadStateV3 {
                    lifecycle,
                    blocking: ThreadBlockingV3::None,
                    execution: ThreadExecutionV3::None,
                    flags: ThreadFlagsV3::default(),
                    turn: TurnStateV3 {
                        outcome: TurnOutcomeV3::Completed,
                        sequence: Some(7),
                        started_at: Some(ts(1)),
                        completed_at: Some(ts(10)),
                    },
                },
                pending_requests: Vec::new(),
                attention: AttentionSummaryV3::none(),
                freshness: FreshnessSummaryV3::fresh(),
                provider_raw: ProviderRawEnvelopeV3::default(),
                updated_at: ts(10),
            };

            let pane = build_managed_snapshot(&snapshot, &pane, pane_instance_id.clone(), ts(30));
            assert_eq!(pane.thread.lifecycle, lifecycle);
            assert_eq!(pane.freshness.snapshot, FreshnessState::Fresh);
            assert_eq!(pane.freshness.blocking, FreshnessState::Fresh);
            assert_eq!(pane.freshness.execution, FreshnessState::Fresh);
        }
    }

    #[test]
    fn build_changes_replaces_row_when_claude_promotion_changes_exact_identity() {
        let mut live = SyncV3LiveState::default();
        let panes = vec![tmux_pane("%34", "review", "@7", "claude")];
        let mut tracker = PaneGenerationTracker::new();
        tracker.update(&["%34"], ts(0));

        live.reconcile(&[], &panes, &tracker, ts(1));

        let managed = PaneRuntimeState {
            pane_instance_id: PaneInstanceId {
                pane_id: "%34".to_string(),
                generation: 2,
                birth_ts: ts(0),
            },
            presence: PanePresence::Managed,
            evidence_mode: agtmux_core_v5::types::EvidenceMode::Deterministic,
            signature_class: PaneSignatureClass::Deterministic,
            signature_reason: "deterministic".to_string(),
            signature_confidence: 1.0,
            no_agent_streak: 0,
            signature_inputs: SignatureInputsCompact::default(),
            activity_state: agtmux_core_v5::types::ActivityState::Idle,
            provider: Some(Provider::Claude),
            session_key: "thr-claude-1".to_string(),
            updated_at: ts(10),
        };

        live.reconcile(&[&managed], &panes, &tracker, ts(11));

        let payload = live.build_changes(Some(SyncV3CursorV3 { seq: 1 }), 10);
        payload.validate().expect("changes should validate");

        assert_eq!(payload.changes.len(), 2);
        let remove = &payload.changes[0];
        assert_eq!(remove.kind, SyncV3ChangeKindV3::Remove);
        assert_eq!(remove.session_name, "review");
        assert_eq!(remove.window_id, "@7");
        assert_eq!(remove.pane_id, "%34");
        assert_eq!(remove.session_key, "shell:%34");

        let upsert = &payload.changes[1];
        assert_eq!(upsert.kind, SyncV3ChangeKindV3::Upsert);
        assert_eq!(upsert.session_name, "review");
        assert_eq!(upsert.window_id, "@7");
        assert_eq!(upsert.pane_id, "%34");
        assert_eq!(upsert.session_key, "claude:%34");
        assert_eq!(upsert.pane_instance_id, remove.pane_instance_id);
        let pane = upsert.pane.as_ref().expect("replacement pane");
        assert_eq!(pane.provider, Some(Provider::Claude));
        assert_eq!(pane.presence, PresenceV3::Managed);
        assert_eq!(pane.session_key, "claude:%34");
    }

    #[test]
    fn build_changes_emits_upsert_with_field_groups_and_completed_truth() {
        let mut live = SyncV3LiveState::default();
        let panes = vec![tmux_pane("%12", "workbench", "@5", "codex")];
        let mut tracker = PaneGenerationTracker::new();
        tracker.update(&["%12"], ts(0));

        live.apply_events(
            &[codex_event("task_complete", "%12", ts(10))],
            &panes,
            &tracker,
        );
        live.reconcile(&[], &panes, &tracker, ts(11));

        let payload = live.build_changes(Some(SyncV3CursorV3 { seq: 0 }), 10);
        payload.validate().expect("changes should validate");

        assert_eq!(payload.from_seq, Some(1));
        assert_eq!(payload.to_seq, Some(1));
        assert_eq!(payload.next_cursor, Some(SyncV3CursorV3 { seq: 1 }));
        assert_eq!(payload.changes.len(), 1);

        let change = &payload.changes[0];
        assert_eq!(change.kind, SyncV3ChangeKindV3::Upsert);
        assert_eq!(change.pane_id, "%12");
        assert!(
            change.field_groups.contains(&SyncV3FieldGroupV3::Thread),
            "initial upsert should identify thread group"
        );
        let pane = change.pane.as_ref().expect("upsert pane");
        assert_eq!(pane.thread.lifecycle, ThreadLifecycleV3::Idle);
        assert_eq!(pane.thread.turn.outcome, TurnOutcomeV3::Completed);
    }

    #[test]
    fn build_changes_replaces_row_when_exact_identity_changes_at_same_location() {
        let mut live = SyncV3LiveState::default();
        let panes = vec![tmux_pane("%12", "workbench", "@5", "node")];
        let mut tracker = PaneGenerationTracker::new();
        tracker.update(&["%12"], ts(0));

        live.reconcile(&[], &panes, &tracker, ts(1));
        let shell_row = live.build_bootstrap(ts(1)).panes[0].clone();
        live.apply_events(
            &[codex_event("task_started", "%12", ts(10))],
            &panes,
            &tracker,
        );
        live.reconcile(&[], &panes, &tracker, ts(11));
        let managed_row = live.build_bootstrap(ts(11)).panes[0].clone();

        let payload = live.build_changes(Some(SyncV3CursorV3 { seq: 1 }), 10);
        payload.validate().expect("changes should validate");

        let buggy_field_groups = diff_field_groups(&shell_row, &managed_row);
        assert!(
            buggy_field_groups.contains(&SyncV3FieldGroupV3::Identity),
            "old buggy in-place promotion would have advertised identity churn in one upsert"
        );
        let buggy_in_place_upsert =
            upsert_change(2, ts(11), &managed_row, buggy_field_groups.clone());
        assert_eq!(buggy_in_place_upsert.session_name, "workbench");
        assert_eq!(buggy_in_place_upsert.window_id, "@5");
        assert_eq!(buggy_in_place_upsert.pane_id, "%12");
        assert_eq!(buggy_in_place_upsert.session_key, "codex:%12");
        assert_eq!(
            buggy_in_place_upsert.pane_instance_id, shell_row.pane_instance_id,
            "the old shape changed exact identity without first removing the shell row"
        );

        assert_eq!(payload.changes.len(), 2);
        let remove = &payload.changes[0];
        assert_eq!(remove.kind, SyncV3ChangeKindV3::Remove);
        assert_eq!(remove.session_name, "workbench");
        assert_eq!(remove.window_id, "@5");
        assert_eq!(remove.pane_id, "%12");
        assert_eq!(remove.session_key, "shell:%12");
        assert!(remove.pane.is_none());

        let upsert = &payload.changes[1];
        assert_eq!(upsert.kind, SyncV3ChangeKindV3::Upsert);
        assert_eq!(upsert.session_name, "workbench");
        assert_eq!(upsert.window_id, "@5");
        assert_eq!(upsert.pane_id, "%12");
        assert_eq!(upsert.session_key, "codex:%12");
        assert_eq!(
            upsert.field_groups,
            all_field_groups(),
            "replacement upsert should describe the full new row"
        );
        let pane = upsert.pane.as_ref().expect("replacement pane");
        assert_eq!(pane.pane_instance_id.pane_id, "%12");
        assert_eq!(pane.session_key, "codex:%12");
        assert_eq!(pane.presence, PresenceV3::Managed);
        assert_ne!(
            payload.changes[1], buggy_in_place_upsert,
            "runtime must replace exact identity instead of emitting the old conflicting upsert"
        );
    }

    #[test]
    fn reconcile_emits_remove_when_pane_leaves_inventory() {
        let mut live = SyncV3LiveState::default();
        let panes = vec![tmux_pane("%12", "workbench", "@5", "codex")];
        let mut tracker = PaneGenerationTracker::new();
        tracker.update(&["%12"], ts(0));

        live.apply_events(
            &[codex_event("task_complete", "%12", ts(10))],
            &panes,
            &tracker,
        );
        live.reconcile(&[], &panes, &tracker, ts(11));

        live.retain_inventory(&[]);
        live.reconcile(&[], &[], &tracker, ts(12));

        let payload = live.build_changes(Some(SyncV3CursorV3 { seq: 1 }), 10);
        payload.validate().expect("remove change should validate");

        assert_eq!(payload.changes.len(), 1);
        let change = &payload.changes[0];
        assert_eq!(change.kind, SyncV3ChangeKindV3::Remove);
        assert_eq!(change.pane_id, "%12");
        assert_eq!(change.session_key, "codex:%12");
        assert_eq!(change.field_groups, vec![SyncV3FieldGroupV3::Identity]);
        assert!(change.pane.is_none());
    }

    #[test]
    fn build_changes_resyncs_on_ahead_of_head_cursor() {
        let live = SyncV3LiveState::default();
        let payload = live.build_changes(Some(SyncV3CursorV3 { seq: 99 }), 10);

        payload.validate().expect("resync response should validate");
        assert_eq!(
            payload
                .resync_required
                .as_ref()
                .expect("resync_required")
                .reason,
            "ahead_of_head"
        );
    }
}

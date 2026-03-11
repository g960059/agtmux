use std::collections::BTreeMap;

use agtmux_core_v5::sync_v3::{
    AgentLifecycleV3, AttentionKindV3, AttentionPriorityV3, AttentionSummaryV3, FreshnessSummaryV3,
    PendingRequestKindV3, PendingRequestStatusV3, PendingRequestV3, SyncV3PaneSnapshot,
    ThreadBlockingV3, ThreadExecutionV3, ThreadLifecycleV3, TurnOutcomeV3, TurnStateV3,
};
use agtmux_core_v5::types::FreshnessState;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Default)]
pub struct RequestTableV3 {
    entries: BTreeMap<String, PendingRequestV3>,
}

impl RequestTableV3 {
    pub fn upsert(&mut self, request: PendingRequestV3) -> bool {
        let key = request.request_id.clone();
        let changed = self.entries.get(&key) != Some(&request);
        self.entries.insert(key, request);
        changed
    }

    pub fn update_status(
        &mut self,
        request_id: &str,
        status: PendingRequestStatusV3,
        updated_at: DateTime<Utc>,
    ) -> bool {
        let Some(existing) = self.entries.get_mut(request_id) else {
            return false;
        };

        if existing.status == status && existing.updated_at == updated_at {
            return false;
        }

        existing.status = status;
        existing.updated_at = updated_at;
        true
    }

    pub fn pending(&self) -> Vec<PendingRequestV3> {
        let mut rows = self
            .entries
            .values()
            .filter(|request| matches!(request.status, PendingRequestStatusV3::Pending))
            .cloned()
            .collect::<Vec<_>>();
        rows.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.request_id.cmp(&right.request_id))
        });
        rows
    }
}

pub fn build_freshness_summary(
    blocking: FreshnessState,
    execution: FreshnessState,
    snapshot_override: Option<FreshnessState>,
) -> FreshnessSummaryV3 {
    let derived_snapshot = worst_freshness(blocking, execution);
    let snapshot = snapshot_override
        .map(|override_level| worst_freshness(derived_snapshot, override_level))
        .unwrap_or(derived_snapshot);

    FreshnessSummaryV3 {
        snapshot,
        blocking,
        execution,
    }
}

pub fn build_attention_summary(
    pending_requests: &[PendingRequestV3],
    agent_lifecycle: AgentLifecycleV3,
    thread_lifecycle: ThreadLifecycleV3,
    turn: &TurnStateV3,
    updated_at: DateTime<Utc>,
    generation: u64,
) -> AttentionSummaryV3 {
    let has_error = matches!(agent_lifecycle, AgentLifecycleV3::Errored)
        || matches!(thread_lifecycle, ThreadLifecycleV3::Errored)
        || matches!(turn.outcome, TurnOutcomeV3::Errored);
    let has_approval = pending_requests
        .iter()
        .any(|request| matches!(request.kind, PendingRequestKindV3::Approval));
    let has_question = pending_requests
        .iter()
        .any(|request| matches!(request.kind, PendingRequestKindV3::UserInput));
    let has_completion = matches!(turn.outcome, TurnOutcomeV3::Completed)
        && !has_error
        && pending_requests.is_empty();

    let mut active_kinds = Vec::new();
    if has_error {
        active_kinds.push(AttentionKindV3::Error);
    }
    if has_approval {
        active_kinds.push(AttentionKindV3::Approval);
    }
    if has_question {
        active_kinds.push(AttentionKindV3::Question);
    }
    if has_completion {
        active_kinds.push(AttentionKindV3::Completion);
    }

    let latest_request_at = pending_requests
        .iter()
        .map(|request| request.updated_at)
        .max();
    let latest_turn_at = if has_error || has_completion {
        turn.completed_at
    } else {
        None
    };

    AttentionSummaryV3 {
        highest_priority: AttentionPriorityV3::from_active_kinds(&active_kinds),
        unresolved_count: pending_requests.len() as u32,
        latest_at: latest_request_at
            .into_iter()
            .chain(latest_turn_at)
            .max()
            .or_else(|| has_error.then_some(updated_at)),
        active_kinds,
        generation,
    }
}

#[derive(Debug, Clone)]
pub struct SyncV3Reducer {
    snapshot: SyncV3PaneSnapshot,
    requests: RequestTableV3,
    attention_generation: u64,
}

impl SyncV3Reducer {
    pub fn new(mut snapshot: SyncV3PaneSnapshot) -> Self {
        let mut requests = RequestTableV3::default();
        for request in snapshot.pending_requests.drain(..) {
            requests.upsert(request);
        }

        let attention_generation = snapshot.attention.generation;
        let mut reducer = Self {
            snapshot,
            requests,
            attention_generation,
        };
        reducer.refresh_requests_and_summaries(reducer.snapshot.updated_at);
        reducer
    }

    pub fn snapshot(&self) -> &SyncV3PaneSnapshot {
        &self.snapshot
    }

    pub fn into_snapshot(self) -> SyncV3PaneSnapshot {
        self.snapshot
    }

    pub fn upsert_request(&mut self, request: PendingRequestV3) -> bool {
        let updated_at = request.updated_at;
        if !self.requests.upsert(request) {
            return false;
        }
        self.refresh_requests_and_summaries(updated_at);
        true
    }

    pub fn resolve_request(
        &mut self,
        request_id: &str,
        status: PendingRequestStatusV3,
        updated_at: DateTime<Utc>,
    ) -> bool {
        if !self.requests.update_status(request_id, status, updated_at) {
            return false;
        }
        self.refresh_requests_and_summaries(updated_at);
        true
    }

    pub fn resolve_requests_matching<F>(
        &mut self,
        status: PendingRequestStatusV3,
        updated_at: DateTime<Utc>,
        mut predicate: F,
    ) -> usize
    where
        F: FnMut(&PendingRequestV3) -> bool,
    {
        let mut changed = 0usize;
        for request in self.requests.entries.values_mut() {
            if predicate(request) && request.status != status {
                request.status = status;
                request.updated_at = updated_at;
                changed += 1;
            }
        }

        if changed > 0 {
            self.refresh_requests_and_summaries(updated_at);
        }

        changed
    }

    pub fn set_review_mode(&mut self, review_mode: bool, updated_at: DateTime<Utc>) {
        if self.snapshot.thread.flags.review_mode == review_mode
            && self.snapshot.updated_at == updated_at
        {
            return;
        }

        self.snapshot.thread.flags.review_mode = review_mode;
        self.snapshot.updated_at = updated_at;
        self.recompute_attention(updated_at);
    }

    pub fn set_provider_raw_codex(&mut self, raw: serde_json::Value, updated_at: DateTime<Utc>) {
        if self.snapshot.provider_raw.codex.as_ref() == Some(&raw)
            && self.snapshot.updated_at == updated_at
        {
            return;
        }

        self.snapshot.provider_raw.codex = Some(raw);
        self.snapshot.updated_at = updated_at;
    }

    pub fn merge_provider_raw_claude(
        &mut self,
        patch: serde_json::Value,
        updated_at: DateTime<Utc>,
    ) {
        let merged = merge_provider_raw_slot(self.snapshot.provider_raw.claude.clone(), patch);
        if self.snapshot.provider_raw.claude.as_ref() == Some(&merged)
            && self.snapshot.updated_at == updated_at
        {
            return;
        }

        self.snapshot.provider_raw.claude = Some(merged);
        self.snapshot.updated_at = updated_at;
    }

    pub fn set_turn(&mut self, turn: TurnStateV3, updated_at: DateTime<Utc>) {
        self.snapshot.thread.turn = turn;
        self.snapshot.updated_at = updated_at;
        self.recompute_attention(updated_at);
    }

    pub fn set_thread_state(
        &mut self,
        agent_lifecycle: AgentLifecycleV3,
        thread_lifecycle: ThreadLifecycleV3,
        execution: ThreadExecutionV3,
        updated_at: DateTime<Utc>,
    ) {
        self.snapshot.agent.lifecycle = agent_lifecycle;
        self.snapshot.thread.lifecycle = thread_lifecycle;
        self.snapshot.thread.execution = execution;
        self.snapshot.updated_at = updated_at;
        self.recompute_attention(updated_at);
    }

    pub fn set_freshness(
        &mut self,
        blocking: FreshnessState,
        execution: FreshnessState,
        snapshot_override: Option<FreshnessState>,
    ) {
        self.snapshot.freshness = build_freshness_summary(blocking, execution, snapshot_override);
    }

    fn refresh_requests_and_summaries(&mut self, updated_at: DateTime<Utc>) {
        self.snapshot.pending_requests = self.requests.pending();
        self.snapshot.thread.blocking = derive_blocking_state(&self.snapshot.pending_requests);
        self.snapshot.updated_at = updated_at;
        self.recompute_attention(updated_at);
    }

    fn recompute_attention(&mut self, updated_at: DateTime<Utc>) {
        let current_generation = self.attention_generation;
        let next = build_attention_summary(
            &self.snapshot.pending_requests,
            self.snapshot.agent.lifecycle,
            self.snapshot.thread.lifecycle,
            &self.snapshot.thread.turn,
            updated_at,
            current_generation,
        );

        let summary_changed = self.snapshot.attention.active_kinds != next.active_kinds
            || self.snapshot.attention.highest_priority != next.highest_priority
            || self.snapshot.attention.unresolved_count != next.unresolved_count
            || self.snapshot.attention.latest_at != next.latest_at;

        if summary_changed {
            self.attention_generation += 1;
        }

        self.snapshot.attention = build_attention_summary(
            &self.snapshot.pending_requests,
            self.snapshot.agent.lifecycle,
            self.snapshot.thread.lifecycle,
            &self.snapshot.thread.turn,
            updated_at,
            self.attention_generation,
        );
    }
}

fn derive_blocking_state(pending_requests: &[PendingRequestV3]) -> ThreadBlockingV3 {
    if pending_requests
        .iter()
        .any(|request| matches!(request.kind, PendingRequestKindV3::Approval))
    {
        ThreadBlockingV3::WaitingApproval
    } else if pending_requests
        .iter()
        .any(|request| matches!(request.kind, PendingRequestKindV3::UserInput))
    {
        ThreadBlockingV3::WaitingUserInput
    } else {
        ThreadBlockingV3::None
    }
}

fn freshness_rank(level: FreshnessState) -> u8 {
    match level {
        FreshnessState::Fresh => 0,
        FreshnessState::Stale => 1,
        FreshnessState::Down => 2,
    }
}

fn worst_freshness(left: FreshnessState, right: FreshnessState) -> FreshnessState {
    if freshness_rank(left) >= freshness_rank(right) {
        left
    } else {
        right
    }
}

fn merge_provider_raw_slot(
    existing: Option<serde_json::Value>,
    patch: serde_json::Value,
) -> serde_json::Value {
    match (existing, patch) {
        (Some(serde_json::Value::Object(mut current)), serde_json::Value::Object(update)) => {
            for (key, value) in update {
                current.insert(key, value);
            }
            serde_json::Value::Object(current)
        }
        (_, replacement) => replacement,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agtmux_core_v5::sync_v3::{
        AgentStateV3, PresenceV3, ProviderRawEnvelopeV3, SyncV3PaneSnapshot, ThreadFlagsV3,
    };
    use agtmux_core_v5::types::{PaneInstanceId, Provider, SourceKind};
    use chrono::TimeZone;

    fn ts(second: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 3, 9, 20, 0, second)
            .single()
            .expect("valid timestamp")
    }

    fn pending_request(id: &str, kind: PendingRequestKindV3, second: u32) -> PendingRequestV3 {
        PendingRequestV3 {
            request_id: id.to_string(),
            kind,
            title: None,
            detail: None,
            created_at: ts(second),
            updated_at: ts(second),
            status: PendingRequestStatusV3::Pending,
            source: agtmux_core_v5::sync_v3::PendingRequestSourceV3 {
                provider: Provider::Codex,
                source_kind: SourceKind::CodexAppserver,
            },
        }
    }

    fn base_snapshot() -> SyncV3PaneSnapshot {
        SyncV3PaneSnapshot {
            session_name: "workbench".to_string(),
            window_id: "@5".to_string(),
            session_key: "codex:%12".to_string(),
            pane_id: "%12".to_string(),
            pane_instance_id: PaneInstanceId {
                pane_id: "%12".to_string(),
                generation: 7,
                birth_ts: ts(0),
            },
            provider: Some(Provider::Codex),
            conversation_title: None,
            session_subtitle: None,
            presence: PresenceV3::Managed,
            agent: AgentStateV3 {
                lifecycle: AgentLifecycleV3::Running,
            },
            thread: agtmux_core_v5::sync_v3::ThreadStateV3 {
                lifecycle: ThreadLifecycleV3::Active,
                blocking: ThreadBlockingV3::None,
                execution: ThreadExecutionV3::Thinking,
                flags: ThreadFlagsV3::default(),
                turn: TurnStateV3 {
                    outcome: TurnOutcomeV3::None,
                    sequence: Some(42),
                    started_at: Some(ts(1)),
                    completed_at: None,
                },
            },
            pending_requests: Vec::new(),
            attention: AttentionSummaryV3::none(),
            freshness: FreshnessSummaryV3::fresh(),
            provider_raw: ProviderRawEnvelopeV3::default(),
            updated_at: ts(2),
        }
    }

    #[test]
    fn request_table_returns_pending_only_in_stable_order() {
        let mut table = RequestTableV3::default();
        table.upsert(pending_request("b", PendingRequestKindV3::Approval, 4));
        table.upsert(pending_request("a", PendingRequestKindV3::Approval, 3));
        table.upsert(PendingRequestV3 {
            status: PendingRequestStatusV3::Resolved,
            ..pending_request("c", PendingRequestKindV3::UserInput, 2)
        });

        let pending = table.pending();
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].request_id, "a");
        assert_eq!(pending[1].request_id, "b");
    }

    #[test]
    fn attention_summary_prioritizes_error_over_requests() {
        let summary = build_attention_summary(
            &[pending_request("req-1", PendingRequestKindV3::Approval, 10)],
            AgentLifecycleV3::Errored,
            ThreadLifecycleV3::Errored,
            &TurnStateV3 {
                outcome: TurnOutcomeV3::Errored,
                sequence: Some(5),
                started_at: Some(ts(5)),
                completed_at: Some(ts(11)),
            },
            ts(12),
            7,
        );

        assert_eq!(
            summary.active_kinds,
            vec![AttentionKindV3::Error, AttentionKindV3::Approval]
        );
        assert_eq!(summary.highest_priority, AttentionPriorityV3::Error);
        assert_eq!(summary.unresolved_count, 1);
        assert_eq!(summary.latest_at, Some(ts(11)));
        assert_eq!(summary.generation, 7);
    }

    #[test]
    fn freshness_summary_uses_worst_subfield_and_override() {
        let summary = build_freshness_summary(FreshnessState::Fresh, FreshnessState::Stale, None);
        assert_eq!(summary.snapshot, FreshnessState::Stale);

        let summary = build_freshness_summary(
            FreshnessState::Fresh,
            FreshnessState::Stale,
            Some(FreshnessState::Down),
        );
        assert_eq!(summary.snapshot, FreshnessState::Down);
    }

    #[test]
    fn reducer_updates_blocking_and_attention_from_request_table() {
        let mut reducer = SyncV3Reducer::new(base_snapshot());

        assert!(reducer.upsert_request(pending_request(
            "req-1",
            PendingRequestKindV3::Approval,
            20
        )));
        assert_eq!(
            reducer.snapshot().thread.blocking,
            ThreadBlockingV3::WaitingApproval
        );
        assert_eq!(
            reducer.snapshot().attention.highest_priority,
            AttentionPriorityV3::Approval
        );
        assert_eq!(reducer.snapshot().attention.generation, 1);
        assert_eq!(reducer.snapshot().pending_requests.len(), 1);

        assert!(reducer.resolve_request("req-1", PendingRequestStatusV3::Resolved, ts(25)));
        assert_eq!(reducer.snapshot().thread.blocking, ThreadBlockingV3::None);
        assert_eq!(
            reducer.snapshot().attention.highest_priority,
            AttentionPriorityV3::None
        );
        assert_eq!(reducer.snapshot().attention.generation, 2);
        assert!(reducer.snapshot().pending_requests.is_empty());
    }

    #[test]
    fn reducer_keeps_completion_without_reintroducing_blocking() {
        let mut reducer = SyncV3Reducer::new(base_snapshot());
        reducer.set_thread_state(
            AgentLifecycleV3::Completed,
            ThreadLifecycleV3::Idle,
            ThreadExecutionV3::None,
            ts(40),
        );
        reducer.set_turn(
            TurnStateV3 {
                outcome: TurnOutcomeV3::Completed,
                sequence: Some(42),
                started_at: Some(ts(1)),
                completed_at: Some(ts(39)),
            },
            ts(40),
        );

        assert_eq!(reducer.snapshot().thread.blocking, ThreadBlockingV3::None);
        assert_eq!(
            reducer.snapshot().attention.active_kinds,
            vec![AttentionKindV3::Completion]
        );
        assert_eq!(
            reducer.snapshot().attention.highest_priority,
            AttentionPriorityV3::Completion
        );
    }

    #[test]
    fn reducer_merges_provider_raw_claude_without_dropping_existing_fields() {
        let mut reducer = SyncV3Reducer::new(base_snapshot());

        reducer.merge_provider_raw_claude(
            serde_json::json!({
                "hook_event": "PermissionRequest",
                "hook_source": "claude_hooks"
            }),
            ts(50),
        );
        reducer.merge_provider_raw_claude(
            serde_json::json!({
                "jsonl_hint": "tool_use"
            }),
            ts(51),
        );

        assert_eq!(
            reducer.snapshot().provider_raw.claude,
            Some(serde_json::json!({
                "hook_event": "PermissionRequest",
                "hook_source": "claude_hooks",
                "jsonl_hint": "tool_use"
            }))
        );
    }
}

use agtmux_core_v5::sync_v3::{
    AgentLifecycleV3, PendingRequestKindV3, PendingRequestSourceV3, PendingRequestStatusV3,
    PendingRequestV3, ThreadExecutionV3, ThreadLifecycleV3, TurnOutcomeV3,
};
use agtmux_core_v5::types::{Provider, SourceEventV2, SourceKind};
use chrono::{DateTime, SecondsFormat, Utc};

use crate::sync_v3::SyncV3Reducer;

pub fn apply_codex_source_event(reducer: &mut SyncV3Reducer, event: &SourceEventV2) -> bool {
    if event.provider != Provider::Codex {
        return false;
    }

    let Some(semantic) = CodexSemanticEvent::from_source_event(event) else {
        return false;
    };

    let updated_at = event.actual_activity_at.unwrap_or(event.observed_at);
    reducer.set_provider_raw_codex(semantic.provider_raw(event, updated_at), updated_at);

    match semantic.inner_type.as_deref() {
        Some("task_started") | Some("user_message") => {
            activate_turn(
                reducer,
                updated_at,
                Some(updated_at),
                ThreadExecutionV3::Thinking,
            );
            true
        }
        Some("function_call") | Some("custom_tool_call") => {
            activate_turn(reducer, updated_at, None, ThreadExecutionV3::ToolRunning);
            true
        }
        Some("function_call_output") | Some("custom_tool_call_output") => {
            activate_turn(reducer, updated_at, None, ThreadExecutionV3::Thinking);
            true
        }
        Some("entered_review_mode") => {
            activate_turn(
                reducer,
                updated_at,
                None,
                execution_for_review_mode(reducer.snapshot().thread.execution),
            );
            reducer.set_review_mode(true, updated_at);
            reducer.upsert_request(semantic.review_request(event, updated_at));
            true
        }
        Some("exited_review_mode") => {
            reducer.set_review_mode(false, updated_at);
            resolve_codex_requests(
                reducer,
                event.source_kind,
                PendingRequestKindV3::Approval,
                semantic.request_id.as_deref(),
                updated_at,
            );
            true
        }
        Some("task_complete") => {
            reducer.set_review_mode(false, updated_at);
            resolve_codex_requests(
                reducer,
                event.source_kind,
                PendingRequestKindV3::Approval,
                None,
                updated_at,
            );
            finish_turn(
                reducer,
                updated_at,
                AgentLifecycleV3::Completed,
                ThreadLifecycleV3::Idle,
                TurnOutcomeV3::Completed,
            );
            true
        }
        Some("turn_aborted") => {
            reducer.set_review_mode(false, updated_at);
            resolve_codex_requests(
                reducer,
                event.source_kind,
                PendingRequestKindV3::Approval,
                None,
                updated_at,
            );

            let lifecycle = if semantic.reason.as_deref() == Some("interrupted") {
                ThreadLifecycleV3::Interrupted
            } else {
                ThreadLifecycleV3::Errored
            };
            let outcome = if lifecycle == ThreadLifecycleV3::Interrupted {
                TurnOutcomeV3::Aborted
            } else {
                TurnOutcomeV3::Errored
            };
            let agent_lifecycle = if lifecycle == ThreadLifecycleV3::Interrupted {
                AgentLifecycleV3::Running
            } else {
                AgentLifecycleV3::Errored
            };

            finish_turn(reducer, updated_at, agent_lifecycle, lifecycle, outcome);
            true
        }
        _ => false,
    }
}

fn activate_turn(
    reducer: &mut SyncV3Reducer,
    updated_at: DateTime<Utc>,
    started_at: Option<DateTime<Utc>>,
    execution: ThreadExecutionV3,
) {
    let mut turn = reducer.snapshot().thread.turn.clone();
    reducer.set_thread_state(
        AgentLifecycleV3::Running,
        ThreadLifecycleV3::Active,
        execution,
        updated_at,
    );

    if turn.started_at.is_none()
        || turn.completed_at.is_some()
        || turn.outcome != TurnOutcomeV3::None
    {
        turn.started_at = started_at.or(Some(updated_at));
    }
    turn.outcome = TurnOutcomeV3::None;
    turn.completed_at = None;
    reducer.set_turn(turn, updated_at);
}

fn finish_turn(
    reducer: &mut SyncV3Reducer,
    updated_at: DateTime<Utc>,
    agent_lifecycle: AgentLifecycleV3,
    lifecycle: ThreadLifecycleV3,
    outcome: TurnOutcomeV3,
) {
    let mut turn = reducer.snapshot().thread.turn.clone();
    reducer.set_thread_state(
        agent_lifecycle,
        lifecycle,
        ThreadExecutionV3::None,
        updated_at,
    );

    if turn.started_at.is_none() {
        turn.started_at = Some(updated_at);
    }
    turn.outcome = outcome;
    turn.completed_at = Some(updated_at);
    reducer.set_turn(turn, updated_at);
}

fn execution_for_review_mode(current: ThreadExecutionV3) -> ThreadExecutionV3 {
    match current {
        ThreadExecutionV3::None => ThreadExecutionV3::Thinking,
        other => other,
    }
}

fn resolve_codex_requests(
    reducer: &mut SyncV3Reducer,
    source_kind: SourceKind,
    kind: PendingRequestKindV3,
    request_id: Option<&str>,
    updated_at: DateTime<Utc>,
) -> usize {
    reducer.resolve_requests_matching(PendingRequestStatusV3::Resolved, updated_at, |request| {
        if request.source.provider != Provider::Codex
            || request.source.source_kind != source_kind
            || request.kind != kind
        {
            return false;
        }

        request_id.is_none_or(|id| request.request_id == id)
    })
}

#[derive(Debug, Clone)]
struct CodexSemanticEvent {
    top_type: Option<String>,
    inner_type: Option<String>,
    turn_id: Option<String>,
    call_id: Option<String>,
    tool_name: Option<String>,
    reason: Option<String>,
    review_target_type: Option<String>,
    user_facing_hint: Option<String>,
    request_id: Option<String>,
    bootstrap: bool,
}

impl CodexSemanticEvent {
    fn from_source_event(event: &SourceEventV2) -> Option<Self> {
        let raw = event.payload.get("codex_jsonl")?;
        Some(Self {
            top_type: raw
                .get("top_type")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            inner_type: raw
                .get("inner_type")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            turn_id: raw
                .get("turn_id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            call_id: raw
                .get("call_id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            tool_name: raw
                .get("tool_name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            reason: raw
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            review_target_type: raw
                .get("review_target_type")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            user_facing_hint: raw
                .get("user_facing_hint")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            request_id: raw
                .get("request_id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            bootstrap: raw
                .get("bootstrap")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        })
    }

    fn review_request(&self, event: &SourceEventV2, created_at: DateTime<Utc>) -> PendingRequestV3 {
        PendingRequestV3 {
            request_id: self
                .request_id
                .clone()
                .unwrap_or_else(|| synth_review_request_id(&event.session_key, self, created_at)),
            kind: PendingRequestKindV3::Approval,
            title: Some(self.review_title()),
            detail: self.review_detail(),
            created_at,
            updated_at: created_at,
            status: PendingRequestStatusV3::Pending,
            source: PendingRequestSourceV3 {
                provider: Provider::Codex,
                source_kind: event.source_kind,
            },
        }
    }

    fn review_title(&self) -> String {
        self.user_facing_hint
            .clone()
            .or_else(|| {
                self.review_target_type
                    .as_deref()
                    .map(|target| match target {
                        "uncommittedChanges" => "current changes".to_string(),
                        other => format!("approval for {other}"),
                    })
            })
            .unwrap_or_else(|| "approval required".to_string())
    }

    fn review_detail(&self) -> Option<String> {
        match (&self.review_target_type, &self.user_facing_hint) {
            (Some(target), Some(hint)) if target != hint => Some(format!("{target}: {hint}")),
            (Some(target), None) => Some(target.clone()),
            (None, Some(hint)) => Some(hint.clone()),
            _ => None,
        }
    }

    fn provider_raw(&self, event: &SourceEventV2, updated_at: DateTime<Utc>) -> serde_json::Value {
        serde_json::json!({
            "source_kind": event.source_kind.as_str(),
            "event_type": event.event_type.as_str(),
            "observed_at": event.observed_at.to_rfc3339_opts(SecondsFormat::Millis, true),
            "actual_activity_at": updated_at.to_rfc3339_opts(SecondsFormat::Millis, true),
            "semantic_event": {
                "top_type": self.top_type.as_deref(),
                "inner_type": self.inner_type.as_deref(),
                "turn_id": self.turn_id.as_deref(),
                "call_id": self.call_id.as_deref(),
                "tool_name": self.tool_name.as_deref(),
                "reason": self.reason.as_deref(),
                "review_target_type": self.review_target_type.as_deref(),
                "user_facing_hint": self.user_facing_hint.as_deref(),
                "request_id": self.request_id.as_deref(),
                "bootstrap": self.bootstrap
            }
        })
    }
}

fn synth_review_request_id(
    session_key: &str,
    semantic: &CodexSemanticEvent,
    created_at: DateTime<Utc>,
) -> String {
    let turn_component = semantic
        .turn_id
        .as_deref()
        .map(sanitize_request_component)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            created_at
                .to_rfc3339_opts(SecondsFormat::Millis, true)
                .replace(':', "_")
        });
    let target_component = semantic
        .review_target_type
        .as_deref()
        .map(sanitize_request_component)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "review".to_string());
    let hint_component = semantic
        .user_facing_hint
        .as_deref()
        .map(sanitize_request_component)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "pending".to_string());

    format!("codex-review:{session_key}:{turn_component}:{target_component}:{hint_component}")
}

fn sanitize_request_component(value: &str) -> String {
    let mut output = String::new();
    let mut last_was_sep = false;

    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            output.push(ch.to_ascii_lowercase());
            last_was_sep = false;
        } else if !last_was_sep {
            output.push('_');
            last_was_sep = true;
        }

        if output.len() >= 64 {
            break;
        }
    }

    output.trim_matches('_').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agtmux_core_v5::sync_v3::{
        AgentStateV3, AttentionSummaryV3, FreshnessSummaryV3, PresenceV3, ProviderRawEnvelopeV3,
        SyncV3PaneSnapshot, ThreadBlockingV3, ThreadFlagsV3, ThreadStateV3, TurnStateV3,
    };
    use agtmux_core_v5::types::{EvidenceTier, PaneInstanceId};
    use chrono::TimeZone;

    fn ts(second: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 3, 9, 21, 0, second)
            .single()
            .expect("valid timestamp")
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
            presence: PresenceV3::Managed,
            agent: AgentStateV3 {
                lifecycle: AgentLifecycleV3::Running,
            },
            thread: ThreadStateV3 {
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

    fn codex_event(
        inner_type: &str,
        actual_activity_at: DateTime<Utc>,
        extra: serde_json::Value,
    ) -> SourceEventV2 {
        codex_event_with_compat_event_type(
            "activity.unknown",
            inner_type,
            actual_activity_at,
            extra,
        )
    }

    fn codex_event_with_compat_event_type(
        compat_event_type: &str,
        inner_type: &str,
        actual_activity_at: DateTime<Utc>,
        extra: serde_json::Value,
    ) -> SourceEventV2 {
        let mut semantic = serde_json::json!({
            "top_type": if inner_type.starts_with("function_call") || inner_type.starts_with("custom_tool_call") {
                "response_item"
            } else {
                "event_msg"
            },
            "inner_type": inner_type,
            "bootstrap": false
        });

        if let Some(target) = semantic.as_object_mut() {
            for (key, value) in extra
                .as_object()
                .into_iter()
                .flat_map(|object| object.iter())
            {
                target.insert(key.clone(), value.clone());
            }
        }

        SourceEventV2 {
            event_id: format!("codex-{inner_type}-{}", actual_activity_at.timestamp()),
            provider: Provider::Codex,
            source_kind: SourceKind::CodexJsonl,
            tier: EvidenceTier::Deterministic,
            observed_at: actual_activity_at,
            session_key: "codex:%12".to_string(),
            pane_id: Some("%12".to_string()),
            pane_generation: Some(7),
            pane_birth_ts: Some(ts(0)),
            source_event_id: None,
            // Legacy compat string is still carried for sync-v2 consumers, but the
            // v3 reducer should derive semantics from `payload.codex_jsonl`.
            event_type: compat_event_type.to_string(),
            payload: serde_json::json!({
                "fsm_state": "legacy",
                "codex_jsonl": semantic
            }),
            confidence: 1.0,
            is_heartbeat: false,
            actual_activity_at: Some(actual_activity_at),
        }
    }

    #[test]
    fn task_complete_normalizes_to_idle_completion_without_blocking() {
        let mut reducer = SyncV3Reducer::new(base_snapshot());
        let event = codex_event(
            "task_complete",
            ts(10),
            serde_json::json!({"turn_id": "turn-1"}),
        );

        assert!(apply_codex_source_event(&mut reducer, &event));
        assert_eq!(
            reducer.snapshot().agent.lifecycle,
            AgentLifecycleV3::Completed
        );
        assert_eq!(reducer.snapshot().thread.lifecycle, ThreadLifecycleV3::Idle);
        assert_eq!(reducer.snapshot().thread.blocking, ThreadBlockingV3::None);
        assert_eq!(reducer.snapshot().thread.execution, ThreadExecutionV3::None);
        assert_eq!(
            reducer.snapshot().thread.turn.outcome,
            TurnOutcomeV3::Completed
        );
        assert_eq!(reducer.snapshot().thread.turn.completed_at, Some(ts(10)));
        assert!(reducer.snapshot().pending_requests.is_empty());
        assert_eq!(
            reducer.snapshot().attention.highest_priority,
            agtmux_core_v5::sync_v3::AttentionPriorityV3::Completion
        );
    }

    #[test]
    fn entered_review_mode_opens_request_and_uses_request_truth_for_blocking() {
        let mut snapshot = base_snapshot();
        snapshot.thread.execution = ThreadExecutionV3::ToolRunning;
        let mut reducer = SyncV3Reducer::new(snapshot);
        let event = codex_event(
            "entered_review_mode",
            ts(11),
            serde_json::json!({
                "turn_id": "turn-1",
                "review_target_type": "uncommittedChanges",
                "user_facing_hint": "current changes"
            }),
        );

        assert!(apply_codex_source_event(&mut reducer, &event));
        assert!(reducer.snapshot().thread.flags.review_mode);
        assert_eq!(
            reducer.snapshot().thread.execution,
            ThreadExecutionV3::ToolRunning
        );
        assert_eq!(
            reducer.snapshot().thread.blocking,
            ThreadBlockingV3::WaitingApproval
        );
        assert_eq!(reducer.snapshot().pending_requests.len(), 1);
        assert_eq!(
            reducer.snapshot().pending_requests[0].kind,
            PendingRequestKindV3::Approval
        );
        assert_eq!(
            reducer.snapshot().attention.highest_priority,
            agtmux_core_v5::sync_v3::AttentionPriorityV3::Approval
        );
    }

    #[test]
    fn review_mode_flag_alone_does_not_drive_blocking() {
        let mut reducer = SyncV3Reducer::new(base_snapshot());
        reducer.set_review_mode(true, ts(12));

        assert!(reducer.snapshot().thread.flags.review_mode);
        assert_eq!(reducer.snapshot().thread.blocking, ThreadBlockingV3::None);
        assert!(reducer.snapshot().pending_requests.is_empty());
        assert_eq!(
            reducer.snapshot().attention.highest_priority,
            agtmux_core_v5::sync_v3::AttentionPriorityV3::None
        );
    }

    #[test]
    fn function_call_normalizes_to_tool_running_execution() {
        let mut reducer = SyncV3Reducer::new(base_snapshot());
        reducer.set_turn(
            TurnStateV3 {
                outcome: TurnOutcomeV3::Completed,
                sequence: Some(42),
                started_at: Some(ts(1)),
                completed_at: Some(ts(9)),
            },
            ts(9),
        );
        let event = codex_event(
            "function_call",
            ts(13),
            serde_json::json!({
                "call_id": "call-1",
                "tool_name": "shell_command"
            }),
        );

        assert!(apply_codex_source_event(&mut reducer, &event));
        assert_eq!(
            reducer.snapshot().agent.lifecycle,
            AgentLifecycleV3::Running
        );
        assert_eq!(
            reducer.snapshot().thread.lifecycle,
            ThreadLifecycleV3::Active
        );
        assert_eq!(
            reducer.snapshot().thread.execution,
            ThreadExecutionV3::ToolRunning
        );
        assert_eq!(reducer.snapshot().thread.turn.outcome, TurnOutcomeV3::None);
        assert_eq!(reducer.snapshot().thread.turn.completed_at, None);
        assert_eq!(reducer.snapshot().thread.turn.started_at, Some(ts(13)));
    }

    #[test]
    fn exited_review_mode_resolves_synthetic_request_and_clears_blocking() {
        let mut reducer = SyncV3Reducer::new(base_snapshot());
        let enter = codex_event(
            "entered_review_mode",
            ts(14),
            serde_json::json!({
                "turn_id": "turn-1",
                "review_target_type": "uncommittedChanges",
                "user_facing_hint": "current changes"
            }),
        );
        let exit = codex_event("exited_review_mode", ts(15), serde_json::json!({}));

        assert!(apply_codex_source_event(&mut reducer, &enter));
        assert_eq!(reducer.snapshot().pending_requests.len(), 1);
        assert!(apply_codex_source_event(&mut reducer, &exit));
        assert!(!reducer.snapshot().thread.flags.review_mode);
        assert_eq!(reducer.snapshot().thread.blocking, ThreadBlockingV3::None);
        assert!(reducer.snapshot().pending_requests.is_empty());
        assert_eq!(
            reducer.snapshot().attention.highest_priority,
            agtmux_core_v5::sync_v3::AttentionPriorityV3::None
        );
    }

    #[test]
    fn task_complete_ignores_contradictory_compat_event_type_when_payload_truth_exists() {
        let mut reducer = SyncV3Reducer::new(base_snapshot());
        let event = codex_event_with_compat_event_type(
            "activity.running",
            "task_complete",
            ts(16),
            serde_json::json!({"turn_id": "turn-1"}),
        );

        assert!(apply_codex_source_event(&mut reducer, &event));
        assert_eq!(
            reducer.snapshot().agent.lifecycle,
            AgentLifecycleV3::Completed
        );
        assert_eq!(reducer.snapshot().thread.lifecycle, ThreadLifecycleV3::Idle);
        assert_eq!(reducer.snapshot().thread.execution, ThreadExecutionV3::None);
        assert_eq!(
            reducer.snapshot().thread.turn.outcome,
            TurnOutcomeV3::Completed
        );
    }
}

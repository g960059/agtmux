use agtmux_core_v5::sync_v3::{
    AgentLifecycleV3, PendingRequestKindV3, PendingRequestSourceV3, PendingRequestStatusV3,
    PendingRequestV3, ThreadExecutionV3, ThreadLifecycleV3, TurnOutcomeV3,
};
use agtmux_core_v5::types::{Provider, SourceEventV2, SourceKind};
use chrono::{DateTime, Utc};

use crate::sync_v3::SyncV3Reducer;

pub fn apply_claude_source_event(reducer: &mut SyncV3Reducer, event: &SourceEventV2) -> bool {
    if event.provider != Provider::Claude {
        return false;
    }

    let updated_at = event.actual_activity_at.unwrap_or(event.observed_at);

    match event.source_kind {
        SourceKind::ClaudeHooks => apply_hook_event(reducer, event, updated_at),
        SourceKind::ClaudeJsonl => apply_jsonl_event(reducer, event, updated_at),
        _ => false,
    }
}

fn apply_hook_event(
    reducer: &mut SyncV3Reducer,
    event: &SourceEventV2,
    updated_at: DateTime<Utc>,
) -> bool {
    let Some(semantic) = ClaudeHookSemantic::from_source_event(event) else {
        return false;
    };

    match semantic.hook_type.as_deref() {
        Some("PermissionRequest") => {
            reducer.merge_provider_raw_claude(semantic.provider_raw_patch(), updated_at);
            activate_turn(reducer, updated_at, reducer.snapshot().thread.execution);
            reducer.upsert_request(semantic.approval_request(event, updated_at));
            true
        }
        Some("Stop") | Some("SubagentStop") => {
            reducer.merge_provider_raw_claude(semantic.provider_raw_patch(), updated_at);
            resolve_claude_hook_requests(reducer, updated_at);
            finish_turn(reducer, updated_at, TurnOutcomeV3::Completed);
            true
        }
        _ => false,
    }
}

fn apply_jsonl_event(
    reducer: &mut SyncV3Reducer,
    event: &SourceEventV2,
    updated_at: DateTime<Utc>,
) -> bool {
    let Some(semantic) = ClaudeJsonlSemantic::from_source_event(event) else {
        return false;
    };

    match semantic.line_type.as_str() {
        "tool_use" | "progress" => {
            reducer.merge_provider_raw_claude(semantic.provider_raw_patch(), updated_at);
            activate_turn(reducer, updated_at, ThreadExecutionV3::ToolRunning);
            true
        }
        "tool_result" => {
            reducer.merge_provider_raw_claude(semantic.provider_raw_patch(), updated_at);
            activate_turn(reducer, updated_at, ThreadExecutionV3::Thinking);
            true
        }
        "assistant" => {
            reducer.merge_provider_raw_claude(semantic.provider_raw_patch(), updated_at);
            reducer.set_thread_state(
                AgentLifecycleV3::Running,
                ThreadLifecycleV3::Idle,
                ThreadExecutionV3::None,
                updated_at,
            );
            true
        }
        _ => false,
    }
}

fn activate_turn(
    reducer: &mut SyncV3Reducer,
    updated_at: DateTime<Utc>,
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
        turn.started_at = Some(updated_at);
    }
    turn.outcome = TurnOutcomeV3::None;
    turn.completed_at = None;
    reducer.set_turn(turn, updated_at);
}

fn finish_turn(reducer: &mut SyncV3Reducer, updated_at: DateTime<Utc>, outcome: TurnOutcomeV3) {
    let mut turn = reducer.snapshot().thread.turn.clone();
    reducer.set_thread_state(
        AgentLifecycleV3::Running,
        ThreadLifecycleV3::Idle,
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

fn resolve_claude_hook_requests(reducer: &mut SyncV3Reducer, updated_at: DateTime<Utc>) -> usize {
    reducer.resolve_requests_matching(PendingRequestStatusV3::Resolved, updated_at, |request| {
        request.source.provider == Provider::Claude
            && request.source.source_kind == SourceKind::ClaudeHooks
    })
}

#[derive(Debug, Clone)]
struct ClaudeHookSemantic {
    hook_type: Option<String>,
    notification_type: Option<String>,
    tool_name: Option<String>,
    request_id: Option<String>,
}

impl ClaudeHookSemantic {
    fn from_source_event(event: &SourceEventV2) -> Option<Self> {
        let raw = event.payload.get("claude_hook")?;
        Some(Self {
            hook_type: raw
                .get("hook_type")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            notification_type: raw
                .get("notification_type")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            tool_name: raw
                .get("tool_name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            request_id: raw
                .get("request_id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
        })
    }

    fn approval_request(
        &self,
        event: &SourceEventV2,
        created_at: DateTime<Utc>,
    ) -> PendingRequestV3 {
        PendingRequestV3 {
            request_id: self
                .request_id
                .clone()
                .unwrap_or_else(|| synth_approval_request_id(&event.session_key, self)),
            kind: PendingRequestKindV3::Approval,
            title: Some(self.approval_title()),
            detail: Some(self.approval_detail()),
            created_at,
            updated_at: created_at,
            status: PendingRequestStatusV3::Pending,
            source: PendingRequestSourceV3 {
                provider: Provider::Claude,
                source_kind: event.source_kind,
            },
        }
    }

    fn approval_title(&self) -> String {
        match self.tool_name.as_deref() {
            Some(tool) if is_shell_tool(tool) => format!("{tool} command approval"),
            Some(tool) => format!("{tool} approval"),
            None => "Claude approval required".to_string(),
        }
    }

    fn approval_detail(&self) -> String {
        match self.tool_name.as_deref() {
            Some(tool) if is_shell_tool(tool) => {
                "Claude Code is requesting permission to run a shell command.".to_string()
            }
            Some(tool) => format!("Claude Code is requesting permission to run the {tool} tool."),
            None => "Claude Code is requesting permission to continue.".to_string(),
        }
    }

    fn provider_raw_patch(&self) -> serde_json::Value {
        let mut patch = serde_json::Map::from_iter([
            (
                "hook_event".to_string(),
                self.hook_type
                    .clone()
                    .map(serde_json::Value::String)
                    .unwrap_or(serde_json::Value::Null),
            ),
            (
                "hook_source".to_string(),
                serde_json::Value::String(SourceKind::ClaudeHooks.as_str().to_string()),
            ),
        ]);

        if let Some(notification_type) = &self.notification_type {
            patch.insert(
                "notification_type".to_string(),
                serde_json::Value::String(notification_type.clone()),
            );
        }
        if let Some(tool_name) = &self.tool_name {
            patch.insert(
                "tool_name".to_string(),
                serde_json::Value::String(tool_name.clone()),
            );
        }
        if let Some(request_id) = &self.request_id {
            patch.insert(
                "request_id".to_string(),
                serde_json::Value::String(request_id.clone()),
            );
        }

        serde_json::Value::Object(patch)
    }
}

#[derive(Debug, Clone)]
struct ClaudeJsonlSemantic {
    line_type: String,
    uuid: Option<String>,
    session_id: Option<String>,
}

impl ClaudeJsonlSemantic {
    fn from_source_event(event: &SourceEventV2) -> Option<Self> {
        let raw = event.payload.get("claude_jsonl");
        let line_type = raw
            .and_then(|value| value.get("line_type"))
            .or_else(|| event.payload.get("line_type"))
            .and_then(serde_json::Value::as_str)?
            .to_string();

        Some(Self {
            line_type,
            uuid: raw
                .and_then(|value| value.get("uuid"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            session_id: raw
                .and_then(|value| value.get("session_id"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
        })
    }

    fn provider_raw_patch(&self) -> serde_json::Value {
        let mut patch = serde_json::Map::from_iter([(
            "jsonl_hint".to_string(),
            serde_json::Value::String(self.line_type.clone()),
        )]);

        if let Some(uuid) = &self.uuid {
            patch.insert(
                "jsonl_uuid".to_string(),
                serde_json::Value::String(uuid.clone()),
            );
        }
        if let Some(session_id) = &self.session_id {
            patch.insert(
                "jsonl_session_id".to_string(),
                serde_json::Value::String(session_id.clone()),
            );
        }

        serde_json::Value::Object(patch)
    }
}

fn synth_approval_request_id(session_key: &str, semantic: &ClaudeHookSemantic) -> String {
    let tool_component = semantic
        .tool_name
        .as_deref()
        .map(sanitize_request_component)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "approval".to_string());

    format!("claude-approval:{session_key}:{tool_component}")
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

fn is_shell_tool(tool_name: &str) -> bool {
    matches!(
        tool_name.to_ascii_lowercase().as_str(),
        "bash" | "shell" | "shell_command"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use agtmux_core_v5::sync_v3::{
        AgentStateV3, AttentionPriorityV3, AttentionSummaryV3, FreshnessSummaryV3, PresenceV3,
        ProviderRawEnvelopeV3, SyncV3PaneSnapshot, ThreadBlockingV3, ThreadFlagsV3, ThreadStateV3,
        TurnStateV3,
    };
    use agtmux_core_v5::types::{ActivityState, EvidenceTier, PaneInstanceId};
    use chrono::TimeZone;

    fn ts(second: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 3, 9, 22, 0, second)
            .single()
            .expect("valid timestamp")
    }

    fn base_snapshot() -> SyncV3PaneSnapshot {
        SyncV3PaneSnapshot {
            session_name: "review".to_string(),
            window_id: "@7".to_string(),
            session_key: "claude:%34".to_string(),
            pane_id: "%34".to_string(),
            pane_instance_id: PaneInstanceId {
                pane_id: "%34".to_string(),
                generation: 2,
                birth_ts: ts(0),
            },
            provider: Some(Provider::Claude),
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
                    sequence: None,
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

    fn claude_hook_event(
        hook_type: &str,
        actual_activity_at: DateTime<Utc>,
        data: serde_json::Value,
    ) -> SourceEventV2 {
        claude_hook_event_with_activity_state(
            ActivityState::Unknown,
            hook_type,
            actual_activity_at,
            data,
        )
    }

    fn claude_hook_event_with_activity_state(
        activity_state: ActivityState,
        hook_type: &str,
        actual_activity_at: DateTime<Utc>,
        data: serde_json::Value,
    ) -> SourceEventV2 {
        let mut payload = match data {
            serde_json::Value::Object(map) => serde_json::Value::Object(map),
            other => serde_json::json!({ "raw_data": other }),
        };
        let notification_type = payload
            .get("notification_type")
            .and_then(serde_json::Value::as_str);
        let tool_name = payload.get("tool_name").and_then(serde_json::Value::as_str);
        let request_id = payload
            .get("request_id")
            .and_then(serde_json::Value::as_str);
        payload["claude_hook"] = serde_json::json!({
            "hook_id": format!("hook-{}", actual_activity_at.timestamp()),
            "hook_type": hook_type,
            "session_id": "claude-session-1",
            "timestamp": actual_activity_at,
            "notification_type": notification_type,
            "tool_name": tool_name,
            "request_id": request_id,
        });

        SourceEventV2 {
            event_id: format!("claude-hook-{hook_type}-{}", actual_activity_at.timestamp()),
            provider: Provider::Claude,
            source_kind: SourceKind::ClaudeHooks,
            tier: EvidenceTier::Deterministic,
            observed_at: actual_activity_at,
            session_key: "claude:%34".to_string(),
            pane_id: Some("%34".to_string()),
            pane_generation: Some(2),
            pane_birth_ts: Some(ts(0)),
            source_event_id: Some(format!("hook-{}", actual_activity_at.timestamp())),
            activity_state,
            payload,
            confidence: 1.0,
            is_heartbeat: false,
            actual_activity_at: Some(actual_activity_at),
        }
    }

    fn claude_jsonl_event(line_type: &str, actual_activity_at: DateTime<Utc>) -> SourceEventV2 {
        claude_jsonl_event_with_activity_state(
            ActivityState::Unknown,
            line_type,
            actual_activity_at,
        )
    }

    fn claude_jsonl_event_with_activity_state(
        activity_state: ActivityState,
        line_type: &str,
        actual_activity_at: DateTime<Utc>,
    ) -> SourceEventV2 {
        SourceEventV2 {
            event_id: format!(
                "claude-jsonl-{line_type}-{}",
                actual_activity_at.timestamp()
            ),
            provider: Provider::Claude,
            source_kind: SourceKind::ClaudeJsonl,
            tier: EvidenceTier::Deterministic,
            observed_at: actual_activity_at,
            session_key: "claude:%34".to_string(),
            pane_id: Some("%34".to_string()),
            pane_generation: Some(2),
            pane_birth_ts: Some(ts(0)),
            source_event_id: Some(format!("uuid-{}", actual_activity_at.timestamp())),
            activity_state,
            payload: serde_json::json!({
                "line_type": line_type,
                "claude_jsonl": {
                    "line_type": line_type,
                    "timestamp": actual_activity_at,
                    "uuid": format!("uuid-{}", actual_activity_at.timestamp()),
                    "session_id": "claude-session-1"
                }
            }),
            confidence: 1.0,
            is_heartbeat: false,
            actual_activity_at: Some(actual_activity_at),
        }
    }

    #[test]
    fn permission_request_opens_pending_approval_from_hook_truth() {
        let mut reducer = SyncV3Reducer::new(base_snapshot());
        let event = claude_hook_event(
            "PermissionRequest",
            ts(10),
            serde_json::json!({
                "tool_name": "Bash",
                "request_id": "req-123"
            }),
        );

        assert!(apply_claude_source_event(&mut reducer, &event));
        assert_eq!(
            reducer.snapshot().thread.blocking,
            ThreadBlockingV3::WaitingApproval
        );
        assert_eq!(
            reducer.snapshot().thread.execution,
            ThreadExecutionV3::Thinking
        );
        assert_eq!(reducer.snapshot().pending_requests.len(), 1);
        assert_eq!(reducer.snapshot().pending_requests[0].request_id, "req-123");
        assert_eq!(
            reducer.snapshot().pending_requests[0].kind,
            PendingRequestKindV3::Approval
        );
        assert_eq!(
            reducer.snapshot().attention.highest_priority,
            AttentionPriorityV3::Approval
        );
        let provider_raw = reducer
            .snapshot()
            .provider_raw
            .claude
            .as_ref()
            .expect("claude provider raw should be present");
        assert_eq!(provider_raw["hook_event"], "PermissionRequest");
        assert_eq!(provider_raw["hook_source"], "claude_hooks");
        assert_eq!(provider_raw["tool_name"], "Bash");
        assert_eq!(provider_raw["request_id"], "req-123");
    }

    #[test]
    fn stop_resolves_pending_approval_without_inventing_waiting_user_input() {
        let mut reducer = SyncV3Reducer::new(base_snapshot());
        let approval = claude_hook_event(
            "PermissionRequest",
            ts(11),
            serde_json::json!({"tool_name": "Bash", "request_id": "req-123"}),
        );
        let stop = claude_hook_event("Stop", ts(12), serde_json::json!({}));

        assert!(apply_claude_source_event(&mut reducer, &approval));
        assert!(apply_claude_source_event(&mut reducer, &stop));
        assert_eq!(reducer.snapshot().thread.lifecycle, ThreadLifecycleV3::Idle);
        assert_eq!(reducer.snapshot().thread.blocking, ThreadBlockingV3::None);
        assert_eq!(reducer.snapshot().thread.execution, ThreadExecutionV3::None);
        assert!(reducer.snapshot().pending_requests.is_empty());
        assert_eq!(
            reducer.snapshot().thread.turn.outcome,
            TurnOutcomeV3::Completed
        );
        assert_eq!(reducer.snapshot().thread.turn.completed_at, Some(ts(12)));
        assert_eq!(
            reducer.snapshot().attention.highest_priority,
            AttentionPriorityV3::Completion
        );
    }

    #[test]
    fn jsonl_tool_use_updates_execution_without_overriding_hook_blocking_truth() {
        let mut snapshot = base_snapshot();
        snapshot.thread.execution = ThreadExecutionV3::None;
        let mut reducer = SyncV3Reducer::new(snapshot);
        let approval = claude_hook_event(
            "PermissionRequest",
            ts(13),
            serde_json::json!({"tool_name": "Bash", "request_id": "req-123"}),
        );
        let tool_use = claude_jsonl_event("tool_use", ts(14));

        assert!(apply_claude_source_event(&mut reducer, &approval));
        assert!(apply_claude_source_event(&mut reducer, &tool_use));
        assert_eq!(
            reducer.snapshot().thread.blocking,
            ThreadBlockingV3::WaitingApproval
        );
        assert_eq!(
            reducer.snapshot().thread.execution,
            ThreadExecutionV3::ToolRunning
        );
        assert_eq!(reducer.snapshot().pending_requests.len(), 1);
        assert_eq!(
            reducer.snapshot().attention.highest_priority,
            AttentionPriorityV3::Approval
        );
        let provider_raw = reducer
            .snapshot()
            .provider_raw
            .claude
            .as_ref()
            .expect("claude provider raw should be present");
        assert_eq!(provider_raw["hook_event"], "PermissionRequest");
        assert_eq!(provider_raw["hook_source"], "claude_hooks");
        assert_eq!(provider_raw["tool_name"], "Bash");
        assert_eq!(provider_raw["request_id"], "req-123");
        assert_eq!(provider_raw["jsonl_hint"], "tool_use");
        assert_eq!(provider_raw["jsonl_session_id"], "claude-session-1");
        assert_eq!(
            provider_raw["jsonl_uuid"],
            format!("uuid-{}", ts(14).timestamp())
        );
    }

    #[test]
    fn notification_waiting_input_does_not_invent_user_input_request() {
        let mut reducer = SyncV3Reducer::new(base_snapshot());
        let event = claude_hook_event(
            "Notification",
            ts(15),
            serde_json::json!({"notification_type": "idle_prompt"}),
        );

        assert!(!apply_claude_source_event(&mut reducer, &event));
        assert_eq!(reducer.snapshot().thread.blocking, ThreadBlockingV3::None);
        assert!(reducer.snapshot().pending_requests.is_empty());
        assert_eq!(
            reducer.snapshot().attention.highest_priority,
            AttentionPriorityV3::None
        );
    }

    #[test]
    fn subagent_stop_reuses_stop_completion_mapping() {
        let mut reducer = SyncV3Reducer::new(base_snapshot());
        let event = claude_hook_event("SubagentStop", ts(16), serde_json::json!({}));

        assert!(apply_claude_source_event(&mut reducer, &event));
        assert_eq!(reducer.snapshot().thread.lifecycle, ThreadLifecycleV3::Idle);
        assert_eq!(
            reducer.snapshot().thread.turn.outcome,
            TurnOutcomeV3::Completed
        );
    }

    #[test]
    fn permission_request_ignores_contradictory_compat_event_type_when_hook_payload_exists() {
        let mut reducer = SyncV3Reducer::new(base_snapshot());
        let event = claude_hook_event_with_activity_state(
            ActivityState::Idle,
            "PermissionRequest",
            ts(17),
            serde_json::json!({
                "tool_name": "Bash",
                "request_id": "req-456"
            }),
        );

        assert!(apply_claude_source_event(&mut reducer, &event));
        assert_eq!(
            reducer.snapshot().thread.blocking,
            ThreadBlockingV3::WaitingApproval
        );
        assert_eq!(reducer.snapshot().pending_requests.len(), 1);
        assert_eq!(reducer.snapshot().pending_requests[0].request_id, "req-456");
    }

    #[test]
    fn jsonl_tool_use_ignores_contradictory_compat_event_type_when_payload_exists() {
        let mut reducer = SyncV3Reducer::new(base_snapshot());
        let event = claude_jsonl_event_with_activity_state(ActivityState::Idle, "tool_use", ts(18));

        assert!(apply_claude_source_event(&mut reducer, &event));
        assert_eq!(
            reducer.snapshot().thread.execution,
            ThreadExecutionV3::ToolRunning
        );
        assert_eq!(
            reducer.snapshot().thread.lifecycle,
            ThreadLifecycleV3::Active
        );
    }
}

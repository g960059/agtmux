//! Event translation from Claude hooks format to [`SourceEventV2`].

use agtmux_core_v5::{
    sync_v2_compat,
    types::{EvidenceTier, Provider, SourceEventV2, SourceKind},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Raw Claude hooks lifecycle event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaudeHookEvent {
    /// Hook event ID.
    pub hook_id: String,
    /// Hook type: "session_start", "session_end", "tool_start", "tool_end",
    /// "thinking", "idle", "error".
    pub hook_type: String,
    /// Session ID from Claude Code.
    pub session_id: String,
    /// Timestamp.
    pub timestamp: DateTime<Utc>,
    /// Optional pane ID (from environment).
    pub pane_id: Option<String>,
    /// Hook-specific data.
    pub data: serde_json::Value,
}

/// Translate a Claude hook event to a [`SourceEventV2`].
pub fn translate(raw: &ClaudeHookEvent) -> SourceEventV2 {
    SourceEventV2 {
        event_id: format!("claude-hooks-{}", raw.hook_id),
        provider: Provider::Claude,
        source_kind: SourceKind::ClaudeHooks,
        tier: EvidenceTier::Deterministic,
        observed_at: raw.timestamp,
        session_key: raw.session_id.clone(),
        pane_id: raw.pane_id.clone(),
        pane_generation: None,
        pane_birth_ts: None,
        source_event_id: Some(raw.hook_id.clone()),
        event_type: resolve_event_type(&raw.hook_type, &raw.data),
        payload: build_payload(raw),
        confidence: 1.0,
        is_heartbeat: false, // Claude hooks are always real activity (not periodic keep-alive)
        actual_activity_at: Some(raw.timestamp),
    }
}

/// Map Claude hook types to normalized event_type strings.
fn normalize_event_type(hook_type: &str) -> String {
    sync_v2_compat::claude_hook_event_type(hook_type).to_owned()
}

/// Resolve event_type, taking hook payload into account for hooks that
/// require data-level dispatch (e.g. `Notification`).
///
/// For all other hook types the call is forwarded to [`normalize_event_type`].
fn resolve_event_type(hook_type: &str, data: &serde_json::Value) -> String {
    if hook_type == "Notification" {
        sync_v2_compat::claude_notification_event_type(
            data.get("notification_type")
                .and_then(serde_json::Value::as_str),
        )
        .to_owned()
    } else {
        normalize_event_type(hook_type)
    }
}

fn build_payload(raw: &ClaudeHookEvent) -> serde_json::Value {
    let mut payload = match raw.data.clone() {
        serde_json::Value::Object(map) => serde_json::Value::Object(map),
        other => serde_json::json!({ "raw_data": other }),
    };

    payload["claude_hook"] = serde_json::json!({
        "hook_id": raw.hook_id,
        "hook_type": raw.hook_type,
        "session_id": raw.session_id,
        "timestamp": raw.timestamp,
        "notification_type": raw.data.get("notification_type").and_then(serde_json::Value::as_str),
        "tool_name": extract_string_field(&raw.data, &["tool_name", "toolName", "tool"]),
        "request_id": extract_string_field(
            &raw.data,
            &["request_id", "requestId", "tool_use_id", "toolUseId", "permission_request_id"]
        )
    });

    payload
}

fn extract_string_field<'a>(value: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(serde_json::Value::as_str))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn sample_event(hook_type: &str, pane_id: Option<&str>) -> ClaudeHookEvent {
        ClaudeHookEvent {
            hook_id: "h-001".to_owned(),
            hook_type: hook_type.to_owned(),
            session_id: "sess-abc".to_owned(),
            timestamp: Utc
                .with_ymd_and_hms(2026, 1, 15, 10, 0, 0)
                .single()
                .expect("valid datetime"),
            pane_id: pane_id.map(String::from),
            data: serde_json::json!({"tool": "bash"}),
        }
    }

    #[test]
    fn single_event_translation_all_fields() {
        let raw = sample_event("tool_start", Some("%3"));
        let ev = translate(&raw);

        assert_eq!(ev.event_id, "claude-hooks-h-001");
        assert_eq!(ev.provider, Provider::Claude);
        assert_eq!(ev.source_kind, SourceKind::ClaudeHooks);
        assert_eq!(ev.tier, EvidenceTier::Deterministic);
        assert_eq!(ev.observed_at, raw.timestamp);
        assert_eq!(ev.session_key, "sess-abc");
        assert_eq!(ev.pane_id, Some("%3".to_owned()));
        assert_eq!(ev.event_type, "lifecycle.running");
        assert_eq!(ev.payload["tool"], "bash");
        assert_eq!(ev.payload["claude_hook"]["hook_type"], "tool_start");
        assert_eq!(ev.payload["claude_hook"]["tool_name"], "bash");
        assert!((ev.confidence - 1.0).abs() < f64::EPSILON);
        assert_eq!(ev.source_event_id, Some("h-001".to_owned()));
        assert!(ev.pane_generation.is_none());
        assert!(ev.pane_birth_ts.is_none());
        assert_eq!(ev.actual_activity_at, Some(raw.timestamp));
    }

    #[test]
    fn event_type_normalization() {
        let cases = [
            ("session_start", "lifecycle.start"),
            ("SessionStart", "lifecycle.start"),
            ("session_end", "lifecycle.end"),
            ("SessionEnd", "lifecycle.end"),
            ("tool_start", "lifecycle.running"),
            ("thinking", "lifecycle.running"),
            ("tool_end", "lifecycle.idle"),
            ("idle", "lifecycle.idle"),
            ("error", "lifecycle.error"),
            ("PermissionRequest", "activity.waiting_approval"),
            ("UserPromptSubmit", "activity.user_input"),
            ("PostToolUseFailure", "lifecycle.error"),
            ("PreCompact", "lifecycle.compacting"),
            ("Stop", "activity.waiting_input"),
            ("SubagentStop", "activity.waiting_input"),
            ("PreToolUse", "activity.running"),
            ("PostToolUse", "activity.running"),
        ];
        for (hook_type, expected) in cases {
            let raw = sample_event(hook_type, None);
            let ev = translate(&raw);
            assert_eq!(
                ev.event_type, expected,
                "hook_type={hook_type} should map to {expected}"
            );
        }
    }

    #[test]
    fn unknown_hook_type_maps_to_lifecycle_unknown() {
        let raw = sample_event("some_future_hook", None);
        let ev = translate(&raw);
        assert_eq!(ev.event_type, "lifecycle.unknown");
    }

    #[test]
    fn deterministic_tier_and_confidence() {
        let raw = sample_event("session_start", None);
        let ev = translate(&raw);
        assert_eq!(ev.tier, EvidenceTier::Deterministic);
        assert!((ev.confidence - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn pane_id_optional_handling() {
        let with_pane = sample_event("idle", Some("%5"));
        let ev_with = translate(&with_pane);
        assert_eq!(ev_with.pane_id, Some("%5".to_owned()));

        let without_pane = sample_event("idle", None);
        let ev_without = translate(&without_pane);
        assert!(ev_without.pane_id.is_none());
    }

    // ── T-E05: Notification hook dispatch ────────────────────────────────────

    fn notification_event(notification_type: Option<&str>) -> ClaudeHookEvent {
        let data = match notification_type {
            Some(nt) => serde_json::json!({ "notification_type": nt }),
            None => serde_json::json!({}),
        };
        ClaudeHookEvent {
            hook_id: "h-notif".to_owned(),
            hook_type: "Notification".to_owned(),
            session_id: "sess-notif".to_owned(),
            timestamp: Utc
                .with_ymd_and_hms(2026, 1, 15, 10, 0, 0)
                .single()
                .expect("valid datetime"),
            pane_id: None,
            data,
        }
    }

    #[test]
    fn notification_idle_prompt() {
        let raw = notification_event(Some("idle_prompt"));
        let ev = translate(&raw);
        assert_eq!(ev.event_type, "activity.waiting_input");
    }

    #[test]
    fn notification_permission_prompt() {
        let raw = notification_event(Some("permission_prompt"));
        let ev = translate(&raw);
        assert_eq!(ev.event_type, "activity.waiting_approval");
        assert_eq!(
            ev.payload["claude_hook"]["notification_type"],
            "permission_prompt"
        );
    }

    #[test]
    fn notification_unknown_type() {
        let raw = notification_event(Some("some_future_notification"));
        let ev = translate(&raw);
        assert_eq!(ev.event_type, "lifecycle.notification");
    }

    #[test]
    fn notification_no_type_field() {
        let raw = notification_event(None);
        let ev = translate(&raw);
        assert_eq!(ev.event_type, "lifecycle.notification");
    }

    // ── T-E06: PreToolUse / PostToolUse mapping ───────────────────────────────

    #[test]
    fn notification_non_string_type_field() {
        // notification_type present but not a string (e.g. integer or null) → lifecycle.notification
        let mut raw = notification_event(None);
        raw.data = serde_json::json!({ "notification_type": 42 });
        let ev = translate(&raw);
        assert_eq!(ev.event_type, "lifecycle.notification");
    }

    #[test]
    fn notification_null_type_field() {
        let mut raw = notification_event(None);
        raw.data = serde_json::json!({ "notification_type": null });
        let ev = translate(&raw);
        assert_eq!(ev.event_type, "lifecycle.notification");
    }

    // ── T-E06: PreToolUse / PostToolUse mapping ───────────────────────────────

    #[test]
    fn pre_tool_use_maps_to_running() {
        let raw = sample_event("PreToolUse", None);
        let ev = translate(&raw);
        assert_eq!(ev.event_type, "activity.running");
        assert_eq!(ev.payload["claude_hook"]["hook_type"], "PreToolUse");
    }

    #[test]
    fn post_tool_use_maps_to_running() {
        let raw = sample_event("PostToolUse", None);
        let ev = translate(&raw);
        assert_eq!(ev.event_type, "activity.running");
        assert_eq!(ev.payload["claude_hook"]["hook_type"], "PostToolUse");
    }

    #[test]
    fn permission_request_payload_preserves_hook_metadata() {
        let mut raw = sample_event("PermissionRequest", Some("%4"));
        raw.data = serde_json::json!({
            "tool_name": "Bash",
            "request_id": "req-123",
            "description": "Run npm install"
        });
        let ev = translate(&raw);
        assert_eq!(ev.event_type, "activity.waiting_approval");
        assert_eq!(ev.payload["claude_hook"]["hook_type"], "PermissionRequest");
        assert_eq!(ev.payload["claude_hook"]["tool_name"], "Bash");
        assert_eq!(ev.payload["claude_hook"]["request_id"], "req-123");
        assert_eq!(ev.payload["description"], "Run npm install");
    }

    #[test]
    fn stop_payload_preserves_hook_type_for_v3_normalization() {
        let raw = sample_event("Stop", Some("%4"));
        let ev = translate(&raw);
        assert_eq!(ev.event_type, "activity.waiting_input");
        assert_eq!(ev.payload["claude_hook"]["hook_type"], "Stop");
    }
}

use crate::types::ActivityState;

pub const ACTIVITY_USER_INPUT_EVENT_TYPE: &str = "activity.user_input";
pub const ACTIVITY_TOOL_COMPLETE_EVENT_TYPE: &str = "activity.tool_complete";

/// Legacy sync-v2-compatible collapsed activity namespace.
///
/// This exists for the old projection / replay / source transport boundary only.
/// Sync-v3 truth must continue to derive semantics from provider-native payloads.
pub fn activity_event_type(state: ActivityState) -> &'static str {
    match state {
        ActivityState::Running => "activity.running",
        ActivityState::Idle => "activity.idle",
        ActivityState::WaitingInput => "activity.waiting_input",
        ActivityState::WaitingApproval => "activity.waiting_approval",
        ActivityState::Error => "activity.error",
        _ => "activity.unknown",
    }
}

/// Legacy sync-v2-compatible mapping for Claude hook types.
pub fn claude_hook_event_type(hook_type: &str) -> &'static str {
    match hook_type {
        "session_start" | "SessionStart" => "lifecycle.start",
        "session_end" | "SessionEnd" => "lifecycle.end",
        "tool_start" | "thinking" => "lifecycle.running",
        "tool_end" | "idle" => "lifecycle.idle",
        "error" | "PostToolUseFailure" => "lifecycle.error",
        "PreCompact" => "lifecycle.compacting",
        "PermissionRequest" => activity_event_type(ActivityState::WaitingApproval),
        "UserPromptSubmit" => ACTIVITY_USER_INPUT_EVENT_TYPE,
        "Stop" | "SubagentStop" => activity_event_type(ActivityState::WaitingInput),
        "PreToolUse" | "PostToolUse" => activity_event_type(ActivityState::Running),
        _ => "lifecycle.unknown",
    }
}

/// Legacy sync-v2-compatible mapping for Claude notification subtypes.
pub fn claude_notification_event_type(notification_type: Option<&str>) -> &'static str {
    match notification_type {
        Some("idle_prompt") => activity_event_type(ActivityState::WaitingInput),
        Some("permission_prompt") => activity_event_type(ActivityState::WaitingApproval),
        Some(_) | None => "lifecycle.notification",
    }
}

/// Legacy sync-v2-compatible mapping for Claude JSONL line types.
pub fn claude_jsonl_event_type(line_type: &str) -> Option<&'static str> {
    match line_type {
        "user" => Some(ACTIVITY_USER_INPUT_EVENT_TYPE),
        "tool_use" => Some(activity_event_type(ActivityState::Running)),
        "tool_result" => Some(ACTIVITY_TOOL_COMPLETE_EVENT_TYPE),
        "assistant" => Some(activity_event_type(ActivityState::Idle)),
        "progress" => Some(activity_event_type(ActivityState::Running)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_event_type_matches_legacy_strings() {
        let cases = [
            (ActivityState::Running, "activity.running"),
            (ActivityState::Idle, "activity.idle"),
            (ActivityState::WaitingInput, "activity.waiting_input"),
            (ActivityState::WaitingApproval, "activity.waiting_approval"),
            (ActivityState::Error, "activity.error"),
            (ActivityState::Unknown, "activity.unknown"),
        ];

        for (state, expected) in cases {
            assert_eq!(activity_event_type(state), expected);
        }
    }

    #[test]
    fn claude_hook_event_type_matches_legacy_strings() {
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
            ("future_hook", "lifecycle.unknown"),
        ];

        for (hook_type, expected) in cases {
            assert_eq!(claude_hook_event_type(hook_type), expected);
        }
    }

    #[test]
    fn claude_notification_event_type_matches_legacy_strings() {
        assert_eq!(
            claude_notification_event_type(Some("idle_prompt")),
            "activity.waiting_input"
        );
        assert_eq!(
            claude_notification_event_type(Some("permission_prompt")),
            "activity.waiting_approval"
        );
        assert_eq!(
            claude_notification_event_type(Some("some_future_notification")),
            "lifecycle.notification"
        );
        assert_eq!(
            claude_notification_event_type(None),
            "lifecycle.notification"
        );
    }

    #[test]
    fn claude_jsonl_event_type_matches_legacy_strings() {
        let cases = [
            ("user", Some("activity.user_input")),
            ("tool_use", Some("activity.running")),
            ("tool_result", Some("activity.tool_complete")),
            ("assistant", Some("activity.idle")),
            ("progress", Some("activity.running")),
            ("system", None),
        ];

        for (line_type, expected) in cases {
            assert_eq!(claude_jsonl_event_type(line_type), expected);
        }
    }
}

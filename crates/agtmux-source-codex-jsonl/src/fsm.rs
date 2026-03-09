//! Finite state machine for Codex JSONL session state tracking.
//!
//! States are derived from semantic events in the JSONL file,
//! NOT from mtime or file-growth heuristics.

use chrono::{DateTime, Utc};
use serde::Deserialize;

/// FSM states for a Codex session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CodexSessionState {
    /// Session discovered, no events processed yet.
    #[default]
    Init,
    /// task_started received — Codex is actively processing.
    Running,
    /// function_call received, awaiting function_call_output.
    ToolExecuting,
    /// entered_review_mode received — waiting for user approval.
    WaitingApproval,
    /// task_complete received, process still alive — waiting for next input.
    WaitingInput,
    /// Process exited OR staleness timeout (>600s since task_complete).
    Ended,
}

/// A parsed Codex JSONL event for FSM processing.
///
/// Top-level JSONL line types: "event_msg", "response_item", "turn_context", "session_meta", "compacted"
/// Inner type lives at `.payload.type`.
#[derive(Debug, Clone, Deserialize)]
pub struct CodexJsonlEvent {
    /// RFC3339 timestamp for the JSONL record.
    #[serde(default)]
    pub timestamp: Option<String>,
    /// Top-level line type (event_msg, response_item, session_meta, etc.)
    #[serde(rename = "type")]
    pub top_type: String,
    /// The payload object containing the inner event type.
    #[serde(default)]
    pub payload: serde_json::Value,
}

impl CodexJsonlEvent {
    /// Extract the inner event type from `.payload.type`.
    pub fn inner_type(&self) -> Option<&str> {
        self.payload["type"].as_str()
    }

    /// Parse the top-level timestamp when present.
    pub fn timestamp(&self) -> Option<DateTime<Utc>> {
        self.timestamp
            .as_deref()
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|ts| ts.with_timezone(&Utc))
    }

    /// Borrow the raw top-level timestamp string when present.
    pub fn timestamp_str(&self) -> Option<&str> {
        self.timestamp.as_deref()
    }

    /// Extract the working directory from a session_meta event (`.payload.cwd`).
    pub fn session_cwd(&self) -> Option<&str> {
        if self.top_type == "session_meta" {
            self.payload["cwd"].as_str()
        } else {
            None
        }
    }

    /// Extract the turn identifier from `.payload.turn_id`.
    pub fn turn_id(&self) -> Option<&str> {
        self.payload["turn_id"].as_str()
    }

    /// Extract the call identifier from `.payload.call_id`.
    pub fn call_id(&self) -> Option<&str> {
        self.payload["call_id"].as_str()
    }

    /// Extract the tool name from `.payload.name`.
    pub fn tool_name(&self) -> Option<&str> {
        self.payload["name"].as_str()
    }

    /// Extract the abort reason from `.payload.reason`.
    pub fn abort_reason(&self) -> Option<&str> {
        self.payload["reason"].as_str()
    }

    /// Extract a review target type from `.payload.target.type`.
    pub fn review_target_type(&self) -> Option<&str> {
        self.payload["target"]["type"].as_str()
    }

    /// Extract the user-facing review hint from `.payload.user_facing_hint`.
    pub fn user_facing_hint(&self) -> Option<&str> {
        self.payload["user_facing_hint"].as_str()
    }

    /// Extract a request identity if the provider payload exposes one directly.
    pub fn request_id(&self) -> Option<&str> {
        self.payload["request_id"]
            .as_str()
            .or_else(|| self.payload["requestId"].as_str())
    }
}

/// Apply FSM transition for a single JSONL event.
///
/// Returns the new state (may be same as current if no transition applies).
pub fn transition(state: CodexSessionState, event: &CodexJsonlEvent) -> CodexSessionState {
    use CodexSessionState::*;
    let inner = event.inner_type().unwrap_or("");
    match (state, event.top_type.as_str(), inner) {
        // task lifecycle — task_started fires Running from any state
        (_, "event_msg", "task_started") => Running,

        // task_complete / turn_aborted → WaitingInput (only from Running/ToolExecuting)
        (Running | ToolExecuting, "event_msg", "task_complete") => WaitingInput,
        (Running | ToolExecuting, "event_msg", "turn_aborted") => WaitingInput,

        // WaitingInput → Running on user message (task_started already handled by wildcard above)
        (WaitingInput, "event_msg", "user_message") => Running,

        // tool execution
        (Running, "response_item", "function_call") => ToolExecuting,
        (Running, "response_item", "custom_tool_call") => ToolExecuting,
        (ToolExecuting, "response_item", "function_call_output") => Running,
        (ToolExecuting, "response_item", "custom_tool_call_output") => Running,

        // approval flow — entered_review_mode can come from any active state
        (_, "event_msg", "entered_review_mode") => WaitingApproval,
        (WaitingApproval, "event_msg", "exited_review_mode") => Running,

        // no state change
        _ => state,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(top: &str, inner: &str) -> CodexJsonlEvent {
        CodexJsonlEvent {
            timestamp: None,
            top_type: top.to_owned(),
            payload: serde_json::json!({"type": inner}),
        }
    }

    fn ev_top(top: &str) -> CodexJsonlEvent {
        CodexJsonlEvent {
            timestamp: None,
            top_type: top.to_owned(),
            payload: serde_json::Value::Null,
        }
    }

    #[test]
    fn init_to_running_on_task_started() {
        let state = transition(CodexSessionState::Init, &ev("event_msg", "task_started"));
        assert_eq!(state, CodexSessionState::Running);
    }

    #[test]
    fn running_to_tool_executing_on_function_call() {
        let state = transition(
            CodexSessionState::Running,
            &ev("response_item", "function_call"),
        );
        assert_eq!(state, CodexSessionState::ToolExecuting);
    }

    #[test]
    fn running_to_tool_executing_on_custom_tool_call() {
        let state = transition(
            CodexSessionState::Running,
            &ev("response_item", "custom_tool_call"),
        );
        assert_eq!(state, CodexSessionState::ToolExecuting);
    }

    #[test]
    fn tool_executing_to_running_on_function_call_output() {
        let state = transition(
            CodexSessionState::ToolExecuting,
            &ev("response_item", "function_call_output"),
        );
        assert_eq!(state, CodexSessionState::Running);
    }

    #[test]
    fn tool_executing_to_running_on_custom_tool_call_output() {
        let state = transition(
            CodexSessionState::ToolExecuting,
            &ev("response_item", "custom_tool_call_output"),
        );
        assert_eq!(state, CodexSessionState::Running);
    }

    #[test]
    fn running_to_waiting_input_on_task_complete() {
        let state = transition(
            CodexSessionState::Running,
            &ev("event_msg", "task_complete"),
        );
        assert_eq!(state, CodexSessionState::WaitingInput);
    }

    #[test]
    fn tool_executing_to_waiting_input_on_task_complete() {
        let state = transition(
            CodexSessionState::ToolExecuting,
            &ev("event_msg", "task_complete"),
        );
        assert_eq!(state, CodexSessionState::WaitingInput);
    }

    #[test]
    fn running_to_waiting_input_on_turn_aborted() {
        let state = transition(CodexSessionState::Running, &ev("event_msg", "turn_aborted"));
        assert_eq!(state, CodexSessionState::WaitingInput);
    }

    #[test]
    fn waiting_input_to_running_on_task_started() {
        let state = transition(
            CodexSessionState::WaitingInput,
            &ev("event_msg", "task_started"),
        );
        assert_eq!(state, CodexSessionState::Running);
    }

    #[test]
    fn waiting_input_to_running_on_user_message() {
        let state = transition(
            CodexSessionState::WaitingInput,
            &ev("event_msg", "user_message"),
        );
        assert_eq!(state, CodexSessionState::Running);
    }

    #[test]
    fn running_to_waiting_approval_on_entered_review_mode() {
        let state = transition(
            CodexSessionState::Running,
            &ev("event_msg", "entered_review_mode"),
        );
        assert_eq!(state, CodexSessionState::WaitingApproval);
    }

    #[test]
    fn tool_executing_to_waiting_approval_on_entered_review_mode() {
        let state = transition(
            CodexSessionState::ToolExecuting,
            &ev("event_msg", "entered_review_mode"),
        );
        assert_eq!(state, CodexSessionState::WaitingApproval);
    }

    #[test]
    fn waiting_approval_to_running_on_exited_review_mode() {
        let state = transition(
            CodexSessionState::WaitingApproval,
            &ev("event_msg", "exited_review_mode"),
        );
        assert_eq!(state, CodexSessionState::Running);
    }

    #[test]
    fn task_started_transitions_from_any_state() {
        for state in [
            CodexSessionState::Init,
            CodexSessionState::Running,
            CodexSessionState::ToolExecuting,
            CodexSessionState::WaitingApproval,
            CodexSessionState::WaitingInput,
            CodexSessionState::Ended,
        ] {
            let new_state = transition(state, &ev("event_msg", "task_started"));
            assert_eq!(
                new_state,
                CodexSessionState::Running,
                "task_started should always → Running from {state:?}"
            );
        }
    }

    #[test]
    fn token_count_no_transition() {
        let state = transition(CodexSessionState::Running, &ev("event_msg", "token_count"));
        assert_eq!(state, CodexSessionState::Running);
    }

    #[test]
    fn agent_reasoning_no_transition() {
        let state = transition(
            CodexSessionState::Running,
            &ev("event_msg", "agent_reasoning"),
        );
        assert_eq!(state, CodexSessionState::Running);
    }

    #[test]
    fn context_compacted_no_transition() {
        let state = transition(
            CodexSessionState::Running,
            &ev("event_msg", "context_compacted"),
        );
        assert_eq!(state, CodexSessionState::Running);
    }

    #[test]
    fn unknown_event_no_transition() {
        let state = transition(CodexSessionState::Running, &ev_top("unknown_top_level"));
        assert_eq!(state, CodexSessionState::Running);
    }

    #[test]
    fn task_complete_from_waiting_input_no_transition() {
        // task_complete while already WaitingInput should NOT re-enter WaitingInput or change state
        let state = transition(
            CodexSessionState::WaitingInput,
            &ev("event_msg", "task_complete"),
        );
        assert_eq!(state, CodexSessionState::WaitingInput);
    }

    #[test]
    fn session_meta_cwd_extraction() {
        let event = CodexJsonlEvent {
            timestamp: None,
            top_type: "session_meta".to_owned(),
            payload: serde_json::json!({"type": "session_meta", "cwd": "/Users/vm/project"}),
        };
        assert_eq!(event.session_cwd(), Some("/Users/vm/project"));
    }

    #[test]
    fn parse_real_codex_event_msg_task_started() {
        // Verify the real JSONL format: payload.type (not data.type)
        let line = r#"{"type":"event_msg","payload":{"type":"task_started","taskId":"abc"}}"#;
        let event: CodexJsonlEvent = serde_json::from_str(line).expect("parse");
        assert_eq!(event.top_type, "event_msg");
        assert_eq!(event.inner_type(), Some("task_started"));
    }

    #[test]
    fn parse_real_codex_response_item_function_call() {
        let line = r#"{"type":"response_item","payload":{"type":"function_call","name":"bash","id":"call_abc"}}"#;
        let event: CodexJsonlEvent = serde_json::from_str(line).expect("parse");
        assert_eq!(event.top_type, "response_item");
        assert_eq!(event.inner_type(), Some("function_call"));
    }

    #[test]
    fn parse_real_codex_session_meta() {
        let line = r#"{"type":"session_meta","payload":{"type":"session_meta","cwd":"/Users/vm/project","sessionId":"uuid-123"}}"#;
        let event: CodexJsonlEvent = serde_json::from_str(line).expect("parse");
        assert_eq!(event.top_type, "session_meta");
        assert_eq!(event.session_cwd(), Some("/Users/vm/project"));
    }

    #[test]
    fn fsm_full_task_lifecycle() {
        // Init → Running → ToolExecuting → Running → WaitingInput → Running → WaitingInput
        let events = vec![
            ev("event_msg", "task_started"),
            ev("response_item", "function_call"),
            ev("response_item", "function_call_output"),
            ev("event_msg", "token_count"),
            ev("event_msg", "task_complete"),
            ev("event_msg", "user_message"),
            ev("event_msg", "task_started"),
            ev("event_msg", "task_complete"),
        ];
        let expected = vec![
            CodexSessionState::Running,
            CodexSessionState::ToolExecuting,
            CodexSessionState::Running,
            CodexSessionState::Running, // token_count: no change
            CodexSessionState::WaitingInput,
            CodexSessionState::Running,
            CodexSessionState::Running, // task_started: back to Running
            CodexSessionState::WaitingInput,
        ];

        let mut state = CodexSessionState::Init;
        for (event, exp) in events.iter().zip(expected.iter()) {
            state = transition(state, event);
            assert_eq!(
                &state,
                exp,
                "after event {:?} inner={:?}",
                event.top_type,
                event.inner_type()
            );
        }
    }

    #[test]
    fn fsm_approval_flow() {
        // Running → WaitingApproval → Running → WaitingInput
        let mut state = CodexSessionState::Running;
        state = transition(state, &ev("event_msg", "entered_review_mode"));
        assert_eq!(state, CodexSessionState::WaitingApproval);
        state = transition(state, &ev("event_msg", "exited_review_mode"));
        assert_eq!(state, CodexSessionState::Running);
        state = transition(state, &ev("event_msg", "task_complete"));
        assert_eq!(state, CodexSessionState::WaitingInput);
    }

    #[test]
    fn waiting_approval_task_complete_is_noop() {
        // task_complete from WaitingApproval has no FSM arm → falls through to _ → no-op.
        // This locks in the behaviour: approval-rejected-internally path remains WaitingApproval
        // until exited_review_mode or staleness timeout.
        let state = transition(
            CodexSessionState::WaitingApproval,
            &ev("event_msg", "task_complete"),
        );
        assert_eq!(state, CodexSessionState::WaitingApproval);
    }
}

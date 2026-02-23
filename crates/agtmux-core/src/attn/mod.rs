use crate::types::{ActivityState, AttentionResult, AttentionState};
use chrono::{DateTime, Utc};

const COMPLETION_SIGNALS: &[&str] = &[
    "completed", "done", "finished", "task-finished", "stop", "session_end",
];

const ADMIN_EVENTS: &[&str] = &["wrapper-start", "wrapper-exit"];

fn is_completion_signal(reason_code: &str, last_event_type: &str) -> bool {
    let combined = format!("{} {}", reason_code, last_event_type).to_lowercase();
    COMPLETION_SIGNALS.iter().any(|s| combined.contains(s))
}

fn is_admin_event(last_event_type: &str) -> bool {
    let lower = last_event_type.to_lowercase();
    ADMIN_EVENTS.iter().any(|s| lower.contains(s))
        || (lower.starts_with("action.") && lower != "action.view-output")
}

/// Derive attention state from activity + context. Pure function, no IO.
pub fn derive_attention_state(
    activity: ActivityState,
    reason_code: &str,
    last_event_type: &str,
    updated_at: DateTime<Utc>,
) -> AttentionResult {
    if is_admin_event(last_event_type) {
        return AttentionResult {
            state: AttentionState::None,
            reason: String::new(),
            since: None,
        };
    }

    match activity {
        ActivityState::WaitingInput => AttentionResult {
            state: AttentionState::ActionRequiredInput,
            reason: "waiting_input".into(),
            since: Some(updated_at),
        },
        ActivityState::WaitingApproval => AttentionResult {
            state: AttentionState::ActionRequiredApproval,
            reason: "waiting_approval".into(),
            since: Some(updated_at),
        },
        ActivityState::Error => AttentionResult {
            state: AttentionState::ActionRequiredError,
            reason: "error".into(),
            since: Some(updated_at),
        },
        _ if is_completion_signal(reason_code, last_event_type)
            && !reason_code.to_lowercase().contains("input")
            && !reason_code.to_lowercase().contains("approval") =>
        {
            AttentionResult {
                state: AttentionState::InformationalCompleted,
                reason: "completed".into(),
                since: Some(updated_at),
            }
        }
        _ => AttentionResult {
            state: AttentionState::None,
            reason: String::new(),
            since: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waiting_approval_triggers_attention() {
        let r = derive_attention_state(
            ActivityState::WaitingApproval,
            "needs-approval",
            "needs-approval",
            Utc::now(),
        );
        assert_eq!(r.state, AttentionState::ActionRequiredApproval);
    }

    #[test]
    fn admin_events_no_attention() {
        let r = derive_attention_state(
            ActivityState::Running,
            "",
            "wrapper-start",
            Utc::now(),
        );
        assert_eq!(r.state, AttentionState::None);
    }

    #[test]
    fn completion_informational() {
        let r = derive_attention_state(
            ActivityState::Idle,
            "task-finished",
            "stop",
            Utc::now(),
        );
        assert_eq!(r.state, AttentionState::InformationalCompleted);
    }
}

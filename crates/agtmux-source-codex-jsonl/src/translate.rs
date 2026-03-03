//! Translation from Codex JSONL FSM state to SourceEventV2.

use agtmux_core_v5::types::{EvidenceTier, Provider, SourceEventV2, SourceKind};
use chrono::{DateTime, Utc};

use crate::fsm::CodexSessionState;

/// Translate a Codex FSM state transition into a SourceEventV2.
///
/// Returns `None` for states that don't produce events (e.g. Init, Ended).
pub fn translate_state_change(
    new_state: CodexSessionState,
    session_key: &str,
    pane_id: &str,
    pane_generation: Option<u64>,
    pane_birth_ts: Option<DateTime<Utc>>,
    observed_at: DateTime<Utc>,
    event_seq: u64,
) -> Option<SourceEventV2> {
    let event_type = state_to_event_type(new_state)?;

    Some(SourceEventV2 {
        event_id: format!("codex-jsonl-{pane_id}-{event_seq}"),
        provider: Provider::Codex,
        source_kind: SourceKind::CodexJsonl,
        tier: EvidenceTier::Deterministic,
        observed_at,
        session_key: session_key.to_owned(),
        pane_id: Some(pane_id.to_owned()),
        pane_generation,
        pane_birth_ts,
        source_event_id: None,
        event_type: event_type.to_owned(),
        payload: serde_json::json!({"fsm_state": format!("{new_state:?}")}),
        confidence: 1.0,
        is_heartbeat: false,
        actual_activity_at: None,
    })
}

/// Build a deterministic idle heartbeat for a discovered Codex JSONL session.
///
/// Emitted when the JSONL file exists but no new state transitions occurred.
/// `is_heartbeat = true` prevents biasing provider arbitration.
pub fn idle_heartbeat(
    session_key: &str,
    pane_id: &str,
    pane_generation: Option<u64>,
    pane_birth_ts: Option<DateTime<Utc>>,
    observed_at: DateTime<Utc>,
) -> SourceEventV2 {
    SourceEventV2 {
        event_id: format!("codex-jsonl-hb-{pane_id}"),
        provider: Provider::Codex,
        source_kind: SourceKind::CodexJsonl,
        tier: EvidenceTier::Deterministic,
        observed_at,
        session_key: session_key.to_owned(),
        pane_id: Some(pane_id.to_owned()),
        pane_generation,
        pane_birth_ts,
        source_event_id: None,
        event_type: "activity.idle".to_owned(),
        payload: serde_json::json!({}),
        confidence: 1.0,
        is_heartbeat: true,
        actual_activity_at: None,
    }
}

/// Map FSM state to event_type string.
///
/// Returns `None` for states that don't produce events.
fn state_to_event_type(state: CodexSessionState) -> Option<&'static str> {
    match state {
        CodexSessionState::Running => Some("activity.running"),
        CodexSessionState::ToolExecuting => Some("activity.running"),
        CodexSessionState::WaitingApproval => Some("activity.waiting_approval"),
        CodexSessionState::WaitingInput => Some("activity.waiting_input"),
        CodexSessionState::Ended => None,
        CodexSessionState::Init => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agtmux_core_v5::types::{EvidenceTier, Provider, SourceKind};
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 3, 2, 10, 0, 0)
            .single()
            .expect("valid datetime")
    }

    #[test]
    fn running_state_produces_activity_running() {
        let ev = translate_state_change(
            CodexSessionState::Running,
            "sess-1",
            "%3",
            Some(1),
            None,
            now(),
            1,
        );
        assert!(ev.is_some());
        let ev = ev.expect("should have event");
        assert_eq!(ev.event_type, "activity.running");
        assert_eq!(ev.provider, Provider::Codex);
        assert_eq!(ev.source_kind, SourceKind::CodexJsonl);
        assert_eq!(ev.tier, EvidenceTier::Deterministic);
        assert_eq!(ev.pane_id, Some("%3".to_owned()));
        assert_eq!(ev.pane_generation, Some(1));
        assert!(!ev.is_heartbeat);
        assert!((ev.confidence - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn tool_executing_state_produces_activity_running() {
        let ev = translate_state_change(
            CodexSessionState::ToolExecuting,
            "sess-1",
            "%3",
            None,
            None,
            now(),
            2,
        );
        assert!(ev.is_some());
        assert_eq!(ev.expect("event").event_type, "activity.running");
    }

    #[test]
    fn waiting_approval_produces_waiting_approval() {
        let ev = translate_state_change(
            CodexSessionState::WaitingApproval,
            "sess-1",
            "%3",
            None,
            None,
            now(),
            3,
        );
        assert!(ev.is_some());
        assert_eq!(ev.expect("event").event_type, "activity.waiting_approval");
    }

    #[test]
    fn waiting_input_produces_activity_waiting_input() {
        let ev = translate_state_change(
            CodexSessionState::WaitingInput,
            "sess-1",
            "%3",
            None,
            None,
            now(),
            4,
        );
        assert!(ev.is_some());
        assert_eq!(ev.expect("event").event_type, "activity.waiting_input");
    }

    #[test]
    fn init_state_produces_no_event() {
        let ev = translate_state_change(
            CodexSessionState::Init,
            "sess-1",
            "%3",
            None,
            None,
            now(),
            5,
        );
        assert!(ev.is_none());
    }

    #[test]
    fn ended_state_produces_no_event() {
        let ev = translate_state_change(
            CodexSessionState::Ended,
            "sess-1",
            "%3",
            None,
            None,
            now(),
            6,
        );
        assert!(ev.is_none());
    }

    #[test]
    fn idle_heartbeat_has_correct_fields() {
        let hb = idle_heartbeat("sess-1", "%5", Some(2), None, now());
        assert_eq!(hb.event_type, "activity.idle");
        assert_eq!(hb.provider, Provider::Codex);
        assert_eq!(hb.source_kind, SourceKind::CodexJsonl);
        assert_eq!(hb.tier, EvidenceTier::Deterministic);
        assert!(hb.is_heartbeat, "heartbeat must have is_heartbeat=true");
        assert_eq!(hb.pane_id, Some("%5".to_owned()));
    }
}

//! Event translation from Claude JSONL transcript lines to [`SourceEventV2`].

use agtmux_core_v5::types::{EvidenceTier, Provider, SourceEventV2, SourceKind};
use chrono::{DateTime, Utc};
use serde::Deserialize;

/// Parsed line from a Claude Code JSONL transcript file.
#[derive(Debug, Clone, Deserialize)]
pub struct ClaudeJsonlLine {
    /// Line type: "user", "assistant", "tool_use", "tool_result", etc.
    #[serde(rename = "type")]
    pub line_type: String,
    /// ISO 8601 timestamp.
    pub timestamp: DateTime<Utc>,
    /// Session ID (present on "user" type lines).
    #[serde(rename = "sessionId")]
    pub session_id: Option<String>,
    /// Working directory (present on "user" type lines).
    pub cwd: Option<String>,
    /// UUID of this line entry.
    pub uuid: Option<String>,
}

/// Contextual info needed to translate a JSONL line into a SourceEventV2.
pub struct TranslateContext {
    pub session_id: String,
    pub pane_id: Option<String>,
    pub pane_generation: Option<u64>,
    pub pane_birth_ts: Option<DateTime<Utc>>,
}

/// Translate a parsed JSONL line into a [`SourceEventV2`].
pub fn translate(line: &ClaudeJsonlLine, ctx: &TranslateContext) -> SourceEventV2 {
    let event_type = normalize_event_type(&line.line_type);
    let event_id = format!("claude-jsonl-{}", line.uuid.as_deref().unwrap_or("unknown"));

    SourceEventV2 {
        event_id,
        provider: Provider::Claude,
        source_kind: SourceKind::ClaudeJsonl,
        tier: EvidenceTier::Deterministic,
        observed_at: line.timestamp,
        session_key: ctx.session_id.clone(),
        pane_id: ctx.pane_id.clone(),
        pane_generation: ctx.pane_generation,
        pane_birth_ts: ctx.pane_birth_ts,
        source_event_id: line.uuid.clone(),
        event_type,
        payload: serde_json::json!({
            "line_type": line.line_type,
        }),
        confidence: 1.0,
    }
}

/// Map JSONL line types to normalized event_type strings.
fn normalize_event_type(line_type: &str) -> String {
    match line_type {
        "user" => "activity.user_input".to_owned(),
        "tool_use" => "activity.running".to_owned(),
        "tool_result" => "activity.tool_complete".to_owned(),
        "assistant" => "activity.idle".to_owned(),
        _ => "activity.unknown".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn sample_line(line_type: &str) -> ClaudeJsonlLine {
        ClaudeJsonlLine {
            line_type: line_type.to_owned(),
            timestamp: Utc
                .with_ymd_and_hms(2026, 2, 25, 13, 0, 0)
                .single()
                .expect("valid datetime"),
            session_id: Some("c4c0766e-test".to_owned()),
            cwd: Some("/Users/vm/project".to_owned()),
            uuid: Some("uuid-001".to_owned()),
        }
    }

    fn ctx() -> TranslateContext {
        TranslateContext {
            session_id: "c4c0766e-test".to_owned(),
            pane_id: Some("%3".to_owned()),
            pane_generation: Some(1),
            pane_birth_ts: Some(
                Utc.with_ymd_and_hms(2026, 2, 25, 12, 0, 0)
                    .single()
                    .expect("valid datetime"),
            ),
        }
    }

    #[test]
    fn translate_user_event() {
        let line = sample_line("user");
        let ev = translate(&line, &ctx());

        assert_eq!(ev.event_id, "claude-jsonl-uuid-001");
        assert_eq!(ev.provider, Provider::Claude);
        assert_eq!(ev.source_kind, SourceKind::ClaudeJsonl);
        assert_eq!(ev.tier, EvidenceTier::Deterministic);
        assert_eq!(ev.event_type, "activity.user_input");
        assert_eq!(ev.session_key, "c4c0766e-test");
        assert_eq!(ev.pane_id, Some("%3".to_owned()));
        assert_eq!(ev.pane_generation, Some(1));
        assert!((ev.confidence - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn translate_tool_use_event() {
        let line = sample_line("tool_use");
        let ev = translate(&line, &ctx());
        assert_eq!(ev.event_type, "activity.running");
    }

    #[test]
    fn translate_tool_result_event() {
        let line = sample_line("tool_result");
        let ev = translate(&line, &ctx());
        assert_eq!(ev.event_type, "activity.tool_complete");
    }

    #[test]
    fn translate_assistant_event() {
        let line = sample_line("assistant");
        let ev = translate(&line, &ctx());
        assert_eq!(ev.event_type, "activity.idle");
    }

    #[test]
    fn translate_unknown_event_type() {
        let line = sample_line("some_future_type");
        let ev = translate(&line, &ctx());
        assert_eq!(ev.event_type, "activity.unknown");
    }

    #[test]
    fn translate_no_uuid_uses_unknown() {
        let mut line = sample_line("user");
        line.uuid = None;
        let ev = translate(&line, &ctx());
        assert_eq!(ev.event_id, "claude-jsonl-unknown");
        assert!(ev.source_event_id.is_none());
    }
}

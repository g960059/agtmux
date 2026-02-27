//! Event translation from Claude hooks format to [`SourceEventV2`].

use agtmux_core_v5::types::{EvidenceTier, Provider, SourceEventV2, SourceKind};
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
        event_type: normalize_event_type(&raw.hook_type),
        payload: raw.data.clone(),
        confidence: 1.0,
        is_heartbeat: false, // Claude hooks are always real activity (not periodic keep-alive)
    }
}

/// Map Claude hook types to normalized event_type strings.
fn normalize_event_type(hook_type: &str) -> String {
    match hook_type {
        "session_start" => "lifecycle.start".to_owned(),
        "session_end" => "lifecycle.end".to_owned(),
        "tool_start" | "thinking" => "lifecycle.running".to_owned(),
        "tool_end" | "idle" => "lifecycle.idle".to_owned(),
        "error" => "lifecycle.error".to_owned(),
        _ => "lifecycle.unknown".to_owned(),
    }
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
        assert_eq!(ev.payload, serde_json::json!({"tool": "bash"}));
        assert!((ev.confidence - 1.0).abs() < f64::EPSILON);
        assert_eq!(ev.source_event_id, Some("h-001".to_owned()));
        assert!(ev.pane_generation.is_none());
        assert!(ev.pane_birth_ts.is_none());
    }

    #[test]
    fn event_type_normalization() {
        let cases = [
            ("session_start", "lifecycle.start"),
            ("session_end", "lifecycle.end"),
            ("tool_start", "lifecycle.running"),
            ("thinking", "lifecycle.running"),
            ("tool_end", "lifecycle.idle"),
            ("idle", "lifecycle.idle"),
            ("error", "lifecycle.error"),
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
}

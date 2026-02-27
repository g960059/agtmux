//! Translates raw Codex appserver lifecycle events into [`SourceEventV2`].

use agtmux_core_v5::types::{EvidenceTier, Provider, SourceEventV2, SourceKind};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Raw Codex lifecycle event (as received from Codex CLI local API).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodexRawEvent {
    /// Unique event ID from Codex.
    pub id: String,
    /// Event type: "session.start", "session.end", "task.running", "task.idle", "task.error".
    pub event_type: String,
    /// Session identifier.
    pub session_id: String,
    /// Timestamp when the event occurred.
    pub timestamp: DateTime<Utc>,
    /// Optional pane ID (from cwd correlation or capture source).
    pub pane_id: Option<String>,
    /// Pane generation (from PaneGenerationTracker, set during cwd correlation).
    pub pane_generation: Option<u64>,
    /// Pane birth timestamp (from PaneGenerationTracker, set during cwd correlation).
    pub pane_birth_ts: Option<DateTime<Utc>>,
    /// Arbitrary payload data.
    pub payload: serde_json::Value,
    /// Whether this event is a periodic heartbeat (not a real state change).
    /// Set by the Codex poller when the only emit reason is elapsed time.
    #[serde(default)]
    pub is_heartbeat: bool,
}

/// Translate a single Codex raw event to a [`SourceEventV2`].
///
/// Translation rules:
/// - `provider` = [`Provider::Codex`]
/// - `source_kind` = [`SourceKind::CodexAppserver`]
/// - `tier` = [`EvidenceTier::Deterministic`]
/// - `event_id` = `"codex-app-{raw.id}"`
/// - `session_key` = `raw.session_id`
/// - `observed_at` = `raw.timestamp`
/// - `event_type` = `raw.event_type`
/// - `confidence` = `1.0` (deterministic source)
/// - `pane_id` = `raw.pane_id`
pub fn translate(raw: &CodexRawEvent) -> SourceEventV2 {
    SourceEventV2 {
        event_id: format!("codex-app-{}", raw.id),
        provider: Provider::Codex,
        source_kind: SourceKind::CodexAppserver,
        tier: EvidenceTier::Deterministic,
        observed_at: raw.timestamp,
        session_key: raw.session_id.clone(),
        pane_id: raw.pane_id.clone(),
        pane_generation: raw.pane_generation,
        pane_birth_ts: raw.pane_birth_ts,
        source_event_id: Some(raw.id.clone()),
        event_type: raw.event_type.clone(),
        payload: raw.payload.clone(),
        confidence: 1.0,
        is_heartbeat: raw.is_heartbeat,
    }
}

/// Translate a batch of raw events, assigning sequential event IDs.
///
/// Each translated event retains its own `event_id` derived from the raw event;
/// `cursor_offset` is provided for caller bookkeeping but does not alter the
/// event IDs (the source server uses it for cursor tracking).
pub fn translate_batch(raw_events: &[CodexRawEvent], _cursor_offset: u64) -> Vec<SourceEventV2> {
    raw_events.iter().map(translate).collect()
}

// ─── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_raw(id: &str, event_type: &str, pane_id: Option<&str>) -> CodexRawEvent {
        CodexRawEvent {
            id: id.to_string(),
            event_type: event_type.to_string(),
            session_id: "sess-abc".to_string(),
            timestamp: Utc::now(),
            pane_id: pane_id.map(String::from),
            pane_generation: None,
            pane_birth_ts: None,
            payload: json!({"key": "value"}),
            is_heartbeat: false,
        }
    }

    #[test]
    fn single_event_translation_correctness() {
        let raw = make_raw("evt-1", "session.start", Some("%1"));
        let translated = translate(&raw);

        assert_eq!(translated.event_id, "codex-app-evt-1");
        assert_eq!(translated.provider, Provider::Codex);
        assert_eq!(translated.source_kind, SourceKind::CodexAppserver);
        assert_eq!(translated.tier, EvidenceTier::Deterministic);
        assert_eq!(translated.session_key, "sess-abc");
        assert_eq!(translated.observed_at, raw.timestamp);
        assert_eq!(translated.event_type, "session.start");
        assert!((translated.confidence - 1.0).abs() < f64::EPSILON);
        assert_eq!(translated.pane_id, Some("%1".to_string()));
        assert_eq!(translated.source_event_id, Some("evt-1".to_string()));
        assert_eq!(translated.payload, json!({"key": "value"}));
        assert!(translated.pane_generation.is_none());
        assert!(translated.pane_birth_ts.is_none());
    }

    #[test]
    fn batch_translation_with_cursor_offset() {
        let events = vec![
            make_raw("a1", "session.start", Some("%1")),
            make_raw("a2", "task.running", Some("%2")),
            make_raw("a3", "session.end", None),
        ];
        let batch = translate_batch(&events, 42);
        assert_eq!(batch.len(), 3);
        assert_eq!(batch[0].event_id, "codex-app-a1");
        assert_eq!(batch[1].event_id, "codex-app-a2");
        assert_eq!(batch[2].event_id, "codex-app-a3");
    }

    #[test]
    fn event_type_preservation() {
        let types = [
            "session.start",
            "session.end",
            "task.running",
            "task.idle",
            "task.error",
        ];
        for ty in types {
            let raw = make_raw("x", ty, None);
            let translated = translate(&raw);
            assert_eq!(translated.event_type, ty);
        }
    }

    #[test]
    fn pane_generation_and_birth_ts_passthrough() {
        let now = Utc::now();
        let raw = CodexRawEvent {
            id: "g1".to_string(),
            event_type: "thread.active".to_string(),
            session_id: "thr-1".to_string(),
            timestamp: now,
            pane_id: Some("%5".to_string()),
            pane_generation: Some(3),
            pane_birth_ts: Some(now),
            payload: json!({}),
            is_heartbeat: false,
        };
        let translated = translate(&raw);
        assert_eq!(translated.pane_generation, Some(3));
        assert_eq!(translated.pane_birth_ts, Some(now));
    }

    #[test]
    fn deterministic_tier_and_confidence() {
        let raw = make_raw("d1", "task.running", None);
        let translated = translate(&raw);
        assert_eq!(translated.tier, EvidenceTier::Deterministic);
        assert!((translated.confidence - 1.0).abs() < f64::EPSILON);
    }

    // ── T-123: is_heartbeat passthrough tests ────────────────────

    #[test]
    fn translate_heartbeat_flag_preserved() {
        let mut raw = make_raw("hb-1", "task.idle", None);
        raw.is_heartbeat = true;
        let translated = translate(&raw);
        assert!(
            translated.is_heartbeat,
            "heartbeat flag must be preserved through translate()"
        );
    }

    #[test]
    fn translate_real_event_flag_false() {
        let raw = make_raw("re-1", "task.running", Some("%3"));
        assert!(
            !raw.is_heartbeat,
            "make_raw should default is_heartbeat=false"
        );
        let translated = translate(&raw);
        assert!(
            !translated.is_heartbeat,
            "real event must have is_heartbeat=false after translate()"
        );
    }
}

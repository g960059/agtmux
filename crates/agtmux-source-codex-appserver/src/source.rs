//! Source server logic: cursor management and health reporting for the Codex appserver source.

use agtmux_core_v5::types::{
    PullEventsRequest, PullEventsResponse, SourceHealthReport, SourceHealthStatus,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::translate::{self, CodexRawEvent};

/// Cursor prefix used to namespace Codex appserver cursors.
const CURSOR_PREFIX: &str = "codex-app:";

/// In-memory cursor state for the source server.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SourceState {
    /// Events buffered since last pull.
    events: Vec<CodexRawEvent>,
    /// Monotonic sequence counter for cursor generation.
    seq: u64,
}

impl SourceState {
    /// Create an empty source state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Ingest a raw event into the buffer.
    ///
    /// Each ingested event advances the internal sequence counter, which is used
    /// for cursor generation during [`Self::pull_events`].
    pub fn ingest(&mut self, event: CodexRawEvent) {
        self.events.push(event);
        self.seq += 1;
    }

    /// Handle a `pull_events` request: return translated events from the cursor position.
    ///
    /// # Cursor semantics
    ///
    /// - `None` cursor starts from the beginning (seq 0).
    /// - Cursor format: `"codex-app:{seq}"` where `seq` is the next position to read from.
    /// - Returns at most `request.limit` events.
    /// - `next_cursor` points one past the last returned event (or remains at the
    ///   current position if no events are returned).
    /// - `heartbeat_ts` is set to `now`.
    /// - `source_health` is always `Healthy` for an in-process source.
    pub fn pull_events(
        &self,
        request: &PullEventsRequest,
        now: DateTime<Utc>,
    ) -> PullEventsResponse {
        let start = match request.cursor.as_deref() {
            Some(cursor) => parse_cursor(cursor),
            None => 0,
        };

        let start = start.min(self.events.len()); // Clamp to avoid out-of-range
        let limit = request.limit as usize;
        let end = self.events.len().min(start.saturating_add(limit));
        let slice = &self.events[start..end];

        let translated = translate::translate_batch(slice, start as u64);

        // Always return current position so the gateway can overwrite its
        // tracker cursor.  Returning None when caught-up would cause the
        // gateway to keep the old cursor and re-deliver the same events.
        let next_cursor = Some(format!("{CURSOR_PREFIX}{end}"));

        PullEventsResponse {
            events: translated,
            next_cursor,
            heartbeat_ts: now,
            source_health: SourceHealthReport {
                status: SourceHealthStatus::Healthy,
                checked_at: now,
            },
        }
    }
}

/// Parse a cursor string to extract the sequence number.
///
/// Falls back to 0 if the cursor does not have the expected prefix or
/// contains an unparseable sequence number.
fn parse_cursor(cursor: &str) -> usize {
    cursor
        .strip_prefix(CURSOR_PREFIX)
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0)
}

// ─── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use agtmux_core_v5::types::SourceHealthStatus;
    use serde_json::json;

    fn make_raw(id: &str, event_type: &str) -> CodexRawEvent {
        CodexRawEvent {
            id: id.to_string(),
            event_type: event_type.to_string(),
            session_id: "sess-1".to_string(),
            timestamp: Utc::now(),
            pane_id: Some("%0".to_string()),
            payload: json!({}),
        }
    }

    fn make_request(cursor: Option<&str>, limit: u32) -> PullEventsRequest {
        PullEventsRequest {
            cursor: cursor.map(String::from),
            limit,
        }
    }

    #[test]
    fn empty_state_returns_empty_events() {
        let state = SourceState::new();
        let resp = state.pull_events(&make_request(None, 500), Utc::now());
        assert!(resp.events.is_empty());
        assert_eq!(resp.next_cursor, Some("codex-app:0".to_string())); // caught up: returns current pos
    }

    #[test]
    fn ingest_and_pull_returns_translated_events() {
        let mut state = SourceState::new();
        state.ingest(make_raw("e1", "session.start"));
        state.ingest(make_raw("e2", "task.running"));

        let resp = state.pull_events(&make_request(None, 500), Utc::now());
        assert_eq!(resp.events.len(), 2);
        assert_eq!(resp.events[0].event_id, "codex-app-e1");
        assert_eq!(resp.events[1].event_id, "codex-app-e2");
    }

    #[test]
    fn cursor_based_pagination_from_middle() {
        let mut state = SourceState::new();
        state.ingest(make_raw("e1", "session.start"));
        state.ingest(make_raw("e2", "task.running"));
        state.ingest(make_raw("e3", "task.idle"));

        // Pull from cursor position 1 (skip the first event).
        let resp = state.pull_events(&make_request(Some("codex-app:1"), 500), Utc::now());
        assert_eq!(resp.events.len(), 2);
        assert_eq!(resp.events[0].event_id, "codex-app-e2");
        assert_eq!(resp.events[1].event_id, "codex-app-e3");
    }

    #[test]
    fn limit_enforcement() {
        let mut state = SourceState::new();
        for i in 0..10 {
            state.ingest(make_raw(&format!("e{i}"), "task.running"));
        }

        let resp = state.pull_events(&make_request(None, 3), Utc::now());
        assert_eq!(resp.events.len(), 3);
        assert_eq!(resp.events[0].event_id, "codex-app-e0");
        assert_eq!(resp.events[2].event_id, "codex-app-e2");
    }

    #[test]
    fn next_cursor_advances_correctly() {
        let mut state = SourceState::new();
        for i in 0..5 {
            state.ingest(make_raw(&format!("e{i}"), "task.running"));
        }

        // First pull: 2 events from the start.
        let resp1 = state.pull_events(&make_request(None, 2), Utc::now());
        assert_eq!(resp1.events.len(), 2);
        assert_eq!(resp1.next_cursor, Some("codex-app:2".to_string()));

        // Second pull: use the returned cursor.
        let resp2 = state.pull_events(&make_request(resp1.next_cursor.as_deref(), 2), Utc::now());
        assert_eq!(resp2.events.len(), 2);
        assert_eq!(resp2.next_cursor, Some("codex-app:4".to_string()));

        // Third pull: only 1 event left.
        let resp3 = state.pull_events(&make_request(resp2.next_cursor.as_deref(), 2), Utc::now());
        assert_eq!(resp3.events.len(), 1);
        assert_eq!(resp3.next_cursor, Some("codex-app:5".to_string())); // caught up: returns current pos

        // Fourth pull: no events left, cursor is None.
        let resp4 = state.pull_events(&make_request(None, 2), Utc::now());
        // Starts from 0 when cursor is None
        assert_eq!(resp4.events.len(), 2);
    }

    #[test]
    fn health_status_is_healthy() {
        let state = SourceState::new();
        let resp = state.pull_events(&make_request(None, 500), Utc::now());
        assert_eq!(resp.source_health.status, SourceHealthStatus::Healthy);
    }
}

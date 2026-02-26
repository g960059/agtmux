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
    /// Offset from compaction: number of events drained from the front.
    /// Cursors are always absolute; `compact_offset` adjusts the index.
    compact_offset: u64,
    /// Whether the Codex App Server is currently connected (set by runtime).
    /// When `true`, health is `Healthy`; when `false`, health is `Degraded`
    /// (capture fallback is active or no evidence path available).
    #[serde(skip)]
    appserver_connected: bool,
}

impl SourceState {
    /// Create an empty source state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Update the App Server connection status (called by runtime each poll tick).
    ///
    /// This controls the health status reported in [`Self::pull_events`] responses:
    /// `true` → `Healthy`, `false` → `Degraded`.
    pub fn set_appserver_connected(&mut self, connected: bool) {
        self.appserver_connected = connected;
    }

    /// Ingest a raw event into the buffer.
    ///
    /// Each ingested event advances the internal sequence counter, which is used
    /// for cursor generation during [`Self::pull_events`].
    pub fn ingest(&mut self, event: CodexRawEvent) {
        self.events.push(event);
        self.seq += 1;
    }

    /// Truncate events that have been consumed (absolute cursor <= `up_to_seq`).
    ///
    /// Adjusts internal state so that subsequent `pull_events` calls with
    /// cursors based on the old sequence numbers still work correctly.
    pub fn compact(&mut self, up_to_seq: u64) {
        // up_to_seq is an absolute cursor; convert to local buffer index
        let local_pos = up_to_seq.saturating_sub(self.compact_offset);
        #[expect(clippy::cast_possible_truncation)]
        let drain_count = (local_pos as usize).min(self.events.len());
        if drain_count > 0 {
            self.events.drain(..drain_count);
            self.compact_offset += drain_count as u64;
        }
    }

    /// Number of events currently buffered.
    pub fn buffered_len(&self) -> usize {
        self.events.len()
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
    /// - `source_health` reflects App Server connectivity: `Healthy` when connected,
    ///   `Degraded` when using capture fallback.
    pub fn pull_events(
        &self,
        request: &PullEventsRequest,
        now: DateTime<Utc>,
    ) -> PullEventsResponse {
        let abs_start = match request.cursor.as_deref() {
            Some(cursor) => parse_cursor(cursor),
            None => 0,
        };

        // Convert absolute cursor to index into current (possibly compacted) buffer
        #[expect(clippy::cast_possible_truncation)]
        let local_start = (abs_start as u64).saturating_sub(self.compact_offset) as usize;
        let start = local_start.min(self.events.len());
        let limit = request.limit as usize;
        let end = self.events.len().min(start.saturating_add(limit));
        let slice = &self.events[start..end];

        // translate_batch offset is the absolute position of the first event
        let abs_offset = self.compact_offset + start as u64;
        let translated = translate::translate_batch(slice, abs_offset);

        // Cursor is always absolute (compact_offset + buffer position)
        let abs_end = self.compact_offset + end as u64;
        let next_cursor = Some(format!("{CURSOR_PREFIX}{abs_end}"));

        PullEventsResponse {
            events: translated,
            next_cursor,
            heartbeat_ts: now,
            source_health: SourceHealthReport {
                status: if self.appserver_connected {
                    SourceHealthStatus::Healthy
                } else {
                    SourceHealthStatus::Degraded
                },
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
            pane_generation: None,
            pane_birth_ts: None,
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
    fn health_status_degraded_without_appserver() {
        let state = SourceState::new();
        let resp = state.pull_events(&make_request(None, 500), Utc::now());
        assert_eq!(resp.source_health.status, SourceHealthStatus::Degraded);
    }

    #[test]
    fn health_status_healthy_with_appserver() {
        let mut state = SourceState::new();
        state.set_appserver_connected(true);
        let resp = state.pull_events(&make_request(None, 500), Utc::now());
        assert_eq!(resp.source_health.status, SourceHealthStatus::Healthy);
    }

    // ── Compaction tests ────────────────────────────────────────────

    #[test]
    fn compact_trims_consumed_events() {
        let mut state = SourceState::new();
        for i in 0..5 {
            state.ingest(make_raw(&format!("e{i}"), "task.running"));
        }
        assert_eq!(state.buffered_len(), 5);

        state.compact(3);
        assert_eq!(state.buffered_len(), 2);
    }

    #[test]
    fn compact_cursors_remain_valid() {
        let mut state = SourceState::new();
        let now = Utc::now();
        for i in 0..5 {
            state.ingest(make_raw(&format!("e{i}"), "task.running"));
        }

        // Pull first 3
        let resp1 = state.pull_events(&make_request(None, 3), now);
        assert_eq!(resp1.events.len(), 3);
        let cursor = resp1.next_cursor.as_deref().expect("has cursor");
        assert_eq!(cursor, "codex-app:3");

        // Compact those 3
        state.compact(3);
        assert_eq!(state.buffered_len(), 2);

        // Pull remaining with old cursor — should still work
        let resp2 = state.pull_events(&make_request(Some(cursor), 10), now);
        assert_eq!(resp2.events.len(), 2);
        assert_eq!(resp2.next_cursor, Some("codex-app:5".to_string()));
    }

    #[test]
    fn compact_repeated_absolute_no_over_drain() {
        let mut state = SourceState::new();
        for i in 0..6 {
            state.ingest(make_raw(&format!("e{i}"), "task.running"));
        }

        state.compact(3);
        assert_eq!(state.buffered_len(), 3);

        // 2nd compact: should drain 2 more (5 - 3), not 5
        state.compact(5);
        assert_eq!(state.buffered_len(), 1);

        // Absolute cursor still works
        let now = Utc::now();
        let resp = state.pull_events(&make_request(Some("codex-app:5"), 10), now);
        assert_eq!(resp.events.len(), 1);
        assert_eq!(resp.next_cursor, Some("codex-app:6".to_string()));
    }
}

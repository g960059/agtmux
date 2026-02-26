//! Source server logic for Claude hooks: cursor management, event storage,
//! and health reporting.

use agtmux_core_v5::types::{
    PullEventsRequest, PullEventsResponse, SourceHealthReport, SourceHealthStatus,
};
use chrono::{DateTime, Utc};

use crate::translate::{self, ClaudeHookEvent};

/// Cursor prefix used for Claude hooks source.
const CURSOR_PREFIX: &str = "claude-hooks:";

/// In-memory cursor state for the Claude hooks source server.
#[derive(Debug, Clone, Default)]
pub struct SourceState {
    /// Ordered list of ingested raw events.
    events: Vec<ClaudeHookEvent>,
    /// Monotonically increasing sequence number.
    seq: u64,
    /// Offset from compaction: number of events drained from the front.
    /// Cursors are always absolute; `compact_offset` adjusts the index.
    compact_offset: u64,
}

impl SourceState {
    /// Create a new empty source state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Ingest a raw Claude hook event into the buffer.
    pub fn ingest(&mut self, event: ClaudeHookEvent) {
        self.events.push(event);
        self.seq += 1;
    }

    /// Truncate events that have been consumed (absolute cursor <= `up_to_seq`).
    pub fn compact(&mut self, up_to_seq: u64) {
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

    /// Pull translated events according to cursor and limit.
    ///
    /// The cursor format is `"claude-hooks:{seq}"` where seq is the 0-based
    /// index of the next event to return. A `None` cursor starts from the
    /// beginning.
    pub fn pull_events(
        &self,
        request: &PullEventsRequest,
        now: DateTime<Utc>,
    ) -> PullEventsResponse {
        let abs_start = request
            .cursor
            .as_deref()
            .and_then(|c| c.strip_prefix(CURSOR_PREFIX))
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);

        // Convert absolute cursor to index into current (possibly compacted) buffer
        #[expect(clippy::cast_possible_truncation)]
        let local_start = abs_start.saturating_sub(self.compact_offset) as usize;
        let start = local_start.min(self.events.len());
        let limit = request.limit as usize;
        let end = self.events.len().min(start.saturating_add(limit));

        let events: Vec<_> = self.events[start..end]
            .iter()
            .map(translate::translate)
            .collect();

        // Cursor is always absolute (compact_offset + buffer position)
        let abs_end = self.compact_offset + end as u64;
        let next_cursor = Some(format!("{CURSOR_PREFIX}{abs_end}"));

        PullEventsResponse {
            events,
            next_cursor,
            heartbeat_ts: now,
            source_health: SourceHealthReport {
                status: SourceHealthStatus::Healthy,
                checked_at: now,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn make_event(id: &str, hook_type: &str) -> ClaudeHookEvent {
        ClaudeHookEvent {
            hook_id: id.to_owned(),
            hook_type: hook_type.to_owned(),
            session_id: "sess-1".to_owned(),
            timestamp: Utc
                .with_ymd_and_hms(2026, 2, 1, 12, 0, 0)
                .single()
                .expect("valid datetime"),
            pane_id: Some("%1".to_owned()),
            data: serde_json::json!({}),
        }
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 2, 1, 12, 5, 0)
            .single()
            .expect("valid datetime")
    }

    #[test]
    fn empty_state_returns_empty_events() {
        let state = SourceState::new();
        let req = PullEventsRequest {
            cursor: None,
            limit: 500,
        };
        let resp = state.pull_events(&req, now());

        assert!(resp.events.is_empty());
        assert_eq!(resp.next_cursor, Some("claude-hooks:0".to_string())); // caught up: returns current pos
        assert_eq!(resp.source_health.status, SourceHealthStatus::Healthy);
    }

    #[test]
    fn ingest_and_pull_returns_translated_events() {
        let mut state = SourceState::new();
        state.ingest(make_event("e1", "session_start"));
        state.ingest(make_event("e2", "tool_start"));

        let req = PullEventsRequest {
            cursor: None,
            limit: 500,
        };
        let resp = state.pull_events(&req, now());

        assert_eq!(resp.events.len(), 2);
        assert_eq!(resp.events[0].event_id, "claude-hooks-e1");
        assert_eq!(resp.events[0].event_type, "lifecycle.start");
        assert_eq!(resp.events[1].event_id, "claude-hooks-e2");
        assert_eq!(resp.events[1].event_type, "lifecycle.running");
        assert_eq!(resp.next_cursor, Some("claude-hooks:2".to_string())); // caught up: returns current pos
        assert_eq!(resp.source_health.status, SourceHealthStatus::Healthy);
    }

    #[test]
    fn out_of_range_cursor_clamps_to_end() {
        let mut state = SourceState::new();
        state.ingest(make_event("e1", "session_start"));

        let req = PullEventsRequest {
            cursor: Some("claude-hooks:999".to_owned()),
            limit: 500,
        };
        let resp = state.pull_events(&req, now());

        // Clamped to len, returns empty without panicking
        assert!(resp.events.is_empty());
        assert_eq!(resp.next_cursor, Some("claude-hooks:1".to_string())); // caught up: returns current pos
    }

    #[test]
    fn cursor_pagination() {
        let mut state = SourceState::new();
        state.ingest(make_event("e1", "session_start"));
        state.ingest(make_event("e2", "tool_start"));
        state.ingest(make_event("e3", "tool_end"));

        // First page: limit 2
        let req1 = PullEventsRequest {
            cursor: None,
            limit: 2,
        };
        let resp1 = state.pull_events(&req1, now());
        assert_eq!(resp1.events.len(), 2);
        assert_eq!(resp1.next_cursor, Some("claude-hooks:2".to_owned()));

        // Second page: from cursor
        let req2 = PullEventsRequest {
            cursor: resp1.next_cursor,
            limit: 2,
        };
        let resp2 = state.pull_events(&req2, now());
        assert_eq!(resp2.events.len(), 1);
        assert_eq!(resp2.events[0].event_id, "claude-hooks-e3");
        assert_eq!(resp2.next_cursor, Some("claude-hooks:3".to_string())); // caught up: returns current pos
    }

    #[test]
    fn limit_enforcement() {
        let mut state = SourceState::new();
        for i in 0..10 {
            state.ingest(make_event(&format!("e{i}"), "idle"));
        }

        let req = PullEventsRequest {
            cursor: None,
            limit: 3,
        };
        let resp = state.pull_events(&req, now());
        assert_eq!(resp.events.len(), 3);
        assert_eq!(resp.next_cursor, Some("claude-hooks:3".to_owned()));
    }

    #[test]
    fn health_status() {
        let mut state = SourceState::new();
        let req = PullEventsRequest {
            cursor: None,
            limit: 500,
        };

        // Empty state -> Healthy (health FSM is at gateway level)
        let resp_empty = state.pull_events(&req, now());
        assert_eq!(resp_empty.source_health.status, SourceHealthStatus::Healthy);

        // After ingest -> still Healthy
        state.ingest(make_event("e1", "session_start"));
        let resp_with = state.pull_events(&req, now());
        assert_eq!(resp_with.source_health.status, SourceHealthStatus::Healthy);
        assert_eq!(resp_with.heartbeat_ts, now());
    }

    // ── Compaction tests ────────────────────────────────────────────

    #[test]
    fn compact_trims_consumed_events() {
        let mut state = SourceState::new();
        for i in 0..5 {
            state.ingest(make_event(&format!("e{i}"), "tool_start"));
        }
        assert_eq!(state.buffered_len(), 5);

        state.compact(3);
        assert_eq!(state.buffered_len(), 2);
    }

    #[test]
    fn compact_cursors_remain_valid() {
        let mut state = SourceState::new();
        for i in 0..5 {
            state.ingest(make_event(&format!("e{i}"), "tool_start"));
        }

        // Pull first 3
        let req = PullEventsRequest {
            cursor: None,
            limit: 3,
        };
        let resp1 = state.pull_events(&req, now());
        assert_eq!(resp1.events.len(), 3);
        let cursor = resp1.next_cursor.as_deref().expect("has cursor");
        assert_eq!(cursor, "claude-hooks:3");

        // Compact those 3
        state.compact(3);
        assert_eq!(state.buffered_len(), 2);

        // Pull remaining with old cursor
        let req2 = PullEventsRequest {
            cursor: Some(cursor.to_owned()),
            limit: 10,
        };
        let resp2 = state.pull_events(&req2, now());
        assert_eq!(resp2.events.len(), 2);
        assert_eq!(resp2.next_cursor, Some("claude-hooks:5".to_string()));
    }

    #[test]
    fn compact_repeated_absolute_no_over_drain() {
        let mut state = SourceState::new();
        for i in 0..6 {
            state.ingest(make_event(&format!("e{i}"), "tool_start"));
        }

        state.compact(3);
        assert_eq!(state.buffered_len(), 3);

        state.compact(5);
        assert_eq!(state.buffered_len(), 1);

        let req = PullEventsRequest {
            cursor: Some("claude-hooks:5".to_owned()),
            limit: 10,
        };
        let resp = state.pull_events(&req, now());
        assert_eq!(resp.events.len(), 1);
        assert_eq!(resp.next_cursor, Some("claude-hooks:6".to_string()));
    }
}

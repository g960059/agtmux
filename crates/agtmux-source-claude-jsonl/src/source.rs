//! Source server logic for Claude JSONL: cursor management, event storage,
//! health reporting, and file watcher orchestration.

use std::collections::HashMap;

use agtmux_core_v5::types::{
    PullEventsRequest, PullEventsResponse, SourceEventV2, SourceHealthReport, SourceHealthStatus,
};
use chrono::{DateTime, Utc};
use tracing::warn;

use crate::discovery::{self, SessionDiscovery};
use crate::translate::{self, ClaudeJsonlLine, TranslateContext};
use crate::watcher::SessionFileWatcher;

/// Cursor prefix used for Claude JSONL source.
const CURSOR_PREFIX: &str = "claude-jsonl:";

/// In-memory cursor state for the Claude JSONL source server.
#[derive(Debug, Clone, Default)]
pub struct ClaudeJsonlSourceState {
    /// Ordered list of translated events ready for pull.
    events: Vec<SourceEventV2>,
    /// Monotonically increasing sequence number.
    seq: u64,
    /// Offset from compaction.
    compact_offset: u64,
    /// Whether the source has ever received events (for health reporting).
    has_received_events: bool,
}

impl ClaudeJsonlSourceState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ingest a translated SourceEventV2.
    pub fn ingest(&mut self, event: SourceEventV2) {
        self.events.push(event);
        self.seq += 1;
        self.has_received_events = true;
    }

    /// Truncate events that have been consumed.
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

    /// Pull events according to cursor and limit.
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

        #[expect(clippy::cast_possible_truncation)]
        let local_start = abs_start.saturating_sub(self.compact_offset) as usize;
        let start = local_start.min(self.events.len());
        let limit = request.limit as usize;
        let end = self.events.len().min(start.saturating_add(limit));

        let events = self.events[start..end].to_vec();

        let abs_end = self.compact_offset + end as u64;
        let next_cursor = Some(format!("{CURSOR_PREFIX}{abs_end}"));

        PullEventsResponse {
            events,
            next_cursor,
            heartbeat_ts: now,
            source_health: SourceHealthReport {
                status: if self.has_received_events {
                    SourceHealthStatus::Healthy
                } else {
                    SourceHealthStatus::Down
                },
                checked_at: now,
            },
        }
    }

    /// Discover JSONL sessions for given pane CWDs, poll new lines from
    /// their files, translate to events, and return them.
    ///
    /// Manages watcher lifecycle (create/reuse/remove).
    pub fn poll_files(
        watchers: &mut HashMap<String, SessionFileWatcher>,
        discoveries: &[SessionDiscovery],
    ) -> Vec<SourceEventV2> {
        let mut events = Vec::new();

        // Track which pane_ids are still discovered (for cleanup)
        let active_pane_ids: std::collections::HashSet<&str> =
            discoveries.iter().map(|d| d.pane_id.as_str()).collect();

        // Remove watchers for panes no longer discovered
        watchers.retain(|k, _| active_pane_ids.contains(k.as_str()));

        for discovery in discoveries {
            // Create or reuse watcher
            let watcher = watchers
                .entry(discovery.pane_id.clone())
                .or_insert_with(|| SessionFileWatcher::new(discovery.jsonl_path.clone()));

            // Check if the watcher is pointing to the right file
            if watcher.path() != discovery.jsonl_path {
                *watcher = SessionFileWatcher::new(discovery.jsonl_path.clone());
            }

            // Poll new lines
            let new_lines = watcher.poll_new_lines();

            let ctx = TranslateContext {
                session_id: discovery.session_id.clone(),
                pane_id: Some(discovery.pane_id.clone()),
                pane_generation: discovery.pane_generation,
                pane_birth_ts: discovery.pane_birth_ts,
            };

            for line_str in &new_lines {
                match serde_json::from_str::<ClaudeJsonlLine>(line_str) {
                    Ok(parsed) => {
                        if let Some(event) = translate::translate(&parsed, &ctx) {
                            events.push(event);
                        }
                    }
                    Err(e) => {
                        warn!(
                            pane_id = %discovery.pane_id,
                            error = %e,
                            "failed to parse JSONL line, skipping"
                        );
                    }
                }
            }
        }

        events
    }

    /// Discover sessions for given pane CWDs.
    #[allow(clippy::type_complexity)]
    pub fn discover_sessions(
        pane_cwds: &[(String, String, Option<u64>, Option<DateTime<Utc>>)],
    ) -> Vec<SessionDiscovery> {
        discovery::discover_sessions(pane_cwds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agtmux_core_v5::types::{EvidenceTier, Provider, SourceKind};
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 2, 25, 14, 0, 0)
            .single()
            .expect("valid datetime")
    }

    fn make_event(id: &str) -> SourceEventV2 {
        SourceEventV2 {
            event_id: format!("claude-jsonl-{id}"),
            provider: Provider::Claude,
            source_kind: SourceKind::ClaudeJsonl,
            tier: EvidenceTier::Deterministic,
            observed_at: now(),
            session_key: "sess-1".to_owned(),
            pane_id: Some("%1".to_owned()),
            pane_generation: Some(1),
            pane_birth_ts: None,
            source_event_id: Some(id.to_owned()),
            event_type: "activity.running".to_owned(),
            payload: serde_json::json!({"line_type": "tool_use"}),
            confidence: 1.0,
            is_heartbeat: false,
        }
    }

    #[test]
    fn empty_state_returns_empty() {
        let state = ClaudeJsonlSourceState::new();
        let req = PullEventsRequest {
            cursor: None,
            limit: 500,
        };
        let resp = state.pull_events(&req, now());

        assert!(resp.events.is_empty());
        assert_eq!(resp.next_cursor, Some("claude-jsonl:0".to_string()));
        assert_eq!(resp.source_health.status, SourceHealthStatus::Down);
    }

    #[test]
    fn ingest_and_pull() {
        let mut state = ClaudeJsonlSourceState::new();
        state.ingest(make_event("e1"));
        state.ingest(make_event("e2"));

        let req = PullEventsRequest {
            cursor: None,
            limit: 500,
        };
        let resp = state.pull_events(&req, now());

        assert_eq!(resp.events.len(), 2);
        assert_eq!(resp.events[0].event_id, "claude-jsonl-e1");
        assert_eq!(resp.events[1].event_id, "claude-jsonl-e2");
        assert_eq!(resp.next_cursor, Some("claude-jsonl:2".to_string()));
        assert_eq!(resp.source_health.status, SourceHealthStatus::Healthy);
    }

    #[test]
    fn cursor_pagination() {
        let mut state = ClaudeJsonlSourceState::new();
        for i in 0..5 {
            state.ingest(make_event(&format!("e{i}")));
        }

        let req1 = PullEventsRequest {
            cursor: None,
            limit: 2,
        };
        let resp1 = state.pull_events(&req1, now());
        assert_eq!(resp1.events.len(), 2);
        assert_eq!(resp1.next_cursor, Some("claude-jsonl:2".to_owned()));

        let req2 = PullEventsRequest {
            cursor: resp1.next_cursor,
            limit: 2,
        };
        let resp2 = state.pull_events(&req2, now());
        assert_eq!(resp2.events.len(), 2);
        assert_eq!(resp2.next_cursor, Some("claude-jsonl:4".to_owned()));

        let req3 = PullEventsRequest {
            cursor: resp2.next_cursor,
            limit: 10,
        };
        let resp3 = state.pull_events(&req3, now());
        assert_eq!(resp3.events.len(), 1);
        assert_eq!(resp3.next_cursor, Some("claude-jsonl:5".to_string()));
    }

    #[test]
    fn compact_trims_events() {
        let mut state = ClaudeJsonlSourceState::new();
        for i in 0..5 {
            state.ingest(make_event(&format!("e{i}")));
        }
        assert_eq!(state.buffered_len(), 5);

        state.compact(3);
        assert_eq!(state.buffered_len(), 2);

        // Cursors still work after compaction
        let req = PullEventsRequest {
            cursor: Some("claude-jsonl:3".to_owned()),
            limit: 10,
        };
        let resp = state.pull_events(&req, now());
        assert_eq!(resp.events.len(), 2);
        assert_eq!(resp.next_cursor, Some("claude-jsonl:5".to_string()));
    }

    #[test]
    fn poll_files_with_real_jsonl() {
        use std::fs;
        use std::io::Write;

        let tmp = std::env::temp_dir().join("agtmux-test-poll-files");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).expect("test");

        let jsonl_path = tmp.join("test-session.jsonl");
        let mut f = fs::File::create(&jsonl_path).expect("test");
        writeln!(f, r#"{{"type":"user","timestamp":"2026-02-25T13:00:00Z","sessionId":"sess-1","uuid":"u1"}}"#).expect("test");
        writeln!(
            f,
            r#"{{"type":"tool_use","timestamp":"2026-02-25T13:00:01Z","uuid":"u2"}}"#
        )
        .expect("test");
        drop(f);

        let discoveries = vec![SessionDiscovery {
            pane_id: "%1".to_owned(),
            session_id: "sess-1".to_owned(),
            jsonl_path: jsonl_path.clone(),
            pane_generation: Some(1),
            pane_birth_ts: None,
        }];

        let mut watchers = HashMap::new();
        // Use a watcher that starts from position 0 for testing
        watchers.insert(
            "%1".to_owned(),
            SessionFileWatcher::new_from_start(jsonl_path),
        );

        let events = ClaudeJsonlSourceState::poll_files(&mut watchers, &discoveries);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, "activity.user_input");
        assert_eq!(events[0].provider, Provider::Claude);
        assert_eq!(events[0].source_kind, SourceKind::ClaudeJsonl);
        assert_eq!(events[0].tier, EvidenceTier::Deterministic);
        assert_eq!(events[1].event_type, "activity.running");

        let _ = fs::remove_dir_all(&tmp);
    }
}

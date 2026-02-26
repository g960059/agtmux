//! Poller source server: ties detection + evidence into a source server
//! that produces `SourceEventV2` events via cursor-based pull.
//!
//! This module implements T-032: the poller fallback source server that
//! processes tmux pane snapshots, detects agents via heuristic pattern
//! matching, and produces typed source events for the gateway.

use agtmux_core_v5::types::{
    ActivityState, EvidenceTier, Provider, PullEventsRequest, PullEventsResponse, SourceEventV2,
    SourceHealthReport, SourceHealthStatus, SourceKind,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::detect::{PaneMeta, detect_best};
use crate::evidence::{claude_activity_signals, codex_activity_signals, match_activity};

// ─── Snapshot ───────────────────────────────────────────────────────

/// Snapshot of a tmux pane for poller processing.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PaneSnapshot {
    pub pane_id: String,
    pub pane_title: String,
    pub current_cmd: String,
    pub process_hint: Option<String>,
    /// Captured terminal output lines (last N lines).
    pub capture_lines: Vec<String>,
    /// Timestamp when the snapshot was taken.
    pub captured_at: DateTime<Utc>,
}

// ─── Poll Result ────────────────────────────────────────────────────

/// Result of processing a single pane snapshot.
#[derive(Debug, Clone, PartialEq)]
pub struct PollResult {
    pub pane_id: String,
    pub provider: Provider,
    pub activity_state: ActivityState,
    pub confidence: f64,
    pub event: SourceEventV2,
}

// ─── Event type mapping ─────────────────────────────────────────────

/// Map an `ActivityState` to the corresponding event_type string.
fn activity_event_type(state: ActivityState) -> &'static str {
    match state {
        ActivityState::Running => "activity.running",
        ActivityState::Idle => "activity.idle",
        ActivityState::WaitingInput => "activity.waiting_input",
        ActivityState::WaitingApproval => "activity.waiting_approval",
        ActivityState::Error => "activity.error",
        ActivityState::Unknown | _ => "activity.unknown",
    }
}

// ─── poll_pane ──────────────────────────────────────────────────────

/// Process a single pane snapshot: detect agent, match activity, produce event.
///
/// Returns `None` if no agent is detected in the pane.
pub fn poll_pane(snapshot: &PaneSnapshot) -> Option<PollResult> {
    // 1. Build PaneMeta from snapshot (including capture_lines for 4th detection signal)
    let meta = PaneMeta {
        pane_title: snapshot.pane_title.clone(),
        current_cmd: snapshot.current_cmd.clone(),
        process_hint: snapshot.process_hint.clone(),
        capture_lines: snapshot.capture_lines.clone(),
    };

    // 2. Detect agent — if None, return None
    let detect_result = detect_best(&meta)?;

    // 3. Get activity signals for the detected provider
    let signals = match detect_result.provider {
        Provider::Claude => claude_activity_signals(),
        Provider::Codex => codex_activity_signals(),
        // For unsupported providers, use Claude signals as default fallback
        _ => claude_activity_signals(),
    };

    // 4. Match activity against capture lines
    let line_refs: Vec<&str> = snapshot.capture_lines.iter().map(String::as_str).collect();
    let activity_match = match_activity(&line_refs, &signals);

    let activity_state = activity_match
        .as_ref()
        .map_or(ActivityState::Unknown, |m| m.state);

    // 5. Build SourceEventV2
    let event_id = format!(
        "poller-{}-{}",
        snapshot.pane_id,
        snapshot.captured_at.timestamp_millis()
    );
    let session_key = format!("poller-{}", snapshot.pane_id);

    let payload = serde_json::json!({
        "provider": detect_result.provider.as_str(),
        "provider_hint": detect_result.provider_hint,
        "cmd_match": detect_result.cmd_match,
        "title_match": detect_result.title_match,
        "capture_match": detect_result.capture_match,
        "is_wrapper_cmd": detect_result.is_wrapper_cmd,
        "detection_confidence": detect_result.confidence,
        "activity_state": format!("{activity_state:?}"),
        "matched_pattern": activity_match.as_ref().map(|m| m.matched_pattern.clone()),
    });

    let event = SourceEventV2 {
        event_id,
        provider: detect_result.provider,
        source_kind: SourceKind::Poller,
        tier: EvidenceTier::Heuristic,
        observed_at: snapshot.captured_at,
        session_key,
        pane_id: Some(snapshot.pane_id.clone()),
        pane_generation: None,
        pane_birth_ts: None,
        source_event_id: None,
        event_type: activity_event_type(activity_state).to_string(),
        payload,
        confidence: detect_result.confidence,
    };

    // 6. Return PollResult
    Some(PollResult {
        pane_id: snapshot.pane_id.clone(),
        provider: detect_result.provider,
        activity_state,
        confidence: detect_result.confidence,
        event,
    })
}

// ─── Poller Source State ────────────────────────────────────────────

/// Cursor prefix for poller source server.
const CURSOR_PREFIX: &str = "poller:";

/// In-memory state for the poller source server.
#[derive(Debug, Clone, Default)]
pub struct PollerSourceState {
    events: Vec<SourceEventV2>,
    seq: u64,
    /// Offset from compaction: number of events drained from the front.
    /// Cursors are always absolute; `compact_offset` adjusts the index.
    compact_offset: u64,
}

impl PollerSourceState {
    /// Create a new empty poller source state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Process a batch of pane snapshots, producing events for detected agents.
    pub fn poll_batch(&mut self, snapshots: &[PaneSnapshot]) {
        for snapshot in snapshots {
            if let Some(result) = poll_pane(snapshot) {
                self.events.push(result.event);
                self.seq = self.seq.saturating_add(1);
            }
        }
    }

    /// Parse a cursor string into its sequence number.
    /// Returns `None` for invalid cursors, `Some(0)` for `None` cursor (start from beginning).
    fn parse_cursor(cursor: &Option<String>) -> Option<u64> {
        match cursor {
            None => Some(0),
            Some(c) => {
                let stripped = c.strip_prefix(CURSOR_PREFIX)?;
                stripped.parse::<u64>().ok()
            }
        }
    }

    /// Truncate events that have been consumed (cursor position <= `up_to_seq`).
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

    /// Handle a `pull_events` request.
    pub fn pull_events(
        &self,
        request: &PullEventsRequest,
        now: DateTime<Utc>,
    ) -> PullEventsResponse {
        let abs_start = Self::parse_cursor(&request.cursor).unwrap_or(0);

        // Convert absolute cursor to index into current (possibly compacted) buffer
        #[expect(clippy::cast_possible_truncation)]
        let local_start = abs_start.saturating_sub(self.compact_offset) as usize;
        let start_index = local_start.min(self.events.len());
        let limit = request.limit as usize;
        let end = self.events.len().min(start_index.saturating_add(limit));

        let page: Vec<SourceEventV2> = self.events[start_index..end].to_vec();

        // Cursor is always absolute (compact_offset + buffer position)
        let abs_end = self.compact_offset + end as u64;
        let next_cursor = Some(format!("{CURSOR_PREFIX}{abs_end}"));

        PullEventsResponse {
            events: page,
            next_cursor,
            heartbeat_ts: now,
            source_health: SourceHealthReport {
                status: SourceHealthStatus::Healthy,
                checked_at: now,
            },
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .expect("valid RFC3339 timestamp")
            .with_timezone(&Utc)
    }

    fn now() -> DateTime<Utc> {
        ts("2026-02-25T12:00:00Z")
    }

    fn claude_snapshot() -> PaneSnapshot {
        PaneSnapshot {
            pane_id: "%1".to_string(),
            pane_title: "claude code".to_string(),
            current_cmd: "claude".to_string(),
            process_hint: Some("claude".to_string()),
            capture_lines: vec!["Thinking about the problem".to_string()],
            captured_at: now(),
        }
    }

    fn codex_snapshot() -> PaneSnapshot {
        PaneSnapshot {
            pane_id: "%2".to_string(),
            pane_title: "codex terminal".to_string(),
            current_cmd: "codex --model o3".to_string(),
            process_hint: Some("codex".to_string()),
            capture_lines: vec!["Processing your request".to_string()],
            captured_at: now(),
        }
    }

    fn no_agent_snapshot() -> PaneSnapshot {
        PaneSnapshot {
            pane_id: "%3".to_string(),
            pane_title: "vim".to_string(),
            current_cmd: "bash".to_string(),
            process_hint: None,
            capture_lines: vec!["some random output".to_string()],
            captured_at: now(),
        }
    }

    // ── 1. poll_pane with Claude pane returns Running event ──────────

    #[test]
    fn poll_pane_claude_running() {
        let snapshot = claude_snapshot();
        let result = poll_pane(&snapshot).expect("should detect Claude");

        assert_eq!(result.provider, Provider::Claude);
        assert_eq!(result.activity_state, ActivityState::Running);
        assert_eq!(result.pane_id, "%1");
        assert_eq!(result.event.event_type, "activity.running");
    }

    // ── 2. poll_pane with Codex pane returns appropriate event ──────

    #[test]
    fn poll_pane_codex_running() {
        let snapshot = codex_snapshot();
        let result = poll_pane(&snapshot).expect("should detect Codex");

        assert_eq!(result.provider, Provider::Codex);
        assert_eq!(result.activity_state, ActivityState::Running);
        assert_eq!(result.pane_id, "%2");
        assert_eq!(result.event.event_type, "activity.running");
    }

    // ── 3. poll_pane with no agent returns None ─────────────────────

    #[test]
    fn poll_pane_no_agent_returns_none() {
        let snapshot = no_agent_snapshot();
        let result = poll_pane(&snapshot);

        assert!(result.is_none());
    }

    // ── 4. poll_pane event fields correctness ───────────────────────

    #[test]
    fn poll_pane_event_fields_correctness() {
        let snapshot = claude_snapshot();
        let result = poll_pane(&snapshot).expect("should detect Claude");
        let event = &result.event;

        // Provider
        assert_eq!(event.provider, Provider::Claude);
        // Source kind = Poller
        assert_eq!(event.source_kind, SourceKind::Poller);
        // Tier = Heuristic
        assert_eq!(event.tier, EvidenceTier::Heuristic);
        // event_id format
        assert!(
            event.event_id.starts_with("poller-%1-"),
            "event_id should start with 'poller-%1-', got: {}",
            event.event_id
        );
        // session_key
        assert_eq!(event.session_key, "poller-%1");
        // pane_id
        assert_eq!(event.pane_id, Some("%1".to_string()));
        // observed_at
        assert_eq!(event.observed_at, now());
        // confidence > 0
        assert!(event.confidence > 0.0);
        // pane_generation and pane_birth_ts are None for poller
        assert!(event.pane_generation.is_none());
        assert!(event.pane_birth_ts.is_none());
        // source_event_id is None for poller
        assert!(event.source_event_id.is_none());
    }

    // ── 5. poll_batch processes multiple snapshots ──────────────────

    #[test]
    fn poll_batch_processes_multiple_snapshots() {
        let mut state = PollerSourceState::new();
        let snapshots = vec![claude_snapshot(), codex_snapshot(), no_agent_snapshot()];

        state.poll_batch(&snapshots);

        // Only 2 of 3 have agents
        assert_eq!(state.events.len(), 2);
        assert_eq!(state.seq, 2);
        assert_eq!(state.events[0].provider, Provider::Claude);
        assert_eq!(state.events[1].provider, Provider::Codex);
    }

    // ── 6. Cursor-based pagination works ────────────────────────────

    #[test]
    fn cursor_based_pagination() {
        let mut state = PollerSourceState::new();

        // Insert 5 events
        for i in 0..5 {
            let snapshot = PaneSnapshot {
                pane_id: format!("%{i}"),
                pane_title: "claude code".to_string(),
                current_cmd: "claude".to_string(),
                process_hint: Some("claude".to_string()),
                capture_lines: vec!["Thinking".to_string()],
                captured_at: now(),
            };
            state.poll_batch(&[snapshot]);
        }
        assert_eq!(state.events.len(), 5);

        let n = now();

        // First page: limit 2, no cursor
        let resp1 = state.pull_events(
            &PullEventsRequest {
                cursor: None,
                limit: 2,
            },
            n,
        );
        assert_eq!(resp1.events.len(), 2);
        assert_eq!(resp1.next_cursor, Some("poller:2".to_string()));

        // Second page: use next_cursor
        let resp2 = state.pull_events(
            &PullEventsRequest {
                cursor: resp1.next_cursor,
                limit: 2,
            },
            n,
        );
        assert_eq!(resp2.events.len(), 2);
        assert_eq!(resp2.next_cursor, Some("poller:4".to_string()));

        // Third page: last event
        let resp3 = state.pull_events(
            &PullEventsRequest {
                cursor: resp2.next_cursor,
                limit: 2,
            },
            n,
        );
        assert_eq!(resp3.events.len(), 1);
        assert_eq!(resp3.next_cursor, Some("poller:5".to_string())); // caught up: returns current pos

        // Fourth page: no cursor → starts from beginning
        let resp4 = state.pull_events(
            &PullEventsRequest {
                cursor: None,
                limit: 2,
            },
            n,
        );
        assert_eq!(resp4.events.len(), 2);
    }

    // ── 7. Empty state returns empty events ─────────────────────────

    #[test]
    fn empty_state_returns_empty_events() {
        let state = PollerSourceState::new();
        let resp = state.pull_events(
            &PullEventsRequest {
                cursor: None,
                limit: 10,
            },
            now(),
        );

        assert!(resp.events.is_empty());
        assert_eq!(resp.next_cursor, Some("poller:0".to_string())); // caught up: returns current pos
    }

    // ── 8. Health status is Healthy ─────────────────────────────────

    #[test]
    fn health_status_is_healthy() {
        let state = PollerSourceState::new();
        let resp = state.pull_events(
            &PullEventsRequest {
                cursor: None,
                limit: 10,
            },
            now(),
        );

        assert_eq!(resp.source_health.status, SourceHealthStatus::Healthy);
        assert_eq!(resp.source_health.checked_at, now());
    }

    // ── 9. Activity state mapping to event_type ─────────────────────

    #[test]
    fn activity_state_mapping_to_event_type() {
        // Running
        assert_eq!(
            activity_event_type(ActivityState::Running),
            "activity.running"
        );
        // Idle
        assert_eq!(activity_event_type(ActivityState::Idle), "activity.idle");
        // WaitingInput
        assert_eq!(
            activity_event_type(ActivityState::WaitingInput),
            "activity.waiting_input"
        );
        // WaitingApproval
        assert_eq!(
            activity_event_type(ActivityState::WaitingApproval),
            "activity.waiting_approval"
        );
        // Error
        assert_eq!(activity_event_type(ActivityState::Error), "activity.error");
        // Unknown
        assert_eq!(
            activity_event_type(ActivityState::Unknown),
            "activity.unknown"
        );
    }

    // ── Verify correct event_type for idle snapshot ─────────────────

    #[test]
    fn poll_pane_idle_event_type() {
        let snapshot = PaneSnapshot {
            pane_id: "%10".to_string(),
            pane_title: "claude code".to_string(),
            current_cmd: "claude".to_string(),
            process_hint: Some("claude".to_string()),
            capture_lines: vec!["\u{276f}".to_string()], // ❯ = idle
            captured_at: now(),
        };
        let result = poll_pane(&snapshot).expect("should detect Claude");
        assert_eq!(result.activity_state, ActivityState::Idle);
        assert_eq!(result.event.event_type, "activity.idle");
    }

    // ── 10. Confidence from detection passed through ────────────────

    #[test]
    fn confidence_from_detection_passed_through() {
        let snapshot = claude_snapshot();
        let result = poll_pane(&snapshot).expect("should detect Claude");

        // Claude snapshot has process_hint="claude", so confidence should be WEIGHT_PROCESS_HINT
        let expected = agtmux_core_v5::signature::WEIGHT_PROCESS_HINT;
        assert!(
            (result.confidence - expected).abs() < f64::EPSILON,
            "expected confidence {expected}, got {}",
            result.confidence
        );
        assert!(
            (result.event.confidence - expected).abs() < f64::EPSILON,
            "event confidence should match: expected {expected}, got {}",
            result.event.confidence
        );
    }

    // ── poll_pane unknown activity when no lines match ───────────────

    #[test]
    fn poll_pane_unknown_activity_no_lines_match() {
        let snapshot = PaneSnapshot {
            pane_id: "%5".to_string(),
            pane_title: "claude code".to_string(),
            current_cmd: "claude".to_string(),
            process_hint: Some("claude".to_string()),
            capture_lines: vec!["no known pattern here".to_string()],
            captured_at: now(),
        };
        let result =
            poll_pane(&snapshot).expect("should detect Claude even without activity match");
        assert_eq!(result.activity_state, ActivityState::Unknown);
        assert_eq!(result.event.event_type, "activity.unknown");
    }

    // ── poll_batch with empty snapshots ─────────────────────────────

    #[test]
    fn poll_batch_empty_snapshots() {
        let mut state = PollerSourceState::new();
        state.poll_batch(&[]);

        assert!(state.events.is_empty());
        assert_eq!(state.seq, 0);
    }

    // ── Invalid cursor falls back to start ──────────────────────────

    #[test]
    fn invalid_cursor_falls_back_to_start() {
        let mut state = PollerSourceState::new();
        state.poll_batch(&[claude_snapshot()]);

        let resp = state.pull_events(
            &PullEventsRequest {
                cursor: Some("garbage".to_string()),
                limit: 10,
            },
            now(),
        );

        // Invalid cursor -> starts from 0
        assert_eq!(resp.events.len(), 1);
    }

    // ── Heartbeat timestamp is passed through ───────────────────────

    #[test]
    fn heartbeat_timestamp_passed_through() {
        let state = PollerSourceState::new();
        let specific_time = ts("2026-06-15T10:30:00Z");
        let resp = state.pull_events(
            &PullEventsRequest {
                cursor: None,
                limit: 10,
            },
            specific_time,
        );

        assert_eq!(resp.heartbeat_ts, specific_time);
    }

    // ── No re-delivery when caught up (cursor contract fix) ────────

    #[test]
    fn no_redelivery_when_caught_up() {
        let mut state = PollerSourceState::new();
        state.poll_batch(&[claude_snapshot(), codex_snapshot()]);
        assert_eq!(state.events.len(), 2);

        let n = now();

        // Pull all events
        let resp1 = state.pull_events(
            &PullEventsRequest {
                cursor: None,
                limit: 100,
            },
            n,
        );
        assert_eq!(resp1.events.len(), 2);
        let cursor = resp1
            .next_cursor
            .expect("cursor must be Some even when caught up");
        assert_eq!(cursor, "poller:2");

        // Re-pull with returned cursor: should get 0 events, not re-delivery
        let resp2 = state.pull_events(
            &PullEventsRequest {
                cursor: Some(cursor.clone()),
                limit: 100,
            },
            n,
        );
        assert!(resp2.events.is_empty(), "no re-delivery when caught up");
        assert_eq!(
            resp2.next_cursor,
            Some(cursor),
            "cursor stays at current position"
        );
    }

    // ── Payload contains detection details ──────────────────────────

    #[test]
    fn payload_contains_detection_details() {
        let snapshot = claude_snapshot();
        let result = poll_pane(&snapshot).expect("should detect Claude");
        let payload = &result.event.payload;

        assert_eq!(payload["provider"], "claude");
        assert_eq!(payload["provider_hint"], true);
        assert_eq!(payload["cmd_match"], true);
        assert_eq!(payload["title_match"], true);
    }

    // ── Capture-match wiring ──────────────────────────────────────────

    #[test]
    fn poll_pane_capture_match_node_cmd() {
        // Node cmd + no title match + capture match → detected via capture
        let snapshot = PaneSnapshot {
            pane_id: "%20".to_string(),
            pane_title: "random dynamic title".to_string(),
            current_cmd: "node".to_string(),
            process_hint: None,
            capture_lines: vec![
                "some output".to_string(),
                "\u{256D} Claude Code".to_string(),
            ],
            captured_at: now(),
        };
        let result = poll_pane(&snapshot).expect("should detect Claude via capture");
        assert_eq!(result.provider, Provider::Claude);
        assert_eq!(result.event.payload["capture_match"], true);
        assert_eq!(result.event.payload["cmd_match"], false);
    }

    #[test]
    fn poll_pane_stale_title_shell_suppressed() {
        // Stale title + shell cmd + no capture → no detection
        let snapshot = PaneSnapshot {
            pane_id: "%21".to_string(),
            pane_title: "\u{2733} Claude Code".to_string(), // stale title
            current_cmd: "zsh".to_string(),
            process_hint: None,
            capture_lines: vec!["$ whoami".to_string()],
            captured_at: now(),
        };
        let result = poll_pane(&snapshot);
        assert!(result.is_none(), "stale title + shell → suppressed");
    }

    // ── Compaction tests ────────────────────────────────────────────

    #[test]
    fn compact_trims_consumed_events() {
        let mut state = PollerSourceState::new();
        for i in 0..5 {
            let snapshot = PaneSnapshot {
                pane_id: format!("%{i}"),
                current_cmd: "claude".to_string(),
                process_hint: Some("claude".to_string()),
                capture_lines: vec!["output".to_string()],
                captured_at: now(),
                ..Default::default()
            };
            state.poll_batch(&[snapshot]);
        }
        assert_eq!(state.buffered_len(), 5);

        // Compact first 3 events
        state.compact(3);
        assert_eq!(state.buffered_len(), 2);
    }

    #[test]
    fn compact_cursors_remain_valid() {
        let mut state = PollerSourceState::new();
        let n = now();

        for i in 0..5 {
            let snapshot = PaneSnapshot {
                pane_id: format!("%{i}"),
                current_cmd: "claude".to_string(),
                process_hint: Some("claude".to_string()),
                capture_lines: vec!["output".to_string()],
                captured_at: n,
                ..Default::default()
            };
            state.poll_batch(&[snapshot]);
        }

        // Pull first 3
        let resp1 = state.pull_events(
            &PullEventsRequest {
                cursor: None,
                limit: 3,
            },
            n,
        );
        assert_eq!(resp1.events.len(), 3);
        let cursor = resp1.next_cursor.expect("has cursor");
        assert_eq!(cursor, "poller:3");

        // Compact those 3
        state.compact(3);
        assert_eq!(state.buffered_len(), 2);

        // Pull remaining with the old cursor — should still work
        let resp2 = state.pull_events(
            &PullEventsRequest {
                cursor: Some(cursor),
                limit: 10,
            },
            n,
        );
        assert_eq!(resp2.events.len(), 2, "remaining 2 events after compact");
        assert_eq!(resp2.next_cursor, Some("poller:5".to_string()));
    }

    #[test]
    fn compact_beyond_buffer_is_safe() {
        let mut state = PollerSourceState::new();
        state.poll_batch(&[claude_snapshot()]);
        assert_eq!(state.buffered_len(), 1);

        // Compact more than what exists
        state.compact(100);
        assert_eq!(state.buffered_len(), 0);
    }

    /// F1 regression: repeated compaction with absolute cursors must not over-drain.
    #[test]
    fn compact_repeated_absolute_cursors_no_over_drain() {
        let mut state = PollerSourceState::new();
        let n = now();

        // Insert 6 events (absolute positions 0..6)
        for i in 0..6 {
            let snapshot = PaneSnapshot {
                pane_id: format!("%{i}"),
                current_cmd: "claude".to_string(),
                process_hint: Some("claude".to_string()),
                capture_lines: vec!["output".to_string()],
                captured_at: n,
                ..Default::default()
            };
            state.poll_batch(&[snapshot]);
        }
        assert_eq!(state.buffered_len(), 6);

        // 1st compact: up_to_seq=3 → drain 3 events, offset=3, buffer=3
        state.compact(3);
        assert_eq!(state.buffered_len(), 3);

        // 2nd compact: up_to_seq=5 → should drain 2 more (5 - 3), not 5
        state.compact(5);
        assert_eq!(
            state.buffered_len(),
            1,
            "2nd compact should only drain 2, not 5"
        );

        // Cursor from before compaction still works
        let resp = state.pull_events(
            &PullEventsRequest {
                cursor: Some("poller:5".to_string()),
                limit: 10,
            },
            n,
        );
        assert_eq!(
            resp.events.len(),
            1,
            "last event accessible via absolute cursor"
        );
        assert_eq!(resp.next_cursor, Some("poller:6".to_string()));
    }
}

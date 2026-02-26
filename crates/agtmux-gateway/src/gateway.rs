//! Gateway: aggregates events from multiple source servers, manages per-source
//! cursors, and serves merged events to the daemon via `gateway.pull_events`.
//!
//! Architecture ref: docs/30_architecture.md C-003
//! Task ref: T-040

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use agtmux_core_v5::types::{
    GatewayPullRequest, GatewayPullResponse, PullEventsResponse, SourceCursorState, SourceEventV2,
    SourceHealthReport, SourceHealthStatus, SourceKind,
};

// ─── Constants ───────────────────────────────────────────────────────

/// Cursor prefix for the gateway's global cursor namespace.
const GATEWAY_CURSOR_PREFIX: &str = "gw:";

/// Default limit for `gateway.pull_events` when not specified.
pub const DEFAULT_PULL_LIMIT: u32 = 500;

// ─── Gateway ─────────────────────────────────────────────────────────

/// Per-source tracking state held by the gateway.
#[derive(Debug, Clone)]
struct SourceTracker {
    /// Last cursor returned by this source server.
    cursor: Option<String>,
    /// Latest health report from this source.
    health: SourceHealthReport,
    /// Last heartbeat timestamp from the source.
    last_heartbeat: DateTime<Utc>,
}

/// Gateway: pull-aggregates events from multiple source servers.
///
/// The gateway is a **pure in-process aggregator** in the MVP. It:
///
/// 1. Accepts [`PullEventsResponse`] from each source (via [`Self::ingest_source_response`]).
/// 2. Merges events chronologically into a single ordered buffer.
/// 3. Tracks per-source cursors and health.
/// 4. Serves aggregated events to the daemon via [`Self::pull_events`].
/// 5. Supports cursor commit to advance the consumption watermark.
#[derive(Debug)]
pub struct Gateway {
    /// Per-source tracking (cursor + health).
    sources: HashMap<SourceKind, SourceTracker>,
    /// Aggregated event buffer (ordered by observed_at, then ingest order).
    buffer: Vec<SourceEventV2>,
    /// Global monotonic sequence (used for gateway cursor generation).
    global_seq: u64,
    /// Offset from compaction: number of events drained from the front.
    /// Cursors are always absolute; `compact_offset` adjusts the index.
    compact_offset: usize,
}

impl Gateway {
    /// Create a new empty gateway.
    pub fn new() -> Self {
        Self {
            sources: HashMap::new(),
            buffer: Vec::new(),
            global_seq: 0,
            compact_offset: 0,
        }
    }

    /// Create a gateway with a set of registered source kinds.
    ///
    /// Pre-registers sources so that `list_source_health` can report
    /// `Down` for sources that have never responded.
    pub fn with_sources(source_kinds: &[SourceKind], now: DateTime<Utc>) -> Self {
        let mut sources = HashMap::new();
        for &kind in source_kinds {
            sources.insert(
                kind,
                SourceTracker {
                    cursor: None,
                    health: SourceHealthReport {
                        status: SourceHealthStatus::Down,
                        checked_at: now,
                    },
                    last_heartbeat: now,
                },
            );
        }
        Self {
            sources,
            buffer: Vec::new(),
            global_seq: 0,
            compact_offset: 0,
        }
    }

    // ── Source Ingestion ──────────────────────────────────────────────

    /// Ingest a source server's response into the gateway.
    ///
    /// This method:
    /// 1. Appends the source's events to the internal buffer.
    /// 2. Updates the per-source cursor to `next_cursor`.
    /// 3. Records the source health and heartbeat.
    /// 4. Sorts the buffer by `(observed_at, ingest_order)` to maintain
    ///    chronological ordering for the daemon.
    pub fn ingest_source_response(
        &mut self,
        source_kind: SourceKind,
        response: PullEventsResponse,
    ) {
        // Track buffer growth for sorting decision
        let had_events = !response.events.is_empty();

        // Append events and assign global sequence numbers
        for event in response.events {
            self.buffer.push(event);
            self.global_seq = self.global_seq.saturating_add(1);
        }

        // Update source tracker
        let tracker = self
            .sources
            .entry(source_kind)
            .or_insert_with(|| SourceTracker {
                cursor: None,
                health: SourceHealthReport {
                    status: SourceHealthStatus::Down,
                    checked_at: response.heartbeat_ts,
                },
                last_heartbeat: response.heartbeat_ts,
            });

        // Always overwrite cursor to match source's reported position.
        // Sources must always return Some(current_pos) even when caught up.
        tracker.cursor = response.next_cursor;
        tracker.health = response.source_health;
        tracker.last_heartbeat = response.heartbeat_ts;

        // Re-sort buffer by observed_at to maintain chronological order
        // (stable sort preserves ingest order for same-timestamp events)
        if had_events {
            self.buffer.sort_by_key(|e| e.observed_at);
        }
    }

    // ── Daemon Pull ──────────────────────────────────────────────────

    /// Handle a `gateway.pull_events` request from the daemon.
    ///
    /// # Cursor semantics
    ///
    /// - `None` cursor starts from the beginning (position 0).
    /// - Cursor format: `"gw:{position}"` where position is an absolute
    ///   index (accounts for compacted events).
    /// - Returns at most `request.limit` events.
    /// - `next_cursor` points one past the last returned event.
    pub fn pull_events(&self, request: &GatewayPullRequest) -> GatewayPullResponse {
        let abs_start = parse_gateway_cursor(request.cursor.as_deref());
        let local_start = abs_start.saturating_sub(self.compact_offset);
        let limit = request.limit as usize;

        let available = if local_start < self.buffer.len() {
            &self.buffer[local_start..]
        } else {
            &[]
        };

        let page: Vec<SourceEventV2> = available.iter().take(limit).cloned().collect();
        let returned_count = page.len();

        let next_cursor = if returned_count > 0 {
            // Clamp abs_start to compact_offset so stale cursors don't
            // produce next_pos values that point into already-compacted range.
            let effective_start = abs_start.max(self.compact_offset);
            let next_pos = effective_start + returned_count;
            Some(format!("{GATEWAY_CURSOR_PREFIX}{next_pos}"))
        } else {
            // No events: keep the same cursor (or None)
            request.cursor.clone()
        };

        GatewayPullResponse {
            events: page,
            next_cursor,
        }
    }

    /// Compact the buffer: remove events before the given absolute cursor position.
    ///
    /// This should be called periodically after the daemon has consumed events.
    /// The cursor must be an absolute position (as returned by `pull_events`).
    pub fn compact_before(&mut self, abs_position: usize) {
        let local_pos = abs_position.saturating_sub(self.compact_offset);
        let drain_count = local_pos.min(self.buffer.len());
        if drain_count > 0 {
            self.buffer.drain(..drain_count);
            self.compact_offset += drain_count;
        }
    }

    /// Commit a cursor position, indicating the daemon has successfully
    /// processed events up to this point. Compacts the buffer.
    pub fn commit_cursor(&mut self, cursor: &str) {
        let abs_pos = parse_gateway_cursor(Some(cursor));
        self.compact_before(abs_pos);
    }

    // ── Source Health ────────────────────────────────────────────────

    /// List health status for all registered sources.
    ///
    /// Returns a `Vec` of `(SourceKind, SourceHealthReport)` sorted by
    /// source kind name for deterministic output.
    pub fn list_source_health(&self) -> Vec<(SourceKind, SourceHealthReport)> {
        let mut result: Vec<(SourceKind, SourceHealthReport)> = self
            .sources
            .iter()
            .map(|(&kind, tracker)| (kind, tracker.health.clone()))
            .collect();
        result.sort_by_key(|(kind, _)| kind.as_str());
        result
    }

    /// Get the current health report for a specific source.
    pub fn source_health(&self, source_kind: SourceKind) -> Option<&SourceHealthReport> {
        self.sources.get(&source_kind).map(|t| &t.health)
    }

    // ── Source Cursor ────────────────────────────────────────────────

    /// Get the current cursor for a specific source (to use in the next poll).
    pub fn source_cursor(&self, source_kind: SourceKind) -> Option<&str> {
        self.sources
            .get(&source_kind)
            .and_then(|t| t.cursor.as_deref())
    }

    /// Build cursor state snapshots for all sources.
    pub fn source_cursor_states(&self, now: DateTime<Utc>) -> Vec<SourceCursorState> {
        self.sources
            .iter()
            .map(|(&kind, tracker)| SourceCursorState {
                source_kind: kind,
                committed_cursor: tracker.cursor.clone(),
                checkpoint_ts: now,
            })
            .collect()
    }

    // ── Buffer Queries ──────────────────────────────────────────────

    /// Total number of events currently in the buffer.
    pub fn buffer_len(&self) -> usize {
        self.buffer.len()
    }

    /// Global sequence counter (total events ever ingested).
    pub fn global_seq(&self) -> u64 {
        self.global_seq
    }
}

impl Default for Gateway {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Cursor Parsing ──────────────────────────────────────────────────

/// Parse a gateway cursor string to extract the buffer position.
///
/// Falls back to 0 if the cursor is `None`, doesn't have the expected prefix,
/// or contains an unparseable position.
fn parse_gateway_cursor(cursor: Option<&str>) -> usize {
    cursor
        .and_then(|c| c.strip_prefix(GATEWAY_CURSOR_PREFIX))
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0)
}

// ─── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use agtmux_core_v5::types::{Provider, SourceKind};
    use chrono::TimeDelta;

    // ── Test Helpers ─────────────────────────────────────────────────

    fn ts(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s)
            .expect("valid RFC3339")
            .with_timezone(&Utc)
    }

    fn now() -> DateTime<Utc> {
        ts("2026-02-25T12:00:00Z")
    }

    fn make_event(
        event_id: &str,
        provider: Provider,
        source_kind: SourceKind,
        observed_at: DateTime<Utc>,
    ) -> SourceEventV2 {
        SourceEventV2 {
            event_id: event_id.to_string(),
            provider,
            source_kind,
            tier: source_kind.tier(),
            observed_at,
            session_key: "sess-1".to_string(),
            pane_id: Some("%1".to_string()),
            pane_generation: None,
            pane_birth_ts: None,
            source_event_id: Some(event_id.to_string()),
            event_type: "lifecycle.running".to_string(),
            payload: serde_json::json!({}),
            confidence: 1.0,
        }
    }

    fn make_source_response(
        events: Vec<SourceEventV2>,
        next_cursor: Option<&str>,
        heartbeat: DateTime<Utc>,
        status: SourceHealthStatus,
    ) -> PullEventsResponse {
        PullEventsResponse {
            events,
            next_cursor: next_cursor.map(String::from),
            heartbeat_ts: heartbeat,
            source_health: SourceHealthReport {
                status,
                checked_at: heartbeat,
            },
        }
    }

    // ── 1. Empty gateway returns empty response ──────────────────────

    #[test]
    fn empty_gateway_returns_empty() {
        let gw = Gateway::new();
        let resp = gw.pull_events(&GatewayPullRequest {
            cursor: None,
            limit: 500,
        });

        assert!(resp.events.is_empty());
        assert!(resp.next_cursor.is_none());
    }

    // ── 2. Single source ingestion and pull ──────────────────────────

    #[test]
    fn single_source_ingest_and_pull() {
        let mut gw = Gateway::new();
        let t = now();

        let events = vec![
            make_event("e1", Provider::Codex, SourceKind::CodexAppserver, t),
            make_event(
                "e2",
                Provider::Codex,
                SourceKind::CodexAppserver,
                t + TimeDelta::seconds(1),
            ),
        ];

        gw.ingest_source_response(
            SourceKind::CodexAppserver,
            make_source_response(events, Some("codex-app:2"), t, SourceHealthStatus::Healthy),
        );

        let resp = gw.pull_events(&GatewayPullRequest {
            cursor: None,
            limit: 500,
        });

        assert_eq!(resp.events.len(), 2);
        assert_eq!(resp.events[0].event_id, "e1");
        assert_eq!(resp.events[1].event_id, "e2");
        assert_eq!(resp.next_cursor, Some("gw:2".to_string()));
    }

    // ── 3. Multi-source chronological merge ──────────────────────────

    #[test]
    fn multi_source_chronological_merge() {
        let mut gw = Gateway::new();
        let t = now();

        // Codex events at t+0s and t+2s
        let codex_events = vec![
            make_event("codex-1", Provider::Codex, SourceKind::CodexAppserver, t),
            make_event(
                "codex-2",
                Provider::Codex,
                SourceKind::CodexAppserver,
                t + TimeDelta::seconds(2),
            ),
        ];
        gw.ingest_source_response(
            SourceKind::CodexAppserver,
            make_source_response(
                codex_events,
                Some("codex-app:2"),
                t,
                SourceHealthStatus::Healthy,
            ),
        );

        // Claude events at t+1s and t+3s
        let claude_events = vec![
            make_event(
                "claude-1",
                Provider::Claude,
                SourceKind::ClaudeHooks,
                t + TimeDelta::seconds(1),
            ),
            make_event(
                "claude-2",
                Provider::Claude,
                SourceKind::ClaudeHooks,
                t + TimeDelta::seconds(3),
            ),
        ];
        gw.ingest_source_response(
            SourceKind::ClaudeHooks,
            make_source_response(
                claude_events,
                Some("claude-hooks:2"),
                t,
                SourceHealthStatus::Healthy,
            ),
        );

        let resp = gw.pull_events(&GatewayPullRequest {
            cursor: None,
            limit: 500,
        });

        // Should be interleaved chronologically: codex-1, claude-1, codex-2, claude-2
        assert_eq!(resp.events.len(), 4);
        assert_eq!(resp.events[0].event_id, "codex-1");
        assert_eq!(resp.events[1].event_id, "claude-1");
        assert_eq!(resp.events[2].event_id, "codex-2");
        assert_eq!(resp.events[3].event_id, "claude-2");
    }

    // ── 4. Gateway cursor pagination ────────────────────────────────

    #[test]
    fn gateway_cursor_pagination() {
        let mut gw = Gateway::new();
        let t = now();

        for i in 0..5 {
            let events = vec![make_event(
                &format!("e{i}"),
                Provider::Codex,
                SourceKind::CodexAppserver,
                t + TimeDelta::seconds(i),
            )];
            gw.ingest_source_response(
                SourceKind::CodexAppserver,
                make_source_response(
                    events,
                    Some(&format!("codex-app:{}", i + 1)),
                    t,
                    SourceHealthStatus::Healthy,
                ),
            );
        }

        // Page 1: limit 2
        let resp1 = gw.pull_events(&GatewayPullRequest {
            cursor: None,
            limit: 2,
        });
        assert_eq!(resp1.events.len(), 2);
        assert_eq!(resp1.events[0].event_id, "e0");
        assert_eq!(resp1.events[1].event_id, "e1");
        assert_eq!(resp1.next_cursor, Some("gw:2".to_string()));

        // Page 2: from cursor
        let resp2 = gw.pull_events(&GatewayPullRequest {
            cursor: resp1.next_cursor,
            limit: 2,
        });
        assert_eq!(resp2.events.len(), 2);
        assert_eq!(resp2.events[0].event_id, "e2");
        assert_eq!(resp2.events[1].event_id, "e3");
        assert_eq!(resp2.next_cursor, Some("gw:4".to_string()));

        // Page 3: last event
        let resp3 = gw.pull_events(&GatewayPullRequest {
            cursor: resp2.next_cursor,
            limit: 2,
        });
        assert_eq!(resp3.events.len(), 1);
        assert_eq!(resp3.events[0].event_id, "e4");
        assert_eq!(resp3.next_cursor, Some("gw:5".to_string()));

        // Page 4: no more events
        let resp4 = gw.pull_events(&GatewayPullRequest {
            cursor: resp3.next_cursor.clone(),
            limit: 2,
        });
        assert!(resp4.events.is_empty());
        assert_eq!(resp4.next_cursor, resp3.next_cursor);
    }

    // ── 5. Per-source cursor tracking ───────────────────────────────

    #[test]
    fn source_cursor_tracking() {
        let mut gw = Gateway::new();
        let t = now();

        // Initially no cursor
        assert!(gw.source_cursor(SourceKind::CodexAppserver).is_none());

        // First ingest
        gw.ingest_source_response(
            SourceKind::CodexAppserver,
            make_source_response(
                vec![make_event(
                    "e1",
                    Provider::Codex,
                    SourceKind::CodexAppserver,
                    t,
                )],
                Some("codex-app:1"),
                t,
                SourceHealthStatus::Healthy,
            ),
        );
        assert_eq!(
            gw.source_cursor(SourceKind::CodexAppserver),
            Some("codex-app:1")
        );

        // Second ingest advances cursor
        gw.ingest_source_response(
            SourceKind::CodexAppserver,
            make_source_response(
                vec![make_event(
                    "e2",
                    Provider::Codex,
                    SourceKind::CodexAppserver,
                    t + TimeDelta::seconds(1),
                )],
                Some("codex-app:2"),
                t,
                SourceHealthStatus::Healthy,
            ),
        );
        assert_eq!(
            gw.source_cursor(SourceKind::CodexAppserver),
            Some("codex-app:2")
        );
    }

    // ── 6. Source health tracking ───────────────────────────────────

    #[test]
    fn source_health_tracking() {
        let t = now();
        let mut gw =
            Gateway::with_sources(&[SourceKind::CodexAppserver, SourceKind::ClaudeHooks], t);

        // Initially all Down
        let health = gw.list_source_health();
        assert_eq!(health.len(), 2);
        for (_, report) in &health {
            assert_eq!(report.status, SourceHealthStatus::Down);
        }

        // Codex reports Healthy
        gw.ingest_source_response(
            SourceKind::CodexAppserver,
            make_source_response(vec![], Some("codex-app:0"), t, SourceHealthStatus::Healthy),
        );

        let codex_health = gw
            .source_health(SourceKind::CodexAppserver)
            .expect("codex health");
        assert_eq!(codex_health.status, SourceHealthStatus::Healthy);

        // Claude still Down
        let claude_health = gw
            .source_health(SourceKind::ClaudeHooks)
            .expect("claude health");
        assert_eq!(claude_health.status, SourceHealthStatus::Down);
    }

    // ── 7. Source health transitions ────────────────────────────────

    #[test]
    fn source_health_transitions() {
        let mut gw = Gateway::new();
        let t = now();

        // Source starts Healthy
        gw.ingest_source_response(
            SourceKind::Poller,
            make_source_response(vec![], None, t, SourceHealthStatus::Healthy),
        );
        assert_eq!(
            gw.source_health(SourceKind::Poller)
                .expect("poller health")
                .status,
            SourceHealthStatus::Healthy
        );

        // Source becomes Degraded
        gw.ingest_source_response(
            SourceKind::Poller,
            make_source_response(
                vec![],
                None,
                t + TimeDelta::seconds(5),
                SourceHealthStatus::Degraded,
            ),
        );
        assert_eq!(
            gw.source_health(SourceKind::Poller)
                .expect("poller health")
                .status,
            SourceHealthStatus::Degraded
        );

        // Source recovers to Healthy
        gw.ingest_source_response(
            SourceKind::Poller,
            make_source_response(
                vec![],
                None,
                t + TimeDelta::seconds(10),
                SourceHealthStatus::Healthy,
            ),
        );
        assert_eq!(
            gw.source_health(SourceKind::Poller)
                .expect("poller health")
                .status,
            SourceHealthStatus::Healthy
        );
    }

    // ── 8. Empty source response (heartbeat only) ───────────────────

    #[test]
    fn empty_source_response_heartbeat_only() {
        let mut gw = Gateway::new();
        let t = now();

        gw.ingest_source_response(
            SourceKind::CodexAppserver,
            make_source_response(vec![], Some("codex-app:0"), t, SourceHealthStatus::Healthy),
        );

        // No events in buffer
        assert_eq!(gw.buffer_len(), 0);
        // But source is tracked
        assert_eq!(
            gw.source_cursor(SourceKind::CodexAppserver),
            Some("codex-app:0")
        );
        assert_eq!(
            gw.source_health(SourceKind::CodexAppserver)
                .expect("health")
                .status,
            SourceHealthStatus::Healthy
        );
    }

    // ── 9. with_sources pre-registers sources ──────────────────────

    #[test]
    fn with_sources_pre_registers() {
        let t = now();
        let gw = Gateway::with_sources(
            &[
                SourceKind::CodexAppserver,
                SourceKind::ClaudeHooks,
                SourceKind::Poller,
            ],
            t,
        );

        let health = gw.list_source_health();
        assert_eq!(health.len(), 3);
        // All initially Down
        for (_, report) in &health {
            assert_eq!(report.status, SourceHealthStatus::Down);
        }
    }

    // ── 10. source_cursor_states snapshot ───────────────────────────

    #[test]
    fn source_cursor_states_snapshot() {
        let mut gw = Gateway::new();
        let t = now();

        gw.ingest_source_response(
            SourceKind::CodexAppserver,
            make_source_response(
                vec![make_event(
                    "e1",
                    Provider::Codex,
                    SourceKind::CodexAppserver,
                    t,
                )],
                Some("codex-app:1"),
                t,
                SourceHealthStatus::Healthy,
            ),
        );

        let states = gw.source_cursor_states(t);
        assert_eq!(states.len(), 1);
        assert_eq!(states[0].source_kind, SourceKind::CodexAppserver);
        assert_eq!(states[0].committed_cursor, Some("codex-app:1".to_string()));
    }

    // ── 11. Invalid gateway cursor falls back to start ──────────────

    #[test]
    fn invalid_gateway_cursor_falls_back_to_start() {
        let mut gw = Gateway::new();
        let t = now();

        gw.ingest_source_response(
            SourceKind::CodexAppserver,
            make_source_response(
                vec![make_event(
                    "e1",
                    Provider::Codex,
                    SourceKind::CodexAppserver,
                    t,
                )],
                Some("codex-app:1"),
                t,
                SourceHealthStatus::Healthy,
            ),
        );

        let resp = gw.pull_events(&GatewayPullRequest {
            cursor: Some("garbage".to_string()),
            limit: 500,
        });

        // Invalid cursor falls back to position 0
        assert_eq!(resp.events.len(), 1);
        assert_eq!(resp.events[0].event_id, "e1");
    }

    // ── 12. Global sequence counter ─────────────────────────────────

    #[test]
    fn global_sequence_counter() {
        let mut gw = Gateway::new();
        let t = now();

        assert_eq!(gw.global_seq(), 0);

        gw.ingest_source_response(
            SourceKind::CodexAppserver,
            make_source_response(
                vec![
                    make_event("e1", Provider::Codex, SourceKind::CodexAppserver, t),
                    make_event(
                        "e2",
                        Provider::Codex,
                        SourceKind::CodexAppserver,
                        t + TimeDelta::seconds(1),
                    ),
                ],
                Some("codex-app:2"),
                t,
                SourceHealthStatus::Healthy,
            ),
        );

        assert_eq!(gw.global_seq(), 2);

        gw.ingest_source_response(
            SourceKind::ClaudeHooks,
            make_source_response(
                vec![make_event(
                    "c1",
                    Provider::Claude,
                    SourceKind::ClaudeHooks,
                    t,
                )],
                Some("claude-hooks:1"),
                t,
                SourceHealthStatus::Healthy,
            ),
        );

        assert_eq!(gw.global_seq(), 3);
    }

    // ── 13. list_source_health sorted output ────────────────────────

    #[test]
    fn list_source_health_sorted() {
        let t = now();
        let gw = Gateway::with_sources(
            &[
                SourceKind::Poller,
                SourceKind::CodexAppserver,
                SourceKind::ClaudeHooks,
            ],
            t,
        );

        let health = gw.list_source_health();
        assert_eq!(health.len(), 3);

        // Should be sorted by as_str(): claude_hooks, codex_appserver, poller
        assert_eq!(health[0].0, SourceKind::ClaudeHooks);
        assert_eq!(health[1].0, SourceKind::CodexAppserver);
        assert_eq!(health[2].0, SourceKind::Poller);
    }

    // ── 14. Poller events merge with deterministic events ───────────

    #[test]
    fn poller_events_merge_with_deterministic() {
        let mut gw = Gateway::new();
        let t = now();

        // Poller at t+0
        gw.ingest_source_response(
            SourceKind::Poller,
            make_source_response(
                vec![make_event("poll-1", Provider::Codex, SourceKind::Poller, t)],
                Some("poller:1"),
                t,
                SourceHealthStatus::Healthy,
            ),
        );

        // Deterministic at t-1 (earlier timestamp but ingested later)
        gw.ingest_source_response(
            SourceKind::CodexAppserver,
            make_source_response(
                vec![make_event(
                    "det-1",
                    Provider::Codex,
                    SourceKind::CodexAppserver,
                    t - TimeDelta::seconds(1),
                )],
                Some("codex-app:1"),
                t,
                SourceHealthStatus::Healthy,
            ),
        );

        let resp = gw.pull_events(&GatewayPullRequest {
            cursor: None,
            limit: 500,
        });

        // det-1 (t-1) should come before poll-1 (t) after chronological sort
        assert_eq!(resp.events.len(), 2);
        assert_eq!(resp.events[0].event_id, "det-1");
        assert_eq!(resp.events[1].event_id, "poll-1");
    }

    // ── 15. Multiple ingestions from same source accumulate ─────────

    #[test]
    fn multiple_ingestions_from_same_source() {
        let mut gw = Gateway::new();
        let t = now();

        // First batch
        gw.ingest_source_response(
            SourceKind::CodexAppserver,
            make_source_response(
                vec![make_event(
                    "e1",
                    Provider::Codex,
                    SourceKind::CodexAppserver,
                    t,
                )],
                Some("codex-app:1"),
                t,
                SourceHealthStatus::Healthy,
            ),
        );

        // Second batch
        gw.ingest_source_response(
            SourceKind::CodexAppserver,
            make_source_response(
                vec![make_event(
                    "e2",
                    Provider::Codex,
                    SourceKind::CodexAppserver,
                    t + TimeDelta::seconds(1),
                )],
                Some("codex-app:2"),
                t + TimeDelta::seconds(1),
                SourceHealthStatus::Healthy,
            ),
        );

        assert_eq!(gw.buffer_len(), 2);
        assert_eq!(gw.global_seq(), 2);

        let resp = gw.pull_events(&GatewayPullRequest {
            cursor: None,
            limit: 500,
        });
        assert_eq!(resp.events.len(), 2);
        assert_eq!(resp.events[0].event_id, "e1");
        assert_eq!(resp.events[1].event_id, "e2");
    }

    // ── 16. Cursor past end returns empty ───────────────────────────

    #[test]
    fn cursor_past_end_returns_empty() {
        let mut gw = Gateway::new();
        let t = now();

        gw.ingest_source_response(
            SourceKind::CodexAppserver,
            make_source_response(
                vec![make_event(
                    "e1",
                    Provider::Codex,
                    SourceKind::CodexAppserver,
                    t,
                )],
                Some("codex-app:1"),
                t,
                SourceHealthStatus::Healthy,
            ),
        );

        let resp = gw.pull_events(&GatewayPullRequest {
            cursor: Some("gw:999".to_string()),
            limit: 500,
        });

        assert!(resp.events.is_empty());
        assert_eq!(resp.next_cursor, Some("gw:999".to_string()));
    }

    // ── 17. Default gateway is empty ────────────────────────────────

    #[test]
    fn default_gateway_is_empty() {
        let gw = Gateway::default();
        assert_eq!(gw.buffer_len(), 0);
        assert_eq!(gw.global_seq(), 0);
        assert!(gw.list_source_health().is_empty());
    }

    // ── 18. Three-source integration scenario ───────────────────────

    #[test]
    fn three_source_integration() {
        let t = now();
        let mut gw = Gateway::with_sources(
            &[
                SourceKind::CodexAppserver,
                SourceKind::ClaudeHooks,
                SourceKind::Poller,
            ],
            t,
        );

        // Codex at t+1
        gw.ingest_source_response(
            SourceKind::CodexAppserver,
            make_source_response(
                vec![make_event(
                    "codex-1",
                    Provider::Codex,
                    SourceKind::CodexAppserver,
                    t + TimeDelta::seconds(1),
                )],
                Some("codex-app:1"),
                t + TimeDelta::seconds(1),
                SourceHealthStatus::Healthy,
            ),
        );

        // Claude at t+0
        gw.ingest_source_response(
            SourceKind::ClaudeHooks,
            make_source_response(
                vec![make_event(
                    "claude-1",
                    Provider::Claude,
                    SourceKind::ClaudeHooks,
                    t,
                )],
                Some("claude-hooks:1"),
                t,
                SourceHealthStatus::Healthy,
            ),
        );

        // Poller at t+2
        gw.ingest_source_response(
            SourceKind::Poller,
            make_source_response(
                vec![make_event(
                    "poll-1",
                    Provider::Claude,
                    SourceKind::Poller,
                    t + TimeDelta::seconds(2),
                )],
                Some("poller:1"),
                t + TimeDelta::seconds(2),
                SourceHealthStatus::Healthy,
            ),
        );

        // All events merged chronologically
        let resp = gw.pull_events(&GatewayPullRequest {
            cursor: None,
            limit: 500,
        });
        assert_eq!(resp.events.len(), 3);
        assert_eq!(resp.events[0].event_id, "claude-1"); // t+0
        assert_eq!(resp.events[1].event_id, "codex-1"); // t+1
        assert_eq!(resp.events[2].event_id, "poll-1"); // t+2

        // All sources healthy
        let health = gw.list_source_health();
        assert_eq!(health.len(), 3);
        for (_, report) in &health {
            assert_eq!(report.status, SourceHealthStatus::Healthy);
        }

        // Cursors tracked independently
        assert_eq!(
            gw.source_cursor(SourceKind::CodexAppserver),
            Some("codex-app:1")
        );
        assert_eq!(
            gw.source_cursor(SourceKind::ClaudeHooks),
            Some("claude-hooks:1")
        );
        assert_eq!(gw.source_cursor(SourceKind::Poller), Some("poller:1"));
    }

    // ── 19. Cursor parsing edge cases ───────────────────────────────

    #[test]
    fn cursor_parsing_edge_cases() {
        assert_eq!(parse_gateway_cursor(None), 0);
        assert_eq!(parse_gateway_cursor(Some("gw:0")), 0);
        assert_eq!(parse_gateway_cursor(Some("gw:42")), 42);
        assert_eq!(parse_gateway_cursor(Some("gw:abc")), 0); // invalid number
        assert_eq!(parse_gateway_cursor(Some("wrong:5")), 0); // wrong prefix
        assert_eq!(parse_gateway_cursor(Some("")), 0); // empty
    }

    // ── 20. Source health report checked_at updated ─────────────────

    #[test]
    fn source_health_checked_at_updated() {
        let mut gw = Gateway::new();
        let t1 = now();
        let t2 = t1 + TimeDelta::seconds(5);

        gw.ingest_source_response(
            SourceKind::CodexAppserver,
            make_source_response(vec![], None, t1, SourceHealthStatus::Healthy),
        );
        let h1 = gw
            .source_health(SourceKind::CodexAppserver)
            .expect("health");
        assert_eq!(h1.checked_at, t1);

        gw.ingest_source_response(
            SourceKind::CodexAppserver,
            make_source_response(vec![], None, t2, SourceHealthStatus::Healthy),
        );
        let h2 = gw
            .source_health(SourceKind::CodexAppserver)
            .expect("health");
        assert_eq!(h2.checked_at, t2);
    }

    // ── 21. Same-timestamp events maintain ingest order ─────────────

    #[test]
    fn same_timestamp_ingest_order_preserved() {
        let mut gw = Gateway::new();
        let t = now();

        // All events at exact same timestamp
        gw.ingest_source_response(
            SourceKind::CodexAppserver,
            make_source_response(
                vec![
                    make_event("a", Provider::Codex, SourceKind::CodexAppserver, t),
                    make_event("b", Provider::Codex, SourceKind::CodexAppserver, t),
                ],
                Some("codex-app:2"),
                t,
                SourceHealthStatus::Healthy,
            ),
        );

        gw.ingest_source_response(
            SourceKind::ClaudeHooks,
            make_source_response(
                vec![make_event(
                    "c",
                    Provider::Claude,
                    SourceKind::ClaudeHooks,
                    t,
                )],
                Some("claude-hooks:1"),
                t,
                SourceHealthStatus::Healthy,
            ),
        );

        let resp = gw.pull_events(&GatewayPullRequest {
            cursor: None,
            limit: 500,
        });

        // All same timestamp → stable sort preserves ingest order: a, b, c
        assert_eq!(resp.events.len(), 3);
        assert_eq!(resp.events[0].event_id, "a");
        assert_eq!(resp.events[1].event_id, "b");
        assert_eq!(resp.events[2].event_id, "c");
    }

    // ── 22. Unregistered source health query returns None ───────────

    #[test]
    fn unregistered_source_health_returns_none() {
        let gw = Gateway::new();
        assert!(gw.source_health(SourceKind::CodexAppserver).is_none());
    }

    // ── 23. No re-delivery: source cursor always overwritten ────────

    #[test]
    fn no_redelivery_source_cursor_always_overwritten() {
        let mut gw = Gateway::new();
        let t = now();

        // First ingest: 1 event, cursor advances to codex-app:1
        gw.ingest_source_response(
            SourceKind::CodexAppserver,
            make_source_response(
                vec![make_event(
                    "e1",
                    Provider::Codex,
                    SourceKind::CodexAppserver,
                    t,
                )],
                Some("codex-app:1"),
                t,
                SourceHealthStatus::Healthy,
            ),
        );
        assert_eq!(
            gw.source_cursor(SourceKind::CodexAppserver),
            Some("codex-app:1")
        );

        // Second ingest: caught up, no new events but cursor stays at codex-app:1
        gw.ingest_source_response(
            SourceKind::CodexAppserver,
            make_source_response(
                vec![],
                Some("codex-app:1"),
                t + TimeDelta::seconds(1),
                SourceHealthStatus::Healthy,
            ),
        );
        // Cursor must still be tracked (not reset to None)
        assert_eq!(
            gw.source_cursor(SourceKind::CodexAppserver),
            Some("codex-app:1")
        );
        // No new events in buffer
        assert_eq!(gw.buffer_len(), 1);
    }

    // ── 24. Commit cursor compacts buffer ──────────────────────────

    #[test]
    fn commit_cursor_compacts_buffer() {
        let mut gw = Gateway::new();
        let t = now();

        gw.ingest_source_response(
            SourceKind::CodexAppserver,
            make_source_response(
                vec![
                    make_event("e1", Provider::Codex, SourceKind::CodexAppserver, t),
                    make_event(
                        "e2",
                        Provider::Codex,
                        SourceKind::CodexAppserver,
                        t + TimeDelta::seconds(1),
                    ),
                ],
                Some("codex-app:2"),
                t,
                SourceHealthStatus::Healthy,
            ),
        );
        assert_eq!(gw.buffer_len(), 2);

        // Commit cursor at position 1 → compact first event
        gw.commit_cursor("gw:1");
        assert_eq!(gw.buffer_len(), 1);

        // Pull from absolute position 1 still works
        let resp = gw.pull_events(&GatewayPullRequest {
            cursor: Some("gw:1".to_string()),
            limit: 500,
        });
        assert_eq!(resp.events.len(), 1);
        assert_eq!(resp.events[0].event_id, "e2");

        // Commit all → buffer empty
        gw.commit_cursor("gw:2");
        assert_eq!(gw.buffer_len(), 0);
    }

    // ── 25. compact_before with pagination ──────────────────────────

    #[test]
    fn compact_before_with_pagination() {
        let mut gw = Gateway::new();
        let t = now();

        // Ingest 5 events
        for i in 0..5 {
            gw.ingest_source_response(
                SourceKind::CodexAppserver,
                make_source_response(
                    vec![make_event(
                        &format!("e{i}"),
                        Provider::Codex,
                        SourceKind::CodexAppserver,
                        t + TimeDelta::seconds(i),
                    )],
                    Some(&format!("codex-app:{}", i + 1)),
                    t,
                    SourceHealthStatus::Healthy,
                ),
            );
        }
        assert_eq!(gw.buffer_len(), 5);

        // Pull first 3
        let resp = gw.pull_events(&GatewayPullRequest {
            cursor: None,
            limit: 3,
        });
        assert_eq!(resp.events.len(), 3);
        let cursor = resp.next_cursor.expect("has cursor");
        assert_eq!(cursor, "gw:3");

        // Compact first 3
        gw.compact_before(3);
        assert_eq!(gw.buffer_len(), 2);

        // Pull remaining with old cursor
        let resp2 = gw.pull_events(&GatewayPullRequest {
            cursor: Some(cursor),
            limit: 500,
        });
        assert_eq!(resp2.events.len(), 2);
        assert_eq!(resp2.events[0].event_id, "e3");
        assert_eq!(resp2.events[1].event_id, "e4");
        assert_eq!(resp2.next_cursor, Some("gw:5".to_string()));
    }

    /// F2 regression: stale cursor (before compact_offset) must not produce
    /// a next_cursor that causes re-delivery.
    #[test]
    fn stale_cursor_after_compaction_no_redelivery() {
        let mut gw = Gateway::new();
        let t = now();

        // Ingest 4 events (abs positions 0..4)
        for i in 0..4 {
            gw.ingest_source_response(
                SourceKind::CodexAppserver,
                make_source_response(
                    vec![make_event(
                        &format!("e{i}"),
                        Provider::Codex,
                        SourceKind::CodexAppserver,
                        t + TimeDelta::seconds(i),
                    )],
                    Some(&format!("codex-app:{}", i + 1)),
                    t,
                    SourceHealthStatus::Healthy,
                ),
            );
        }

        // Compact first 3 → buffer = [e3], compact_offset = 3
        gw.compact_before(3);
        assert_eq!(gw.buffer_len(), 1);

        // Pull with stale cursor "gw:1" (before compact_offset=3)
        let resp = gw.pull_events(&GatewayPullRequest {
            cursor: Some("gw:1".to_string()),
            limit: 500,
        });
        assert_eq!(resp.events.len(), 1, "should get remaining event e3");
        assert_eq!(resp.events[0].event_id, "e3");
        // next_cursor must be gw:4 (compact_offset + 1), not gw:2 (stale abs_start + 1)
        assert_eq!(
            resp.next_cursor,
            Some("gw:4".to_string()),
            "next_cursor must account for compact_offset, not stale abs_start"
        );

        // Re-pull with the returned cursor: no re-delivery
        let resp2 = gw.pull_events(&GatewayPullRequest {
            cursor: resp.next_cursor,
            limit: 500,
        });
        assert!(
            resp2.events.is_empty(),
            "no re-delivery after stale cursor pull"
        );
    }
}

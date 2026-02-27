//! Source server logic for Claude JSONL: cursor management, event storage,
//! health reporting, and file watcher orchestration.

use std::collections::HashMap;

use agtmux_core_v5::types::{
    EvidenceTier, Provider, PullEventsRequest, PullEventsResponse, SourceEventV2,
    SourceHealthReport, SourceHealthStatus, SourceKind,
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
    /// When a JSONL file is discovered but no real (non-metadata) lines are
    /// found this tick — e.g. idle pane after daemon restart — a heartbeat
    /// event (`is_heartbeat = true`, `activity.idle`) is emitted so that
    /// `deterministic_last_seen` stays fresh and the resolver does not fall
    /// back to heuristic or mis-attribute the pane to another provider.
    ///
    /// Manages watcher lifecycle (create/reuse/remove).
    pub fn poll_files(
        watchers: &mut HashMap<String, SessionFileWatcher>,
        discoveries: &[SessionDiscovery],
        now: DateTime<Utc>,
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

            let mut emitted_real_event = false;
            for line_str in &new_lines {
                match serde_json::from_str::<ClaudeJsonlLine>(line_str) {
                    Ok(parsed) => {
                        if let Some(event) = translate::translate(&parsed, &ctx) {
                            emitted_real_event = true;
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

            // Bootstrap or heartbeat: keep Claude deterministic evidence alive when idle.
            //
            // Codex App Server re-emits unchanged thread status every ~2s as a
            // heartbeat.  Without a symmetric signal, an idle Claude pane (no
            // new JSONL lines since daemon restart) would have zero deterministic
            // evidence while Codex still has fresh heartbeats — causing a
            // false-positive Codex attribution.
            //
            // On the very FIRST poll of a newly-created watcher (bootstrapped=false):
            //   - If real events were emitted: `last_real_activity[Claude]` is already set;
            //     no additional bootstrap needed.  We just mark bootstrapped.
            //   - If no real events: emit a bootstrap event with `is_heartbeat=false`.
            //     This writes `last_real_activity[Claude]` so `select_winning_provider`
            //     can compare it against Codex's own `last_real_activity` (set on its
            //     first thread detection).  Because Step 6b runs after Step 6a in each
            //     tick, the bootstrap `observed_at=now()` is slightly newer than Codex's
            //     initial detection → Claude wins the provider conflict for that pane.
            //
            // On all subsequent polls (bootstrapped=true), emit `is_heartbeat=true` to
            // keep `deterministic_last_seen` fresh without biasing the arbitration.
            if !watcher.is_bootstrapped() {
                // First poll: mark bootstrapped regardless; emit bootstrap only if no real events.
                if !emitted_real_event {
                    if discovery.cwd_candidate_count > 1 {
                        // Multiple panes share this CWD (e.g. Codex + Claude in the same
                        // project dir).  Emitting a full bootstrap (is_heartbeat=false) for
                        // every pane would write last_real_activity[Claude] for ALL of them,
                        // causing Claude to win provider arbitration even for Codex panes.
                        // Instead, emit an ambiguous bootstrap (is_heartbeat=true) that only
                        // refreshes deterministic_last_seen — leaving last_real_activity
                        // untouched so select_winning_provider can resolve via actual events.
                        events.push(ambiguous_cwd_bootstrap(discovery, now));
                    } else {
                        events.push(bootstrap_event(discovery, now));
                    }
                }
                watcher.mark_bootstrapped();
            } else if !emitted_real_event {
                events.push(idle_heartbeat(discovery, now));
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

/// Build a one-shot bootstrap event emitted on the first poll of a newly-created watcher.
///
/// Setting `is_heartbeat = false` writes `last_real_activity[Claude]` in the projection,
/// enabling `select_winning_provider` to resolve Codex vs. Claude conflicts when both
/// have sessions at the same CWD (e.g. a stale Codex App Server thread vs. an idle
/// Claude JSONL session after daemon restart).
///
/// `observed_at = now` (not the JSONL file's last-line timestamp) ensures the event
/// is fresh and doesn't trigger the resolver's stale-detection threshold.
fn bootstrap_event(discovery: &SessionDiscovery, now: DateTime<Utc>) -> SourceEventV2 {
    SourceEventV2 {
        event_id: format!("claude-jsonl-boot-{}", discovery.pane_id),
        provider: Provider::Claude,
        source_kind: SourceKind::ClaudeJsonl,
        tier: EvidenceTier::Deterministic,
        observed_at: now,
        session_key: discovery.session_id.clone(),
        pane_id: Some(discovery.pane_id.clone()),
        pane_generation: discovery.pane_generation,
        pane_birth_ts: discovery.pane_birth_ts,
        source_event_id: None,
        event_type: "activity.idle".to_owned(),
        payload: serde_json::json!({}),
        confidence: 1.0,
        is_heartbeat: false, // KEY: updates last_real_activity in the projection
    }
}

/// Build a bootstrap event for the ambiguous multi-pane case.
///
/// Emitted on the FIRST poll when `cwd_candidate_count > 1` (multiple panes
/// share the same CWD) and no real events were observed.
///
/// Setting `is_heartbeat = true` keeps the semantics of an idle heartbeat:
/// it refreshes `deterministic_last_seen` without writing `last_real_activity`.
/// This prevents a false-positive Claude attribution for panes that are
/// actually running Codex (or another agent) in the same project directory.
/// `select_winning_provider` will correctly award that pane to the provider
/// that has actual `last_real_activity` evidence (e.g. Codex App Server events).
fn ambiguous_cwd_bootstrap(discovery: &SessionDiscovery, now: DateTime<Utc>) -> SourceEventV2 {
    SourceEventV2 {
        event_id: format!("claude-jsonl-ambi-boot-{}", discovery.pane_id),
        provider: Provider::Claude,
        source_kind: SourceKind::ClaudeJsonl,
        tier: EvidenceTier::Deterministic,
        observed_at: now,
        session_key: discovery.session_id.clone(),
        pane_id: Some(discovery.pane_id.clone()),
        pane_generation: discovery.pane_generation,
        pane_birth_ts: discovery.pane_birth_ts,
        source_event_id: None,
        event_type: "activity.idle".to_owned(),
        payload: serde_json::json!({}),
        confidence: 1.0,
        is_heartbeat: true, // KEY: does NOT write last_real_activity
    }
}

/// Build a deterministic idle heartbeat for a discovered Claude JSONL session.
///
/// Used when the JSONL file exists for a pane but no new real activity lines
/// have been observed this poll tick (e.g. idle session after daemon restart),
/// AND the watcher has already been bootstrapped.
/// Setting `is_heartbeat = true` lets the projection update
/// `deterministic_last_seen` without affecting `last_real_activity`, so
/// cross-provider arbitration is not biased toward the heartbeat emitter.
fn idle_heartbeat(discovery: &SessionDiscovery, now: DateTime<Utc>) -> SourceEventV2 {
    SourceEventV2 {
        event_id: format!("claude-jsonl-hb-{}", discovery.pane_id),
        provider: Provider::Claude,
        source_kind: SourceKind::ClaudeJsonl,
        tier: EvidenceTier::Deterministic,
        observed_at: now,
        session_key: discovery.session_id.clone(),
        pane_id: Some(discovery.pane_id.clone()),
        pane_generation: discovery.pane_generation,
        pane_birth_ts: discovery.pane_birth_ts,
        source_event_id: None,
        event_type: "activity.idle".to_owned(),
        payload: serde_json::json!({}),
        confidence: 1.0,
        is_heartbeat: true,
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
            cwd_candidate_count: 1,
        }];

        let mut watchers = HashMap::new();
        // Use a watcher that starts from position 0 for testing
        watchers.insert(
            "%1".to_owned(),
            SessionFileWatcher::new_from_start(jsonl_path),
        );

        let events = ClaudeJsonlSourceState::poll_files(&mut watchers, &discoveries, now());
        // user + tool_use = 2 real events; no heartbeat because emitted_real_event = true
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, "activity.user_input");
        assert_eq!(events[0].provider, Provider::Claude);
        assert_eq!(events[0].source_kind, SourceKind::ClaudeJsonl);
        assert_eq!(events[0].tier, EvidenceTier::Deterministic);
        assert_eq!(events[1].event_type, "activity.running");
        assert!(!events[0].is_heartbeat);
        assert!(!events[1].is_heartbeat);

        let _ = fs::remove_dir_all(&tmp);
    }

    /// After daemon restart, an idle Claude pane produces no new JSONL lines.
    ///
    /// T-126 bootstrap: the VERY FIRST poll emits a bootstrap event
    /// (`is_heartbeat=false`) to set `last_real_activity[Claude]`.  This lets
    /// `select_winning_provider` pick Claude over a stale Codex App Server thread.
    /// Subsequent polls emit the usual idle heartbeat (`is_heartbeat=true`).
    #[test]
    fn poll_files_emits_bootstrap_on_first_poll_when_no_new_lines() {
        use std::fs;

        let tmp = std::env::temp_dir().join("agtmux-test-hb-no-new-lines");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).expect("test");

        let jsonl_path = tmp.join("idle-session.jsonl");
        // Historical content already on disk — daemon starts at EOF, sees nothing new.
        fs::write(
            &jsonl_path,
            "{\"type\":\"assistant\",\"timestamp\":\"2026-02-25T10:00:00Z\",\"uuid\":\"u0\"}\n",
        )
        .expect("test");

        let discoveries = vec![SessionDiscovery {
            pane_id: "%9".to_owned(),
            session_id: "idle-sess".to_owned(),
            jsonl_path: jsonl_path.clone(),
            pane_generation: Some(3),
            pane_birth_ts: None,
            cwd_candidate_count: 1,
        }];

        let mut watchers = HashMap::new();
        // Watcher starts at EOF (production behaviour after daemon restart).
        watchers.insert("%9".to_owned(), SessionFileWatcher::new(jsonl_path));

        // FIRST poll → bootstrap event (is_heartbeat=false) to set last_real_activity.
        let events = ClaudeJsonlSourceState::poll_files(&mut watchers, &discoveries, now());
        assert_eq!(events.len(), 1, "expected exactly one bootstrap event");
        let boot = &events[0];
        assert!(
            !boot.is_heartbeat,
            "first poll must emit bootstrap (is_heartbeat=false)"
        );
        assert_eq!(boot.event_type, "activity.idle");
        assert_eq!(boot.provider, Provider::Claude);
        assert_eq!(boot.source_kind, SourceKind::ClaudeJsonl);
        assert_eq!(boot.tier, EvidenceTier::Deterministic);
        assert_eq!(boot.pane_id, Some("%9".to_owned()));
        assert_eq!(boot.pane_generation, Some(3));
        assert_eq!(boot.observed_at, now());

        // SECOND poll → idle heartbeat (is_heartbeat=true), bootstrap is done.
        let events2 = ClaudeJsonlSourceState::poll_files(&mut watchers, &discoveries, now());
        assert_eq!(
            events2.len(),
            1,
            "expected exactly one heartbeat on second poll"
        );
        let hb = &events2[0];
        assert!(
            hb.is_heartbeat,
            "subsequent polls must emit heartbeat (is_heartbeat=true)"
        );
        assert_eq!(hb.event_type, "activity.idle");

        let _ = fs::remove_dir_all(&tmp);
    }

    /// metadata-only lines (type=system) do not count as real events;
    /// the first poll emits a bootstrap (is_heartbeat=false) even in that case.
    #[test]
    fn poll_files_emits_bootstrap_when_only_metadata_lines() {
        use std::fs;
        use std::io::Write;

        let tmp = std::env::temp_dir().join("agtmux-test-hb-metadata-only");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).expect("test");

        let jsonl_path = tmp.join("meta-session.jsonl");
        fs::write(&jsonl_path, "").expect("test");

        let discoveries = vec![SessionDiscovery {
            pane_id: "%7".to_owned(),
            session_id: "meta-sess".to_owned(),
            jsonl_path: jsonl_path.clone(),
            pane_generation: None,
            pane_birth_ts: None,
            cwd_candidate_count: 1,
        }];

        let mut watchers = HashMap::new();
        watchers.insert(
            "%7".to_owned(),
            SessionFileWatcher::new_from_start(jsonl_path.clone()),
        );

        // Append only metadata lines (type=system is skipped by translate).
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&jsonl_path)
            .expect("test");
        writeln!(
            f,
            r#"{{"type":"system","timestamp":"2026-02-25T14:00:00Z","uuid":"sys1"}}"#
        )
        .expect("test");
        drop(f);

        // First poll: metadata lines don't count as real events → bootstrap emitted.
        let events = ClaudeJsonlSourceState::poll_files(&mut watchers, &discoveries, now());
        assert_eq!(
            events.len(),
            1,
            "expected one bootstrap event for metadata-only first tick"
        );
        assert!(
            !events[0].is_heartbeat,
            "first poll must be bootstrap (is_heartbeat=false)"
        );
        assert_eq!(events[0].event_type, "activity.idle");

        // Second poll (no new lines): heartbeat emitted.
        let events2 = ClaudeJsonlSourceState::poll_files(&mut watchers, &discoveries, now());
        assert_eq!(events2.len(), 1, "second poll emits heartbeat");
        assert!(events2[0].is_heartbeat, "subsequent poll must be heartbeat");

        let _ = fs::remove_dir_all(&tmp);
    }

    /// When `cwd_candidate_count > 1`, the first idle poll must emit an
    /// ambiguous bootstrap (`is_heartbeat=true`) instead of a full bootstrap
    /// (`is_heartbeat=false`).  This prevents Claude from winning
    /// `select_winning_provider` for Codex panes that share the same CWD.
    #[test]
    fn poll_files_emits_ambiguous_bootstrap_when_cwd_has_multiple_panes() {
        use std::fs;

        let tmp = std::env::temp_dir().join("agtmux-test-ambi-boot");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).expect("test");

        let jsonl_a = tmp.join("pane-a.jsonl");
        let jsonl_b = tmp.join("pane-b.jsonl");
        // Both watchers start at EOF (production behaviour): no new lines.
        fs::write(
            &jsonl_a,
            "{\"type\":\"assistant\",\"timestamp\":\"2026-02-27T10:00:00Z\",\"uuid\":\"ua\"}\n",
        )
        .expect("test");
        fs::write(
            &jsonl_b,
            "{\"type\":\"assistant\",\"timestamp\":\"2026-02-27T10:00:00Z\",\"uuid\":\"ub\"}\n",
        )
        .expect("test");

        // cwd_candidate_count=2 for both panes (shared CWD scenario).
        let discoveries = vec![
            SessionDiscovery {
                pane_id: "%35".to_owned(),
                session_id: "sess-a".to_owned(),
                jsonl_path: jsonl_a.clone(),
                pane_generation: Some(1),
                pane_birth_ts: None,
                cwd_candidate_count: 2,
            },
            SessionDiscovery {
                pane_id: "%297".to_owned(),
                session_id: "sess-b".to_owned(),
                jsonl_path: jsonl_b.clone(),
                pane_generation: Some(1),
                pane_birth_ts: None,
                cwd_candidate_count: 2,
            },
        ];

        let mut watchers = HashMap::new();
        watchers.insert("%35".to_owned(), SessionFileWatcher::new(jsonl_a));
        watchers.insert("%297".to_owned(), SessionFileWatcher::new(jsonl_b));

        // First poll: both panes idle + cwd_candidate_count=2 → ambiguous bootstrap
        let events = ClaudeJsonlSourceState::poll_files(&mut watchers, &discoveries, now());
        assert_eq!(events.len(), 2, "one event per pane");
        for ev in &events {
            assert!(
                ev.is_heartbeat,
                "pane {} with shared CWD must emit ambiguous bootstrap (is_heartbeat=true)",
                ev.pane_id.as_deref().unwrap_or("?")
            );
            assert_eq!(ev.event_type, "activity.idle");
        }

        // Second poll: still no new lines → regular idle heartbeat (same semantics)
        let events2 = ClaudeJsonlSourceState::poll_files(&mut watchers, &discoveries, now());
        assert_eq!(events2.len(), 2);
        for ev in &events2 {
            assert!(ev.is_heartbeat, "subsequent polls always emit heartbeat");
        }

        let _ = fs::remove_dir_all(&tmp);
    }
}

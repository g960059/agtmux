//! Source server logic for Codex JSONL: cursor management, event storage,
//! watcher orchestration, and FSM state tracking per pane.

use std::collections::HashMap;

use agtmux_core_v5::types::{
    PullEventsRequest, PullEventsResponse, SourceEventV2, SourceHealthReport, SourceHealthStatus,
};
use chrono::{DateTime, Utc};

use crate::discovery::{CodexPaneHint, CodexSessionDiscovery};
use crate::fsm::{CodexJsonlEvent, CodexSessionState, transition};
use crate::translate;
use crate::watcher::CodexSessionFileWatcher;

const CURSOR_PREFIX: &str = "codex-jsonl:";

/// Per-pane FSM tracking state.
#[derive(Debug)]
#[allow(dead_code)]
struct PaneFsmState {
    fsm: CodexSessionState,
    session_key: String,
    pane_generation: Option<u64>,
    pane_birth_ts: Option<DateTime<Utc>>,
    event_seq: u64,
    bootstrapped: bool,
}

/// In-memory cursor state for the Codex JSONL source.
#[derive(Debug, Default)]
pub struct CodexJsonlSourceState {
    events: Vec<SourceEventV2>,
    seq: u64,
    compact_offset: u64,
    has_received_events: bool,
    /// Per-pane FSM state.
    pane_fsm: HashMap<String, PaneFsmState>,
}

impl CodexJsonlSourceState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn ingest(&mut self, event: SourceEventV2) {
        self.events.push(event);
        self.seq += 1;
        self.has_received_events = true;
    }

    pub fn compact(&mut self, up_to_seq: u64) {
        let local_pos = up_to_seq.saturating_sub(self.compact_offset);
        #[expect(clippy::cast_possible_truncation)]
        let drain_count = (local_pos as usize).min(self.events.len());
        if drain_count > 0 {
            self.events.drain(..drain_count);
            self.compact_offset += drain_count as u64;
        }
    }

    pub fn buffered_len(&self) -> usize {
        self.events.len()
    }

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

    /// Poll JSONL files for new events, run FSM transitions, emit SourceEventV2s.
    pub fn poll_files(
        &mut self,
        watchers: &mut HashMap<String, CodexSessionFileWatcher>,
        discoveries: &[CodexSessionDiscovery],
        now: DateTime<Utc>,
    ) -> Vec<SourceEventV2> {
        let mut events = Vec::new();

        // Cleanup: remove watchers and FSM state for panes no longer discovered
        let active_ids: std::collections::HashSet<&str> =
            discoveries.iter().map(|d| d.pane_id.as_str()).collect();
        watchers.retain(|k, _| active_ids.contains(k.as_str()));
        self.pane_fsm.retain(|k, _| active_ids.contains(k.as_str()));

        for discovery in discoveries {
            // Create or reuse watcher
            let watcher = watchers
                .entry(discovery.pane_id.clone())
                .or_insert_with(|| CodexSessionFileWatcher::new(discovery.jsonl_path.clone()));

            // If watcher path changed (new session), reset
            if watcher.path() != discovery.jsonl_path {
                *watcher = CodexSessionFileWatcher::new(discovery.jsonl_path.clone());
                self.pane_fsm.remove(&discovery.pane_id);
            }

            // Create or reuse per-pane FSM state
            let fsm_state = self
                .pane_fsm
                .entry(discovery.pane_id.clone())
                .or_insert_with(|| PaneFsmState {
                    fsm: CodexSessionState::Init,
                    session_key: discovery.session_key.clone(),
                    pane_generation: discovery.pane_generation,
                    pane_birth_ts: discovery.pane_birth_ts,
                    event_seq: 0,
                    bootstrapped: false,
                });

            // Poll new lines and run FSM
            let new_lines = watcher.poll_new_lines();
            let mut emitted_real_event = false;

            for line_str in &new_lines {
                let Ok(codex_event) = serde_json::from_str::<CodexJsonlEvent>(line_str) else {
                    continue;
                };

                let old_state = fsm_state.fsm;
                let new_state = transition(old_state, &codex_event);

                if new_state != old_state {
                    fsm_state.fsm = new_state;
                    fsm_state.event_seq += 1;

                    if let Some(ev) = translate::translate_state_change(
                        new_state,
                        &discovery.session_key,
                        &discovery.pane_id,
                        discovery.pane_generation,
                        discovery.pane_birth_ts,
                        now,
                        fsm_state.event_seq,
                    ) {
                        emitted_real_event = true;
                        events.push(ev);
                    }
                }
            }

            // Bootstrap or heartbeat
            if !watcher.is_bootstrapped() {
                watcher.mark_bootstrapped();
                if !emitted_real_event {
                    events.push(translate::idle_heartbeat(
                        &discovery.session_key,
                        &discovery.pane_id,
                        discovery.pane_generation,
                        discovery.pane_birth_ts,
                        now,
                    ));
                }
            } else if !emitted_real_event {
                events.push(translate::idle_heartbeat(
                    &discovery.session_key,
                    &discovery.pane_id,
                    discovery.pane_generation,
                    discovery.pane_birth_ts,
                    now,
                ));
            }
        }

        events
    }

    /// Discover Codex sessions for given pane hints.
    pub fn discover_sessions(hints: &[CodexPaneHint]) -> Vec<CodexSessionDiscovery> {
        crate::discovery::discover_sessions(hints)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agtmux_core_v5::types::{EvidenceTier, Provider, SourceKind};
    use chrono::TimeZone;
    use std::fs;
    use std::io::Write;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 3, 2, 10, 0, 0)
            .single()
            .expect("valid datetime")
    }

    fn tmp_dir(label: &str) -> std::path::PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("test")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("agtmux-codex-source-{label}-{nonce}"));
        fs::create_dir_all(&dir).expect("test");
        dir
    }

    fn make_discovery(
        pane_id: &str,
        session_key: &str,
        path: std::path::PathBuf,
    ) -> CodexSessionDiscovery {
        CodexSessionDiscovery {
            pane_id: pane_id.to_owned(),
            session_key: session_key.to_owned(),
            jsonl_path: path,
            pane_generation: Some(1),
            pane_birth_ts: None,
        }
    }

    #[test]
    fn empty_state_returns_empty_pull() {
        let state = CodexJsonlSourceState::new();
        let req = PullEventsRequest {
            cursor: None,
            limit: 500,
        };
        let resp = state.pull_events(&req, now());
        assert!(resp.events.is_empty());
        assert_eq!(resp.next_cursor, Some("codex-jsonl:0".to_string()));
        assert_eq!(resp.source_health.status, SourceHealthStatus::Down);
    }

    #[test]
    fn poll_files_emits_running_on_task_started() {
        let tmp = tmp_dir("poll-running");
        let path = tmp.join("session.jsonl");
        let mut f = fs::File::create(&path).expect("test");
        writeln!(
            f,
            r#"{{"type":"event_msg","payload":{{"type":"task_started"}}}}"#
        )
        .expect("test");
        drop(f);

        let discoveries = vec![make_discovery("%1", "session", path.clone())];
        let mut watchers = HashMap::new();
        watchers.insert(
            "%1".to_owned(),
            CodexSessionFileWatcher::new_from_start(path),
        );

        let mut source = CodexJsonlSourceState::new();
        let events = source.poll_files(&mut watchers, &discoveries, now());

        // Should emit activity.running for task_started (Init → Running transition)
        let running_events: Vec<_> = events
            .iter()
            .filter(|e| e.event_type == "activity.running")
            .collect();
        assert!(
            !running_events.is_empty(),
            "task_started should produce activity.running event"
        );
        let ev = running_events[0];
        assert_eq!(ev.provider, Provider::Codex);
        assert_eq!(ev.source_kind, SourceKind::CodexJsonl);
        assert_eq!(ev.tier, EvidenceTier::Deterministic);
        assert!(!ev.is_heartbeat);

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn poll_files_emits_idle_on_task_complete() {
        let tmp = tmp_dir("poll-idle");
        let path = tmp.join("session.jsonl");
        let mut f = fs::File::create(&path).expect("test");
        writeln!(
            f,
            r#"{{"type":"event_msg","payload":{{"type":"task_started"}}}}"#
        )
        .expect("test");
        writeln!(
            f,
            r#"{{"type":"event_msg","payload":{{"type":"task_complete"}}}}"#
        )
        .expect("test");
        drop(f);

        let discoveries = vec![make_discovery("%2", "session", path.clone())];
        let mut watchers = HashMap::new();
        watchers.insert(
            "%2".to_owned(),
            CodexSessionFileWatcher::new_from_start(path),
        );

        let mut source = CodexJsonlSourceState::new();
        let events = source.poll_files(&mut watchers, &discoveries, now());

        // Should transition: Init→Running→WaitingInput
        // activity.idle (from WaitingInput) should be present
        let idle_real: Vec<_> = events
            .iter()
            .filter(|e| e.event_type == "activity.idle" && !e.is_heartbeat)
            .collect();
        assert!(
            !idle_real.is_empty(),
            "task_complete should produce activity.idle (non-heartbeat)"
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn poll_files_emits_waiting_approval() {
        let tmp = tmp_dir("poll-approval");
        let path = tmp.join("session.jsonl");
        let mut f = fs::File::create(&path).expect("test");
        writeln!(
            f,
            r#"{{"type":"event_msg","payload":{{"type":"task_started"}}}}"#
        )
        .expect("test");
        writeln!(
            f,
            r#"{{"type":"event_msg","payload":{{"type":"entered_review_mode"}}}}"#
        )
        .expect("test");
        drop(f);

        let discoveries = vec![make_discovery("%3", "session", path.clone())];
        let mut watchers = HashMap::new();
        watchers.insert(
            "%3".to_owned(),
            CodexSessionFileWatcher::new_from_start(path),
        );

        let mut source = CodexJsonlSourceState::new();
        let events = source.poll_files(&mut watchers, &discoveries, now());

        let approval_events: Vec<_> = events
            .iter()
            .filter(|e| e.event_type == "activity.waiting_approval")
            .collect();
        assert!(
            !approval_events.is_empty(),
            "entered_review_mode should produce activity.waiting_approval"
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn poll_files_heartbeat_when_no_state_change() {
        let tmp = tmp_dir("poll-heartbeat");
        let path = tmp.join("session.jsonl");
        fs::write(&path, "").expect("test");

        let discoveries = vec![make_discovery("%4", "session", path.clone())];
        let mut watchers = HashMap::new();
        watchers.insert(
            "%4".to_owned(),
            CodexSessionFileWatcher::new_from_start(path),
        );

        let mut source = CodexJsonlSourceState::new();

        // First poll: bootstrap heartbeat (file is empty)
        let events1 = source.poll_files(&mut watchers, &discoveries, now());
        assert_eq!(events1.len(), 1);
        // Bootstrap: is_heartbeat=true since no real events
        assert!(
            events1[0].is_heartbeat,
            "bootstrap with no events should be heartbeat"
        );

        // Second poll: idle heartbeat
        let events2 = source.poll_files(&mut watchers, &discoveries, now());
        assert_eq!(events2.len(), 1);
        assert!(
            events2[0].is_heartbeat,
            "subsequent poll with no events should be heartbeat"
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn poll_files_fsm_state_persists_across_polls() {
        let tmp = tmp_dir("poll-persist");
        let path = tmp.join("session.jsonl");
        fs::write(&path, "").expect("test");

        let discoveries = vec![make_discovery("%5", "session", path.clone())];
        let mut watchers = HashMap::new();
        watchers.insert(
            "%5".to_owned(),
            CodexSessionFileWatcher::new_from_start(path.clone()),
        );

        let mut source = CodexJsonlSourceState::new();

        // Poll 1: empty file
        let _ = source.poll_files(&mut watchers, &discoveries, now());

        // Append task_started
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("test");
        writeln!(
            f,
            r#"{{"type":"event_msg","payload":{{"type":"task_started"}}}}"#
        )
        .expect("test");
        drop(f);

        // Poll 2: should see Running
        let events2 = source.poll_files(&mut watchers, &discoveries, now());
        let running: Vec<_> = events2
            .iter()
            .filter(|e| e.event_type == "activity.running")
            .collect();
        assert!(
            !running.is_empty(),
            "task_started in second poll should produce running"
        );

        // Append token_count (no transition)
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("test");
        writeln!(
            f,
            r#"{{"type":"event_msg","payload":{{"type":"token_count","inputTokens":100}}}}"#
        )
        .expect("test");
        drop(f);

        // Poll 3: token_count doesn't change state → only heartbeat
        let events3 = source.poll_files(&mut watchers, &discoveries, now());
        let non_hb: Vec<_> = events3.iter().filter(|e| !e.is_heartbeat).collect();
        assert!(
            non_hb.is_empty(),
            "token_count should not produce non-heartbeat events"
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn cursor_pagination_works() {
        let mut state = CodexJsonlSourceState::new();

        let make_ev = |id: &str| SourceEventV2 {
            event_id: format!("codex-jsonl-{id}"),
            provider: Provider::Codex,
            source_kind: SourceKind::CodexJsonl,
            tier: EvidenceTier::Deterministic,
            observed_at: now(),
            session_key: "sess".to_owned(),
            pane_id: Some("%1".to_owned()),
            pane_generation: None,
            pane_birth_ts: None,
            source_event_id: None,
            event_type: "activity.running".to_owned(),
            payload: serde_json::json!({}),
            confidence: 1.0,
            is_heartbeat: false,
            actual_activity_at: None,
        };

        for i in 0..4 {
            state.ingest(make_ev(&format!("e{i}")));
        }

        let req1 = PullEventsRequest {
            cursor: None,
            limit: 2,
        };
        let resp1 = state.pull_events(&req1, now());
        assert_eq!(resp1.events.len(), 2);
        assert_eq!(resp1.next_cursor, Some("codex-jsonl:2".to_owned()));

        let req2 = PullEventsRequest {
            cursor: resp1.next_cursor,
            limit: 10,
        };
        let resp2 = state.pull_events(&req2, now());
        assert_eq!(resp2.events.len(), 2);
        assert_eq!(resp2.next_cursor, Some("codex-jsonl:4".to_owned()));
    }
}

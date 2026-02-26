//! Poll loop: wires tmux → poller → gateway → daemon pipeline.
//! Runs as a tokio task, polling tmux at configurable intervals.

use std::collections::HashSet;
use std::sync::Arc;

use chrono::Utc;
use tokio::sync::Mutex;
use tokio::time::{Duration, interval};

use agtmux_core_v5::types::{GatewayPullRequest, PullEventsRequest, SourceKind};
use agtmux_daemon_v5::projection::DaemonProjection;
use agtmux_gateway::gateway::Gateway;
use agtmux_source_poller::source::{PollerSourceState, poll_pane};
use agtmux_tmux_v5::{
    PaneGenerationTracker, TmuxCommandRunner, TmuxExecutor, TmuxPaneInfo, capture_pane, list_panes,
    to_pane_snapshot,
};

use crate::cli::DaemonOpts;
use crate::server;

/// Shared daemon state protected by a mutex.
pub struct DaemonState {
    pub poller: PollerSourceState,
    pub gateway: Gateway,
    pub daemon: DaemonProjection,
    pub generation_tracker: PaneGenerationTracker,
    pub gateway_cursor: Option<String>,
    /// Latest tmux pane list (for unmanaged pane display).
    pub last_panes: Vec<TmuxPaneInfo>,
}

impl DaemonState {
    pub fn new() -> Self {
        Self {
            poller: PollerSourceState::new(),
            gateway: Gateway::with_sources(&[SourceKind::Poller], Utc::now()),
            daemon: DaemonProjection::new(),
            generation_tracker: PaneGenerationTracker::new(),
            gateway_cursor: None,
            last_panes: Vec::new(),
        }
    }
}

/// Run the daemon: starts poll loop and UDS server, waits for shutdown signal.
pub async fn run_daemon(opts: DaemonOpts, socket_path: &str) -> anyhow::Result<()> {
    let executor = Arc::new(build_executor(&opts));
    let state = Arc::new(Mutex::new(DaemonState::new()));

    // Start UDS server
    let server_state = Arc::clone(&state);
    let server_socket = socket_path.to_string();
    let server_handle = tokio::spawn(async move {
        if let Err(e) = server::run_server(&server_socket, server_state).await {
            tracing::error!("UDS server error: {e}");
        }
    });

    // Start poll loop
    let poll_state = Arc::clone(&state);
    let poll_executor = Arc::clone(&executor);
    let poll_ms = opts.poll_interval_ms;
    let poll_handle = tokio::spawn(async move {
        run_poll_loop(poll_executor, poll_state, poll_ms).await;
    });

    // Wait for shutdown signal
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("received ctrl-c, shutting down");
        }
        _ = poll_handle => {
            tracing::warn!("poll loop exited unexpectedly");
        }
        _ = server_handle => {
            tracing::warn!("server exited unexpectedly");
        }
    }

    // Cleanup socket
    let _ = std::fs::remove_file(socket_path);
    tracing::info!("daemon stopped");
    Ok(())
}

fn build_executor(opts: &DaemonOpts) -> TmuxExecutor {
    let mut executor = TmuxExecutor::default();

    // Socket targeting: --tmux-socket > AGTMUX_TMUX_SOCKET_PATH > AGTMUX_TMUX_SOCKET_NAME
    if let Some(ref socket) = opts.tmux_socket {
        executor = executor.with_socket_path(socket.clone());
    } else if let Ok(path) = std::env::var("AGTMUX_TMUX_SOCKET_PATH") {
        executor = executor.with_socket_path(path);
    } else if let Ok(name) = std::env::var("AGTMUX_TMUX_SOCKET_NAME") {
        executor = executor.with_socket_name(name);
    }

    executor
}

async fn run_poll_loop<R: TmuxCommandRunner + 'static>(
    executor: Arc<R>,
    state: Arc<Mutex<DaemonState>>,
    poll_ms: u64,
) {
    let mut ticker = interval(Duration::from_millis(poll_ms));

    loop {
        ticker.tick().await;

        if let Err(e) = poll_tick(&executor, &state).await {
            tracing::warn!("poll tick failed: {e}");
        }
    }
}

async fn poll_tick<R: TmuxCommandRunner + 'static>(
    executor: &Arc<R>,
    state: &Arc<Mutex<DaemonState>>,
) -> anyhow::Result<()> {
    let now = Utc::now();

    // 1. List panes (blocking subprocess)
    let exec = Arc::clone(executor);
    let panes: Vec<TmuxPaneInfo> =
        tokio::task::spawn_blocking(move || list_panes(&*exec)).await??;

    tracing::debug!("listed {} panes", panes.len());

    // 2. Update generation tracker
    {
        let mut st = state.lock().await;
        let pane_ids: Vec<&str> = panes.iter().map(|p| p.pane_id.as_str()).collect();
        st.generation_tracker.update(&pane_ids, now);
        st.last_panes = panes.clone();
    }

    // 3. Capture each pane and build snapshots
    let mut snapshots = Vec::with_capacity(panes.len());

    for pane in &panes {
        let exec = Arc::clone(executor);
        let pane_id = pane.pane_id.clone();

        let capture_lines =
            match tokio::task::spawn_blocking(move || capture_pane(&*exec, &pane_id, 50)).await {
                Ok(Ok(lines)) => lines,
                Ok(Err(e)) => {
                    tracing::debug!("capture failed for {}: {e}", pane.pane_id);
                    Vec::new()
                }
                Err(e) => {
                    tracing::debug!("capture task failed for {}: {e}", pane.pane_id);
                    Vec::new()
                }
            };

        let st = state.lock().await;
        let snapshot = to_pane_snapshot(pane, capture_lines, &st.generation_tracker, now);
        drop(st);
        snapshots.push(snapshot);
    }

    // 4. Process through pipeline
    let mut st = state.lock().await;

    // 5. Poll batch for agent detection
    st.poller.poll_batch(&snapshots);

    // 6. Identify agent vs unmanaged panes (for logging)
    let agent_pane_ids: HashSet<String> = snapshots
        .iter()
        .filter(|s| poll_pane(s).is_some())
        .map(|s| s.pane_id.clone())
        .collect();

    let unmanaged_count = snapshots.len() - agent_pane_ids.len();
    if !agent_pane_ids.is_empty() || unmanaged_count > 0 {
        tracing::debug!(
            "agents: {}, unmanaged: {}",
            agent_pane_ids.len(),
            unmanaged_count
        );
    }

    // 7. Pull events from poller
    let poller_cursor = st
        .gateway
        .source_cursor(SourceKind::Poller)
        .map(String::from);
    let pull_request = PullEventsRequest {
        cursor: poller_cursor,
        limit: 500,
    };
    let poller_response = st.poller.pull_events(&pull_request, now);

    // 8. Ingest into gateway
    st.gateway
        .ingest_source_response(SourceKind::Poller, poller_response);

    // 9. Pull from gateway
    let gw_request = GatewayPullRequest {
        cursor: st.gateway_cursor.clone(),
        limit: 500,
    };
    let gw_response = st.gateway.pull_events(&gw_request);

    // Update gateway cursor for next tick
    st.gateway_cursor.clone_from(&gw_response.next_cursor);

    // 10. Apply to daemon
    if !gw_response.events.is_empty() {
        tracing::debug!("applying {} events to daemon", gw_response.events.len());
        st.daemon.apply_events(gw_response.events, now);
    }

    // 11. Compact — in MVP single-process, buffer growth is bounded by poll rate.
    // Full compaction (poller buffer trim, gateway truncation) is Post-MVP.

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agtmux_tmux_v5::error::TmuxError;
    use std::collections::HashMap;

    /// Fake tmux backend for integration testing.
    /// Configurable to return canned list-panes and capture-pane data.
    struct FakeTmuxBackend {
        /// Raw list-panes output string.
        list_panes_output: String,
        /// Per-pane capture data: pane_id -> capture lines.
        captures: HashMap<String, String>,
        /// If set, list-panes will fail with this error.
        list_panes_error: Option<String>,
        /// Set of pane_ids whose capture should fail.
        capture_errors: HashSet<String>,
    }

    impl FakeTmuxBackend {
        fn new() -> Self {
            Self {
                list_panes_output: String::new(),
                captures: HashMap::new(),
                list_panes_error: None,
                capture_errors: HashSet::new(),
            }
        }

        fn with_pane(mut self, pane_id: &str, session: &str, cmd: &str, capture: &str) -> Self {
            // Append a list-panes line in tab-delimited format
            let line =
                format!("$0\t{session}\t@0\tdev\t{pane_id}\t{cmd}\t/home\t{cmd}\t200\t50\t1\t1");
            if !self.list_panes_output.is_empty() {
                self.list_panes_output.push('\n');
            }
            self.list_panes_output.push_str(&line);
            self.captures
                .insert(pane_id.to_string(), capture.to_string());
            self
        }

        fn with_list_panes_error(mut self, err: &str) -> Self {
            self.list_panes_error = Some(err.to_string());
            self
        }

        fn with_capture_error(mut self, pane_id: &str) -> Self {
            self.capture_errors.insert(pane_id.to_string());
            self
        }
    }

    impl TmuxCommandRunner for FakeTmuxBackend {
        fn run(&self, args: &[&str]) -> Result<String, TmuxError> {
            if args.first() == Some(&"list-panes") {
                if let Some(ref err) = self.list_panes_error {
                    return Err(TmuxError::CommandFailed(err.clone()));
                }
                return Ok(self.list_panes_output.clone());
            }
            if args.first() == Some(&"capture-pane") {
                // Extract pane_id from -t flag
                let pane_id = args
                    .iter()
                    .zip(args.iter().skip(1))
                    .find(|(a, _)| **a == "-t")
                    .map(|(_, b)| *b)
                    .unwrap_or("");

                if self.capture_errors.contains(pane_id) {
                    return Err(TmuxError::CommandFailed(format!(
                        "capture failed for {pane_id}"
                    )));
                }

                return Ok(self.captures.get(pane_id).cloned().unwrap_or_default());
            }
            Err(TmuxError::CommandFailed(format!(
                "unexpected command: {args:?}"
            )))
        }
    }

    fn new_state() -> Arc<Mutex<DaemonState>> {
        Arc::new(Mutex::new(DaemonState::new()))
    }

    // --- Integration tests ---

    #[tokio::test]
    async fn poll_tick_detects_claude_agent() {
        let backend = Arc::new(FakeTmuxBackend::new().with_pane(
            "%0",
            "main",
            "claude",
            "╭ Claude Code\n│ Working...",
        ));
        let state = new_state();

        poll_tick(&backend, &state)
            .await
            .expect("tick should succeed");

        let st = state.lock().await;
        let managed = st.daemon.list_panes();
        assert_eq!(managed.len(), 1, "claude pane should be managed");
        assert_eq!(managed[0].pane_instance_id.pane_id, "%0");
    }

    #[tokio::test]
    async fn poll_tick_detects_codex_agent() {
        let backend = Arc::new(FakeTmuxBackend::new().with_pane(
            "%0",
            "work",
            "codex --model o3",
            "Codex is thinking...",
        ));
        let state = new_state();

        poll_tick(&backend, &state)
            .await
            .expect("tick should succeed");

        let st = state.lock().await;
        let managed = st.daemon.list_panes();
        assert_eq!(managed.len(), 1, "codex pane should be managed");
    }

    #[tokio::test]
    async fn poll_tick_unmanaged_pane_tracked() {
        let backend =
            Arc::new(FakeTmuxBackend::new().with_pane("%0", "main", "zsh", "$ ls\nfile.txt"));
        let state = new_state();

        poll_tick(&backend, &state)
            .await
            .expect("tick should succeed");

        let st = state.lock().await;
        // zsh is not an agent — daemon should have no managed panes
        let managed = st.daemon.list_panes();
        assert!(managed.is_empty(), "zsh should not be managed");
        // But last_panes should track it
        assert_eq!(st.last_panes.len(), 1);
        assert_eq!(st.last_panes[0].pane_id, "%0");
    }

    #[tokio::test]
    async fn poll_tick_mixed_agents_and_unmanaged() {
        let backend = Arc::new(
            FakeTmuxBackend::new()
                .with_pane("%0", "main", "claude", "╭ Claude Code")
                .with_pane("%1", "main", "zsh", "$ whoami")
                .with_pane("%2", "work", "codex --model o3", "Codex output")
                .with_pane("%3", "work", "vim", "-- INSERT --"),
        );
        let state = new_state();

        poll_tick(&backend, &state)
            .await
            .expect("tick should succeed");

        let st = state.lock().await;
        let managed = st.daemon.list_panes();
        assert_eq!(managed.len(), 2, "claude + codex should be managed");
        assert_eq!(st.last_panes.len(), 4, "all 4 panes in last_panes");
    }

    #[tokio::test]
    async fn poll_tick_empty_tmux() {
        let backend = Arc::new(FakeTmuxBackend::new());
        let state = new_state();

        poll_tick(&backend, &state)
            .await
            .expect("tick should succeed");

        let st = state.lock().await;
        assert!(st.daemon.list_panes().is_empty());
        assert!(st.last_panes.is_empty());
    }

    #[tokio::test]
    async fn poll_tick_list_panes_failure() {
        let backend = Arc::new(FakeTmuxBackend::new().with_list_panes_error("server not found"));
        let state = new_state();

        let result = poll_tick(&backend, &state).await;
        assert!(result.is_err(), "should propagate list-panes failure");
    }

    #[tokio::test]
    async fn poll_tick_capture_failure_continues() {
        let backend = Arc::new(
            FakeTmuxBackend::new()
                .with_pane("%0", "main", "claude", "")
                .with_capture_error("%0"),
        );
        let state = new_state();

        // Even if capture fails, poll_tick should succeed (skip pane capture)
        poll_tick(&backend, &state)
            .await
            .expect("tick should succeed");

        let st = state.lock().await;
        // Pane is still tracked (list_panes succeeded)
        assert_eq!(st.last_panes.len(), 1);
    }

    #[tokio::test]
    async fn poll_tick_gateway_cursor_set_after_events() {
        let backend =
            Arc::new(FakeTmuxBackend::new().with_pane("%0", "main", "claude", "╭ Claude Code"));
        let state = new_state();

        // Before any tick, cursor is None
        {
            let st = state.lock().await;
            assert!(st.gateway_cursor.is_none(), "initial cursor should be None");
        }

        poll_tick(&backend, &state).await.expect("tick 1");

        let cursor_after_1 = {
            let st = state.lock().await;
            st.gateway_cursor.clone()
        };
        assert!(
            cursor_after_1.is_some(),
            "gateway cursor should be set after first tick with events"
        );

        // Second tick (no new events from poller — same pane, same capture).
        // Cursor should remain stable (no re-delivery).
        poll_tick(&backend, &state).await.expect("tick 2");

        let cursor_after_2 = {
            let st = state.lock().await;
            st.gateway_cursor.clone()
        };
        assert!(cursor_after_2.is_some(), "cursor still set");
    }

    #[tokio::test]
    async fn poll_tick_no_redelivery_on_second_tick() {
        let backend =
            Arc::new(FakeTmuxBackend::new().with_pane("%0", "main", "claude", "╭ Claude Code"));
        let state = new_state();

        poll_tick(&backend, &state).await.expect("tick 1");
        let managed_after_1 = {
            let st = state.lock().await;
            st.daemon.list_panes().len()
        };

        poll_tick(&backend, &state).await.expect("tick 2");
        let managed_after_2 = {
            let st = state.lock().await;
            st.daemon.list_panes().len()
        };

        // Should still have 1 managed pane (not duplicated)
        assert_eq!(managed_after_1, 1);
        assert_eq!(managed_after_2, 1);
    }

    #[tokio::test]
    async fn poll_tick_generation_tracker_updates() {
        let backend = Arc::new(
            FakeTmuxBackend::new()
                .with_pane("%0", "main", "zsh", "$ ls")
                .with_pane("%1", "main", "claude", "╭ Claude Code"),
        );
        let state = new_state();

        poll_tick(&backend, &state).await.expect("tick");

        let st = state.lock().await;
        assert!(
            st.generation_tracker.get("%0").is_some(),
            "%0 should be tracked"
        );
        assert!(
            st.generation_tracker.get("%1").is_some(),
            "%1 should be tracked"
        );
        let (gen0, _) = st.generation_tracker.get("%0").expect("tracked");
        assert_eq!(gen0, 0, "first-seen pane should have generation 0");
    }

    #[tokio::test]
    async fn poll_tick_large_batch() {
        let mut backend = FakeTmuxBackend::new();
        for i in 0..20 {
            let pane_id = format!("%{i}");
            let cmd = if i % 3 == 0 { "claude" } else { "zsh" };
            backend = backend.with_pane(&pane_id, "main", cmd, "output");
        }
        let backend = Arc::new(backend);
        let state = new_state();

        poll_tick(&backend, &state).await.expect("tick");

        let st = state.lock().await;
        assert_eq!(st.last_panes.len(), 20, "all 20 panes tracked");
        let agent_count = st.daemon.list_panes().len();
        // Panes 0, 3, 6, 9, 12, 15, 18 → 7 claude panes
        assert_eq!(agent_count, 7, "7 claude panes should be managed");
    }

    #[tokio::test]
    async fn poll_tick_multiple_sessions() {
        let backend = Arc::new(
            FakeTmuxBackend::new()
                .with_pane("%0", "project-a", "claude", "╭ Claude Code")
                .with_pane("%1", "project-b", "codex --model o3", "Codex output"),
        );
        let state = new_state();

        poll_tick(&backend, &state).await.expect("tick");

        let st = state.lock().await;
        let managed = st.daemon.list_panes();
        assert_eq!(managed.len(), 2, "agents from both sessions managed");
    }
}

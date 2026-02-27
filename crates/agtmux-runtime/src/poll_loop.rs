//! Poll loop: wires tmux → poller → gateway → daemon pipeline.
//! Runs as a tokio task, polling tmux at configurable intervals.

use std::collections::HashSet;
use std::sync::Arc;

use chrono::Utc;
use tokio::sync::Mutex;
use tokio::time::{Duration, interval};

use agtmux_core_v5::types::{GatewayPullRequest, Provider, PullEventsRequest, SourceKind};
use agtmux_daemon_v5::projection::DaemonProjection;
use agtmux_gateway::cursor_hardening::{
    CursorRecoveryAction, CursorWatermarks, InvalidCursorTracker,
};
use agtmux_gateway::gateway::Gateway;
use agtmux_gateway::latency_window::{LatencyEvaluation, LatencyWindow};
use agtmux_gateway::source_registry::SourceRegistry;
use agtmux_gateway::trust_guard::TrustGuard;
use agtmux_source_claude_hooks::source::SourceState as ClaudeSourceState;
use agtmux_source_claude_jsonl::source::ClaudeJsonlSourceState;
use agtmux_source_claude_jsonl::watcher::SessionFileWatcher;
use agtmux_source_codex_appserver::source::SourceState as CodexSourceState;
use agtmux_source_poller::source::{PollerSourceState, poll_pane};
use agtmux_tmux_v5::{
    PaneGenerationTracker, TmuxCommandRunner, TmuxExecutor, TmuxPaneInfo, capture_pane, list_panes,
    to_pane_snapshot,
};

use crate::cli::DaemonOpts;
use crate::codex_poller::{
    CodexAppServerClient, CodexCaptureTracker, PaneCwdInfo, parse_codex_capture_events,
};
use crate::server;

/// Shared daemon state protected by a mutex.
pub struct DaemonState {
    pub poller: PollerSourceState,
    pub codex_source: CodexSourceState,
    pub claude_source: ClaudeSourceState,
    pub claude_jsonl_source: ClaudeJsonlSourceState,
    pub claude_jsonl_watchers: std::collections::HashMap<String, SessionFileWatcher>,
    pub gateway: Gateway,
    pub daemon: DaemonProjection,
    pub generation_tracker: PaneGenerationTracker,
    pub gateway_cursor: Option<String>,
    /// Latest tmux pane list (for unmanaged pane display).
    pub last_panes: Vec<TmuxPaneInfo>,
    /// UDS trust admission guard (peer UID, source registry, nonce).
    pub trust_guard: TrustGuard,
    /// Source connection registry (hello/heartbeat/staleness lifecycle).
    pub source_registry: SourceRegistry,
    /// Two-watermark cursor tracking (fetched vs committed) for gateway cursor.
    pub cursor_watermarks: CursorWatermarks,
    /// Invalid cursor streak tracker — triggers recovery after consecutive failures.
    pub invalid_cursor_tracker: InvalidCursorTracker,
    /// Rolling p95 latency window (SLO: 3000ms = freshness boundary).
    pub latency_window: LatencyWindow,
    /// Cached latency evaluation from the last poll_tick (for read-only API access).
    pub last_latency_eval: Option<LatencyEvaluation>,
    /// Tracks Codex JSON events already ingested from tmux capture (dedup).
    pub codex_capture_tracker: CodexCaptureTracker,
    /// Codex App Server client (JSON-RPC over stdio). `None` if not available.
    pub codex_appserver_client: Option<CodexAppServerClient>,
    /// True if App Server was ever connected (triggers reconnection on death).
    pub codex_appserver_had_connection: bool,
    /// Consecutive failed reconnection attempts (for exponential backoff).
    pub codex_reconnect_failures: u32,
}

impl DaemonState {
    pub fn new() -> Self {
        // Generate a runtime nonce: PID + monotonic nanoseconds
        let nonce = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );

        // Get current process UID for trust guard (UDS peer credential check)
        #[cfg(unix)]
        let uid = {
            // SAFETY: getuid() has no arguments, no side effects, and cannot fail.
            unsafe extern "C" {
                safe fn getuid() -> u32;
            }
            getuid()
        };
        #[cfg(not(unix))]
        let uid = 0u32;

        let mut trust_guard = TrustGuard::new(uid, nonce);
        trust_guard.register_source("poller");
        trust_guard.register_source("codex_appserver");
        trust_guard.register_source("claude_hooks");
        trust_guard.register_source("claude_jsonl");

        Self {
            poller: PollerSourceState::new(),
            codex_source: CodexSourceState::new(),
            claude_source: ClaudeSourceState::new(),
            claude_jsonl_source: ClaudeJsonlSourceState::new(),
            claude_jsonl_watchers: std::collections::HashMap::new(),
            gateway: Gateway::with_sources(
                &[
                    SourceKind::Poller,
                    SourceKind::CodexAppserver,
                    SourceKind::ClaudeHooks,
                    SourceKind::ClaudeJsonl,
                ],
                Utc::now(),
            ),
            daemon: DaemonProjection::new(),
            generation_tracker: PaneGenerationTracker::new(),
            gateway_cursor: None,
            last_panes: Vec::new(),
            trust_guard,
            source_registry: SourceRegistry::new(),
            cursor_watermarks: CursorWatermarks::new(),
            invalid_cursor_tracker: InvalidCursorTracker::new(),
            latency_window: LatencyWindow::new(3000),
            last_latency_eval: None,
            codex_capture_tracker: CodexCaptureTracker::new(),
            codex_appserver_client: None, // Spawned asynchronously in run_daemon
            codex_appserver_had_connection: false,
            codex_reconnect_failures: 0,
        }
    }
}

/// Run the daemon: starts poll loop and UDS server, waits for shutdown signal.
pub async fn run_daemon(opts: DaemonOpts, socket_path: &str) -> anyhow::Result<()> {
    let executor = Arc::new(build_executor(&opts));
    let state = Arc::new(Mutex::new(DaemonState::new()));

    // Attempt initial Codex App Server connection.
    // If codex binary is not found or handshake fails, this is None — fallback path is used.
    // If connected, set had_connection so poll_tick will reconnect on death.
    {
        let client = CodexAppServerClient::spawn().await;
        let mut st = state.lock().await;
        if client.is_some() {
            st.codex_appserver_had_connection = true;
        }
        st.codex_appserver_client = client;
    }

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

    // Wait for shutdown signal (ctrl-c or SIGTERM)
    let shutdown = async {
        let ctrl_c = tokio::signal::ctrl_c();

        #[cfg(unix)]
        {
            let mut sigterm =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("failed to register SIGTERM handler");
            tokio::select! {
                _ = ctrl_c => tracing::info!("received ctrl-c, shutting down"),
                _ = sigterm.recv() => tracing::info!("received SIGTERM, shutting down"),
            }
        }

        #[cfg(not(unix))]
        {
            ctrl_c.await.ok();
            tracing::info!("received ctrl-c, shutting down");
        }
    };

    tokio::select! {
        () = shutdown => {}
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

/// Parse a gateway cursor string `"gw:{position}"` into a numeric position.
fn parse_gw_cursor(cursor: &str) -> Option<u64> {
    cursor
        .strip_prefix("gw:")
        .and_then(|s| s.parse::<u64>().ok())
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
    let tick_start = std::time::Instant::now();
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

    // 6a. Codex deterministic evidence: App Server (primary) or capture extraction (fallback).
    //
    // B5 fix: poll_threads() is called OUTSIDE the mutex to avoid blocking
    // all DaemonState access during the 5s App Server timeout.
    let appserver_poll_result = {
        let mut client_taken = st.codex_appserver_client.take();
        let alive = client_taken.as_mut().is_some_and(|c| c.is_alive());
        if alive {
            // T-119: Build pane cwd info for thread ↔ pane correlation
            let pane_cwds: Vec<PaneCwdInfo> = st
                .last_panes
                .iter()
                .map(|pane| {
                    let gen_info = st.generation_tracker.get(&pane.pane_id);
                    let has_codex_hint = snapshots.iter().any(|s| {
                        s.pane_id == pane.pane_id && s.process_hint.as_deref() == Some("codex")
                    });
                    PaneCwdInfo {
                        pane_id: pane.pane_id.clone(),
                        cwd: pane.current_path.clone(),
                        generation: gen_info.map(|(g, _)| g),
                        birth_ts: gen_info.map(|(_, ts)| ts),
                        has_codex_hint,
                    }
                })
                .collect();

            // Release mutex before async I/O
            drop(st);
            let events = if let Some(ref mut client) = client_taken {
                client.poll_threads(&pane_cwds).await
            } else {
                Vec::new()
            };
            st = state.lock().await;
            st.codex_appserver_client = client_taken;
            st.codex_reconnect_failures = 0; // connection healthy
            Some(events)
        } else if client_taken.is_some() || st.codex_appserver_had_connection {
            // Process exited — attempt reconnection with exponential backoff (B4)
            tracing::info!("codex app-server process exited, attempting reconnect");
            drop(client_taken); // drop dead process
            let backoff_ticks = 2u32.saturating_pow(st.codex_reconnect_failures.min(6)); // max ~64 ticks
            if st.codex_reconnect_failures == 0 || st.codex_reconnect_failures % backoff_ticks == 0
            {
                drop(st);
                let new_client = CodexAppServerClient::spawn().await;
                st = state.lock().await;
                if new_client.is_some() {
                    tracing::info!("codex app-server reconnected");
                    st.codex_reconnect_failures = 0;
                    st.codex_appserver_had_connection = true;
                } else {
                    st.codex_reconnect_failures = st.codex_reconnect_failures.saturating_add(1);
                }
                st.codex_appserver_client = new_client;
            } else {
                st.codex_reconnect_failures = st.codex_reconnect_failures.saturating_add(1);
                st.codex_appserver_client = None;
            }
            None // no events this tick
        } else {
            // No client ever connected and none was established by run_daemon.
            // poll_tick does NOT attempt initial spawn — that's run_daemon's job.
            // This avoids spawning codex in tests or when the binary is unavailable.
            None
        }
    };

    // B3 fix: used_appserver is true when client is alive (regardless of event count)
    let used_appserver = appserver_poll_result.is_some();
    if let Some(events) = appserver_poll_result
        && !events.is_empty()
    {
        tracing::debug!("codex app-server: {} events from thread/list", events.len());
        for event in events {
            st.codex_source.ingest(event);
        }
    }

    // Fallback: parse Codex NDJSON from tmux capture text (only when App Server unavailable)
    if !used_appserver {
        let active_pane_ids: Vec<&str> = panes.iter().map(|p| p.pane_id.as_str()).collect();
        st.codex_capture_tracker.retain_panes(&active_pane_ids);

        for snapshot in &snapshots {
            if let Some(result) = poll_pane(snapshot)
                && result.provider == Provider::Codex
            {
                let new_events = parse_codex_capture_events(
                    &snapshot.capture_lines,
                    &snapshot.pane_id,
                    &mut st.codex_capture_tracker,
                );
                for event in new_events {
                    tracing::debug!(
                        "codex capture event: {} (pane={})",
                        event.event_type,
                        snapshot.pane_id
                    );
                    st.codex_source.ingest(event);
                }
            }
        }
    }

    // B6: propagate App Server connectivity to codex source health
    st.codex_source.set_appserver_connected(used_appserver);

    // 6b. Claude JSONL discovery + poll
    // For panes known to be Claude (from poller OR projection/hooks), look up JSONL transcripts.
    // CWD comes from TmuxPaneInfo (panes), not PaneSnapshot.
    {
        // Collect pane_ids that poller detected as Claude
        let mut claude_pane_ids: HashSet<&str> = snapshots
            .iter()
            .filter(|s| {
                poll_pane(s)
                    .map(|r| r.provider == Provider::Claude)
                    .unwrap_or(false)
            })
            .map(|s| s.pane_id.as_str())
            .collect();

        // Also include panes that projection already knows are Claude
        // (e.g. detected via hooks, not just poller)
        for pane_state in st.daemon.list_panes() {
            if pane_state.provider == Some(Provider::Claude) {
                let pid = &pane_state.pane_instance_id.pane_id;
                if let Some(tmux_pane) = panes.iter().find(|p| p.pane_id == *pid) {
                    claude_pane_ids.insert(tmux_pane.pane_id.as_str());
                }
            }
        }

        let claude_pane_cwds: Vec<(
            String,
            String,
            Option<u64>,
            Option<chrono::DateTime<chrono::Utc>>,
        )> = panes
            .iter()
            .filter(|p| claude_pane_ids.contains(p.pane_id.as_str()))
            .map(|p| {
                let (pane_gen, pane_birth) = st
                    .generation_tracker
                    .get(&p.pane_id)
                    .map(|(g, b)| (Some(g), Some(b)))
                    .unwrap_or((None, None));
                (
                    p.pane_id.clone(),
                    p.current_path.clone(),
                    pane_gen,
                    pane_birth,
                )
            })
            .collect();

        if !claude_pane_cwds.is_empty() {
            let discoveries = ClaudeJsonlSourceState::discover_sessions(&claude_pane_cwds);
            let jsonl_events =
                ClaudeJsonlSourceState::poll_files(&mut st.claude_jsonl_watchers, &discoveries);
            for event in jsonl_events {
                st.claude_jsonl_source.ingest(event);
            }
        }
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

    // 8a. Pull events from codex source (populated via source.ingest UDS)
    let codex_cursor = st
        .gateway
        .source_cursor(SourceKind::CodexAppserver)
        .map(String::from);
    let codex_response = st.codex_source.pull_events(
        &PullEventsRequest {
            cursor: codex_cursor,
            limit: 500,
        },
        now,
    );
    st.gateway
        .ingest_source_response(SourceKind::CodexAppserver, codex_response);

    // 8b. Pull events from claude source (populated via source.ingest UDS)
    let claude_cursor = st
        .gateway
        .source_cursor(SourceKind::ClaudeHooks)
        .map(String::from);
    let claude_response = st.claude_source.pull_events(
        &PullEventsRequest {
            cursor: claude_cursor,
            limit: 500,
        },
        now,
    );
    st.gateway
        .ingest_source_response(SourceKind::ClaudeHooks, claude_response);

    // 8c. Pull events from claude JSONL source
    let jsonl_cursor = st
        .gateway
        .source_cursor(SourceKind::ClaudeJsonl)
        .map(String::from);
    let jsonl_response = st.claude_jsonl_source.pull_events(
        &PullEventsRequest {
            cursor: jsonl_cursor,
            limit: 500,
        },
        now,
    );
    st.gateway
        .ingest_source_response(SourceKind::ClaudeJsonl, jsonl_response);

    // 9. Pull from gateway
    let gw_request = GatewayPullRequest {
        cursor: st.gateway_cursor.clone(),
        limit: 500,
    };
    let gw_response = st.gateway.pull_events(&gw_request);

    // 9a. Track fetched position via watermarks
    if let Some(ref next_cursor) = gw_response.next_cursor
        && let Some(pos) = parse_gw_cursor(next_cursor)
    {
        match st.cursor_watermarks.advance_fetched(pos) {
            Ok(()) => {
                st.invalid_cursor_tracker.record_valid();
            }
            Err(e) => {
                tracing::warn!("cursor watermark advance_fetched error: {e}");
                match st.invalid_cursor_tracker.record_invalid() {
                    CursorRecoveryAction::RetryFromCommitted => {
                        let committed = st.cursor_watermarks.committed;
                        tracing::info!("cursor recovery: retry from committed={committed}");
                        st.gateway_cursor = if committed > 0 {
                            Some(format!("gw:{committed}"))
                        } else {
                            None
                        };
                    }
                    CursorRecoveryAction::FullResync => {
                        tracing::error!("cursor recovery: full resync (streak exceeded)");
                        st.gateway_cursor = None;
                        st.cursor_watermarks = CursorWatermarks::new();
                    }
                }
                // Skip normal cursor update on error — recovery cursor is already set
                // Continue to apply any events already pulled
            }
        }
    }

    // Update gateway cursor for next tick (normal path)
    if st.invalid_cursor_tracker.streak() == 0 {
        st.gateway_cursor.clone_from(&gw_response.next_cursor);
    }

    // 10. Apply to daemon
    if !gw_response.events.is_empty() {
        tracing::debug!("applying {} events to daemon", gw_response.events.len());
        st.daemon.apply_events(gw_response.events, now);
    }

    // 11. Compact consumed events to prevent unbounded memory growth.
    // Poller: trim events up to the gateway's source cursor.
    if let Some(poller_cursor) = st.gateway.source_cursor(SourceKind::Poller)
        && let Some(seq_str) = poller_cursor.strip_prefix("poller:")
        && let Ok(seq) = seq_str.parse::<u64>()
    {
        st.poller.compact(seq);
    }
    // Codex: trim events up to the gateway's source cursor.
    if let Some(codex_cursor) = st.gateway.source_cursor(SourceKind::CodexAppserver)
        && let Some(seq_str) = codex_cursor.strip_prefix("codex-app:")
        && let Ok(seq) = seq_str.parse::<u64>()
    {
        st.codex_source.compact(seq);
    }
    // Claude hooks: trim events up to the gateway's source cursor.
    if let Some(claude_cursor) = st.gateway.source_cursor(SourceKind::ClaudeHooks)
        && let Some(seq_str) = claude_cursor.strip_prefix("claude-hooks:")
        && let Ok(seq) = seq_str.parse::<u64>()
    {
        st.claude_source.compact(seq);
    }
    // Claude JSONL: trim events up to the gateway's source cursor.
    if let Some(jsonl_cursor) = st.gateway.source_cursor(SourceKind::ClaudeJsonl)
        && let Some(seq_str) = jsonl_cursor.strip_prefix("claude-jsonl:")
        && let Ok(seq) = seq_str.parse::<u64>()
    {
        st.claude_jsonl_source.compact(seq);
    }
    // Gateway: trim events up to the daemon's committed cursor.
    if let Some(gw_cursor) = st.gateway_cursor.clone() {
        // 11a. Track committed position via watermarks
        if let Some(pos) = parse_gw_cursor(&gw_cursor)
            && let Err(e) = st.cursor_watermarks.commit(pos)
        {
            tracing::warn!("cursor watermark commit error: {e}");
        }
        st.gateway.commit_cursor(&gw_cursor);
    }

    // 11b. Check source staleness
    let now_ms_staleness = now.timestamp_millis() as u64;
    let stale_sources = st.source_registry.check_staleness(now_ms_staleness);
    for source_id in &stale_sources {
        tracing::warn!("source stale: {source_id}");
    }

    // 12. Record tick latency and evaluate SLO
    let tick_ms = tick_start.elapsed().as_millis() as u64;
    let now_ms = now.timestamp_millis() as u64;
    st.latency_window.record(tick_ms, now_ms);
    let eval = st.latency_window.evaluate(now_ms);
    match &eval {
        LatencyEvaluation::Breached {
            p95_ms,
            consecutive,
            ..
        } => {
            tracing::warn!("SLO breach: p95={p95_ms}ms, consecutive={consecutive}");
        }
        LatencyEvaluation::Degraded {
            p95_ms,
            consecutive,
        } => {
            tracing::error!("SLO DEGRADED: p95={p95_ms}ms, consecutive={consecutive}");
        }
        _ => {}
    }
    st.last_latency_eval = Some(eval);

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

        fn with_pane(self, pane_id: &str, session: &str, cmd: &str, capture: &str) -> Self {
            self.with_pane_cwd(pane_id, session, cmd, capture, "/home")
        }

        fn with_pane_cwd(
            mut self,
            pane_id: &str,
            session: &str,
            cmd: &str,
            capture: &str,
            cwd: &str,
        ) -> Self {
            // Append a list-panes line in tab-delimited format
            let line =
                format!("$0\t{session}\t@0\tdev\t{pane_id}\t{cmd}\t{cwd}\t{cmd}\t200\t50\t1\t1");
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

    // ── Deterministic source integration tests ──────────────────────

    #[tokio::test]
    async fn poll_tick_pulls_from_claude_source() {
        use agtmux_source_claude_hooks::translate::ClaudeHookEvent;

        let backend = Arc::new(FakeTmuxBackend::new().with_pane("%0", "main", "zsh", "$ ls"));
        let state = new_state();

        // Pre-ingest a Claude hook event (use Utc::now() so resolver sees it as fresh)
        {
            let mut st = state.lock().await;
            st.claude_source.ingest(ClaudeHookEvent {
                hook_id: "h-001".to_string(),
                hook_type: "tool_start".to_string(),
                session_id: "claude-sess-1".to_string(),
                timestamp: Utc::now(),
                pane_id: Some("%0".to_string()),
                data: serde_json::json!({}),
            });
        }

        poll_tick(&backend, &state).await.expect("tick");

        let st = state.lock().await;
        // The claude hook event should have flowed through gateway to daemon
        let managed = st.daemon.list_panes();
        assert!(
            !managed.is_empty(),
            "claude hook event should create managed pane"
        );
    }

    #[tokio::test]
    async fn poll_tick_pulls_from_codex_source() {
        use agtmux_source_codex_appserver::translate::CodexRawEvent;

        let backend = Arc::new(FakeTmuxBackend::new().with_pane("%0", "main", "zsh", "$ ls"));
        let state = new_state();

        // Pre-ingest a Codex appserver event
        {
            let mut st = state.lock().await;
            st.codex_source.ingest(CodexRawEvent {
                id: "cx-001".to_string(),
                event_type: "task.running".to_string(),
                session_id: "codex-sess-1".to_string(),
                timestamp: Utc::now(),
                pane_id: Some("%0".to_string()),
                pane_generation: None,
                pane_birth_ts: None,
                payload: serde_json::json!({}),
            });
        }

        poll_tick(&backend, &state).await.expect("tick");

        let st = state.lock().await;
        let managed = st.daemon.list_panes();
        assert!(
            !managed.is_empty(),
            "codex appserver event should create managed pane"
        );
    }

    #[tokio::test]
    async fn poll_tick_mixed_poller_and_deterministic() {
        use agtmux_source_claude_hooks::translate::ClaudeHookEvent;

        let backend = Arc::new(
            FakeTmuxBackend::new()
                .with_pane("%0", "main", "claude", "╭ Claude Code") // detected by poller
                .with_pane("%1", "main", "zsh", "$ ls"), // only via hooks
        );
        let state = new_state();

        // Pre-ingest a Claude hook event for pane %1 (use Utc::now() for freshness)
        {
            let mut st = state.lock().await;
            st.claude_source.ingest(ClaudeHookEvent {
                hook_id: "h-002".to_string(),
                hook_type: "session_start".to_string(),
                session_id: "claude-sess-2".to_string(),
                timestamp: Utc::now(),
                pane_id: Some("%1".to_string()),
                data: serde_json::json!({}),
            });
        }

        poll_tick(&backend, &state).await.expect("tick");

        let st = state.lock().await;
        let managed = st.daemon.list_panes();
        // %0 via poller + %1 via hooks = 2 managed panes
        assert_eq!(
            managed.len(),
            2,
            "both poller and deterministic events should create managed panes"
        );
    }

    #[tokio::test]
    async fn poll_tick_compacts_deterministic_sources() {
        use agtmux_source_claude_hooks::translate::ClaudeHookEvent;

        let backend = Arc::new(FakeTmuxBackend::new().with_pane("%0", "main", "zsh", "$ ls"));
        let state = new_state();

        // Pre-ingest events (use Utc::now() for freshness)
        {
            let mut st = state.lock().await;
            let now = Utc::now();
            for i in 0..3 {
                st.claude_source.ingest(ClaudeHookEvent {
                    hook_id: format!("h-{i}"),
                    hook_type: "tool_start".to_string(),
                    session_id: "claude-sess-1".to_string(),
                    timestamp: now,
                    pane_id: Some("%0".to_string()),
                    data: serde_json::json!({}),
                });
            }
            assert_eq!(st.claude_source.buffered_len(), 3);
        }

        // First tick: pulls events and compacts
        poll_tick(&backend, &state).await.expect("tick 1");

        {
            let st = state.lock().await;
            assert_eq!(
                st.claude_source.buffered_len(),
                0,
                "compaction should trim consumed events"
            );
        }

        // Second tick: no new events, should be clean
        poll_tick(&backend, &state).await.expect("tick 2");
    }

    #[tokio::test]
    async fn gateway_registers_all_four_sources() {
        let state = new_state();
        let st = state.lock().await;
        let health = st.gateway.list_source_health();
        assert_eq!(
            health.len(),
            4,
            "poller + codex + claude_hooks + claude_jsonl registered"
        );
    }

    // ── T-118: Latency window integration tests ──────────────────────

    #[tokio::test]
    async fn poll_tick_records_latency_sample() {
        let backend = Arc::new(FakeTmuxBackend::new().with_pane("%0", "main", "zsh", "$ ls"));
        let state = new_state();

        poll_tick(&backend, &state).await.expect("tick");

        let st = state.lock().await;
        assert!(
            st.latency_window.sample_count() >= 1,
            "tick should record at least 1 latency sample"
        );
        assert!(
            st.last_latency_eval.is_some(),
            "tick should cache latency evaluation"
        );
    }

    #[tokio::test]
    async fn poll_tick_latency_accumulates() {
        let backend = Arc::new(FakeTmuxBackend::new().with_pane("%0", "main", "zsh", "$ ls"));
        let state = new_state();

        for _ in 0..5 {
            poll_tick(&backend, &state).await.expect("tick");
        }

        let st = state.lock().await;
        assert!(
            st.latency_window.sample_count() >= 5,
            "5 ticks should record at least 5 latency samples, got {}",
            st.latency_window.sample_count()
        );
    }

    // ── T-116: Cursor watermarks integration tests ──────────────────

    #[tokio::test]
    async fn poll_tick_cursor_watermarks_advance_on_events() {
        let backend = Arc::new(FakeTmuxBackend::new().with_pane(
            "%0",
            "main",
            "claude",
            "╭ Claude Code\n│ Working...",
        ));
        let state = new_state();

        poll_tick(&backend, &state).await.expect("tick");

        let st = state.lock().await;
        assert!(
            st.cursor_watermarks.fetched > 0,
            "fetched watermark should advance after events, got {}",
            st.cursor_watermarks.fetched
        );
    }

    #[tokio::test]
    async fn poll_tick_cursor_watermarks_commit_after_apply() {
        let backend = Arc::new(FakeTmuxBackend::new().with_pane(
            "%0",
            "main",
            "claude",
            "╭ Claude Code\n│ Working...",
        ));
        let state = new_state();

        poll_tick(&backend, &state).await.expect("tick");

        let st = state.lock().await;
        assert_eq!(
            st.cursor_watermarks.committed, st.cursor_watermarks.fetched,
            "committed should equal fetched after single tick (all events applied)"
        );
    }

    #[tokio::test]
    async fn poll_tick_cursor_watermarks_monotonic_across_ticks() {
        let backend = Arc::new(FakeTmuxBackend::new().with_pane(
            "%0",
            "main",
            "claude",
            "╭ Claude Code\n│ Working...",
        ));
        let state = new_state();

        poll_tick(&backend, &state).await.expect("tick 1");
        let fetched_after_1 = {
            let st = state.lock().await;
            st.cursor_watermarks.fetched
        };

        poll_tick(&backend, &state).await.expect("tick 2");
        let fetched_after_2 = {
            let st = state.lock().await;
            st.cursor_watermarks.fetched
        };

        assert!(
            fetched_after_2 >= fetched_after_1,
            "fetched should be monotonically non-decreasing: {} -> {}",
            fetched_after_1,
            fetched_after_2
        );
    }

    #[tokio::test]
    async fn poll_tick_cursor_caught_up_steady_state() {
        let backend = Arc::new(FakeTmuxBackend::new().with_pane("%0", "main", "zsh", "$ ls"));
        let state = new_state();

        // Two ticks with no agent events — no gateway events generated
        poll_tick(&backend, &state).await.expect("tick 1");
        poll_tick(&backend, &state).await.expect("tick 2");

        let st = state.lock().await;
        assert!(
            st.cursor_watermarks.is_caught_up(),
            "cursor should be caught up in steady state (fetched={}, committed={})",
            st.cursor_watermarks.fetched,
            st.cursor_watermarks.committed
        );
    }

    // ── Codex capture JSON extraction integration tests ──────────────

    #[tokio::test]
    async fn poll_tick_codex_json_capture_ingested() {
        // Codex pane with --json output: NDJSON events visible in capture
        let backend = Arc::new(FakeTmuxBackend::new().with_pane(
            "%0",
            "main",
            "codex --model o3",
            "{\"type\":\"message.created\",\"id\":\"m1\"}\n{\"type\":\"turn.completed\",\"id\":\"t1\"}\nwait_result=idle",
        ));
        let state = new_state();

        poll_tick(&backend, &state).await.expect("tick");

        let st = state.lock().await;
        // Codex JSON events should have been parsed from capture and ingested.
        // Both heuristic (poller) and deterministic (codex_source) evidence
        // flow through the gateway to the daemon.
        let managed = st.daemon.list_panes();
        assert!(
            !managed.is_empty(),
            "codex pane with JSON events should be managed"
        );
    }

    #[tokio::test]
    async fn poll_tick_codex_json_dedup_across_ticks() {
        let backend = Arc::new(FakeTmuxBackend::new().with_pane(
            "%0",
            "main",
            "codex --model o3",
            "{\"type\":\"turn.completed\",\"id\":\"t1\"}",
        ));
        let state = new_state();

        poll_tick(&backend, &state).await.expect("tick 1");
        let codex_cursor_after_1 = {
            let st = state.lock().await;
            st.gateway
                .source_cursor(SourceKind::CodexAppserver)
                .map(String::from)
        };

        poll_tick(&backend, &state).await.expect("tick 2");
        let codex_cursor_after_2 = {
            let st = state.lock().await;
            st.gateway
                .source_cursor(SourceKind::CodexAppserver)
                .map(String::from)
        };

        // Cursor should not advance on the second tick because the same
        // JSON event was already ingested — dedup prevents re-ingestion.
        assert_eq!(
            codex_cursor_after_1, codex_cursor_after_2,
            "codex source cursor should not advance on duplicate capture"
        );
    }

    #[tokio::test]
    async fn poll_tick_codex_no_json_still_detected_by_poller() {
        // Codex pane without --json output (no NDJSON in capture)
        let backend = Arc::new(FakeTmuxBackend::new().with_pane(
            "%0",
            "main",
            "codex --model o3",
            "Codex is thinking...\nProcessing request...",
        ));
        let state = new_state();

        poll_tick(&backend, &state).await.expect("tick");

        let st = state.lock().await;
        // Poller heuristic should still detect this as a Codex agent pane
        let managed = st.daemon.list_panes();
        assert!(
            !managed.is_empty(),
            "codex pane without JSON should still be detected by poller"
        );
    }
}

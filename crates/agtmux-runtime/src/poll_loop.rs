//! Poll loop: wires tmux → poller → gateway → daemon pipeline.
//! Runs as a tokio task, polling tmux at configurable intervals.

use std::collections::HashSet;
use std::sync::Arc;

use chrono::{TimeDelta, Utc};
use tokio::sync::Mutex;
use tokio::time::{Duration, interval};

use agtmux_core_v5::types::{GatewayPullRequest, PullEventsRequest, SourceKind};
use agtmux_daemon_v5::projection::DaemonProjection;
use agtmux_gateway::cursor_hardening::{
    CursorRecoveryAction, CursorWatermarks, InvalidCursorTracker,
};
use agtmux_gateway::gateway::Gateway;
use agtmux_gateway::latency_window::{LatencyEvaluation, LatencyWindow};
use agtmux_gateway::source_registry::SourceRegistry;
use agtmux_gateway::trust_guard::TrustGuard;
use agtmux_source_claude_hooks::source::SourceState as ClaudeSourceState;
use agtmux_source_claude_jsonl::discovery::{PaneDiscoveryHint, discovery_from_transcript_path};
use agtmux_source_claude_jsonl::source::ClaudeJsonlSourceState;
use agtmux_source_claude_jsonl::watcher::SessionFileWatcher;
use agtmux_source_codex_jsonl::discovery::CodexPaneHint;
use agtmux_source_codex_jsonl::source::CodexJsonlSourceState;
use agtmux_source_codex_jsonl::watcher::CodexSessionFileWatcher;
use agtmux_source_poller::source::{PollerSourceState, poll_pane};
use agtmux_tmux_v5::{
    PaneGenerationTracker, TmuxCommandRunner, TmuxExecutor, TmuxPaneInfo, capture_pane, list_panes,
    scan_all_processes, to_pane_snapshot,
};

use crate::cli::DaemonOpts;
use crate::server;
use crate::sync_v3_runtime::SyncV3LiveState;

/// Shared daemon state protected by a mutex.
pub struct DaemonState {
    pub poller: PollerSourceState,
    pub claude_source: ClaudeSourceState,
    pub claude_jsonl_source: ClaudeJsonlSourceState,
    pub claude_jsonl_watchers: std::collections::HashMap<String, SessionFileWatcher>,
    pub codex_jsonl_source: CodexJsonlSourceState,
    pub codex_jsonl_watchers: std::collections::HashMap<String, CodexSessionFileWatcher>,
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
    /// Conversation titles keyed by session_key (T-135a/b).
    /// Claude: session_key → title from custom-title JSONL events (T-135b).
    pub conversation_titles: std::collections::HashMap<String, String>,
    /// pane_id → transcript JSONL path, populated by SessionStart hooks.
    /// Used for P1 (highest-priority) JSONL discovery in poll_tick.
    pub transcript_path_hints: std::collections::HashMap<String, std::path::PathBuf>,
    /// True when metadata overlay is stale and inventory-only fallback is active.
    pub metadata_stale: bool,
    /// Last successful metadata overlay time.
    pub metadata_last_success_at: Option<chrono::DateTime<Utc>>,
    /// Last metadata overlay error message.
    pub metadata_last_error: Option<String>,
    /// Consecutive metadata overlay failures.
    pub metadata_failure_streak: u32,
    /// Next allowed metadata refresh time under backoff.
    pub metadata_backoff_until: Option<chrono::DateTime<Utc>>,
    /// Runtime start timestamp for daemon health observability.
    pub runtime_started_at: chrono::DateTime<Utc>,
    /// Last successful poll-loop tick completion.
    pub runtime_last_ok_at: Option<chrono::DateTime<Utc>>,
    /// Last runtime failure detail for inventory/probe path.
    pub runtime_last_error: Option<String>,
    /// Best current focus candidate derived from tmux active panes.
    pub focused_pane_id: Option<String>,
    /// Number of tmux windows whose active-pane invariant is currently broken.
    pub focus_mismatch_count: u64,
    /// Last time focus reflection was internally consistent.
    pub focus_last_sync_at: Option<chrono::DateTime<Utc>>,
    /// Canonical live sync-v3 semantic truth keyed by pane_id.
    pub sync_v3: SyncV3LiveState,
}

impl DaemonState {
    pub fn new() -> Self {
        let now = Utc::now();
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
        trust_guard.register_source("claude_hooks");
        trust_guard.register_source("claude_jsonl");

        Self {
            poller: PollerSourceState::new(),
            claude_source: ClaudeSourceState::new(),
            claude_jsonl_source: ClaudeJsonlSourceState::new(),
            claude_jsonl_watchers: std::collections::HashMap::new(),
            codex_jsonl_source: CodexJsonlSourceState::new(),
            codex_jsonl_watchers: std::collections::HashMap::new(),
            gateway: Gateway::with_sources(
                &[
                    SourceKind::Poller,
                    SourceKind::CodexJsonl,
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
            conversation_titles: std::collections::HashMap::new(),
            transcript_path_hints: std::collections::HashMap::new(),
            metadata_stale: false,
            metadata_last_success_at: None,
            metadata_last_error: None,
            metadata_failure_streak: 0,
            metadata_backoff_until: None,
            runtime_started_at: now,
            runtime_last_ok_at: None,
            runtime_last_error: None,
            focused_pane_id: None,
            focus_mismatch_count: 0,
            focus_last_sync_at: None,
            sync_v3: SyncV3LiveState::default(),
        }
    }
}

/// Run the daemon: starts poll loop and UDS server, waits for shutdown signal.
pub async fn run_daemon(opts: DaemonOpts, socket_path: &str) -> anyhow::Result<()> {
    let executor = Arc::new(build_executor(&opts));
    let state = Arc::new(Mutex::new(DaemonState::new()));
    let pane_cache = server::new_pane_cache();

    {
        let st = state.lock().await;
        server::refresh_pane_cache(&pane_cache, &st, Utc::now());
    }

    // Start UDS server
    let server_state = Arc::clone(&state);
    let server_cache = Arc::clone(&pane_cache);
    let server_socket = socket_path.to_string();
    let server_handle = tokio::spawn(async move {
        if let Err(e) = server::run_server(&server_socket, server_state, server_cache).await {
            tracing::error!("UDS server error: {e}");
        }
    });

    // Start poll loop
    let poll_state = Arc::clone(&state);
    let poll_cache = Arc::clone(&pane_cache);
    let poll_executor = Arc::clone(&executor);
    let poll_ms = opts.poll_interval_ms;
    let poll_handle = tokio::spawn(async move {
        run_poll_loop(poll_executor, poll_state, poll_cache, poll_ms).await;
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
    let mut target_source = "default";

    // Socket targeting: --tmux-socket > AGTMUX_TMUX_SOCKET_PATH > AGTMUX_TMUX_SOCKET_NAME
    if let Some(ref socket) = opts.tmux_socket {
        executor = executor.with_socket_path(socket.clone());
        target_source = "--tmux-socket";
    } else if let Ok(path) = std::env::var("AGTMUX_TMUX_SOCKET_PATH") {
        executor = executor.with_socket_path(path);
        target_source = "AGTMUX_TMUX_SOCKET_PATH";
    } else if let Ok(name) = std::env::var("AGTMUX_TMUX_SOCKET_NAME") {
        executor = executor.with_socket_name(name);
        target_source = "AGTMUX_TMUX_SOCKET_NAME";
    }

    tracing::info!(
        "tmux executor configured: bin={} target={} source={target_source}",
        executor.tmux_bin_path(),
        executor.target_description()
    );

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
    pane_cache: server::SharedPaneCache,
    poll_ms: u64,
) {
    let mut ticker = interval(Duration::from_millis(poll_ms));

    loop {
        ticker.tick().await;

        if let Err(e) = poll_tick_with_cache(&executor, &state, &pane_cache).await {
            tracing::warn!("poll tick failed: {e}");
        }
    }
}

#[cfg(test)]
async fn poll_tick<R: TmuxCommandRunner + 'static>(
    executor: &Arc<R>,
    state: &Arc<Mutex<DaemonState>>,
) -> anyhow::Result<()> {
    let ephemeral_cache = server::new_pane_cache();
    poll_tick_with_cache(executor, state, &ephemeral_cache).await
}

fn metadata_backoff_delay_ms(streak: u32) -> i64 {
    const INITIAL_MS: i64 = 500;
    const MAX_MS: i64 = 8000;

    let exp = streak.saturating_sub(1).min(8);
    let delay = INITIAL_MS.saturating_mul(1_i64 << exp);
    delay.min(MAX_MS)
}

fn is_codex_jsonl_candidate(process_hint: Option<&str>, current_cmd: &str) -> bool {
    const CODEX_JSONL_RUNTIME_CMDS: &[&str] = &["node"];

    match process_hint {
        Some("codex") => true,
        Some("runtime_unknown") => CODEX_JSONL_RUNTIME_CMDS.contains(&current_cmd),
        Some("shell") | Some("claude") => false,
        Some(_) => false,
        None => CODEX_JSONL_RUNTIME_CMDS.contains(&current_cmd),
    }
}

fn record_runtime_error(st: &mut DaemonState, detail: impl Into<String>) {
    st.runtime_last_error = Some(detail.into());
}

fn record_runtime_ok(st: &mut DaemonState, now: chrono::DateTime<Utc>) {
    st.runtime_last_ok_at = Some(now);
    st.runtime_last_error = None;
}

fn update_focus_state(st: &mut DaemonState, now: chrono::DateTime<Utc>) {
    use std::collections::BTreeMap;

    let mut active_by_window: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut active_attached = Vec::new();
    let mut active_any = Vec::new();

    for pane in &st.last_panes {
        let active_count = active_by_window
            .entry((pane.session_id.clone(), pane.window_id.clone()))
            .or_insert(0);
        if pane.active {
            *active_count += 1;
            if pane.session_attached {
                active_attached.push(pane);
            }
            active_any.push(pane);
        }
    }

    active_attached.sort_by(|a, b| {
        (&a.session_id, &a.window_id, &a.pane_id).cmp(&(&b.session_id, &b.window_id, &b.pane_id))
    });
    active_any.sort_by(|a, b| {
        (&a.session_id, &a.window_id, &a.pane_id).cmp(&(&b.session_id, &b.window_id, &b.pane_id))
    });

    st.focus_mismatch_count = active_by_window
        .values()
        .filter(|count| **count != 1)
        .count() as u64;
    st.focused_pane_id = active_attached
        .first()
        .or_else(|| active_any.first())
        .map(|pane| pane.pane_id.clone());

    if st.focus_mismatch_count == 0 && st.focused_pane_id.is_some() {
        st.focus_last_sync_at = Some(now);
    }
}

async fn poll_tick_with_cache<R: TmuxCommandRunner + 'static>(
    executor: &Arc<R>,
    state: &Arc<Mutex<DaemonState>>,
    pane_cache: &server::SharedPaneCache,
) -> anyhow::Result<()> {
    let tick_start = std::time::Instant::now();
    let now = Utc::now();

    // 1. List panes (blocking subprocess)
    let exec = Arc::clone(executor);
    let panes: Vec<TmuxPaneInfo> =
        match tokio::task::spawn_blocking(move || list_panes(&*exec)).await {
            Ok(Ok(panes)) => panes,
            Ok(Err(e)) => {
                tracing::warn!("tmux list-panes failed: {e}");
                let mut st = state.lock().await;
                st.metadata_stale = true;
                let detail = format!("inventory fetch failed: {e}");
                st.metadata_last_error = Some(detail.clone());
                record_runtime_error(&mut st, detail);
                server::refresh_pane_cache(pane_cache, &st, now);
                return Ok(());
            }
            Err(e) => {
                tracing::warn!("tmux inventory task failed: {e}");
                let mut st = state.lock().await;
                st.metadata_stale = true;
                let detail = format!("inventory task failed: {e}");
                st.metadata_last_error = Some(detail.clone());
                record_runtime_error(&mut st, detail);
                server::refresh_pane_cache(pane_cache, &st, now);
                return Ok(());
            }
        };

    tracing::debug!("listed {} panes", panes.len());

    // 2. Update generation tracker and publish inventory-first cached snapshot.
    let generation_tracker = {
        let mut st = state.lock().await;
        let pane_ids: Vec<&str> = panes.iter().map(|p| p.pane_id.as_str()).collect();
        st.generation_tracker.update(&pane_ids, now);
        st.last_panes = panes.clone();
        st.sync_v3.retain_inventory(&panes);
        update_focus_state(&mut st, now);
        server::refresh_pane_cache(pane_cache, &st, now);
        st.generation_tracker.clone()
    };

    // 2b. Metadata backoff gate.
    let metadata_backoff_active = {
        let st = state.lock().await;
        st.metadata_backoff_until
            .map(|until| now < until)
            .unwrap_or(false)
    };

    // 2.5. Scan all processes once per tick for deep agent identification (T-128).
    // Executed in a blocking thread to avoid starving the async runtime.
    let mut metadata_failure_reason: Option<String> = None;
    let process_map = if metadata_backoff_active {
        std::collections::HashMap::new()
    } else {
        match tokio::time::timeout(
            Duration::from_millis(300),
            tokio::task::spawn_blocking(scan_all_processes),
        )
        .await
        {
            Ok(Ok(Ok(map))) => {
                if map.is_empty() {
                    metadata_failure_reason =
                        Some("process scan returned no processes".to_string());
                    tracing::warn!("process scan returned no processes");
                }
                map
            }
            Ok(Ok(Err(e))) => {
                metadata_failure_reason = Some(format!("process scan failed: {e}"));
                tracing::warn!("process scan failed: {e}");
                std::collections::HashMap::new()
            }
            Ok(Err(e)) => {
                metadata_failure_reason = Some(format!("process scan task join failed: {e}"));
                tracing::warn!("process scan task join failed: {e}");
                std::collections::HashMap::new()
            }
            Err(_) => {
                metadata_failure_reason = Some("process scan timeout".to_string());
                tracing::warn!("process scan timeout");
                std::collections::HashMap::new()
            }
        }
    };

    // 3. Capture each pane and build snapshots
    let mut snapshots = Vec::with_capacity(panes.len());
    let mut capture_failures = 0usize;

    for pane in &panes {
        let capture_lines = if metadata_backoff_active {
            Vec::new()
        } else {
            let exec = Arc::clone(executor);
            let pane_id = pane.pane_id.clone();
            match tokio::time::timeout(
                Duration::from_millis(150),
                tokio::task::spawn_blocking(move || capture_pane(&*exec, &pane_id, 50)),
            )
            .await
            {
                Ok(Ok(Ok(lines))) => lines,
                Ok(Ok(Err(e))) => {
                    capture_failures += 1;
                    tracing::debug!("capture failed for {}: {e}", pane.pane_id);
                    Vec::new()
                }
                Ok(Err(e)) => {
                    capture_failures += 1;
                    tracing::debug!("capture task failed for {}: {e}", pane.pane_id);
                    Vec::new()
                }
                Err(_) => {
                    capture_failures += 1;
                    tracing::debug!("capture timeout for {}", pane.pane_id);
                    Vec::new()
                }
            }
        };

        let snapshot = to_pane_snapshot(
            pane,
            capture_lines,
            &generation_tracker,
            now,
            Some(&process_map),
        );
        snapshots.push(snapshot);
    }

    if !metadata_backoff_active
        && capture_failures > 0
        && capture_failures.saturating_mul(2) >= panes.len().max(1)
        && metadata_failure_reason.is_none()
    {
        metadata_failure_reason = Some(format!(
            "pane capture degraded: {capture_failures}/{}",
            panes.len()
        ));
        tracing::warn!("pane capture degraded: {capture_failures}/{}", panes.len());
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

    // 6a. Codex JSONL semantic detection.
    //
    // Uses agtmux-source-codex-jsonl to detect Codex sessions by reading
    // their JSONL files and running a semantic FSM (not mtime heuristics).
    //
    // Neutral `node` runtimes are also valid candidates here. In app-child
    // launch paths, tmux can already show `pane_current_command=node` on the
    // correct socket while deep process inspection is degraded or only yields
    // `process_hint=runtime_unknown`. Discovery must still be able to recover
    // Codex truth from tmux CWD + CODEX_HOME.
    if !metadata_backoff_active {
        let snapshot_hint: std::collections::HashMap<&str, Option<&str>> = snapshots
            .iter()
            .map(|s| (s.pane_id.as_str(), s.process_hint.as_deref()))
            .collect();
        let snapshot_cmd: std::collections::HashMap<&str, &str> = snapshots
            .iter()
            .map(|s| (s.pane_id.as_str(), s.current_cmd.as_str()))
            .collect();

        let codex_hints: Vec<CodexPaneHint> = panes
            .iter()
            .filter(|p| {
                let process_hint = snapshot_hint.get(p.pane_id.as_str()).copied().flatten();
                let current_cmd = snapshot_cmd.get(p.pane_id.as_str()).copied().unwrap_or("");
                is_codex_jsonl_candidate(process_hint, current_cmd)
            })
            .map(|p| {
                let (pane_gen, pane_birth) = st
                    .generation_tracker
                    .get(&p.pane_id)
                    .map(|(g, b)| (Some(g), Some(b)))
                    .unwrap_or((None, None));
                CodexPaneHint {
                    pane_id: p.pane_id.clone(),
                    pane_pid: p.pane_pid,
                    cwd: p.current_path.clone(),
                    existing_jsonl_path: st
                        .codex_jsonl_watchers
                        .get(&p.pane_id)
                        .map(|watcher| watcher.path().to_path_buf()),
                    pane_generation: pane_gen,
                    pane_birth_ts: pane_birth,
                }
            })
            .collect();

        if !codex_hints.is_empty() {
            let discoveries = CodexJsonlSourceState::discover_sessions(&codex_hints);
            // Split borrow: take the watchers map out temporarily so Rust allows
            // &mut source alongside the watchers reference.
            let mut watchers = std::mem::take(&mut st.codex_jsonl_watchers);
            let events = st
                .codex_jsonl_source
                .poll_files(&mut watchers, &discoveries, now);
            st.codex_jsonl_watchers = watchers;
            let codex_titles: Vec<(String, String)> = discoveries
                .iter()
                .filter_map(|disc| {
                    st.codex_jsonl_watchers
                        .get(&disc.pane_id)
                        .and_then(|w| w.last_first_prompt())
                        .map(|title| (disc.session_key.clone(), title.to_string()))
                })
                .collect();
            for (session_key, title) in codex_titles {
                st.conversation_titles.entry(session_key).or_insert(title);
            }
            if !events.is_empty() {
                tracing::debug!("codex jsonl: {} events", events.len());
            }
            for event in events {
                st.codex_jsonl_source.ingest(event);
            }
        }
    }

    // 6b. Claude JSONL discovery + poll
    // Scan all panes that might be running Claude for JSONL transcripts (T-126 fix).
    //
    // Previously, discovery was gated on the poller/projection already detecting the pane as
    // Claude.  After a daemon restart the projection is empty, so idle Claude panes (running
    // as `node`) were never discovered → no heartbeat → Codex CWD assignment won falsely.
    //
    // T-127: positive allowlist for neutral-process panes.
    // We include panes that fall into exactly one of these categories:
    //   a) process_hint="claude"  → explicit Claude CLI pane (always include)
    //   b) process_hint=None + current_cmd in CLAUDE_JSONL_RUNTIME_CMDS
    //      → neutral runtime that can host Claude Code (node/bun/deno/python/python3)
    //
    // We EXCLUDE:
    //   - process_hint="shell" (zsh/bash/…): never an agent runtime
    //   - process_hint="codex":  handled by Step 6a (Codex JSONL source)
    //   - process_hint=None + current_cmd NOT in allowlist (yazi, htop, vim, …)
    //   - any other unknown hint: fail-closed
    if !metadata_backoff_active && metadata_failure_reason.is_none() {
        /// Neutral-runtime commands that can host a Claude JSONL session.
        /// Panes with process_hint=None are only included if current_cmd matches.
        /// This prevents false-positive Claude attribution for terminal tools
        /// (yazi, htop, vim, …) that happen to share a CWD with old JSONL files.
        const CLAUDE_JSONL_RUNTIME_CMDS: &[&str] = &["node", "bun", "deno", "python", "python3"];

        // Build snapshot lookup for process_hint and current_cmd.
        let snapshot_hint: std::collections::HashMap<&str, Option<&str>> = snapshots
            .iter()
            .map(|s| (s.pane_id.as_str(), s.process_hint.as_deref()))
            .collect();
        let snapshot_cmd: std::collections::HashMap<&str, &str> = snapshots
            .iter()
            .map(|s| (s.pane_id.as_str(), s.current_cmd.as_str()))
            .collect();

        let candidate_pane_cwds: Vec<PaneDiscoveryHint> = panes
            .iter()
            .filter(|p| {
                let hint = snapshot_hint.get(p.pane_id.as_str()).copied().flatten();
                match hint {
                    // Definite non-Claude processes: exclude
                    Some("shell") | Some("codex") => false,
                    // Known Claude CLI: always include
                    Some("claude") => true,
                    // Unknown non-None hint: fail-closed, exclude
                    Some(_) => false,
                    // Neutral runtime: include only if current_cmd is a known Claude runtime
                    None => {
                        let cmd = snapshot_cmd.get(p.pane_id.as_str()).copied().unwrap_or("");
                        CLAUDE_JSONL_RUNTIME_CMDS.contains(&cmd)
                    }
                }
            })
            .map(|p| {
                let (pane_gen, pane_birth) = st
                    .generation_tracker
                    .get(&p.pane_id)
                    .map(|(g, b)| (Some(g), Some(b)))
                    .unwrap_or((None, None));
                PaneDiscoveryHint {
                    pane_id: p.pane_id.clone(),
                    cwd: p.current_path.clone(),
                    pane_generation: pane_gen,
                    pane_birth_ts: pane_birth,
                    pane_pid: p.pane_pid,
                }
            })
            .collect();

        if !candidate_pane_cwds.is_empty() {
            // P3: CWD-based discovery
            let mut discoveries = ClaudeJsonlSourceState::discover_sessions(&candidate_pane_cwds);

            // P1: transcript_path hints (SessionStart hook payload) override P3.
            for hint in &candidate_pane_cwds {
                if let Some(hint_path) = st.transcript_path_hints.get(&hint.pane_id)
                    && let Some(hint_disc) = discovery_from_transcript_path(
                        &hint.pane_id,
                        hint_path,
                        hint.pane_generation,
                        hint.pane_birth_ts,
                    )
                {
                    // Replace or append: remove any CWD-based discovery for this pane.
                    discoveries.retain(|d| d.pane_id != hint.pane_id);
                    discoveries.push(hint_disc);
                }
            }
            // Use Utc::now() (not poll_tick's `now`) so the bootstrap event's observed_at
            // is fresh relative to the Codex JSONL events emitted in Step 6a.  This ensures
            // last_real_activity[Claude] > last_real_activity[Codex] → Claude wins the
            // select_winning_provider tiebreaker when both have fresh deterministic evidence.
            let jsonl_events = ClaudeJsonlSourceState::poll_files(
                &mut st.claude_jsonl_watchers,
                &discoveries,
                Utc::now(),
            );
            // Collect title signals from watchers for all discovered sessions.
            // Applied in reverse-priority order (lowest first) so later `insert` calls win.
            //
            // Priority chain (highest → lowest):
            //   1. custom-title (explicit user action)             → insert (always wins)
            //   2. summary from watcher (real-time AI summary)     → insert
            //   3. summary from sessions-index.json (historical)   → insert
            //   4. firstPrompt from sessions-index.json            → or_insert
            //   5. first_prompt from watcher history               → or_insert (baseline)

            // Collect all title signals from watchers before mutating conversation_titles
            // (borrow checker: cannot hold &st.claude_jsonl_watchers while mutating st).
            let first_prompts: Vec<(String, String)> = discoveries
                .iter()
                .filter_map(|disc| {
                    st.claude_jsonl_watchers
                        .get(&disc.pane_id)
                        .and_then(|w| w.last_first_prompt())
                        .map(|p| (disc.session_id.clone(), p.to_string()))
                })
                .collect();
            let summaries: Vec<(String, String)> = discoveries
                .iter()
                .filter_map(|disc| {
                    st.claude_jsonl_watchers
                        .get(&disc.pane_id)
                        .and_then(|w| w.last_summary())
                        .map(|s| (disc.session_id.clone(), s.to_string()))
                })
                .collect();
            let titles: Vec<(String, String)> = discoveries
                .iter()
                .filter_map(|disc| {
                    st.claude_jsonl_watchers
                        .get(&disc.pane_id)
                        .and_then(|w| w.last_title())
                        .map(|t| (disc.session_id.clone(), t.to_string()))
                })
                .collect();

            // Apply in reverse-priority order (lowest first, `insert` overwrites lower tiers).
            // Priority: custom-title > summary(watcher) > summary(idx) > firstPrompt > first_prompt

            // 5 (lowest baseline): first user prompt from JSONL watcher history.
            for (session_id, prompt) in first_prompts {
                st.conversation_titles.entry(session_id).or_insert(prompt);
            }

            // 4+3: sessions-index.json (firstPrompt or_insert, then summary insert).
            for disc in &discoveries {
                if let Some(project_dir) = disc.jsonl_path.parent()
                    && let Some(entry) =
                        agtmux_source_claude_jsonl::discovery::read_session_index_entry(
                            project_dir,
                            &disc.session_id,
                        )
                {
                    if let Some(p) = entry.first_prompt.filter(|s| !s.is_empty()) {
                        st.conversation_titles
                            .entry(disc.session_id.clone())
                            .or_insert(p);
                    }
                    if let Some(s) = entry.summary.filter(|s| !s.is_empty()) {
                        st.conversation_titles.insert(disc.session_id.clone(), s);
                    }
                }
            }

            // 2: summary from JSONL watcher (real-time AI summary).
            for (session_id, summary) in summaries {
                st.conversation_titles.insert(session_id, summary);
            }

            // 1 (highest): custom-title from watcher (explicit user action).
            for (session_id, title) in titles {
                st.conversation_titles.insert(session_id, title);
            }
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

    // 8a. Pull events from Codex JSONL source
    let codex_jsonl_cursor = st
        .gateway
        .source_cursor(SourceKind::CodexJsonl)
        .map(String::from);
    let codex_jsonl_response = st.codex_jsonl_source.pull_events(
        &PullEventsRequest {
            cursor: codex_jsonl_cursor,
            limit: 500,
        },
        now,
    );
    st.gateway
        .ingest_source_response(SourceKind::CodexJsonl, codex_jsonl_response);

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

    // 8b-hint: Cache transcript_path from SessionStart for P1 JSONL discovery.
    // Also evict stale hints on SessionEnd.
    for event in &claude_response.events {
        if let Some(pane_id) = &event.pane_id {
            match event.event_type.as_str() {
                "lifecycle.start" => {
                    if let Some(path_str) = event
                        .payload
                        .get("transcript_path")
                        .and_then(|v| v.as_str())
                        && !path_str.is_empty()
                    {
                        st.transcript_path_hints
                            .insert(pane_id.clone(), std::path::PathBuf::from(path_str));
                    }
                }
                "lifecycle.end" => {
                    st.transcript_path_hints.remove(pane_id);
                }
                _ => {}
            }
        }
    }

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

    // 10. Apply to daemon.
    let mut gw_events = gw_response.events;
    for event in &mut gw_events {
        // Source adapters do not all carry tmux generation identity.
        // Stamp any pane-targeted event from the current inventory tracker so
        // sync-v2/v3 exact-row matching stays pane-instance stable.
        if (event.pane_generation.is_none() || event.pane_birth_ts.is_none())
            && let Some(pane_id) = event.pane_id.as_deref()
            && let Some((generation, birth_ts)) = st.generation_tracker.get(pane_id)
        {
            event.pane_generation.get_or_insert(generation);
            event.pane_birth_ts.get_or_insert(birth_ts);
        }
    }
    if !gw_events.is_empty() {
        tracing::debug!("applying {} events to daemon", gw_events.len());
        let last_panes = st.last_panes.clone();
        let generation_tracker = st.generation_tracker.clone();
        st.sync_v3
            .apply_events(&gw_events, &last_panes, &generation_tracker);
        st.daemon.apply_events(gw_events, now);
    }

    // 10b. Tick freshness: downgrade stale deterministic panes to heuristic.
    // This ensures panes whose deterministic source stopped emitting events
    // (e.g. Codex exited, Claude idle) correctly fall back to heuristic.
    st.daemon.tick_freshness(now);

    // 10c. Exact-row shell truth beats stale managed state.
    // If tmux now reports that a pane is back to a plain shell and the
    // projected row is not backed by a live agent process, remove the lingering
    // managed row so CLI/UI surfaces fall back to unmanaged truth immediately.
    const SHELL_CMDS: &[&str] = &[
        "zsh", "bash", "fish", "sh", "csh", "tcsh", "ksh", "dash", "nu", "pwsh",
    ];
    let shell_pane_ids: Vec<String> = snapshots
        .iter()
        .filter(|snapshot| {
            let cmd = snapshot.current_cmd.to_ascii_lowercase();
            SHELL_CMDS.contains(&cmd.as_str())
        })
        .filter(|snapshot| !agent_pane_ids.contains(&snapshot.pane_id))
        .filter(|snapshot| st.daemon.get_pane(&snapshot.pane_id).is_some())
        .map(|snapshot| snapshot.pane_id.clone())
        .collect();
    if !shell_pane_ids.is_empty() {
        st.daemon.demote_panes_to_unmanaged(&shell_pane_ids, now);
        st.sync_v3.demote_panes_to_unmanaged(&shell_pane_ids);
        for pane_id in &shell_pane_ids {
            st.transcript_path_hints.remove(pane_id);
        }
    }

    let managed = st
        .daemon
        .list_panes()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let managed_refs = managed.iter().collect::<Vec<_>>();
    let last_panes = st.last_panes.clone();
    let generation_tracker = st.generation_tracker.clone();
    st.sync_v3
        .reconcile(&managed_refs, &last_panes, &generation_tracker, now);

    // 11. Compact consumed events to prevent unbounded memory growth.
    // Poller: trim events up to the gateway's source cursor.
    if let Some(poller_cursor) = st.gateway.source_cursor(SourceKind::Poller)
        && let Some(seq_str) = poller_cursor.strip_prefix("poller:")
        && let Ok(seq) = seq_str.parse::<u64>()
    {
        st.poller.compact(seq);
    }
    // Codex JSONL: trim events up to the gateway's source cursor.
    if let Some(codex_jsonl_cursor) = st.gateway.source_cursor(SourceKind::CodexJsonl)
        && let Some(seq_str) = codex_jsonl_cursor.strip_prefix("codex-jsonl:")
        && let Ok(seq) = seq_str.parse::<u64>()
    {
        st.codex_jsonl_source.compact(seq);
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

    if metadata_backoff_active {
        st.metadata_stale = true;
    } else if let Some(reason) = metadata_failure_reason {
        st.metadata_stale = true;
        st.metadata_failure_streak = st.metadata_failure_streak.saturating_add(1);
        st.metadata_last_error = Some(reason);
        let delay_ms = metadata_backoff_delay_ms(st.metadata_failure_streak);
        st.metadata_backoff_until = Some(now + TimeDelta::milliseconds(delay_ms));
        tracing::warn!(
            "metadata degraded: streak={} backoff_ms={} reason={}",
            st.metadata_failure_streak,
            delay_ms,
            st.metadata_last_error.as_deref().unwrap_or("unknown")
        );
    } else {
        st.metadata_stale = false;
        st.metadata_last_success_at = Some(now);
        st.metadata_last_error = None;
        st.metadata_failure_streak = 0;
        st.metadata_backoff_until = None;
    }

    record_runtime_ok(&mut st, now);

    server::refresh_pane_cache(pane_cache, &st, now);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agtmux_core_v5::types::{EvidenceTier, Provider, SourceEventV2, SourceKind};
    use agtmux_tmux_v5::error::TmuxError;
    use chrono::DateTime;
    use std::collections::HashMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex as StdMutex;

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

    fn codex_jsonl_semantic_event(
        pane_id: &str,
        inner_type: &str,
        observed_at: DateTime<Utc>,
    ) -> SourceEventV2 {
        SourceEventV2 {
            event_id: format!("codex-jsonl-{inner_type}-{}", observed_at.timestamp()),
            provider: Provider::Codex,
            source_kind: SourceKind::CodexJsonl,
            tier: EvidenceTier::Deterministic,
            observed_at,
            session_key: format!("codex-session-{pane_id}"),
            pane_id: Some(pane_id.to_string()),
            pane_generation: None,
            pane_birth_ts: None,
            source_event_id: None,
            // Legacy compat string stays present for old projection paths, but
            // these runtime fixtures carry native Codex JSONL semantics too.
            event_type: "activity.unknown".to_string(),
            payload: serde_json::json!({
                "codex_jsonl": {
                    "top_type": "event_msg",
                    "inner_type": inner_type,
                    "bootstrap": false
                }
            }),
            confidence: 1.0,
            is_heartbeat: false,
            actual_activity_at: Some(observed_at),
        }
    }

    static ENV_LOCK: StdMutex<()> = StdMutex::new(());

    struct TestEnvGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl TestEnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var(key).ok();
            // SAFETY: test-only env mutation is serialized via ENV_LOCK.
            unsafe { std::env::set_var(key, value) };
            Self { key, previous }
        }
    }

    impl Drop for TestEnvGuard {
        fn drop(&mut self) {
            if let Some(previous) = self.previous.as_ref() {
                // SAFETY: test-only env mutation is serialized via ENV_LOCK.
                unsafe { std::env::set_var(self.key, previous) };
            } else {
                // SAFETY: test-only env mutation is serialized via ENV_LOCK.
                unsafe { std::env::remove_var(self.key) };
            }
        }
    }

    fn temp_dir(label: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("valid unix epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("agtmux-{label}-{nonce}"));
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn write_codex_session_file(codex_home: &Path, cwd: &Path, lines: &[&str]) -> PathBuf {
        let day_dir = codex_home
            .join("sessions")
            .join("2026")
            .join("03")
            .join("09");
        fs::create_dir_all(&day_dir).expect("sessions dir");

        let session_path = day_dir.join("midflight-proof.jsonl");
        let mut payload = Vec::with_capacity(lines.len() + 1);
        payload.push(format!(
            r#"{{"type":"session_meta","payload":{{"type":"session_meta","cwd":"{}","sessionId":"sess-1"}}}}"#,
            cwd.display()
        ));
        payload.extend(lines.iter().map(|line| (*line).to_string()));
        fs::write(&session_path, payload.join("\n") + "\n").expect("session file");
        session_path
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
    async fn poll_tick_shell_pane_with_live_codex_json_promotes_managed() {
        let backend = Arc::new(FakeTmuxBackend::new().with_pane(
            "%0",
            "main",
            "zsh",
            "{\"type\":\"thread.started\"}\n{\"type\":\"turn.started\"}\n{\"type\":\"item.started\",\"status\":\"in_progress\"}",
        ));
        let state = new_state();

        poll_tick(&backend, &state)
            .await
            .expect("tick should succeed");

        let st = state.lock().await;
        let managed = st.daemon.list_panes();
        assert_eq!(
            managed.len(),
            1,
            "live codex JSON stream should promote shell pane"
        );
        assert_eq!(managed[0].provider.map(|p| p.as_str()), Some("codex"));
    }

    #[tokio::test]
    async fn poll_tick_shell_pane_with_prompt_tail_stays_unmanaged() {
        let backend = Arc::new(FakeTmuxBackend::new().with_pane(
            "%0",
            "main",
            "zsh",
            "{\"type\":\"thread.started\"}\n{\"type\":\"turn.started\"}\n❯",
        ));
        let state = new_state();

        poll_tick(&backend, &state)
            .await
            .expect("tick should succeed");

        let st = state.lock().await;
        let managed = st.daemon.list_panes();
        assert!(
            managed.is_empty(),
            "prompt tail should suppress stale shell attribution"
        );
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
        assert!(
            result.is_ok(),
            "inventory failure should preserve state without aborting tick"
        );
        let st = state.lock().await;
        assert!(
            st.metadata_stale,
            "metadata should be marked stale on failure"
        );
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
    async fn poll_tick_poller_events_use_generation_tracker_identity() {
        let backend =
            Arc::new(FakeTmuxBackend::new().with_pane("%0", "main", "claude", "╭ Claude Code"));
        let state = new_state();

        poll_tick(&backend, &state).await.expect("tick");

        let st = state.lock().await;
        let (expected_generation, expected_birth_ts) =
            st.generation_tracker.get("%0").expect("generation tracked");
        let managed = st.daemon.get_pane("%0").expect("managed pane");

        assert_eq!(managed.pane_instance_id.generation, expected_generation);
        assert_eq!(managed.pane_instance_id.birth_ts, expected_birth_ts);
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

    #[tokio::test]
    async fn poll_tick_demotes_managed_pane_after_return_to_shell() {
        let state = new_state();

        let codex_backend = Arc::new(FakeTmuxBackend::new().with_pane(
            "%0",
            "work",
            "codex --model o3",
            "Codex is thinking...",
        ));
        poll_tick(&codex_backend, &state).await.expect("codex tick");

        {
            let st = state.lock().await;
            let pane = st.daemon.get_pane("%0").expect("managed codex pane");
            assert_eq!(pane.provider, Some(agtmux_core_v5::types::Provider::Codex));
        }

        let shell_backend =
            Arc::new(FakeTmuxBackend::new().with_pane("%0", "work", "zsh", "$ echo done"));
        poll_tick(&shell_backend, &state).await.expect("shell tick");

        let st = state.lock().await;
        assert!(
            st.daemon.get_pane("%0").is_none(),
            "pane should be demoted out of managed state once tmux truth is back to shell"
        );
        assert!(
            st.daemon.list_sessions().is_empty(),
            "managed session state should not survive after exact-row return to shell"
        );
    }

    #[tokio::test]
    async fn poll_tick_demotes_deterministic_pane_immediately_after_return_to_shell() {
        let state = new_state();
        let codex_backend = Arc::new(FakeTmuxBackend::new().with_pane(
            "%0",
            "work",
            "codex --model o3",
            "Codex is thinking...",
        ));

        {
            let mut st = state.lock().await;
            st.codex_jsonl_source.ingest(codex_jsonl_semantic_event(
                "%0",
                "task_started",
                Utc::now(),
            ));
        }

        poll_tick(&codex_backend, &state)
            .await
            .expect("deterministic codex tick");

        let shell_backend =
            Arc::new(FakeTmuxBackend::new().with_pane("%0", "work", "zsh", "$ echo done"));
        poll_tick(&shell_backend, &state)
            .await
            .expect("shell demotion tick");

        let st = state.lock().await;
        assert!(
            st.daemon.get_pane("%0").is_none(),
            "deterministic pane should demote as soon as tmux reports an exact shell row with no live agent"
        );
    }

    // ── Deterministic source integration tests ──────────────────────

    #[tokio::test]
    async fn poll_tick_pulls_from_claude_source() {
        use agtmux_source_claude_hooks::translate::ClaudeHookEvent;

        let backend = Arc::new(FakeTmuxBackend::new().with_pane("%0", "main", "node", "$ ls"));
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
    async fn poll_tick_pulls_from_codex_jsonl_source() {
        let backend = Arc::new(FakeTmuxBackend::new().with_pane("%0", "main", "node", "$ ls"));
        let state = new_state();

        // Pre-ingest a Codex JSONL event directly into the codex_jsonl_source
        {
            let mut st = state.lock().await;
            st.codex_jsonl_source.ingest(codex_jsonl_semantic_event(
                "%0",
                "task_started",
                Utc::now(),
            ));
        }

        poll_tick(&backend, &state).await.expect("tick");

        let st = state.lock().await;
        let managed = st.daemon.list_panes();
        assert!(
            !managed.is_empty(),
            "codex jsonl event should create managed pane"
        );
    }

    #[tokio::test]
    async fn poll_tick_discovers_codex_jsonl_from_node_runtime_without_process_hint() {
        let _env_lock = ENV_LOCK.lock().expect("env lock");
        let temp = temp_dir("codex-node-runtime");
        let codex_home = temp.join("codex-home");
        let project_dir = temp.join("project");
        fs::create_dir_all(&project_dir).expect("project dir");
        let _session_path = write_codex_session_file(
            &codex_home,
            &project_dir,
            &[r#"{"type":"event_msg","payload":{"type":"task_started"}}"#],
        );

        let _codex_home = TestEnvGuard::set("CODEX_HOME", codex_home.to_str().expect("utf8 path"));
        let backend = Arc::new(FakeTmuxBackend::new().with_pane_cwd(
            "%0",
            "main",
            "node",
            "$ ls",
            project_dir.to_str().expect("utf8 path"),
        ));
        let state = new_state();

        poll_tick(&backend, &state).await.expect("tick");

        let st = state.lock().await;
        let managed = st.daemon.list_panes();
        assert_eq!(
            managed.len(),
            1,
            "Codex JSONL discovery should manage node runtime"
        );
        assert_eq!(managed[0].provider.map(|p| p.as_str()), Some("codex"));

        drop(st);
        let _ = fs::remove_dir_all(temp);
    }

    #[tokio::test]
    async fn poll_tick_discovers_codex_jsonl_via_home_dot_codex_fallback() {
        let _env_lock = ENV_LOCK.lock().expect("env lock");
        let temp = temp_dir("codex-home-fallback");
        let home_dir = temp.join("home");
        let codex_home = home_dir.join(".codex");
        let project_dir = temp.join("project");
        fs::create_dir_all(&project_dir).expect("project dir");
        let _session_path = write_codex_session_file(
            &codex_home,
            &project_dir,
            &[r#"{"type":"event_msg","payload":{"type":"task_started"}}"#],
        );

        let _home = TestEnvGuard::set("HOME", home_dir.to_str().expect("utf8 path"));
        let _codex_home = TestEnvGuard::set("CODEX_HOME", "");
        let backend = Arc::new(FakeTmuxBackend::new().with_pane_cwd(
            "%0",
            "main",
            "node",
            "$ ls",
            project_dir.to_str().expect("utf8 path"),
        ));
        let state = new_state();

        poll_tick(&backend, &state).await.expect("tick");

        let st = state.lock().await;
        let managed = st.daemon.list_panes();
        assert_eq!(
            managed.len(),
            1,
            "HOME fallback should discover Codex JSONL"
        );
        assert_eq!(managed[0].provider.map(|p| p.as_str()), Some("codex"));

        let payload = st.sync_v3.build_bootstrap(Utc::now());
        let pane = payload
            .panes
            .iter()
            .find(|pane| pane.pane_id == "%0")
            .expect("sync-v3 pane row");
        assert_eq!(
            pane.thread.lifecycle,
            agtmux_core_v5::sync_v3::ThreadLifecycleV3::Active
        );
        assert_eq!(pane.provider, Some(Provider::Codex));
        assert_eq!(pane.presence, agtmux_core_v5::sync_v3::PresenceV3::Managed);

        drop(st);
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn codex_jsonl_candidates_include_neutral_node_runtime() {
        assert!(is_codex_jsonl_candidate(Some("codex"), "zsh"));
        assert!(is_codex_jsonl_candidate(None, "node"));
        assert!(is_codex_jsonl_candidate(Some("runtime_unknown"), "node"));
        assert!(!is_codex_jsonl_candidate(Some("shell"), "node"));
        assert!(!is_codex_jsonl_candidate(Some("claude"), "node"));
        assert!(!is_codex_jsonl_candidate(None, "zsh"));
        assert!(!is_codex_jsonl_candidate(Some("runtime_unknown"), "python"));
    }

    #[tokio::test]
    async fn poll_tick_mixed_poller_and_deterministic() {
        use agtmux_source_claude_hooks::translate::ClaudeHookEvent;

        let backend = Arc::new(
            FakeTmuxBackend::new()
                .with_pane("%0", "main", "claude", "╭ Claude Code") // detected by poller
                .with_pane("%1", "main", "node", "$ ls"), // only via hooks
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

    // ── T-126: JSONL discovery for all panes ──────────────────────

    /// T-126: JSONL discovery must attempt all panes, not just those the poller already
    /// identified as Claude.  After daemon restart, idle Claude panes (no new JSONL lines)
    /// have no poller/projection evidence, so the old filter (`claude_pane_ids`) would
    /// silently skip them — causing Codex CWD assignment to win falsely.
    ///
    /// For panes whose CWD has no ~/.claude/projects/<cwd>/*.jsonl file, discover_sessions
    /// returns empty, so no false events are emitted.  This test uses a unique temp path
    /// that is guaranteed not to have any real JSONL file.
    #[tokio::test]
    async fn poll_tick_jsonl_discovery_scans_all_panes() {
        // %0 is 'node' (Claude Code runtime process).  Before T-126 the poller would
        // NOT include it in claude_pane_ids (poller looks for "claude" in cmd string),
        // so discover_sessions was never called for it.  After T-126 it is called.
        //
        // No JSONL file exists for this temp CWD, so the watcher list stays empty and
        // no events are emitted — the important thing is the code path doesn't panic
        // and does not silently skip the pane.
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let tmp_cwd = format!("/tmp/agtmux-t126-test-{nonce}");

        let backend = Arc::new(
            FakeTmuxBackend::new()
                .with_pane_cwd("%0", "main", "node", "", &tmp_cwd)
                .with_pane_cwd("%1", "main", "zsh", "$ ls", "/no-jsonl-here-either"),
        );
        let state = new_state();

        poll_tick(&backend, &state)
            .await
            .expect("tick should succeed");

        let st = state.lock().await;
        // No JSONL files on disk for these CWDs → no watchers, no JSONL events
        assert!(
            st.claude_jsonl_watchers.is_empty(),
            "no JSONL files → no watchers should be created"
        );
        assert_eq!(
            st.claude_jsonl_source.buffered_len(),
            0,
            "no JSONL files → no events buffered"
        );
        // All panes are still tracked by the poll loop
        assert_eq!(
            st.last_panes.len(),
            2,
            "both panes must be tracked in last_panes"
        );
    }
}

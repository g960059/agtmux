//! Codex App Server integration.
//!
//! Two evidence paths for Codex:
//!
//! 1. **App Server client** (primary): Spawns `codex app-server` in stdio mode,
//!    performs JSON-RPC 2.0 handshake, and polls `thread/list` for active thread
//!    status. See <https://developers.openai.com/codex/app-server/>.
//!
//! 2. **Capture-based extraction** (fallback): When `codex exec --json` outputs
//!    NDJSON events to stdout, they appear in tmux capture text and can be parsed
//!    as deterministic evidence without an app-server connection.
//!
//! The poll_loop tries the app-server first. If spawning fails (no `codex` binary,
//! auth issues, etc.), the capture-based fallback is used automatically.

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::time::Duration;

use agtmux_source_codex_appserver::translate::CodexRawEvent;
use chrono::{DateTime, Utc};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

// ─── Pane CWD info for cwd correlation (T-119) ─────────────────────

/// Pane metadata used for cwd ↔ thread correlation.
#[derive(Debug, Clone)]
pub struct PaneCwdInfo {
    /// tmux pane ID (e.g. "%5").
    pub pane_id: String,
    /// Current working directory of the pane.
    pub cwd: String,
    /// Pane generation from PaneGenerationTracker.
    pub generation: Option<u64>,
    /// Pane birth timestamp from PaneGenerationTracker.
    pub birth_ts: Option<chrono::DateTime<Utc>>,
    /// The agent CLI detected in this pane's current command, if any.
    /// Examples: `Some("codex")`, `Some("claude")`, `None` for neutral shells.
    /// Used for 3-tier assignment priority: codex(0) > neutral(1) > competing-agent(2).
    pub process_hint: Option<String>,
}

/// Correlated pane identity for a Codex thread.
#[derive(Debug, Clone)]
struct ThreadPaneBinding {
    pane_id: String,
    pane_generation: Option<u64>,
    pane_birth_ts: Option<chrono::DateTime<Utc>>,
}

/// Last emitted thread state (used for deduplication + heartbeat).
#[derive(Debug, Clone)]
struct LastThreadState {
    status: String,
    pane_id: Option<String>,
    /// When this state was last emitted (for heartbeat re-emission).
    emitted_at: DateTime<Utc>,
}

/// Heartbeat interval: re-emit unchanged thread state to keep
/// deterministic freshness alive. Must be shorter than
/// `agtmux_core_v5::resolver::FRESH_THRESHOLD_SECS` (3s).
const HEARTBEAT_INTERVAL_SECS: i64 = 2;

/// Assignment priority tier for a pane based on its process hint.
/// Lower value = higher priority for Codex thread assignment.
/// - 0: Running Codex → preferred
/// - 1: Neutral shell (zsh, bash, etc.) → acceptable
/// - 2: Running a competing agent (claude, gemini, …) → deprioritized
///   (those panes have their own deterministic sources; T-123 arbitration resolves conflicts)
fn pane_tier(p: &PaneCwdInfo) -> u8 {
    match p.process_hint.as_deref() {
        Some("codex") => 0,
        None => 1,
        _ => 2,
    }
}

/// Build a cwd → pane-group map for multi-pane Codex thread assignment.
///
/// All panes sharing the same cwd are collected into a `Vec`, sorted by
/// assignment priority (tier 0→2) then `pane_id` for deterministic ordering.
/// Every pane in the group is eligible for assignment; threads are round-robined
/// across the group in `process_thread_list_response`.
fn build_cwd_pane_groups(pane_cwds: &[PaneCwdInfo]) -> HashMap<String, Vec<PaneCwdInfo>> {
    let mut map: HashMap<String, Vec<PaneCwdInfo>> = HashMap::new();
    for info in pane_cwds {
        if !info.cwd.is_empty() {
            map.entry(info.cwd.clone()).or_default().push(info.clone());
        }
    }
    for panes in map.values_mut() {
        panes.sort_by(|a, b| {
            pane_tier(a)
                .cmp(&pane_tier(b))
                .then_with(|| a.pane_id.cmp(&b.pane_id))
        });
    }
    map
}

const MAX_CWD_QUERIES_PER_TICK: usize = 40;
const THREAD_LIST_REQUEST_TIMEOUT: Duration = Duration::from_millis(500);

// ─── App Server client (JSON-RPC 2.0 over stdio) ────────────────────

/// Codex App Server client connected via stdio JSON-RPC 2.0.
///
/// Protocol reference: <https://developers.openai.com/codex/app-server/>
///
/// Lifecycle:
/// 1. Spawn `codex app-server` with stdio transport
/// 2. Send `initialize` request, receive response
/// 3. Send `initialized` notification
/// 4. Issue `thread/list` requests to discover active threads
pub struct CodexAppServerClient {
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    /// Tracks which thread states we've already emitted.
    last_thread_states: HashMap<String, LastThreadState>,
    /// Best-known thread -> pane binding, reused for pane-less events.
    thread_pane_bindings: HashMap<String, ThreadPaneBinding>,
}

/// Result of attempting to spawn and connect to the Codex App Server.
#[derive(Debug)]
pub enum AppServerSpawnResult {
    Connected,
    Failed(String),
}

impl CodexAppServerClient {
    /// Spawn `codex app-server` and perform the JSON-RPC initialize handshake.
    ///
    /// Returns `None` if:
    /// - `codex` binary is not found
    /// - The app-server process fails to start
    /// - The initialize handshake times out or fails
    pub async fn spawn() -> Option<Self> {
        let mut child = match Command::new("codex")
            .arg("app-server")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                tracing::info!("codex app-server not available: {e}");
                return None;
            }
        };

        let stdin = child.stdin.take()?;
        let stdout = BufReader::new(child.stdout.take()?);

        let mut client = Self {
            _child: child,
            stdin,
            stdout,
            next_id: 1,
            last_thread_states: HashMap::new(),
            thread_pane_bindings: HashMap::new(),
        };

        // Perform JSON-RPC initialize handshake with 10s timeout
        match tokio::time::timeout(std::time::Duration::from_secs(10), client.initialize()).await {
            Ok(Ok(())) => {
                tracing::info!("codex app-server connected (stdio transport)");
                Some(client)
            }
            Ok(Err(e)) => {
                tracing::warn!("codex app-server handshake failed: {e}");
                None
            }
            Err(_) => {
                tracing::warn!("codex app-server handshake timed out");
                None
            }
        }
    }

    /// Send the JSON-RPC `initialize` request and `initialized` notification.
    async fn initialize(&mut self) -> anyhow::Result<()> {
        // Step 1: Send initialize request
        let init_request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "initialize",
            "id": self.alloc_id(),
            "params": {
                "clientInfo": {
                    "name": "agtmux",
                    "title": "agtmux-v5",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": {}
            }
        });
        self.send(&init_request).await?;

        // Step 2: Read response
        let response = self.recv().await?;
        if response.get("error").is_some() {
            anyhow::bail!(
                "initialize error: {}",
                response["error"]["message"].as_str().unwrap_or("unknown")
            );
        }

        // Step 3: Send initialized notification (no id = notification)
        let initialized =
            serde_json::json!({"jsonrpc": "2.0", "method": "initialized", "params": {}});
        self.send(&initialized).await?;

        Ok(())
    }

    /// Poll the app-server for active threads and return new events.
    ///
    /// Calls `thread/list` (with per-pane `cwd` filter when pane info is available)
    /// and translates thread status changes into [`CodexRawEvent`] objects.
    ///
    /// `pane_cwds` maps `(pane_id, cwd, generation, birth_ts)` for cwd correlation (T-119).
    /// When provided, each unique cwd gets its own `thread/list` request, and matched
    /// threads have `pane_id`, `pane_generation`, and `pane_birth_ts` set on their events.
    pub async fn poll_threads(&mut self, pane_cwds: &[PaneCwdInfo]) -> Vec<CodexRawEvent> {
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.poll_threads_inner(pane_cwds),
        )
        .await
        {
            Ok(Ok(events)) => events,
            Ok(Err(e)) => {
                tracing::debug!("codex app-server poll failed: {e}");
                Vec::new()
            }
            Err(_) => {
                tracing::debug!("codex app-server poll timed out");
                Vec::new()
            }
        }
    }

    async fn poll_threads_inner(
        &mut self,
        pane_cwds: &[PaneCwdInfo],
    ) -> anyhow::Result<Vec<CodexRawEvent>> {
        // Drain any pending notifications first
        let notifications = self.drain_notifications().await;

        let now = Utc::now();
        let mut events = Vec::new();

        // Process notifications (turn/started, turn/completed, thread/status/changed).
        // Notifications don't carry cwd directly; when possible we enrich them via
        // cached thread -> pane correlation from thread/list.
        for notif in &notifications {
            let pane_binding = notification_thread_id(notif)
                .and_then(|thread_id| self.thread_pane_bindings.get(thread_id));
            if let Some(event) = notification_to_event_with_pane(notif, now, pane_binding) {
                events.push(event);
            }
        }

        // Build cwd → Vec<PaneCwdInfo> groups (all panes per CWD, sorted by tier).
        let cwd_pane_groups = build_cwd_pane_groups(pane_cwds);

        // Step 1: Per-cwd queries. CWDs with actual Codex panes are queried first.
        let mut query_plan: Vec<(String, Vec<PaneCwdInfo>)> = cwd_pane_groups.into_iter().collect();
        query_plan.sort_by(|(cwd_a, panes_a), (cwd_b, panes_b)| {
            let has_codex_a = panes_a
                .iter()
                .any(|p| p.process_hint.as_deref() == Some("codex"));
            let has_codex_b = panes_b
                .iter()
                .any(|p| p.process_hint.as_deref() == Some("codex"));
            has_codex_b.cmp(&has_codex_a).then_with(|| cwd_a.cmp(cwd_b))
        });
        if query_plan.len() > MAX_CWD_QUERIES_PER_TICK {
            tracing::debug!(
                "codex thread/list cwd queries capped: {} -> {}",
                query_plan.len(),
                MAX_CWD_QUERIES_PER_TICK
            );
        }

        // H2: tick-scope dedup — prevents same thread being assigned in multiple CWD queries
        // (guards against cwd filter returning broader results than expected).
        let mut assigned_in_tick: HashSet<String> = HashSet::new();

        for (cwd, pane_group) in query_plan.into_iter().take(MAX_CWD_QUERIES_PER_TICK) {
            match self
                .send_thread_list_timed(Some(&cwd), THREAD_LIST_REQUEST_TIMEOUT)
                .await
            {
                Ok(response) => {
                    self.process_thread_list_response(
                        &response,
                        &pane_group,
                        &mut assigned_in_tick,
                        now,
                        &mut events,
                    );
                }
                Err(e) => {
                    tracing::debug!("codex thread/list cwd query failed (cwd={cwd}): {e}");
                }
            }
        }

        // Step 2: Global query — empty pane slice means no new assignments.
        // Existing bindings are reused for heartbeat continuity.
        match self
            .send_thread_list_timed(None, THREAD_LIST_REQUEST_TIMEOUT)
            .await
        {
            Ok(response) => {
                self.process_thread_list_response(
                    &response,
                    &[],
                    &mut assigned_in_tick,
                    now,
                    &mut events,
                );
            }
            Err(e) => {
                tracing::debug!("codex thread/list global query failed: {e}");
            }
        }

        Ok(events)
    }

    async fn send_thread_list_timed(
        &mut self,
        cwd: Option<&str>,
        timeout: Duration,
    ) -> anyhow::Result<serde_json::Value> {
        match tokio::time::timeout(timeout, self.send_thread_list(cwd)).await {
            Ok(result) => result,
            Err(_) => anyhow::bail!("thread/list timeout"),
        }
    }

    /// Send a `thread/list` request, optionally filtered by `cwd`.
    /// Returns the response JSON, collecting interleaved notifications into `self`.
    async fn send_thread_list(&mut self, cwd: Option<&str>) -> anyhow::Result<serde_json::Value> {
        let id = self.alloc_id();
        let mut params = serde_json::json!({
            "limit": 50,
            "sortKey": "updated_at"
        });
        if let Some(cwd) = cwd {
            params["cwd"] = serde_json::Value::String(cwd.to_string());
        }
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "thread/list",
            "id": id,
            "params": params
        });
        self.send(&request).await?;

        // Read response (skip notifications until we get our response)
        loop {
            let msg = self.recv().await?;
            if msg.get("id").and_then(|v| v.as_u64()) == Some(id) {
                return Ok(msg);
            }
            // Interleaved notification — ignored (already drained above)
        }
    }

    /// Process a `thread/list` response, emitting events for status changes.
    ///
    /// `pane_infos` is the sorted pane group for this CWD (tier 0→2, pane_id asc).
    /// Pass `&[]` for the global query — no new pane assignments are made.
    ///
    /// `assigned_in_tick` prevents the same thread from being re-assigned by a
    /// subsequent CWD query in the same tick (cwd filter anomaly guard).
    fn process_thread_list_response(
        &mut self,
        response: &serde_json::Value,
        pane_infos: &[PaneCwdInfo],
        assigned_in_tick: &mut HashSet<String>,
        now: chrono::DateTime<Utc>,
        events: &mut Vec<CodexRawEvent>,
    ) {
        // API returns { result: { data: [...] } } per docs/codex-appserver-api-reference.md
        let Some(result) = response.get("result") else {
            return;
        };
        let Some(threads) = result.get("data").and_then(|t| t.as_array()) else {
            return;
        };

        // Sort threads by thread_id for deterministic, stable assignment across ticks.
        let mut sorted_threads: Vec<&serde_json::Value> = threads.iter().collect();
        sorted_threads.sort_by_key(|t| t["id"].as_str().unwrap_or(""));

        // Panes already claimed by cache entries IN THIS RESPONSE.
        // Only threads present in this response count — exited threads release their pane.
        // Additionally, only count a binding as "claimed" if the pane generation still matches
        // the current pane group; a stale binding must not block the pane from being unclaimed.
        let cached_pane_ids: HashSet<String> = sorted_threads
            .iter()
            .filter_map(|t| {
                let tid = t["id"].as_str()?;
                let binding = self.thread_pane_bindings.get(tid)?;
                // For global query (pane_infos empty): trust all existing bindings.
                let generation_valid = pane_infos.is_empty()
                    || pane_infos.iter().any(|p| {
                        p.pane_id == binding.pane_id
                            && p.generation == binding.pane_generation
                            && p.birth_ts == binding.pane_birth_ts
                    });
                if generation_valid {
                    Some(binding.pane_id.clone())
                } else {
                    None // stale binding — leave pane available as unclaimed
                }
            })
            .collect();

        // Unclaimed panes: group members not held by any live thread's cache.
        let mut unclaimed: VecDeque<ThreadPaneBinding> = pane_infos
            .iter()
            .filter(|p| !cached_pane_ids.contains(&p.pane_id))
            .map(|p| ThreadPaneBinding {
                pane_id: p.pane_id.clone(),
                pane_generation: p.generation,
                pane_birth_ts: p.birth_ts,
            })
            .collect();

        for thread in sorted_threads {
            let thread_id = thread["id"].as_str().unwrap_or("");
            // Status is an object { type: "idle" } per the API reference.
            // The real App Server (v0.104.0+) may omit `status` from `thread/list` —
            // default to "idle" (a listed thread is at least available/loaded).
            let status = thread
                .get("status")
                .and_then(|s| s.get("type"))
                .and_then(|t| t.as_str())
                .unwrap_or("idle");

            // Skip notLoaded threads (historical, not active agents).
            if status == "notLoaded" {
                continue;
            }
            if thread_id.is_empty() {
                continue;
            }

            // H2: tick-scope dedup — skip threads already assigned earlier in this tick.
            if assigned_in_tick.contains(thread_id) {
                continue;
            }

            // Determine pane binding for this thread:
            // H1: validate cached binding by pane generation + birth_ts to detect reuse.
            let assigned_binding =
                if let Some(cached) = self.thread_pane_bindings.get(thread_id).cloned() {
                    // For global query (pane_infos empty): trust existing binding as-is.
                    // For CWD queries: verify the pane instance hasn't been recycled.
                    let pane_still_valid = pane_infos.is_empty()
                        || pane_infos.iter().any(|p| {
                            p.pane_id == cached.pane_id
                                && p.generation == cached.pane_generation
                                && p.birth_ts == cached.pane_birth_ts
                        });
                    if pane_still_valid {
                        Some(cached)
                    } else {
                        // Pane was recycled — invalidate stale binding, try unclaimed.
                        self.thread_pane_bindings.remove(thread_id);
                        let b = unclaimed.pop_front();
                        if let Some(ref binding) = b {
                            self.thread_pane_bindings
                                .insert(thread_id.to_string(), binding.clone());
                        }
                        b
                    }
                } else if !pane_infos.is_empty() {
                    // No cache: assign next available pane in group order.
                    let b = unclaimed.pop_front();
                    if let Some(ref binding) = b {
                        self.thread_pane_bindings
                            .insert(thread_id.to_string(), binding.clone());
                    }
                    b
                } else {
                    // Global query: no new assignments.
                    None
                };

            assigned_in_tick.insert(thread_id.to_string());

            // effective_pane_id: new assignment, or fall back to last known state.
            let effective_pane_id = assigned_binding
                .as_ref()
                .map(|b| b.pane_id.clone())
                .or_else(|| {
                    self.last_thread_states
                        .get(thread_id)
                        .and_then(|s| s.pane_id.clone())
                });

            // Emit when status/pane changed, or heartbeat interval elapsed.
            let should_emit = match self.last_thread_states.get(thread_id) {
                None => true,
                Some(prev) => {
                    prev.status != status
                        || prev.pane_id != effective_pane_id
                        || (now - prev.emitted_at).num_seconds() >= HEARTBEAT_INTERVAL_SECS
                }
            };

            if should_emit {
                // Heartbeat: emit triggered by elapsed time only (no status/pane change).
                let is_heartbeat = match self.last_thread_states.get(thread_id) {
                    None => false,
                    Some(prev) => prev.status == status && prev.pane_id == effective_pane_id,
                };

                self.last_thread_states.insert(
                    thread_id.to_string(),
                    LastThreadState {
                        status: status.to_string(),
                        pane_id: effective_pane_id,
                        emitted_at: now,
                    },
                );

                let event_binding =
                    assigned_binding.or_else(|| self.thread_pane_bindings.get(thread_id).cloned());

                let event_type = match status {
                    "active" => "thread.active",
                    "idle" => "thread.idle",
                    "systemError" => "thread.error",
                    _ => "thread.status_changed",
                };

                events.push(CodexRawEvent {
                    id: format!("appserver-{thread_id}-{}", now.timestamp_millis()),
                    event_type: event_type.to_string(),
                    session_id: thread_id.to_string(),
                    timestamp: now,
                    pane_id: event_binding.as_ref().map(|b| b.pane_id.clone()),
                    pane_generation: event_binding.as_ref().and_then(|b| b.pane_generation),
                    pane_birth_ts: event_binding.as_ref().and_then(|b| b.pane_birth_ts),
                    payload: thread.clone(),
                    is_heartbeat,
                });
            }
        }
    }

    /// Try to read any pending notifications (non-blocking).
    async fn drain_notifications(&mut self) -> Vec<serde_json::Value> {
        let mut notifications = Vec::new();
        while let Ok(Ok(msg)) =
            tokio::time::timeout(std::time::Duration::from_millis(10), self.recv()).await
        {
            if msg.get("id").is_none() {
                notifications.push(msg);
            }
        }
        notifications
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    async fn send(&mut self, msg: &serde_json::Value) -> anyhow::Result<()> {
        let mut line = serde_json::to_string(msg)?;
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn recv(&mut self) -> anyhow::Result<serde_json::Value> {
        let mut line = String::new();
        let n = self.stdout.read_line(&mut line).await?;
        if n == 0 {
            anyhow::bail!("codex app-server closed stdout");
        }
        Ok(serde_json::from_str(line.trim())?)
    }

    /// Check if the app-server process is still alive.
    pub fn is_alive(&mut self) -> bool {
        match self._child.try_wait() {
            Ok(None) => true,     // still running
            Ok(Some(_)) => false, // exited
            Err(_) => false,
        }
    }
}

/// Convert a Codex App Server notification to a [`CodexRawEvent`].
///
/// Handles: `turn/started`, `turn/completed`, `thread/status/changed`.
/// Returns `None` for unrecognized notification methods.
fn notification_to_event(
    notif: &serde_json::Value,
    now: chrono::DateTime<Utc>,
) -> Option<CodexRawEvent> {
    notification_to_event_with_pane(notif, now, None)
}

fn notification_thread_id(notif: &serde_json::Value) -> Option<&str> {
    let method = notif["method"].as_str()?;
    let params = notif.get("params")?;
    match method {
        "turn/started" | "turn/completed" | "thread/status/changed" => params["threadId"].as_str(),
        _ => None,
    }
}

fn notification_to_event_with_pane(
    notif: &serde_json::Value,
    now: chrono::DateTime<Utc>,
    pane_binding: Option<&ThreadPaneBinding>,
) -> Option<CodexRawEvent> {
    let method = notif["method"].as_str()?;
    let params = notif.get("params")?;

    match method {
        "turn/started" => {
            let turn_id = params["turn"]["id"].as_str().unwrap_or("unknown");
            Some(CodexRawEvent {
                id: format!("notif-turn-started-{turn_id}"),
                event_type: "turn.started".to_string(),
                session_id: params["threadId"].as_str().unwrap_or("unknown").to_string(),
                timestamp: now,
                pane_id: pane_binding.map(|b| b.pane_id.clone()),
                pane_generation: pane_binding.and_then(|b| b.pane_generation),
                pane_birth_ts: pane_binding.and_then(|b| b.pane_birth_ts),
                payload: params.clone(),
                is_heartbeat: false, // notifications = real activity
            })
        }
        "turn/completed" => {
            let turn_id = params["turn"]["id"].as_str().unwrap_or("unknown");
            let status = params["turn"]["status"].as_str().unwrap_or("completed");
            Some(CodexRawEvent {
                id: format!("notif-turn-completed-{turn_id}"),
                event_type: format!("turn.{status}"),
                session_id: params["threadId"].as_str().unwrap_or("unknown").to_string(),
                timestamp: now,
                pane_id: pane_binding.map(|b| b.pane_id.clone()),
                pane_generation: pane_binding.and_then(|b| b.pane_generation),
                pane_birth_ts: pane_binding.and_then(|b| b.pane_birth_ts),
                payload: params.clone(),
                is_heartbeat: false, // notifications = real activity
            })
        }
        "thread/status/changed" => {
            let thread_id = params["threadId"].as_str().unwrap_or("unknown");
            // Status can be object { type: "active" } or string "active"
            let status = params
                .get("status")
                .and_then(|s| {
                    s.get("type")
                        .and_then(|t| t.as_str())
                        .or_else(|| s.as_str())
                })
                .unwrap_or("unknown");
            Some(CodexRawEvent {
                id: format!("notif-status-{thread_id}-{}", now.timestamp_millis()),
                event_type: format!("thread.{status}"),
                session_id: thread_id.to_string(),
                timestamp: now,
                pane_id: pane_binding.map(|b| b.pane_id.clone()),
                pane_generation: pane_binding.and_then(|b| b.pane_generation),
                pane_birth_ts: pane_binding.and_then(|b| b.pane_birth_ts),
                payload: params.clone(),
                is_heartbeat: false, // notifications = real activity
            })
        }
        _ => None,
    }
}

// ─── Capture-based JSON extraction (fallback) ──────────────────────────

/// Tracks which Codex JSON events have been ingested from tmux capture,
/// preventing re-ingestion of the same event across poll ticks.
///
/// Uses content-based fingerprinting: each JSON line is hashed, and the
/// hash is stored per-pane. This is not cryptographic — it's a fast
/// dedup mechanism for the same terminal output appearing in consecutive
/// captures.
#[derive(Debug, Default)]
pub struct CodexCaptureTracker {
    /// Per-pane set of seen event fingerprints.
    seen: HashMap<String, HashSet<u64>>,
}

impl CodexCaptureTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if this fingerprint is new for the given pane.
    /// Returns `true` if the fingerprint was not seen before (and records it).
    fn is_new(&mut self, pane_id: &str, fingerprint: u64) -> bool {
        self.seen
            .entry(pane_id.to_string())
            .or_default()
            .insert(fingerprint)
    }

    /// Remove tracking for panes that no longer exist.
    pub fn retain_panes(&mut self, active_pane_ids: &[&str]) {
        let active: HashSet<&str> = active_pane_ids.iter().copied().collect();
        self.seen.retain(|k, _| active.contains(k.as_str()));
    }
}

/// Compute a content fingerprint for dedup (not cryptographic).
fn fingerprint(line: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    line.hash(&mut hasher);
    hasher.finish()
}

/// Parse Codex NDJSON events from tmux capture lines (fallback path).
///
/// When Codex CLI runs with `--json`, it outputs newline-delimited JSON events
/// to stdout (e.g., `{"type": "turn.completed", ...}`). This function extracts
/// those events from captured terminal output and converts them to [`CodexRawEvent`].
///
/// Only lines that parse as JSON objects with a `"type"` field are considered
/// Codex events. Other lines (prompts, plain text, partial JSON) are ignored.
///
/// The `tracker` parameter provides cross-tick deduplication: events whose
/// content fingerprint has already been seen for this pane are skipped.
pub fn parse_codex_capture_events(
    capture_lines: &[String],
    pane_id: &str,
    tracker: &mut CodexCaptureTracker,
) -> Vec<CodexRawEvent> {
    let now = Utc::now();
    let mut events = Vec::new();

    for line in capture_lines {
        let trimmed = line.trim();
        if trimmed.is_empty() || !trimmed.starts_with('{') {
            continue;
        }

        let fp = fingerprint(trimmed);
        if !tracker.is_new(pane_id, fp) {
            continue; // Already ingested in a previous tick
        }

        let parsed: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // Must have a "type" field to be a Codex event
        let event_type = match parsed["type"].as_str() {
            Some(t) => t.to_string(),
            None => continue,
        };

        // Generate deterministic event ID from fingerprint
        let id = format!("cap-{fp:016x}");

        // Extract session_id if available, otherwise derive from pane_id
        let session_id = parsed["session_id"]
            .as_str()
            .or_else(|| parsed["id"].as_str())
            .map(String::from)
            .unwrap_or_else(|| format!("codex-{pane_id}"));

        events.push(CodexRawEvent {
            id,
            event_type,
            session_id,
            timestamp: now,
            pane_id: Some(pane_id.to_string()),
            pane_generation: None,
            pane_birth_ts: None,
            payload: parsed,
            is_heartbeat: false, // captured NDJSON events = real activity
        });
    }

    events
}

// ─── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn app_server_spawn_fails_gracefully_when_codex_missing() {
        // If codex is not installed, spawn should return None (not panic)
        // We can't guarantee codex is installed in CI, so this test just
        // verifies no panic/crash on attempt.
        let _result = CodexAppServerClient::spawn().await;
        // Result is either Some (codex installed) or None (not installed) — both OK
    }

    #[test]
    fn notification_to_event_turn_started() {
        let notif = serde_json::json!({
            "method": "turn/started",
            "params": {
                "threadId": "thr_abc",
                "turn": {"id": "turn_1", "status": "inProgress", "items": []}
            }
        });
        let event = notification_to_event(&notif, Utc::now()).expect("should parse turn/started");
        assert_eq!(event.event_type, "turn.started");
        assert_eq!(event.session_id, "thr_abc");
    }

    #[test]
    fn notification_to_event_with_cached_pane_binding() {
        let notif = serde_json::json!({
            "method": "turn/started",
            "params": {
                "threadId": "thr_abc",
                "turn": {"id": "turn_1", "status": "inProgress", "items": []}
            }
        });
        let binding = ThreadPaneBinding {
            pane_id: "%9".to_string(),
            pane_generation: Some(3),
            pane_birth_ts: None,
        };
        let event = notification_to_event_with_pane(&notif, Utc::now(), Some(&binding))
            .expect("should parse turn/started");
        assert_eq!(event.pane_id, Some("%9".to_string()));
        assert_eq!(event.pane_generation, Some(3));
    }

    #[test]
    fn notification_to_event_turn_completed() {
        let notif = serde_json::json!({
            "method": "turn/completed",
            "params": {
                "threadId": "thr_abc",
                "turn": {"id": "turn_1", "status": "completed"}
            }
        });
        let event = notification_to_event(&notif, Utc::now()).expect("should parse turn/completed");
        assert_eq!(event.event_type, "turn.completed");
    }

    #[test]
    fn notification_to_event_thread_status_changed_object() {
        // Official API format: status is { type: "active", activeFlags: [...] }
        let notif = serde_json::json!({
            "method": "thread/status/changed",
            "params": {
                "threadId": "thr_xyz",
                "status": { "type": "active", "activeFlags": ["waitingOnApproval"] }
            }
        });
        let event =
            notification_to_event(&notif, Utc::now()).expect("should parse thread/status/changed");
        assert_eq!(event.event_type, "thread.active");
        assert_eq!(event.session_id, "thr_xyz");
    }

    #[test]
    fn notification_to_event_thread_status_changed_string() {
        // Backwards compat: status as plain string
        let notif = serde_json::json!({
            "method": "thread/status/changed",
            "params": {
                "threadId": "thr_xyz",
                "status": "idle"
            }
        });
        let event = notification_to_event(&notif, Utc::now()).expect("should parse string status");
        assert_eq!(event.event_type, "thread.idle");
    }

    #[test]
    fn notification_to_event_ignores_unknown_methods() {
        let notif = serde_json::json!({
            "method": "item/agentMessage/delta",
            "params": {"text": "hello"}
        });
        assert!(
            notification_to_event(&notif, Utc::now()).is_none(),
            "should ignore non-lifecycle notifications"
        );
    }

    // ── Capture-based JSON extraction tests ─────────────────────────

    #[test]
    fn parse_codex_capture_extracts_json_events() {
        let lines = vec![
            "$ codex exec --json \"do something\"".to_string(),
            "{\"type\":\"message.created\",\"id\":\"msg-1\"}".to_string(),
            "".to_string(),
            "{\"type\":\"turn.completed\",\"id\":\"turn-1\"}".to_string(),
            "wait_result=idle".to_string(),
        ];
        let mut tracker = CodexCaptureTracker::new();
        let events = parse_codex_capture_events(&lines, "%0", &mut tracker);

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, "message.created");
        assert_eq!(events[1].event_type, "turn.completed");
        assert_eq!(events[0].pane_id, Some("%0".to_string()));
        assert_eq!(events[1].pane_id, Some("%0".to_string()));
    }

    #[test]
    fn parse_codex_capture_skips_non_json_and_typeless() {
        let lines = vec![
            "$ ls".to_string(),
            "file.txt".to_string(),
            "not json at all".to_string(),
            "{invalid json}".to_string(),
            "{\"key\":\"value\"}".to_string(),
        ];
        let mut tracker = CodexCaptureTracker::new();
        let events = parse_codex_capture_events(&lines, "%0", &mut tracker);

        assert!(events.is_empty());
    }

    #[test]
    fn codex_capture_tracker_deduplicates_across_ticks() {
        let lines = vec!["{\"type\":\"turn.completed\",\"id\":\"t1\"}".to_string()];
        let mut tracker = CodexCaptureTracker::new();

        let events1 = parse_codex_capture_events(&lines, "%0", &mut tracker);
        assert_eq!(events1.len(), 1);

        let events2 = parse_codex_capture_events(&lines, "%0", &mut tracker);
        assert!(
            events2.is_empty(),
            "same event should be deduplicated across ticks"
        );
    }

    #[test]
    fn codex_capture_tracker_retains_only_active_panes() {
        let mut tracker = CodexCaptureTracker::new();
        let lines = vec!["{\"type\":\"turn.completed\"}".to_string()];

        parse_codex_capture_events(&lines, "%0", &mut tracker);
        parse_codex_capture_events(&lines, "%1", &mut tracker);

        tracker.retain_panes(&["%0"]);

        let events = parse_codex_capture_events(&lines, "%1", &mut tracker);
        assert_eq!(
            events.len(),
            1,
            "should see event again after pane tracking cleared"
        );

        let events0 = parse_codex_capture_events(&lines, "%0", &mut tracker);
        assert!(events0.is_empty(), "%0 should still be deduped");
    }

    // ── T-119/T-124: cwd correlation + multi-pane assignment tests ───

    fn make_pane(pane_id: &str, cwd: &str, hint: Option<&str>) -> PaneCwdInfo {
        make_pane_with_gen(pane_id, cwd, hint, Some(1), None)
    }

    fn make_pane_with_gen(
        pane_id: &str,
        cwd: &str,
        hint: Option<&str>,
        generation: Option<u64>,
        birth_ts: Option<chrono::DateTime<Utc>>,
    ) -> PaneCwdInfo {
        PaneCwdInfo {
            pane_id: pane_id.to_string(),
            cwd: cwd.to_string(),
            generation,
            birth_ts,
            process_hint: hint.map(String::from),
        }
    }

    #[test]
    fn build_cwd_pane_groups_single_pane() {
        let infos = vec![make_pane("%0", "/home/user/project", None)];
        let map = build_cwd_pane_groups(&infos);
        assert_eq!(map.len(), 1);
        assert_eq!(map["/home/user/project"].len(), 1);
        assert_eq!(map["/home/user/project"][0].pane_id, "%0");
    }

    #[test]
    fn build_cwd_pane_groups_different_cwds() {
        let infos = vec![
            make_pane("%0", "/project-a", None),
            make_pane("%1", "/project-b", None),
        ];
        let map = build_cwd_pane_groups(&infos);
        assert_eq!(map.len(), 2);
        assert_eq!(map["/project-a"][0].pane_id, "%0");
        assert_eq!(map["/project-b"][0].pane_id, "%1");
    }

    #[test]
    fn build_cwd_pane_groups_multiple_panes_same_cwd() {
        // 3 panes at same CWD: codex, zsh (None), claude — group should hold all 3
        let infos = vec![
            make_pane("%3", "/proj", Some("claude")),
            make_pane("%1", "/proj", None),
            make_pane("%2", "/proj", Some("codex")),
        ];
        let map = build_cwd_pane_groups(&infos);
        assert_eq!(map.len(), 1);
        let group = &map["/proj"];
        assert_eq!(group.len(), 3, "all 3 panes must be in the group");
        // tier sort: codex(%2,tier0) → neutral(%1,tier1) → claude(%3,tier2)
        assert_eq!(group[0].pane_id, "%2", "codex pane first (tier 0)");
        assert_eq!(group[1].pane_id, "%1", "neutral pane second (tier 1)");
        assert_eq!(group[2].pane_id, "%3", "competing-agent pane last (tier 2)");
    }

    #[test]
    fn build_cwd_pane_groups_tier_sort() {
        // Verify 3-tier: codex(0) < neutral(1) < competing-agent(2), tiebreak by pane_id
        let infos = vec![
            make_pane("%5", "/x", Some("gemini")), // tier 2
            make_pane("%3", "/x", None),           // tier 1
            make_pane("%1", "/x", Some("codex")),  // tier 0
            make_pane("%4", "/x", Some("claude")), // tier 2
            make_pane("%2", "/x", None),           // tier 1
        ];
        let map = build_cwd_pane_groups(&infos);
        let group = &map["/x"];
        assert_eq!(group[0].pane_id, "%1"); // codex, tier 0
        assert_eq!(group[1].pane_id, "%2"); // None, tier 1, pane_id "%2" < "%3"
        assert_eq!(group[2].pane_id, "%3"); // None, tier 1
        assert_eq!(group[3].pane_id, "%4"); // claude, tier 2, pane_id "%4" < "%5"
        assert_eq!(group[4].pane_id, "%5"); // gemini, tier 2
    }

    // ── process_thread_list_response: stable multi-pane assignment ────

    /// Creates a minimal `CodexAppServerClient` backed by a `cat` subprocess.
    /// The subprocess is never actually communicated with in these tests;
    /// we only exercise the pure-Rust assignment logic.
    async fn make_test_client() -> CodexAppServerClient {
        let mut child = Command::new("cat")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("cat must be available on the test host");
        let stdin = child.stdin.take().expect("stdin piped");
        let stdout = BufReader::new(child.stdout.take().expect("stdout piped"));
        CodexAppServerClient {
            _child: child,
            stdin,
            stdout,
            next_id: 1,
            last_thread_states: HashMap::new(),
            thread_pane_bindings: HashMap::new(),
        }
    }

    fn make_thread_list_response(thread_ids: &[&str]) -> serde_json::Value {
        let threads: Vec<serde_json::Value> = thread_ids
            .iter()
            .map(|id| serde_json::json!({"id": id, "status": {"type": "idle"}}))
            .collect();
        serde_json::json!({"result": {"data": threads}})
    }

    #[tokio::test]
    async fn process_thread_list_multiple_threads_stable_assignment() {
        let mut client = make_test_client().await;
        let now = Utc::now();
        let panes = vec![
            make_pane("%1", "/proj", Some("codex")),
            make_pane("%2", "/proj", None),
        ];
        let resp = make_thread_list_response(&["t-a", "t-b"]);
        let mut tick = HashSet::new();
        let mut events = Vec::new();

        client.process_thread_list_response(&resp, &panes, &mut tick, now, &mut events);

        // t-a (first in thread_id sort) → %1 (first in pane group), t-b → %2
        assert_eq!(client.thread_pane_bindings["t-a"].pane_id, "%1");
        assert_eq!(client.thread_pane_bindings["t-b"].pane_id, "%2");
        assert_eq!(events.len(), 2);
    }

    #[tokio::test]
    async fn process_thread_list_stable_assignment_across_ticks() {
        let mut client = make_test_client().await;
        let now = Utc::now();
        let panes = vec![
            make_pane_with_gen("%1", "/proj", Some("codex"), Some(1), None),
            make_pane_with_gen("%2", "/proj", None, Some(1), None),
        ];
        let resp = make_thread_list_response(&["t-a", "t-b"]);

        // Tick 1
        let mut tick1 = HashSet::new();
        client.process_thread_list_response(&resp, &panes, &mut tick1, now, &mut Vec::new());
        assert_eq!(client.thread_pane_bindings["t-a"].pane_id, "%1");
        assert_eq!(client.thread_pane_bindings["t-b"].pane_id, "%2");

        // Tick 2 — same panes (generation matches) → same assignment
        let now2 = now + chrono::Duration::seconds(3);
        let mut tick2 = HashSet::new();
        let mut events2 = Vec::new();
        client.process_thread_list_response(&resp, &panes, &mut tick2, now2, &mut events2);
        assert_eq!(
            client.thread_pane_bindings["t-a"].pane_id, "%1",
            "assignment must be stable across ticks"
        );
        assert_eq!(client.thread_pane_bindings["t-b"].pane_id, "%2");
    }

    #[tokio::test]
    async fn process_thread_list_freed_pane_reassigned_to_new_thread() {
        let mut client = make_test_client().await;
        let now = Utc::now();
        let panes = vec![
            make_pane_with_gen("%1", "/proj", Some("codex"), Some(1), None),
            make_pane_with_gen("%2", "/proj", None, Some(1), None),
        ];

        // Tick 1: t-a → %1, t-b → %2
        let resp1 = make_thread_list_response(&["t-a", "t-b"]);
        let mut tick1 = HashSet::new();
        client.process_thread_list_response(&resp1, &panes, &mut tick1, now, &mut Vec::new());

        // Tick 2: t-a exits, t-c appears. t-b keeps %2; t-c should get %1 (freed).
        let now2 = now + chrono::Duration::seconds(3);
        let resp2 = make_thread_list_response(&["t-b", "t-c"]);
        let mut tick2 = HashSet::new();
        let mut events2 = Vec::new();
        client.process_thread_list_response(&resp2, &panes, &mut tick2, now2, &mut events2);

        assert_eq!(
            client.thread_pane_bindings["t-b"].pane_id, "%2",
            "t-b keeps its pane"
        );
        assert_eq!(
            client.thread_pane_bindings["t-c"].pane_id, "%1",
            "t-c gets the pane freed by t-a"
        );
    }

    #[tokio::test]
    async fn process_thread_list_generation_mismatch_invalidates_cache() {
        let mut client = make_test_client().await;
        let now = Utc::now();

        // Tick 1: bind t-a → %1 (gen=1)
        let panes_gen1 = vec![make_pane_with_gen(
            "%1",
            "/proj",
            Some("codex"),
            Some(1),
            None,
        )];
        let resp = make_thread_list_response(&["t-a"]);
        let mut tick1 = HashSet::new();
        client.process_thread_list_response(&resp, &panes_gen1, &mut tick1, now, &mut Vec::new());
        assert_eq!(client.thread_pane_bindings["t-a"].pane_id, "%1");

        // Pane %1 is recycled (generation bumped to 2). New pane %2 added.
        let panes_gen2 = vec![
            make_pane_with_gen("%1", "/proj", Some("codex"), Some(2), None), // recycled
            make_pane_with_gen("%2", "/proj", None, Some(1), None),
        ];
        let now2 = now + chrono::Duration::seconds(3);
        let mut tick2 = HashSet::new();
        let mut events2 = Vec::new();
        client.process_thread_list_response(&resp, &panes_gen2, &mut tick2, now2, &mut events2);

        // Old binding (gen=1) is invalid → should get new unclaimed pane (%1, gen=2)
        let binding = &client.thread_pane_bindings["t-a"];
        assert_eq!(binding.pane_id, "%1");
        assert_eq!(
            binding.pane_generation,
            Some(2),
            "binding must use new generation"
        );
    }
}

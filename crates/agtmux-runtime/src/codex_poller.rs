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

use std::collections::{HashMap, HashSet};
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
    /// Whether this pane has a Codex process hint (used for disambiguation).
    pub has_codex_hint: bool,
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

/// Build a cwd → best-matching pane map for thread/list correlation.
///
/// When multiple panes share the same cwd, prefer the one with `has_codex_hint`.
/// If still tied, the first one wins (stable ordering from tmux list-panes).
fn build_cwd_pane_map(pane_cwds: &[PaneCwdInfo]) -> HashMap<String, PaneCwdInfo> {
    let mut map: HashMap<String, PaneCwdInfo> = HashMap::new();
    for info in pane_cwds {
        match map.get(&info.cwd) {
            Some(existing) if existing.has_codex_hint && !info.has_codex_hint => {
                // Existing pane has codex hint, keep it
            }
            _ => {
                // New entry, or current pane is better (has codex hint or first)
                map.insert(info.cwd.clone(), info.clone());
            }
        }
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

        // Build cwd → pane mapping for correlation.
        // Unique cwds to query; when multiple panes share a cwd, prefer the one
        // with a Codex process_hint (set by the caller).
        let cwd_pane_map = build_cwd_pane_map(pane_cwds);

        // Step 1: Per-cwd queries for pane correlation (threads get pane_id set).
        // Prioritize codex candidates and cap the number of requests per tick.
        let mut query_plan: Vec<PaneCwdInfo> = cwd_pane_map
            .into_values()
            .filter(|p| !p.cwd.is_empty())
            .collect();
        query_plan.sort_by(|a, b| {
            b.has_codex_hint
                .cmp(&a.has_codex_hint)
                .then_with(|| a.pane_id.cmp(&b.pane_id))
        });
        if query_plan.len() > MAX_CWD_QUERIES_PER_TICK {
            tracing::debug!(
                "codex thread/list cwd queries capped: {} -> {}",
                query_plan.len(),
                MAX_CWD_QUERIES_PER_TICK
            );
        }
        for pane_info in query_plan.into_iter().take(MAX_CWD_QUERIES_PER_TICK) {
            match self
                .send_thread_list_timed(Some(&pane_info.cwd), THREAD_LIST_REQUEST_TIMEOUT)
                .await
            {
                Ok(response) => {
                    self.process_thread_list_response(
                        &response,
                        Some(&pane_info),
                        now,
                        &mut events,
                    );
                }
                Err(e) => {
                    tracing::debug!(
                        "codex thread/list cwd query failed (cwd={}): {e}",
                        pane_info.cwd
                    );
                }
            }
        }

        // Step 2: Global query to catch threads at cwds that don't match any pane.
        // For known threads, cached pane bindings are reused.
        // Threads already seen in Step 1 are deduplicated by last_thread_states.
        match self
            .send_thread_list_timed(None, THREAD_LIST_REQUEST_TIMEOUT)
            .await
        {
            Ok(response) => {
                self.process_thread_list_response(&response, None, now, &mut events);
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
    /// If `pane_info` is provided, sets pane_id/generation/birth_ts on events.
    fn process_thread_list_response(
        &mut self,
        response: &serde_json::Value,
        pane_info: Option<&PaneCwdInfo>,
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

        for thread in threads {
            let thread_id = thread["id"].as_str().unwrap_or("");
            // Status is an object { type: "idle" } per the API reference.
            // However, the real App Server (v0.104.0+) may omit `status` from
            // `thread/list` results — it's only guaranteed in `thread/status/changed`
            // notifications and `thread/read`. Default to "idle" when absent:
            // a listed thread is at least available/loaded.
            let status = thread
                .get("status")
                .and_then(|s| s.get("type"))
                .and_then(|t| t.as_str())
                .unwrap_or("idle");

            // Skip notLoaded threads — they're historical (on disk, not in memory)
            // and don't represent active agents. See codex-appserver-api-reference.md.
            if status == "notLoaded" {
                continue;
            }

            if thread_id.is_empty() {
                continue;
            }

            let observed_binding = pane_info.map(|p| ThreadPaneBinding {
                pane_id: p.pane_id.clone(),
                pane_generation: p.generation,
                pane_birth_ts: p.birth_ts,
            });
            if let Some(binding) = observed_binding.clone() {
                self.thread_pane_bindings
                    .insert(thread_id.to_string(), binding);
            }

            // Keep existing pane association when current observation is global (pane-less).
            let effective_pane_id = observed_binding
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
                    None => false, // first discovery = real event
                    Some(prev) => {
                        prev.status == status && prev.pane_id == effective_pane_id
                    }
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
                    observed_binding.or_else(|| self.thread_pane_bindings.get(thread_id).cloned());

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

    // ── T-119: cwd correlation tests ─────────────────────────────────

    #[test]
    fn build_cwd_pane_map_single_pane() {
        let infos = vec![PaneCwdInfo {
            pane_id: "%0".to_string(),
            cwd: "/home/user/project".to_string(),
            generation: Some(1),
            birth_ts: None,
            has_codex_hint: false,
        }];
        let map = build_cwd_pane_map(&infos);
        assert_eq!(map.len(), 1);
        assert_eq!(map["/home/user/project"].pane_id, "%0");
    }

    #[test]
    fn build_cwd_pane_map_disambiguates_by_codex_hint() {
        let now = Utc::now();
        let infos = vec![
            PaneCwdInfo {
                pane_id: "%0".to_string(),
                cwd: "/home/user/project".to_string(),
                generation: Some(1),
                birth_ts: Some(now),
                has_codex_hint: false,
            },
            PaneCwdInfo {
                pane_id: "%1".to_string(),
                cwd: "/home/user/project".to_string(),
                generation: Some(2),
                birth_ts: Some(now),
                has_codex_hint: true,
            },
        ];
        let map = build_cwd_pane_map(&infos);
        assert_eq!(map.len(), 1);
        assert_eq!(
            map["/home/user/project"].pane_id, "%1",
            "pane with codex hint should win"
        );
    }

    #[test]
    fn build_cwd_pane_map_different_cwds() {
        let infos = vec![
            PaneCwdInfo {
                pane_id: "%0".to_string(),
                cwd: "/project-a".to_string(),
                generation: None,
                birth_ts: None,
                has_codex_hint: false,
            },
            PaneCwdInfo {
                pane_id: "%1".to_string(),
                cwd: "/project-b".to_string(),
                generation: None,
                birth_ts: None,
                has_codex_hint: false,
            },
        ];
        let map = build_cwd_pane_map(&infos);
        assert_eq!(map.len(), 2);
        assert_eq!(map["/project-a"].pane_id, "%0");
        assert_eq!(map["/project-b"].pane_id, "%1");
    }

    #[test]
    fn build_cwd_pane_map_codex_hint_wins_regardless_of_order() {
        // Codex hint pane listed first
        let infos = vec![
            PaneCwdInfo {
                pane_id: "%1".to_string(),
                cwd: "/shared".to_string(),
                generation: Some(1),
                birth_ts: None,
                has_codex_hint: true,
            },
            PaneCwdInfo {
                pane_id: "%0".to_string(),
                cwd: "/shared".to_string(),
                generation: Some(1),
                birth_ts: None,
                has_codex_hint: false,
            },
        ];
        let map = build_cwd_pane_map(&infos);
        assert_eq!(
            map["/shared"].pane_id, "%1",
            "codex hint pane should win even when listed first"
        );
    }
}

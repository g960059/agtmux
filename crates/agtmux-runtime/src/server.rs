//! UDS JSON-RPC server: minimal hand-rolled implementation.
//! Connection-per-request, newline-delimited JSON.

use std::sync::Arc;

use agtmux_core_v5::sync_v3::{SyncV3CursorV3, UiBootstrapV3, UiChangesV3};
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::Mutex;

use agtmux_core_v5::title::{TitleInput, resolve_title};
use agtmux_core_v5::types::{EvidenceMode, PanePresence};

use crate::poll_loop::DaemonState;
use crate::sync_v2_compat::{build_ui_bootstrap_v2, build_ui_changes_v2, parse_replay_cursor};

const REPLAY_HEALTHY_LAG_WINDOW: u64 = 10;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
enum UiHealthStatus {
    Ok,
    Degraded,
    Unavailable,
}

#[derive(Debug, Clone, Serialize)]
struct UiComponentHealth {
    status: UiHealthStatus,
    detail: Option<String>,
    last_updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, Serialize)]
struct UiReplayHealth {
    status: UiHealthStatus,
    current_epoch: Option<u64>,
    cursor_seq: Option<u64>,
    head_seq: Option<u64>,
    lag: Option<u64>,
    last_resync_reason: Option<String>,
    last_resync_at: Option<chrono::DateTime<chrono::Utc>>,
    detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct UiFocusHealth {
    status: UiHealthStatus,
    focused_pane_id: Option<String>,
    mismatch_count: Option<u64>,
    last_sync_at: Option<chrono::DateTime<chrono::Utc>>,
    detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct UiHealthV1 {
    generated_at: chrono::DateTime<chrono::Utc>,
    runtime: UiComponentHealth,
    replay: UiReplayHealth,
    overlay: UiComponentHealth,
    focus: UiFocusHealth,
}

/// Cached pane snapshot shared between poll loop (writer) and UDS server (reader).
#[derive(Debug, Clone)]
pub struct PaneCacheSnapshot {
    pub panes: serde_json::Value,
    pub inventory_updated_at: chrono::DateTime<chrono::Utc>,
    pub metadata_stale: bool,
    pub metadata_last_success_at: Option<chrono::DateTime<chrono::Utc>>,
    pub metadata_failure_streak: u32,
    pub metadata_backoff_until: Option<chrono::DateTime<chrono::Utc>>,
    pub metadata_last_error: Option<String>,
}

impl Default for PaneCacheSnapshot {
    fn default() -> Self {
        Self {
            panes: serde_json::Value::Array(Vec::new()),
            inventory_updated_at: chrono::Utc::now(),
            metadata_stale: false,
            metadata_last_success_at: None,
            metadata_failure_streak: 0,
            metadata_backoff_until: None,
            metadata_last_error: None,
        }
    }
}

pub type SharedPaneCache = Arc<std::sync::RwLock<PaneCacheSnapshot>>;

pub fn new_pane_cache() -> SharedPaneCache {
    Arc::new(std::sync::RwLock::new(PaneCacheSnapshot::default()))
}

pub(crate) fn refresh_pane_cache(
    cache: &SharedPaneCache,
    state: &DaemonState,
    inventory_updated_at: chrono::DateTime<chrono::Utc>,
) {
    let panes = build_pane_list(state);
    if let Ok(mut snapshot) = cache.write() {
        snapshot.panes = panes;
        snapshot.inventory_updated_at = inventory_updated_at;
        snapshot.metadata_stale = state.metadata_stale;
        snapshot.metadata_last_success_at = state.metadata_last_success_at;
        snapshot.metadata_failure_streak = state.metadata_failure_streak;
        snapshot.metadata_backoff_until = state.metadata_backoff_until;
        snapshot.metadata_last_error = state.metadata_last_error.clone();
    }
}

fn read_cached_panes(cache: &SharedPaneCache) -> serde_json::Value {
    cache
        .read()
        .map(|snapshot| snapshot.panes.clone())
        .unwrap_or_else(|_| serde_json::Value::Array(Vec::new()))
}

fn read_cached_snapshot(cache: &SharedPaneCache) -> serde_json::Value {
    match cache.read() {
        Ok(snapshot) => serde_json::json!({
            "panes": snapshot.panes.clone(),
            "inventory_updated_at": snapshot.inventory_updated_at,
            "metadata": {
                "stale": snapshot.metadata_stale,
                "last_success_at": snapshot.metadata_last_success_at,
                "failure_streak": snapshot.metadata_failure_streak,
                "backoff_until": snapshot.metadata_backoff_until,
                "last_error": snapshot.metadata_last_error.clone(),
            },
        }),
        Err(_) => serde_json::json!({
            "panes": [],
            "inventory_updated_at": chrono::Utc::now(),
            "metadata": {
                "stale": true,
                "last_success_at": null,
                "failure_streak": 0,
                "backoff_until": null,
                "last_error": "cache unavailable",
            },
        }),
    }
}

/// Run the UDS JSON-RPC server.
pub async fn run_server(
    socket_path: &str,
    state: Arc<Mutex<DaemonState>>,
    pane_cache: SharedPaneCache,
) -> anyhow::Result<()> {
    // Create socket directory with mode 0700
    let socket_dir = std::path::Path::new(socket_path)
        .parent()
        .ok_or_else(|| anyhow::anyhow!("invalid socket path"))?;

    std::fs::create_dir_all(socket_dir)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(socket_dir, std::fs::Permissions::from_mode(0o700))?;
    }

    // Check for stale or live socket
    if std::path::Path::new(socket_path).exists() {
        match tokio::net::UnixStream::connect(socket_path).await {
            Err(_) => {
                // Stale socket — remove and continue
                std::fs::remove_file(socket_path)?;
                tracing::info!("removed stale socket at {socket_path}");
            }
            Ok(mut stream) => {
                // Live daemon: query its PID then send SIGTERM to replace it
                let request = "{\"method\":\"daemon.info\"}\n";
                let old_pid: Option<u32> = if stream.write_all(request.as_bytes()).await.is_ok() {
                    let mut reader = BufReader::new(&mut stream);
                    let mut line = String::new();
                    match tokio::time::timeout(
                        std::time::Duration::from_secs(2),
                        reader.read_line(&mut line),
                    )
                    .await
                    {
                        Ok(Ok(_)) => serde_json::from_str::<serde_json::Value>(&line)
                            .ok()
                            .and_then(|v| v["result"]["pid"].as_u64())
                            .and_then(|n| u32::try_from(n).ok()),
                        _ => None,
                    }
                } else {
                    None
                };
                drop(stream);

                if let Some(pid) = old_pid {
                    tracing::info!("replacing existing daemon (pid={pid})");
                    let _ = std::process::Command::new("kill")
                        .args(["-TERM", &pid.to_string()])
                        .status();
                    // Wait up to 3 s for the old daemon to exit and remove its socket
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
                    while std::path::Path::new(socket_path).exists()
                        && std::time::Instant::now() < deadline
                    {
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                } else {
                    tracing::warn!(
                        "could not query PID from existing daemon; forcing socket removal"
                    );
                }
                // Remove socket if still present (old daemon didn't clean up in time)
                if std::path::Path::new(socket_path).exists() {
                    std::fs::remove_file(socket_path)?;
                }
            }
        }
    }

    let listener = UnixListener::bind(socket_path)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))?;
    }

    tracing::info!("UDS server listening on {socket_path}");

    loop {
        let (stream, _) = listener.accept().await?;
        let state = Arc::clone(&state);
        let pane_cache = Arc::clone(&pane_cache);
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, state, pane_cache).await {
                tracing::debug!("connection error: {e}");
            }
        });
    }
}

async fn handle_connection(
    stream: tokio::net::UnixStream,
    state: Arc<Mutex<DaemonState>>,
    pane_cache: SharedPaneCache,
) -> anyhow::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    reader.read_line(&mut line).await?;

    let request: serde_json::Value = serde_json::from_str(line.trim())?;
    let method = request["method"].as_str().unwrap_or("");
    let id = request["id"].clone();

    let result = match method {
        "list_panes" => read_cached_panes(&pane_cache),
        "list_panes_snapshot" => read_cached_snapshot(&pane_cache),
        "list_sessions" => {
            let st = state.lock().await;
            let sessions = st.daemon.list_sessions();
            serde_json::to_value(sessions)?
        }
        "list_source_health" => {
            let st = state.lock().await;
            let health = st.gateway.list_source_health();
            serde_json::to_value(health)?
        }
        "ui.bootstrap.v2" => {
            let mut st = state.lock().await;
            let replay_cursor = st.daemon.replay_cursor();
            st.daemon.acknowledge_replay_cursor(replay_cursor);
            build_ui_bootstrap_v2(&st)
        }
        "ui.bootstrap.v3" => {
            let mut st = state.lock().await;
            build_ui_bootstrap_v3(&mut st)
        }
        "ui.changes.v3" => {
            let params = &request["params"];
            let cursor = parse_sync_v3_cursor(&params["cursor"]);
            let limit = params["limit"]
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .unwrap_or(100)
                .clamp(1, 1000);
            let mut st = state.lock().await;
            build_ui_changes_v3(&mut st, cursor, limit)
        }
        "ui.changes.v2" => {
            let params = &request["params"];
            let cursor = parse_replay_cursor(&params["cursor"]);
            let limit = params["limit"]
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .unwrap_or(100)
                .clamp(1, 1000);
            let mut st = state.lock().await;
            build_ui_changes_v2(&mut st, cursor, limit)
        }
        "ui.health.v1" => {
            let st = state.lock().await;
            build_ui_health_v1(&st)
        }
        "state_changed" => {
            let params = &request["params"];
            let since_version = params["since_version"].as_u64().unwrap_or(0);
            let st = state.lock().await;
            build_state_changed(&st, since_version)
        }
        "summary_changed" => {
            let params = &request["params"];
            let since_version = params["since_version"].as_u64().unwrap_or(0);
            let st = state.lock().await;
            build_summary_changed(&st, since_version)
        }
        "latency_status" => {
            let st = state.lock().await;
            build_latency_status(&st)
        }
        "source.hello" => {
            let params = &request["params"];
            let source_id = params["source_id"].as_str().unwrap_or("").to_string();
            let source_kind_str = params["source_kind"].as_str().unwrap_or("");
            let protocol_version = params["protocol_version"].as_u64().unwrap_or(0) as u32;
            let socket_path = params["socket_path"].as_str().map(String::from);

            let source_kind = match source_kind_str {
                "poller" => agtmux_core_v5::types::SourceKind::Poller,
                "claude_hooks" => agtmux_core_v5::types::SourceKind::ClaudeHooks,
                "claude_jsonl" => agtmux_core_v5::types::SourceKind::ClaudeJsonl,
                _ => {
                    let error_response = serde_json::json!({
                        "jsonrpc": "2.0",
                        "error": {"code": -32602, "message": format!("unknown source_kind: {source_kind_str:?}")},
                        "id": id,
                    });
                    let mut resp = serde_json::to_string(&error_response)?;
                    resp.push('\n');
                    writer.write_all(resp.as_bytes()).await?;
                    return Ok(());
                }
            };

            let req = agtmux_gateway::source_registry::HelloRequest {
                source_id,
                source_kind,
                protocol_version,
                socket_path,
            };
            let now_ms = chrono::Utc::now().timestamp_millis() as u64;
            let mut st = state.lock().await;
            let resp = st.source_registry.handle_hello(req, now_ms);
            match resp {
                agtmux_gateway::source_registry::HelloResponse::Accepted { source_id } => {
                    serde_json::json!({"status": "accepted", "source_id": source_id})
                }
                agtmux_gateway::source_registry::HelloResponse::Rejected { reason } => {
                    serde_json::json!({"status": "rejected", "reason": reason})
                }
            }
        }
        "source.heartbeat" => {
            let params = &request["params"];
            let source_id = params["source_id"].as_str().unwrap_or("");
            let now_ms = chrono::Utc::now().timestamp_millis() as u64;
            let mut st = state.lock().await;
            let acked = st.source_registry.heartbeat(source_id, now_ms);
            serde_json::json!({"acknowledged": acked})
        }
        "list_source_registry" => {
            let st = state.lock().await;
            let entries: Vec<serde_json::Value> = st
                .source_registry
                .list()
                .iter()
                .map(|e| serde_json::to_value(e).unwrap_or_default())
                .collect();
            serde_json::Value::Array(entries)
        }
        "daemon.info" => {
            let st = state.lock().await;
            serde_json::json!({
                "nonce": st.trust_guard.nonce(),
                "version": env!("CARGO_PKG_VERSION"),
                "pid": std::process::id(),
            })
        }
        "source.ingest" => {
            let params = &request["params"];
            let source_kind = params["source_kind"].as_str().unwrap_or("");

            // T-115: Warn-only admission gate (Phase 1)
            // Check trust guard if source_id/nonce are provided
            {
                let source_id = params["source_id"].as_str().unwrap_or(source_kind);
                let nonce = params["nonce"].as_str().unwrap_or("");
                let st = state.lock().await;
                // Use daemon's own UID as peer_uid (same-process, warn-only)
                let peer_uid = st.trust_guard.expected_uid();
                if !nonce.is_empty() {
                    let result = st.trust_guard.check_admission(peer_uid, source_id, nonce);
                    if let agtmux_gateway::trust_guard::AdmissionResult::Rejected(reason) = result {
                        tracing::warn!(
                            "source.ingest admission warning: {reason} (warn-only, processing continues)"
                        );
                    }
                } else if !st.trust_guard.is_registered(source_id) {
                    tracing::warn!("source.ingest: unregistered source_id={source_id} (warn-only)");
                }
            }
            match source_kind {
                "claude_hooks" => {
                    match serde_json::from_value::<
                        agtmux_source_claude_hooks::translate::ClaudeHookEvent,
                    >(params["event"].clone())
                    {
                        Ok(event) => {
                            let mut st = state.lock().await;
                            st.claude_source.ingest(event);
                            serde_json::json!({"status": "ok"})
                        }
                        Err(e) => {
                            let error_response = serde_json::json!({
                                "jsonrpc": "2.0",
                                "error": {"code": -32602, "message": format!("invalid event: {e}")},
                                "id": id,
                            });
                            let mut resp = serde_json::to_string(&error_response)?;
                            resp.push('\n');
                            writer.write_all(resp.as_bytes()).await?;
                            return Ok(());
                        }
                    }
                }
                _ => {
                    let error_response = serde_json::json!({
                        "jsonrpc": "2.0",
                        "error": {"code": -32602, "message": format!("unknown source_kind: {source_kind:?}")},
                        "id": id,
                    });
                    let mut resp = serde_json::to_string(&error_response)?;
                    resp.push('\n');
                    writer.write_all(resp.as_bytes()).await?;
                    return Ok(());
                }
            }
        }
        _ => {
            let error_response = serde_json::json!({
                "jsonrpc": "2.0",
                "error": {"code": -32601, "message": "method not found"},
                "id": id,
            });
            let mut resp = serde_json::to_string(&error_response)?;
            resp.push('\n');
            writer.write_all(resp.as_bytes()).await?;
            return Ok(());
        }
    };

    let response = serde_json::json!({
        "jsonrpc": "2.0",
        "result": result,
        "id": id,
    });
    let mut resp = serde_json::to_string(&response)?;
    resp.push('\n');
    writer.write_all(resp.as_bytes()).await?;

    Ok(())
}

/// Build a combined pane list: managed panes from daemon + unmanaged panes from tmux.
pub(crate) fn build_pane_list(state: &DaemonState) -> serde_json::Value {
    let managed_panes = state.daemon.list_panes();
    let managed_ids: std::collections::HashSet<&str> = managed_panes
        .iter()
        .map(|p| p.pane_instance_id.pane_id.as_str())
        .collect();

    let mut result: Vec<serde_json::Value> = Vec::new();

    // Add managed panes
    for pane in &managed_panes {
        let tmux_info = state
            .last_panes
            .iter()
            .find(|p| p.pane_id == pane.pane_instance_id.pane_id);

        // Resolve display title (FR-015/FR-016)
        let pane_title = tmux_info.map_or(String::new(), |t| t.pane_title.clone());
        let title_input = TitleInput {
            pane_title,
            provider: pane.provider,
            deterministic_session_key: if pane.evidence_mode == EvidenceMode::Deterministic {
                Some(pane.session_key.clone())
            } else {
                None
            },
            handshake_confirmed: false, // Post-MVP: needs T-042 handshake tracking
            canonical_session_name: None, // Post-MVP: needs per-provider session file parser
            is_managed: true,
        };
        let title_decision = resolve_title(&title_input);

        result.push(serde_json::json!({
            "pane_id": pane.pane_instance_id.pane_id,
            "presence": "managed",
            "evidence_mode": pane.evidence_mode,
            "signature_class": pane.signature_class,
            "signature_reason": pane.signature_reason,
            "signature_confidence": pane.signature_confidence,
            "signature_inputs": {
                "provider_hint": pane.signature_inputs.provider_hint,
                "cmd_match": pane.signature_inputs.cmd_match,
                "poller_match": pane.signature_inputs.poller_match,
                "title_match": pane.signature_inputs.title_match,
            },
            "activity_state": format!("{:?}", pane.activity_state),
            "provider": pane.provider.map(|p| p.as_str()),
            "conversation_title": state.conversation_titles.get(&pane.session_key),
            "title": title_decision.title,
            "title_quality": format!("{:?}", title_decision.quality),
            "session_id": tmux_info.map(|t| &t.session_id),
            "session_name": tmux_info.map(|t| &t.session_name),
            "window_id": tmux_info.map(|t| &t.window_id),
            "window_name": tmux_info.map(|t| &t.window_name),
            "current_cmd": tmux_info.map(|t| &t.current_cmd),
            "current_path": tmux_info.map(|t| &t.current_path),
            "git_branch": serde_json::Value::Null,
            "updated_at": pane.updated_at,
            "metadata_stale": state.metadata_stale,
            "metadata_last_success_at": state.metadata_last_success_at,
            "metadata_failure_streak": state.metadata_failure_streak,
            "metadata_backoff_until": state.metadata_backoff_until,
            "metadata_last_error": state.metadata_last_error.clone(),
        }));
    }

    // Add unmanaged panes
    for tmux_pane in &state.last_panes {
        if !managed_ids.contains(tmux_pane.pane_id.as_str()) {
            let title_input = TitleInput {
                pane_title: tmux_pane.pane_title.clone(),
                provider: None,
                deterministic_session_key: None,
                handshake_confirmed: false,
                canonical_session_name: None,
                is_managed: false,
            };
            let title_decision = resolve_title(&title_input);

            result.push(serde_json::json!({
                "pane_id": tmux_pane.pane_id,
                "presence": PanePresence::Unmanaged,
                "title": title_decision.title,
                "title_quality": format!("{:?}", title_decision.quality),
                "session_id": tmux_pane.session_id,
                "session_name": tmux_pane.session_name,
                "window_id": tmux_pane.window_id,
                "window_name": tmux_pane.window_name,
                "current_cmd": tmux_pane.current_cmd,
                "current_path": tmux_pane.current_path,
                "git_branch": serde_json::Value::Null,
                "metadata_stale": state.metadata_stale,
                "metadata_last_success_at": state.metadata_last_success_at,
                "metadata_failure_streak": state.metadata_failure_streak,
                "metadata_backoff_until": state.metadata_backoff_until,
                "metadata_last_error": state.metadata_last_error.clone(),
            }));
        }
    }

    serde_json::Value::Array(result)
}

/// Build a `latency_status` response from cached evaluation (Codex F4: read-only, no evaluate()).
pub(crate) fn build_latency_status(state: &DaemonState) -> serde_json::Value {
    use agtmux_gateway::latency_window::LatencyEvaluation;

    match &state.last_latency_eval {
        Some(LatencyEvaluation::InsufficientData {
            sample_count,
            min_required,
        }) => serde_json::json!({
            "status": "insufficient_data",
            "sample_count": sample_count,
            "min_required": min_required,
            "p95_ms": null,
            "consecutive_breaches": 0,
        }),
        Some(LatencyEvaluation::Healthy { p95_ms }) => serde_json::json!({
            "status": "healthy",
            "p95_ms": p95_ms,
            "consecutive_breaches": 0,
            "sample_count": state.latency_window.sample_count(),
        }),
        Some(LatencyEvaluation::Breached {
            p95_ms,
            consecutive,
            threshold,
        }) => serde_json::json!({
            "status": "breached",
            "p95_ms": p95_ms,
            "consecutive_breaches": consecutive,
            "breach_threshold": threshold,
            "sample_count": state.latency_window.sample_count(),
        }),
        Some(LatencyEvaluation::Degraded {
            p95_ms,
            consecutive,
        }) => serde_json::json!({
            "status": "degraded",
            "p95_ms": p95_ms,
            "consecutive_breaches": consecutive,
            "sample_count": state.latency_window.sample_count(),
        }),
        None => serde_json::json!({
            "status": "not_started",
            "sample_count": 0,
            "p95_ms": null,
            "consecutive_breaches": 0,
        }),
    }
}

fn parse_sync_v3_cursor(value: &serde_json::Value) -> Option<SyncV3CursorV3> {
    Some(SyncV3CursorV3 {
        seq: value.get("seq")?.as_u64()?,
    })
}

fn reconcile_sync_v3(state: &mut DaemonState, now: chrono::DateTime<chrono::Utc>) {
    let managed = state
        .daemon
        .list_panes()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let managed_refs = managed.iter().collect::<Vec<_>>();
    let last_panes = state.last_panes.clone();
    let generation_tracker = state.generation_tracker.clone();
    state
        .sync_v3
        .reconcile(&managed_refs, &last_panes, &generation_tracker, now);
}

pub(crate) fn build_ui_bootstrap_v3(state: &mut DaemonState) -> serde_json::Value {
    let now = chrono::Utc::now();
    reconcile_sync_v3(state, now);
    let payload = state.sync_v3.build_bootstrap(now);

    if let Err(err) = payload.validate() {
        tracing::warn!("ui.bootstrap.v3 validation error: {err}");
        let mut fallback = UiBootstrapV3::new(payload.generated_at, Vec::new());
        fallback.replay_cursor = Some(state.sync_v3.current_cursor());
        return serde_json::to_value(fallback).unwrap_or_else(|_| {
            serde_json::json!({
                "version": agtmux_core_v5::sync_v3::UI_BOOTSTRAP_V3_VERSION,
                "generated_at": chrono::Utc::now(),
                "replay_cursor": {
                    "seq": state.sync_v3.current_cursor().seq,
                },
                "panes": [],
            })
        });
    }

    serde_json::to_value(payload).unwrap_or_else(|_| {
        serde_json::json!({
            "version": agtmux_core_v5::sync_v3::UI_BOOTSTRAP_V3_VERSION,
            "generated_at": chrono::Utc::now(),
            "replay_cursor": {
                "seq": state.sync_v3.current_cursor().seq,
            },
            "panes": [],
        })
    })
}

pub(crate) fn build_ui_changes_v3(
    state: &mut DaemonState,
    cursor: Option<SyncV3CursorV3>,
    limit: usize,
) -> serde_json::Value {
    reconcile_sync_v3(state, chrono::Utc::now());
    let payload = state.sync_v3.build_changes(cursor, limit);

    if let Err(err) = payload.validate() {
        tracing::warn!("ui.changes.v3 validation error: {err}");
        let fallback = UiChangesV3::resync_required(
            state.sync_v3.current_cursor().seq,
            "invalid_server_payload",
        );
        return serde_json::to_value(fallback).unwrap_or_else(|_| {
            serde_json::json!({
                "version": agtmux_core_v5::sync_v3::UI_BOOTSTRAP_V3_VERSION,
                "changes": [],
                "resync_required": {
                    "latest_snapshot_seq": state.sync_v3.current_cursor().seq,
                    "reason": "invalid_server_payload",
                },
            })
        });
    }

    serde_json::to_value(payload).unwrap_or_else(|_| {
        serde_json::json!({
            "version": agtmux_core_v5::sync_v3::UI_BOOTSTRAP_V3_VERSION,
            "changes": [],
            "resync_required": {
                "latest_snapshot_seq": state.sync_v3.current_cursor().seq,
                "reason": "serialization_failed",
            },
        })
    })
}

pub(crate) fn build_ui_health_v1(state: &DaemonState) -> serde_json::Value {
    let replay = state.daemon.replay_health_snapshot();
    let runtime_status = if state.runtime_last_error.is_some() && state.runtime_last_ok_at.is_none()
    {
        UiHealthStatus::Unavailable
    } else if state.runtime_last_error.is_some() {
        UiHealthStatus::Degraded
    } else {
        UiHealthStatus::Ok
    };
    let overlay_status = if !state.metadata_stale {
        UiHealthStatus::Ok
    } else if state.metadata_last_success_at.is_some() {
        UiHealthStatus::Degraded
    } else {
        UiHealthStatus::Unavailable
    };
    let replay_status =
        if replay.last_resync_reason.is_some() || replay.lag > REPLAY_HEALTHY_LAG_WINDOW {
            UiHealthStatus::Degraded
        } else {
            UiHealthStatus::Ok
        };
    let focus_status = if state.last_panes.is_empty() {
        UiHealthStatus::Unavailable
    } else if state.focus_mismatch_count > 0 {
        UiHealthStatus::Degraded
    } else if state.focused_pane_id.is_some() {
        UiHealthStatus::Ok
    } else {
        UiHealthStatus::Unavailable
    };

    let replay_detail = if let Some(reason) = replay.last_resync_reason {
        Some(format!("resync required: {reason}"))
    } else if replay.lag > REPLAY_HEALTHY_LAG_WINDOW {
        Some("replay lag above healthy window".to_string())
    } else {
        None
    };
    let focus_detail = match focus_status {
        UiHealthStatus::Degraded => Some(format!(
            "focus mismatch across {} window(s)",
            state.focus_mismatch_count
        )),
        UiHealthStatus::Unavailable if state.last_panes.is_empty() => {
            Some("no panes in inventory".to_string())
        }
        UiHealthStatus::Unavailable => Some("no active pane detected".to_string()),
        UiHealthStatus::Ok => None,
    };

    let payload = UiHealthV1 {
        generated_at: chrono::Utc::now(),
        runtime: UiComponentHealth {
            status: runtime_status,
            detail: state.runtime_last_error.clone(),
            last_updated_at: state.runtime_last_ok_at.or(Some(state.runtime_started_at)),
        },
        replay: UiReplayHealth {
            status: replay_status,
            current_epoch: Some(replay.current_epoch),
            cursor_seq: Some(replay.cursor_seq),
            head_seq: Some(replay.head_seq),
            lag: Some(replay.lag),
            last_resync_reason: replay.last_resync_reason.map(str::to_string),
            last_resync_at: replay.last_resync_at,
            detail: replay_detail,
        },
        overlay: UiComponentHealth {
            status: overlay_status,
            detail: if state.metadata_stale {
                state
                    .metadata_last_error
                    .clone()
                    .or_else(|| Some("metadata overlay stale".to_string()))
            } else {
                None
            },
            last_updated_at: state.metadata_last_success_at,
        },
        focus: UiFocusHealth {
            status: focus_status,
            focused_pane_id: state.focused_pane_id.clone(),
            mismatch_count: Some(state.focus_mismatch_count),
            last_sync_at: state.focus_last_sync_at,
            detail: focus_detail,
        },
    };

    serde_json::to_value(payload).unwrap_or_else(|_| {
        serde_json::json!({
            "generated_at": chrono::Utc::now(),
            "runtime": {
                "status": "unavailable",
                "detail": "health serialization failed",
                "last_updated_at": null,
            },
            "replay": {
                "status": "unavailable",
                "current_epoch": null,
                "cursor_seq": null,
                "head_seq": null,
                "lag": null,
                "last_resync_reason": null,
                "last_resync_at": null,
                "detail": "health serialization failed",
            },
            "overlay": {
                "status": "unavailable",
                "detail": "health serialization failed",
                "last_updated_at": null,
            },
            "focus": {
                "status": "unavailable",
                "focused_pane_id": null,
                "mismatch_count": null,
                "last_sync_at": null,
                "detail": "health serialization failed",
            },
        })
    })
}

/// Build a `state_changed` response: changes since a given version with full state.
///
/// Returns pane/session state for each change, plus the current version for
/// the client to use in subsequent `state_changed` calls.
pub(crate) fn build_state_changed(state: &DaemonState, since_version: u64) -> serde_json::Value {
    let changes = state.daemon.changes_since(since_version);
    let current_version = state.daemon.version();

    let entries = changes
        .iter()
        .map(|change| {
            let mut entry = serde_json::json!({
                "version": change.version,
                "session_key": change.session_key,
                "timestamp": change.timestamp,
            });

            if let Some(ref pane_id) = change.pane_id {
                entry["pane_id"] = serde_json::Value::String(pane_id.clone());
            }
            if let Some(ref pane_state) = change.pane_state {
                entry["pane_state"] = serde_json::json!({
                    "signature_class": pane_state.signature_class,
                    "evidence_mode": pane_state.evidence_mode,
                    "activity_state": format!("{:?}", pane_state.activity_state),
                    "provider": pane_state.provider.map(|p| p.as_str()),
                    "signature_confidence": pane_state.signature_confidence,
                });
            }
            if let Some(ref session_state) = change.session_state {
                entry["session_state"] = serde_json::json!({
                    "presence": session_state.presence,
                    "evidence_mode": session_state.evidence_mode,
                    "activity_state": format!("{:?}", session_state.activity_state),
                    "winner_tier": session_state.winner_tier,
                });
            }

            entry
        })
        .collect::<Vec<_>>();

    serde_json::json!({
        "changes": entries,
        "version": current_version,
    })
}

/// Build a `summary_changed` response: summary counts when there are changes.
pub(crate) fn build_summary_changed(state: &DaemonState, since_version: u64) -> serde_json::Value {
    let changes = state.daemon.changes_since(since_version);
    let current_version = state.daemon.version();

    let pane_changes = changes.iter().filter(|c| c.pane_id.is_some()).count();
    let session_changes = changes.iter().filter(|c| c.pane_id.is_none()).count();

    let managed_panes = state.daemon.list_panes();
    let managed_count = managed_panes.len();
    let total_panes = state.last_panes.len();
    let unmanaged_count = total_panes - managed_count.min(total_panes);

    let deterministic_count = managed_panes
        .iter()
        .filter(|p| p.evidence_mode == EvidenceMode::Deterministic)
        .count();
    let heuristic_count = managed_count - deterministic_count;

    serde_json::json!({
        "has_changes": !changes.is_empty(),
        "pane_changes": pane_changes,
        "session_changes": session_changes,
        "version": current_version,
        "summary": {
            "managed": managed_count,
            "unmanaged": unmanaged_count,
            "total": total_panes,
            "deterministic": deterministic_count,
            "heuristic": heuristic_count,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use agtmux_core_v5::types::{EvidenceTier, Provider, SourceEventV2, SourceKind};
    use agtmux_daemon_v5::projection::ReplayCursor;
    use agtmux_tmux_v5::TmuxPaneInfo;
    use chrono::Utc;

    fn make_state() -> DaemonState {
        DaemonState::new()
    }

    fn codex_v3_event(
        pane_id: &str,
        inner_type: &str,
        observed_at: chrono::DateTime<Utc>,
    ) -> SourceEventV2 {
        SourceEventV2 {
            event_id: format!("codex-{inner_type}-{}", observed_at.timestamp()),
            provider: Provider::Codex,
            source_kind: SourceKind::CodexJsonl,
            tier: EvidenceTier::Deterministic,
            observed_at,
            session_key: "thr-bootstrap-v3".to_string(),
            pane_id: Some(pane_id.to_string()),
            pane_generation: None,
            pane_birth_ts: None,
            source_event_id: None,
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

    fn tmux_pane(pane_id: &str, session: &str, cmd: &str) -> TmuxPaneInfo {
        TmuxPaneInfo {
            pane_id: pane_id.to_string(),
            session_name: session.to_string(),
            window_name: "dev".to_string(),
            current_cmd: cmd.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn build_pane_list_empty_state() {
        let state = make_state();
        let result = build_pane_list(&state);
        assert_eq!(result, serde_json::Value::Array(vec![]));
    }

    #[test]
    fn build_pane_list_all_unmanaged() {
        let mut state = make_state();
        state.last_panes = vec![
            tmux_pane("%0", "main", "zsh"),
            tmux_pane("%1", "main", "vim"),
        ];

        let result = build_pane_list(&state);
        let arr = result.as_array().expect("should be array");
        assert_eq!(arr.len(), 2, "both panes should appear");
        assert_eq!(arr[0]["pane_id"], "%0");
        assert_eq!(arr[0]["presence"], "unmanaged");
        assert_eq!(arr[1]["pane_id"], "%1");
        assert_eq!(arr[1]["presence"], "unmanaged");
    }

    #[test]
    fn build_pane_list_managed_and_unmanaged() {
        let mut state = make_state();
        // Create managed pane by ingesting events through the pipeline
        let now = Utc::now();
        let snapshot = agtmux_source_poller::source::PaneSnapshot {
            pane_id: "%0".to_string(),
            pane_title: "claude code".to_string(),
            current_cmd: "claude".to_string(),
            process_hint: Some("claude".to_string()),
            capture_lines: vec!["╭ Claude Code".to_string()],
            captured_at: now,
        };
        state.poller.poll_batch(&[snapshot]);
        let pull_req = agtmux_core_v5::types::PullEventsRequest {
            cursor: None,
            limit: 100,
        };
        let poller_resp = state.poller.pull_events(&pull_req, now);
        state
            .gateway
            .ingest_source_response(SourceKind::Poller, poller_resp);
        let gw_req = agtmux_core_v5::types::GatewayPullRequest {
            cursor: None,
            limit: 100,
        };
        let gw_resp = state.gateway.pull_events(&gw_req);
        state.daemon.apply_events(gw_resp.events, now);

        // Add tmux panes (both the managed one and an unmanaged one)
        state.last_panes = vec![
            tmux_pane("%0", "main", "claude"),
            tmux_pane("%1", "main", "zsh"),
        ];

        let result = build_pane_list(&state);
        let arr = result.as_array().expect("should be array");
        assert_eq!(arr.len(), 2, "managed + unmanaged");

        // Find managed pane
        let managed = arr.iter().find(|p| p["pane_id"] == "%0").expect("has %0");
        assert_eq!(managed["presence"], "managed");

        // Find unmanaged pane
        let unmanaged = arr.iter().find(|p| p["pane_id"] == "%1").expect("has %1");
        assert_eq!(unmanaged["presence"], "unmanaged");
    }

    #[test]
    fn build_pane_list_no_duplicate_for_managed() {
        let mut state = make_state();
        let now = Utc::now();
        let snapshot = agtmux_source_poller::source::PaneSnapshot {
            pane_id: "%0".to_string(),
            pane_title: "claude code".to_string(),
            current_cmd: "claude".to_string(),
            process_hint: Some("claude".to_string()),
            capture_lines: vec!["output".to_string()],
            captured_at: now,
        };
        state.poller.poll_batch(&[snapshot]);
        let pull_req = agtmux_core_v5::types::PullEventsRequest {
            cursor: None,
            limit: 100,
        };
        let poller_resp = state.poller.pull_events(&pull_req, now);
        state
            .gateway
            .ingest_source_response(SourceKind::Poller, poller_resp);
        let gw_req = agtmux_core_v5::types::GatewayPullRequest {
            cursor: None,
            limit: 100,
        };
        let gw_resp = state.gateway.pull_events(&gw_req);
        state.daemon.apply_events(gw_resp.events, now);

        // last_panes includes the same pane_id
        state.last_panes = vec![tmux_pane("%0", "main", "claude")];

        let result = build_pane_list(&state);
        let arr = result.as_array().expect("should be array");
        // Should NOT duplicate — managed pane already covers it
        assert_eq!(arr.len(), 1, "no duplicate for managed pane");
        assert_eq!(arr[0]["presence"], "managed");
    }

    #[test]
    fn build_pane_list_includes_signature_fields() {
        let mut state = make_state();
        let now = Utc::now();
        let snapshot = agtmux_source_poller::source::PaneSnapshot {
            pane_id: "%0".to_string(),
            pane_title: "claude code".to_string(),
            current_cmd: "claude".to_string(),
            process_hint: Some("claude".to_string()),
            capture_lines: vec!["\u{256D} Claude Code".to_string()],
            captured_at: now,
        };
        state.poller.poll_batch(&[snapshot]);
        let pull_req = agtmux_core_v5::types::PullEventsRequest {
            cursor: None,
            limit: 100,
        };
        let poller_resp = state.poller.pull_events(&pull_req, now);
        state
            .gateway
            .ingest_source_response(SourceKind::Poller, poller_resp);
        let gw_req = agtmux_core_v5::types::GatewayPullRequest {
            cursor: None,
            limit: 100,
        };
        let gw_resp = state.gateway.pull_events(&gw_req);
        state.daemon.apply_events(gw_resp.events, now);

        state.last_panes = vec![tmux_pane("%0", "main", "claude")];

        let result = build_pane_list(&state);
        let arr = result.as_array().expect("should be array");
        let managed = &arr[0];

        // FR-024: signature_reason and signature_inputs must be present
        assert!(
            managed.get("signature_reason").is_some(),
            "signature_reason must be in API response"
        );
        assert!(
            managed.get("signature_inputs").is_some(),
            "signature_inputs must be in API response"
        );

        // Verify signature_inputs structure
        let inputs = &managed["signature_inputs"];
        assert!(
            inputs.get("provider_hint").is_some(),
            "signature_inputs.provider_hint present"
        );
        assert!(
            inputs.get("cmd_match").is_some(),
            "signature_inputs.cmd_match present"
        );
        assert!(
            inputs.get("poller_match").is_some(),
            "signature_inputs.poller_match present"
        );
        assert!(
            inputs.get("title_match").is_some(),
            "signature_inputs.title_match present"
        );

        // Claude with process_hint=claude → provider_hint should be true
        assert_eq!(inputs["provider_hint"], true);
    }

    #[test]
    fn build_pane_list_includes_resolved_title() {
        let mut state = make_state();
        let now = Utc::now();
        let snapshot = agtmux_source_poller::source::PaneSnapshot {
            pane_id: "%0".to_string(),
            pane_title: "claude code".to_string(),
            current_cmd: "claude".to_string(),
            process_hint: Some("claude".to_string()),
            capture_lines: vec!["\u{256D} Claude Code".to_string()],
            captured_at: now,
        };
        state.poller.poll_batch(&[snapshot]);
        let pull_req = agtmux_core_v5::types::PullEventsRequest {
            cursor: None,
            limit: 100,
        };
        let poller_resp = state.poller.pull_events(&pull_req, now);
        state
            .gateway
            .ingest_source_response(SourceKind::Poller, poller_resp);
        let gw_req = agtmux_core_v5::types::GatewayPullRequest {
            cursor: None,
            limit: 100,
        };
        let gw_resp = state.gateway.pull_events(&gw_req);
        state.daemon.apply_events(gw_resp.events, now);

        // Use tmux_pane helper but need pane_title field
        let mut tmux = tmux_pane("%0", "main", "claude");
        tmux.pane_title = "claude code".to_string();
        state.last_panes = vec![tmux, tmux_pane("%1", "main", "zsh")];

        let result = build_pane_list(&state);
        let arr = result.as_array().expect("should be array");

        // Managed pane: title resolved via HeuristicTitle (provider detected, pane_title set)
        let managed = arr.iter().find(|p| p["pane_id"] == "%0").expect("has %0");
        assert!(
            managed.get("title").is_some(),
            "managed pane must have title field"
        );
        assert_eq!(managed["title"], "claude code");
        assert_eq!(managed["title_quality"], "HeuristicTitle");

        // Unmanaged pane: title resolved via Unmanaged fallback
        let unmanaged = arr.iter().find(|p| p["pane_id"] == "%1").expect("has %1");
        assert!(
            unmanaged.get("title").is_some(),
            "unmanaged pane must have title field"
        );
        assert_eq!(unmanaged["title_quality"], "Unmanaged");
    }

    /// Helper to create a managed state (pane ingested through pipeline).
    fn make_managed_state() -> DaemonState {
        let mut state = make_state();
        let now = Utc::now();
        let snapshot = agtmux_source_poller::source::PaneSnapshot {
            pane_id: "%0".to_string(),
            pane_title: "claude code".to_string(),
            current_cmd: "claude".to_string(),
            process_hint: Some("claude".to_string()),
            capture_lines: vec!["\u{256D} Claude Code".to_string()],
            captured_at: now,
        };
        state.poller.poll_batch(&[snapshot]);
        let pull_req = agtmux_core_v5::types::PullEventsRequest {
            cursor: None,
            limit: 100,
        };
        let poller_resp = state.poller.pull_events(&pull_req, now);
        state
            .gateway
            .ingest_source_response(SourceKind::Poller, poller_resp);
        let gw_req = agtmux_core_v5::types::GatewayPullRequest {
            cursor: None,
            limit: 100,
        };
        let gw_resp = state.gateway.pull_events(&gw_req);
        state.daemon.apply_events(gw_resp.events, now);
        state.last_panes = vec![tmux_pane("%0", "main", "claude")];
        state
    }

    /// Helper to create a deterministic-managed state (pane ingested through claude hooks).
    fn make_deterministic_state() -> DaemonState {
        let mut state = make_state();
        let now = Utc::now();
        // Ingest via claude hooks source (deterministic)
        use agtmux_source_claude_hooks::translate::ClaudeHookEvent;
        state.claude_source.ingest(ClaudeHookEvent {
            hook_id: "h-det-1".to_string(),
            hook_type: "tool_start".to_string(),
            session_id: "claude-det-sess".to_string(),
            timestamp: now,
            pane_id: Some("%0".to_string()),
            data: serde_json::json!({}),
        });
        let pull_req = agtmux_core_v5::types::PullEventsRequest {
            cursor: None,
            limit: 100,
        };
        let claude_resp = state.claude_source.pull_events(&pull_req, now);
        state
            .gateway
            .ingest_source_response(SourceKind::ClaudeHooks, claude_resp);
        let gw_req = agtmux_core_v5::types::GatewayPullRequest {
            cursor: None,
            limit: 100,
        };
        let gw_resp = state.gateway.pull_events(&gw_req);
        state.daemon.apply_events(gw_resp.events, now);
        state.last_panes = vec![tmux_pane("%0", "main", "claude")];
        state
    }

    fn make_bootstrap_v3_codex_completed_state() -> DaemonState {
        let mut state = make_state();
        let now = Utc::now();
        state.last_panes = vec![TmuxPaneInfo {
            pane_id: "%12".to_string(),
            session_id: "$5".to_string(),
            session_name: "workbench".to_string(),
            window_id: "@5".to_string(),
            window_name: "main".to_string(),
            current_cmd: "codex".to_string(),
            ..Default::default()
        }];
        state.generation_tracker.update(&["%12"], now);
        state.sync_v3.apply_events(
            &[codex_v3_event("%12", "task_complete", now)],
            &state.last_panes,
            &state.generation_tracker,
        );
        state
    }

    fn populate_sync_v2_replay(state: &mut DaemonState) {
        let now = Utc::now();
        let snapshot = agtmux_source_poller::source::PaneSnapshot {
            pane_id: "%0".to_string(),
            pane_title: "claude code".to_string(),
            current_cmd: "claude".to_string(),
            process_hint: Some("claude".to_string()),
            capture_lines: vec!["\u{256D} Claude Code".to_string()],
            captured_at: now,
        };
        state.poller.poll_batch(&[snapshot]);
        let pull_req = agtmux_core_v5::types::PullEventsRequest {
            cursor: None,
            limit: 100,
        };
        let poller_resp = state.poller.pull_events(&pull_req, now);
        state
            .gateway
            .ingest_source_response(SourceKind::Poller, poller_resp);
        let gw_req = agtmux_core_v5::types::GatewayPullRequest {
            cursor: None,
            limit: 100,
        };
        let gw_resp = state.gateway.pull_events(&gw_req);
        state.daemon.apply_events(gw_resp.events, now);
    }

    #[test]
    fn build_pane_list_deterministic_title_quality() {
        let state = make_deterministic_state();
        let result = build_pane_list(&state);
        let arr = result.as_array().expect("should be array");
        let managed = arr.iter().find(|p| p["pane_id"] == "%0").expect("has %0");

        assert_eq!(managed["presence"], "managed");
        assert_eq!(
            managed["title_quality"], "DeterministicBinding",
            "deterministic source pane should have DeterministicBinding quality"
        );
        // Title should be the session_key
        assert_eq!(managed["title"], "claude-det-sess");
    }

    #[test]
    fn summary_changed_includes_evidence_mode_counts() {
        let state = make_managed_state(); // poller-based = heuristic
        let result = build_summary_changed(&state, 0);
        assert_eq!(result["summary"]["deterministic"], 0);
        assert_eq!(result["summary"]["heuristic"], 1);

        let det_state = make_deterministic_state(); // claude hooks = deterministic
        let det_result = build_summary_changed(&det_state, 0);
        assert_eq!(det_result["summary"]["deterministic"], 1);
        assert_eq!(det_result["summary"]["heuristic"], 0);
    }

    #[test]
    fn state_changed_returns_changes() {
        let state = make_managed_state();

        // Version 0 → should have changes
        let result = build_state_changed(&state, 0);
        let changes = result["changes"].as_array().expect("changes array");
        assert!(!changes.is_empty(), "should have changes since v0");
        assert!(result["version"].as_u64().expect("version") > 0);

        // Each change should have session_key and timestamp
        for change in changes {
            assert!(change.get("session_key").is_some());
            assert!(change.get("timestamp").is_some());
        }
    }

    #[test]
    fn state_changed_no_changes_at_current_version() {
        let state = make_managed_state();
        let current_version = state.daemon.version();

        let result = build_state_changed(&state, current_version);
        let changes = result["changes"].as_array().expect("changes array");
        assert!(changes.is_empty(), "no changes at current version");
        assert_eq!(result["version"], current_version);
    }

    #[test]
    fn summary_changed_returns_counts() {
        let state = make_managed_state();

        let result = build_summary_changed(&state, 0);
        assert_eq!(result["has_changes"], true);
        assert!(result["pane_changes"].as_u64().expect("pane_changes") > 0);
        assert_eq!(result["summary"]["managed"], 1);
        assert_eq!(result["summary"]["unmanaged"], 0);
        assert_eq!(result["summary"]["total"], 1);
    }

    #[test]
    fn summary_changed_no_changes_at_current_version() {
        let state = make_managed_state();
        let current_version = state.daemon.version();

        let result = build_summary_changed(&state, current_version);
        assert_eq!(result["has_changes"], false);
        assert_eq!(result["pane_changes"], 0);
    }

    #[test]
    fn ui_bootstrap_v2_includes_required_fields() {
        let state = make_managed_state();

        let result = build_ui_bootstrap_v2(&state);
        assert_eq!(result["epoch"], 1);
        assert_eq!(result["snapshot_seq"], state.daemon.version());
        assert!(result["panes"].is_array());
        assert!(result["sessions"].is_array());
        assert!(result.get("generated_at").is_some());
        assert_eq!(result["replay_cursor"]["epoch"], 1);
        assert_eq!(result["replay_cursor"]["seq"], state.daemon.version());
    }

    // FR-066 regression: sync-v2 bootstrap panes must not contain legacy identity aliases.
    #[test]
    fn ui_bootstrap_v2_pane_no_legacy_session_id() {
        let mut state = make_managed_state();
        // Add an unmanaged pane to cover both branches.
        state.last_panes.push(TmuxPaneInfo {
            pane_id: "%99".to_string(),
            session_id: "$99".to_string(),
            session_name: "other".to_string(),
            window_id: "@99".to_string(),
            window_name: "misc".to_string(),
            current_cmd: "zsh".to_string(),
            ..Default::default()
        });

        let result = build_ui_bootstrap_v2(&state);
        let panes = result["panes"].as_array().expect("panes array");
        assert!(!panes.is_empty(), "must have managed panes");

        // No legacy session_id in any pane.
        for pane in panes {
            assert!(
                pane.get("session_id").is_none(),
                "sync-v2 pane must not contain legacy 'session_id' field (FR-066): pane_id={}",
                pane["pane_id"]
            );
        }
    }

    #[test]
    fn ui_bootstrap_v2_includes_unmanaged_pane_with_exact_identity() {
        let mut state = make_state();
        let now = Utc::now();
        state.last_panes.push(TmuxPaneInfo {
            pane_id: "%99".to_string(),
            session_name: "other".to_string(),
            window_id: "@99".to_string(),
            window_name: "misc".to_string(),
            current_cmd: "zsh".to_string(),
            current_path: "/tmp".to_string(),
            ..Default::default()
        });
        state.generation_tracker.update(&["%99"], now);

        let result = build_ui_bootstrap_v2(&state);
        let panes = result["panes"].as_array().expect("panes array");
        let unmanaged = panes
            .iter()
            .find(|pane| pane["pane_id"] == "%99")
            .expect("unmanaged pane in bootstrap");

        assert_eq!(unmanaged["presence"], "unmanaged");
        assert_eq!(unmanaged["session_name"], "other");
        assert_eq!(unmanaged["window_id"], "@99");
        assert_eq!(unmanaged["session_key"], "poller-%99");
        assert_eq!(unmanaged["pane_instance_id"]["pane_id"], "%99");
        assert!(unmanaged["pane_instance_id"]["generation"].is_number());
        assert!(unmanaged["pane_instance_id"]["birth_ts"].is_string());
        assert_eq!(unmanaged["current_cmd"], "zsh");
    }

    // FR-065 regression: managed pane in sync-v2 bootstrap must carry all required identity fields.
    #[test]
    fn ui_bootstrap_v2_managed_pane_required_identity_fields_present() {
        let state = make_managed_state();

        let result = build_ui_bootstrap_v2(&state);
        let panes = result["panes"].as_array().expect("panes array");
        let managed = panes
            .iter()
            .find(|p| p["presence"] == "managed")
            .expect("managed pane");

        assert!(
            managed.get("pane_id").is_some() && !managed["pane_id"].is_null(),
            "pane_id required"
        );
        assert!(
            managed.get("session_key").is_some() && !managed["session_key"].is_null(),
            "session_key required"
        );
        assert!(
            managed.get("session_name").is_some() && !managed["session_name"].is_null(),
            "session_name required"
        );
        assert!(managed.get("window_id").is_some(), "window_id required");
        assert!(
            !managed["window_id"].is_null(),
            "window_id must not be null"
        );
        assert!(
            managed.get("pane_instance_id").is_some() && managed["pane_instance_id"].is_object(),
            "pane_instance_id required and must be object"
        );
        assert!(
            managed["pane_instance_id"].get("pane_id").is_some(),
            "pane_instance_id.pane_id required"
        );
        assert!(
            managed["pane_instance_id"].get("generation").is_some(),
            "pane_instance_id.generation required"
        );
        assert!(
            managed["pane_instance_id"].get("birth_ts").is_some(),
            "pane_instance_id.birth_ts required"
        );
        assert_eq!(
            managed["session_key"], "poller-%0",
            "sync-v2 managed pane session_key must stay pane-stable across source transitions"
        );
    }

    // FR-065 regression: unresolved exact location must exclude the managed pane from sync-v2.
    #[test]
    fn ui_bootstrap_v2_excludes_managed_pane_when_exact_location_is_unresolved() {
        let mut state = make_managed_state();
        state.last_panes.clear();

        let result = build_ui_bootstrap_v2(&state);
        let panes = result["panes"].as_array().expect("panes array");

        assert!(
            panes.iter().all(|pane| pane["pane_id"] != "%0"),
            "managed pane with unresolved exact location must be excluded from sync-v2 bootstrap"
        );
        assert!(
            panes
                .iter()
                .all(|pane| !pane["session_name"].is_null() && !pane["window_id"].is_null()),
            "sync-v2 bootstrap must not emit null exact-location fields"
        );
    }

    // FR-066 regression: ui.changes.v2 change entries must not contain legacy session_id.
    #[test]
    fn ui_changes_v2_no_legacy_session_id() {
        let mut state = make_managed_state();

        let result = build_ui_changes_v2(&mut state, Some(ReplayCursor { epoch: 1, seq: 0 }), 10);
        let changes = result["changes"].as_array().expect("changes array");
        assert!(!changes.is_empty(), "must have changes");

        for change in changes {
            assert!(
                change.get("session_id").is_none(),
                "sync-v2 change entry must not contain legacy 'session_id' field (FR-066)"
            );
        }
    }

    #[test]
    fn ui_changes_v2_returns_ordered_changes() {
        let mut state = make_managed_state();

        let result = build_ui_changes_v2(&mut state, Some(ReplayCursor { epoch: 1, seq: 0 }), 10);
        let changes = result["changes"].as_array().expect("changes array");

        assert_eq!(result["epoch"], 1);
        assert_eq!(result["from_seq"], 1);
        assert_eq!(result["to_seq"], state.daemon.version());
        assert_eq!(result["next_cursor"]["epoch"], 1);
        assert_eq!(result["next_cursor"]["seq"], state.daemon.version());
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0]["seq"], 1);
        assert_eq!(changes[1]["seq"], 2);
        assert!(changes[0]["session"].is_object());
        assert!(changes[1]["pane"].is_object());
    }

    #[test]
    fn ui_changes_v2_pane_change_uses_pane_stable_session_key() {
        let mut state = make_managed_state();

        let result = build_ui_changes_v2(&mut state, Some(ReplayCursor { epoch: 1, seq: 0 }), 10);
        let changes = result["changes"].as_array().expect("changes array");
        let pane_change = changes
            .iter()
            .find(|c| c.get("pane").is_some())
            .expect("pane change present");

        assert_eq!(pane_change["pane_id"], "%0");
        assert_eq!(pane_change["session_key"], "poller-%0");
        assert_eq!(pane_change["pane"]["session_key"], "poller-%0");
    }

    #[test]
    fn ui_health_v1_reports_component_statuses() {
        let mut state = make_managed_state();
        let now = Utc::now();

        state.runtime_last_ok_at = Some(now);
        state.metadata_stale = true;
        state.metadata_last_success_at = Some(now);
        state.metadata_last_error = Some("metadata timeout".to_string());
        state.focused_pane_id = Some("%0".to_string());
        state.focus_last_sync_at = Some(now);
        state.daemon.record_replay_resync("trimmed_cursor", now);

        let result = build_ui_health_v1(&state);

        assert_eq!(result["runtime"]["status"], "ok");
        assert_eq!(result["overlay"]["status"], "degraded");
        assert_eq!(result["overlay"]["detail"], "metadata timeout");
        assert_eq!(result["replay"]["status"], "degraded");
        assert_eq!(result["replay"]["last_resync_reason"], "trimmed_cursor");
        assert_eq!(result["focus"]["status"], "ok");
        assert_eq!(result["focus"]["focused_pane_id"], "%0");
    }

    #[test]
    fn ui_health_v1_marks_runtime_unavailable_before_first_success() {
        let mut state = make_state();
        state.runtime_last_error = Some("socket bind failed".to_string());

        let result = build_ui_health_v1(&state);
        assert_eq!(result["runtime"]["status"], "unavailable");
        assert_eq!(result["runtime"]["detail"], "socket bind failed");
    }

    #[test]
    fn ui_health_v1_marks_focus_degraded_on_mismatch() {
        let mut state = make_state();
        let now = Utc::now();
        state.last_panes = vec![TmuxPaneInfo {
            pane_id: "%0".to_string(),
            session_id: "$0".to_string(),
            session_name: "main".to_string(),
            window_id: "@0".to_string(),
            window_name: "dev".to_string(),
            current_cmd: "zsh".to_string(),
            active: true,
            session_attached: true,
            ..Default::default()
        }];
        state.focused_pane_id = Some("%0".to_string());
        state.focus_mismatch_count = 2;
        state.focus_last_sync_at = Some(now);

        let result = build_ui_health_v1(&state);
        assert_eq!(result["focus"]["status"], "degraded");
        assert_eq!(result["focus"]["mismatch_count"], 2);
    }

    #[tokio::test]
    async fn list_panes_snapshot_includes_cache_metadata() {
        let mut state = make_state();
        state.last_panes = vec![TmuxPaneInfo {
            pane_id: "%40".to_string(),
            session_name: "work".to_string(),
            window_name: "main".to_string(),
            current_cmd: "zsh".to_string(),
            ..Default::default()
        }];
        state.metadata_stale = true;
        state.metadata_failure_streak = 2;
        state.metadata_last_error = Some("metadata timeout".to_string());
        let state = Arc::new(Mutex::new(state));

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "list_panes_snapshot",
            "id": 90,
            "params": {}
        });

        let resp = call_handler(Arc::clone(&state), request).await;
        assert!(resp["result"]["panes"].is_array());
        assert_eq!(resp["result"]["panes"][0]["pane_id"], "%40");
        assert_eq!(resp["result"]["metadata"]["stale"], true);
        assert_eq!(resp["result"]["metadata"]["failure_streak"], 2);
        assert_eq!(resp["result"]["metadata"]["last_error"], "metadata timeout");
    }

    #[tokio::test]
    async fn list_panes_returns_cached_rows() {
        let mut state = make_state();
        state.last_panes = vec![TmuxPaneInfo {
            pane_id: "%41".to_string(),
            session_name: "work".to_string(),
            window_name: "main".to_string(),
            current_cmd: "zsh".to_string(),
            ..Default::default()
        }];
        let state = Arc::new(Mutex::new(state));

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "list_panes",
            "id": 91,
            "params": {}
        });

        let resp = call_handler(Arc::clone(&state), request).await;
        let panes = resp["result"].as_array().expect("array");
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0]["pane_id"], "%41");
    }

    #[tokio::test]
    async fn ui_bootstrap_v2_handler_returns_snapshot() {
        let state = Arc::new(Mutex::new(make_managed_state()));
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "ui.bootstrap.v2",
            "id": 92,
            "params": {}
        });

        let resp = call_handler(Arc::clone(&state), request).await;
        assert_eq!(resp["result"]["epoch"], 1);
        assert!(resp["result"]["panes"].is_array());
        assert!(resp["result"]["sessions"].is_array());
    }

    #[test]
    fn ui_bootstrap_v3_emits_strict_identity_and_normalized_codex_truth() {
        let mut state = make_bootstrap_v3_codex_completed_state();

        let result = build_ui_bootstrap_v3(&mut state);
        let payload: UiBootstrapV3 =
            serde_json::from_value(result).expect("ui.bootstrap.v3 should parse");
        payload.validate().expect("ui.bootstrap.v3 should validate");

        assert_eq!(payload.version, 3);
        assert_eq!(payload.replay_cursor, Some(SyncV3CursorV3 { seq: 1 }));
        assert_eq!(payload.panes.len(), 1);
        let pane = &payload.panes[0];
        assert_eq!(pane.session_name, "workbench");
        assert_eq!(pane.window_id, "@5");
        assert_eq!(pane.session_key, "codex:%12");
        assert_eq!(pane.pane_id, "%12");
        assert_eq!(pane.pane_instance_id.pane_id, "%12");
        assert_eq!(
            pane.thread.lifecycle,
            agtmux_core_v5::sync_v3::ThreadLifecycleV3::Idle
        );
        assert_eq!(
            pane.thread.blocking,
            agtmux_core_v5::sync_v3::ThreadBlockingV3::None
        );
        assert_eq!(
            pane.thread.turn.outcome,
            agtmux_core_v5::sync_v3::TurnOutcomeV3::Completed
        );
    }

    #[test]
    fn codex_task_complete_intentionally_diverges_between_sync_v2_and_v3_surfaces() {
        let mut state = make_state();
        let now = Utc::now();
        state.last_panes = vec![TmuxPaneInfo {
            pane_id: "%1".to_string(),
            session_id: "$1".to_string(),
            session_name: "workbench".to_string(),
            window_id: "@1".to_string(),
            window_name: "main".to_string(),
            current_cmd: "node".to_string(),
            ..Default::default()
        }];
        state.generation_tracker.update(&["%1"], now);

        let mut event = codex_v3_event("%1", "task_complete", now);
        event.event_type = "activity.waiting_input".to_string();

        state.daemon.apply_events(vec![event.clone()], now);
        state
            .sync_v3
            .apply_events(&[event], &state.last_panes, &state.generation_tracker);

        let panes = build_pane_list(&state)
            .as_array()
            .expect("pane list should be array")
            .clone();
        let sync_v2_row = panes
            .iter()
            .find(|pane| pane["pane_id"] == "%1")
            .expect("sync-v2 row");
        assert_eq!(sync_v2_row["presence"], "managed");
        assert_eq!(sync_v2_row["provider"], "codex");
        assert_eq!(sync_v2_row["activity_state"], "WaitingInput");
        assert_eq!(sync_v2_row["current_cmd"], "node");

        let payload: UiBootstrapV3 = serde_json::from_value(build_ui_bootstrap_v3(&mut state))
            .expect("ui.bootstrap.v3 should parse");
        payload.validate().expect("ui.bootstrap.v3 should validate");

        let sync_v3_row = payload
            .panes
            .iter()
            .find(|pane| pane.pane_id == "%1")
            .expect("sync-v3 row");
        assert_eq!(sync_v3_row.session_name, "workbench");
        assert_eq!(sync_v3_row.window_id, "@1");
        assert_eq!(
            sync_v3_row.presence,
            agtmux_core_v5::sync_v3::PresenceV3::Managed
        );
        assert_eq!(sync_v3_row.provider, Some(Provider::Codex));
        assert_eq!(
            sync_v3_row.thread.lifecycle,
            agtmux_core_v5::sync_v3::ThreadLifecycleV3::Idle
        );
        assert_eq!(
            sync_v3_row.thread.blocking,
            agtmux_core_v5::sync_v3::ThreadBlockingV3::None
        );
        assert_eq!(
            sync_v3_row.thread.turn.outcome,
            agtmux_core_v5::sync_v3::TurnOutcomeV3::Completed
        );
    }

    #[test]
    fn ui_bootstrap_v3_emits_unmanaged_row_when_no_semantic_truth_exists() {
        let mut state = make_state();
        let now = Utc::now();
        state.last_panes = vec![TmuxPaneInfo {
            pane_id: "%99".to_string(),
            session_id: "$9".to_string(),
            session_name: "shells".to_string(),
            window_id: "@9".to_string(),
            window_name: "misc".to_string(),
            current_cmd: "zsh".to_string(),
            ..Default::default()
        }];
        state.generation_tracker.update(&["%99"], now);

        let result = build_ui_bootstrap_v3(&mut state);
        let payload: UiBootstrapV3 =
            serde_json::from_value(result).expect("ui.bootstrap.v3 should parse");
        payload.validate().expect("ui.bootstrap.v3 should validate");

        assert_eq!(payload.panes.len(), 1);
        assert_eq!(payload.replay_cursor, Some(SyncV3CursorV3 { seq: 1 }));
        let pane = &payload.panes[0];
        assert_eq!(pane.session_key, "shell:%99");
        assert_eq!(
            pane.presence,
            agtmux_core_v5::sync_v3::PresenceV3::Unmanaged
        );
        assert!(pane.provider.is_none());
        assert_eq!(
            pane.freshness.snapshot,
            agtmux_core_v5::types::FreshnessState::Down
        );
    }

    #[test]
    fn plain_shell_inventory_remains_unmanaged_in_bootstrap_until_provider_truth_arrives() {
        let mut state = make_state();
        let now = Utc::now();
        state.last_panes = vec![TmuxPaneInfo {
            pane_id: "%0".to_string(),
            session_id: "$0".to_string(),
            session_name: "agtmux-e2e-managed".to_string(),
            window_id: "@0".to_string(),
            window_name: "main".to_string(),
            current_cmd: "zsh".to_string(),
            ..Default::default()
        }];
        state.generation_tracker.update(&["%0"], now);

        let cache = new_pane_cache();
        refresh_pane_cache(&cache, &state, now);
        let cached_snapshot = read_cached_snapshot(&cache);
        let cached_row = &cached_snapshot["panes"][0];
        assert_eq!(cached_row["pane_id"], "%0");
        assert_eq!(cached_row["session_name"], "agtmux-e2e-managed");
        assert_eq!(cached_row["current_cmd"], "zsh");
        assert_eq!(cached_row["presence"], "unmanaged");

        let initial_payload: UiBootstrapV3 =
            serde_json::from_value(build_ui_bootstrap_v3(&mut state))
                .expect("ui.bootstrap.v3 should parse");
        initial_payload
            .validate()
            .expect("ui.bootstrap.v3 should validate");

        let initial_row = &initial_payload.panes[0];
        assert_eq!(initial_row.pane_id, "%0");
        assert_eq!(initial_row.session_name, "agtmux-e2e-managed");
        assert_eq!(initial_row.window_id, "@0");
        assert_eq!(initial_row.session_key, "shell:%0");
        assert_eq!(
            initial_row.presence,
            agtmux_core_v5::sync_v3::PresenceV3::Unmanaged
        );
        assert!(initial_row.provider.is_none());

        let event = codex_v3_event("%0", "task_started", now);
        state.daemon.apply_events(vec![event.clone()], now);
        state
            .sync_v3
            .apply_events(&[event], &state.last_panes, &state.generation_tracker);

        let managed_payload: UiBootstrapV3 =
            serde_json::from_value(build_ui_bootstrap_v3(&mut state))
                .expect("ui.bootstrap.v3 should parse after provider truth");
        managed_payload
            .validate()
            .expect("managed ui.bootstrap.v3 should validate");

        let managed_row = managed_payload
            .panes
            .iter()
            .find(|pane| pane.pane_id == "%0")
            .expect("managed row");
        assert_eq!(
            managed_row.presence,
            agtmux_core_v5::sync_v3::PresenceV3::Managed
        );
        assert_eq!(managed_row.provider, Some(Provider::Codex));
        assert_eq!(managed_row.session_key, "codex:%0");
    }

    #[test]
    fn ui_bootstrap_v3_preserves_linked_session_rows_even_when_v2_cache_compacts() {
        let mut state = make_state();
        let now = Utc::now();
        state.last_panes = vec![
            TmuxPaneInfo {
                pane_id: "%7".to_string(),
                session_id: "$1".to_string(),
                session_name: "linked".to_string(),
                window_id: "@0".to_string(),
                window_name: "main".to_string(),
                current_cmd: "node".to_string(),
                ..Default::default()
            },
            TmuxPaneInfo {
                pane_id: "%7".to_string(),
                session_id: "$2".to_string(),
                session_name: "primary".to_string(),
                window_id: "@0".to_string(),
                window_name: "main".to_string(),
                current_cmd: "node".to_string(),
                ..Default::default()
            },
        ];
        state.generation_tracker.update(&["%7"], now);

        let event = codex_v3_event("%7", "task_started", now);
        state.daemon.apply_events(vec![event.clone()], now);
        state
            .sync_v3
            .apply_events(&[event], &state.last_panes, &state.generation_tracker);

        let cache = new_pane_cache();
        refresh_pane_cache(&cache, &state, now);
        let cached_snapshot = read_cached_snapshot(&cache);
        let cached_panes = cached_snapshot["panes"].as_array().expect("cached panes");
        assert_eq!(
            cached_panes.len(),
            1,
            "sync-v2/list snapshot still compacts linked managed rows by pane_id"
        );

        let payload: UiBootstrapV3 = serde_json::from_value(build_ui_bootstrap_v3(&mut state))
            .expect("ui.bootstrap.v3 should parse");
        payload.validate().expect("ui.bootstrap.v3 should validate");

        assert_eq!(payload.panes.len(), 2);
        let linked = payload
            .panes
            .iter()
            .find(|pane| pane.session_name == "linked")
            .expect("linked row");
        let primary = payload
            .panes
            .iter()
            .find(|pane| pane.session_name == "primary")
            .expect("primary row");

        for pane in [linked, primary] {
            assert_eq!(pane.window_id, "@0");
            assert_eq!(pane.pane_id, "%7");
            assert_eq!(pane.pane_instance_id.pane_id, "%7");
            assert_eq!(pane.session_key, "codex:%7");
            assert_eq!(pane.presence, agtmux_core_v5::sync_v3::PresenceV3::Managed);
            assert_eq!(pane.provider, Some(Provider::Codex));
        }
    }

    #[test]
    fn ui_changes_v3_replaces_shell_row_when_exact_identity_changes_on_promotion() {
        let mut state = make_state();
        let now = Utc::now();
        state.last_panes = vec![TmuxPaneInfo {
            pane_id: "%0".to_string(),
            session_id: "$0".to_string(),
            session_name: "agtmux-e2e-managed".to_string(),
            window_id: "@0".to_string(),
            window_name: "main".to_string(),
            current_cmd: "node".to_string(),
            ..Default::default()
        }];
        state.generation_tracker.update(&["%0"], now);

        let bootstrap: UiBootstrapV3 = serde_json::from_value(build_ui_bootstrap_v3(&mut state))
            .expect("ui.bootstrap.v3 should parse");
        bootstrap.validate().expect("bootstrap should validate");
        let cursor = bootstrap.replay_cursor.expect("bootstrap cursor");
        let unmanaged_row = bootstrap.panes.first().expect("shell row");
        let pane_instance_id = unmanaged_row.pane_instance_id.clone();
        assert_eq!(unmanaged_row.session_name, "agtmux-e2e-managed");
        assert_eq!(unmanaged_row.window_id, "@0");
        assert_eq!(unmanaged_row.pane_id, "%0");
        assert_eq!(unmanaged_row.session_key, "shell:%0");

        let event = codex_v3_event("%0", "task_started", now);
        state.daemon.apply_events(vec![event.clone()], now);
        state
            .sync_v3
            .apply_events(&[event], &state.last_panes, &state.generation_tracker);

        let result = build_ui_changes_v3(&mut state, Some(cursor), 100);
        let payload: UiChangesV3 =
            serde_json::from_value(result).expect("ui.changes.v3 should parse");
        payload.validate().expect("ui.changes.v3 should validate");

        assert_eq!(payload.changes.len(), 2);
        let remove = &payload.changes[0];
        assert_eq!(
            remove.kind,
            agtmux_core_v5::sync_v3::SyncV3ChangeKindV3::Remove
        );
        assert_eq!(remove.pane_id, "%0");
        assert_eq!(remove.session_name, "agtmux-e2e-managed");
        assert_eq!(remove.window_id, "@0");
        assert_eq!(remove.pane_instance_id, pane_instance_id);
        assert_eq!(remove.session_key, "shell:%0");
        assert!(remove.pane.is_none());

        let change = &payload.changes[1];
        assert_eq!(
            change.kind,
            agtmux_core_v5::sync_v3::SyncV3ChangeKindV3::Upsert
        );
        assert_eq!(change.pane_id, "%0");
        assert_eq!(change.session_name, "agtmux-e2e-managed");
        assert_eq!(change.window_id, "@0");
        assert_eq!(change.pane_instance_id, pane_instance_id);
        assert_eq!(change.session_key, "codex:%0");
        assert!(
            change
                .field_groups
                .contains(&agtmux_core_v5::sync_v3::SyncV3FieldGroupV3::Identity),
            "replacement upsert should still advertise identity fields"
        );
        let pane = change.pane.as_ref().expect("managed pane");
        assert_eq!(pane.presence, agtmux_core_v5::sync_v3::PresenceV3::Managed);
        assert_eq!(pane.provider, Some(Provider::Codex));
        assert_eq!(pane.session_key, "codex:%0");
        assert_eq!(pane.pane_instance_id, pane_instance_id);
    }

    #[test]
    fn ui_changes_v3_emits_upsert_with_strict_identity_from_sync_v3_truth() {
        let mut state = make_bootstrap_v3_codex_completed_state();
        let bootstrap: UiBootstrapV3 = serde_json::from_value(build_ui_bootstrap_v3(&mut state))
            .expect("ui.bootstrap.v3 should parse");
        let cursor = bootstrap.replay_cursor.expect("bootstrap cursor");
        let observed_at = Utc::now();

        state.sync_v3.apply_events(
            &[codex_v3_event("%12", "function_call", observed_at)],
            &state.last_panes,
            &state.generation_tracker,
        );

        let result = build_ui_changes_v3(&mut state, Some(cursor), 100);
        let payload: UiChangesV3 =
            serde_json::from_value(result).expect("ui.changes.v3 should parse");
        payload.validate().expect("ui.changes.v3 should validate");

        assert_eq!(payload.from_seq, Some(2));
        assert_eq!(payload.to_seq, Some(2));
        assert_eq!(payload.next_cursor, Some(SyncV3CursorV3 { seq: 2 }));
        assert_eq!(payload.changes.len(), 1);

        let change = &payload.changes[0];
        assert_eq!(change.pane_id, "%12");
        assert_eq!(change.session_name, "workbench");
        assert_eq!(change.window_id, "@5");
        assert_eq!(change.session_key, "codex:%12");
        assert_eq!(change.pane_instance_id.pane_id, "%12");
        assert!(
            change
                .field_groups
                .contains(&agtmux_core_v5::sync_v3::SyncV3FieldGroupV3::Thread)
        );
        let pane = change.pane.as_ref().expect("upsert pane");
        assert_eq!(
            pane.thread.execution,
            agtmux_core_v5::sync_v3::ThreadExecutionV3::ToolRunning
        );
    }

    #[test]
    fn ui_changes_v3_emits_remove_with_strict_identity_when_row_disappears() {
        let mut state = make_bootstrap_v3_codex_completed_state();
        let bootstrap: UiBootstrapV3 = serde_json::from_value(build_ui_bootstrap_v3(&mut state))
            .expect("ui.bootstrap.v3 should parse");
        let cursor = bootstrap.replay_cursor.expect("bootstrap cursor");

        state.last_panes.clear();
        state.sync_v3.retain_inventory(&state.last_panes);

        let result = build_ui_changes_v3(&mut state, Some(cursor), 100);
        let payload: UiChangesV3 =
            serde_json::from_value(result).expect("ui.changes.v3 should parse");
        payload.validate().expect("ui.changes.v3 should validate");

        assert_eq!(payload.changes.len(), 1);
        let change = &payload.changes[0];
        assert_eq!(
            change.kind,
            agtmux_core_v5::sync_v3::SyncV3ChangeKindV3::Remove
        );
        assert_eq!(change.pane_id, "%12");
        assert_eq!(change.session_name, "workbench");
        assert_eq!(change.window_id, "@5");
        assert_eq!(change.session_key, "codex:%12");
        assert_eq!(
            change.field_groups,
            vec![agtmux_core_v5::sync_v3::SyncV3FieldGroupV3::Identity]
        );
        assert!(change.pane.is_none());
    }

    #[tokio::test]
    async fn ui_bootstrap_v3_handler_does_not_compact_sync_v2_log() {
        let state = Arc::new(Mutex::new(make_bootstrap_v3_codex_completed_state()));
        {
            let mut st = state.lock().await;
            populate_sync_v2_replay(&mut st);
            assert!(
                st.daemon.replay_len() > 0,
                "sync-v2 replay log should be populated"
            );
        }

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "ui.bootstrap.v3",
            "id": 192,
            "params": {}
        });

        let resp = call_handler(Arc::clone(&state), request).await;
        assert_eq!(resp["result"]["version"], 3);

        let st = state.lock().await;
        assert!(
            st.daemon.replay_len() > 0,
            "ui.bootstrap.v3 must not compact sync-v2 replay state"
        );
    }

    #[tokio::test]
    async fn ui_changes_v3_handler_resyncs_on_invalid_cursor() {
        let state = Arc::new(Mutex::new(make_bootstrap_v3_codex_completed_state()));
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "ui.changes.v3",
            "id": 193,
            "params": {
                "cursor": {},
                "limit": 100
            }
        });

        let resp = call_handler(Arc::clone(&state), request).await;
        assert_eq!(
            resp["result"]["resync_required"]["reason"],
            "invalid_cursor"
        );
        assert_eq!(resp["result"]["version"], 3);
    }

    #[tokio::test]
    async fn ui_changes_v2_handler_resyncs_on_epoch_mismatch() {
        let state = Arc::new(Mutex::new(make_managed_state()));
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "ui.changes.v2",
            "id": 93,
            "params": {
                "cursor": {"epoch": 99, "seq": 0},
                "limit": 100
            }
        });

        let resp = call_handler(Arc::clone(&state), request).await;
        assert_eq!(
            resp["result"]["resync_required"]["reason"],
            "epoch_mismatch"
        );
        assert_eq!(resp["result"]["resync_required"]["current_epoch"], 1);
    }

    #[tokio::test]
    async fn ui_changes_v2_handler_resyncs_on_trimmed_cursor() {
        let mut state = make_managed_state();
        state.daemon.trim_replay_before(1);
        let state = Arc::new(Mutex::new(state));
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "ui.changes.v2",
            "id": 94,
            "params": {
                "cursor": {"epoch": 1, "seq": 0},
                "limit": 100
            }
        });

        let resp = call_handler(Arc::clone(&state), request).await;
        assert_eq!(
            resp["result"]["resync_required"]["reason"],
            "trimmed_cursor"
        );
        assert_eq!(resp["result"]["resync_required"]["latest_snapshot_seq"], 2);
    }

    #[tokio::test]
    async fn ui_changes_v2_handler_resyncs_on_invalid_cursor() {
        let state = Arc::new(Mutex::new(make_managed_state()));
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "ui.changes.v2",
            "id": 95,
            "params": {
                "cursor": {"epoch": 1},
                "limit": 100
            }
        });

        let resp = call_handler(Arc::clone(&state), request).await;
        assert_eq!(
            resp["result"]["resync_required"]["reason"],
            "invalid_cursor"
        );
        assert_eq!(resp["result"]["resync_required"]["current_epoch"], 1);
    }

    #[tokio::test]
    async fn ui_bootstrap_v2_handler_compacts_sync_v2_without_touching_sync_v3_cursor() {
        let mut state = make_bootstrap_v3_codex_completed_state();
        let expected_v3_cursor = {
            let payload: UiBootstrapV3 = serde_json::from_value(build_ui_bootstrap_v3(&mut state))
                .expect("ui.bootstrap.v3 should parse");
            payload.replay_cursor.expect("v3 replay cursor")
        };
        populate_sync_v2_replay(&mut state);
        let state = Arc::new(Mutex::new(state));
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "ui.bootstrap.v2",
            "id": 96,
            "params": {}
        });

        let resp = call_handler(Arc::clone(&state), request).await;
        assert_eq!(resp["result"]["epoch"], 1);

        let st = state.lock().await;
        assert_eq!(st.daemon.replay_len(), 0, "sync-v2 replay should compact");
        assert!(
            !st.daemon.changes_since(0).is_empty(),
            "legacy change log must remain available"
        );
        assert_eq!(
            st.sync_v3.current_cursor(),
            expected_v3_cursor,
            "ui.bootstrap.v2 must not perturb the sync-v3 replay cursor"
        );
    }

    #[tokio::test]
    async fn ui_changes_v2_handler_acknowledges_sync_v2_without_touching_sync_v3_cursor() {
        let mut state = make_bootstrap_v3_codex_completed_state();
        let expected_v3_cursor = {
            let payload: UiBootstrapV3 = serde_json::from_value(build_ui_bootstrap_v3(&mut state))
                .expect("ui.bootstrap.v3 should parse");
            payload.replay_cursor.expect("v3 replay cursor")
        };
        populate_sync_v2_replay(&mut state);
        let state = Arc::new(Mutex::new(state));
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "ui.changes.v2",
            "id": 196,
            "params": {
                "cursor": {"epoch": 1, "seq": 0},
                "limit": 100
            }
        });

        let resp = call_handler(Arc::clone(&state), request).await;
        assert_eq!(resp["result"]["epoch"], 1);
        assert!(resp["result"]["changes"].is_array());

        let ack_request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "ui.changes.v2",
            "id": 197,
            "params": {
                "cursor": resp["result"]["next_cursor"].clone(),
                "limit": 100
            }
        });
        let ack_resp = call_handler(Arc::clone(&state), ack_request).await;
        assert_eq!(ack_resp["result"]["epoch"], 1);

        let st = state.lock().await;
        assert_eq!(
            st.daemon.replay_len(),
            0,
            "sync-v2 replay should acknowledge and compact"
        );
        assert_eq!(
            st.sync_v3.current_cursor(),
            expected_v3_cursor,
            "ui.changes.v2 must not perturb the sync-v3 replay cursor"
        );
    }

    #[tokio::test]
    async fn ui_health_v1_handler_returns_snapshot() {
        let mut state = make_managed_state();
        let now = Utc::now();
        state.runtime_last_ok_at = Some(now);
        state.focused_pane_id = Some("%0".to_string());
        state.focus_last_sync_at = Some(now);
        let state = Arc::new(Mutex::new(state));
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "ui.health.v1",
            "id": 97,
            "params": {}
        });

        let resp = call_handler(Arc::clone(&state), request).await;
        assert!(resp["result"].get("generated_at").is_some());
        assert_eq!(resp["result"]["runtime"]["status"], "ok");
        assert_eq!(resp["result"]["focus"]["focused_pane_id"], "%0");
    }

    // ── source.ingest tests (via UDS handler) ──────────────────────────

    /// Helper: send a JSON-RPC request through handle_connection and return the response.
    async fn call_handler(
        state: Arc<Mutex<DaemonState>>,
        request: serde_json::Value,
    ) -> serde_json::Value {
        let pane_cache = new_pane_cache();
        {
            let st = state.lock().await;
            refresh_pane_cache(&pane_cache, &st, Utc::now());
        }

        let (client, server) = tokio::net::UnixStream::pair().expect("unix pair");
        let (mut c_reader, mut c_writer) = client.into_split();

        let req_str = format!("{}\n", serde_json::to_string(&request).expect("serialize"));

        // Write request and read response concurrently
        let write_fut = async move {
            use tokio::io::AsyncWriteExt;
            c_writer.write_all(req_str.as_bytes()).await.expect("write");
            c_writer.shutdown().await.expect("shutdown");
        };

        let read_fut = async move {
            let mut buf = String::new();
            let mut reader = tokio::io::BufReader::new(&mut c_reader);
            use tokio::io::AsyncBufReadExt;
            reader.read_line(&mut buf).await.expect("read");
            serde_json::from_str::<serde_json::Value>(buf.trim()).expect("parse response")
        };

        let handle_fut = handle_connection(server, state, pane_cache);

        let (_, response, _) = tokio::join!(write_fut, read_fut, handle_fut);
        response
    }

    #[tokio::test]
    async fn source_ingest_claude_hooks_accepted() {
        let state = Arc::new(Mutex::new(make_state()));
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "source.ingest",
            "id": 1,
            "params": {
                "source_kind": "claude_hooks",
                "event": {
                    "hook_id": "h-test-1",
                    "hook_type": "tool_start",
                    "session_id": "sess-test",
                    "timestamp": "2026-02-25T12:00:00Z",
                    "pane_id": "%0",
                    "data": {}
                }
            }
        });

        let resp = call_handler(Arc::clone(&state), request).await;
        assert_eq!(resp["result"]["status"], "ok");

        let st = state.lock().await;
        assert_eq!(st.claude_source.buffered_len(), 1);
    }

    #[tokio::test]
    async fn source_ingest_codex_appserver_rejected() {
        // codex_appserver source kind is no longer supported via UDS ingest;
        // all Codex detection now uses agtmux-source-codex-jsonl (T-codex01c).
        let state = Arc::new(Mutex::new(make_state()));
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "source.ingest",
            "id": 2,
            "params": {
                "source_kind": "codex_appserver",
                "event": {}
            }
        });

        let resp = call_handler(Arc::clone(&state), request).await;
        assert_eq!(resp["error"]["code"], -32602);
        assert!(
            resp["error"]["message"]
                .as_str()
                .expect("message")
                .contains("unknown source_kind")
        );
    }

    #[tokio::test]
    async fn source_ingest_unknown_source_kind_rejected() {
        let state = Arc::new(Mutex::new(make_state()));
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "source.ingest",
            "id": 3,
            "params": {
                "source_kind": "unknown",
                "event": {}
            }
        });

        let resp = call_handler(Arc::clone(&state), request).await;
        assert_eq!(resp["error"]["code"], -32602);
        assert!(
            resp["error"]["message"]
                .as_str()
                .expect("message")
                .contains("unknown source_kind")
        );
    }

    #[tokio::test]
    async fn source_ingest_malformed_event_rejected() {
        let state = Arc::new(Mutex::new(make_state()));
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "source.ingest",
            "id": 4,
            "params": {
                "source_kind": "claude_hooks",
                "event": {"bad": "data"}
            }
        });

        let resp = call_handler(Arc::clone(&state), request).await;
        assert_eq!(resp["error"]["code"], -32602);
        assert!(
            resp["error"]["message"]
                .as_str()
                .expect("message")
                .contains("invalid event")
        );
    }

    // ── T-118: latency_status API test ────────────────────────────────

    #[tokio::test]
    async fn latency_status_returns_evaluation() {
        let state = Arc::new(Mutex::new(make_state()));
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "latency_status",
            "id": 10,
            "params": {}
        });

        // Before any poll tick, should return "not_started"
        let resp = call_handler(Arc::clone(&state), request.clone()).await;
        assert_eq!(resp["result"]["status"], "not_started");
        assert_eq!(resp["result"]["sample_count"], 0);

        // Simulate a poll tick with latency recording
        {
            let mut st = state.lock().await;
            let now_ms = chrono::Utc::now().timestamp_millis() as u64;
            st.latency_window.record(10, now_ms);
            st.last_latency_eval = Some(st.latency_window.evaluate(now_ms));
        }

        let resp2 = call_handler(Arc::clone(&state), request).await;
        // After recording, status should be "insufficient_data" (only 1 sample)
        assert_eq!(resp2["result"]["status"], "insufficient_data");
        assert_eq!(resp2["result"]["sample_count"], 1);
    }

    // ── T-117: source registry API tests ──────────────────────────────

    #[tokio::test]
    async fn source_hello_accepted() {
        let state = Arc::new(Mutex::new(make_state()));
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "source.hello",
            "id": 20,
            "params": {
                "source_id": "poller",
                "source_kind": "poller",
                "protocol_version": 1
            }
        });

        let resp = call_handler(Arc::clone(&state), request).await;
        assert_eq!(resp["result"]["status"], "accepted");
        assert_eq!(resp["result"]["source_id"], "poller");
    }

    #[tokio::test]
    async fn source_hello_rejected_bad_protocol() {
        let state = Arc::new(Mutex::new(make_state()));
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "source.hello",
            "id": 21,
            "params": {
                "source_id": "poller",
                "source_kind": "poller",
                "protocol_version": 0
            }
        });

        let resp = call_handler(Arc::clone(&state), request).await;
        assert_eq!(resp["result"]["status"], "rejected");
    }

    #[tokio::test]
    async fn source_heartbeat_acknowledged() {
        let state = Arc::new(Mutex::new(make_state()));
        // First register via hello
        let hello = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "source.hello",
            "id": 22,
            "params": {
                "source_id": "claude_hooks",
                "source_kind": "claude_hooks",
                "protocol_version": 1
            }
        });
        call_handler(Arc::clone(&state), hello).await;

        // Then heartbeat
        let hb = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "source.heartbeat",
            "id": 23,
            "params": {"source_id": "claude_hooks"}
        });
        let resp = call_handler(Arc::clone(&state), hb).await;
        assert_eq!(resp["result"]["acknowledged"], true);
    }

    #[tokio::test]
    async fn source_heartbeat_unknown_false() {
        let state = Arc::new(Mutex::new(make_state()));
        let hb = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "source.heartbeat",
            "id": 24,
            "params": {"source_id": "nonexistent"}
        });
        let resp = call_handler(Arc::clone(&state), hb).await;
        assert_eq!(resp["result"]["acknowledged"], false);
    }

    #[tokio::test]
    async fn poll_tick_staleness_check() {
        // Directly test staleness via DaemonState
        use agtmux_gateway::source_registry::{HelloRequest, HelloResponse};

        let mut state = make_state();
        let old_ms = 0_u64; // very old heartbeat
        let req = HelloRequest {
            source_id: "test-source".to_string(),
            source_kind: SourceKind::Poller,
            protocol_version: 1,
            socket_path: None,
        };
        let resp = state.source_registry.handle_hello(req, old_ms);
        assert!(matches!(resp, HelloResponse::Accepted { .. }));

        // Check staleness with a much later timestamp
        let stale = state.source_registry.check_staleness(999_999_999);
        assert!(
            stale.contains(&"test-source".to_string()),
            "source should be stale"
        );
    }

    #[tokio::test]
    async fn list_source_registry_returns_entries() {
        let state = Arc::new(Mutex::new(make_state()));
        // Register a source
        let hello = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "source.hello",
            "id": 25,
            "params": {
                "source_id": "claude_hooks",
                "source_kind": "claude_hooks",
                "protocol_version": 1
            }
        });
        call_handler(Arc::clone(&state), hello).await;

        let list_req = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "list_source_registry",
            "id": 26,
            "params": {}
        });
        let resp = call_handler(Arc::clone(&state), list_req).await;
        let entries = resp["result"].as_array().expect("should be array");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["source_id"], "claude_hooks");
        assert_eq!(entries[0]["lifecycle"], "active");
    }

    // ── T-115: TrustGuard admission + daemon.info tests ───────────────

    #[tokio::test]
    async fn trust_guard_admits_matching_uid() {
        // source.ingest with a registered source_id should succeed (warn-only)
        let state = Arc::new(Mutex::new(make_state()));
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "source.ingest",
            "id": 30,
            "params": {
                "source_kind": "claude_hooks",
                "source_id": "claude_hooks",
                "event": {
                    "hook_id": "h-trust-1",
                    "hook_type": "tool_start",
                    "session_id": "sess-trust",
                    "timestamp": "2026-02-25T12:00:00Z",
                    "pane_id": "%0",
                    "data": {}
                }
            }
        });

        let resp = call_handler(Arc::clone(&state), request).await;
        assert_eq!(resp["result"]["status"], "ok");
    }

    #[tokio::test]
    async fn trust_guard_warns_unregistered_source() {
        // source.ingest with unknown source_id — still processed (warn-only)
        let state = Arc::new(Mutex::new(make_state()));
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "source.ingest",
            "id": 31,
            "params": {
                "source_kind": "claude_hooks",
                "source_id": "unknown_source",
                "event": {
                    "hook_id": "h-warn-1",
                    "hook_type": "tool_start",
                    "session_id": "sess-warn",
                    "timestamp": "2026-02-25T12:00:00Z",
                    "pane_id": "%0",
                    "data": {}
                }
            }
        });

        let resp = call_handler(Arc::clone(&state), request).await;
        // Should still succeed (warn-only, processing continues)
        assert_eq!(resp["result"]["status"], "ok");
    }

    #[tokio::test]
    async fn trust_guard_warns_wrong_nonce() {
        // source.ingest with wrong nonce — still processed (warn-only)
        let state = Arc::new(Mutex::new(make_state()));
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "source.ingest",
            "id": 32,
            "params": {
                "source_kind": "claude_hooks",
                "source_id": "claude_hooks",
                "nonce": "wrong-nonce",
                "event": {
                    "hook_id": "h-nonce-1",
                    "hook_type": "tool_start",
                    "session_id": "sess-nonce",
                    "timestamp": "2026-02-25T12:00:00Z",
                    "pane_id": "%0",
                    "data": {}
                }
            }
        });

        let resp = call_handler(Arc::clone(&state), request).await;
        // Should still succeed (warn-only, processing continues)
        assert_eq!(resp["result"]["status"], "ok");
    }

    #[tokio::test]
    async fn daemon_info_returns_nonce() {
        let state = Arc::new(Mutex::new(make_state()));
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "daemon.info",
            "id": 33,
            "params": {}
        });

        let resp = call_handler(Arc::clone(&state), request).await;
        let nonce = resp["result"]["nonce"].as_str().expect("nonce string");
        assert!(!nonce.is_empty(), "nonce should not be empty");
        assert!(resp["result"]["pid"].as_u64().is_some(), "pid should exist");
        assert!(
            resp["result"]["version"].as_str().is_some(),
            "version should exist"
        );
    }

    #[test]
    fn trust_guard_pre_registers_three_sources() {
        let state = make_state();
        assert_eq!(
            state.trust_guard.registered_count(),
            3,
            "DaemonState::new() should pre-register poller, claude_hooks, claude_jsonl"
        );
        assert!(state.trust_guard.is_registered("poller"));
        assert!(state.trust_guard.is_registered("claude_hooks"));
        assert!(state.trust_guard.is_registered("claude_jsonl"));
    }

    // T-130: window_id / session_id / current_path exposed in list_panes response
    #[test]
    fn build_pane_list_includes_window_session_path_for_unmanaged() {
        let mut state = make_state();
        state.last_panes = vec![TmuxPaneInfo {
            pane_id: "%10".to_string(),
            session_id: "$3".to_string(),
            session_name: "work".to_string(),
            window_id: "@7".to_string(),
            window_name: "editor".to_string(),
            current_cmd: "zsh".to_string(),
            current_path: "/home/user/project".to_string(),
            ..Default::default()
        }];

        let result = build_pane_list(&state);
        let arr = result.as_array().expect("array");
        let pane = &arr[0];
        assert_eq!(pane["pane_id"], "%10");
        assert_eq!(pane["presence"], "unmanaged");
        assert_eq!(pane["session_id"], "$3");
        assert_eq!(pane["window_id"], "@7");
        assert_eq!(pane["current_path"], "/home/user/project");
    }

    #[test]
    fn build_pane_list_includes_window_session_path_for_managed() {
        use agtmux_core_v5::types::SourceKind;

        let mut state = make_state();
        let now = Utc::now();
        let snapshot = agtmux_source_poller::source::PaneSnapshot {
            pane_id: "%20".to_string(),
            pane_title: String::new(),
            current_cmd: "claude".to_string(),
            process_hint: Some("claude".to_string()),
            capture_lines: vec!["\u{256D} Claude Code".to_string()],
            captured_at: now,
        };
        state.poller.poll_batch(&[snapshot]);
        let pull_req = agtmux_core_v5::types::PullEventsRequest {
            cursor: None,
            limit: 100,
        };
        let poller_resp = state.poller.pull_events(&pull_req, now);
        state
            .gateway
            .ingest_source_response(SourceKind::Poller, poller_resp);
        let gw_resp = state
            .gateway
            .pull_events(&agtmux_core_v5::types::GatewayPullRequest {
                cursor: None,
                limit: 100,
            });
        state.daemon.apply_events(gw_resp.events, now);

        state.last_panes = vec![TmuxPaneInfo {
            pane_id: "%20".to_string(),
            session_id: "$5".to_string(),
            session_name: "agents".to_string(),
            window_id: "@12".to_string(),
            window_name: "main".to_string(),
            current_cmd: "claude".to_string(),
            current_path: "/Users/user/repo".to_string(),
            ..Default::default()
        }];

        let result = build_pane_list(&state);
        let arr = result.as_array().expect("array");
        let managed = arr.iter().find(|p| p["pane_id"] == "%20").expect("%20");
        assert_eq!(managed["presence"], "managed");
        assert_eq!(managed["session_id"], "$5");
        assert_eq!(managed["window_id"], "@12");
        assert_eq!(managed["current_path"], "/Users/user/repo");
    }

    // T-135a: conversation_title exposed from DaemonState.conversation_titles
    #[test]
    fn build_pane_list_includes_conversation_title_when_available() {
        let mut state = make_state();
        let now = Utc::now();
        let snapshot = agtmux_source_poller::source::PaneSnapshot {
            pane_id: "%30".to_string(),
            pane_title: String::new(),
            current_cmd: "codex".to_string(),
            process_hint: Some("codex".to_string()),
            capture_lines: vec![],
            captured_at: now,
        };
        state.poller.poll_batch(&[snapshot]);
        let pull_req = agtmux_core_v5::types::PullEventsRequest {
            cursor: None,
            limit: 100,
        };
        let poller_resp = state.poller.pull_events(&pull_req, now);
        state
            .gateway
            .ingest_source_response(SourceKind::Poller, poller_resp);
        let gw_resp = state
            .gateway
            .pull_events(&agtmux_core_v5::types::GatewayPullRequest {
                cursor: None,
                limit: 100,
            });
        state.daemon.apply_events(gw_resp.events, now);

        // Simulate T-135a: conversation_titles populated by Codex poller
        // session_key for the managed pane is its source session_key from the event
        let managed_panes = state.daemon.list_panes();
        let pane = managed_panes
            .iter()
            .find(|p| p.pane_instance_id.pane_id == "%30")
            .expect("managed pane %30");
        state
            .conversation_titles
            .insert(pane.session_key.clone(), "TUI prototype".to_string());

        state.last_panes = vec![agtmux_tmux_v5::TmuxPaneInfo {
            pane_id: "%30".to_string(),
            current_cmd: "codex".to_string(),
            ..Default::default()
        }];

        let result = build_pane_list(&state);
        let arr = result.as_array().expect("array");
        let managed = arr.iter().find(|p| p["pane_id"] == "%30").expect("%30");
        assert_eq!(managed["conversation_title"], "TUI prototype");
    }

    #[test]
    fn build_pane_list_conversation_title_null_when_absent() {
        let mut state = make_state();
        let now = Utc::now();
        let snapshot = agtmux_source_poller::source::PaneSnapshot {
            pane_id: "%31".to_string(),
            pane_title: String::new(),
            current_cmd: "codex".to_string(),
            process_hint: Some("codex".to_string()),
            capture_lines: vec![],
            captured_at: now,
        };
        state.poller.poll_batch(&[snapshot]);
        let pull_req = agtmux_core_v5::types::PullEventsRequest {
            cursor: None,
            limit: 100,
        };
        let poller_resp = state.poller.pull_events(&pull_req, now);
        state
            .gateway
            .ingest_source_response(SourceKind::Poller, poller_resp);
        let gw_resp = state
            .gateway
            .pull_events(&agtmux_core_v5::types::GatewayPullRequest {
                cursor: None,
                limit: 100,
            });
        state.daemon.apply_events(gw_resp.events, now);
        state.last_panes = vec![agtmux_tmux_v5::TmuxPaneInfo {
            pane_id: "%31".to_string(),
            current_cmd: "codex".to_string(),
            ..Default::default()
        }];

        let result = build_pane_list(&state);
        let arr = result.as_array().expect("array");
        let managed = arr.iter().find(|p| p["pane_id"] == "%31").expect("%31");
        // No entry in conversation_titles → field is null
        assert!(
            managed["conversation_title"].is_null(),
            "conversation_title should be null when absent"
        );
    }
}

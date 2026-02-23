use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, RwLock};
use tokio_util::sync::CancellationToken;

use crate::orchestrator::StateNotification;

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

/// Thread-safe handle to daemon state shared between the orchestrator and server.
pub type SharedState = Arc<RwLock<DaemonState>>;

/// Snapshot of all known panes, owned by the orchestrator and read by the server.
#[derive(Debug, Default)]
pub struct DaemonState {
    pub panes: Vec<PaneInfo>,
}

/// Wire-format pane info returned by `list_panes` and embedded in notifications.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaneInfo {
    pub pane_id: String,
    pub session_name: String,
    pub window_id: String,
    pub pane_title: String,
    pub current_cmd: String,
    pub provider: Option<String>,
    pub provider_confidence: f64,
    pub activity_state: String,
    pub activity_confidence: f64,
    pub activity_source: String,
    pub attention_state: String,
    pub attention_reason: String,
    pub attention_since: Option<String>,
    pub updated_at: String,
}

// ---------------------------------------------------------------------------
// JSON-RPC types (newline-delimited JSON)
// ---------------------------------------------------------------------------

fn default_jsonrpc() -> String {
    "2.0".into()
}

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    #[serde(default = "default_jsonrpc")]
    pub jsonrpc: String,
    pub id: Option<u64>,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
}

/// Server-initiated push (no `id`).
#[derive(Debug, Serialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    pub params: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Subscribe params
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct SubscribeParams {
    #[serde(default)]
    events: Vec<String>,
}

// ---------------------------------------------------------------------------
// DaemonServer
// ---------------------------------------------------------------------------

/// Unix-socket server that exposes the daemon API to local clients.
///
/// Protocol: Newline-delimited JSON over Unix stream sockets.
///
/// Supported methods:
///   - `list_panes`        -- returns current pane state snapshot
///   - `subscribe`         -- subscribe to state/topology push notifications
///   - `subscribe_summary` -- subscribe to aggregated summary pushes
pub struct DaemonServer {
    socket_path: PathBuf,
    state: SharedState,
    notify_tx: broadcast::Sender<StateNotification>,
    /// Cancellation token for graceful shutdown.
    cancel: CancellationToken,
}

impl DaemonServer {
    /// Create a new server.
    ///
    /// * `socket_path` -- path for the Unix domain socket (e.g. `/tmp/agtmux/agtmuxd.sock`).
    /// * `state`       -- shared daemon state (the orchestrator writes, the server reads).
    /// * `notify_tx`   -- broadcast sender the orchestrator uses to publish notifications.
    ///                    The server clones receivers for each accepted client.
    pub fn new(
        socket_path: impl Into<PathBuf>,
        state: SharedState,
        notify_tx: broadcast::Sender<StateNotification>,
    ) -> Self {
        Self::with_cancel(socket_path, state, notify_tx, CancellationToken::new())
    }

    /// Create a server with an explicit cancellation token for graceful shutdown.
    pub fn with_cancel(
        socket_path: impl Into<PathBuf>,
        state: SharedState,
        notify_tx: broadcast::Sender<StateNotification>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            socket_path: socket_path.into(),
            state,
            notify_tx,
            cancel,
        }
    }

    /// Run the server: bind the listener and accept connections until
    /// cancelled or a fatal listener error occurs.
    pub async fn run(self) -> std::io::Result<()> {
        // Ensure parent directory exists.
        if let Some(parent) = self.socket_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Clean up stale socket file from a previous run.
        cleanup_socket(&self.socket_path).await;

        let listener = UnixListener::bind(&self.socket_path)?;
        tracing::info!(path = %self.socket_path.display(), "daemon server listening");

        loop {
            tokio::select! {
                result = listener.accept() => {
                    match result {
                        Ok((stream, _addr)) => {
                            let state = Arc::clone(&self.state);
                            let notify_rx = self.notify_tx.subscribe();
                            tokio::spawn(async move {
                                if let Err(e) = handle_client(stream, state, notify_rx).await {
                                    tracing::debug!(error = %e, "client handler finished with error");
                                }
                            });
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "accept failed");
                        }
                    }
                }
                _ = self.cancel.cancelled() => {
                    tracing::info!("daemon server: cancellation requested, shutting down");
                    break;
                }
            }
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Per-client handler
// ---------------------------------------------------------------------------

async fn handle_client(
    stream: UnixStream,
    state: SharedState,
    mut notify_rx: broadcast::Receiver<StateNotification>,
) -> std::io::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    tracing::debug!("client connected");

    // Track which event kinds this client is subscribed to.
    let mut subscribed_events: Vec<String> = Vec::new();
    let mut subscribed_summary = false;

    loop {
        tokio::select! {
            // --- incoming request from client ---
            line = lines.next_line() => {
                let line = match line {
                    Ok(Some(l)) => l,
                    Ok(None) => {
                        tracing::debug!("client disconnected (EOF)");
                        return Ok(());
                    }
                    Err(e) => {
                        tracing::debug!(error = %e, "read error, dropping client");
                        return Err(e);
                    }
                };

                let req: JsonRpcRequest = match serde_json::from_str(&line) {
                    Ok(r) => r,
                    Err(e) => {
                        let resp = JsonRpcResponse {
                            jsonrpc: "2.0".into(),
                            id: None,
                            result: None,
                            error: Some(JsonRpcError {
                                code: -32700,
                                message: format!("parse error: {e}"),
                            }),
                        };
                        write_json(&mut writer, &resp).await?;
                        continue;
                    }
                };

                tracing::debug!(method = %req.method, id = ?req.id, "request received");

                match req.method.as_str() {
                    "list_panes" => {
                        let panes = {
                            let s = state.read().await;
                            s.panes.clone()
                        };
                        let resp = JsonRpcResponse {
                            jsonrpc: "2.0".into(),
                            id: req.id,
                            result: Some(serde_json::json!({ "panes": panes })),
                            error: None,
                        };
                        write_json(&mut writer, &resp).await?;
                    }

                    "subscribe" => {
                        let params: SubscribeParams = serde_json::from_value(req.params)
                            .unwrap_or(SubscribeParams {
                                events: vec!["state".into(), "topology".into()],
                            });
                        subscribed_events = params.events;
                        tracing::debug!(events = ?subscribed_events, "client subscribed");

                        let resp = JsonRpcResponse {
                            jsonrpc: "2.0".into(),
                            id: req.id,
                            result: Some(serde_json::json!({ "subscribed": true })),
                            error: None,
                        };
                        write_json(&mut writer, &resp).await?;
                    }

                    "subscribe_summary" => {
                        subscribed_summary = true;
                        tracing::debug!("client subscribed to summary");

                        let resp = JsonRpcResponse {
                            jsonrpc: "2.0".into(),
                            id: req.id,
                            result: Some(serde_json::json!({ "subscribed": true })),
                            error: None,
                        };
                        write_json(&mut writer, &resp).await?;

                        // Send an immediate summary snapshot so the client
                        // does not have to wait for the next state change.
                        let counts = compute_summary_counts(&state).await;
                        let notif = JsonRpcNotification {
                            jsonrpc: "2.0".into(),
                            method: "summary".into(),
                            params: serde_json::json!({ "counts": counts }),
                        };
                        write_json(&mut writer, &notif).await?;
                    }

                    _ => {
                        let resp = JsonRpcResponse {
                            jsonrpc: "2.0".into(),
                            id: req.id,
                            result: None,
                            error: Some(JsonRpcError {
                                code: -32601,
                                message: format!("method not found: {}", req.method),
                            }),
                        };
                        write_json(&mut writer, &resp).await?;
                    }
                }
            }

            // --- push notification from orchestrator ---
            notification = notify_rx.recv() => {
                let notification = match notification {
                    Ok(n) => n,
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(skipped, "client lagged, dropped notifications");
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        tracing::debug!("notification channel closed, dropping client");
                        return Ok(());
                    }
                };

                // Forward state/topology events to subscribed clients.
                if !subscribed_events.is_empty() {
                    if let Some(notif) = notification_to_push(&notification, &subscribed_events) {
                        if let Err(e) = write_json(&mut writer, &notif).await {
                            tracing::debug!(error = %e, "failed to push notification, dropping client");
                            return Err(e);
                        }
                    }
                }

                // Forward summary to subscribed clients.
                if subscribed_summary {
                    let counts = compute_summary_counts(&state).await;
                    let notif = JsonRpcNotification {
                        jsonrpc: "2.0".into(),
                        method: "summary".into(),
                        params: serde_json::json!({ "counts": counts }),
                    };
                    if let Err(e) = write_json(&mut writer, &notif).await {
                        tracing::debug!(error = %e, "failed to push summary, dropping client");
                        return Err(e);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Notification mapping
// ---------------------------------------------------------------------------

/// Map a `StateNotification` to a JSON-RPC push if the client is subscribed
/// to the relevant event kind.
fn notification_to_push(
    notification: &StateNotification,
    subscribed_events: &[String],
) -> Option<JsonRpcNotification> {
    match notification {
        StateNotification::StateChanged {
            pane_id,
            state: pane_state,
        } => {
            if subscribed_events.iter().any(|e| e == "state") {
                Some(JsonRpcNotification {
                    jsonrpc: "2.0".into(),
                    method: "state_changed".into(),
                    params: serde_json::json!({
                        "pane_id": pane_id,
                        "state": pane_state,
                    }),
                })
            } else {
                None
            }
        }
        StateNotification::PaneAdded { pane_id } => {
            if subscribed_events.iter().any(|e| e == "topology") {
                Some(JsonRpcNotification {
                    jsonrpc: "2.0".into(),
                    method: "pane_added".into(),
                    params: serde_json::json!({ "pane_id": pane_id }),
                })
            } else {
                None
            }
        }
        StateNotification::PaneRemoved { pane_id } => {
            if subscribed_events.iter().any(|e| e == "topology") {
                Some(JsonRpcNotification {
                    jsonrpc: "2.0".into(),
                    method: "pane_removed".into(),
                    params: serde_json::json!({ "pane_id": pane_id }),
                })
            } else {
                None
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Serialize a value as a single JSON line terminated by `\n` and flush.
async fn write_json<T: Serialize>(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    value: &T,
) -> std::io::Result<()> {
    let mut buf = serde_json::to_vec(value)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    buf.push(b'\n');
    writer.write_all(&buf).await?;
    writer.flush().await
}

/// Compute aggregated counts from the current state, grouped by `activity_state`.
async fn compute_summary_counts(state: &SharedState) -> HashMap<String, usize> {
    let s = state.read().await;
    let mut counts: HashMap<String, usize> = HashMap::new();
    for pane in &s.panes {
        *counts.entry(pane.activity_state.clone()).or_insert(0) += 1;
    }
    counts
}

/// Remove a stale socket file if it exists.
async fn cleanup_socket(path: &Path) {
    if path.exists() {
        tracing::info!(path = %path.display(), "removing stale socket");
        if let Err(e) = tokio::fs::remove_file(path).await {
            tracing::warn!(
                error = %e,
                path = %path.display(),
                "failed to remove stale socket"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_list_panes_request() {
        let json = r#"{"jsonrpc": "2.0", "id": 1, "method": "list_panes", "params": {}}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.id, Some(1));
        assert_eq!(req.method, "list_panes");
    }

    #[test]
    fn parse_subscribe_request() {
        let json =
            r#"{"jsonrpc": "2.0", "id": 2, "method": "subscribe", "params": {"events": ["state", "topology"]}}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.method, "subscribe");
        let params: SubscribeParams = serde_json::from_value(req.params).unwrap();
        assert_eq!(params.events, vec!["state", "topology"]);
    }

    #[test]
    fn parse_subscribe_summary_request() {
        let json = r#"{"jsonrpc": "2.0", "id": 3, "method": "subscribe_summary", "params": {}}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.method, "subscribe_summary");
    }

    #[test]
    fn parse_request_without_id() {
        let json = r#"{"jsonrpc": "2.0", "method": "list_panes", "params": {}}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.id, None);
        assert_eq!(req.method, "list_panes");
    }

    #[test]
    fn parse_request_without_params() {
        let json = r#"{"jsonrpc": "2.0", "id": 1, "method": "list_panes"}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.params, serde_json::Value::Null);
    }

    #[test]
    fn parse_request_without_jsonrpc_uses_default() {
        let json = r#"{"id": 1, "method": "list_panes", "params": {}}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.id, Some(1));
        assert_eq!(req.method, "list_panes");
    }

    #[test]
    fn serialize_response_omits_none_fields() {
        let resp = JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: Some(1),
            result: Some(serde_json::json!({"panes": []})),
            error: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"id\":1"));
        assert!(json.contains("\"result\""));
        assert!(!json.contains("\"error\""));
    }

    #[test]
    fn serialize_error_response_omits_none_fields() {
        let resp = JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: None,
            result: None,
            error: Some(JsonRpcError {
                code: -32601,
                message: "method not found".into(),
            }),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("-32601"));
        assert!(!json.contains("\"result\""));
        assert!(!json.contains("\"id\""));
    }

    #[test]
    fn serialize_notification() {
        let notif = JsonRpcNotification {
            jsonrpc: "2.0".into(),
            method: "state_changed".into(),
            params: serde_json::json!({ "pane_id": "%1" }),
        };
        let json = serde_json::to_string(&notif).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"method\":\"state_changed\""));
        assert!(json.contains("\"pane_id\":\"%1\""));
    }

    #[test]
    fn summary_counts_from_empty_state() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let state: SharedState = Arc::new(RwLock::new(DaemonState::default()));
        let counts = rt.block_on(compute_summary_counts(&state));
        assert!(counts.is_empty());
    }

    #[test]
    fn summary_counts_aggregation() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let state: SharedState = Arc::new(RwLock::new(DaemonState {
            panes: vec![
                PaneInfo {
                    pane_id: "%1".into(),
                    session_name: "s".into(),
                    window_id: "@1".into(),
                    pane_title: "".into(),
                    current_cmd: "claude".into(),
                    provider: Some("claude".into()),
                    provider_confidence: 0.95,
                    activity_state: "running".into(),
                    activity_confidence: 0.9,
                    activity_source: "hook".into(),
                    attention_state: "none".into(),
                    attention_reason: "".into(),
                    attention_since: None,
                    updated_at: "2026-01-01T00:00:00Z".into(),
                },
                PaneInfo {
                    pane_id: "%2".into(),
                    session_name: "s".into(),
                    window_id: "@1".into(),
                    pane_title: "".into(),
                    current_cmd: "codex".into(),
                    provider: Some("codex".into()),
                    provider_confidence: 0.9,
                    activity_state: "running".into(),
                    activity_confidence: 0.85,
                    activity_source: "poller".into(),
                    attention_state: "none".into(),
                    attention_reason: "".into(),
                    attention_since: None,
                    updated_at: "2026-01-01T00:00:00Z".into(),
                },
                PaneInfo {
                    pane_id: "%3".into(),
                    session_name: "s".into(),
                    window_id: "@2".into(),
                    pane_title: "".into(),
                    current_cmd: "bash".into(),
                    provider: None,
                    provider_confidence: 0.0,
                    activity_state: "idle".into(),
                    activity_confidence: 0.5,
                    activity_source: "poller".into(),
                    attention_state: "none".into(),
                    attention_reason: "".into(),
                    attention_since: None,
                    updated_at: "2026-01-01T00:00:00Z".into(),
                },
            ],
        }));

        let counts = rt.block_on(compute_summary_counts(&state));
        assert_eq!(counts.get("running"), Some(&2));
        assert_eq!(counts.get("idle"), Some(&1));
        assert_eq!(counts.len(), 2);
    }

    #[test]
    fn notification_to_push_state_changed() {
        use crate::orchestrator::PaneState;
        use agtmux_core::engine::ResolvedActivity;
        use agtmux_core::types::*;
        use chrono::Utc;

        let notif = StateNotification::StateChanged {
            pane_id: "%1".into(),
            state: PaneState {
                pane_id: "%1".into(),
                provider: Some(Provider::Claude),
                provider_confidence: 0.0,
                activity: ResolvedActivity {
                    state: ActivityState::Running,
                    confidence: 0.9,
                    source: SourceType::Hook,
                    reason_code: "running".into(),
                },
                attention: AttentionResult {
                    state: AttentionState::None,
                    reason: "".into(),
                    since: None,
                },
                last_event_type: "".into(),
                updated_at: Utc::now(),
            },
        };

        // Subscribed to "state" => should produce a push
        let events = vec!["state".to_string()];
        let push = notification_to_push(&notif, &events);
        assert!(push.is_some());
        assert_eq!(push.unwrap().method, "state_changed");

        // Not subscribed to "state" => should produce None
        let events = vec!["topology".to_string()];
        let push = notification_to_push(&notif, &events);
        assert!(push.is_none());
    }

    #[test]
    fn notification_to_push_topology_events() {
        let added = StateNotification::PaneAdded {
            pane_id: "%1".into(),
        };
        let removed = StateNotification::PaneRemoved {
            pane_id: "%2".into(),
        };

        let topo_events = vec!["topology".to_string()];
        let state_events = vec!["state".to_string()];

        // PaneAdded with topology subscription
        let push = notification_to_push(&added, &topo_events);
        assert!(push.is_some());
        assert_eq!(push.unwrap().method, "pane_added");

        // PaneAdded without topology subscription
        let push = notification_to_push(&added, &state_events);
        assert!(push.is_none());

        // PaneRemoved with topology subscription
        let push = notification_to_push(&removed, &topo_events);
        assert!(push.is_some());
        assert_eq!(push.unwrap().method, "pane_removed");

        // PaneRemoved without topology subscription
        let push = notification_to_push(&removed, &state_events);
        assert!(push.is_none());
    }

    #[test]
    fn notification_to_push_empty_subscriptions() {
        let notif = StateNotification::PaneAdded {
            pane_id: "%1".into(),
        };
        let empty: Vec<String> = vec![];
        let push = notification_to_push(&notif, &empty);
        assert!(push.is_none());
    }

    #[test]
    fn pane_info_round_trip() {
        let info = PaneInfo {
            pane_id: "%1".into(),
            session_name: "main".into(),
            window_id: "@1".into(),
            pane_title: "claude".into(),
            current_cmd: "claude".into(),
            provider: Some("claude".into()),
            provider_confidence: 0.95,
            activity_state: "waiting_input".into(),
            activity_confidence: 0.92,
            activity_source: "hook".into(),
            attention_state: "action_required_input".into(),
            attention_reason: "needs input".into(),
            attention_since: Some("2026-01-01T00:00:00Z".into()),
            updated_at: "2026-01-01T00:00:01Z".into(),
        };

        let json = serde_json::to_string(&info).unwrap();
        let parsed: PaneInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.pane_id, "%1");
        assert_eq!(parsed.provider, Some("claude".into()));
        assert_eq!(parsed.activity_state, "waiting_input");
        assert_eq!(parsed.attention_since, Some("2026-01-01T00:00:00Z".into()));
    }
}

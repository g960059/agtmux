use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use crate::orchestrator::StateNotification;
use crate::server::{
    JsonRpcError, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, SharedState,
};

// ---------------------------------------------------------------------------
// Subscribe params (mirrors server.rs)
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
struct SubscribeParams {
    #[serde(default)]
    events: Vec<String>,
}

// ---------------------------------------------------------------------------
// WsServer
// ---------------------------------------------------------------------------

/// WebSocket server exposing the same JSON-RPC 2.0 protocol as `DaemonServer`.
///
/// Transports newline-delimited JSON-RPC messages over WebSocket text frames
/// rather than Unix stream sockets.
pub struct WsServer {
    addr: SocketAddr,
    state: SharedState,
    notify_tx: broadcast::Sender<StateNotification>,
    cancel: CancellationToken,
}

impl WsServer {
    pub fn new(
        addr: SocketAddr,
        state: SharedState,
        notify_tx: broadcast::Sender<StateNotification>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            addr,
            state,
            notify_tx,
            cancel,
        }
    }

    /// Run the WebSocket server: bind TCP, accept connections, and spawn
    /// per-client handlers until the cancellation token fires.
    pub async fn run(&self) -> std::io::Result<()> {
        let listener = TcpListener::bind(self.addr).await?;
        tracing::info!(addr = %self.addr, "ws server listening");

        loop {
            tokio::select! {
                result = listener.accept() => {
                    match result {
                        Ok((stream, peer)) => {
                            tracing::debug!(peer = %peer, "ws: TCP connection accepted");
                            let state = Arc::clone(&self.state);
                            let notify_rx = self.notify_tx.subscribe();
                            let cancel = self.cancel.clone();
                            tokio::spawn(async move {
                                match tokio_tungstenite::accept_async(stream).await {
                                    Ok(ws_stream) => {
                                        if let Err(e) = handle_ws_client(ws_stream, state, notify_rx, cancel).await {
                                            tracing::debug!(peer = %peer, error = %e, "ws client handler finished with error");
                                        }
                                    }
                                    Err(e) => {
                                        tracing::debug!(peer = %peer, error = %e, "ws handshake failed");
                                    }
                                }
                            });
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "ws: TCP accept failed");
                        }
                    }
                }
                _ = self.cancel.cancelled() => {
                    tracing::info!("ws server: cancellation requested, shutting down");
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

async fn handle_ws_client(
    ws_stream: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    state: SharedState,
    mut notify_rx: broadcast::Receiver<StateNotification>,
    cancel: CancellationToken,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (mut ws_tx, mut ws_rx) = ws_stream.split();

    tracing::debug!("ws client connected");

    let mut subscribed_events: Vec<String> = Vec::new();
    let mut subscribed_summary = false;

    loop {
        tokio::select! {
            // --- incoming WebSocket message ---
            msg = ws_rx.next() => {
                let msg = match msg {
                    Some(Ok(m)) => m,
                    Some(Err(e)) => {
                        tracing::debug!(error = %e, "ws read error, dropping client");
                        return Err(e.into());
                    }
                    None => {
                        tracing::debug!("ws client disconnected (stream ended)");
                        return Ok(());
                    }
                };

                let text = match msg {
                    Message::Text(t) => t,
                    Message::Close(_) => {
                        tracing::debug!("ws client sent close frame");
                        return Ok(());
                    }
                    Message::Ping(data) => {
                        ws_tx.send(Message::Pong(data)).await?;
                        continue;
                    }
                    _ => continue,
                };

                let req: JsonRpcRequest = match serde_json::from_str(&text) {
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
                        ws_tx.send(Message::Text(serde_json::to_string(&resp)?)).await?;
                        continue;
                    }
                };

                tracing::debug!(method = %req.method, id = ?req.id, "ws: request received");

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
                        ws_tx.send(Message::Text(serde_json::to_string(&resp)?)).await?;
                    }

                    "subscribe" => {
                        let params: SubscribeParams = serde_json::from_value(req.params)
                            .unwrap_or(SubscribeParams {
                                events: vec!["state".into(), "topology".into()],
                            });
                        subscribed_events = params.events;
                        tracing::debug!(events = ?subscribed_events, "ws client subscribed");

                        let resp = JsonRpcResponse {
                            jsonrpc: "2.0".into(),
                            id: req.id,
                            result: Some(serde_json::json!({ "subscribed": true })),
                            error: None,
                        };
                        ws_tx.send(Message::Text(serde_json::to_string(&resp)?)).await?;
                    }

                    "subscribe_summary" => {
                        subscribed_summary = true;
                        tracing::debug!("ws client subscribed to summary");

                        let resp = JsonRpcResponse {
                            jsonrpc: "2.0".into(),
                            id: req.id,
                            result: Some(serde_json::json!({ "subscribed": true })),
                            error: None,
                        };
                        ws_tx.send(Message::Text(serde_json::to_string(&resp)?)).await?;

                        // Immediate summary snapshot.
                        let counts = compute_summary_counts(&state).await;
                        let notif = JsonRpcNotification {
                            jsonrpc: "2.0".into(),
                            method: "summary".into(),
                            params: serde_json::json!({ "counts": counts }),
                        };
                        ws_tx.send(Message::Text(serde_json::to_string(&notif)?)).await?;
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
                        ws_tx.send(Message::Text(serde_json::to_string(&resp)?)).await?;
                    }
                }
            }

            // --- push notification from orchestrator ---
            notification = notify_rx.recv() => {
                let notification = match notification {
                    Ok(n) => n,
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(skipped, "ws client lagged, dropped notifications");
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        tracing::debug!("ws notification channel closed, dropping client");
                        return Ok(());
                    }
                };

                if !subscribed_events.is_empty() {
                    if let Some(notif) = notification_to_push(&notification, &subscribed_events) {
                        let text = serde_json::to_string(&notif)?;
                        ws_tx.send(Message::Text(text)).await?;
                    }
                }

                if subscribed_summary {
                    let counts = compute_summary_counts(&state).await;
                    let notif = JsonRpcNotification {
                        jsonrpc: "2.0".into(),
                        method: "summary".into(),
                        params: serde_json::json!({ "counts": counts }),
                    };
                    let text = serde_json::to_string(&notif)?;
                    ws_tx.send(Message::Text(text)).await?;
                }
            }

            // --- cancellation ---
            _ = cancel.cancelled() => {
                tracing::debug!("ws client handler: cancellation requested");
                let _ = ws_tx.send(Message::Close(None)).await;
                return Ok(());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Notification mapping (mirrors server.rs logic)
// ---------------------------------------------------------------------------

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

async fn compute_summary_counts(state: &SharedState) -> HashMap<String, usize> {
    let s = state.read().await;
    let mut counts: HashMap<String, usize> = HashMap::new();
    for pane in &s.panes {
        *counts.entry(pane.activity_state.clone()).or_insert(0) += 1;
    }
    counts
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::{DaemonState, PaneInfo};
    use std::sync::Arc;
    use tokio::sync::{broadcast, RwLock};

    fn make_shared_state(panes: Vec<PaneInfo>) -> SharedState {
        Arc::new(RwLock::new(DaemonState { panes }))
    }

    fn sample_pane(id: &str, activity_state: &str) -> PaneInfo {
        PaneInfo {
            pane_id: id.into(),
            session_name: "s".into(),
            window_id: "@1".into(),
            pane_title: "".into(),
            current_cmd: "claude".into(),
            provider: Some("claude".into()),
            provider_confidence: 0.95,
            activity_state: activity_state.into(),
            activity_confidence: 0.9,
            activity_source: "hook".into(),
            attention_state: "none".into(),
            attention_reason: "".into(),
            attention_since: None,
            updated_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    #[test]
    fn ws_server_can_be_constructed() {
        let state = make_shared_state(vec![]);
        let (notify_tx, _) = broadcast::channel(16);
        let cancel = CancellationToken::new();
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();

        let server = WsServer::new(addr, state, notify_tx, cancel);
        assert_eq!(server.addr, addr);
    }

    #[test]
    fn json_rpc_list_panes_request_response_parsing() {
        // Parse a list_panes request
        let json = r#"{"jsonrpc": "2.0", "id": 1, "method": "list_panes", "params": {}}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.method, "list_panes");
        assert_eq!(req.id, Some(1));

        // Build the response as the handler would
        let panes = vec![sample_pane("%1", "running")];
        let resp = JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: req.id,
            result: Some(serde_json::json!({ "panes": panes })),
            error: None,
        };

        let serialized = serde_json::to_string(&resp).unwrap();
        assert!(serialized.contains("\"panes\""));
        assert!(serialized.contains("\"%1\""));

        // Verify we can round-trip through JSON (as WebSocket text frames)
        let parsed: serde_json::Value = serde_json::from_str(&serialized).unwrap();
        assert_eq!(parsed["jsonrpc"], "2.0");
        assert_eq!(parsed["id"], 1);
        assert!(parsed["result"]["panes"].is_array());
    }

    #[test]
    fn subscribe_request_returns_acknowledgement() {
        let json = r#"{"jsonrpc": "2.0", "id": 5, "method": "subscribe", "params": {"events": ["state"]}}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.method, "subscribe");

        let params: SubscribeParams = serde_json::from_value(req.params).unwrap();
        assert_eq!(params.events, vec!["state"]);

        // Build the acknowledgement response
        let resp = JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: req.id,
            result: Some(serde_json::json!({ "subscribed": true })),
            error: None,
        };

        let serialized = serde_json::to_string(&resp).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&serialized).unwrap();
        assert_eq!(parsed["id"], 5);
        assert_eq!(parsed["result"]["subscribed"], true);
    }

    #[test]
    fn notification_serialization_for_ws_frames() {
        // StateChanged notification
        let notif = JsonRpcNotification {
            jsonrpc: "2.0".into(),
            method: "state_changed".into(),
            params: serde_json::json!({
                "pane_id": "%1",
                "state": { "activity_state": "running" },
            }),
        };

        let text = serde_json::to_string(&notif).unwrap();
        // Verify it can be sent as a WebSocket text frame (valid UTF-8 string)
        assert!(!text.is_empty());
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed["method"], "state_changed");
        assert_eq!(parsed["params"]["pane_id"], "%1");

        // PaneAdded notification
        let notif2 = JsonRpcNotification {
            jsonrpc: "2.0".into(),
            method: "pane_added".into(),
            params: serde_json::json!({ "pane_id": "%2" }),
        };
        let text2 = serde_json::to_string(&notif2).unwrap();
        let parsed2: serde_json::Value = serde_json::from_str(&text2).unwrap();
        assert_eq!(parsed2["method"], "pane_added");

        // Summary notification
        let mut counts = HashMap::new();
        counts.insert("running".to_string(), 2usize);
        counts.insert("idle".to_string(), 1usize);
        let summary_notif = JsonRpcNotification {
            jsonrpc: "2.0".into(),
            method: "summary".into(),
            params: serde_json::json!({ "counts": counts }),
        };
        let text3 = serde_json::to_string(&summary_notif).unwrap();
        let parsed3: serde_json::Value = serde_json::from_str(&text3).unwrap();
        assert_eq!(parsed3["method"], "summary");
        assert_eq!(parsed3["params"]["counts"]["running"], 2);
    }

    #[tokio::test]
    async fn cancel_token_stops_server() {
        let state = make_shared_state(vec![]);
        let (notify_tx, _) = broadcast::channel(16);
        let cancel = CancellationToken::new();

        // Bind to port 0 so the OS assigns an ephemeral port.
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let server = WsServer::new(addr, state, notify_tx, cancel.clone());

        // We need to actually bind before cancelling so the server task enters
        // its loop. We spawn the run task and cancel shortly after.
        let handle = tokio::spawn(async move { server.run().await });

        // Give it a moment to bind and enter the accept loop.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Cancel the server.
        cancel.cancel();

        // The run() should return Ok(()) within a reasonable time.
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
        assert!(result.is_ok(), "server should have stopped within timeout");
        let inner = result.unwrap().unwrap();
        assert!(inner.is_ok(), "server run should return Ok on cancellation");
    }

    #[test]
    fn notification_to_push_filters_correctly() {
        let added = StateNotification::PaneAdded {
            pane_id: "%1".into(),
        };

        // Subscribed to topology => should produce push
        let topo = vec!["topology".to_string()];
        assert!(notification_to_push(&added, &topo).is_some());

        // Not subscribed => no push
        let state_only = vec!["state".to_string()];
        assert!(notification_to_push(&added, &state_only).is_none());

        // Empty subscription => no push
        let empty: Vec<String> = vec![];
        assert!(notification_to_push(&added, &empty).is_none());
    }

    #[tokio::test]
    async fn ws_compute_summary_counts() {
        let state = make_shared_state(vec![
            sample_pane("%1", "running"),
            sample_pane("%2", "running"),
            sample_pane("%3", "idle"),
        ]);
        let counts = compute_summary_counts(&state).await;
        assert_eq!(counts.get("running"), Some(&2));
        assert_eq!(counts.get("idle"), Some(&1));
        assert_eq!(counts.len(), 2);
    }
}

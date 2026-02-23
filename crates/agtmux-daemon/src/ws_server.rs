use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, Semaphore};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use crate::orchestrator::StateNotification;
use crate::server::{
    compute_summary_counts, notification_to_push, JsonRpcError, JsonRpcNotification,
    JsonRpcRequest, JsonRpcResponse, SharedState, SubscribeParams,
};
use crate::terminal_output::{
    encode_output_frame, resize_pane, send_keys, OutputBroadcaster,
};

// ---------------------------------------------------------------------------
// Origin validation
// ---------------------------------------------------------------------------

/// Validate the `Origin` header on an incoming WebSocket upgrade request.
///
/// Allowed origins:
/// - `tauri://localhost` (Tauri desktop app)
/// - `http://localhost:*` or `http://127.0.0.1:*` (local dev)
/// - `null` (file:// contexts)
/// - Absent origin header (non-browser clients like curl, native apps)
///
/// All other origins are rejected with HTTP 403.
fn validate_origin(
    req: &tokio_tungstenite::tungstenite::handshake::server::Request,
    resp: tokio_tungstenite::tungstenite::handshake::server::Response,
) -> Result<
    tokio_tungstenite::tungstenite::handshake::server::Response,
    tokio_tungstenite::tungstenite::handshake::server::ErrorResponse,
> {
    if let Some(origin) = req.headers().get("origin") {
        let origin_str = origin.to_str().unwrap_or("");
        if origin_str == "null"
            || origin_str.starts_with("tauri://")
            || origin_str.starts_with("http://localhost")
            || origin_str.starts_with("http://127.0.0.1")
        {
            return Ok(resp);
        }
        tracing::warn!(origin = %origin_str, "ws: rejected connection from disallowed origin");
        let err_resp = http::Response::builder()
            .status(http::StatusCode::FORBIDDEN)
            .body(Some("Origin not allowed".into()))
            .expect("building error response");
        return Err(err_resp);
    }
    // No origin header = non-browser client (curl, native app), allow.
    Ok(resp)
}

// ---------------------------------------------------------------------------
// WsServer
// ---------------------------------------------------------------------------

/// Default maximum number of concurrent WebSocket connections.
const DEFAULT_MAX_CONNECTIONS: usize = 64;

/// WebSocket server exposing the same JSON-RPC 2.0 protocol as `DaemonServer`.
///
/// Transports newline-delimited JSON-RPC messages over WebSocket text frames
/// rather than Unix stream sockets. Terminal output is streamed as binary
/// frames for xterm.js consumption.
pub struct WsServer {
    addr: SocketAddr,
    state: SharedState,
    notify_tx: broadcast::Sender<StateNotification>,
    cancel: CancellationToken,
    max_connections: usize,
    output_broadcaster: Arc<OutputBroadcaster>,
}

impl WsServer {
    pub fn new(
        addr: SocketAddr,
        state: SharedState,
        notify_tx: broadcast::Sender<StateNotification>,
        cancel: CancellationToken,
    ) -> Self {
        let (broadcaster, _rx) = OutputBroadcaster::new();
        Self {
            addr,
            state,
            notify_tx,
            cancel,
            max_connections: DEFAULT_MAX_CONNECTIONS,
            output_broadcaster: Arc::new(broadcaster),
        }
    }

    pub fn with_output_broadcaster(
        addr: SocketAddr,
        state: SharedState,
        notify_tx: broadcast::Sender<StateNotification>,
        cancel: CancellationToken,
        output_broadcaster: Arc<OutputBroadcaster>,
    ) -> Self {
        Self {
            addr,
            state,
            notify_tx,
            cancel,
            max_connections: DEFAULT_MAX_CONNECTIONS,
            output_broadcaster,
        }
    }

    /// Set the maximum number of concurrent WebSocket connections.
    #[allow(dead_code)]
    pub fn with_max_connections(mut self, max: usize) -> Self {
        self.max_connections = max;
        self
    }

    /// Run the WebSocket server: bind TCP, accept connections, and spawn
    /// per-client handlers until the cancellation token fires.
    pub async fn run(&self) -> std::io::Result<()> {
        let listener = TcpListener::bind(self.addr).await?;
        tracing::info!(addr = %self.addr, max_connections = self.max_connections, "ws server listening");
        self.serve(listener).await
    }

    /// Bind to the configured address and return the actual local address.
    /// Useful when binding to port 0 to get an OS-assigned ephemeral port.
    pub async fn bind(&self) -> std::io::Result<(TcpListener, SocketAddr)> {
        let listener = TcpListener::bind(self.addr).await?;
        let local_addr = listener.local_addr()?;
        tracing::info!(addr = %local_addr, max_connections = self.max_connections, "ws server bound");
        Ok((listener, local_addr))
    }

    /// Run the accept loop on a pre-bound listener.
    pub async fn serve(&self, listener: TcpListener) -> std::io::Result<()> {
        let semaphore = Arc::new(Semaphore::new(self.max_connections));

        loop {
            tokio::select! {
                result = listener.accept() => {
                    match result {
                        Ok((stream, peer)) => {
                            let permit = match semaphore.clone().try_acquire_owned() {
                                Ok(permit) => permit,
                                Err(_) => {
                                    tracing::warn!(
                                        peer = %peer,
                                        max = self.max_connections,
                                        "ws: connection limit reached, rejecting"
                                    );
                                    drop(stream);
                                    continue;
                                }
                            };
                            tracing::debug!(peer = %peer, "ws: TCP connection accepted");
                            let state = Arc::clone(&self.state);
                            let notify_rx = self.notify_tx.subscribe();
                            let cancel = self.cancel.clone();
                            let broadcaster = Arc::clone(&self.output_broadcaster);
                            tokio::spawn(async move {
                                let _permit = permit;
                                match tokio_tungstenite::accept_hdr_async(stream, validate_origin).await {
                                    Ok(ws_stream) => {
                                        if let Err(e) = handle_ws_client(ws_stream, state, notify_rx, cancel, broadcaster).await {
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
    broadcaster: Arc<OutputBroadcaster>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (mut ws_tx, mut ws_rx) = ws_stream.split();

    tracing::debug!("ws client connected");

    let mut subscribed_events: Vec<String> = Vec::new();
    let mut subscribed_summary = false;
    let mut subscribed_panes: HashSet<String> = HashSet::new();
    let mut output_rx = broadcaster.subscribe_receiver();

    loop {
        tokio::select! {
            // --- incoming WebSocket message ---
            msg = ws_rx.next() => {
                let msg = match msg {
                    Some(Ok(m)) => m,
                    Some(Err(e)) => {
                        tracing::debug!(error = %e, "ws read error, dropping client");
                        broadcaster.unsubscribe_all(&subscribed_panes).await;
                        return Err(e.into());
                    }
                    None => {
                        tracing::debug!("ws client disconnected (stream ended)");
                        broadcaster.unsubscribe_all(&subscribed_panes).await;
                        return Ok(());
                    }
                };

                let text = match msg {
                    Message::Text(t) => t,
                    Message::Close(_) => {
                        tracing::debug!("ws client sent close frame");
                        broadcaster.unsubscribe_all(&subscribed_panes).await;
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

                    "subscribe_output" => {
                        let pane_id = req.params.get("pane_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();

                        if pane_id.is_empty() {
                            let resp = JsonRpcResponse {
                                jsonrpc: "2.0".into(),
                                id: req.id,
                                result: None,
                                error: Some(JsonRpcError {
                                    code: -32602,
                                    message: "missing required param: pane_id".into(),
                                }),
                            };
                            ws_tx.send(Message::Text(serde_json::to_string(&resp)?)).await?;
                        } else {
                            broadcaster.subscribe_pane(&pane_id).await;
                            subscribed_panes.insert(pane_id.clone());
                            tracing::debug!(pane_id = %pane_id, "ws client subscribed to output");

                            let resp = JsonRpcResponse {
                                jsonrpc: "2.0".into(),
                                id: req.id,
                                result: Some(serde_json::json!({ "subscribed": true, "pane_id": pane_id })),
                                error: None,
                            };
                            ws_tx.send(Message::Text(serde_json::to_string(&resp)?)).await?;
                        }
                    }

                    "unsubscribe_output" => {
                        let pane_id = req.params.get("pane_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();

                        if pane_id.is_empty() {
                            let resp = JsonRpcResponse {
                                jsonrpc: "2.0".into(),
                                id: req.id,
                                result: None,
                                error: Some(JsonRpcError {
                                    code: -32602,
                                    message: "missing required param: pane_id".into(),
                                }),
                            };
                            ws_tx.send(Message::Text(serde_json::to_string(&resp)?)).await?;
                        } else {
                            broadcaster.unsubscribe_pane(&pane_id).await;
                            subscribed_panes.remove(&pane_id);
                            tracing::debug!(pane_id = %pane_id, "ws client unsubscribed from output");

                            let resp = JsonRpcResponse {
                                jsonrpc: "2.0".into(),
                                id: req.id,
                                result: Some(serde_json::json!({ "unsubscribed": true, "pane_id": pane_id })),
                                error: None,
                            };
                            ws_tx.send(Message::Text(serde_json::to_string(&resp)?)).await?;
                        }
                    }

                    "write_input" => {
                        let pane_id = req.params.get("pane_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let data = req.params.get("data")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");

                        if pane_id.is_empty() {
                            let resp = JsonRpcResponse {
                                jsonrpc: "2.0".into(),
                                id: req.id,
                                result: None,
                                error: Some(JsonRpcError {
                                    code: -32602,
                                    message: "missing required param: pane_id".into(),
                                }),
                            };
                            ws_tx.send(Message::Text(serde_json::to_string(&resp)?)).await?;
                        } else {
                            match send_keys(pane_id, data).await {
                                Ok(()) => {
                                    let resp = JsonRpcResponse {
                                        jsonrpc: "2.0".into(),
                                        id: req.id,
                                        result: Some(serde_json::json!({ "ok": true })),
                                        error: None,
                                    };
                                    ws_tx.send(Message::Text(serde_json::to_string(&resp)?)).await?;
                                }
                                Err(e) => {
                                    let resp = JsonRpcResponse {
                                        jsonrpc: "2.0".into(),
                                        id: req.id,
                                        result: None,
                                        error: Some(JsonRpcError {
                                            code: -32000,
                                            message: e,
                                        }),
                                    };
                                    ws_tx.send(Message::Text(serde_json::to_string(&resp)?)).await?;
                                }
                            }
                        }
                    }

                    "resize_pane" => {
                        let pane_id = req.params.get("pane_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let cols = req.params.get("cols")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as u16;
                        let rows = req.params.get("rows")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as u16;

                        if pane_id.is_empty() || cols == 0 || rows == 0 {
                            let resp = JsonRpcResponse {
                                jsonrpc: "2.0".into(),
                                id: req.id,
                                result: None,
                                error: Some(JsonRpcError {
                                    code: -32602,
                                    message: "missing required params: pane_id, cols, rows".into(),
                                }),
                            };
                            ws_tx.send(Message::Text(serde_json::to_string(&resp)?)).await?;
                        } else {
                            match resize_pane(pane_id, cols, rows).await {
                                Ok(()) => {
                                    let resp = JsonRpcResponse {
                                        jsonrpc: "2.0".into(),
                                        id: req.id,
                                        result: Some(serde_json::json!({ "ok": true })),
                                        error: None,
                                    };
                                    ws_tx.send(Message::Text(serde_json::to_string(&resp)?)).await?;
                                }
                                Err(e) => {
                                    let resp = JsonRpcResponse {
                                        jsonrpc: "2.0".into(),
                                        id: req.id,
                                        result: None,
                                        error: Some(JsonRpcError {
                                            code: -32000,
                                            message: e,
                                        }),
                                    };
                                    ws_tx.send(Message::Text(serde_json::to_string(&resp)?)).await?;
                                }
                            }
                        }
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

            // --- terminal output from PaneTap ---
            output = output_rx.recv() => {
                let output = match output {
                    Ok(o) => o,
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(skipped, "ws client output lagged");
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        continue;
                    }
                };
                if subscribed_panes.contains(&output.pane_id) {
                    let frame = encode_output_frame(&output.pane_id, &output.data);
                    ws_tx.send(Message::Binary(frame)).await?;
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
                        broadcaster.unsubscribe_all(&subscribed_panes).await;
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
                broadcaster.unsubscribe_all(&subscribed_panes).await;
                let _ = ws_tx.send(Message::Close(None)).await;
                return Ok(());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::{DaemonState, PaneInfo};
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;
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

    struct TestServer {
        addr: SocketAddr,
        cancel: CancellationToken,
        _handle: tokio::task::JoinHandle<std::io::Result<()>>,
    }

    async fn start_test_server(
        panes: Vec<PaneInfo>,
        notify_tx: broadcast::Sender<StateNotification>,
        max_connections: Option<usize>,
    ) -> TestServer {
        let state = make_shared_state(panes);
        let cancel = CancellationToken::new();
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let mut server = WsServer::new(addr, state, notify_tx, cancel.clone());
        if let Some(max) = max_connections {
            server = server.with_max_connections(max);
        }
        let (listener, local_addr) = server.bind().await.unwrap();
        let handle = tokio::spawn(async move { server.serve(listener).await });
        TestServer {
            addr: local_addr,
            cancel,
            _handle: handle,
        }
    }

    impl TestServer {
        fn ws_url(&self) -> String {
            format!("ws://127.0.0.1:{}", self.addr.port())
        }

        async fn connect(&self) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
            let (ws, _) = tokio_tungstenite::connect_async(&self.ws_url()).await.unwrap();
            ws
        }

        async fn connect_with_origin(&self, origin: &str) -> Result<
            tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
            tokio_tungstenite::tungstenite::Error,
        > {
            let req = tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(
                &self.ws_url(),
            )
            .unwrap();
            let mut req = req;
            req.headers_mut().insert(
                "Origin",
                origin.parse().unwrap(),
            );
            let (ws, _) = tokio_tungstenite::connect_async(req).await?;
            Ok(ws)
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.cancel.cancel();
        }
    }

    async fn send_rpc(
        ws: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
        id: u64,
        method: &str,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        ws.send(Message::Text(req.to_string())).await.unwrap();
        let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("timeout waiting for response")
            .expect("stream ended")
            .expect("read error");
        let Message::Text(text) = msg else {
            panic!("expected text frame, got {:?}", msg);
        };
        serde_json::from_str(&text).unwrap()
    }

    async fn recv_notification(
        ws: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    ) -> serde_json::Value {
        let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("timeout waiting for notification")
            .expect("stream ended")
            .expect("read error");
        let Message::Text(text) = msg else {
            panic!("expected text frame, got {:?}", msg);
        };
        serde_json::from_str(&text).unwrap()
    }

    // -----------------------------------------------------------------------
    // Unit tests (existing)
    // -----------------------------------------------------------------------

    #[test]
    fn ws_server_can_be_constructed() {
        let state = make_shared_state(vec![]);
        let (notify_tx, _) = broadcast::channel(16);
        let cancel = CancellationToken::new();
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();

        let server = WsServer::new(addr, state, notify_tx, cancel);
        assert_eq!(server.addr, addr);
        assert_eq!(server.max_connections, DEFAULT_MAX_CONNECTIONS);
    }

    #[test]
    fn ws_server_custom_max_connections() {
        let state = make_shared_state(vec![]);
        let (notify_tx, _) = broadcast::channel(16);
        let cancel = CancellationToken::new();
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();

        let server = WsServer::new(addr, state, notify_tx, cancel).with_max_connections(128);
        assert_eq!(server.max_connections, 128);
    }

    #[test]
    fn json_rpc_list_panes_request_response_parsing() {
        let json = r#"{"jsonrpc": "2.0", "id": 1, "method": "list_panes", "params": {}}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.method, "list_panes");
        assert_eq!(req.id, Some(1));

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
        let notif = JsonRpcNotification {
            jsonrpc: "2.0".into(),
            method: "state_changed".into(),
            params: serde_json::json!({
                "pane_id": "%1",
                "state": { "activity_state": "running" },
            }),
        };

        let text = serde_json::to_string(&notif).unwrap();
        assert!(!text.is_empty());
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed["method"], "state_changed");
        assert_eq!(parsed["params"]["pane_id"], "%1");

        let notif2 = JsonRpcNotification {
            jsonrpc: "2.0".into(),
            method: "pane_added".into(),
            params: serde_json::json!({ "pane_id": "%2" }),
        };
        let text2 = serde_json::to_string(&notif2).unwrap();
        let parsed2: serde_json::Value = serde_json::from_str(&text2).unwrap();
        assert_eq!(parsed2["method"], "pane_added");

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

        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let server = WsServer::new(addr, state, notify_tx, cancel.clone());

        let handle = tokio::spawn(async move { server.run().await });

        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel.cancel();

        let result = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(result.is_ok(), "server should have stopped within timeout");
        let inner = result.unwrap().unwrap();
        assert!(inner.is_ok(), "server run should return Ok on cancellation");
    }

    #[test]
    fn notification_to_push_filters_correctly() {
        let added = StateNotification::PaneAdded {
            pane_id: "%1".into(),
        };

        let topo = vec!["topology".to_string()];
        assert!(notification_to_push(&added, &topo).is_some());

        let state_only = vec!["state".to_string()];
        assert!(notification_to_push(&added, &state_only).is_none());

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

    #[test]
    fn validate_origin_allows_tauri() {
        let req = http::Request::builder()
            .header("origin", "tauri://localhost")
            .body(())
            .unwrap();
        let resp = http::Response::builder()
            .status(http::StatusCode::SWITCHING_PROTOCOLS)
            .body(())
            .unwrap();
        assert!(validate_origin(&req, resp).is_ok());
    }

    #[test]
    fn validate_origin_allows_localhost() {
        let req = http::Request::builder()
            .header("origin", "http://localhost:3000")
            .body(())
            .unwrap();
        let resp = http::Response::builder()
            .status(http::StatusCode::SWITCHING_PROTOCOLS)
            .body(())
            .unwrap();
        assert!(validate_origin(&req, resp).is_ok());
    }

    #[test]
    fn validate_origin_allows_127_0_0_1() {
        let req = http::Request::builder()
            .header("origin", "http://127.0.0.1:9780")
            .body(())
            .unwrap();
        let resp = http::Response::builder()
            .status(http::StatusCode::SWITCHING_PROTOCOLS)
            .body(())
            .unwrap();
        assert!(validate_origin(&req, resp).is_ok());
    }

    #[test]
    fn validate_origin_allows_null() {
        let req = http::Request::builder()
            .header("origin", "null")
            .body(())
            .unwrap();
        let resp = http::Response::builder()
            .status(http::StatusCode::SWITCHING_PROTOCOLS)
            .body(())
            .unwrap();
        assert!(validate_origin(&req, resp).is_ok());
    }

    #[test]
    fn validate_origin_allows_no_origin() {
        let req = http::Request::builder().body(()).unwrap();
        let resp = http::Response::builder()
            .status(http::StatusCode::SWITCHING_PROTOCOLS)
            .body(())
            .unwrap();
        assert!(validate_origin(&req, resp).is_ok());
    }

    #[test]
    fn validate_origin_rejects_remote() {
        let req = http::Request::builder()
            .header("origin", "https://evil.example.com")
            .body(())
            .unwrap();
        let resp = http::Response::builder()
            .status(http::StatusCode::SWITCHING_PROTOCOLS)
            .body(())
            .unwrap();
        let result = validate_origin(&req, resp);
        assert!(result.is_err());
        let err_resp = result.unwrap_err();
        assert_eq!(err_resp.status(), http::StatusCode::FORBIDDEN);
    }

    // -----------------------------------------------------------------------
    // Integration tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn list_panes_over_ws() {
        let (notify_tx, _) = broadcast::channel(16);
        let server = start_test_server(
            vec![sample_pane("%1", "running"), sample_pane("%2", "idle")],
            notify_tx,
            None,
        )
        .await;

        let mut ws = server.connect().await;
        let resp = send_rpc(&mut ws, 1, "list_panes", serde_json::json!({})).await;

        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], 1);
        let panes = resp["result"]["panes"].as_array().unwrap();
        assert_eq!(panes.len(), 2);
        assert_eq!(panes[0]["pane_id"], "%1");
        assert_eq!(panes[0]["activity_state"], "running");
        assert_eq!(panes[1]["pane_id"], "%2");
        assert_eq!(panes[1]["activity_state"], "idle");
    }

    #[tokio::test]
    async fn list_panes_empty_state() {
        let (notify_tx, _) = broadcast::channel(16);
        let server = start_test_server(vec![], notify_tx, None).await;

        let mut ws = server.connect().await;
        let resp = send_rpc(&mut ws, 42, "list_panes", serde_json::json!({})).await;

        assert_eq!(resp["id"], 42);
        let panes = resp["result"]["panes"].as_array().unwrap();
        assert!(panes.is_empty());
    }

    #[tokio::test]
    async fn unknown_method_returns_error() {
        let (notify_tx, _) = broadcast::channel(16);
        let server = start_test_server(vec![], notify_tx, None).await;

        let mut ws = server.connect().await;
        let resp = send_rpc(&mut ws, 99, "nonexistent", serde_json::json!({})).await;

        assert_eq!(resp["id"], 99);
        assert!(resp["result"].is_null());
        assert_eq!(resp["error"]["code"], -32601);
        assert!(resp["error"]["message"].as_str().unwrap().contains("nonexistent"));
    }

    #[tokio::test]
    async fn subscribe_and_receive_topology_notification() {
        let (notify_tx, _) = broadcast::channel(16);
        let server = start_test_server(vec![], notify_tx.clone(), None).await;

        let mut ws = server.connect().await;

        let resp = send_rpc(
            &mut ws,
            1,
            "subscribe",
            serde_json::json!({"events": ["topology"]}),
        )
        .await;
        assert_eq!(resp["result"]["subscribed"], true);

        notify_tx
            .send(StateNotification::PaneAdded {
                pane_id: "%5".into(),
            })
            .unwrap();

        let notif = recv_notification(&mut ws).await;
        assert_eq!(notif["method"], "pane_added");
        assert_eq!(notif["params"]["pane_id"], "%5");
    }

    #[tokio::test]
    async fn subscribe_and_receive_state_notification() {
        use crate::orchestrator::PaneState;
        use agtmux_core::engine::ResolvedActivity;
        use agtmux_core::types::*;
        use chrono::Utc;

        let (notify_tx, _) = broadcast::channel(16);
        let server = start_test_server(vec![], notify_tx.clone(), None).await;

        let mut ws = server.connect().await;

        let resp = send_rpc(
            &mut ws,
            1,
            "subscribe",
            serde_json::json!({"events": ["state"]}),
        )
        .await;
        assert_eq!(resp["result"]["subscribed"], true);

        notify_tx
            .send(StateNotification::StateChanged {
                pane_id: "%1".into(),
                state: PaneState {
                    pane_id: "%1".into(),
                    provider: Some(Provider::Claude),
                    provider_confidence: 0.95,
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
            })
            .unwrap();

        let notif = recv_notification(&mut ws).await;
        assert_eq!(notif["method"], "state_changed");
        assert_eq!(notif["params"]["pane_id"], "%1");
    }

    #[tokio::test]
    async fn subscribe_filters_unsubscribed_events() {
        let (notify_tx, _) = broadcast::channel(16);
        let server = start_test_server(vec![], notify_tx.clone(), None).await;

        let mut ws = server.connect().await;

        send_rpc(
            &mut ws,
            1,
            "subscribe",
            serde_json::json!({"events": ["state"]}),
        )
        .await;

        // Topology event should not be forwarded to a state-only subscriber.
        notify_tx
            .send(StateNotification::PaneAdded {
                pane_id: "%1".into(),
            })
            .unwrap();

        // Send a follow-up list_panes to prove the connection is still alive
        // and the topology event was silently filtered.
        let resp = send_rpc(&mut ws, 2, "list_panes", serde_json::json!({})).await;
        assert_eq!(resp["id"], 2);
    }

    #[tokio::test]
    async fn subscribe_summary_sends_immediate_snapshot() {
        let (notify_tx, _) = broadcast::channel(16);
        let server = start_test_server(
            vec![sample_pane("%1", "running"), sample_pane("%2", "idle")],
            notify_tx,
            None,
        )
        .await;

        let mut ws = server.connect().await;

        let resp = send_rpc(&mut ws, 1, "subscribe_summary", serde_json::json!({})).await;
        assert_eq!(resp["result"]["subscribed"], true);

        let notif = recv_notification(&mut ws).await;
        assert_eq!(notif["method"], "summary");
        assert_eq!(notif["params"]["counts"]["running"], 1);
        assert_eq!(notif["params"]["counts"]["idle"], 1);
    }

    #[tokio::test]
    async fn origin_localhost_accepted() {
        let (notify_tx, _) = broadcast::channel(16);
        let server = start_test_server(vec![], notify_tx, None).await;

        let mut ws = server.connect_with_origin("http://localhost:3000").await.unwrap();
        let resp = send_rpc(&mut ws, 1, "list_panes", serde_json::json!({})).await;
        assert_eq!(resp["id"], 1);
    }

    #[tokio::test]
    async fn origin_remote_rejected() {
        let (notify_tx, _) = broadcast::channel(16);
        let server = start_test_server(vec![], notify_tx, None).await;

        let result = server.connect_with_origin("https://evil.example.com").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn connection_limit_enforced() {
        let (notify_tx, _) = broadcast::channel(16);
        let server = start_test_server(vec![], notify_tx, Some(2)).await;

        let _ws1 = server.connect().await;
        let _ws2 = server.connect().await;

        // Third connection should be rejected. The server drops the TCP stream,
        // so the WS handshake will fail.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let result = tokio::time::timeout(Duration::from_secs(2), async {
            tokio_tungstenite::connect_async(&server.ws_url()).await
        })
        .await;

        match result {
            Ok(Ok((mut ws, _))) => {
                // Connection may have been accepted at TCP level before the
                // server dropped it. Sending a message should fail.
                let send_result = ws
                    .send(Message::Text(
                        r#"{"jsonrpc":"2.0","id":1,"method":"list_panes","params":{}}"#.into(),
                    ))
                    .await;
                let next = ws.next().await;
                assert!(
                    send_result.is_err() || next.is_none() || next.unwrap().is_err(),
                    "third connection should not be fully functional"
                );
            }
            Ok(Err(_)) => {} // handshake failed — expected
            Err(_) => {}     // timeout — server dropped connection, also fine
        }
    }

    #[tokio::test]
    async fn invalid_json_returns_parse_error() {
        let (notify_tx, _) = broadcast::channel(16);
        let server = start_test_server(vec![], notify_tx, None).await;

        let mut ws = server.connect().await;
        ws.send(Message::Text("not valid json".into())).await.unwrap();

        let resp = recv_notification(&mut ws).await;
        assert_eq!(resp["error"]["code"], -32700);
        assert!(resp["error"]["message"].as_str().unwrap().contains("parse error"));
    }

    #[tokio::test]
    async fn multiple_requests_on_same_connection() {
        let (notify_tx, _) = broadcast::channel(16);
        let server = start_test_server(
            vec![sample_pane("%1", "running")],
            notify_tx,
            None,
        )
        .await;

        let mut ws = server.connect().await;

        let resp1 = send_rpc(&mut ws, 1, "list_panes", serde_json::json!({})).await;
        assert_eq!(resp1["id"], 1);
        assert_eq!(resp1["result"]["panes"].as_array().unwrap().len(), 1);

        let resp2 = send_rpc(&mut ws, 2, "list_panes", serde_json::json!({})).await;
        assert_eq!(resp2["id"], 2);
        assert_eq!(resp2["result"]["panes"].as_array().unwrap().len(), 1);

        let resp3 = send_rpc(
            &mut ws,
            3,
            "subscribe",
            serde_json::json!({"events": ["state", "topology"]}),
        )
        .await;
        assert_eq!(resp3["id"], 3);
        assert_eq!(resp3["result"]["subscribed"], true);
    }

    // -----------------------------------------------------------------------
    // Terminal output streaming tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn subscribe_output_returns_ack() {
        let (notify_tx, _) = broadcast::channel(16);
        let server = start_test_server(vec![], notify_tx, None).await;

        let mut ws = server.connect().await;
        let resp = send_rpc(
            &mut ws,
            10,
            "subscribe_output",
            serde_json::json!({"pane_id": "%1"}),
        )
        .await;

        assert_eq!(resp["id"], 10);
        assert_eq!(resp["result"]["subscribed"], true);
        assert_eq!(resp["result"]["pane_id"], "%1");
    }

    #[tokio::test]
    async fn subscribe_output_missing_pane_id_returns_error() {
        let (notify_tx, _) = broadcast::channel(16);
        let server = start_test_server(vec![], notify_tx, None).await;

        let mut ws = server.connect().await;
        let resp = send_rpc(
            &mut ws,
            11,
            "subscribe_output",
            serde_json::json!({}),
        )
        .await;

        assert_eq!(resp["id"], 11);
        assert_eq!(resp["error"]["code"], -32602);
        assert!(resp["error"]["message"].as_str().unwrap().contains("pane_id"));
    }

    #[tokio::test]
    async fn unsubscribe_output_returns_ack() {
        let (notify_tx, _) = broadcast::channel(16);
        let server = start_test_server(vec![], notify_tx, None).await;

        let mut ws = server.connect().await;

        // Subscribe first, then unsubscribe.
        send_rpc(
            &mut ws,
            10,
            "subscribe_output",
            serde_json::json!({"pane_id": "%1"}),
        )
        .await;

        let resp = send_rpc(
            &mut ws,
            11,
            "unsubscribe_output",
            serde_json::json!({"pane_id": "%1"}),
        )
        .await;

        assert_eq!(resp["id"], 11);
        assert_eq!(resp["result"]["unsubscribed"], true);
        assert_eq!(resp["result"]["pane_id"], "%1");
    }

    #[tokio::test]
    async fn unsubscribe_output_missing_pane_id_returns_error() {
        let (notify_tx, _) = broadcast::channel(16);
        let server = start_test_server(vec![], notify_tx, None).await;

        let mut ws = server.connect().await;
        let resp = send_rpc(
            &mut ws,
            12,
            "unsubscribe_output",
            serde_json::json!({}),
        )
        .await;

        assert_eq!(resp["id"], 12);
        assert_eq!(resp["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn write_input_missing_pane_id_returns_error() {
        let (notify_tx, _) = broadcast::channel(16);
        let server = start_test_server(vec![], notify_tx, None).await;

        let mut ws = server.connect().await;
        let resp = send_rpc(
            &mut ws,
            20,
            "write_input",
            serde_json::json!({"data": "ls\n"}),
        )
        .await;

        assert_eq!(resp["id"], 20);
        assert_eq!(resp["error"]["code"], -32602);
        assert!(resp["error"]["message"].as_str().unwrap().contains("pane_id"));
    }

    #[tokio::test]
    async fn resize_pane_missing_params_returns_error() {
        let (notify_tx, _) = broadcast::channel(16);
        let server = start_test_server(vec![], notify_tx, None).await;

        let mut ws = server.connect().await;

        // Missing all params.
        let resp = send_rpc(
            &mut ws,
            30,
            "resize_pane",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(resp["id"], 30);
        assert_eq!(resp["error"]["code"], -32602);

        // Missing cols/rows.
        let resp2 = send_rpc(
            &mut ws,
            31,
            "resize_pane",
            serde_json::json!({"pane_id": "%1"}),
        )
        .await;
        assert_eq!(resp2["id"], 31);
        assert_eq!(resp2["error"]["code"], -32602);
    }
}

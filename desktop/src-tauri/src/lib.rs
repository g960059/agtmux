use serde::{Deserialize, Serialize};
use std::time::Duration;
use tauri::Emitter;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

// ---------------------------------------------------------------------------
// PaneInfo — mirrors the daemon's wire-format struct
// ---------------------------------------------------------------------------

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
// JSON-RPC helper types (deserialization only)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    #[allow(dead_code)]
    id: Option<u64>,
    result: Option<serde_json::Value>,
    error: Option<JsonRpcError>,
    /// For server-initiated notifications (no `id`, has `method`).
    method: Option<String>,
    /// Notifications carry their payload in `params`.
    params: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    #[allow(dead_code)]
    code: i32,
    message: String,
}

#[derive(Debug, Deserialize)]
struct ListPanesResult {
    panes: Vec<PaneInfo>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Open a fresh connection to the daemon, send a `list_panes` request, and
/// return the resulting pane list.  Used both by the Tauri command and by the
/// subscription loop when it needs to refetch after a `state_changed` event.
async fn fetch_panes_from_daemon(
    socket_path: &str,
) -> Result<Vec<PaneInfo>, Box<dyn std::error::Error + Send + Sync>> {
    let stream = UnixStream::connect(socket_path).await?;
    let mut reader = BufReader::new(stream);

    let request = r#"{"jsonrpc":"2.0","id":1,"method":"list_panes","params":{}}"#;
    let writer = reader.get_mut();
    writer.write_all(request.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;

    let mut line = String::new();
    reader.read_line(&mut line).await?;

    let resp: JsonRpcResponse = serde_json::from_str(&line)?;
    if let Some(err) = resp.error {
        return Err(format!("daemon error: {}", err.message).into());
    }

    let result_value = resp.result.ok_or("missing result in response")?;
    let list: ListPanesResult = serde_json::from_value(result_value)?;
    Ok(list.panes)
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

/// Call `list_panes` on the daemon and return the current state snapshot.
/// The call is wrapped with a 5-second timeout to avoid hanging forever if
/// the daemon is unresponsive.
#[tauri::command]
async fn list_panes(socket_path: String) -> Result<Vec<PaneInfo>, String> {
    // Fix 4: Wrap with timeout
    match tokio::time::timeout(
        Duration::from_secs(5),
        fetch_panes_from_daemon(&socket_path),
    )
    .await
    {
        Ok(Ok(panes)) => Ok(panes),
        Ok(Err(e)) => Err(format!("{}", e)),
        Err(_) => Err("list_panes timed out after 5 seconds".into()),
    }
}

/// Connect to the daemon, subscribe to state/topology events, and emit
/// Tauri events (`pane-update`, `pane-added`, `pane-removed`) to the frontend.
#[tauri::command]
async fn subscribe_panes(socket_path: String, app: tauri::AppHandle) -> Result<(), String> {
    subscribe_panes_internal(socket_path, app)
        .await
        .map_err(|e| format!("{}", e))
}

/// Internal subscribe logic shared by the command and the auto-setup.
async fn subscribe_panes_internal(
    socket_path: String,
    app: tauri::AppHandle,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let stream = UnixStream::connect(&socket_path).await?;
    let mut reader = BufReader::new(stream);

    // Send subscribe request for state + topology events
    let request =
        r#"{"jsonrpc":"2.0","id":1,"method":"subscribe","params":{"events":["state","topology"]}}"#;
    let writer = reader.get_mut();
    writer.write_all(request.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;

    // Read the subscribe acknowledgement
    let mut ack_line = String::new();
    reader.read_line(&mut ack_line).await?;

    let ack: JsonRpcResponse = serde_json::from_str(&ack_line)?;
    if let Some(err) = ack.error {
        return Err(format!("subscribe failed: {}", err.message).into());
    }

    // Emit connected status
    let _ = app.emit(
        "daemon-status",
        serde_json::json!({"connected": true}),
    );

    // Read push notifications in a loop
    let mut lines = reader.lines();
    while let Some(line) = lines.next_line().await? {
        if line.is_empty() {
            continue;
        }

        let msg: JsonRpcResponse = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("failed to parse notification: {}", e);
                continue;
            }
        };

        match msg.method.as_deref() {
            // Fix 1: state_changed sends { pane_id, state: PaneState } which has
            // different field names than flat PaneInfo.  Instead of trying to
            // transform, re-fetch the full pane list so the frontend always
            // receives consistent PaneInfo objects.
            Some("state_changed") => {
                match fetch_panes_from_daemon(&socket_path).await {
                    Ok(panes) => {
                        if let Err(e) = app.emit("pane-update-all", &panes) {
                            eprintln!("failed to emit pane-update-all: {}", e);
                        }
                    }
                    Err(e) => {
                        eprintln!("failed to refetch panes after state_changed: {}", e);
                    }
                }
            }

            Some("pane_added") => {
                // Emit pane-added with the pane_id, then also refetch the
                // full list so the frontend can get the complete PaneInfo.
                if let Some(params) = msg.params {
                    if let Err(e) = app.emit("pane-added", params) {
                        eprintln!("failed to emit pane-added: {}", e);
                    }
                }
            }

            Some("pane_removed") => {
                if let Some(params) = msg.params {
                    if let Err(e) = app.emit("pane-removed", params) {
                        eprintln!("failed to emit pane-removed: {}", e);
                    }
                }
            }

            _ => continue,
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// App entry point
// ---------------------------------------------------------------------------

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![list_panes, subscribe_panes])
        .setup(|app| {
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let socket_path = "/tmp/agtmux.sock".to_string();

                // Fix 3: Retry loop — reconnect when daemon restarts or
                // the connection drops.
                loop {
                    match subscribe_panes_internal(socket_path.clone(), handle.clone()).await {
                        Ok(()) => {
                            // Daemon closed the connection normally.
                            eprintln!("subscribe connection closed, will reconnect");
                        }
                        Err(e) => {
                            eprintln!("subscribe error: {}", e);
                        }
                    }

                    // Emit disconnected status
                    let _ = handle.emit(
                        "daemon-status",
                        serde_json::json!({"connected": false}),
                    );

                    tokio::time::sleep(Duration::from_secs(3)).await;

                    // Emit reconnecting status
                    let _ = handle.emit(
                        "daemon-status",
                        serde_json::json!({"connected": false, "reconnecting": true}),
                    );
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

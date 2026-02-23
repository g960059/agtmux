use serde::{Deserialize, Serialize};
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
// Tauri commands
// ---------------------------------------------------------------------------

/// Call `list_panes` on the daemon and return the current state snapshot.
#[tauri::command]
async fn list_panes(socket_path: String) -> Result<Vec<PaneInfo>, String> {
    let stream = UnixStream::connect(&socket_path)
        .await
        .map_err(|e| format!("failed to connect to daemon at {}: {}", socket_path, e))?;

    let mut reader = BufReader::new(stream);

    // Send JSON-RPC request
    let request = r#"{"jsonrpc":"2.0","id":1,"method":"list_panes","params":{}}"#;
    let writer = reader.get_mut();
    writer
        .write_all(request.as_bytes())
        .await
        .map_err(|e| format!("write error: {}", e))?;
    writer
        .write_all(b"\n")
        .await
        .map_err(|e| format!("write error: {}", e))?;
    writer
        .flush()
        .await
        .map_err(|e| format!("flush error: {}", e))?;

    // Read response line
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .await
        .map_err(|e| format!("read error: {}", e))?;

    // Parse response
    let resp: JsonRpcResponse =
        serde_json::from_str(&line).map_err(|e| format!("JSON parse error: {}", e))?;

    if let Some(err) = resp.error {
        return Err(format!("daemon error: {}", err.message));
    }

    let result_value = resp.result.ok_or("missing result in response")?;
    let list: ListPanesResult =
        serde_json::from_value(result_value).map_err(|e| format!("result parse error: {}", e))?;

    Ok(list.panes)
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

        // Determine the event name from the notification method
        let event_name = match msg.method.as_deref() {
            Some("state_changed") => "pane-update",
            Some("pane_added") => "pane-added",
            Some("pane_removed") => "pane-removed",
            _ => continue, // skip unknown notifications
        };

        // The notification payload is in `params`
        if let Some(params) = msg.params {
            if let Err(e) = app.emit(event_name, params) {
                eprintln!("failed to emit {}: {}", event_name, e);
            }
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
                if let Err(e) = subscribe_panes_internal(socket_path, handle).await {
                    eprintln!("auto-subscribe error: {}", e);
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

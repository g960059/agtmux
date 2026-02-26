//! UDS JSON-RPC server: minimal hand-rolled implementation.
//! Connection-per-request, newline-delimited JSON.

use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::Mutex;

use agtmux_core_v5::types::PanePresence;

use crate::poll_loop::DaemonState;

/// Run the UDS JSON-RPC server.
pub async fn run_server(socket_path: &str, state: Arc<Mutex<DaemonState>>) -> anyhow::Result<()> {
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

    // Check for stale socket
    if std::path::Path::new(socket_path).exists() {
        if tokio::net::UnixStream::connect(socket_path).await.is_err() {
            std::fs::remove_file(socket_path)?;
            tracing::info!("removed stale socket at {socket_path}");
        } else {
            anyhow::bail!("another daemon is already running at {socket_path}");
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
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, state).await {
                tracing::debug!("connection error: {e}");
            }
        });
    }
}

async fn handle_connection(
    stream: tokio::net::UnixStream,
    state: Arc<Mutex<DaemonState>>,
) -> anyhow::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    reader.read_line(&mut line).await?;

    let request: serde_json::Value = serde_json::from_str(line.trim())?;
    let method = request["method"].as_str().unwrap_or("");
    let id = request["id"].clone();

    let result = match method {
        "list_panes" => {
            let st = state.lock().await;
            build_pane_list(&st)
        }
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

        result.push(serde_json::json!({
            "pane_id": pane.pane_instance_id.pane_id,
            "presence": "managed",
            "evidence_mode": pane.evidence_mode,
            "signature_class": pane.signature_class,
            "signature_confidence": pane.signature_confidence,
            "activity_state": format!("{:?}", pane.activity_state),
            "provider": pane.provider.map(|p| p.as_str()),
            "session_name": tmux_info.map(|t| &t.session_name),
            "window_name": tmux_info.map(|t| &t.window_name),
            "current_cmd": tmux_info.map(|t| &t.current_cmd),
            "updated_at": pane.updated_at,
        }));
    }

    // Add unmanaged panes
    for tmux_pane in &state.last_panes {
        if !managed_ids.contains(tmux_pane.pane_id.as_str()) {
            result.push(serde_json::json!({
                "pane_id": tmux_pane.pane_id,
                "presence": PanePresence::Unmanaged,
                "session_name": tmux_pane.session_name,
                "window_name": tmux_pane.window_name,
                "current_cmd": tmux_pane.current_cmd,
            }));
        }
    }

    serde_json::Value::Array(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agtmux_core_v5::types::SourceKind;
    use agtmux_tmux_v5::{PaneGenerationTracker, TmuxPaneInfo};
    use chrono::Utc;

    fn make_state() -> DaemonState {
        use agtmux_daemon_v5::projection::DaemonProjection;
        use agtmux_gateway::gateway::Gateway;
        use agtmux_source_poller::source::PollerSourceState;

        DaemonState {
            poller: PollerSourceState::new(),
            gateway: Gateway::with_sources(&[SourceKind::Poller], Utc::now()),
            daemon: DaemonProjection::new(),
            generation_tracker: PaneGenerationTracker::new(),
            gateway_cursor: None,
            last_panes: Vec::new(),
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
}

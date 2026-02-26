//! UDS JSON-RPC server: minimal hand-rolled implementation.
//! Connection-per-request, newline-delimited JSON.

use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::Mutex;

use agtmux_core_v5::title::{TitleInput, resolve_title};
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
            deterministic_session_key: None, // MVP: no deterministic sources wired yet
            handshake_confirmed: false,      // MVP: no handshake yet
            canonical_session_name: None,    // MVP: no canonical lookup yet
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
            "title": title_decision.title,
            "title_quality": format!("{:?}", title_decision.quality),
            "session_name": tmux_info.map(|t| &t.session_name),
            "window_name": tmux_info.map(|t| &t.window_name),
            "current_cmd": tmux_info.map(|t| &t.current_cmd),
            "updated_at": pane.updated_at,
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
                "session_name": tmux_pane.session_name,
                "window_name": tmux_pane.window_name,
                "current_cmd": tmux_pane.current_cmd,
            }));
        }
    }

    serde_json::Value::Array(result)
}

/// Build a `state_changed` response: changes since a given version with full state.
///
/// Returns pane/session state for each change, plus the current version for
/// the client to use in subsequent `state_changed` calls.
pub(crate) fn build_state_changed(state: &DaemonState, since_version: u64) -> serde_json::Value {
    let changes = state.daemon.changes_since(since_version);
    let current_version = state.daemon.version();

    let mut entries = Vec::new();
    for change in &changes {
        let mut entry = serde_json::json!({
            "version": change.version,
            "session_key": change.session_key,
            "timestamp": change.timestamp,
        });

        // Include pane state if the change is pane-level
        if let Some(ref pane_id) = change.pane_id {
            entry["pane_id"] = serde_json::Value::String(pane_id.clone());
            if let Some(pane) = state.daemon.get_pane(pane_id) {
                entry["pane_state"] = serde_json::json!({
                    "signature_class": pane.signature_class,
                    "evidence_mode": pane.evidence_mode,
                    "activity_state": format!("{:?}", pane.activity_state),
                    "provider": pane.provider.map(|p| p.as_str()),
                    "signature_confidence": pane.signature_confidence,
                });
            }
        }

        // Include session state
        if let Some(session) = state.daemon.get_session(&change.session_key) {
            entry["session_state"] = serde_json::json!({
                "presence": session.presence,
                "evidence_mode": session.evidence_mode,
                "activity_state": format!("{:?}", session.activity_state),
                "winner_tier": session.winner_tier,
            });
        }

        entries.push(entry);
    }

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

    let managed_count = state.daemon.list_panes().len();
    let total_panes = state.last_panes.len();
    let unmanaged_count = total_panes - managed_count.min(total_panes);

    serde_json::json!({
        "has_changes": !changes.is_empty(),
        "pane_changes": pane_changes,
        "session_changes": session_changes,
        "version": current_version,
        "summary": {
            "managed": managed_count,
            "unmanaged": unmanaged_count,
            "total": total_panes,
        },
    })
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
}

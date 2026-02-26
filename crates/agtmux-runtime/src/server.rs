//! UDS JSON-RPC server: minimal hand-rolled implementation.
//! Connection-per-request, newline-delimited JSON.

use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::Mutex;

use agtmux_core_v5::title::{TitleInput, resolve_title};
use agtmux_core_v5::types::{EvidenceMode, PanePresence};

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
                "codex_appserver" => agtmux_core_v5::types::SourceKind::CodexAppserver,
                "claude_hooks" => agtmux_core_v5::types::SourceKind::ClaudeHooks,
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
                "codex_appserver" => {
                    match serde_json::from_value::<
                        agtmux_source_codex_appserver::translate::CodexRawEvent,
                    >(params["event"].clone())
                    {
                        Ok(event) => {
                            let mut st = state.lock().await;
                            st.codex_source.ingest(event);
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
    use agtmux_core_v5::types::SourceKind;
    use agtmux_tmux_v5::TmuxPaneInfo;
    use chrono::Utc;

    fn make_state() -> DaemonState {
        DaemonState::new()
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

    // ── source.ingest tests (via UDS handler) ──────────────────────────

    /// Helper: send a JSON-RPC request through handle_connection and return the response.
    async fn call_handler(
        state: Arc<Mutex<DaemonState>>,
        request: serde_json::Value,
    ) -> serde_json::Value {
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

        let handle_fut = handle_connection(server, state);

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
    async fn source_ingest_codex_appserver_accepted() {
        let state = Arc::new(Mutex::new(make_state()));
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "source.ingest",
            "id": 2,
            "params": {
                "source_kind": "codex_appserver",
                "event": {
                    "id": "cx-test-1",
                    "event_type": "task.running",
                    "session_id": "codex-sess",
                    "timestamp": "2026-02-25T12:00:00Z",
                    "pane_id": "%0",
                    "payload": {}
                }
            }
        });

        let resp = call_handler(Arc::clone(&state), request).await;
        assert_eq!(resp["result"]["status"], "ok");

        let st = state.lock().await;
        assert_eq!(st.codex_source.buffered_len(), 1);
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
                "source_id": "codex_appserver",
                "source_kind": "codex_appserver",
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
        assert_eq!(entries[0]["source_id"], "codex_appserver");
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
            "DaemonState::new() should pre-register poller, codex_appserver, claude_hooks"
        );
        assert!(state.trust_guard.is_registered("poller"));
        assert!(state.trust_guard.is_registered("codex_appserver"));
        assert!(state.trust_guard.is_registered("claude_hooks"));
    }
}

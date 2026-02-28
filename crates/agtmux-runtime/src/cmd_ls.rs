//! `agtmux ls` — hierarchical tree, session summary, or flat pane view.

use std::collections::HashMap;

use crate::context::{
    build_branch_map, consensus_str, provider_short, relative_time, short_path, truncate_branch,
};

/// RPC call (re-use from client).
async fn rpc_call(socket_path: &str, method: &str) -> anyhow::Result<serde_json::Value> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;

    let stream = UnixStream::connect(socket_path)
        .await
        .map_err(|e| anyhow::anyhow!("cannot connect to daemon at {socket_path}: {e}"))?;

    let (reader, mut writer) = stream.into_split();

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": {},
        "id": 1,
    });
    let mut req = serde_json::to_string(&request)?;
    req.push('\n');
    writer.write_all(req.as_bytes()).await?;
    writer.shutdown().await?;

    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    reader.read_line(&mut line).await?;

    let response: serde_json::Value = serde_json::from_str(line.trim())?;

    if let Some(error) = response.get("error") {
        anyhow::bail!("RPC error: {error}");
    }

    Ok(response["result"].clone())
}

/// Entry point for `agtmux ls`.
pub async fn cmd_ls(socket_path: &str, group: &str, use_color: bool) -> anyhow::Result<()> {
    let panes = rpc_call(socket_path, "list_panes").await?;
    let arr = panes.as_array().cloned().unwrap_or_default();

    let branch_map = build_branch_map(&arr);

    let output = match group {
        "session" => format_ls_session(&arr, &branch_map, use_color),
        "pane" => format_ls_pane(&arr, &branch_map, use_color),
        _ => format_ls_tree(&arr, &branch_map, use_color),
    };

    if !output.is_empty() {
        println!("{output}");
    }

    Ok(())
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Normalize WaitingInput/WaitingApproval to "Waiting" for display.
fn display_state(state: &str) -> &str {
    match state {
        "WaitingInput" | "WaitingApproval" => "Waiting",
        other => other,
    }
}

/// Age string from updated_at ISO timestamp.
fn age_from_updated_at(pane: &serde_json::Value) -> String {
    pane["updated_at"]
        .as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| {
            let secs = (chrono::Utc::now() - dt.with_timezone(&chrono::Utc)).num_seconds();
            relative_time(secs)
        })
        .unwrap_or_default()
}

/// Get git branch for a pane from the branch map.
fn pane_branch<'a>(
    pane: &serde_json::Value,
    branch_map: &'a HashMap<String, String>,
) -> Option<&'a str> {
    pane["current_path"]
        .as_str()
        .and_then(|p| branch_map.get(p))
        .map(|s| s.as_str())
}

/// Build state summary string like "Waiting(1) Running(2)".
fn state_summary(panes: &[&serde_json::Value]) -> String {
    let mut waiting = 0usize;
    let mut running = 0usize;
    let mut idle = 0usize;
    let mut shell = 0usize;

    for pane in panes {
        if pane["presence"].as_str() == Some("managed") {
            match display_state(pane["activity_state"].as_str().unwrap_or("")) {
                "Waiting" => waiting += 1,
                "Running" => running += 1,
                "Idle" => idle += 1,
                _ => {}
            }
        } else {
            shell += 1;
        }
    }

    let mut parts = Vec::new();
    if waiting > 0 {
        parts.push(format!("Waiting({waiting})"));
    }
    if running > 0 {
        parts.push(format!("Running({running})"));
    }
    if idle > 0 {
        parts.push(format!("Idle({idle})"));
    }
    if shell > 0 {
        parts.push(format!("shell({shell})"));
    }
    parts.join(" ")
}

// ── Tree format ─────────────────────────────────────────────────────────────

/// `--group=tree` (default): session -> window -> pane hierarchy.
pub fn format_ls_tree(
    panes: &[serde_json::Value],
    branch_map: &HashMap<String, String>,
    use_color: bool,
) -> String {
    if panes.is_empty() {
        return String::new();
    }

    // Group: session -> window -> panes (preserving first-seen order)
    let mut session_order: Vec<String> = Vec::new();
    let mut session_windows: HashMap<String, Vec<String>> = HashMap::new();
    let mut window_panes: HashMap<(String, String), Vec<&serde_json::Value>> = HashMap::new();

    for pane in panes {
        let sess = pane["session_name"].as_str().unwrap_or("?").to_string();
        let win_id = pane["window_id"].as_str().unwrap_or("@?").to_string();
        let key = (sess.clone(), win_id.clone());

        if !session_windows.contains_key(&sess) {
            session_order.push(sess.clone());
            session_windows.insert(sess.clone(), Vec::new());
        }
        let wins = session_windows.get_mut(&sess).expect("just inserted");
        if !wins.contains(&win_id) {
            wins.push(win_id.clone());
        }
        window_panes.entry(key).or_default().push(pane);
    }

    let mut out = String::new();

    for sess_name in &session_order {
        let win_ids = &session_windows[sess_name];

        // Collect all panes in session for consensus
        let empty: Vec<&serde_json::Value> = Vec::new();
        let all_sess_panes: Vec<&serde_json::Value> = win_ids
            .iter()
            .flat_map(|wid| {
                window_panes
                    .get(&(sess_name.clone(), wid.clone()))
                    .unwrap_or(&empty)
                    .as_slice()
            })
            .copied()
            .collect();

        // Session-level consensus cwd
        let cwd_consensus =
            consensus_str(all_sess_panes.iter().map(|p| p["current_path"].as_str()));
        let cwd_display = match &cwd_consensus {
            Some(cwd) => short_path(cwd),
            None => "[cwd: mixed]".to_string(),
        };

        // Session-level consensus branch
        let branch_consensus =
            consensus_str(all_sess_panes.iter().map(|p| pane_branch(p, branch_map)));
        let branch_display = match &branch_consensus {
            Some(b) => format!("[{}]", truncate_branch(b, 20)),
            None => "[branch: mixed]".to_string(),
        };

        // Session header
        if use_color {
            out.push_str(&format!(
                "\x1b[1m{sess_name}\x1b[0m  {cwd_display} \x1b[36m{branch_display}\x1b[0m\n"
            ));
        } else {
            out.push_str(&format!("{sess_name}  {cwd_display} {branch_display}\n"));
        }

        // Windows
        for win_id in win_ids {
            let key = (sess_name.clone(), win_id.clone());
            let panes_in_win = window_panes.get(&key).map(|v| v.as_slice()).unwrap_or(&[]);
            let win_name = panes_in_win
                .first()
                .and_then(|p| p["window_name"].as_str())
                .unwrap_or("(unnamed)");
            let win_name = if win_name.is_empty() {
                "(unnamed)"
            } else {
                win_name
            };

            // Window-level branch (show only if different from session)
            let win_branch = consensus_str(panes_in_win.iter().map(|p| pane_branch(p, branch_map)));
            let win_branch_suffix = match (&win_branch, &branch_consensus) {
                (Some(wb), Some(sb)) if wb != sb => {
                    let tb = truncate_branch(wb, 20);
                    if use_color {
                        format!(" \x1b[36m[{tb}]\x1b[0m")
                    } else {
                        format!(" [{tb}]")
                    }
                }
                (Some(wb), None) => {
                    let tb = truncate_branch(wb, 20);
                    if use_color {
                        format!(" \x1b[36m[{tb}]\x1b[0m")
                    } else {
                        format!(" [{tb}]")
                    }
                }
                _ => String::new(),
            };

            let summary = state_summary(&panes_in_win.iter().copied().collect::<Vec<_>>());
            let summary_suffix = if summary.is_empty() {
                String::new()
            } else {
                format!(" \u{2014} {summary}")
            };

            if use_color {
                out.push_str(&format!(
                    "  \x1b[1;33m{win_name}\x1b[0m{win_branch_suffix}{summary_suffix}\n"
                ));
            } else {
                out.push_str(&format!(
                    "  {win_name}{win_branch_suffix}{summary_suffix}\n"
                ));
            }

            // Panes (sorted by pane_id numerically)
            let mut sorted_panes: Vec<&serde_json::Value> = panes_in_win.to_vec();
            sorted_panes.sort_by_key(|p| {
                p["pane_id"]
                    .as_str()
                    .unwrap_or("")
                    .trim_start_matches('%')
                    .parse::<u32>()
                    .unwrap_or(0)
            });

            for pane in sorted_panes {
                let presence = pane["presence"].as_str().unwrap_or("unmanaged");

                if presence == "managed" {
                    let provider = pane["provider"].as_str().unwrap_or("?");
                    let evidence = pane["evidence_mode"].as_str().unwrap_or("");
                    let is_heur = evidence != "deterministic";
                    let state = display_state(pane["activity_state"].as_str().unwrap_or("?"));
                    let title = pane["conversation_title"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| provider_short(provider));
                    let age = age_from_updated_at(pane);

                    let prov = format!("{:<6}", provider_short(provider));
                    let title_padded = format!("{title:<20}");
                    let state_padded = format!("{state:<7}");

                    if is_heur {
                        // Heuristic: ~Provider
                        if use_color {
                            let state_colored = match state {
                                "Waiting" => format!("\x1b[1;33m{state_padded}\x1b[0m"),
                                "Running" => format!("\x1b[32m{state_padded}\x1b[0m"),
                                _ => format!("\x1b[2m{state_padded}\x1b[0m"),
                            };
                            out.push_str(&format!(
                                "    \x1b[33m~\x1b[0m{prov}  {title_padded}  {state_colored}  \x1b[2m{age}\x1b[0m\n"
                            ));
                        } else {
                            out.push_str(&format!(
                                "    ~{prov}  {title_padded}  {state_padded}  {age}\n"
                            ));
                        }
                    } else {
                        // Deterministic
                        let marker = if state == "Waiting" { "!" } else { " " };
                        if use_color {
                            let state_colored = match state {
                                "Waiting" => format!("\x1b[1;33m{state_padded}\x1b[0m"),
                                "Running" => format!("\x1b[32m{state_padded}\x1b[0m"),
                                _ => format!("\x1b[2m{state_padded}\x1b[0m"),
                            };
                            let marker_colored = if state == "Waiting" {
                                format!("\x1b[1;33m{marker}\x1b[0m")
                            } else {
                                marker.to_string()
                            };
                            out.push_str(&format!(
                                "    {marker_colored} {prov}  {title_padded}  {state_colored}  \x1b[2m{age}\x1b[0m\n"
                            ));
                        } else {
                            out.push_str(&format!(
                                "    {marker} {prov}  {title_padded}  {state_padded}  {age}\n"
                            ));
                        }
                    }
                } else {
                    // Unmanaged pane
                    let cmd = pane["current_cmd"].as_str().unwrap_or("?");
                    if use_color {
                        out.push_str(&format!("      \x1b[2m{cmd}\x1b[0m\n"));
                    } else {
                        out.push_str(&format!("      {cmd}\n"));
                    }
                }
            }
        }

        out.push('\n');
    }

    // Trim trailing newlines
    while out.ends_with('\n') {
        out.pop();
    }
    out
}

// ── Session format ──────────────────────────────────────────────────────────

/// `--group=session`: one line per session with aggregate counts.
pub fn format_ls_session(
    panes: &[serde_json::Value],
    branch_map: &HashMap<String, String>,
    use_color: bool,
) -> String {
    if panes.is_empty() {
        return String::new();
    }

    // Group by session (alphabetical)
    let mut sessions: HashMap<String, Vec<&serde_json::Value>> = HashMap::new();
    for pane in panes {
        let sess = pane["session_name"].as_str().unwrap_or("?").to_string();
        sessions.entry(sess).or_default().push(pane);
    }
    let mut session_names: Vec<String> = sessions.keys().cloned().collect();
    session_names.sort();

    let max_name_len = session_names.iter().map(|s| s.len()).max().unwrap_or(0);

    let mut out = String::new();

    for sess_name in &session_names {
        let panes_in_sess = &sessions[sess_name];

        // Window count
        let window_count = panes_in_sess
            .iter()
            .filter_map(|p| p["window_id"].as_str())
            .collect::<std::collections::HashSet<_>>()
            .len();

        // Agent counts
        let mut running = 0usize;
        let mut idle = 0usize;
        let mut waiting = 0usize;
        for pane in panes_in_sess {
            if pane["presence"].as_str() == Some("managed") {
                match display_state(pane["activity_state"].as_str().unwrap_or("")) {
                    "Waiting" => waiting += 1,
                    "Running" => running += 1,
                    "Idle" => idle += 1,
                    _ => {}
                }
            }
        }

        let agent_count = running + idle + waiting;
        let agent_word = if agent_count == 1 { "agent " } else { "agents" };
        let win_word = "win";

        let mut state_parts: Vec<String> = Vec::new();
        if waiting > 0 {
            state_parts.push(format!("{waiting} Waiting"));
        }
        if running > 0 {
            state_parts.push(format!("{running} Running"));
        }
        if idle > 0 {
            state_parts.push(format!("{idle} Idle"));
        }
        let state_str = if state_parts.is_empty() {
            String::new()
        } else {
            format!(" ({})", state_parts.join(", "))
        };

        // CWD consensus
        let cwd_consensus = consensus_str(panes_in_sess.iter().map(|p| p["current_path"].as_str()));
        let cwd_display = match &cwd_consensus {
            Some(cwd) => short_path(cwd),
            None => "[cwd: mixed]".to_string(),
        };

        // Branch consensus
        let branch_consensus =
            consensus_str(panes_in_sess.iter().map(|p| pane_branch(p, branch_map)));
        let branch_display = match &branch_consensus {
            Some(b) => format!("[{}]", truncate_branch(b, 20)),
            None => "[branch: mixed]".to_string(),
        };

        let name_padded = format!("{sess_name:<width$}", width = max_name_len);

        if use_color {
            out.push_str(&format!(
                "\x1b[1m{name_padded}\x1b[0m  {window_count} {win_word}  {agent_count} {agent_word}{state_str}  {cwd_display}  \x1b[36m{branch_display}\x1b[0m\n"
            ));
        } else {
            out.push_str(&format!(
                "{name_padded}  {window_count} {win_word}  {agent_count} {agent_word}{state_str}  {cwd_display}  {branch_display}\n"
            ));
        }
    }

    // Trim trailing newlines
    while out.ends_with('\n') {
        out.pop();
    }
    out
}

// ── Pane format ─────────────────────────────────────────────────────────────

/// `--group=pane`: flat one-line-per-agent view.
pub fn format_ls_pane(
    panes: &[serde_json::Value],
    branch_map: &HashMap<String, String>,
    use_color: bool,
) -> String {
    if panes.is_empty() {
        return String::new();
    }

    // Only show managed panes in pane view
    let managed: Vec<&serde_json::Value> = panes
        .iter()
        .filter(|p| p["presence"].as_str() == Some("managed"))
        .collect();

    if managed.is_empty() {
        return String::new();
    }

    // Compute max session:window width for alignment
    let max_loc_len = managed
        .iter()
        .map(|p| {
            let sess = p["session_name"].as_str().unwrap_or("?");
            let win = p["window_name"].as_str().unwrap_or("");
            if win.is_empty() {
                sess.len()
            } else {
                sess.len() + 1 + win.len() // sess:win
            }
        })
        .max()
        .unwrap_or(0);

    let mut out = String::new();

    for pane in &managed {
        let sess = pane["session_name"].as_str().unwrap_or("?");
        let win = pane["window_name"].as_str().unwrap_or("");
        let location = if win.is_empty() {
            sess.to_string()
        } else {
            format!("{sess}:{win}")
        };
        let loc_padded = format!("{location:<width$}", width = max_loc_len);

        let provider = pane["provider"].as_str().unwrap_or("?");
        let evidence = pane["evidence_mode"].as_str().unwrap_or("");
        let is_heur = evidence != "deterministic";
        let state = display_state(pane["activity_state"].as_str().unwrap_or("?"));
        let title = pane["conversation_title"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| provider_short(provider));
        let age = age_from_updated_at(pane);

        let prov = format!("{:<6}", provider_short(provider));
        let state_padded = format!("{state:<7}");
        let title_padded = format!("{title:<20}");

        let branch = pane_branch(pane, branch_map)
            .map(|b| truncate_branch(b, 20))
            .unwrap_or_default();
        let branch_display = if branch.is_empty() {
            String::new()
        } else if use_color {
            format!("  \x1b[36m[{branch}]\x1b[0m")
        } else {
            format!("  [{branch}]")
        };

        let marker = if state == "Waiting" {
            if use_color { "\x1b[1;33m!\x1b[0m" } else { "!" }
        } else if is_heur {
            if use_color { "\x1b[33m~\x1b[0m" } else { "~" }
        } else {
            " "
        };

        if use_color {
            let state_colored = match state {
                "Waiting" => format!("\x1b[1;33m{state_padded}\x1b[0m"),
                "Running" => format!("\x1b[32m{state_padded}\x1b[0m"),
                _ => format!("\x1b[2m{state_padded}\x1b[0m"),
            };
            out.push_str(&format!(
                "{loc_padded}  {marker} {prov}  {state_colored}  {title_padded}{branch_display}  \x1b[2m{age}\x1b[0m\n"
            ));
        } else {
            out.push_str(&format!(
                "{loc_padded}  {marker} {prov}  {state_padded}  {title_padded}{branch_display}  {age}\n"
            ));
        }
    }

    // Trim trailing newlines
    while out.ends_with('\n') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::too_many_arguments)]
    fn make_pane(
        pane_id: &str,
        session_name: &str,
        window_id: &str,
        window_name: &str,
        presence: &str,
        provider: Option<&str>,
        evidence_mode: &str,
        activity_state: &str,
        current_cmd: &str,
        current_path: &str,
    ) -> serde_json::Value {
        let mut v = serde_json::json!({
            "pane_id": pane_id,
            "session_name": session_name,
            "session_id": "$0",
            "window_id": window_id,
            "window_name": window_name,
            "presence": presence,
            "evidence_mode": evidence_mode,
            "activity_state": activity_state,
            "current_cmd": current_cmd,
            "current_path": current_path,
        });
        if let Some(p) = provider {
            v["provider"] = serde_json::Value::String(p.to_string());
        }
        v
    }

    fn make_branch_map(entries: &[(&str, &str)]) -> HashMap<String, String> {
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    // ── format_ls_tree tests ────────────────────────────────────────────

    #[test]
    fn format_ls_tree_empty() {
        let panes: Vec<serde_json::Value> = vec![];
        let branch_map = HashMap::new();
        assert_eq!(format_ls_tree(&panes, &branch_map, false), "");
    }

    #[test]
    fn format_ls_tree_session_header() {
        let panes = vec![make_pane(
            "%0",
            "work",
            "@0",
            "dev",
            "unmanaged",
            None,
            "",
            "",
            "zsh",
            "/repo/project",
        )];
        let branch_map = make_branch_map(&[("/repo/project", "main")]);
        let out = format_ls_tree(&panes, &branch_map, false);
        assert!(out.contains("work"), "session name in header");
        assert!(out.contains("repo/project"), "short cwd in header");
        assert!(out.contains("[main]"), "branch in header");
    }

    #[test]
    fn format_ls_tree_window_state_summary() {
        let panes = vec![
            make_pane(
                "%0",
                "work",
                "@0",
                "dev",
                "managed",
                Some("ClaudeCode"),
                "deterministic",
                "Running",
                "claude",
                "/repo",
            ),
            make_pane(
                "%1",
                "work",
                "@0",
                "dev",
                "unmanaged",
                None,
                "",
                "",
                "zsh",
                "/repo",
            ),
        ];
        let branch_map = make_branch_map(&[("/repo", "main")]);
        let out = format_ls_tree(&panes, &branch_map, false);
        assert!(
            out.contains("Running(1)"),
            "Running count in window summary"
        );
        assert!(out.contains("shell(1)"), "shell count in window summary");
    }

    #[test]
    fn format_ls_tree_waiting_pane_exclamation() {
        let panes = vec![make_pane(
            "%0",
            "work",
            "@0",
            "dev",
            "managed",
            Some("ClaudeCode"),
            "deterministic",
            "WaitingInput",
            "claude",
            "/repo",
        )];
        let branch_map = HashMap::new();
        let out = format_ls_tree(&panes, &branch_map, false);
        assert!(out.contains('!'), "Waiting pane has ! marker");
        assert!(
            out.contains("Waiting"),
            "WaitingInput normalized to Waiting"
        );
    }

    #[test]
    fn format_ls_tree_heuristic_tilde() {
        let panes = vec![make_pane(
            "%0",
            "work",
            "@0",
            "dev",
            "managed",
            Some("ClaudeCode"),
            "heuristic",
            "Running",
            "claude",
            "/repo",
        )];
        let branch_map = HashMap::new();
        let out = format_ls_tree(&panes, &branch_map, false);
        assert!(out.contains('~'), "heuristic pane has ~ marker");
    }

    #[test]
    fn format_ls_tree_mixed_cwd() {
        let panes = vec![
            make_pane(
                "%0",
                "work",
                "@0",
                "dev",
                "unmanaged",
                None,
                "",
                "",
                "zsh",
                "/repo/a",
            ),
            make_pane(
                "%1",
                "work",
                "@1",
                "api",
                "unmanaged",
                None,
                "",
                "",
                "zsh",
                "/repo/b",
            ),
        ];
        let branch_map = HashMap::new();
        let out = format_ls_tree(&panes, &branch_map, false);
        assert!(out.contains("[cwd: mixed]"), "mixed cwd shown");
    }

    #[test]
    fn format_ls_tree_mixed_branch() {
        let panes = vec![
            make_pane(
                "%0",
                "work",
                "@0",
                "dev",
                "unmanaged",
                None,
                "",
                "",
                "zsh",
                "/repo/a",
            ),
            make_pane(
                "%1",
                "work",
                "@1",
                "api",
                "unmanaged",
                None,
                "",
                "",
                "zsh",
                "/repo/b",
            ),
        ];
        let branch_map = make_branch_map(&[("/repo/a", "main"), ("/repo/b", "dev")]);
        let out = format_ls_tree(&panes, &branch_map, false);
        assert!(out.contains("[branch: mixed]"), "mixed branch shown");
    }

    #[test]
    fn format_ls_tree_unmanaged_shows_cmd() {
        let panes = vec![make_pane(
            "%0",
            "work",
            "@0",
            "dev",
            "unmanaged",
            None,
            "",
            "",
            "vim",
            "/repo",
        )];
        let branch_map = HashMap::new();
        let out = format_ls_tree(&panes, &branch_map, false);
        assert!(out.contains("vim"), "unmanaged pane shows command");
    }

    #[test]
    fn format_ls_tree_no_ansi_without_color() {
        let panes = vec![make_pane(
            "%0",
            "work",
            "@0",
            "dev",
            "managed",
            Some("ClaudeCode"),
            "deterministic",
            "Running",
            "claude",
            "/repo",
        )];
        let branch_map = HashMap::new();
        let out = format_ls_tree(&panes, &branch_map, false);
        assert!(!out.contains('\x1b'), "no ANSI in no-color mode");
    }

    #[test]
    fn format_ls_tree_ansi_with_color() {
        let panes = vec![make_pane(
            "%0",
            "work",
            "@0",
            "dev",
            "managed",
            Some("ClaudeCode"),
            "deterministic",
            "Running",
            "claude",
            "/repo",
        )];
        let branch_map = HashMap::new();
        let out = format_ls_tree(&panes, &branch_map, true);
        assert!(out.contains('\x1b'), "ANSI codes present in color mode");
    }

    #[test]
    fn format_ls_tree_conversation_title_shown() {
        let mut pane = make_pane(
            "%0",
            "work",
            "@0",
            "dev",
            "managed",
            Some("ClaudeCode"),
            "deterministic",
            "Running",
            "claude",
            "/repo",
        );
        pane["conversation_title"] = serde_json::Value::String("fix: T-139 redesign".to_string());
        let panes = vec![pane];
        let branch_map = HashMap::new();
        let out = format_ls_tree(&panes, &branch_map, false);
        assert!(
            out.contains("fix: T-139 redesign"),
            "conversation_title shown"
        );
    }

    #[test]
    fn format_ls_tree_conversation_title_null_falls_back() {
        let panes = vec![make_pane(
            "%0",
            "work",
            "@0",
            "dev",
            "managed",
            Some("ClaudeCode"),
            "deterministic",
            "Running",
            "claude",
            "/repo",
        )];
        let branch_map = HashMap::new();
        let out = format_ls_tree(&panes, &branch_map, false);
        assert!(out.contains("Claude"), "falls back to provider short name");
    }

    // ── format_ls_session tests ─────────────────────────────────────────

    #[test]
    fn format_ls_session_empty() {
        let panes: Vec<serde_json::Value> = vec![];
        let branch_map = HashMap::new();
        assert_eq!(format_ls_session(&panes, &branch_map, false), "");
    }

    #[test]
    fn format_ls_session_counts() {
        let panes = vec![
            make_pane(
                "%0",
                "work",
                "@0",
                "dev",
                "managed",
                Some("ClaudeCode"),
                "deterministic",
                "WaitingInput",
                "claude",
                "/repo",
            ),
            make_pane(
                "%1",
                "work",
                "@0",
                "dev",
                "managed",
                Some("Codex"),
                "deterministic",
                "Running",
                "codex",
                "/repo",
            ),
            make_pane(
                "%2",
                "work",
                "@1",
                "api",
                "managed",
                Some("ClaudeCode"),
                "deterministic",
                "Idle",
                "claude",
                "/repo",
            ),
        ];
        let branch_map = make_branch_map(&[("/repo", "main")]);
        let out = format_ls_session(&panes, &branch_map, false);
        assert!(out.contains("work"), "session name present");
        assert!(out.contains("2 win"), "window count");
        assert!(out.contains("3 agents"), "agent count");
        assert!(out.contains("1 Waiting"), "waiting count");
        assert!(out.contains("1 Running"), "running count");
        assert!(out.contains("1 Idle"), "idle count");
        assert!(out.contains("[main]"), "branch shown");
    }

    #[test]
    fn format_ls_session_no_ansi_without_color() {
        let panes = vec![make_pane(
            "%0",
            "work",
            "@0",
            "dev",
            "managed",
            Some("ClaudeCode"),
            "deterministic",
            "Running",
            "claude",
            "/repo",
        )];
        let branch_map = HashMap::new();
        let out = format_ls_session(&panes, &branch_map, false);
        assert!(!out.contains('\x1b'), "no ANSI in no-color mode");
    }

    // ── format_ls_pane tests ────────────────────────────────────────────

    #[test]
    fn format_ls_pane_empty() {
        let panes: Vec<serde_json::Value> = vec![];
        let branch_map = HashMap::new();
        assert_eq!(format_ls_pane(&panes, &branch_map, false), "");
    }

    #[test]
    fn format_ls_pane_columns() {
        let panes = vec![
            make_pane(
                "%0",
                "work",
                "@0",
                "api",
                "managed",
                Some("ClaudeCode"),
                "deterministic",
                "WaitingInput",
                "claude",
                "/repo",
            ),
            make_pane(
                "%1",
                "work",
                "@1",
                "dev",
                "managed",
                Some("Codex"),
                "deterministic",
                "Running",
                "codex",
                "/repo",
            ),
        ];
        let branch_map = make_branch_map(&[("/repo", "feat/oauth")]);
        let out = format_ls_pane(&panes, &branch_map, false);
        assert!(out.contains("work:api"), "session:window location");
        assert!(out.contains("work:dev"), "session:window location");
        assert!(out.contains("Claude"), "provider short name");
        assert!(out.contains("Codex"), "provider short name");
        assert!(out.contains("[feat/oauth]"), "branch shown");
    }

    #[test]
    fn format_ls_pane_only_managed() {
        let panes = vec![
            make_pane(
                "%0",
                "work",
                "@0",
                "dev",
                "managed",
                Some("ClaudeCode"),
                "deterministic",
                "Running",
                "claude",
                "/repo",
            ),
            make_pane(
                "%1",
                "work",
                "@0",
                "dev",
                "unmanaged",
                None,
                "",
                "",
                "zsh",
                "/repo",
            ),
        ];
        let branch_map = HashMap::new();
        let out = format_ls_pane(&panes, &branch_map, false);
        assert!(out.contains("Claude"), "managed pane shown");
        assert!(!out.contains("zsh"), "unmanaged pane not shown");
    }

    #[test]
    fn format_ls_pane_no_ansi_without_color() {
        let panes = vec![make_pane(
            "%0",
            "work",
            "@0",
            "dev",
            "managed",
            Some("ClaudeCode"),
            "deterministic",
            "Running",
            "claude",
            "/repo",
        )];
        let branch_map = HashMap::new();
        let out = format_ls_pane(&panes, &branch_map, false);
        assert!(!out.contains('\x1b'), "no ANSI in no-color mode");
    }

    #[test]
    fn format_ls_pane_waiting_has_exclamation() {
        let panes = vec![make_pane(
            "%0",
            "work",
            "@0",
            "dev",
            "managed",
            Some("ClaudeCode"),
            "deterministic",
            "WaitingApproval",
            "claude",
            "/repo",
        )];
        let branch_map = HashMap::new();
        let out = format_ls_pane(&panes, &branch_map, false);
        assert!(out.contains('!'), "Waiting pane has ! marker");
    }

    #[test]
    fn format_ls_pane_heuristic_has_tilde() {
        let panes = vec![make_pane(
            "%0",
            "work",
            "@0",
            "dev",
            "managed",
            Some("ClaudeCode"),
            "heuristic",
            "Running",
            "claude",
            "/repo",
        )];
        let branch_map = HashMap::new();
        let out = format_ls_pane(&panes, &branch_map, false);
        assert!(out.contains('~'), "heuristic pane has ~ marker");
    }
}

//! `agtmux pick` — interactive agent picker via fzf.

use std::collections::HashMap;
use std::io::Write;
use std::process::{Command, Stdio};

use crate::client::rpc_call;
use crate::context::{
    build_branch_map, provider_short, relative_time, resolve_color, truncate_branch,
};

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

/// Build candidate lines for the pick command.
///
/// Each line: `session:window  marker provider  state  title  [branch]  age`
pub(crate) fn format_pick_candidates(
    panes: &[serde_json::Value],
    branch_map: &HashMap<String, String>,
    waiting_only: bool,
) -> Vec<String> {
    // Only managed panes
    let managed: Vec<&serde_json::Value> = panes
        .iter()
        .filter(|p| p["presence"].as_str() == Some("managed"))
        .collect();

    if managed.is_empty() {
        return Vec::new();
    }

    // Optionally filter to waiting-only
    let filtered: Vec<&serde_json::Value> = if waiting_only {
        managed
            .into_iter()
            .filter(|p| {
                matches!(
                    p["activity_state"].as_str(),
                    Some("WaitingInput") | Some("WaitingApproval")
                )
            })
            .collect()
    } else {
        managed
    };

    if filtered.is_empty() {
        return Vec::new();
    }

    // Compute max location width for alignment
    let max_loc_len = filtered
        .iter()
        .map(|p| {
            let sess = p["session_name"].as_str().unwrap_or("?");
            let win = p["window_name"].as_str().unwrap_or("");
            if win.is_empty() {
                sess.len()
            } else {
                sess.len() + 1 + win.len()
            }
        })
        .max()
        .unwrap_or(0);

    let mut lines = Vec::new();

    for pane in &filtered {
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

        let marker = if state == "Waiting" {
            "!"
        } else if is_heur {
            "~"
        } else {
            " "
        };

        let prov = format!("{:<6}", provider_short(provider));
        let state_padded = format!("{state:<8}");
        let title_padded = format!("{title:<20}");

        let branch = pane["current_path"]
            .as_str()
            .and_then(|p| branch_map.get(p))
            .map(|b| truncate_branch(b, 20))
            .unwrap_or_default();
        let branch_display = if branch.is_empty() {
            String::new()
        } else {
            format!("[{branch}]")
        };

        let line = format!(
            "{loc_padded}  {marker} {prov}  {state_padded}  {title_padded}  {branch_display:<22}  {age}"
        );
        lines.push(line);
    }

    lines
}

/// Entry point for `agtmux pick`.
pub async fn cmd_pick(
    socket_path: &str,
    dry_run: bool,
    waiting_only: bool,
    color: &str,
) -> anyhow::Result<()> {
    let _use_color = resolve_color(color);

    let panes = rpc_call(socket_path, "list_panes").await?;
    let arr = panes.as_array().cloned().unwrap_or_default();
    let branch_map = build_branch_map(&arr);

    let candidates = format_pick_candidates(&arr, &branch_map, waiting_only);

    if candidates.is_empty() {
        if waiting_only {
            eprintln!("no waiting agents");
        } else {
            eprintln!("no managed agents");
        }
        return Ok(());
    }

    let candidate_text = candidates.join("\n");

    if dry_run {
        println!("{candidate_text}");
        return Ok(());
    }

    // Check if fzf is available
    let fzf_available = Command::new("which")
        .arg("fzf")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if !fzf_available {
        eprintln!("error: fzf not found; install fzf or use --dry-run");
        std::process::exit(1);
    }

    // Spawn fzf
    let mut child = Command::new("fzf")
        .args(["--color=never", "--no-multi", "--ansi"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| anyhow::anyhow!("failed to spawn fzf: {e}"))?;

    // Write candidates to fzf stdin
    if let Some(ref mut stdin) = child.stdin {
        stdin
            .write_all(candidate_text.as_bytes())
            .map_err(|e| anyhow::anyhow!("failed to write to fzf stdin: {e}"))?;
    }
    // Drop stdin to signal EOF
    drop(child.stdin.take());

    let output = child
        .wait_with_output()
        .map_err(|e| anyhow::anyhow!("fzf failed: {e}"))?;

    if !output.status.success() {
        // User pressed Escape or Ctrl-C in fzf
        return Ok(());
    }

    let selected = String::from_utf8_lossy(&output.stdout);
    let selected = selected.trim();

    if selected.is_empty() {
        return Ok(());
    }

    // Parse session:window from the first token
    let target = selected.split_whitespace().next().unwrap_or("");
    if target.is_empty() {
        return Ok(());
    }

    // tmux switch-client -t session:window
    let status = Command::new("tmux")
        .args(["switch-client", "-t", target])
        .status()
        .map_err(|e| anyhow::anyhow!("failed to run tmux switch-client: {e}"))?;

    if !status.success() {
        // Fallback: try select-window (if not in tmux client context)
        let _ = Command::new("tmux")
            .args(["select-window", "-t", target])
            .status();
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pane(
        session_name: &str,
        window_name: &str,
        presence: &str,
        provider: Option<&str>,
        evidence_mode: &str,
        activity_state: &str,
    ) -> serde_json::Value {
        let mut v = serde_json::json!({
            "pane_id": "%0",
            "session_name": session_name,
            "session_id": "$0",
            "window_id": "@0",
            "window_name": window_name,
            "presence": presence,
            "evidence_mode": evidence_mode,
            "activity_state": activity_state,
            "current_cmd": "claude",
            "current_path": "/repo",
        });
        if let Some(p) = provider {
            v["provider"] = serde_json::Value::String(p.to_string());
        }
        v
    }

    #[test]
    fn format_pick_candidates_empty() {
        let panes: Vec<serde_json::Value> = vec![];
        let branch_map = HashMap::new();
        let result = format_pick_candidates(&panes, &branch_map, false);
        assert!(result.is_empty());
    }

    #[test]
    fn format_pick_candidates_basic() {
        let panes = vec![
            make_pane(
                "work",
                "api",
                "managed",
                Some("ClaudeCode"),
                "deterministic",
                "WaitingApproval",
            ),
            make_pane(
                "work",
                "dev",
                "managed",
                Some("Codex"),
                "deterministic",
                "Running",
            ),
            make_pane("work", "zsh", "unmanaged", None, "", ""),
        ];
        let branch_map: HashMap<String, String> =
            [("/repo".to_string(), "main".to_string())].into();
        let result = format_pick_candidates(&panes, &branch_map, false);

        assert_eq!(result.len(), 2, "only managed panes");
        assert!(result[0].contains("work:api"), "session:window present");
        assert!(result[0].contains("Claude"), "provider shown");
        assert!(result[0].contains("Waiting"), "state shown");
        assert!(result[0].contains("!"), "waiting marker");
        assert!(result[1].contains("work:dev"), "second pane");
        assert!(result[1].contains("Codex"), "second provider");
        assert!(result[1].contains("[main]"), "branch shown");
    }

    #[test]
    fn format_pick_candidates_waiting_filter() {
        let panes = vec![
            make_pane(
                "work",
                "api",
                "managed",
                Some("ClaudeCode"),
                "deterministic",
                "WaitingInput",
            ),
            make_pane(
                "work",
                "dev",
                "managed",
                Some("Codex"),
                "deterministic",
                "Running",
            ),
            make_pane(
                "work",
                "test",
                "managed",
                Some("ClaudeCode"),
                "deterministic",
                "Idle",
            ),
        ];
        let branch_map = HashMap::new();
        let result = format_pick_candidates(&panes, &branch_map, true);

        assert_eq!(result.len(), 1, "only waiting panes");
        assert!(result[0].contains("work:api"), "waiting pane included");
    }
}

//! UDS JSON-RPC client for CLI subcommands.

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

pub(crate) async fn rpc_call(socket_path: &str, method: &str) -> anyhow::Result<serde_json::Value> {
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

/// `agtmux bar` — single-line status for tmux status bar or terminal.
///
/// ANSI mode (default): " 1W 2R 2I" with colored output.
/// tmux mode (`--tmux`): `#[fg=yellow,bold] 1W#[default] #[fg=green] 2R#[default] 2I`
///
/// W/R/I are shown only when non-zero. Daemon unreachable: `--`.
pub async fn cmd_bar(socket_path: &str, tmux_mode: bool) -> anyhow::Result<()> {
    let panes = match rpc_call(socket_path, "list_panes").await {
        Ok(p) => p,
        Err(_) => {
            print!("--");
            return Ok(());
        }
    };

    let output = format_bar(&panes, tmux_mode);
    print!("{output}");
    Ok(())
}

/// Pure formatting logic for bar output, separated for testability.
pub(crate) fn format_bar(panes: &serde_json::Value, tmux_mode: bool) -> String {
    let arr = match panes.as_array() {
        Some(a) => a,
        None => return "--".to_string(),
    };

    let mut waiting = 0usize;
    let mut running = 0usize;
    let mut idle = 0usize;

    for pane in arr {
        if pane["presence"].as_str() != Some("managed") {
            continue;
        }
        match pane["activity_state"].as_str() {
            Some("WaitingInput") | Some("WaitingApproval") => waiting += 1,
            Some("Running") => running += 1,
            Some("Idle") => idle += 1,
            _ => {}
        }
    }

    if waiting == 0 && running == 0 && idle == 0 {
        return String::new();
    }

    let mut parts = Vec::new();

    if tmux_mode {
        if waiting > 0 {
            parts.push(format!("#[fg=yellow,bold] {waiting}W#[default]"));
        }
        if running > 0 {
            parts.push(format!("#[fg=green] {running}R#[default]"));
        }
        if idle > 0 {
            parts.push(format!(" {idle}I"));
        }
    } else {
        if waiting > 0 {
            parts.push(format!("\x1b[1;33m {waiting}W\x1b[0m"));
        }
        if running > 0 {
            parts.push(format!("\x1b[32m {running}R\x1b[0m"));
        }
        if idle > 0 {
            parts.push(format!(" {idle}I"));
        }
    }

    parts.join("")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pane(presence: &str, activity_state: &str) -> serde_json::Value {
        serde_json::json!({
            "pane_id": "%0",
            "session_name": "work",
            "session_id": "$0",
            "window_id": "@0",
            "window_name": "dev",
            "presence": presence,
            "evidence_mode": "deterministic",
            "activity_state": activity_state,
            "current_cmd": "claude",
            "current_path": "/repo",
        })
    }

    #[test]
    fn format_bar_empty() {
        let panes = serde_json::json!([]);
        assert_eq!(format_bar(&panes, false), "");
    }

    #[test]
    fn format_bar_ansi_mode() {
        let panes = serde_json::json!([
            make_pane("managed", "WaitingInput"),
            make_pane("managed", "Running"),
            make_pane("managed", "Running"),
            make_pane("managed", "Idle"),
            make_pane("managed", "Idle"),
            make_pane("unmanaged", ""),
        ]);
        let out = format_bar(&panes, false);
        assert!(out.contains("1W"), "waiting count");
        assert!(out.contains("2R"), "running count");
        assert!(out.contains("2I"), "idle count");
        assert!(out.contains('\x1b'), "ANSI codes present");
    }

    #[test]
    fn format_bar_tmux_mode() {
        let panes = serde_json::json!([
            make_pane("managed", "WaitingApproval"),
            make_pane("managed", "Running"),
        ]);
        let out = format_bar(&panes, true);
        assert!(out.contains("#[fg=yellow,bold]"), "tmux yellow for waiting");
        assert!(out.contains("1W"), "waiting count");
        assert!(out.contains("#[fg=green]"), "tmux green for running");
        assert!(out.contains("1R"), "running count");
        assert!(!out.contains('\x1b'), "no ANSI in tmux mode");
    }

    #[test]
    fn format_bar_only_nonzero() {
        let panes = serde_json::json!([make_pane("managed", "Running"),]);
        let out = format_bar(&panes, false);
        assert!(out.contains("1R"), "running shown");
        assert!(!out.contains('W'), "no waiting");
        assert!(!out.contains('I'), "no idle");
    }

    #[test]
    fn format_bar_daemon_unreachable() {
        // null result simulates unreachable
        let panes = serde_json::json!(null);
        assert_eq!(format_bar(&panes, false), "--");
    }

    #[test]
    fn format_bar_no_agents() {
        let panes = serde_json::json!([make_pane("unmanaged", ""),]);
        let out = format_bar(&panes, false);
        assert_eq!(out, "", "no agents = empty output");
    }
}

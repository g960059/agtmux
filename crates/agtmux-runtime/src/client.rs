//! UDS JSON-RPC client for CLI subcommands.

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

async fn rpc_call(socket_path: &str, method: &str) -> anyhow::Result<serde_json::Value> {
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

/// `agtmux status` — show daemon summary.
pub async fn cmd_status(socket_path: &str) -> anyhow::Result<()> {
    let panes = rpc_call(socket_path, "list_panes").await?;
    let health = rpc_call(socket_path, "list_source_health").await?;

    let pane_count = panes.as_array().map_or(0, |a| a.len());
    let agent_count = panes.as_array().map_or(0, |a| {
        a.iter()
            .filter(|p| p["presence"].as_str() == Some("managed"))
            .count()
    });
    let unmanaged_count = pane_count - agent_count;

    println!("agtmux daemon running");
    println!("Panes: {pane_count} total ({agent_count} agents, {unmanaged_count} unmanaged)");

    if let Some(sources) = health.as_array() {
        let health_strs: Vec<String> = sources
            .iter()
            .map(|s| {
                let kind = s[0].as_str().unwrap_or("?");
                let status = s[1]["status"].as_str().unwrap_or("unknown");
                format!("{kind}={status}")
            })
            .collect();
        println!("Sources: {}", health_strs.join(", "));
    }

    Ok(())
}

/// `agtmux list-panes` — print pane states as JSON.
pub async fn cmd_list_panes(socket_path: &str) -> anyhow::Result<()> {
    let panes = rpc_call(socket_path, "list_panes").await?;
    println!("{}", serde_json::to_string_pretty(&panes)?);
    Ok(())
}

/// `agtmux tmux-status` — single-line output for tmux status bar.
pub async fn cmd_tmux_status(socket_path: &str) -> anyhow::Result<()> {
    let panes = rpc_call(socket_path, "list_panes").await?;

    let pane_arr = panes.as_array();
    let agent_count = pane_arr.map_or(0, |a| {
        a.iter()
            .filter(|p| p["presence"].as_str() == Some("managed"))
            .count()
    });
    let unmanaged_count = pane_arr.map_or(0, |a| a.len()) - agent_count;

    print!("A:{agent_count} U:{unmanaged_count}");
    Ok(())
}

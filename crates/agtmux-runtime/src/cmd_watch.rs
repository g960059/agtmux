//! `agtmux watch` — live-refresh agent tree view.

use std::time::Duration;

use crate::client::rpc_call;
use crate::cmd_ls::format_ls_tree;
use crate::context::{build_branch_map, resolve_color};

/// Entry point for `agtmux watch`.
pub async fn cmd_watch(socket_path: &str, interval: u64, color: &str) -> anyhow::Result<()> {
    let use_color = resolve_color(color);

    loop {
        // Clear screen + cursor home
        print!("\x1b[2J\x1b[H");

        match rpc_call(socket_path, "list_panes").await {
            Ok(panes) => {
                let arr = panes.as_array().cloned().unwrap_or_default();
                let branch_map = build_branch_map(&arr);
                let output = format_ls_tree(&arr, &branch_map, use_color);
                if output.is_empty() {
                    println!("(no agents detected)");
                } else {
                    println!("{output}");
                }
            }
            Err(e) => {
                println!("Cannot connect to daemon: {e}");
            }
        }

        if use_color {
            println!("\n\x1b[2magtmux watch \u{2014} Ctrl-C to quit\x1b[0m");
        } else {
            println!("\nagtmux watch \u{2014} Ctrl-C to quit");
        }

        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(interval)) => {}
            _ = tokio::signal::ctrl_c() => { break; }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::cli::WatchOpts;

    #[test]
    fn watch_interval_default() {
        let opts = WatchOpts {
            session: None,
            interval: 1,
            color: "auto".to_string(),
        };
        assert_eq!(opts.interval, 1);
    }

    #[test]
    fn watch_interval_custom() {
        let opts = WatchOpts {
            session: None,
            interval: 5,
            color: "never".to_string(),
        };
        assert_eq!(opts.interval, 5);
        assert_eq!(opts.color, "never");
    }
}

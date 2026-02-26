//! CLI definition using clap derive.

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "agtmux", about = "tmux agent multiplexer")]
pub struct Cli {
    /// UDS socket path (default: /tmp/agtmux-$USER/agtmuxd.sock)
    #[arg(long, short = 's', global = true)]
    pub socket_path: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Start the daemon (poll loop + UDS server)
    Daemon(DaemonOpts),
    /// Show daemon status summary
    Status,
    /// List all pane states (JSON)
    ListPanes,
    /// Single-line output for tmux status bar
    TmuxStatus,
}

#[derive(clap::Args)]
pub struct DaemonOpts {
    /// Poll interval in milliseconds
    #[arg(long, default_value = "1000")]
    pub poll_interval_ms: u64,

    /// tmux socket path
    #[arg(long)]
    pub tmux_socket: Option<String>,
}

/// Default socket path using $USER for per-user isolation.
pub fn default_socket_path() -> String {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        return format!("{dir}/agtmux/agtmuxd.sock");
    }
    let user = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());
    format!("/tmp/agtmux-{user}/agtmuxd.sock")
}

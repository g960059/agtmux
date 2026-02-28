//! CLI definition using clap derive.

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "agtmux",
    about = "tmux agent multiplexer",
    subcommand_required = false,
    arg_required_else_help = false
)]
pub struct Cli {
    /// UDS socket path (default: /tmp/agtmux-$USER/agtmuxd.sock)
    #[arg(long, short = 's', global = true)]
    pub socket_path: Option<String>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Start the daemon (poll loop + UDS server)
    Daemon(DaemonOpts),
    /// List agents in hierarchical tree (default), session summary, or flat pane view
    Ls(LsOpts),
    /// Single-line status bar output (ANSI or tmux color codes)
    Bar(BarOpts),
    /// Interactive agent picker (T-139b)
    Pick(PickOpts),
    /// Watch agent state changes in real-time (T-139c)
    Watch(WatchOpts),
    /// Wait for agent state condition (T-139d)
    Wait(WaitOpts),
    /// Machine-readable JSON output (T-139d)
    Json(JsonOpts),
    /// Configure Claude Code hooks for agtmux integration
    SetupHooks(SetupHooksOpts),
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

#[derive(clap::Args, Default)]
pub struct LsOpts {
    /// Grouping: tree (default), session, pane
    #[arg(long, default_value = "tree")]
    pub group: String,

    /// Color output: always, never, auto
    #[arg(long, default_value = "auto")]
    pub color: String,

    /// Show Nerd Font icons (requires Nerd Font)
    #[arg(long)]
    pub icons: bool,
}

#[derive(clap::Args)]
pub struct BarOpts {
    /// Output tmux color codes (#[fg=...]) instead of ANSI
    #[arg(long)]
    pub tmux: bool,
}

#[derive(clap::Args)]
pub struct PickOpts {
    /// Print candidate lines to stdout without launching fzf
    #[arg(long)]
    pub dry_run: bool,

    /// Show only Waiting panes
    #[arg(long)]
    pub waiting: bool,

    /// Color output: always, never, auto
    #[arg(long, default_value = "auto")]
    pub color: String,
}

#[derive(clap::Args)]
pub struct WatchOpts {
    /// Filter by session name
    #[arg(long)]
    pub session: Option<String>,

    /// Refresh interval in seconds
    #[arg(long, default_value = "1")]
    pub interval: u64,

    /// Color output: always, never, auto
    #[arg(long, default_value = "auto")]
    pub color: String,
}

#[derive(clap::Args)]
pub struct WaitOpts {
    /// Wait until all agents are idle (default)
    #[arg(long)]
    pub idle: bool,

    /// Wait until no agents are in Waiting state
    #[arg(long)]
    pub no_waiting: bool,

    /// Scope to specific session
    #[arg(long, short = 's')]
    pub session: Option<String>,

    /// Timeout in seconds
    #[arg(long)]
    pub timeout: Option<u64>,

    /// Suppress progress output
    #[arg(long)]
    pub quiet: bool,
}

#[derive(clap::Args)]
pub struct JsonOpts {
    /// Show source health instead of pane list
    #[arg(long)]
    pub health: bool,
}

#[derive(clap::Args)]
pub struct SetupHooksOpts {
    /// Scope: "project" writes to .claude/settings.json, "user" writes to ~/.claude/settings.json
    #[arg(long, default_value = "project")]
    pub scope: String,

    /// Path to the hook script (auto-detected if omitted)
    #[arg(long)]
    pub hook_script: Option<String>,
}

/// Default socket path using $USER for per-user isolation.
pub fn default_socket_path() -> String {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        return format!("{dir}/agtmux/agtmuxd.sock");
    }
    let user = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());
    format!("/tmp/agtmux-{user}/agtmuxd.sock")
}

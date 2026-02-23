use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agtmux_core::adapt::loader::{builtin_adapters, load_adapters_from_dir, merge_adapters};
use agtmux_tmux::TmuxBackend;
use clap::{Parser, Subcommand};
use tokio::sync::{broadcast, mpsc};

use agtmux_daemon::client::DaemonClient;
use agtmux_daemon::orchestrator::Orchestrator;
use agtmux_daemon::recorder::Recorder;
use agtmux_daemon::server::{DaemonServer, DaemonState, SharedState};
use agtmux_daemon::sources::hook::HookSource;
use agtmux_daemon::sources::poller::PollerSource;
use agtmux_daemon::status::format_status;
use agtmux_daemon::tmux_status::format_tmux_status;

/// Default directory for runtime sockets.
const DEFAULT_SOCKET_DIR: &str = "/tmp/agtmux";
const DEFAULT_DAEMON_SOCKET: &str = "/tmp/agtmux/agtmuxd.sock";
const DEFAULT_HOOK_SOCKET: &str = "/tmp/agtmux/hook.sock";

#[derive(Parser)]
#[command(name = "agtmux", about = "AI agent terminal multiplexer monitor")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the daemon (default when no subcommand given)
    Daemon {
        /// Daemon socket path for client connections
        #[arg(long, default_value = DEFAULT_DAEMON_SOCKET)]
        socket: String,

        /// Hook socket path for agent hook events
        #[arg(long, default_value = DEFAULT_HOOK_SOCKET)]
        hook_socket: String,

        /// Polling interval in milliseconds
        #[arg(long, default_value_t = 500)]
        poll_interval_ms: u64,

        /// Record all state events to a JSONL file for later labeling/accuracy analysis
        #[arg(long)]
        record: Option<String>,

        /// Directory containing custom provider TOML files (overrides builtins)
        #[arg(long)]
        config_dir: Option<String>,
    },
    /// Show pane status (one-shot)
    Status {
        /// Daemon socket path
        #[arg(long, default_value = DEFAULT_DAEMON_SOCKET)]
        socket: String,
    },
    /// Interactive TUI — live pane monitor
    Tui {
        /// Daemon socket path to connect to
        #[arg(long, default_value = DEFAULT_DAEMON_SOCKET)]
        socket: String,
    },
    /// Tmux status line output
    TmuxStatus {
        /// Daemon socket path
        #[arg(long, default_value = DEFAULT_DAEMON_SOCKET)]
        socket: String,
    },
    /// Label recorded events for accuracy measurement
    Label {
        /// Path to the recorded JSONL file
        file: String,
    },
    /// Compute accuracy metrics from labeled events
    Accuracy {
        /// Path to the labeled JSONL file
        file: String,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing. Respects RUST_LOG env var, defaults to info.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        // Default to daemon when no subcommand is given.
        None | Some(Commands::Daemon { .. }) => {
            let (socket, hook_socket, poll_interval_ms, record, config_dir) = match cli.command {
                Some(Commands::Daemon {
                    socket,
                    hook_socket,
                    poll_interval_ms,
                    record,
                    config_dir,
                }) => (socket, hook_socket, poll_interval_ms, record, config_dir),
                _ => (
                    DEFAULT_DAEMON_SOCKET.to_string(),
                    DEFAULT_HOOK_SOCKET.to_string(),
                    500,
                    None,
                    None,
                ),
            };
            run_daemon(socket, hook_socket, poll_interval_ms, record, config_dir).await?;
        }
        Some(Commands::Status { socket }) => {
            run_status(&socket).await?;
        }
        Some(Commands::Tui { socket }) => {
            agtmux_daemon::tui::run_tui(std::path::Path::new(&socket)).await?;
        }
        Some(Commands::TmuxStatus { socket }) => {
            run_tmux_status(&socket).await;
        }
        Some(Commands::Label { file }) => {
            agtmux_daemon::label::run_label(std::path::Path::new(&file))?;
        }
        Some(Commands::Accuracy { file }) => {
            agtmux_daemon::accuracy::run_accuracy(std::path::Path::new(&file))?;
        }
    }

    Ok(())
}

async fn run_daemon(
    socket: String,
    hook_socket: String,
    poll_interval_ms: u64,
    record: Option<String>,
    config_dir: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!(
        socket = %socket,
        hook_socket = %hook_socket,
        poll_interval_ms = poll_interval_ms,
        record = ?record,
        config_dir = ?config_dir,
        "starting agtmux daemon"
    );

    // Ensure the socket directory exists.
    let socket_dir = PathBuf::from(DEFAULT_SOCKET_DIR);
    std::fs::create_dir_all(&socket_dir)?;

    // ---------------------------------------------------------------
    // 1. Create channels
    // ---------------------------------------------------------------
    // SourceEvent channel: sources -> orchestrator (capacity 256)
    let (source_tx, source_rx) = mpsc::channel(256);
    // StateNotification broadcast: orchestrator -> server/clients (capacity 64)
    let (notify_tx, _notify_rx) = broadcast::channel(64);

    // ---------------------------------------------------------------
    // 2. Create the TmuxBackend
    // ---------------------------------------------------------------
    let backend = Arc::new(TmuxBackend::new());

    // ---------------------------------------------------------------
    // 3. Create PollerSource with adapters (builtins + optional runtime overrides)
    // ---------------------------------------------------------------
    let adapters = {
        let builtins = builtin_adapters();
        if let Some(ref dir) = config_dir {
            let dir_path = PathBuf::from(dir);
            match load_adapters_from_dir(&dir_path) {
                Ok(runtime) => {
                    tracing::info!(
                        dir = %dir,
                        count = runtime.len(),
                        "loaded runtime provider configs"
                    );
                    merge_adapters(builtins, runtime)
                }
                Err(e) => {
                    tracing::warn!(dir = %dir, error = %e, "failed to load runtime configs, using builtins only");
                    builtins
                }
            }
        } else {
            builtins
        }
    };

    let (detectors, builders): (Vec<_>, Vec<_>) = adapters.into_iter().unzip();

    let mut poller = PollerSource::new(
        backend,
        builders,
        source_tx.clone(),
        Duration::from_millis(poll_interval_ms),
    );

    // ---------------------------------------------------------------
    // 4. Create HookSource
    // ---------------------------------------------------------------
    let hook_source = HookSource::new(source_tx.clone(), PathBuf::from(&hook_socket));

    // ---------------------------------------------------------------
    // 5. Create shared state (orchestrator writes, server reads)
    // ---------------------------------------------------------------
    let shared_state: SharedState =
        std::sync::Arc::new(tokio::sync::RwLock::new(DaemonState::default()));

    // ---------------------------------------------------------------
    // 6. Create Orchestrator (with shared_state so it can write pane info)
    // ---------------------------------------------------------------
    let mut orchestrator =
        Orchestrator::new(source_rx, notify_tx.clone(), shared_state.clone(), detectors);

    // ---------------------------------------------------------------
    // 7. Create DaemonServer (reads shared_state for list_panes API)
    // ---------------------------------------------------------------
    let server = DaemonServer::new(PathBuf::from(&socket), shared_state, notify_tx.clone());

    // ---------------------------------------------------------------
    // 8. Optionally create the JSONL recorder
    // ---------------------------------------------------------------
    let mut recorder = match record {
        Some(ref path) => {
            let recorder_rx = notify_tx.subscribe();
            let r = Recorder::new(std::path::Path::new(path), recorder_rx)?;
            tracing::info!(path = %path, "JSONL recorder enabled");
            Some(r)
        }
        None => None,
    };

    // ---------------------------------------------------------------
    // 9. Spawn all tasks, wait for shutdown
    // ---------------------------------------------------------------
    tracing::info!("all components created, starting event loops");

    tokio::select! {
        _ = orchestrator.run() => {
            tracing::warn!("orchestrator exited unexpectedly");
        }
        _ = poller.run() => {
            tracing::warn!("poller exited unexpectedly");
        }
        result = hook_source.run() => {
            match result {
                Ok(()) => tracing::warn!("hook source exited unexpectedly"),
                Err(e) => tracing::warn!("hook source error: {e}"),
            }
        }
        result = server.run() => {
            match result {
                Ok(()) => tracing::warn!("server exited unexpectedly"),
                Err(e) => tracing::warn!("server error: {e}"),
            }
        }
        _ = async {
            match recorder.as_mut() {
                Some(r) => r.run().await,
                None => std::future::pending().await,
            }
        } => {
            tracing::warn!("recorder exited unexpectedly");
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("received ctrl-c, shutting down");
        }
    }

    // Cleanup: remove socket files.
    for path in [&socket, &hook_socket] {
        let p = PathBuf::from(path);
        if p.exists() {
            if let Err(e) = std::fs::remove_file(&p) {
                tracing::warn!(path = %p.display(), "failed to remove socket file: {e}");
            }
        }
    }

    tracing::info!("agtmux daemon stopped");
    Ok(())
}

/// Connect to the daemon, fetch pane list, and print a formatted status overview.
async fn run_status(socket: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut client = DaemonClient::connect(socket).await.map_err(|e| {
        eprintln!("Failed to connect to daemon at {}: {}", socket, e);
        eprintln!("Is the daemon running? Start it with: agtmux daemon");
        e
    })?;

    let panes = client.list_panes().await?;
    print!("{}", format_status(&panes));
    Ok(())
}

/// Connect to the daemon, fetch pane list, and print a compact tmux status string.
///
/// On any error (daemon not running, socket missing, etc.) prints an empty
/// string so the tmux status line is not broken.
async fn run_tmux_status(socket: &str) {
    let output = match DaemonClient::connect(socket).await {
        Ok(mut client) => match client.list_panes().await {
            Ok(panes) => format_tmux_status(&panes),
            Err(_) => String::new(),
        },
        Err(_) => String::new(),
    };
    println!("{}", output);
}

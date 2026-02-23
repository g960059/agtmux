use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agtmux_core::adapt::loader::{
    builtin_adapters, builtin_normalizers, load_adapters_from_dir, merge_adapters,
};
use agtmux_tmux::TmuxBackend;
use clap::{Parser, Subcommand};
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use agtmux_daemon::client::DaemonClient;
use agtmux_daemon::orchestrator::{Orchestrator, PaneState};
use agtmux_daemon::recorder::Recorder;
use agtmux_daemon::server::{DaemonServer, DaemonState, PaneInfo, SharedState};
use agtmux_daemon::ws_server::WsServer;
use agtmux_daemon::sources::hook::HookSource;
use agtmux_daemon::sources::poller::PollerSource;
use agtmux_daemon::status::format_status;
use agtmux_daemon::store::Store;
use agtmux_daemon::tmux_status::format_tmux_status;

/// Default directory for runtime sockets.
const DEFAULT_SOCKET_DIR: &str = "/tmp/agtmux";
const DEFAULT_DAEMON_SOCKET: &str = "/tmp/agtmux/agtmuxd.sock";
const DEFAULT_HOOK_SOCKET: &str = "/tmp/agtmux/hook.sock";
const DEFAULT_DB_PATH: &str = "/tmp/agtmux.db";

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

        /// SQLite database path for state persistence across restarts
        #[arg(long, default_value = DEFAULT_DB_PATH)]
        db_path: String,

        /// WebSocket listen address for browser/Tauri clients
        #[arg(long, default_value = "127.0.0.1:9780")]
        ws_addr: String,
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
    /// Automatically label a daemon recording using expected-labels JSONL
    AutoLabel {
        /// Path to the daemon recording JSONL
        recording: String,
        /// Path to the expected-labels JSONL
        expected: String,
        /// Timestamp matching window in seconds
        #[arg(long, default_value_t = 5)]
        window_sec: u64,
        /// Output path
        #[arg(long)]
        output: Option<String>,
    },
    /// Run preflight checks for live testing
    Preflight {
        /// Also check that real AI CLIs are available
        #[arg(long, default_value_t = false)]
        real: bool,
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
            let (socket, hook_socket, poll_interval_ms, record, config_dir, db_path, ws_addr) =
                match cli.command {
                    Some(Commands::Daemon {
                        socket,
                        hook_socket,
                        poll_interval_ms,
                        record,
                        config_dir,
                        db_path,
                        ws_addr,
                    }) => (socket, hook_socket, poll_interval_ms, record, config_dir, db_path, ws_addr),
                    _ => (
                        DEFAULT_DAEMON_SOCKET.to_string(),
                        DEFAULT_HOOK_SOCKET.to_string(),
                        500,
                        None,
                        None,
                        DEFAULT_DB_PATH.to_string(),
                        "127.0.0.1:9780".to_string(),
                    ),
                };
            run_daemon(socket, hook_socket, poll_interval_ms, record, config_dir, db_path, ws_addr).await?;
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
            let passed = agtmux_daemon::accuracy::run_accuracy(std::path::Path::new(&file))?;
            if !passed {
                std::process::exit(1);
            }
        }
        Some(Commands::AutoLabel {
            recording,
            expected,
            window_sec,
            output,
        }) => {
            let output_path = output.unwrap_or_else(|| {
                let p = std::path::Path::new(&recording);
                let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("recording");
                let parent = p.parent().unwrap_or_else(|| std::path::Path::new("."));
                parent.join(format!("{}.labeled.jsonl", stem)).to_string_lossy().into_owned()
            });
            let report = agtmux_daemon::auto_label::run_auto_label(
                std::path::Path::new(&recording),
                std::path::Path::new(&expected),
                chrono::Duration::seconds(window_sec as i64),
                std::path::Path::new(&output_path),
            )?;
            println!(
                "Auto-labeled: {} total, {} labeled, {} unlabeled -> {}",
                report.total_events, report.labeled_events, report.unlabeled_events, output_path
            );
        }
        Some(Commands::Preflight { real }) => {
            std::process::exit(agtmux_daemon::preflight::run_preflight(real));
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
    db_path: String,
    ws_addr: String,
) -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!(
        socket = %socket,
        hook_socket = %hook_socket,
        poll_interval_ms = poll_interval_ms,
        record = ?record,
        config_dir = ?config_dir,
        db_path = %db_path,
        ws_addr = %ws_addr,
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

    // ---------------------------------------------------------------
    // Create the shared CancellationToken for graceful shutdown
    // ---------------------------------------------------------------
    let cancel = CancellationToken::new();

    let mut poller = PollerSource::with_cancel(
        backend,
        builders,
        source_tx.clone(),
        Duration::from_millis(poll_interval_ms),
        cancel.clone(),
    );

    // ---------------------------------------------------------------
    // 4. Create HookSource
    // ---------------------------------------------------------------
    let hook_source = HookSource::with_cancel(
        source_tx.clone(),
        PathBuf::from(&hook_socket),
        cancel.clone(),
    );

    // ---------------------------------------------------------------
    // 5. Open SQLite store and load persisted state
    // ---------------------------------------------------------------
    let store = Store::open(std::path::Path::new(&db_path))?;
    let persisted_states = store.load_all_pane_states().unwrap_or_else(|e| {
        tracing::warn!(error = %e, "failed to load persisted state, starting fresh");
        Vec::new()
    });

    let initial_panes: Vec<PaneInfo> = persisted_states.iter().map(PaneInfo::from).collect();
    tracing::info!(
        count = persisted_states.len(),
        "loaded persisted pane states from SQLite"
    );

    let store = Arc::new(Mutex::new(store));

    // ---------------------------------------------------------------
    // 6. Create shared state (orchestrator writes, server reads)
    // ---------------------------------------------------------------
    let shared_state: SharedState =
        std::sync::Arc::new(tokio::sync::RwLock::new(DaemonState {
            panes: initial_panes,
        }));

    // ---------------------------------------------------------------
    // 7. Create Orchestrator (with shared_state so it can write pane info)
    // ---------------------------------------------------------------
    let normalizers = builtin_normalizers();
    let mut orchestrator = Orchestrator::with_cancel(
        source_rx,
        notify_tx.clone(),
        shared_state.clone(),
        detectors,
        normalizers,
        cancel.clone(),
    );

    // ---------------------------------------------------------------
    // 8. Create DaemonServer (reads shared_state for list_panes API)
    // ---------------------------------------------------------------
    let server = DaemonServer::with_cancel(
        PathBuf::from(&socket),
        shared_state.clone(),
        notify_tx.clone(),
        cancel.clone(),
    );

    // ---------------------------------------------------------------
    // 8b. Create WsServer (WebSocket, same protocol as Unix socket)
    // ---------------------------------------------------------------
    let ws_addr: std::net::SocketAddr = ws_addr.parse()?;
    let ws_server = WsServer::new(
        ws_addr,
        shared_state.clone(),
        notify_tx.clone(),
        cancel.clone(),
    );

    // ---------------------------------------------------------------
    // 9. Optionally create the JSONL recorder
    // ---------------------------------------------------------------
    let mut recorder = match record {
        Some(ref path) => {
            let recorder_rx = notify_tx.subscribe();
            let r = Recorder::with_cancel(
                std::path::Path::new(path),
                recorder_rx,
                cancel.clone(),
            )?;
            tracing::info!(path = %path, "JSONL recorder enabled");
            Some(r)
        }
        None => None,
    };

    // ---------------------------------------------------------------
    // 10. Spawn all tasks, wait for Ctrl+C then cancel
    // ---------------------------------------------------------------
    tracing::info!("all components created, starting event loops");

    let save_store = Arc::clone(&store);
    let save_shared = shared_state.clone();
    let save_cancel = cancel.clone();

    let orch_handle = tokio::spawn(async move { orchestrator.run().await });
    let poller_handle = tokio::spawn(async move { poller.run().await });
    let hook_handle = tokio::spawn(async move { hook_source.run().await });
    let server_handle = tokio::spawn(async move { server.run().await });
    let ws_handle = tokio::spawn(async move { ws_server.run().await });
    let recorder_cancel = cancel.clone();
    let recorder_handle = tokio::spawn(async move {
        if let Some(ref mut r) = recorder {
            r.run().await;
        } else {
            // No recorder configured; park this task until cancelled.
            recorder_cancel.cancelled().await;
        }
    });
    let save_handle = tokio::spawn(async move {
        periodic_save(save_store, save_shared, Duration::from_secs(5), save_cancel).await
    });

    // Wait for Ctrl+C, then trigger graceful shutdown via the token.
    tokio::signal::ctrl_c().await.ok();
    tracing::info!("received ctrl-c, initiating graceful shutdown");
    cancel.cancel();

    // Give tasks up to 3 seconds to finish draining.
    let _ = tokio::time::timeout(Duration::from_secs(3), async {
        let _ = tokio::join!(orch_handle, poller_handle, hook_handle, server_handle, ws_handle, recorder_handle, save_handle);
    }).await;

    // Final save before shutdown.
    {
        let state = shared_state.read().await;
        let st = store.lock().unwrap();
        for pane in &state.panes {
            if let Err(e) = st.save_pane_state(&PaneState::from(pane)) {
                tracing::warn!(pane_id = %pane.pane_id, error = %e, "failed to save pane on shutdown");
            }
        }
    }
    tracing::info!("final state persisted to SQLite");

    // Cleanup: remove socket files.
    for path in [&socket, &hook_socket] {
        let p = PathBuf::from(path);
        if p.exists() {
            if let Err(e) = std::fs::remove_file(&p) {
                tracing::warn!(path = %p.display(), "failed to remove socket file: {e}");
            }
        }
    }

    tracing::info!("agtmux daemon shutdown complete");
    Ok(())
}

/// Periodically persist shared state to the SQLite store until cancelled.
async fn periodic_save(
    store: Arc<Mutex<Store>>,
    shared: SharedState,
    interval: Duration,
    cancel: CancellationToken,
) {
    let mut ticker = tokio::time::interval(interval);
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let state = shared.read().await;
                // SAFETY: MutexGuard is never held across an .await point.
                // The shared.read().await completes before we lock the store.
                let st = store.lock().unwrap();
                for pane in &state.panes {
                    if let Err(e) = st.save_pane_state(&PaneState::from(pane)) {
                        tracing::warn!(pane_id = %pane.pane_id, error = %e, "periodic save failed");
                    }
                }
            }
            _ = cancel.cancelled() => {
                tracing::info!("periodic save: cancellation requested, shutting down");
                break;
            }
        }
    }
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

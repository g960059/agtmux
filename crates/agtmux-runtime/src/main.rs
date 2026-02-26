//! agtmux: tmux agent multiplexer runtime binary.
//! Single-process binary embedding all MVP components in-process.
//!
//! Architecture ref: docs/30_architecture.md C-016, docs/40_design.md Section 9

use clap::Parser;

mod cli;
mod client;
#[allow(dead_code)] // Skeleton module — wired into poll_tick once Codex protocol is finalized
mod codex_poller;
mod poll_loop;
mod server;
mod setup_hooks;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = cli::Cli::parse();

    match args.command {
        cli::Command::Daemon(opts) => {
            let filter = std::env::var("AGTMUX_LOG")
                .or_else(|_| std::env::var("RUST_LOG"))
                .unwrap_or_else(|_| "info".to_string());
            tracing_subscriber::fmt()
                .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
                .init();

            tracing::info!("agtmux daemon starting");

            let socket_path = args.socket_path.unwrap_or_else(cli::default_socket_path);
            poll_loop::run_daemon(opts, &socket_path).await?;
        }
        cli::Command::Status => {
            let socket_path = args.socket_path.unwrap_or_else(cli::default_socket_path);
            client::cmd_status(&socket_path).await?;
        }
        cli::Command::ListPanes => {
            let socket_path = args.socket_path.unwrap_or_else(cli::default_socket_path);
            client::cmd_list_panes(&socket_path).await?;
        }
        cli::Command::TmuxStatus => {
            let socket_path = args.socket_path.unwrap_or_else(cli::default_socket_path);
            client::cmd_tmux_status(&socket_path).await?;
        }
        cli::Command::SetupHooks(opts) => {
            let path = setup_hooks::apply_hooks(&opts)?;
            println!("hooks written to {}", path.display());
        }
    }

    Ok(())
}

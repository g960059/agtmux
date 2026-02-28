//! agtmux: tmux agent multiplexer runtime binary.
//! Single-process binary embedding all MVP components in-process.
//!
//! Architecture ref: docs/30_architecture.md C-016, docs/40_design.md Section 9

use clap::Parser;

mod cli;
mod client;
mod cmd_json;
mod cmd_ls;
mod cmd_pick;
mod cmd_wait;
mod cmd_watch;
#[allow(dead_code)] // Skeleton module — wired into poll_tick once Codex protocol is finalized
mod codex_poller;
mod context;
mod poll_loop;
mod server;
mod setup_hooks;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = cli::Cli::parse();

    let command = args
        .command
        .unwrap_or_else(|| cli::Command::Ls(cli::LsOpts::default()));

    match command {
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
        cli::Command::Ls(opts) => {
            let socket_path = args.socket_path.unwrap_or_else(cli::default_socket_path);
            let use_color = context::resolve_color(&opts.color);
            cmd_ls::cmd_ls(&socket_path, &opts.group, use_color).await?;
        }
        cli::Command::Bar(opts) => {
            let socket_path = args.socket_path.unwrap_or_else(cli::default_socket_path);
            client::cmd_bar(&socket_path, opts.tmux).await?;
        }
        cli::Command::Pick(opts) => {
            let socket_path = args.socket_path.unwrap_or_else(cli::default_socket_path);
            cmd_pick::cmd_pick(&socket_path, opts.dry_run, opts.waiting, &opts.color).await?;
        }
        cli::Command::Watch(opts) => {
            let socket_path = args.socket_path.unwrap_or_else(cli::default_socket_path);
            cmd_watch::cmd_watch(&socket_path, opts.interval, &opts.color).await?;
        }
        cli::Command::Wait(opts) => {
            let socket_path = args.socket_path.unwrap_or_else(cli::default_socket_path);
            let condition = if opts.no_waiting {
                cmd_wait::WaitCondition::NoWaiting
            } else {
                cmd_wait::WaitCondition::Idle
            };
            let exit_code = cmd_wait::cmd_wait(
                &socket_path,
                condition,
                opts.session.as_deref(),
                opts.timeout,
                opts.quiet,
            )
            .await;
            if exit_code != 0 {
                std::process::exit(exit_code);
            }
        }
        cli::Command::Json(opts) => {
            let socket_path = args.socket_path.unwrap_or_else(cli::default_socket_path);
            cmd_json::cmd_json(&socket_path, opts.health).await?;
        }
        cli::Command::SetupHooks(opts) => {
            let path = setup_hooks::apply_hooks(&opts)?;
            println!("hooks written to {}", path.display());
        }
    }

    Ok(())
}

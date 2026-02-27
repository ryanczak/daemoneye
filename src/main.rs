mod ai;
mod config;
mod sys_context;
mod daemon;
mod ipc;
mod tmux;
mod client;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the background daemon
    Daemon {
        /// Redirect daemon output to FILE instead of stdout (default: ~/.t1000/daemon.log)
        #[arg(long, value_name = "FILE")]
        log_file: Option<PathBuf>,
    },
    /// Tail the daemon log
    Logs {
        /// Log file to tail (default: ~/.t1000/daemon.log)
        #[arg(long, value_name = "FILE")]
        log_file: Option<PathBuf>,
    },
    /// Chat with the AI agent
    Chat,
    /// Ask the AI agent a question
    Ask { query: String },
    /// Check whether the daemon is running
    Ping,
    /// Stop the background daemon
    Stop,
    /// Print the tmux configuration for T1000
    Setup,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if let Err(e) = config::Config::ensure_dirs() {
        eprintln!("Warning: could not initialise config directory: {}", e);
    }

    let cli = Cli::parse();

    match cli.command {
        Commands::Daemon { log_file } => {
            let log_file = Some(log_file.unwrap_or_else(config::default_log_path));
            daemon::run_daemon(log_file).await?;
        }
        Commands::Logs { log_file } => {
            let path = log_file.unwrap_or_else(config::default_log_path);
            client::run_logs(path)?;
        }
        Commands::Chat => {
            client::run_chat().await?;
        }
        Commands::Ask { query } => {
            client::run_ask(query).await?;
        }
        Commands::Ping => {
            client::run_ping().await?;
        }
        Commands::Stop => {
            client::run_stop().await?;
        }
        Commands::Setup => {
            client::run_setup()?;
        }
    }

    Ok(())
}

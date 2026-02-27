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
        /// Log to the console instead of a file (useful for troubleshooting)
        #[arg(long)]
        console: bool,
        /// Write command execution audit log to FILE (default: ~/.t1000/commands.log)
        #[arg(long, value_name = "FILE")]
        command_log_file: Option<PathBuf>,
        /// Disable command execution audit logging
        #[arg(long)]
        no_command_log: bool,
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
        Commands::Daemon { log_file, console, command_log_file, no_command_log } => {
            let log_file = if console {
                None
            } else {
                Some(log_file.unwrap_or_else(config::default_log_path))
            };
            let command_log = if no_command_log {
                None
            } else {
                Some(command_log_file.unwrap_or_else(|| config::config_dir().join("commands.log")))
            };
            daemon::run_daemon(log_file, command_log).await?;
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

mod ai;
mod cli;
mod config;
mod daemon;
mod log;
mod ipc;
mod runbook;
mod scheduler;
mod scripts;
mod sys_context;
mod tmux;

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
        /// Redirect daemon output to FILE instead of stdout (default: ~/.daemoneye/daemon.log)
        #[arg(long, value_name = "FILE")]
        log_file: Option<PathBuf>,
        /// Log to the console instead of a file (useful for troubleshooting)
        #[arg(long)]
        console: bool,
    },
    /// Tail the daemon log
    Logs {
        /// Log file to tail (default: ~/.daemoneye/daemon.log)
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
    /// Print the tmux configuration for DaemonEye
    Setup,
    /// List available prompts in ~/.daemoneye/prompts/
    Prompts,
    /// List scripts in ~/.daemoneye/scripts/
    Scripts,
    /// Manage scheduled jobs
    Schedule {
        #[command(subcommand)]
        cmd: SchedCommands,
    },
    /// Internal out-of-band notifications (e.g. from tmux hooks)
    Notify {
        #[command(subcommand)]
        cmd: NotifyCommands,
    },
}

#[derive(Subcommand)]
enum NotifyCommands {
    /// Notify that a monitored pane has produced output
    Activity {
        /// Target pane ID (e.g. %3)
        pane_id: String,
        /// The integer index of the alert-activity hook
        hook_index: usize,
        /// Target session name where the hook was set
        session_name: String,
    },
}

#[derive(Subcommand)]
enum SchedCommands {
    /// List all scheduled jobs
    List,
    /// Cancel a scheduled job by UUID
    Cancel { id: String },
    /// Permanently delete a scheduled job by UUID
    Delete { id: String },
    /// List leftover de-* tmux windows from failed scheduled jobs
    Windows,
}

// main() is a plain synchronous function so we can fork() before the tokio
// runtime starts.  Forking inside a live multi-threaded runtime is unsafe
// (only the calling thread survives in the child but mutex state from other
// threads may be inconsistent).
fn main() -> anyhow::Result<()> {
    if let Err(e) = config::Config::ensure_dirs() {
        eprintln!("Warning: could not initialise config directory: {}", e);
    }

    let cli = Cli::parse();

    // For `daemon` without `--console`, fork into the background before
    // starting the async runtime so the calling shell is released immediately.
    if let Commands::Daemon { console: false, .. } = &cli.command {
        unsafe {
            let pid = libc::fork();
            if pid < 0 {
                anyhow::bail!("fork() failed: {}", std::io::Error::last_os_error());
            }
            if pid > 0 {
                // Parent: report the child PID and exit cleanly.
                println!("daemoneye daemon started (PID {})", pid);
                return Ok(());
            }
            // Child: create a new session so we are no longer attached to the
            // calling terminal, then redirect stdin from /dev/null.
            libc::setsid();
            let devnull = libc::open(
                b"/dev/null\0".as_ptr() as *const libc::c_char,
                libc::O_RDONLY,
            );
            if devnull >= 0 {
                libc::dup2(devnull, libc::STDIN_FILENO);
                libc::close(devnull);
            }
        }
    }

    // Build the tokio runtime and run async work in the child (or directly
    // for --console / all other subcommands).
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async_main(cli))
}

async fn async_main(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Commands::Daemon {
            log_file,
            console,
        } => {
            let log_file = if console {
                None
            } else {
                Some(log_file.unwrap_or_else(config::default_log_path))
            };
            daemon::run_daemon(log_file).await?;
        }
        Commands::Logs { log_file } => {
            let path = log_file.unwrap_or_else(config::default_log_path);
            cli::run_logs(path)?;
        }
        Commands::Chat => {
            cli::run_chat().await?;
        }
        Commands::Ask { query } => {
            cli::run_ask(query).await?;
        }
        Commands::Ping => {
            cli::run_ping().await?;
        }
        Commands::Stop => {
            cli::run_stop().await?;
        }
        Commands::Setup => {
            cli::run_setup()?;
        }
        Commands::Prompts => {
            cli::run_prompts()?;
        }
        Commands::Scripts => {
            cli::run_scripts()?;
        }
        Commands::Schedule { cmd } => match cmd {
            SchedCommands::List => {
                cli::run_sched_list()?;
            }
            SchedCommands::Cancel { id } => {
                cli::run_sched_cancel(id)?;
            }
            SchedCommands::Delete { id } => {
                cli::run_sched_delete(id)?;
            }
            SchedCommands::Windows => {
                cli::run_sched_windows()?;
            }
        },
        Commands::Notify { cmd } => match cmd {
            NotifyCommands::Activity {
                pane_id,
                hook_index,
                session_name,
            } => {
                cli::run_notify_activity(pane_id, hook_index, session_name).await?;
            }
        },
    }

    Ok(())
}

mod ai;
mod cli;
mod config;
mod daemon;
mod ipc;
mod log;
mod manifest;
mod memory;
mod pane_prefs;
mod runbook;
mod scheduler;
mod scripts;
mod search;
mod sys_context;
mod tmux;
pub(crate) mod util;
mod webhook;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Single global lock used by tests that mutate the HOME environment variable.
/// All test modules that call `env::set_var("HOME", ...)` must hold this lock.
#[cfg(test)]
pub(crate) static TEST_HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
        /// Override the tmux session name from config.toml [daemon] tmux_session.
        /// Useful for testing or running multiple daemon instances.
        #[arg(long, value_name = "NAME")]
        session: Option<String>,
    },
    /// Tail the daemon log
    Logs {
        /// Log file to tail (default: ~/.daemoneye/daemon.log)
        #[arg(long, value_name = "FILE")]
        log_file: Option<PathBuf>,
    },
    /// Chat with the AI agent
    Chat {
        /// Override the tmux session to attach to (overrides config.daemon.tmux_session).
        /// When set and running outside tmux, opens a chat window in this session
        /// and exec-attaches to it.
        #[arg(long, value_name = "NAME")]
        session: Option<String>,
    },
    /// Ask the AI agent a question
    Ask {
        query: String,
        /// Output only the agent's response text and exit, with no decorations,
        /// spinner, or interactive prompts. Tool calls are auto-denied. Useful
        /// for scripting and piping.
        #[arg(long)]
        min_output: bool,
    },
    /// Check whether the daemon is running
    Ping,
    /// Show daemon status (uptime, sessions, provider, circuit breaker)
    Status,
    /// Stop the background daemon
    Stop,
    /// Initialise ~/.daemoneye/ and print tmux/systemd configuration
    Setup {
        /// Overwrite ~/.daemoneye/bin/daemoneye with the binary currently running this command.
        /// Use this after building a new release to update the installed copy.
        #[arg(long)]
        overwrite_bin: bool,
        /// Overwrite the built-in knowledge memory files in ~/.daemoneye/memory/knowledge/
        /// with the versions bundled in this binary.  User-created memories are not affected.
        #[arg(long)]
        overwrite_memory: bool,
        /// Overwrite all seeded files: binary, knowledge memories, and the built-in SRE prompt.
        /// Equivalent to passing both --overwrite-bin and --overwrite-memory, and additionally
        /// refreshes etc/prompts/sre.toml.  User configuration (config.toml) is never touched.
        #[arg(long)]
        overwrite_all: bool,
    },
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
    /// Install a NOPASSWD sudoers rule for a script in ~/.daemoneye/scripts/.
    ///
    /// Grants the current user sudo access to the named script without a password,
    /// enabling ghost shells and scheduled jobs to run it with elevated privileges.
    /// Writes to /etc/sudoers.d/daemoneye-<name> (requires sudo).
    ///
    /// Example: daemoneye install-sudoers check-disk.sh
    InstallSudoers {
        /// Name of the script in ~/.daemoneye/scripts/ (e.g. check-disk.sh)
        script_name: String,
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
    /// Notify that a background command finished (carries exit code)
    Complete {
        /// Target pane ID (e.g. %3)
        pane_id: String,
        /// Exit code of the finished command
        exit_code: i32,
        /// Target session name
        session_name: String,
    },
    /// Notify that a pane received focus (pane-focus-in hook, N1)
    Focus {
        /// Pane that received focus (e.g. %3)
        pane_id: String,
        /// Session name
        session_name: String,
    },
    /// Notify that the active window changed (session-window-changed hook, N2)
    WindowChanged {
        /// Session name
        session_name: String,
    },
    /// Notify that a new tmux session was created (after-new-session hook, N14)
    SessionCreated {
        /// Name of the newly created session
        session_name: String,
    },
    /// Notify that a tmux session was destroyed (session-closed hook, A6)
    SessionClosed {
        /// Name of the closed session
        session_name: String,
    },
    /// Notify that a tmux client attached to a session (client-attached hook, N15)
    ClientAttached {
        /// Session name
        session_name: String,
    },
    /// Notify that a tmux client detached from a session (client-detached hook, N15)
    ClientDetached {
        /// Session name
        session_name: String,
    },
    /// Notify that the terminal was resized (client-resized hook, N8)
    Resize {
        /// New terminal width in columns
        width: u16,
        /// New terminal height in rows
        height: u16,
        /// Session name
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
    config::Config::ensure_dirs()
        .map_err(|e| anyhow::anyhow!("Failed to initialise config directory: {}", e))?;

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
            if libc::setsid() < 0 {
                eprintln!(
                    "daemoneye: setsid() failed: {} — daemon may not be fully detached from terminal",
                    std::io::Error::last_os_error()
                );
            }
            let devnull = libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY);
            if devnull < 0 {
                eprintln!(
                    "daemoneye: warning: failed to open /dev/null: {} — stdin not redirected",
                    std::io::Error::last_os_error()
                );
            } else {
                if libc::dup2(devnull, libc::STDIN_FILENO) < 0 {
                    eprintln!(
                        "daemoneye: warning: failed to redirect stdin from /dev/null: {}",
                        std::io::Error::last_os_error()
                    );
                }
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
            session,
        } => {
            let log_file = if console {
                None
            } else {
                Some(log_file.unwrap_or_else(config::default_log_path))
            };
            daemon::run_daemon(log_file, session).await?;
        }
        Commands::Logs { log_file } => {
            let path = log_file.unwrap_or_else(config::default_log_path);
            cli::run_logs(path)?;
        }
        Commands::Chat { session } => {
            cli::run_chat(session).await?;
        }
        Commands::Ask { query, min_output } => {
            cli::run_ask(query, min_output).await?;
        }
        Commands::Ping => {
            cli::run_ping().await?;
        }
        Commands::Status => {
            cli::run_status().await?;
        }
        Commands::Stop => {
            cli::run_stop().await?;
        }
        Commands::Setup {
            overwrite_bin,
            overwrite_memory,
            overwrite_all,
        } => {
            cli::run_setup(
                overwrite_bin || overwrite_all,
                overwrite_memory || overwrite_all,
                overwrite_all,
            )?;
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
            NotifyCommands::Complete {
                pane_id,
                exit_code,
                session_name,
            } => {
                cli::run_notify_complete(pane_id, exit_code, session_name).await?;
            }
            NotifyCommands::Focus {
                pane_id,
                session_name,
            } => {
                cli::run_notify_focus(pane_id, session_name).await?;
            }
            NotifyCommands::WindowChanged { session_name } => {
                cli::run_notify_window_changed(session_name).await?;
            }
            NotifyCommands::SessionCreated { session_name } => {
                cli::run_notify_session_created(session_name).await?;
            }
            NotifyCommands::SessionClosed { session_name } => {
                cli::run_notify_session_closed(session_name).await?;
            }
            NotifyCommands::ClientAttached { session_name } => {
                cli::run_notify_client_attached(session_name).await?;
            }
            NotifyCommands::ClientDetached { session_name } => {
                cli::run_notify_client_detached(session_name).await?;
            }
            NotifyCommands::Resize {
                width,
                height,
                session_name,
            } => {
                cli::run_notify_resize(width, height, session_name).await?;
            }
        },
        Commands::InstallSudoers { script_name } => {
            scripts::install_sudoers(&script_name)?;
        }
    }

    Ok(())
}

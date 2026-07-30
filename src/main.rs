use anyhow::Context;
use clap::{Parser, Subcommand};
use daemoneye::{agents, ai, cli, config, daemon, scripts, session_store};
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
        raw: bool,
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
    /// Manage named sessions
    Session {
        #[command(subcommand)]
        cmd: SessionCommands,
    },
    /// Manage named agents
    Agent {
        #[command(subcommand)]
        cmd: AgentCommands,
    },
    /// Show AI cost summary from events log
    Costs {
        /// Start date (YYYY-MM-DD, inclusive)
        #[arg(long, value_name = "DATE")]
        since: Option<String>,
        /// End date (YYYY-MM-DD, inclusive)
        #[arg(long, value_name = "DATE")]
        until: Option<String>,
        /// Group results by this dimension
        #[arg(long, value_enum, default_value = "day")]
        by: cli::GroupBy,
        /// Filter to a specific agent name
        #[arg(long, value_name = "NAME")]
        agent: Option<String>,
        /// Output machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Audit installed prompt and knowledge memory files for stale path references.
    ///
    /// Reads the files directly from ~/.daemoneye/ and checks every path literal
    /// against the known inventory. Exits non-zero if any path is superseded or
    /// unknown. Never writes or modifies any file.
    AuditPrompts,
}

#[derive(Subcommand)]
enum SessionCommands {
    /// Import an orphaned ephemeral session log into the named session store.
    ///
    /// Reads `~/.daemoneye/var/log/sessions/<id>.jsonl` and saves it as a
    /// named session so it can be loaded with `/session load <name>`.
    ///
    /// Example: daemoneye session import abc123def456 --name postgres-incident
    Import {
        /// Ephemeral session ID (the hex string visible in var/log/sessions/)
        id: String,
        /// Name to give the imported session
        #[arg(long, short = 'n')]
        name: String,
        /// Optional description
        #[arg(long, short = 'd')]
        desc: Option<String>,
        /// Overwrite an existing saved session with the same name
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum AgentCommands {
    /// List all named agents
    List,
    /// Show full config for a named agent
    Show { name: String },
    /// Create a new agent (opens $EDITOR with a starter config)
    Create { name: String },
    /// Delete a named agent
    Delete { name: String },
    /// Show or clear an agent's briefing
    Briefing {
        name: String,
        /// Clear the briefing file
        #[arg(long)]
        clear: bool,
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
        let (read_end, write_end) =
            daemon::ready::create_pipe().context("failed to create the daemon readiness pipe")?;

        // SAFETY: This runs before the tokio runtime starts, so only the main
        // thread exists. Forking a live multi-threaded runtime is unsound because
        // only the calling thread survives in the child.
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            anyhow::bail!("fork() failed: {}", std::io::Error::last_os_error());
        }
        if pid > 0 {
            // Parent: drop our copy of the write end so the child's is the only
            // one left — otherwise the read below never sees EOF.
            drop(write_end);
            return match daemon::ready::await_child_report(read_end) {
                daemon::ready::ChildReport::Ready => {
                    println!("daemoneye daemon started (PID {})", pid);
                    Ok(())
                }
                daemon::ready::ChildReport::Failed(msg) => {
                    eprintln!("daemoneye: daemon failed to start: {msg}");
                    std::process::exit(1);
                }
                daemon::ready::ChildReport::Died => {
                    eprintln!(
                        "daemoneye: daemon exited during startup without reporting — \
                         see ~/.daemoneye/var/log/daemon.log"
                    );
                    std::process::exit(1);
                }
            };
        }
        // Child: drop the read end, keep the write end as our reporter.
        drop(read_end);
        daemon::ready::set_reporter(write_end);

        // SAFETY: setsid/dup2 operate on raw file descriptors that we control.
        unsafe {
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
            if let Err(e) = daemon::run_daemon(log_file, session).await {
                daemon::ready::report_failure(&e.to_string());
                return Err(e);
            }
        }
        Commands::Logs { log_file } => {
            let path = log_file.unwrap_or_else(config::default_log_path);
            cli::run_logs(path)?;
        }
        Commands::Chat { session } => {
            cli::run_chat(session).await?;
        }
        Commands::Ask { query, raw } => {
            cli::run_ask(query, raw).await?;
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
        Commands::Session { cmd } => match cmd {
            SessionCommands::Import {
                id,
                name,
                desc,
                force,
            } => {
                run_session_import(&id, &name, desc.as_deref(), force)?;
            }
        },
        Commands::Agent { cmd } => match cmd {
            AgentCommands::List => {
                run_agent_list()?;
            }
            AgentCommands::Show { name } => {
                run_agent_show(&name)?;
            }
            AgentCommands::Create { name } => {
                run_agent_create(&name)?;
            }
            AgentCommands::Delete { name } => {
                run_agent_delete(&name)?;
            }
            AgentCommands::Briefing { name, clear } => {
                run_agent_briefing(&name, clear)?;
            }
        },
        Commands::Costs {
            since,
            until,
            by,
            agent,
            json,
        } => {
            cli::run_costs(since, until, by, agent, json)?;
        }
        Commands::AuditPrompts => {
            cli::run_audit_prompts();
        }
    }

    Ok(())
}

fn run_session_import(id: &str, name: &str, desc: Option<&str>, force: bool) -> anyhow::Result<()> {
    let path = daemon::session::session_file(id);
    if !path.exists() {
        anyhow::bail!(
            "session log not found: {}\n  \
             Check available sessions with: ls ~/.daemoneye/var/log/sessions/",
            path.display()
        );
    }
    let text = std::fs::read_to_string(&path)?;
    let messages: Vec<ai::Message> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let turn_count = messages.iter().filter(|m| m.role == "user").count();
    session_store::save_session(session_store::SaveSessionArgs {
        name,
        current_saved_name: None,
        description: desc.unwrap_or(""),
        messages: &messages,
        turn_count,
        model: "default",
        artifacts: &[],
        force,
    })?;
    println!(
        "Imported {} message(s) ({} turns) from session '{}' → saved as '{}'",
        messages.len(),
        turn_count,
        id,
        name
    );
    Ok(())
}

fn run_agent_list() -> anyhow::Result<()> {
    let agents = agents::list_agents()?;
    if agents.is_empty() {
        println!("No agents defined. Use `daemoneye agent create <name>` to create one.");
        return Ok(());
    }
    let name_w = agents
        .iter()
        .map(|a| a.name.len())
        .max()
        .unwrap_or(4)
        .max(4);
    let model_w = agents
        .iter()
        .map(|a| a.model.as_deref().unwrap_or("(default)").len())
        .max()
        .unwrap_or(8)
        .max(8);
    println!(
        "  \x1b[2m{:<name_w$}  {:<model_w$}  description\x1b[0m",
        "name",
        "model",
        name_w = name_w,
        model_w = model_w
    );
    for a in &agents {
        let model = a.model.as_deref().unwrap_or("(default)");
        println!(
            "  \x1b[96m{:<name_w$}\x1b[0m  {:<model_w$}  \x1b[2m{}\x1b[0m",
            a.name,
            model,
            a.description,
            name_w = name_w,
            model_w = model_w
        );
    }
    Ok(())
}

fn run_agent_show(name: &str) -> anyhow::Result<()> {
    let cfg = agents::load_agent(name)?;
    println!("\x1b[1mAgent: {}\x1b[0m", cfg.name);
    println!("  description:          {}", cfg.description);
    println!(
        "  model:                {}",
        cfg.model.as_deref().unwrap_or("(default)")
    );
    println!("  memory_namespace:     {}", cfg.memory_namespace);
    println!(
        "  max_turns:            {}",
        cfg.max_turns
            .map_or("(default)".to_string(), |v| v.to_string())
    );
    println!("  auto_approve_read_only: {}", cfg.auto_approve_read_only);
    if !cfg.auto_approve_scripts.is_empty() {
        println!("  auto_approve_scripts:");
        for s in &cfg.auto_approve_scripts {
            println!("    - {}", s);
        }
    }
    if !cfg.prompt.is_empty() {
        println!("\n  prompt:");
        for line in cfg.prompt.lines() {
            println!("    {}", line);
        }
    }
    Ok(())
}

fn run_agent_create(name: &str) -> anyhow::Result<()> {
    agents::validate_agent_name(name)?;
    let dir = agents::agent_dir(name);
    if dir.exists() {
        anyhow::bail!(
            "Agent '{}' already exists. Use `daemoneye agent show {}` to view it.",
            name,
            name
        );
    }
    let starter = format!(
        r#"# Agent config: {name}
# Edit this file and save to apply changes.

name = "{name}"
description = ""
prompt = ""
# model = "haiku"
# memory_namespace = "{name}"
# max_turns = 10
# auto_approve_read_only = false
# auto_approve_scripts = []
"#
    );
    agents::ensure_agents_dir()?;
    std::fs::create_dir_all(&dir)?;
    let path = agents::config_path(name);
    std::fs::write(&path, &starter)?;
    println!("Created starter config for agent '{}'.", name);
    println!("Edit the file at: {}", path.display());

    // Try to open $EDITOR
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let status = std::process::Command::new(&editor).arg(&path).status();
    match status {
        Ok(s) if s.success() => {
            // Validate the edited config
            match agents::load_agent(name) {
                Ok(_) => println!("Agent '{}' saved successfully.", name),
                Err(e) => eprintln!("Warning: config may be invalid: {}", e),
            }
        }
        Ok(s) => eprintln!("Editor exited with status: {}", s),
        Err(e) => eprintln!("Failed to launch editor ({}): {}", editor, e),
    }
    Ok(())
}

fn run_agent_delete(name: &str) -> anyhow::Result<()> {
    let dir = agents::agent_dir(name);
    if !dir.exists() {
        anyhow::bail!("Agent '{}' does not exist.", name);
    }
    print!("Delete agent '{}'? [y/N] ", name);
    use std::io::Write;
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    if input.trim().to_lowercase() == "y" {
        agents::delete_agent(name)?;
        println!("Agent '{}' deleted.", name);
    } else {
        println!("Cancelled.");
    }
    Ok(())
}

fn run_agent_briefing(name: &str, clear: bool) -> anyhow::Result<()> {
    if clear {
        if daemon::briefing::read_briefing(name).is_some() {
            daemon::briefing::clear_briefing(name);
            println!("Briefing for agent '{}' cleared.", name);
        } else {
            println!("No briefing found for agent '{}'.", name);
        }
        return Ok(());
    }
    match daemon::briefing::read_briefing(name) {
        Some(content) => {
            println!("\x1b[1mBriefing for {}\x1b[0m\n", name);
            println!("{}", content);
        }
        None => {
            println!("No briefing found for agent '{}'.", name);
        }
    }
    Ok(())
}

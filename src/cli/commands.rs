use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

use crate::config::Config;
use crate::cli::render::*;
use crate::cli::input::*;
use crate::daemon::utils::command_has_sudo;
use crate::ipc::{Request, Response, DEFAULT_SOCKET_PATH};


/// Per-session auto-approval flags for the two command classes.
/// Once set, the corresponding class is approved without prompting
/// for the rest of the chat session.
#[derive(Default, Clone)]
struct SessionApproval {
    regular: bool, // auto-approve non-sudo commands
    sudo: bool,    // auto-approve sudo commands
}

impl SessionApproval {
    /// Build the status-bar hint string.
    fn hint(&self) -> String {
        match (self.regular, self.sudo) {
            (false, false) => "auto-approve: off".to_string(),
            (true, false)  => "⚡ auto-approve: regular  ·  Ctrl+C to stop".to_string(),
            (false, true)  => "⚡ auto-approve: sudo  ·  Ctrl+C to stop".to_string(),
            (true, true)   => "⚡ auto-approve: all  ·  Ctrl+C to stop".to_string(),
        }
    }
}

pub fn run_setup() -> Result<()> {
    // Write the systemd user service file.
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let systemd_dir = PathBuf::from(&home).join(".config/systemd/user");
    let service_path = systemd_dir.join("daemoneye.service");

    let service_content = "\
[Unit]
Description=DaemonEye Tmux Daemon
After=network.target

[Service]
Type=simple
ExecStart=%h/.cargo/bin/daemoneye daemon
ExecStop=%h/.cargo/bin/daemoneye stop
Restart=on-failure
RestartSec=5
Environment=\"PATH=%h/.cargo/bin:/usr/local/bin:/usr/bin:/bin\"

[Install]
WantedBy=default.target
";

    match std::fs::create_dir_all(&systemd_dir)
        .and_then(|_| std::fs::write(&service_path, service_content))
    {
        Ok(()) => {
            println!("Wrote {}", service_path.display());
            println!();
            println!("# Enable and start the daemon:");
            println!("systemctl --user daemon-reload");
            println!("systemctl --user enable --now daemoneye");
            println!();
            println!("# Check status and view logs:");
            println!("systemctl --user status daemoneye");
            println!("daemoneye logs");
        }
        Err(e) => {
            eprintln!("Warning: could not write service file: {}", e);
            eprintln!("You can install it manually:");
            eprintln!("  mkdir -p ~/.config/systemd/user");
            eprintln!("  cp daemoneye.service ~/.config/systemd/user/");
        }
    }

    let position = Config::load()
        .unwrap_or_default()
        .ai
        .position;
    let split_flag = match position.as_str() {
        "right"  => "-h",
        "left"   => "-bh",
        "top"    => "-bv",
        _        => "-v",   // "bottom" or any unrecognised value
    };

    // Use the absolute path to the running binary so the bind-key works even
    // when ~/.cargo/bin is not in the PATH inherited by the tmux session (a
    // common issue when the daemon created the session from a background
    // process or service with a minimal environment).
    let daemon_bin = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "daemoneye".to_string());

    println!();
    println!("# Add this to your ~/.tmux.conf:");
    println!("bind-key T split-window {} '{} chat'", split_flag, daemon_bin);
    println!();
    println!("# Then reload tmux config:");
    println!("tmux source-file ~/.tmux.conf");
    println!();
    println!("# If you already have a bind-key that uses the bare name 'daemoneye',");
    println!("# replace it with the full path above — the tmux session may not");
    println!("# inherit ~/.cargo/bin in its PATH.");

    Ok(())
}

pub fn run_logs(path: PathBuf) -> Result<()> {
    if !path.exists() {
        eprintln!("No log file found at {}.", path.display());
        eprintln!("The daemon writes logs there by default when started with: daemoneye daemon");
        std::process::exit(1);
    }
    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new("tail")
        .args(["-f", path.to_str().unwrap_or("")])
        .exec();
    anyhow::bail!("Failed to exec tail: {}", err)
}

pub async fn run_stop() -> Result<()> {
    match connect().await {
        Err(_) => {
            println!("Daemon is not running.");
            std::process::exit(1);
        }
        Ok(stream) => {
            let (rx, mut tx) = stream.into_split();
            let mut rx = BufReader::new(rx);
            send_request(&mut tx, Request::Shutdown).await?;
            match recv(&mut rx).await {
                Ok(Response::Ok) => println!("Daemon stopped."),
                _ => {
                    println!("Daemon did not respond to shutdown.");
                    std::process::exit(1);
                }
            }
        }
    }
    Ok(())
}

pub async fn run_ping() -> Result<()> {
    match connect().await {
        Err(_) => {
            println!("Daemon is not running.");
            std::process::exit(1);
        }
        Ok(stream) => {
            let (rx, mut tx) = stream.into_split();
            let mut rx = BufReader::new(rx);
            send_request(&mut tx, Request::Ping).await?;
            match recv(&mut rx).await {
                Ok(Response::Ok) => println!("Daemon is running."),
                _ => {
                    println!("Daemon is not responding.");
                    std::process::exit(1);
                }
            }
        }
    }
    Ok(())
}

pub async fn run_ask(query: String) -> Result<()> {
    let stdin = AsyncStdin::new()?;
    let mut approval = SessionApproval::default(); // never persists; single-shot has no session
    
    let old = crate::cli::input::set_raw_mode()?;
    let tmux_session = crate::tmux::current_session_name();
    // For one-shot asks the user's current pane IS the working pane; no split/discovery needed.
    let result = ask_with_session(query.clone(), &query, None, None, &stdin, Some(terminal_width()), &mut approval, old, tmux_session.as_deref(), None, &mut 0, 0).await;
    crate::cli::input::restore_termios(old);
    result
}

/// List all available prompts from ~/.daemoneye/prompts/.
pub fn run_prompts() -> Result<()> {
    use crate::config::{load_named_prompt, prompts_dir};

    let dir = prompts_dir();
    let mut entries: Vec<(String, String)> = Vec::new();

    if dir.is_dir() {
        let mut paths: Vec<_> = std::fs::read_dir(&dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map_or(false, |x| x == "toml"))
            .collect();
        paths.sort_by_key(|e| e.file_name());

        for entry in paths {
            let name = entry.path()
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let def = load_named_prompt(&name);
            let preview: String = def.system.chars().take(60).collect();
            entries.push((name, preview));
        }
    }

    if entries.is_empty() {
        println!("No prompts found in {}", dir.display());
        println!("Create a prompt file: {}/my-prompt.toml", dir.display());
        return Ok(());
    }

    let name_width = entries.iter().map(|(n, _)| n.len()).max().unwrap_or(4).max(4);
    println!("\x1b[1mAvailable prompts\x1b[0m  ({})", dir.display());
    println!();
    for (name, desc) in &entries {
        println!("  \x1b[1m\x1b[96m{:<width$}\x1b[0m  {}", name, desc, width = name_width);
    }
    println!();
    println!("  Use \x1b[1m/prompt <name>\x1b[0m in chat to switch, or set \x1b[1mprompt = \"<name>\"\x1b[0m in config.toml.");
    Ok(())
}

/// List scripts in ~/.daemoneye/scripts/ (read directly, no daemon needed).
pub fn run_scripts() -> Result<()> {
    let scripts = crate::scripts::list_scripts()?;
    if scripts.is_empty() {
        let dir = crate::scripts::scripts_dir();
        println!("No scripts found in {}", dir.display());
        println!("Ask the AI to write a script, or place one there manually.");
        return Ok(());
    }
    let name_w = scripts.iter().map(|s| s.name.len()).max().unwrap_or(4).max(4);
    println!("\x1b[1mScripts\x1b[0m  ({})", crate::scripts::scripts_dir().display());
    println!();
    for s in &scripts {
        println!("  \x1b[1m\x1b[96m{:<width$}\x1b[0m  {} bytes", s.name, s.size, width = name_w);
    }
    println!();
    Ok(())
}

/// List scheduled jobs (reads schedules.json directly, no daemon needed).
pub fn run_sched_list() -> Result<()> {
    let path = crate::config::Config::schedules_path();
    let store = crate::scheduler::ScheduleStore::load_or_create(path)?;
    let jobs = store.list();
    if jobs.is_empty() {
        println!("No scheduled jobs.");
        return Ok(());
    }
    let name_w = jobs.iter().map(|j| j.name.len()).max().unwrap_or(4).max(4);
    println!("\x1b[1mScheduled Jobs\x1b[0m");
    println!();
    println!("  {:<8}  {:<name_w$}  {:<16}  {:<12}  {}",
        "ID", "Name", "Schedule", "Status", "Next Run", name_w = name_w);
    println!("  {}  {}  {}  {}  {}",
        "─".repeat(8), "─".repeat(name_w), "─".repeat(16), "─".repeat(12), "─".repeat(24));
    for job in &jobs {
        let id_short = &job.id[..job.id.len().min(8)];
        let next = job.kind.next_run()
            .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "—".to_string());
        println!("  \x1b[96m{:<8}\x1b[0m  {:<name_w$}  {:<16}  {:<12}  {}",
            id_short, job.name, job.kind.describe(), job.status.describe(), next,
            name_w = name_w);
    }
    println!();
    Ok(())
}

/// Cancel a scheduled job by UUID prefix (reads/writes schedules.json directly).
pub fn run_sched_cancel(id: String) -> Result<()> {
    let path = crate::config::Config::schedules_path();
    let store = crate::scheduler::ScheduleStore::load_or_create(path)?;
    // Support prefix matching
    let jobs = store.list();
    let matched: Vec<&crate::scheduler::ScheduledJob> = jobs.iter()
        .filter(|j| j.id.starts_with(&id))
        .collect();
    match matched.len() {
        0 => {
            eprintln!("No job found with ID starting with '{}'", id);
            std::process::exit(1);
        }
        1 => {
            let full_id = matched[0].id.clone();
            store.cancel(&full_id)?;
            println!("Cancelled job {} ({})", full_id, matched[0].name);
        }
        _ => {
            eprintln!("Ambiguous ID prefix '{}' — matches {} jobs. Use more characters.", id, matched.len());
            std::process::exit(1);
        }
    }
    Ok(())
}

/// List leftover de-* tmux windows (from failed scheduled jobs).
pub fn run_sched_windows() -> Result<()> {
    // Use tmux list-windows to find de-* windows
    let output = std::process::Command::new("tmux")
        .args(["list-windows", "-a", "-F", "#{session_name}:#{window_name}"])
        .output();
    match output {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            let de_windows: Vec<&str> = text.lines()
                .filter(|l| {
                    let name = l.splitn(2, ':').nth(1).unwrap_or("");
                    name.starts_with(crate::daemon::DAEMON_WINDOW_PREFIX)
                })
                .collect();
            if de_windows.is_empty() {
                println!("No leftover de-* tmux windows found.");
            } else {
                println!("\x1b[1mLeftover scheduled job windows:\x1b[0m");
                println!();
                for w in &de_windows {
                    println!("  \x1b[96m{}\x1b[0m", w);
                }
                println!();
                println!("Kill a window:  tmux kill-window -t <session>:<window>");
            }
        }
        Err(e) => {
            eprintln!("Failed to list tmux windows: {}", e);
        }
    }
    Ok(())
}

pub async fn run_notify_activity(
    pane_id: String,
    hook_index: usize,
    session_name: String,
) -> Result<()> {
    match connect().await {
        Err(_) => Ok(()), // Silently abort if daemon is not running (e.g. hook fires but daemon was killed)
        Ok(stream) => {
            let (rx, mut tx) = stream.into_split();
            let mut rx = BufReader::new(rx);
            send_request(
                &mut tx,
                crate::ipc::Request::NotifyActivity {
                    pane_id,
                    hook_index,
                    session_name,
                },
            )
            .await?;
            let _ = recv(&mut rx).await; // Consume Response::Ok
            Ok(())
        }
    }
}

pub async fn run_notify_complete(
    pane_id: String,
    exit_code: i32,
    session_name: String,
) -> Result<()> {
    match connect().await {
        Err(_) => Ok(()), // Silently abort if daemon is not running
        Ok(stream) => {
            let (rx, mut tx) = stream.into_split();
            let mut rx = BufReader::new(rx);
            send_request(
                &mut tx,
                crate::ipc::Request::NotifyComplete {
                    pane_id,
                    exit_code,
                    session_name,
                },
            )
            .await?;
            let _ = recv(&mut rx).await; // Consume Response::Ok
            Ok(())
        }
    }
}

pub async fn run_chat() -> Result<()> {
    let result = run_chat_inner().await;
    if let Err(ref e) = result {
        // AsyncStdin has been dropped by now; synchronous stdin is safe.
        use std::io::Write;
        eprintln!("\n\x1b[31m✗\x1b[0m daemoneye error: {}", e);
        eprint!("\x1b[2mPress Enter to close this pane…\x1b[0m");
        std::io::stderr().flush().ok();
        let _ = std::io::stdin().read_line(&mut String::new());
    }
    result
}

async fn run_chat_inner() -> Result<()> {
    let start_time = std::time::Instant::now();
    let session_id = new_session_id();
    // None = use daemon's configured default prompt; Some(name) = override.
    let current_prompt: Option<String> = None;
    let stdin = crate::cli::input::AsyncStdin::new()?;
    let mut input_state = InputState::new();
    let mut approval = SessionApproval::default();
    // Register the SIGWINCH listener before doing anything that depends on
    // terminal size.  tokio queues signals from the moment the listener is
    // created, so no resize event can slip through the gap between process
    // start and our first poll.
    let mut sigwinch = {
        use tokio::signal::unix::{SignalKind, signal};
        signal(SignalKind::window_change())?
    };

    // Resolve the tmux session name and target pane for foreground commands
    // before any terminal setup.  This may prompt the user once if the window
    // has no sibling pane and they opt to split or pick from another window.
    let tmux_session = crate::tmux::current_session_name();
    let pane_id_opt = std::env::var("TMUX_PANE").ok();
    let target_pane: Option<String> = match (&pane_id_opt, &tmux_session) {
        (Some(my_pane), Some(session)) => resolve_target_pane(my_pane, session),
        _ => None,
    };
    let mut chat_width: usize;
    let mut chat_height: usize;
    if let Some(ref pane_id) = pane_id_opt {
        let target_w = crate::tmux::query_window_width(pane_id)
            .map(|w| (w * 25 / 100).max(20))
            .unwrap_or(100);
        let current_w = crate::tmux::query_pane_width(pane_id).unwrap_or(0);
        if current_w < target_w {
            let _ = crate::tmux::resize_pane_width(pane_id, target_w);
            chat_width = crate::tmux::query_pane_width(pane_id).unwrap_or(target_w);
        } else {
            chat_width = current_w;
        }
        chat_height = crate::tmux::query_pane_height(pane_id).unwrap_or_else(|_| terminal_height());
    } else {
        chat_width  = terminal_width();
        chat_height = terminal_height();
    }

    // When running inside tmux a new split pane triggers one or more SIGWINCH
    // signals as the layout is negotiated.  Wait here until no SIGWINCH has
    // arrived for SETTLE_MS milliseconds so we know the final dimensions before
    // printing anything.  Re-query on every signal so we always end up with
    // the correct settled size.
    if pane_id_opt.is_some() {
        const SETTLE_MS: u64 = 500;
        loop {
            match tokio::time::timeout(
                std::time::Duration::from_millis(SETTLE_MS),
                sigwinch.recv(),
            ).await {
                Ok(_) => {
                    // Another resize — update dims and restart the quiet timer.
                    chat_width  = terminal_width();
                    chat_height = terminal_height();
                }
                Err(_elapsed) => break, // stable for SETTLE_MS — proceed
            }
        }
    }

    // Install the scroll region.  The input frame and status bar are
    // intentionally NOT drawn yet — the greeting streams next and the
    // dimensions may still shift.  Drawing the frame now would show it in
    // the wrong place or have it visually overwritten by the greeting content.
    setup_scroll_region(chat_height);

    // ASCII logo — centered using the settled chat_width.
    {
        let logo_lines = [
            "                        ▄      ▄",
            "                       ██▄    ▄██",
            "                      █████▄▄█████",
            "                   ▄████████████████▄",
            "                  ████████████████████",
            "                 ████████  ▀▀  ████████",
            "                ██████▀   ▄██▄   ▀██████",
            "                █████    ███ ██    █████",
            "                █████    ▀████▀    █████",
            "                ██████▄   ▀██▀   ▄██████",
            "                 ████████▄▄  ▄▄████████",
            "                  ████████████████████",
            "                   ▀████▀▀████▀▀████▀",
            "                   ▄▀  █  █  █  █  ▀▄",
            "                  █    █  █  █  █    █",
            "                 ▄▀   ▄▀  █  █  ▀▄   ▀▄",
            "                 █   █    █  █    █   █",
            "",
            "████▄   ▄▄▄  ▄▄▄▄▄ ▄▄   ▄▄  ▄▄▄  ▄▄  ▄▄ ██████ ▄▄ ▄▄ ▄▄▄▄▄",
            "██  ██ ██▀██ ██▄▄  ██▀▄▀██ ██▀██ ███▄██ ██▄▄   ▀███▀ ██▄▄",
            "████▀  ██▀██ ██▄▄▄ ██   ██ ▀███▀ ██ ▀██ ██▄▄▄▄   █   ██▄▄▄",
        ];
        let subtitle = "                 AI POWERED OPERATOR";
        let logo_w = logo_lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
        let pad = " ".repeat((chat_width.saturating_sub(logo_w)) / 2);
        println!();
        let blood_red   = "\x1b[1m\x1b[38;2;180;0;0m";
        let deep_yellow = "\x1b[38;2;220;160;0m"; // bold inherited from blood_red prefix
        for (i, line) in logo_lines.iter().enumerate() {
            // For eye lines, split the line around the yellow segment and render
            // the outer body in red and the inner pupil/iris in deep yellow.
            let eye = match i {
                6 => "▄██▄",    // line 7 of art — iris
                7 => "███ ██",  // line 8 — pupil
                8 => "▀████▀",  // line 9 — eye interior
                9 => "▀██▀",    // line 10 — pupil highlight
                _ => "",
            };
            let s = if !eye.is_empty() {
                if let Some(p) = line.find(eye) {
                    format!("{blood_red}{}{deep_yellow}{eye}{blood_red}{}\x1b[0m",
                            &line[..p], &line[p + eye.len()..])
                } else {
                    format!("{blood_red}{line}\x1b[0m")
                }
            } else if i >= 18 {
                format!("\x1b[1m\x1b[97m{line}\x1b[0m")
            } else {
                format!("{blood_red}{line}\x1b[0m")
            };
            println!("{pad}{s}");
        }
        println!("{pad}\x1b[2m{subtitle}\x1b[0m");
    }

    // One-time usage hints — stacked vertically, centered in the pane.
    {
        let center = |vis_len: usize| -> String {
            " ".repeat((chat_width.saturating_sub(vis_len)) / 2)
        };
        println!();
        // visible lengths (no ANSI): 22, 23, 26, 30
        println!("{}\x1b[93mexit\x1b[0m or \x1b[93mCtrl-C\x1b[0m to quit",           center(22));
        println!("{}\x1b[96m/clear\x1b[0m to reset session",                           center(23));
        println!("{}\x1b[96m/refresh\x1b[0m to resync context",                        center(26));
        println!("{}\x1b[2mcontext: panes · windows · env\x1b[0m",                    center(30));
        println!();
    }

    // Hold off on the AI greeting until a tmux client is attached to this
    // session.  When the daemon auto-opens the chat pane in a freshly-created
    // (detached) session, nobody is watching yet; firing the greeting
    // immediately would waste an API call and surface a stale response when
    // the user eventually attaches.
    //
    // In the normal keybinding workflow (user already inside an active tmux
    // session), #{session_attached} is already ≥ 1 so the loop exits on the
    // first check with no perceptible delay.
    let config = Config::load().unwrap_or_default();
    let model_pre  = config.ai.model.clone();
    let ctx_pre    = config.ai.context_window();
    let hint = approval.hint();
    draw_status_bar(chat_height, chat_width, &session_id, &hint, &model_pre, 0, ctx_pre, false);

    // Switch to raw mode for the entire chat session so we can trap Ctrl+C.
    let old_termios = crate::cli::input::set_raw_mode()?;

    let result = run_chat_inner_raw(
        &mut input_state, &stdin, &mut sigwinch,
        chat_width, start_time, session_id,
        current_prompt, &mut approval, old_termios,
        tmux_session, target_pane,
    ).await;

    crate::cli::input::restore_termios(old_termios);
    result
}

async fn run_chat_inner_raw(
    input_state:    &mut InputState,
    stdin:          &AsyncStdin,
    sigwinch:       &mut tokio::signal::unix::Signal,
    mut chat_width: usize,
    start_time:     std::time::Instant,
    mut session_id: String,
    mut current_prompt: Option<String>,
    approval:   &mut SessionApproval,
    old_termios:    libc::termios,
    tmux_session:   Option<String>,
    target_pane:    Option<String>,
) -> Result<()> {
    let mut last_ctrl_c: Option<std::time::Instant> = None;
    let mut daemon_up = false;
    // Accumulated prompt token count — carried across turns so the query box
    // shows the context size from the *previous* completed turn.
    let mut prompt_tokens: u32 = 0;
    let config = Config::load().unwrap_or_default();
    let context_window = config.ai.context_window();
    let model = config.ai.model.clone();

    loop {
        let attached = std::process::Command::new("tmux")
            .args(["display-message", "-p", "#{session_attached}"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(1); // treat errors as attached (e.g. running outside tmux)
        if attached > 0 { break; }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    // A client is now attached — send the greeting.
    match ask_with_session("Hello!".to_string(), "", Some(&session_id), current_prompt.as_deref(), &stdin, Some(chat_width), approval, old_termios, tmux_session.as_deref(), target_pane.as_deref(), &mut prompt_tokens, context_window).await {
        Ok(()) => daemon_up = true,
        Err(e) => {
            eprintln!("\x1b[31m✗\x1b[0m Could not reach the daemon: {}", e);
            eprintln!("  Make sure it is running:  \x1b[1mdaemoneye daemon --console\x1b[0m");
            eprintln!("  \x1b[2mWaiting for your input…\x1b[0m");
        }
    }

    // Greeting is done.  Re-query dimensions in case the pane was resized
    // while it streamed, then draw the full chrome for the first time.
    chat_width          = terminal_width();
    let mut chat_height = terminal_height();
    setup_scroll_region(chat_height);
    draw_input_frame(chat_height, chat_width, start_time);
    let hint = approval.hint();
    draw_status_bar(chat_height, chat_width, &session_id, &hint, &model, prompt_tokens, context_window, daemon_up);

    loop {
        // read_input_line handles its own rendering and SIGWINCH internally.
        let hint = approval.hint();
        let line_opt = read_input_line(
            input_state, stdin, sigwinch,
            &mut chat_width, &mut chat_height,
            start_time, &session_id, &hint, &model, prompt_tokens, context_window,
            &mut last_ctrl_c, daemon_up,
        ).await?;

        let Some(line) = line_opt else { break }; // EOF or Ctrl+D on empty line

        // Clear the input row and anchor to the scroll region's bottom so
        // all subsequent output scrolls upward.
        {
            use std::io::Write;
            let input_row     = chat_height.saturating_sub(2).max(1);
            let scroll_bottom = chat_height.saturating_sub(4).max(1);
            print!("\x1b[{input_row};1H\x1b[2K");
            print!("\x1b[{scroll_bottom};1H");
            std::io::stdout().flush()?;
        }

        let query = line.trim().to_string();
        if query.is_empty() { continue; }

        // Push to history before processing so /clear etc. are also navigable.
        input_state.push_history(query.clone());

        if query == "exit" || query == "quit" { break; }
        if query == "/clear" {
            session_id = new_session_id();
            *approval = SessionApproval::default();
            current_prompt = None;
            let label = format!(" session cleared · new session:{} ", &session_id[..8]);
            let dashes = chat_width.min(72).saturating_sub(visual_len(&label) + 1);
            println!("\x1b[2m─{}{}\x1b[0m", label, "─".repeat(dashes));
            let hint = approval.hint();
            draw_input_frame(chat_height, chat_width, start_time);
            draw_status_bar(chat_height, chat_width, &session_id, &hint, &model, prompt_tokens, context_window, daemon_up);
            continue;
        }
        if let Some(name) = query.strip_prefix("/prompt ").map(str::trim) {
            let name = name.to_string();
            let path = crate::config::prompts_dir().join(format!("{}.toml", name));
            if !path.exists() && name != "sre" {
                println!("\x1b[31m✗\x1b[0m  Unknown prompt \x1b[1m{}\x1b[0m — run \x1b[1mdaemoneye prompts\x1b[0m to list available prompts.", name);
            } else {
                session_id = new_session_id();
                *approval = SessionApproval::default();
                current_prompt = Some(name.clone());
                let label = format!(" prompt: {}  ·  new session:{} ", name, &session_id[..8]);
                let dashes = chat_width.min(72).saturating_sub(visual_len(&label) + 1);
                println!("\x1b[2m─{}{}\x1b[0m", label, "─".repeat(dashes));
                draw_input_frame(chat_height, chat_width, start_time);
                let hint = approval.hint();
                draw_status_bar(chat_height, chat_width, &session_id, &hint, &model, prompt_tokens, context_window, daemon_up);
            }
            continue;
        }
        if query == "/refresh" {
            match send_refresh().await {
                Ok(()) => {
                    session_id = new_session_id();
                    *approval = SessionApproval::default();
                    let label = format!(" context refreshed  ·  new session:{} ", &session_id[..8]);
                    let dashes = chat_width.min(72).saturating_sub(visual_len(&label) + 1);
                    println!("\x1b[2m─{}{}\x1b[0m", label, "─".repeat(dashes));
                    draw_input_frame(chat_height, chat_width, start_time);
                    let hint = approval.hint();
                    draw_status_bar(chat_height, chat_width, &session_id, &hint, &model, prompt_tokens, context_window, daemon_up);
                }
                Err(e) => println!("\x1b[31m✗\x1b[0m  Refresh failed: {}", e),
            }
            continue;
        }
        match ask_with_session(query.clone(), &query, Some(&session_id), current_prompt.as_deref(), stdin, Some(chat_width), approval, old_termios, tmux_session.as_deref(), target_pane.as_deref(), &mut prompt_tokens, context_window).await {
            Ok(()) => daemon_up = true,
            Err(e) => eprintln!("\n\x1b[31m✗\x1b[0m {}", e),
        }
        // Turn completed: reset the double-tap exit timer.
        last_ctrl_c = None;

        // Re-sync dimensions after the (potentially long) streaming response.
        chat_width  = terminal_width();
        chat_height = terminal_height();
        setup_scroll_region(chat_height);
        draw_input_frame(chat_height, chat_width, start_time);
        let hint = approval.hint();
        draw_status_bar(chat_height, chat_width, &session_id, &hint, &model, prompt_tokens, context_window, daemon_up);
    }

    teardown_scroll_region(chat_height);
    println!("\n\x1b[2mGoodbye.\x1b[0m");
    Ok(())
}


// ── Pane discovery ─────────────────────────────────────────────────────────

/// Determine the target pane for foreground commands.
///
/// Resolution order:
/// 1. Persisted preference from a previous session (validated that it still exists).
/// 2. Exactly one sibling in the same window → use it automatically.
/// 3. Multiple siblings → prompt the user to pick one.
/// 4. No siblings (chat pane fills the whole window) → offer to split or pick
///    from other windows in the session.
fn resolve_target_pane(chat_pane: &str, session: &str) -> Option<String> {
    // 1. Check persisted preference.
    if let Some(saved) = crate::pane_prefs::get(session) {
        if saved != chat_pane && crate::tmux::pane_exists(&saved) {
            return Some(saved);
        }
    }

    // 2 & 3. Siblings in the same tmux window.
    let window_id = crate::tmux::pane_window_id(chat_pane).unwrap_or_default();
    let siblings: Vec<String> = if !window_id.is_empty() {
        crate::tmux::list_panes_in_window(&window_id)
            .unwrap_or_default()
            .into_iter()
            .filter(|p| p != chat_pane)
            .collect()
    } else {
        vec![]
    };

    match siblings.len() {
        0 => {
            // 4. No siblings — offer split or cross-window pick.
            offer_no_sibling_options(chat_pane, session)
        }
        1 => {
            let target = siblings.into_iter().next().unwrap();
            crate::pane_prefs::save(session, &target);
            Some(target)
        }
        _ => pick_sibling_pane(chat_pane, siblings, session),
    }
}

/// When the chat pane is alone in its window, offer three options:
/// split side-by-side (default), pick from another window, or proceed with
/// background-only mode.
/// Read one line from stdin synchronously, temporarily clearing O_NONBLOCK so
/// the call blocks even when AsyncStdin has already set the non-blocking flag.
fn sync_read_line() -> String {
    use std::io::BufRead;
    let fd = libc::STDIN_FILENO;
    // Save and clear O_NONBLOCK so the synchronous read blocks.
    let saved = unsafe { libc::fcntl(fd, libc::F_GETFL, 0) };
    if saved >= 0 {
        unsafe { libc::fcntl(fd, libc::F_SETFL, saved & !libc::O_NONBLOCK) };
    }
    let mut line = String::new();
    let _ = std::io::BufReader::new(std::io::stdin()).read_line(&mut line);
    // Restore original flags (O_NONBLOCK) so AsyncStdin continues to work.
    if saved >= 0 {
        unsafe { libc::fcntl(fd, libc::F_SETFL, saved) };
    }
    line
}

fn offer_no_sibling_options(chat_pane: &str, session: &str) -> Option<String> {
    use std::io::Write;

    let other_panes: Vec<String> = crate::tmux::list_pane_ids_in_session(session)
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p != chat_pane)
        .collect();

    println!();
    println!("No sibling pane in this window for foreground commands.");
    println!("  [S]  Split this window (side by side) and use the new pane  \x1b[2m← default\x1b[0m");
    if !other_panes.is_empty() {
        println!("  [P]  Pick from another pane in this session ({} available)", other_panes.len());
    }
    println!("  [N]  No foreground target (background commands only)");
    let opts = if other_panes.is_empty() { "S/N" } else { "S/P/N" };
    print!("Choose [{}] (Enter = S): ", opts);
    let _ = std::io::stdout().flush();

    let input = sync_read_line();
    let choice = input.trim().to_ascii_lowercase();

    match choice.as_str() {
        "" | "s" => {
            let out = std::process::Command::new("tmux")
                .args(["split-window", "-h", "-t", chat_pane, "-P", "-F", "#{pane_id}"])
                .output()
                .ok()?;
            let new_pane = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if new_pane.is_empty() || !out.status.success() {
                eprintln!("Failed to split window.");
                return None;
            }
            println!("Using pane {} for foreground commands.", new_pane);
            crate::pane_prefs::save(session, &new_pane);
            Some(new_pane)
        }
        "p" if !other_panes.is_empty() => pick_sibling_pane(chat_pane, other_panes, session),
        _ => {
            println!("No foreground target set. Only background commands will run.");
            None
        }
    }
}

/// Present a numbered list of candidate panes and let the user choose one.
fn pick_sibling_pane(_chat_pane: &str, candidates: Vec<String>, session: &str) -> Option<String> {
    use std::io::Write;

    println!();
    println!("Multiple panes available. Which should I use for foreground commands?");
    for (i, pane_id) in candidates.iter().enumerate() {
        let info = std::process::Command::new("tmux")
            .args(["display-message", "-t", pane_id, "-p",
                   "#{pane_current_command}  #{pane_current_path}"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        println!("  [{}]  {}  {}", i + 1, pane_id, info);
    }
    println!("  [N]  No foreground target");
    print!("Choose [1-{}/N]: ", candidates.len());
    let _ = std::io::stdout().flush();

    let input = sync_read_line();
    let input = input.trim().to_ascii_lowercase();

    if input == "n" {
        println!("No foreground target set.");
        return None;
    }
    if let Ok(n) = input.parse::<usize>() {
        if n >= 1 && n <= candidates.len() {
            let chosen = candidates[n - 1].clone();
            crate::pane_prefs::save(session, &chosen);
            return Some(chosen);
        }
    }
    println!("Invalid choice. No foreground target set.");
    None
}

// ── AI conversation ─────────────────────────────────────────────────────────

async fn ask_with_session(
    query: String,
    display_query: &str,
    session_id: Option<&str>,
    prompt_override: Option<&str>,
    stdin: &AsyncStdin,
    chat_width: Option<usize>,
    approval: &mut SessionApproval,
    old_termios: libc::termios,
    tmux_session: Option<&str>,
    target_pane: Option<&str>,
    prompt_tokens: &mut u32,
    context_window: u32,
) -> Result<()> {
    use std::io::Write;
    use std::time::Duration;

    let stream = connect().await?;
    let (rx, mut tx) = stream.into_split();
    let mut rx = BufReader::new(rx);

    // The chat pane is this process's own pane ($TMUX_PANE).  The daemon uses
    // it to switch focus back to the AI interface after a foreground sudo
    // command hands control to the user's target pane.
    let chat_pane = std::env::var("TMUX_PANE").ok();

    // Use the client-resolved target_pane as the source pane for AI context.
    // Falls back to $TMUX_PANE when no target was resolved (e.g. `daemoneye ask`).
    let tmux_pane = target_pane
        .map(|s| s.to_string())
        .or_else(|| std::env::var("TMUX_PANE").ok());

    send_request(&mut tx, Request::Ask {
        query,
        tmux_pane,
        session_id: session_id.map(|s| s.to_string()),
        chat_pane,
        prompt: prompt_override.map(|s| s.to_string()),
        chat_width,
        tmux_session: tmux_session.map(|s| s.to_string()),
        target_pane: target_pane.map(|s| s.to_string()),
    }).await?;

    // Braille-pattern spinner frames, updated every 80 ms while waiting for
    // the first response from the daemon.
    const SPINNER: &[&str] = &[
        "\x1b[31m(\x1b[33m─\x1b[31m)\x1b[0m",
        "\x1b[31m(\x1b[33m○\x1b[31m)\x1b[0m",
        "\x1b[31m(\x1b[33m◎\x1b[31m)\x1b[0m",
        "\x1b[31m(\x1b[33m◉\x1b[31m)\x1b[0m",
        "\x1b[31m(\x1b[33m◎\x1b[31m)\x1b[0m",
        "\x1b[31m(\x1b[33m○\x1b[31m)\x1b[0m",
    ];
    // Verbs rotate every ~30 s (375 ticks × 80 ms = 30 000 ms).
    const VERBS: &[&str] = &[
        "scrying",
        "divining",
        "auguring",
        "communing",
        "pondering",
        "cogitating",
        "attuning",
        "consulting",
        "channeling",
        "deliberating",
    ];
    const TICKS_PER_VERB: usize = 375;
    let mut spin = 0usize;
    let mut response_started = false;
    // prompt_tokens is passed in from the outer loop so the value from the
    // previous turn is visible when print_user_query renders the query box.

    // Markdown renderer — parses inline markdown and block-level elements,
    // applies ANSI styling, and word-wraps prose at the current terminal width.
    // Shared across the whole response (including tool-call sub-turns) so that
    // column position and code-block state remain consistent throughout.
    let display_query = display_query.to_string();
    let mut md = MarkdownRenderer::new();

    loop {
        // Phase 1 — waiting for the first content: poll recv() with a short
        // timeout so we can animate the spinner between each check.
        let msg = if !response_started {
            loop {
                tokio::select! {
                    biased;
                    byte = stdin.read_byte() => {
                        if byte == Some(0x03) { // Ctrl+C during spinner
                            md.flush();
                            println!("\r\x1b[K\n\x1b[33m⚠ Interrupted\x1b[0m  Session approval revoked.");
                            *approval = SessionApproval::default();
                            return Ok(());
                        }
                    }
                    result = tokio::time::timeout(Duration::from_millis(80), recv(&mut rx)) => {
                        match result {
                            Err(_timeout) => {
                                let verb = VERBS[(spin / TICKS_PER_VERB) % VERBS.len()];
                                print!("\r{} \x1b[2m{}…\x1b[0m", SPINNER[spin % SPINNER.len()], verb);
                                std::io::stdout().flush()?;
                                spin = spin.wrapping_add(1);
                            }
                            Ok(r) => break r?,
                        }
                    }
                }
            }
        } else {
            // Phase 2 — streaming: race recv against Ctrl+C with a 60 s per-token deadline.
            let result = tokio::time::timeout(Duration::from_secs(60), async {
                loop {
                    tokio::select! {
                        biased;
                        byte = stdin.read_byte() => {
                            if byte == Some(0x03) { return Ok(None); } // Ctrl+C
                            // any other key while streaming is ignored
                        }
                        msg = recv(&mut rx) => { break msg.map(Some); }
                    }
                }
            }).await;
            match result {
                Ok(Ok(Some(msg))) => msg,
                Ok(Ok(None)) => {
                    md.flush();
                    println!("\n\x1b[33m⚠ Interrupted\x1b[0m  Session approval revoked.");
                    *approval = SessionApproval::default();
                    return Ok(());
                }
                Ok(Err(e)) => return Err(e),
                Err(_) => anyhow::bail!("Daemon stopped responding (60 s inter-token timeout)"),
            }
        };

        match msg {
            Response::Ok => {
                md.flush();
                print!("\x1b[0m"); // reset prose tint
                println!();
                break;
            }
            Response::Error(e) => {
                if !response_started {
                    print!("\r\x1b[K"); // erase spinner line
                }
                md.flush();
                eprintln!("\n\x1b[31m✗\x1b[0m {}", e);
                break;
            }
            Response::SessionInfo { message_count } => {
                // Print the user query as a bordered box with token budget in the bottom border.
                // Skip for the greeting turn (display_query is empty).
                let turn = (message_count / 2) + 1; // each turn = 1 user + 1 assistant msg
                print!("\r\x1b[K"); // erase spinner line
                if !display_query.is_empty() {
                    print_user_query(&display_query, turn, *prompt_tokens, context_window);
                }
            }
            Response::UsageUpdate { prompt_tokens: pt } => {
                *prompt_tokens = pt;
            }
            Response::Token(t) => {
                if !response_started {
                    print!("\r\x1b[K"); // erase spinner line
                    response_started = true;
                }
                md.feed(&t);
                std::io::stdout().flush()?;
            }
            Response::ToolCallPrompt { id, command, background } => {
                if !response_started {
                    print!("\r\x1b[K");
                    response_started = true;
                }
                md.flush();
                println!(); // blank line before panel
                let where_label = if background {
                    "daemon · runs silently"
                } else {
                    "terminal · visible to you"
                };
                let cmd_line = format!("$ {}", command);
                print_tool_panel(where_label, &[&cmd_line], false);

                let is_sudo = command_has_sudo(&command);
                let auto_approved = if is_sudo { approval.sudo } else { approval.regular };

                // Outcome of the approval prompt.
                enum ApprovalDecision {
                    Approved,
                    ApprovedSession,
                    Denied,
                    UserMessage(String),
                }

                let decision = if auto_approved {
                    println!("  \x1b[32m✓\x1b[0m \x1b[2mauto-approved (session)\x1b[0m");
                    ApprovalDecision::Approved
                } else {
                    let session_label = if is_sudo { "sudo session" } else { "session" };
                    print!(
                        "  \x1b[32mApprove?\x1b[0m \
                         [\x1b[1;92mY\x1b[0m]es  \
                         [\x1b[1;91mN\x1b[0m]o  \
                         [\x1b[1;93mA\x1b[0m]pprove for {session_label}  \
                         or type a message to redirect \
                         \x1b[32m›\x1b[0m "
                    );
                    std::io::stdout().flush()?;

                    // Temporarily revert to cooked mode for the tool approval prompt.
                    crate::cli::input::restore_termios(old_termios);
                    let input = stdin.read_line().await.unwrap_or_default();
                    crate::cli::input::set_raw_mode()?; // back to raw mode for turn trap

                    let trimmed = input.trim();
                    if trimmed.eq_ignore_ascii_case("y") {
                        println!("  \x1b[32m✓ approved\x1b[0m");
                        ApprovalDecision::Approved
                    } else if trimmed.eq_ignore_ascii_case("a") {
                        if is_sudo { approval.sudo = true; } else { approval.regular = true; }
                        println!("  \x1b[32m✓ approved — all {} commands auto-approved for this session\x1b[0m",
                                 if is_sudo { "sudo" } else { "regular" });
                        ApprovalDecision::ApprovedSession
                    } else if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("n") {
                        println!("  \x1b[2m✗ skipped\x1b[0m");
                        ApprovalDecision::Denied
                    } else {
                        // Anything else is treated as a corrective message to the AI.
                        println!("  \x1b[33m↩ redirecting agent with your message…\x1b[0m");
                        ApprovalDecision::UserMessage(trimmed.to_string())
                    }
                };

                let (approved, user_message) = match decision {
                    ApprovalDecision::Approved | ApprovalDecision::ApprovedSession => (true, None),
                    ApprovalDecision::Denied => (false, None),
                    ApprovalDecision::UserMessage(msg) => (false, Some(msg)),
                };

                md.reset();
                send_request(&mut tx, Request::ToolCallResponse { id, approved, user_message }).await?;
            }
            Response::SystemMsg(msg) => {
                if !response_started {
                    print!("\r\x1b[K");
                    response_started = true;
                }
                md.flush();
                println!("\x1b[33m⚙\x1b[0m  \x1b[33m{}\x1b[0m", msg);
                md.reset();
            }
            Response::ToolResult(output) => {
                md.flush();
                const MAX_RESULT_LINES: usize = 10;
                let all_lines: Vec<&str> = output.lines().collect();
                let total = all_lines.len();
                // When overflow occurs the indicator itself occupies one row,
                // so only MAX_RESULT_LINES-1 content lines fit within the cap.
                let content_rows = if total > MAX_RESULT_LINES {
                    MAX_RESULT_LINES - 1
                } else {
                    total
                };
                let mut body: Vec<String> = all_lines[..content_rows]
                    .iter().map(|s| s.to_string()).collect();
                if total > MAX_RESULT_LINES {
                    body.push(format!("… {} more lines", total - content_rows));
                }
                if body.is_empty() {
                    body.push("(no output)".to_string());
                }
                let body_refs: Vec<&str> = body.iter().map(|s| s.as_str()).collect();
                print_tool_panel("output", &body_refs, true);
                md.reset();
                // Reset so the spinner re-appears while the AI processes the tool result.
                response_started = false;
            }
            Response::CredentialPrompt { id, prompt } => {
                md.flush();
                println!("\n\x1b[33m⚠\x1b[0m  \x1b[1m{}\x1b[0m", prompt);
                let credential = read_password_silent("   \x1b[33mPassword:\x1b[0m ").unwrap_or_default();
                md.reset();
                send_request(&mut tx, Request::CredentialResponse { id, credential }).await?;
            }
            Response::PaneSelectPrompt { id, panes } => {
                if !response_started {
                    print!("\r\x1b[K");
                    response_started = true;
                }
                md.flush();
                println!();
                println!("  \x1b[33m⚙\x1b[0m \x1b[1mWhich pane should receive this command?\x1b[0m");
                println!();
                for (i, pane) in panes.iter().enumerate() {
                    println!("  \x1b[32m[{}]\x1b[0m  {} — {} — {}",
                        i + 1, pane.id, pane.current_cmd, pane.summary);
                }
                println!();
                print!("  Select pane \x1b[32m›\x1b[0m ");
                std::io::stdout().flush()?;
                // Temporarily revert to cooked mode for user input
                crate::cli::input::restore_termios(old_termios);
                let input = stdin.read_line().await.unwrap_or_default();
                let _ = crate::cli::input::set_raw_mode(); // back to raw mode for turn trap
                let pane_id = input.trim().parse::<usize>()
                    .ok()
                    .and_then(|n| panes.get(n.saturating_sub(1)))
                    .map(|p| p.id.clone())
                    .unwrap_or_else(|| panes.first().map(|p| p.id.clone()).unwrap_or_default());
                md.reset();
                send_request(&mut tx, Request::PaneSelectResponse { id, pane_id }).await?;
            }
            Response::ScriptWritePrompt { id, script_name, content } => {
                if !response_started {
                    print!("\r\x1b[K");
                    response_started = true;
                }
                md.flush();
                println!();
                println!("  \x1b[33m⚙\x1b[0m \x1b[1mAI wants to write script:\x1b[0m \x1b[96m{}\x1b[0m", script_name);
                println!();
                // Show up to 40 lines of the script content
                let lines: Vec<&str> = content.lines().collect();
                let show = lines.len().min(40);
                for line in &lines[..show] {
                    println!("  \x1b[2m{}\x1b[0m", line);
                }
                if lines.len() > 40 {
                    println!("  \x1b[2m… ({} more lines)\x1b[0m", lines.len() - 40);
                }
                println!();
                print!("  Approve writing to ~/.daemoneye/scripts/{}? \x1b[32m[y/N]\x1b[0m \x1b[32m›\x1b[0m ", script_name);
                std::io::stdout().flush()?;
                // Temporarily revert to cooked mode for user input
                crate::cli::input::restore_termios(old_termios);
                let input = stdin.read_line().await.unwrap_or_default();
                let _ = crate::cli::input::set_raw_mode(); // back to raw mode for turn trap
                let approved = matches!(input.trim().to_lowercase().as_str(), "y" | "yes");
                md.reset();
                send_request(&mut tx, Request::ScriptWriteResponse { id, approved }).await?;
            }
            Response::ScheduleWritePrompt { id, name, kind, action } => {
                if !response_started {
                    print!("\r\x1b[K");
                    response_started = true;
                }
                md.flush();
                println!();
                println!("  \x1b[33m⚙\x1b[0m \x1b[1mAI wants to schedule a job:\x1b[0m \x1b[96m{}\x1b[0m", name);
                println!();
                println!("  \x1b[2mSchedule : {}\x1b[0m", kind);
                println!("  \x1b[2mAction   : {}\x1b[0m", action);
                println!();
                print!("  Approve scheduling this job? \x1b[32m[y/N]\x1b[0m \x1b[32m›\x1b[0m ");
                std::io::stdout().flush()?;
                // Temporarily revert to cooked mode for user input
                crate::cli::input::restore_termios(old_termios);
                let input = stdin.read_line().await.unwrap_or_default();
                let _ = crate::cli::input::set_raw_mode(); // back to raw mode for turn trap
                let approved = matches!(input.trim().to_lowercase().as_str(), "y" | "yes");
                md.reset();
                send_request(&mut tx, Request::ScheduleWriteResponse { id, approved }).await?;
            }
            Response::ScheduleList { jobs } => {
                if !response_started {
                    print!("\r\x1b[K");
                    response_started = true;
                }
                md.flush();
                println!();
                if jobs.is_empty() {
                    println!("  No scheduled jobs.");
                } else {
                    println!("  \x1b[1mScheduled Jobs\x1b[0m");
                    println!();
                    let id_w = jobs.iter().map(|j| j.id.len().min(8)).max().unwrap_or(8);
                    let name_w = jobs.iter().map(|j| j.name.len()).max().unwrap_or(4).max(4);
                    let kind_w = jobs.iter().map(|j| j.kind.len()).max().unwrap_or(8).max(8);
                    println!("  {:<id_w$}  {:<name_w$}  {:<kind_w$}  {:<12}  {}",
                        "ID", "Name", "Schedule", "Status", "Next Run",
                        id_w = id_w, name_w = name_w, kind_w = kind_w);
                    println!("  {}  {}  {}  {}  {}",
                        "─".repeat(id_w), "─".repeat(name_w), "─".repeat(kind_w),
                        "─".repeat(12), "─".repeat(24));
                    for job in &jobs {
                        let id_short = &job.id[..job.id.len().min(8)];
                        let next = job.next_run.as_deref().unwrap_or("—");
                        println!("  \x1b[96m{:<id_w$}\x1b[0m  {:<name_w$}  {:<kind_w$}  {:<12}  {}",
                            id_short, job.name, job.kind, job.status, next,
                            id_w = id_w, name_w = name_w, kind_w = kind_w);
                    }
                }
                println!();
                md.reset();
            }
            Response::ScriptList { scripts } => {
                if !response_started {
                    print!("\r\x1b[K");
                    response_started = true;
                }
                md.flush();
                println!();
                if scripts.is_empty() {
                    println!("  No scripts in ~/.daemoneye/scripts/");
                } else {
                    println!("  \x1b[1mScripts\x1b[0m  (~/.daemoneye/scripts/)");
                    println!();
                    let name_w = scripts.iter().map(|s| s.name.len()).max().unwrap_or(4).max(4);
                    for s in &scripts {
                        println!("  \x1b[96m{:<name_w$}\x1b[0m  {} bytes", s.name, s.size, name_w = name_w);
                    }
                }
                println!();
                md.reset();
            }
            Response::RunbookWritePrompt { id, runbook_name, content } => {
                if !response_started {
                    print!("\r\x1b[K");
                    response_started = true;
                }
                md.flush();
                println!();
                println!("  \x1b[33m⚙\x1b[0m \x1b[1mAI wants to write runbook:\x1b[0m \x1b[96m{}\x1b[0m", runbook_name);
                println!();
                let lines: Vec<&str> = content.lines().collect();
                let show = lines.len().min(40);
                for line in &lines[..show] {
                    println!("  \x1b[2m{}\x1b[0m", line);
                }
                if lines.len() > 40 {
                    println!("  \x1b[2m… ({} more lines)\x1b[0m", lines.len() - 40);
                }
                println!();
                print!("  Approve writing to ~/.daemoneye/runbooks/{}.md? \x1b[32m[y/N]\x1b[0m \x1b[32m›\x1b[0m ", runbook_name);
                std::io::stdout().flush()?;
                crate::cli::input::restore_termios(old_termios);
                let input = stdin.read_line().await.unwrap_or_default();
                let _ = crate::cli::input::set_raw_mode();
                let approved = matches!(input.trim().to_lowercase().as_str(), "y" | "yes");
                md.reset();
                send_request(&mut tx, Request::RunbookWriteResponse { id, approved }).await?;
            }
            Response::RunbookDeletePrompt { id, runbook_name, active_jobs } => {
                if !response_started {
                    print!("\r\x1b[K");
                    response_started = true;
                }
                md.flush();
                println!();
                println!("  \x1b[33m⚙\x1b[0m \x1b[1mAI wants to delete runbook:\x1b[0m \x1b[96m{}\x1b[0m", runbook_name);
                if !active_jobs.is_empty() {
                    println!();
                    println!("  \x1b[33mWarning:\x1b[0m the following scheduled jobs reference this runbook:");
                    for job in &active_jobs {
                        println!("    \x1b[2m- {}\x1b[0m", job);
                    }
                }
                println!();
                print!("  Approve deleting ~/.daemoneye/runbooks/{}.md? \x1b[32m[y/N]\x1b[0m \x1b[32m›\x1b[0m ", runbook_name);
                std::io::stdout().flush()?;
                crate::cli::input::restore_termios(old_termios);
                let input = stdin.read_line().await.unwrap_or_default();
                let _ = crate::cli::input::set_raw_mode();
                let approved = matches!(input.trim().to_lowercase().as_str(), "y" | "yes");
                md.reset();
                send_request(&mut tx, Request::RunbookDeleteResponse { id, approved }).await?;
            }
            Response::RunbookList { runbooks } => {
                if !response_started {
                    print!("\r\x1b[K");
                    response_started = true;
                }
                md.flush();
                println!();
                if runbooks.is_empty() {
                    println!("  No runbooks in ~/.daemoneye/runbooks/");
                } else {
                    println!("  \x1b[1mRunbooks\x1b[0m  (~/.daemoneye/runbooks/)");
                    println!();
                    let name_w = runbooks.iter().map(|r| r.name.len()).max().unwrap_or(4).max(4);
                    for r in &runbooks {
                        let tags = if r.tags.is_empty() {
                            String::new()
                        } else {
                            format!("  \x1b[2m[{}]\x1b[0m", r.tags.join(", "))
                        };
                        println!("  \x1b[96m{:<name_w$}\x1b[0m{}", r.name, tags, name_w = name_w);
                    }
                }
                println!();
                md.reset();
            }
            Response::MemoryList { entries } => {
                if !response_started {
                    print!("\r\x1b[K");
                    response_started = true;
                }
                md.flush();
                println!();
                if entries.is_empty() {
                    println!("  No memory entries in ~/.daemoneye/memory/");
                } else {
                    println!("  \x1b[1mMemory Entries\x1b[0m  (~/.daemoneye/memory/)");
                    println!();
                    let cat_w = entries.iter().map(|e| e.category.len()).max().unwrap_or(8).max(8);
                    let key_w = entries.iter().map(|e| e.key.len()).max().unwrap_or(3).max(3);
                    println!("  {:<cat_w$}  {}", "Category", "Key", cat_w = cat_w);
                    println!("  {}  {}", "─".repeat(cat_w), "─".repeat(key_w));
                    for e in &entries {
                        println!("  \x1b[2m{:<cat_w$}\x1b[0m  \x1b[96m{}\x1b[0m", e.category, e.key, cat_w = cat_w);
                    }
                }
                println!();
                md.reset();
            }
        }
    }

    Ok(())
}


/// Generate a random session ID from /dev/urandom.
/// Falls back to timestamp+PID entropy if /dev/urandom is unavailable,
/// avoiding the predictable all-zeros key produced by the old code.
fn new_session_id() -> String {
    let mut bytes = [0u8; 8];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        if f.read_exact(&mut bytes).is_ok() {
            return bytes.iter().map(|b| format!("{:02x}", b)).collect();
        }
    }
    // /dev/urandom unavailable — mix nanosecond timestamp with PID.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    format!("{:08x}{:08x}", nanos ^ pid, pid.wrapping_mul(2_654_435_761))
}

/// Ask the daemon to re-collect system context (OS info, memory, processes, history).
async fn send_refresh() -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let stream = connect().await?;
    let (rx, mut tx) = stream.into_split();
    let mut data = serde_json::to_vec(&crate::ipc::Request::Refresh)?;
    data.push(b'\n');
    tx.write_all(&data).await?;
    let mut rx = tokio::io::BufReader::new(rx);
    let mut line = String::new();
    rx.read_line(&mut line).await?;
    Ok(())
}

pub async fn connect() -> Result<UnixStream> {
    let socket_path = Path::new(DEFAULT_SOCKET_PATH);
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        UnixStream::connect(socket_path),
    )
    .await
    .with_context(|| format!("Timed out connecting to daemon at {} (is it running?)", DEFAULT_SOCKET_PATH))?
    .with_context(|| format!("Failed to connect to daemon at {}", DEFAULT_SOCKET_PATH))
}

pub async fn send_request(tx: &mut OwnedWriteHalf, req: Request) -> Result<()> {
    let mut data = serde_json::to_vec(&req)?;
    data.push(b'\n');
    tx.write_all(&data).await?;
    Ok(())
}

pub async fn recv(rx: &mut BufReader<OwnedReadHalf>) -> Result<Response> {
    let mut line = String::new();
    let n = rx.read_line(&mut line).await?;
    if n == 0 {
        anyhow::bail!("Daemon closed connection unexpectedly.");
    }
    let response: Response = serde_json::from_str(line.trim())?;
    Ok(response)
}

/// Permanently delete a scheduled job by UUID prefix (reads/writes schedules.json directly).
pub fn run_sched_delete(id: String) -> Result<()> {
    let path = crate::config::Config::schedules_path();
    let store = crate::scheduler::ScheduleStore::load_or_create(path)?;
    // Support prefix matching
    let jobs = store.list();
    let matched: Vec<&crate::scheduler::ScheduledJob> =
        jobs.iter().filter(|j| j.id.starts_with(&id)).collect();
    match matched.len() {
        0 => {
            eprintln!("No job found with ID starting with '{}'", id);
            std::process::exit(1);
        }
        1 => {
            let full_id = matched[0].id.clone();
            store.delete(&full_id)?;
            println!("Permanently deleted job {} ({})", full_id, matched[0].name);
        }
        _ => {
            eprintln!(
                "Ambiguous ID prefix '{}' — matches {} jobs. Use more characters.",
                id,
                matched.len()
            );
            std::process::exit(1);
        }
    }
    Ok(())
}

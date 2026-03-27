use anyhow::{Context, Result};
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

use crate::cli::input::*;
use crate::cli::render::*;
use crate::config::{Config, default_socket_path};
use crate::daemon::utils::command_has_sudo;
use crate::ipc::{Request, Response};

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
            (true, false) => "⚡ auto-approve: regular  ·  Ctrl+C to stop".to_string(),
            (false, true) => "⚡ auto-approve: sudo  ·  Ctrl+C to stop".to_string(),
            (true, true) => "⚡ auto-approve: all  ·  Ctrl+C to stop".to_string(),
        }
    }
}

/// Run `daemoneye setup`.
///
/// - `overwrite_bin`    — copy the current executable to `~/.daemoneye/bin/daemoneye`
///   even if a copy already exists there.
/// - `overwrite_memory` — overwrite the six built-in knowledge memory files with the
///   versions bundled in this binary.
/// - `overwrite_prompt` — overwrite `~/.daemoneye/etc/prompts/sre.toml` with the
///   version bundled in this binary (implied by `--overwrite-all`).
pub fn run_setup(overwrite_bin: bool, overwrite_memory: bool, overwrite_prompt: bool) -> Result<()> {
    // Ensure the full ~/.daemoneye/ directory tree and default files are in place.
    // (Also called at the top of main(), but being explicit here makes setup self-contained.)
    crate::config::Config::ensure_dirs()
        .map_err(|e| anyhow::anyhow!("Failed to initialise config directory: {}", e))?;

    let dir = crate::config::config_dir();
    println!("Initialised ~/.daemoneye/ layout:");
    println!(
        "  {}/etc/config.toml       ← edit this to configure the daemon",
        dir.display()
    );
    println!(
        "  {}/etc/prompts/           ← system prompt files (.toml)",
        dir.display()
    );
    println!(
        "  {}/var/run/               ← socket, schedules, pane prefs",
        dir.display()
    );
    println!(
        "  {}/var/log/               ← daemon.log and pipe-pane capture logs",
        dir.display()
    );
    println!(
        "  {}/bin/                   ← place symlinks/wrappers here",
        dir.display()
    );
    println!(
        "  {}/lib/                   ← shared SDK modules (de_sdk, Python helpers)",
        dir.display()
    );
    println!(
        "  {}/scripts/               ← automation scripts",
        dir.display()
    );
    println!(
        "  {}/runbooks/              ← procedure runbooks",
        dir.display()
    );
    println!(
        "  {}/memory/                ← persistent AI memory",
        dir.display()
    );
    println!();
    let knowledge_dir = dir.join("memory").join("knowledge");
    let seeded = [
        "webhook-setup",
        "runbook-format",
        "runbook-ghost-template",
        "ghost-shell-guide",
        "scheduling-guide",
        "scripts-and-sudoers",
    ];
    if overwrite_memory {
        println!("Overwriting built-in knowledge memories:");
        match crate::config::overwrite_knowledge_memories() {
            Ok(()) => {
                for key in &seeded {
                    println!("  {}  ✓ (overwritten)", key);
                }
            }
            Err(e) => eprintln!("Warning: could not overwrite knowledge memories: {}", e),
        }
    } else {
        println!("Seeded knowledge memories (written once, preserved on upgrade):");
        for key in &seeded {
            let exists = knowledge_dir.join(format!("{}.md", key)).exists();
            println!("  {}  {}", key, if exists { "✓" } else { "(missing)" });
        }
    }
    println!();

    // Copy the running binary into ~/.daemoneye/bin/daemoneye.
    // On first run (no binary present) always copy; on upgrade require --overwrite-bin.
    let bin_dest = crate::config::bin_dir().join("daemoneye");
    let current_exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("daemoneye"));
    let bin_exists = bin_dest.exists();
    if !bin_exists || overwrite_bin {
        match std::fs::copy(&current_exe, &bin_dest) {
            Ok(_) => {
                if bin_exists {
                    println!("Updated binary → {}", bin_dest.display());
                } else {
                    println!("Copied binary → {}", bin_dest.display());
                }
            }
            Err(e) => eprintln!(
                "Warning: could not copy binary to {}: {}",
                bin_dest.display(),
                e
            ),
        }
    } else {
        println!(
            "Binary already installed at {} (use --overwrite-bin to update)",
            bin_dest.display()
        );
    }
    println!();

    // Overwrite the built-in SRE prompt when --overwrite-all is in effect.
    if overwrite_prompt {
        match crate::config::overwrite_sre_prompt() {
            Ok(()) => println!(
                "Refreshed built-in SRE prompt → {}/etc/prompts/sre.toml",
                dir.display()
            ),
            Err(e) => eprintln!("Warning: could not overwrite SRE prompt: {}", e),
        }
        println!();
    }

    // Write the systemd user service file using the bin/ path.
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let systemd_dir = PathBuf::from(&home).join(".config/systemd/user");
    let service_path = systemd_dir.join("daemoneye.service");

    let service_content = "\
[Unit]
Description=DaemonEye Tmux Daemon
After=network.target

[Service]
Type=simple
ExecStart=%h/.daemoneye/bin/daemoneye daemon
ExecStop=%h/.daemoneye/bin/daemoneye stop
Restart=on-failure
RestartSec=5
Environment=\"PATH=%h/.daemoneye/bin:/usr/local/bin:/usr/bin:/bin\"

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

    let position = Config::load().unwrap_or_default().ai.position;
    let split_flag = match position.as_str() {
        "right" => "-h",
        "left" => "-bh",
        "top" => "-bv",
        _ => "-v", // "bottom" or any unrecognised value
    };

    // Use the ~/.daemoneye/bin/ copy so the bind-key is stable across cargo reinstalls
    // and works even when ~/.cargo/bin is not in the PATH inherited by tmux.
    let daemon_bin = bin_dest.to_string_lossy().into_owned();

    println!();
    println!("# Add this to your ~/.tmux.conf:");
    println!(
        "bind-key T split-window {} '{} chat'",
        split_flag, daemon_bin
    );
    println!();
    println!("# Then reload tmux config:");
    println!("tmux source-file ~/.tmux.conf");
    println!();
    println!("# If you already have a bind-key that uses the bare name 'daemoneye',");
    println!("# replace it with the full path above — the tmux session may not");
    println!("# inherit ~/.cargo/bin in its PATH.");
    println!();
    println!("# To enable accurate exit-code tracking for foreground commands,");
    println!("# add the appropriate snippet to your shell config:");
    println!();
    println!("# bash (~/.bashrc):");
    println!(
        "_de_exit_trap() {{ tmux set-environment \"DE_EXIT_${{TMUX_PANE#%}}\" \"$?\" 2>/dev/null; }}"
    );
    println!("PROMPT_COMMAND=\"_de_exit_trap${{PROMPT_COMMAND:+; $PROMPT_COMMAND}}\"");
    println!();
    println!("# zsh (~/.zshrc):");
    println!(
        "_de_precmd() {{ tmux set-environment \"DE_EXIT_${{TMUX_PANE#%}}\" \"$?\" 2>/dev/null; }}"
    );
    println!("precmd_functions+=(_de_precmd)");

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
    let result = ask_with_session(
        QueryArgs {
            query: query.clone(),
            display_query: &query,
            prompt_override: None,
        },
        None,
        &mut approval,
        AskTmuxCtx {
            session: tmux_session.as_deref(),
            pane: None,
        },
        TokenCtx {
            prompt_tokens: &mut 0,
            context_window: 0,
        },
        StreamCtx {
            stdin: &stdin,
            chat_width: Some(terminal_width()),
            old_termios: old,
            sigwinch: None,
            resize: None,
        },
    )
    .await;
    crate::cli::input::restore_termios(old);
    result
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
        chat_width = terminal_width();
        chat_height = terminal_height();
    }

    // When running inside tmux a new split pane triggers one or more SIGWINCH
    // signals as the layout is negotiated.  Wait here until no SIGWINCH has
    // arrived for SETTLE_MS milliseconds so we know the final dimensions before
    // printing anything.  Re-query on every signal so we always end up with
    // the correct settled size.
    if pane_id_opt.is_some() {
        const SETTLE_MS: u64 = 500;
        while tokio::time::timeout(std::time::Duration::from_millis(SETTLE_MS), sigwinch.recv())
            .await
            .is_ok()
        {
            // Another resize — update dims and restart the quiet timer.
            chat_width = terminal_width();
            chat_height = terminal_height();
        } // stable for SETTLE_MS — proceed
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
        let logo_w = logo_lines
            .iter()
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(0);
        let pad = " ".repeat((chat_width.saturating_sub(logo_w)) / 2);
        println!();
        let blood_red = "\x1b[1m\x1b[38;2;180;0;0m";
        let deep_yellow = "\x1b[38;2;220;160;0m"; // bold inherited from blood_red prefix
        for (i, line) in logo_lines.iter().enumerate() {
            // For eye lines, split the line around the yellow segment and render
            // the outer body in red and the inner pupil/iris in deep yellow.
            let eye = match i {
                6 => "▄██▄",   // line 7 of art — iris
                7 => "███ ██", // line 8 — pupil
                8 => "▀████▀", // line 9 — eye interior
                9 => "▀██▀",   // line 10 — pupil highlight
                _ => "",
            };
            let s = if !eye.is_empty() {
                if let Some(p) = line.find(eye) {
                    format!(
                        "{blood_red}{}{deep_yellow}{eye}{blood_red}{}\x1b[0m",
                        &line[..p],
                        &line[p + eye.len()..]
                    )
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
        let center =
            |vis_len: usize| -> String { " ".repeat((chat_width.saturating_sub(vis_len)) / 2) };
        println!();
        // visible lengths (no ANSI): 22, 23, 26, 30
        println!(
            "{}\x1b[93mexit\x1b[0m or \x1b[93mCtrl-C\x1b[0m to quit",
            center(22)
        );
        println!("{}\x1b[96m/clear\x1b[0m to reset session", center(23));
        println!("{}\x1b[96m/refresh\x1b[0m to resync context", center(26));
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
    let model_pre = config.ai.model.clone();
    let ctx_pre = config.ai.context_window();
    let hint = approval.hint();
    draw_status_bar(
        chat_height,
        chat_width,
        &StatusBarState {
            session_id: &session_id,
            approval_hint: &hint,
            model: &model_pre,
            prompt_tokens: 0,
            context_window: ctx_pre,
            daemon_up: false,
        },
    );

    // Switch to raw mode for the entire chat session so we can trap Ctrl+C.
    let old_termios = crate::cli::input::set_raw_mode()?;

    let result = run_chat_inner_raw(
        InputHandles {
            state: &mut input_state,
            stdin: &stdin,
            sigwinch: &mut sigwinch,
        },
        TerminalCtx {
            chat_width,
            start_time,
            old_termios,
        },
        session_id,
        current_prompt,
        &mut approval,
        TmuxCtx {
            session: tmux_session,
            pane: target_pane,
        },
    )
    .await;

    crate::cli::input::restore_termios(old_termios);
    result
}

struct InputHandles<'a> {
    state: &'a mut InputState,
    stdin: &'a AsyncStdin,
    sigwinch: &'a mut tokio::signal::unix::Signal,
}

struct TerminalCtx {
    chat_width: usize,
    start_time: std::time::Instant,
    old_termios: libc::termios,
}

struct TmuxCtx {
    session: Option<String>,
    pane: Option<String>,
}

async fn run_chat_inner_raw(
    handles: InputHandles<'_>,
    term: TerminalCtx,
    mut session_id: String,
    mut current_prompt: Option<String>,
    approval: &mut SessionApproval,
    tmux: TmuxCtx,
) -> Result<()> {
    let InputHandles {
        state: input_state,
        stdin,
        sigwinch,
    } = handles;
    let TerminalCtx {
        chat_width,
        start_time,
        old_termios,
    } = term;
    let TmuxCtx {
        session: tmux_session,
        pane: target_pane,
    } = tmux;
    let mut chat_width = chat_width;
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
        if attached > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    // A client is now attached — send the greeting.
    // chat_height is declared here so it can be passed to the resize context.
    let mut chat_height = terminal_height();
    {
        let cw = chat_width; // copy for Request::Ask; &mut chat_width goes into resize
        let resize = StreamResizeDims {
            width: &mut chat_width,
            height: &mut chat_height,
            start: start_time,
            model: model.clone(),
            daemon_up: false,
            has_frame: false,
        };
        match ask_with_session(
            QueryArgs {
                query: "Hello!".to_string(),
                display_query: "",
                prompt_override: current_prompt.as_deref(),
            },
            Some(&session_id),
            approval,
            AskTmuxCtx {
                session: tmux_session.as_deref(),
                pane: target_pane.as_deref(),
            },
            TokenCtx {
                prompt_tokens: &mut prompt_tokens,
                context_window,
            },
            StreamCtx {
                stdin,
                chat_width: Some(cw),
                old_termios,
                sigwinch: Some(sigwinch),
                resize: Some(resize),
            },
        )
        .await
        {
            Ok(()) => daemon_up = true,
            Err(e) => {
                eprintln!("\x1b[31m✗\x1b[0m Could not reach the daemon: {}", e);
                eprintln!("  Make sure it is running:  \x1b[1mdaemoneye daemon --console\x1b[0m");
                eprintln!("  \x1b[2mWaiting for your input…\x1b[0m");
            }
        }
    }

    // Greeting is done.  Re-query dimensions in case the pane was resized
    // while it streamed, then draw the full chrome for the first time.
    chat_width = terminal_width();
    chat_height = terminal_height();
    setup_scroll_region(chat_height);
    draw_input_frame(chat_height, chat_width, start_time);
    let hint = approval.hint();
    draw_status_bar(
        chat_height,
        chat_width,
        &StatusBarState {
            session_id: &session_id,
            approval_hint: &hint,
            model: &model,
            prompt_tokens,
            context_window,
            daemon_up,
        },
    );

    loop {
        // read_input_line handles its own rendering and SIGWINCH internally.
        let hint = approval.hint();
        let line_opt = read_input_line(
            input_state,
            stdin,
            sigwinch,
            &mut chat_width,
            &mut chat_height,
            start_time,
            &StatusBarState {
                session_id: &session_id,
                approval_hint: &hint,
                model: &model,
                prompt_tokens,
                context_window,
                daemon_up,
            },
            &mut last_ctrl_c,
        )
        .await?;

        let Some(line) = line_opt else { break }; // EOF or Ctrl+D on empty line

        // Clear the input row and anchor to the scroll region's bottom so
        // all subsequent output scrolls upward.
        {
            use std::io::Write;
            let input_row = chat_height.saturating_sub(2).max(1);
            let scroll_bottom = chat_height.saturating_sub(4).max(1);
            print!("\x1b[{input_row};1H\x1b[2K");
            print!("\x1b[{scroll_bottom};1H");
            std::io::stdout().flush()?;
        }

        let query = line.trim().to_string();
        if query.is_empty() {
            continue;
        }

        // Push to history before processing so /clear etc. are also navigable.
        input_state.push_history(query.clone());

        if query == "exit" || query == "quit" {
            break;
        }
        if query == "/clear" {
            session_id = new_session_id();
            *approval = SessionApproval::default();
            current_prompt = None;
            let label = format!(" session cleared · new session:{} ", &session_id[..8]);
            let dashes = chat_width.min(72).saturating_sub(visual_len(&label) + 1);
            println!("\x1b[2m─{}{}\x1b[0m", label, "─".repeat(dashes));
            let hint = approval.hint();
            draw_input_frame(chat_height, chat_width, start_time);
            draw_status_bar(
                chat_height,
                chat_width,
                &StatusBarState {
                    session_id: &session_id,
                    approval_hint: &hint,
                    model: &model,
                    prompt_tokens,
                    context_window,
                    daemon_up,
                },
            );
            continue;
        }
        if let Some(name) = query.strip_prefix("/prompt ").map(str::trim) {
            let name = name.to_string();
            let path = crate::config::prompts_dir().join(format!("{}.toml", name));
            if !path.exists() && name != "sre" {
                println!(
                    "\x1b[31m✗\x1b[0m  Unknown prompt \x1b[1m{}\x1b[0m — run \x1b[1mdaemoneye prompts\x1b[0m to list available prompts.",
                    name
                );
            } else {
                session_id = new_session_id();
                *approval = SessionApproval::default();
                current_prompt = Some(name.clone());
                let label = format!(" prompt: {}  ·  new session:{} ", name, &session_id[..8]);
                let dashes = chat_width.min(72).saturating_sub(visual_len(&label) + 1);
                println!("\x1b[2m─{}{}\x1b[0m", label, "─".repeat(dashes));
                draw_input_frame(chat_height, chat_width, start_time);
                let hint = approval.hint();
                draw_status_bar(
                    chat_height,
                    chat_width,
                    &StatusBarState {
                        session_id: &session_id,
                        approval_hint: &hint,
                        model: &model,
                        prompt_tokens,
                        context_window,
                        daemon_up,
                    },
                );
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
                    draw_status_bar(
                        chat_height,
                        chat_width,
                        &StatusBarState {
                            session_id: &session_id,
                            approval_hint: &hint,
                            model: &model,
                            prompt_tokens,
                            context_window,
                            daemon_up,
                        },
                    );
                }
                Err(e) => println!("\x1b[31m✗\x1b[0m  Refresh failed: {}", e),
            }
            continue;
        }
        {
            let cw = chat_width; // copy for Request::Ask
            let resize = StreamResizeDims {
                width: &mut chat_width,
                height: &mut chat_height,
                start: start_time,
                model: model.clone(),
                daemon_up,
                has_frame: true,
            };
            match ask_with_session(
                QueryArgs {
                    query: query.clone(),
                    display_query: &query,
                    prompt_override: current_prompt.as_deref(),
                },
                Some(&session_id),
                approval,
                AskTmuxCtx {
                    session: tmux_session.as_deref(),
                    pane: target_pane.as_deref(),
                },
                TokenCtx {
                    prompt_tokens: &mut prompt_tokens,
                    context_window,
                },
                StreamCtx {
                    stdin,
                    chat_width: Some(cw),
                    old_termios,
                    sigwinch: Some(sigwinch),
                    resize: Some(resize),
                },
            )
            .await
            {
                Ok(()) => daemon_up = true,
                Err(e) => eprintln!("\n\x1b[31m✗\x1b[0m {}", e),
            }
        }
        // Turn completed: reset the double-tap exit timer.
        last_ctrl_c = None;

        // Re-sync dimensions after the (potentially long) streaming response.
        chat_width = terminal_width();
        chat_height = terminal_height();
        setup_scroll_region(chat_height);
        draw_input_frame(chat_height, chat_width, start_time);
        let hint = approval.hint();
        draw_status_bar(
            chat_height,
            chat_width,
            &StatusBarState {
                session_id: &session_id,
                approval_hint: &hint,
                model: &model,
                prompt_tokens,
                context_window,
                daemon_up,
            },
        );
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
    if let Some(saved) = crate::pane_prefs::get(session)
        && saved != chat_pane
        && crate::tmux::pane_exists(&saved)
    {
        return Some(saved);
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
    println!(
        "  [S]  Split this window (side by side) and use the new pane  \x1b[2m← default\x1b[0m"
    );
    if !other_panes.is_empty() {
        println!(
            "  [P]  Pick from another pane in this session ({} available)",
            other_panes.len()
        );
    }
    println!("  [N]  No foreground target (background commands only)");
    let opts = if other_panes.is_empty() {
        "S/N"
    } else {
        "S/P/N"
    };
    print!("Choose [{}] (Enter = S): ", opts);
    let _ = std::io::stdout().flush();

    let input = sync_read_line();
    let choice = input.trim().to_ascii_lowercase();

    match choice.as_str() {
        "" | "s" => {
            let out = std::process::Command::new("tmux")
                .args([
                    "split-window",
                    "-h",
                    "-t",
                    chat_pane,
                    "-P",
                    "-F",
                    "#{pane_id}",
                ])
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
            .args([
                "display-message",
                "-t",
                pane_id,
                "-p",
                "#{pane_current_command}  #{pane_current_path}",
            ])
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
    if let Ok(n) = input.parse::<usize>()
        && n >= 1
        && n <= candidates.len()
    {
        let chosen = candidates[n - 1].clone();
        crate::pane_prefs::save(session, &chosen);
        return Some(chosen);
    }
    println!("Invalid choice. No foreground target set.");
    None
}

// ── AI conversation ─────────────────────────────────────────────────────────

/// Context for SIGWINCH handling during streaming in `ask_with_session`.
struct StreamResizeDims<'a> {
    width: &'a mut usize,
    height: &'a mut usize,
    start: std::time::Instant,
    model: String,
    daemon_up: bool,
    /// True when the input frame (borders + status bar) is currently drawn.
    /// When false, only dimensions are updated; caller redraws after streaming.
    has_frame: bool,
}

/// Called from the SIGWINCH arms inside `ask_with_session`.
/// Re-queries dimensions, erases the old frame if visible, and redraws.
fn apply_stream_resize(
    d: &mut StreamResizeDims<'_>,
    session_id: Option<&str>,
    approval: &SessionApproval,
    prompt_tokens: u32,
    context_window: u32,
) {
    use std::io::Write;
    let old_height = *d.height;
    *d.width = terminal_width();
    *d.height = terminal_height();

    if !d.has_frame {
        // Frame not drawn yet; caller will set up scroll region after streaming.
        return;
    }

    // Reset scroll region so absolute cursor positioning can reach any row.
    print!("\x1b[r");
    // With input_rows == 1, 4 rows are reserved: top_border (height-3),
    // input row (height-2), bottom_border (height-1), status bar (height).
    let old_frame_top = old_height.saturating_sub(3).max(1);
    for r in old_frame_top..=old_height {
        print!("\x1b[{r};1H\x1b[2K");
    }
    std::io::stdout().flush().ok();

    setup_scroll_region(*d.height);
    draw_input_frame(*d.height, *d.width, d.start);
    let hint = approval.hint();
    draw_status_bar(
        *d.height,
        *d.width,
        &StatusBarState {
            session_id: session_id.unwrap_or(""),
            approval_hint: &hint,
            model: &d.model,
            prompt_tokens,
            context_window,
            daemon_up: d.daemon_up,
        },
    );
}

struct QueryArgs<'a> {
    query: String,
    display_query: &'a str,
    prompt_override: Option<&'a str>,
}

struct AskTmuxCtx<'a> {
    session: Option<&'a str>,
    pane: Option<&'a str>,
}

struct TokenCtx<'a> {
    prompt_tokens: &'a mut u32,
    context_window: u32,
}

struct StreamCtx<'a> {
    stdin: &'a AsyncStdin,
    chat_width: Option<usize>,
    old_termios: libc::termios,
    sigwinch: Option<&'a mut tokio::signal::unix::Signal>,
    resize: Option<StreamResizeDims<'a>>,
}

async fn ask_with_session(
    qa: QueryArgs<'_>,
    session_id: Option<&str>,
    approval: &mut SessionApproval,
    tmux: AskTmuxCtx<'_>,
    tok: TokenCtx<'_>,
    stream: StreamCtx<'_>,
) -> Result<()> {
    let QueryArgs {
        query,
        display_query,
        prompt_override,
    } = qa;
    let AskTmuxCtx {
        session: tmux_session,
        pane: target_pane,
    } = tmux;
    let TokenCtx {
        prompt_tokens,
        context_window,
    } = tok;
    let StreamCtx {
        stdin,
        chat_width,
        old_termios,
        sigwinch,
        resize,
    } = stream;
    let mut sigwinch = sigwinch;
    let mut resize = resize;
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

    send_request(
        &mut tx,
        Request::Ask {
            query,
            tmux_pane,
            session_id: session_id.map(|s| s.to_string()),
            chat_pane,
            prompt: prompt_override.map(|s| s.to_string()),
            chat_width,
            tmux_session: tmux_session.map(|s| s.to_string()),
            target_pane: target_pane.map(|s| s.to_string()),
        },
    )
    .await?;

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
    // Verbs rotate every ~5 s (62 ticks × 80 ms = 4 960 ms).
    const VERBS: &[&str] = &[
        "scrying",
        "peering",
        "gazing",
        "surveying",
        "scanning",
        "beholding",
        "watching",
        "glimpsing",
        "piercing",
        "discerning",
    ];
    const TICKS_PER_VERB: usize = 62;
    // Start at a random verb so consecutive invocations feel varied.
    let verb_offset = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as usize)
        % VERBS.len();
    let mut spin = verb_offset * TICKS_PER_VERB;
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
                    _ = async {
                        match sigwinch.as_mut() {
                            Some(sw) => { sw.recv().await; }
                            None     => { std::future::pending::<()>().await; }
                        }
                    } => {
                        if let Some(ref mut d) = resize {
                            apply_stream_resize(d, session_id, approval, *prompt_tokens, context_window);
                        }
                    }
                    result = tokio::time::timeout(Duration::from_millis(80), recv(&mut rx)) => {
                        match result {
                            Err(_timeout) => {
                                let verb = VERBS[(spin / TICKS_PER_VERB) % VERBS.len()];
                                const MAX_DOTS: usize = 10;
                                let period = (MAX_DOTS - 1) * 2; // 18
                                let pos = (spin % TICKS_PER_VERB) % period;
                                let dot_count = if pos < MAX_DOTS { pos + 1 } else { period - pos + 1 };
                                let trail = "\x1b[31m".to_string() + &".".repeat(dot_count - 1) + "\x1b[0m";
                                let cursor = "\x1b[33m.\x1b[0m";
                                let dots = format!("{}{}", trail, cursor);
                                print!("\r{} \x1b[33m{}\x1b[0m{}\x1b[K", SPINNER[spin % SPINNER.len()], verb, dots);
                                std::io::stdout().flush()?;
                                spin = spin.wrapping_add(1);
                            }
                            Ok(r) => break r?,
                        }
                    }
                }
            }
        } else {
            // Phase 2 — streaming: race recv against Ctrl+C and SIGWINCH.
            // The timeout is per-message (120 s without any response token).
            loop {
                tokio::select! {
                    biased;
                    byte = stdin.read_byte() => {
                        if byte == Some(0x03) { // Ctrl+C
                            md.flush();
                            println!("\n\x1b[33m⚠ Interrupted\x1b[0m  Session approval revoked.");
                            *approval = SessionApproval::default();
                            return Ok(());
                        }
                        // any other key while streaming is ignored
                    }
                    _ = async {
                        match sigwinch.as_mut() {
                            Some(sw) => { sw.recv().await; }
                            None     => { std::future::pending::<()>().await; }
                        }
                    } => {
                        if let Some(ref mut d) = resize {
                            apply_stream_resize(d, session_id, approval, *prompt_tokens, context_window);
                        }
                    }
                    result = tokio::time::timeout(Duration::from_secs(120), recv(&mut rx)) => {
                        match result {
                            Ok(Ok(msg))   => break msg,
                            Ok(Err(e))    => return Err(e),
                            Err(_elapsed) => anyhow::bail!("Daemon stopped responding (120 s inter-token timeout)"),
                        }
                    }
                }
            }
        };

        match msg {
            Response::KeepAlive => continue,
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
            Response::SessionInfo {
                message_count: _,
                turn_count,
            } => {
                // Print the user query as a bordered box with token budget in the bottom border.
                // Skip for the greeting turn (display_query is empty).
                print!("\r\x1b[K"); // erase spinner line
                if !display_query.is_empty() {
                    print_user_query(&display_query, turn_count, *prompt_tokens, context_window);
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
            Response::ToolCallPrompt {
                id,
                command,
                background,
                target_pane,
            } => {
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

                // Resolve window-relative index for foreground target pane so the
                // user can visually map the pane ID to their tmux layout.
                let target_label = target_pane.as_deref().and_then(|tp| {
                    let out = std::process::Command::new("tmux")
                        .args([
                            "display-message",
                            "-t",
                            tp,
                            "-p",
                            "#{pane_index}\t#{window_name}",
                        ])
                        .output()
                        .ok()?;
                    let s = String::from_utf8_lossy(&out.stdout);
                    let mut parts = s.trim().splitn(2, '\t');
                    let idx = parts.next()?;
                    let win = parts.next().unwrap_or("");
                    Some(format!("pane {} in '{}' ({})", idx, win, tp))
                });

                let mut panel_lines: Vec<String> = vec![cmd_line];
                if let Some(ref lbl) = target_label {
                    panel_lines.push(format!("→ target: {}", lbl));
                }
                let panel_refs: Vec<&str> = panel_lines.iter().map(|s| s.as_str()).collect();
                print_tool_panel(where_label, &panel_refs, false);

                // Visually highlight the target pane while the user decides,
                // then immediately restore focus to the chat pane so the user
                // does not have to manually switch back.
                if let Some(ref tp) = target_pane
                    && !background
                {
                    let _ = std::process::Command::new("tmux")
                        .args(["select-pane", "-t", tp, "-P", "bg=colour17"])
                        .output();
                    if let Ok(my_pane) = std::env::var("TMUX_PANE") {
                        let _ = std::process::Command::new("tmux")
                            .args(["select-pane", "-t", &my_pane])
                            .output();
                    }
                }

                let is_sudo = command_has_sudo(&command);
                let auto_approved = if is_sudo {
                    approval.sudo
                } else {
                    approval.regular
                };

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
                        if is_sudo {
                            approval.sudo = true;
                        } else {
                            approval.regular = true;
                        }
                        println!(
                            "  \x1b[32m✓ approved — all {} commands auto-approved for this session\x1b[0m",
                            if is_sudo { "sudo" } else { "regular" }
                        );
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

                // Remove highlight when denied or redirected — daemon won't execute
                // so it won't clean up the highlight itself.
                if !approved
                    && let Some(ref tp) = target_pane
                    && !background
                {
                    let _ = std::process::Command::new("tmux")
                        .args(["select-pane", "-t", tp, "-P", "default"])
                        .output();
                }

                md.reset();
                send_request(
                    &mut tx,
                    Request::ToolCallResponse {
                        id,
                        approved,
                        user_message,
                    },
                )
                .await?;
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
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
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
                let credential =
                    read_password_silent("   \x1b[33mPassword:\x1b[0m ").unwrap_or_default();
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
                println!(
                    "  \x1b[33m⚙\x1b[0m \x1b[1mWhich pane should receive this command?\x1b[0m"
                );
                println!();
                for (i, pane) in panes.iter().enumerate() {
                    println!(
                        "  \x1b[32m[{}]\x1b[0m  {} — {} — {}",
                        i + 1,
                        pane.id,
                        pane.current_cmd,
                        pane.summary
                    );
                }
                println!();
                print!("  Select pane \x1b[32m›\x1b[0m ");
                std::io::stdout().flush()?;
                // Temporarily revert to cooked mode for user input
                crate::cli::input::restore_termios(old_termios);
                let input = stdin.read_line().await.unwrap_or_default();
                let _ = crate::cli::input::set_raw_mode(); // back to raw mode for turn trap
                let pane_id = input
                    .trim()
                    .parse::<usize>()
                    .ok()
                    .and_then(|n| panes.get(n.saturating_sub(1)))
                    .map(|p| p.id.clone())
                    .unwrap_or_else(|| panes.first().map(|p| p.id.clone()).unwrap_or_default());
                md.reset();
                send_request(&mut tx, Request::PaneSelectResponse { id, pane_id }).await?;
            }
            Response::ScriptDeletePrompt { id, script_name } => {
                if !response_started {
                    print!("\r\x1b[K");
                    response_started = true;
                }
                md.flush();
                println!();
                println!(
                    "  \x1b[33m⚙\x1b[0m \x1b[1mAI wants to delete script:\x1b[0m \x1b[96m{}\x1b[0m",
                    script_name
                );
                println!();
                print!(
                    "  Approve deleting ~/.daemoneye/scripts/{}? \x1b[32m[y/N]\x1b[0m \x1b[32m›\x1b[0m ",
                    script_name
                );
                std::io::stdout().flush()?;
                crate::cli::input::restore_termios(old_termios);
                let input = stdin.read_line().await.unwrap_or_default();
                let _ = crate::cli::input::set_raw_mode();
                let approved = matches!(input.trim().to_lowercase().as_str(), "y" | "yes");
                md.reset();
                send_request(&mut tx, Request::ScriptDeleteResponse { id, approved }).await?;
            }
            Response::ScriptWritePrompt {
                id,
                script_name,
                content,
            } => {
                if !response_started {
                    print!("\r\x1b[K");
                    response_started = true;
                }
                md.flush();
                println!();
                println!(
                    "  \x1b[33m⚙\x1b[0m \x1b[1mAI wants to write script:\x1b[0m \x1b[96m{}\x1b[0m",
                    script_name
                );
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
                print!(
                    "  Approve writing to ~/.daemoneye/scripts/{}? \x1b[32m[y/N]\x1b[0m \x1b[32m›\x1b[0m ",
                    script_name
                );
                std::io::stdout().flush()?;
                // Temporarily revert to cooked mode for user input
                crate::cli::input::restore_termios(old_termios);
                let input = stdin.read_line().await.unwrap_or_default();
                let _ = crate::cli::input::set_raw_mode(); // back to raw mode for turn trap
                let approved = matches!(input.trim().to_lowercase().as_str(), "y" | "yes");
                md.reset();
                send_request(&mut tx, Request::ScriptWriteResponse { id, approved }).await?;
            }
            Response::ScheduleWritePrompt {
                id,
                name,
                kind,
                action,
            } => {
                if !response_started {
                    print!("\r\x1b[K");
                    response_started = true;
                }
                md.flush();
                println!();
                println!(
                    "  \x1b[33m⚙\x1b[0m \x1b[1mAI wants to schedule a job:\x1b[0m \x1b[96m{}\x1b[0m",
                    name
                );
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
                    println!(
                        "  {:<id_w$}  {:<name_w$}  {:<kind_w$}  {:<12}  Next Run",
                        "ID",
                        "Name",
                        "Schedule",
                        "Status",
                        id_w = id_w,
                        name_w = name_w,
                        kind_w = kind_w
                    );
                    println!(
                        "  {}  {}  {}  {}  {}",
                        "─".repeat(id_w),
                        "─".repeat(name_w),
                        "─".repeat(kind_w),
                        "─".repeat(12),
                        "─".repeat(24)
                    );
                    for job in &jobs {
                        let id_short = &job.id[..job.id.len().min(8)];
                        let next = job.next_run.as_deref().unwrap_or("—");
                        println!(
                            "  \x1b[96m{:<id_w$}\x1b[0m  {:<name_w$}  {:<kind_w$}  {:<12}  {}",
                            id_short,
                            job.name,
                            job.kind,
                            job.status,
                            next,
                            id_w = id_w,
                            name_w = name_w,
                            kind_w = kind_w
                        );
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
                    let name_w = scripts
                        .iter()
                        .map(|s| s.name.len())
                        .max()
                        .unwrap_or(4)
                        .max(4);
                    for s in &scripts {
                        println!(
                            "  \x1b[96m{:<name_w$}\x1b[0m  {} bytes",
                            s.name,
                            s.size,
                            name_w = name_w
                        );
                    }
                }
                println!();
                md.reset();
            }
            Response::RunbookWritePrompt {
                id,
                runbook_name,
                content,
            } => {
                if !response_started {
                    print!("\r\x1b[K");
                    response_started = true;
                }
                md.flush();
                println!();
                println!(
                    "  \x1b[33m⚙\x1b[0m \x1b[1mAI wants to write runbook:\x1b[0m \x1b[96m{}\x1b[0m",
                    runbook_name
                );
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
                print!(
                    "  Approve writing to ~/.daemoneye/runbooks/{}.md? \x1b[32m[y/N]\x1b[0m \x1b[32m›\x1b[0m ",
                    runbook_name
                );
                std::io::stdout().flush()?;
                crate::cli::input::restore_termios(old_termios);
                let input = stdin.read_line().await.unwrap_or_default();
                let _ = crate::cli::input::set_raw_mode();
                let approved = matches!(input.trim().to_lowercase().as_str(), "y" | "yes");
                md.reset();
                send_request(&mut tx, Request::RunbookWriteResponse { id, approved }).await?;
            }
            Response::RunbookDeletePrompt {
                id,
                runbook_name,
                active_jobs,
            } => {
                if !response_started {
                    print!("\r\x1b[K");
                    response_started = true;
                }
                md.flush();
                println!();
                println!(
                    "  \x1b[33m⚙\x1b[0m \x1b[1mAI wants to delete runbook:\x1b[0m \x1b[96m{}\x1b[0m",
                    runbook_name
                );
                if !active_jobs.is_empty() {
                    println!();
                    println!(
                        "  \x1b[33mWarning:\x1b[0m the following scheduled jobs reference this runbook:"
                    );
                    for job in &active_jobs {
                        println!("    \x1b[2m- {}\x1b[0m", job);
                    }
                }
                println!();
                print!(
                    "  Approve deleting ~/.daemoneye/runbooks/{}.md? \x1b[32m[y/N]\x1b[0m \x1b[32m›\x1b[0m ",
                    runbook_name
                );
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
                    let name_w = runbooks
                        .iter()
                        .map(|r| r.name.len())
                        .max()
                        .unwrap_or(4)
                        .max(4);
                    for r in &runbooks {
                        let tags = if r.tags.is_empty() {
                            String::new()
                        } else {
                            format!("  \x1b[2m[{}]\x1b[0m", r.tags.join(", "))
                        };
                        println!(
                            "  \x1b[96m{:<name_w$}\x1b[0m{}",
                            r.name,
                            tags,
                            name_w = name_w
                        );
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
                    let cat_w = entries
                        .iter()
                        .map(|e| e.category.len())
                        .max()
                        .unwrap_or(8)
                        .max(8);
                    let key_w = entries
                        .iter()
                        .map(|e| e.key.len())
                        .max()
                        .unwrap_or(3)
                        .max(3);
                    println!("  {:<cat_w$}  Key", "Category", cat_w = cat_w);
                    println!("  {}  {}", "─".repeat(cat_w), "─".repeat(key_w));
                    for e in &entries {
                        println!(
                            "  \x1b[2m{:<cat_w$}\x1b[0m  \x1b[96m{}\x1b[0m",
                            e.category,
                            e.key,
                            cat_w = cat_w
                        );
                    }
                }
                println!();
                md.reset();
            }
            Response::DaemonStatus { .. } => {
                // Not expected in the AI streaming loop; ignore.
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
    let socket_path = default_socket_path();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        UnixStream::connect(&socket_path),
    )
    .await
    .with_context(|| {
        format!(
            "Timed out connecting to daemon at {} (is it running?)",
            socket_path.display()
        )
    })?
    .with_context(|| format!("Failed to connect to daemon at {}", socket_path.display()))
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

use anyhow::Result;
use std::collections::HashSet;

use crate::cli::input::*;
use crate::cli::render::*;
use crate::config::Config;

mod approval;
mod approval_ui;
mod ask;
mod ipc_client;
mod lifecycle;
mod pane;
mod setup;
mod stream;

pub use ask::run_ask;
pub use ipc_client::{connect, recv, send_request};
pub use lifecycle::{run_logs, run_ping, run_stop};
pub use setup::run_setup;

use approval::SessionApproval;
use ipc_client::{
    new_session_id, send_delete_saved_session, send_diff_sessions, send_list_models,
    send_list_panes_for_session, send_list_saved_sessions, send_load_session, send_query_limits,
    send_refresh, send_rename_session, send_reset_session_tool_count, send_save_session,
    send_set_model, send_set_pane,
};
use pane::resolve_target_pane;
use stream::{AskTmuxCtx, QueryArgs, StreamCtx, StreamResizeDims, TokenCtx, ask_with_session};

/// Render the two-column slash-command reference, centered to `chat_width`.
/// Shown on `/help` during chat and whenever the caller wants the full command list.
fn render_slash_command_help(chat_width: usize) {
    // (left_cmd, left_desc, right_cmd, right_desc)
    let rows: &[(&str, &str, &str, &str)] = &[
        ("/help", "show this list", "/exit", "quit"),
        ("/clear", "reset session", "/refresh", "resync context"),
        (
            "/model [name]",
            "list or switch model",
            "/pane [%N]",
            "list or pin target pane",
        ),
        (
            "/approvals [revoke]",
            "list or revoke approvals",
            "/prompt <name>",
            "switch system prompt",
        ),
        (
            "/limits",
            "show active limits",
            "/limits reset",
            "reset session tool counter",
        ),
        (
            "/session save <name>",
            "save session",
            "/session load <name>",
            "resume session",
        ),
        (
            "/session list",
            "list saved sessions",
            "/session delete <name>",
            "delete saved session",
        ),
        (
            "/session rename <old> <new>",
            "rename session",
            "/session diff <n1> <n2>",
            "compare two sessions",
        ),
        ("/session tag <name>", "alias for /session save", "", ""),
    ];
    let lc_w = rows.iter().map(|(c, _, _, _)| c.len()).max().unwrap_or(0);
    let ld_w = rows.iter().map(|(_, d, _, _)| d.len()).max().unwrap_or(0);
    let rc_w = rows.iter().map(|(_, _, c, _)| c.len()).max().unwrap_or(0);
    let rd_w = rows.iter().map(|(_, _, _, d)| d.len()).max().unwrap_or(0);
    // visible block width: lc_w + " — " (3) + ld_w + "    " (4) + rc_w + " — " (3) + rd_w
    let block_w = lc_w + 3 + ld_w + 4 + rc_w + 3 + rd_w;
    let pad = " ".repeat((chat_width.saturating_sub(block_w)) / 2);
    let divider = format!("{pad}\x1b[2m{}\x1b[0m", "─".repeat(block_w));
    println!();
    println!("{divider}");
    for (lc, ld, rc, rd) in rows {
        let left_cmd = format!("\x1b[96m{lc:<lc_w$}\x1b[0m");
        let left_desc = format!("\x1b[2m— {ld:<ld_w$}\x1b[0m");
        let right_cmd = format!("\x1b[96m{rc:<rc_w$}\x1b[0m");
        let right_desc = format!("\x1b[2m— {rd}\x1b[0m");
        println!("{pad}{left_cmd} {left_desc}    {right_cmd} {right_desc}");
    }
    println!("{divider}");
    println!();
}

pub async fn run_chat(session_override: Option<String>) -> Result<()> {
    let result = run_chat_inner(session_override).await;
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

async fn run_chat_inner(session_override: Option<String>) -> Result<()> {
    // ── Managed-session auto-attach ────────────────────────────────────────────
    // When the daemon is configured with a managed tmux session and `daemoneye chat`
    // is invoked from outside tmux, transparently open a chat window in that session
    // and exec-attach so the user lands in the right place.
    //
    // This is a no-op when:
    //   - $TMUX_PANE is already set (already inside tmux), or
    //   - no managed session is configured, or
    //   - tmux is not available.
    if std::env::var("TMUX_PANE").is_err() {
        use std::os::unix::process::CommandExt as _;
        let managed = session_override.clone().or_else(|| {
            let cfg = Config::load().unwrap_or_default();
            let s = cfg.daemon.tmux_session;
            if s.is_empty() { None } else { Some(s) }
        });
        if let Some(ref sname) = managed {
            if crate::tmux::session_exists(sname) {
                // Open a chat window in the managed session.  The new window
                // runs its own `daemoneye chat` invocation which will find
                // $TMUX_PANE set and proceed normally.
                let chat_cmd = std::env::current_exe()
                    .map(|p| format!("{} chat", p.display()))
                    .unwrap_or_else(|_| "daemoneye chat".to_string());
                let _ = std::process::Command::new("tmux")
                    .args(["new-window", "-t", sname, "-n", "chat", &chat_cmd])
                    .output();
                // Replace this process with `tmux attach-session`.  The user's
                // terminal is now "inside" the session where the chat window lives.
                let err = std::process::Command::new("tmux")
                    .args(["attach-session", "-t", sname])
                    .exec();
                // exec() only returns on error.
                anyhow::bail!(
                    "Failed to attach to managed tmux session '{}': {}",
                    sname,
                    err
                );
            } else {
                eprintln!(
                    "Managed tmux session '{}' does not exist yet.\n\
                     Is the daemon running?  daemoneye daemon\n\
                     Once it starts, run: tmux attach -t {}",
                    sname, sname
                );
                anyhow::bail!("Managed session '{}' not found", sname);
            }
        }
    }

    let start_time = std::time::Instant::now();
    let session_id = new_session_id();
    // None = use daemon's configured default prompt; Some(name) = override.
    let current_prompt: Option<String> = None;
    let stdin = crate::cli::input::AsyncStdin::new()?;
    let mut input_state = InputState::new();
    let chat_start_config = Config::load().unwrap_or_default();
    let mut approval = SessionApproval::from_config(&chat_start_config.approvals);
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

    // Compact splash hint — full command list is available via /help.
    {
        let hint_plain = "/help  — list commands      /exit  — quit";
        let hint = format!(
            "\x1b[96m/help\x1b[0m  \x1b[2m— list commands\x1b[0m      \x1b[96m/exit\x1b[0m  \x1b[2m— quit\x1b[0m"
        );
        let pad = " ".repeat((chat_width.saturating_sub(hint_plain.chars().count())) / 2);
        println!();
        println!("{pad}{hint}");
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
    let model_pre = config.resolve_model(None).model.clone();
    let ctx_pre = config.resolve_model(None).context_window();
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
    let mut context_window = config.resolve_model(None).context_window();
    let mut model = config.resolve_model(None).model.clone();

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

        if query == "/exit" || query == "/quit" || query == "exit" || query == "quit" {
            break;
        }
        if query == "/help" || query == "help" || query == "?" || query == "/?" {
            render_slash_command_help(chat_width);
            continue;
        }
        if query == "/clear" {
            session_id = new_session_id();
            *approval = SessionApproval::from_config(&config.approvals);
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
                *approval = SessionApproval::from_config(&config.approvals);
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
        if query == "/model" {
            match send_list_models(&session_id).await {
                Ok((models, active)) => {
                    let col_w = models.iter().map(|(key, _)| key.len()).max().unwrap_or(0);
                    println!();
                    for (key, model_id) in &models {
                        if key == &active {
                            println!(
                                "  \x1b[32m▸\x1b[0m \x1b[1m{:<col_w$}  {}\x1b[0m \x1b[90m(active)\x1b[0m",
                                key,
                                model_id,
                                col_w = col_w
                            );
                        } else {
                            println!("    {:<col_w$}  {}", key, model_id, col_w = col_w);
                        }
                    }
                    println!();
                }
                Err(e) => println!("\x1b[31m✗\x1b[0m  /model failed: {}", e),
            }
            continue;
        }
        if let Some(name) = query.strip_prefix("/model ").map(str::trim) {
            let name = name.to_string();
            match send_set_model(&session_id, &name).await {
                Ok(new_model) => {
                    // Update the local model name and context window for the status bar.
                    model = new_model.clone();
                    context_window = config.resolve_model(Some(&name)).context_window();
                    let label = format!(" model: {} ", new_model);
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
                Err(e) => println!("\x1b[31m✗\x1b[0m  {}", e),
            }
            continue;
        }
        if query == "/pane" {
            match send_list_panes_for_session(&session_id).await {
                Ok(panes) if panes.is_empty() => {
                    println!(
                        "\x1b[90mNo targetable panes found. Open a terminal pane alongside chat.\x1b[0m"
                    );
                }
                Ok(panes) => {
                    println!();
                    println!(
                        "    \x1b[2m{:<6}  {:<4}  {:<14}  WINDOW\x1b[0m",
                        "ID", "IDX", "COMMAND"
                    );
                    for (id, cmd, window, pane_idx, is_target) in &panes {
                        if *is_target {
                            println!(
                                "  \x1b[32m▸\x1b[0m \x1b[1m{:<6}  {:<4}  {:<14}  {}\x1b[0m \x1b[90m(current target)\x1b[0m",
                                id, pane_idx, cmd, window
                            );
                        } else {
                            println!("    {:<6}  {:<4}  {:<14}  {}", id, pane_idx, cmd, window);
                        }
                    }
                    println!();
                    println!(
                        "\x1b[90mUse \x1b[0m\x1b[96m/pane %N\x1b[0m\x1b[90m to pin a pane by ID.\x1b[0m"
                    );
                    println!();
                }
                Err(e) => println!("\x1b[31m✗\x1b[0m  /pane failed: {}", e),
            }
            continue;
        }
        if let Some(arg) = query.strip_prefix("/pane ").map(str::trim) {
            // Accept "%N" pane IDs directly.
            let pane_id = arg.to_string();
            match send_set_pane(&session_id, &pane_id).await {
                Ok((id, desc)) => {
                    let label = format!(" pane target: {} ", desc);
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
                    // Emit a system message into the AI context so it knows the target changed.
                    // We inject it as a user turn on the next send — but simpler: just note it
                    // locally. The [FOREGROUND TARGET] line on the next turn carries the update.
                    let _ = id; // used in the label above
                }
                Err(e) => println!("\x1b[31m✗\x1b[0m  {}", e),
            }
            continue;
        }
        if query == "/refresh" {
            match send_refresh().await {
                Ok(()) => {
                    session_id = new_session_id();
                    *approval = SessionApproval::from_config(&config.approvals);
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
        if query == "/approvals" {
            println!();
            println!("  \x1b[1mApproval status\x1b[0m");
            println!();
            let cmd_regular = if approval.regular {
                "\x1b[32m⚡ auto (default — revoke to gate)\x1b[0m"
            } else {
                "\x1b[31m✗ gated (requires confirmation)\x1b[0m"
            };
            let cmd_sudo = if approval.sudo {
                "\x1b[32m⚡ session\x1b[0m"
            } else {
                "\x1b[2moff\x1b[0m"
            };
            println!("  Terminal commands (regular)  {}", cmd_regular);
            println!("  Terminal commands (sudo)     {}", cmd_sudo);
            if approval.scripts_all {
                println!("  Scripts                      \x1b[32m⚡ all (config)\x1b[0m");
            } else if approval.scripts.is_empty() {
                println!("  Scripts                      \x1b[2mnone\x1b[0m");
            } else {
                let mut names: Vec<&str> = approval.scripts.iter().map(|s| s.as_str()).collect();
                names.sort_unstable();
                for (i, name) in names.iter().enumerate() {
                    if i == 0 {
                        println!("  Scripts                      \x1b[32m⚡\x1b[0m {}", name);
                    } else {
                        println!("                               \x1b[32m⚡\x1b[0m {}", name);
                    }
                }
            }
            if approval.runbooks_all {
                println!("  Runbooks                     \x1b[32m⚡ all (config)\x1b[0m");
            } else if approval.runbooks.is_empty() {
                println!("  Runbooks                     \x1b[2mnone\x1b[0m");
            } else {
                let mut names: Vec<&str> = approval.runbooks.iter().map(|s| s.as_str()).collect();
                names.sort_unstable();
                for (i, name) in names.iter().enumerate() {
                    if i == 0 {
                        println!("  Runbooks                     \x1b[32m⚡\x1b[0m {}", name);
                    } else {
                        println!("                               \x1b[32m⚡\x1b[0m {}", name);
                    }
                }
            }
            if approval.file_edits_all {
                println!("  Files                        \x1b[32m⚡ all (config)\x1b[0m");
            } else if approval.file_edits.is_empty() {
                println!("  Files                        \x1b[2mnone\x1b[0m");
            } else {
                let home = std::env::var("HOME").unwrap_or_default();
                let mut paths: Vec<&str> = approval.file_edits.iter().map(|s| s.as_str()).collect();
                paths.sort_unstable();
                for (i, path) in paths.iter().enumerate() {
                    let display = if !home.is_empty() && path.starts_with(&home) {
                        format!("~{}", &path[home.len()..])
                    } else {
                        path.to_string()
                    };
                    if i == 0 {
                        println!(
                            "  Files                        \x1b[32m⚡\x1b[0m {}",
                            display
                        );
                    } else {
                        println!(
                            "                               \x1b[32m⚡\x1b[0m {}",
                            display
                        );
                    }
                }
            }
            println!();
            println!("  Use \x1b[96m/approvals revoke\x1b[0m to reset all, or revoke by class:");
            println!(
                "    \x1b[96m/approvals revoke commands\x1b[0m  \
                 \x1b[96m/approvals revoke scripts\x1b[0m"
            );
            println!(
                "    \x1b[96m/approvals revoke runbooks\x1b[0m  \
                 \x1b[96m/approvals revoke files\x1b[0m"
            );
            println!();
            continue;
        }
        // Per-class revoke helpers: update approval, print separator, refresh bar.
        macro_rules! do_revoke {
            ($label:expr) => {{
                let label = $label;
                let dashes = chat_width.min(72).saturating_sub(visual_len(label) + 1);
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
            }};
        }
        if query == "/approvals revoke" {
            *approval = SessionApproval {
                regular: false,
                sudo: false,
                scripts_all: false,
                scripts: HashSet::new(),
                runbooks_all: false,
                runbooks: HashSet::new(),
                file_edits_all: false,
                file_edits: HashSet::new(),
            };
            do_revoke!(" all approvals revoked — commands now require confirmation ");
        }
        if query == "/approvals revoke commands" {
            approval.regular = false;
            approval.sudo = false;
            do_revoke!(" command approvals revoked — commands now require confirmation ");
        }
        if query == "/approvals revoke scripts" {
            approval.scripts_all = false;
            approval.scripts.clear();
            do_revoke!(" script approvals reset ");
        }
        if query == "/approvals revoke runbooks" {
            approval.runbooks_all = false;
            approval.runbooks.clear();
            do_revoke!(" runbook approvals reset ");
        }
        if query == "/approvals revoke files" {
            approval.file_edits_all = false;
            approval.file_edits.clear();
            do_revoke!(" file approvals reset ");
        }
        if query == "/limits" {
            match send_query_limits(&session_id).await {
                Ok((limits, turn_count, tool_calls_this_session, history_len)) => {
                    let fmt_u32 = |v: u32| {
                        if v == 0 {
                            "unlimited".to_string()
                        } else {
                            v.to_string()
                        }
                    };
                    let fmt_us = |v: usize| {
                        if v == 0 {
                            "unlimited".to_string()
                        } else {
                            v.to_string()
                        }
                    };
                    println!();
                    println!("  \x1b[1mLimits\x1b[0m");
                    println!(
                        "  per_tool_batch             {}",
                        fmt_u32(limits.per_tool_batch)
                    );
                    println!(
                        "  total_tool_calls_per_turn  {}",
                        fmt_u32(limits.total_tool_calls_per_turn)
                    );
                    println!(
                        "  max_tool_calls_per_session {}",
                        fmt_us(limits.max_tool_calls_per_session)
                    );
                    println!(
                        "  tool_result_chars          {}",
                        fmt_us(limits.tool_result_chars)
                    );
                    println!(
                        "  max_history                {}",
                        fmt_us(limits.max_history)
                    );
                    println!("  max_turns                  {}", fmt_us(limits.max_turns));
                    if !limits.per_tool_overrides.is_empty() {
                        println!("  per_tool overrides:");
                        for (name, cap) in &limits.per_tool_overrides {
                            println!("    {}  {}", name, fmt_u32(*cap));
                        }
                    }
                    println!();
                    println!("  \x1b[1mSession counters\x1b[0m");
                    println!("  turn count       {}", turn_count);
                    println!("  session tools    {}", tool_calls_this_session);
                    println!("  history length   {}", history_len);
                    println!();
                }
                Err(e) => println!("\x1b[31m✗\x1b[0m  /limits failed: {}", e),
            }
            continue;
        }
        if query == "/limits reset" {
            match send_reset_session_tool_count(&session_id).await {
                Ok(()) => {
                    let label = " session tool call counter reset ";
                    let dashes = chat_width.min(72).saturating_sub(label.len() + 1);
                    println!("\x1b[2m─{}{}\x1b[0m", label, "─".repeat(dashes));
                }
                Err(e) => println!("\x1b[31m✗\x1b[0m  /limits reset failed: {}", e),
            }
            continue;
        }

        // ── /session commands ─────────────────────────────────────────────────
        if query == "/session list" || query == "/session" {
            match send_list_saved_sessions().await {
                Ok(sessions_list) if sessions_list.is_empty() => {
                    println!(
                        "\x1b[90mNo saved sessions. Use \x1b[0m\x1b[96m/session save <name>\x1b[0m\x1b[90m to save this session.\x1b[0m"
                    );
                }
                Ok(sessions_list) => {
                    println!();
                    let name_w = sessions_list
                        .iter()
                        .map(|s| s.name.len())
                        .max()
                        .unwrap_or(4)
                        .max(4);
                    println!(
                        "  \x1b[2m{:<name_w$}  {:<26}  turns  msgs  artifacts  description\x1b[0m",
                        "name",
                        "last updated",
                        name_w = name_w
                    );
                    for s in &sessions_list {
                        let last = s
                            .last_updated
                            .get(..16)
                            .unwrap_or(&s.last_updated)
                            .replace('T', " ");
                        let desc = if s.description.len() > 40 {
                            format!("{}…", &s.description[..39])
                        } else {
                            s.description.clone()
                        };
                        println!(
                            "  \x1b[96m{:<name_w$}\x1b[0m  {:<26}  {:<5}  {:<4}  {:<9}  \x1b[2m{}\x1b[0m",
                            s.name,
                            last,
                            s.turn_count,
                            s.message_count,
                            s.artifact_count,
                            desc,
                            name_w = name_w
                        );
                    }
                    println!();
                }
                Err(e) => println!("\x1b[31m✗\x1b[0m  /session list failed: {}", e),
            }
            continue;
        }

        // /session save <name> [description...]
        if let Some(rest) = query.strip_prefix("/session save ").map(str::trim) {
            let (name, description) = rest
                .split_once(' ')
                .map(|(n, d)| (n.trim(), d.trim()))
                .unwrap_or((rest, ""));
            let name = name.to_string();
            let description = description.to_string();
            let force = name.ends_with(" --force") || description.ends_with("--force");
            let name = name.trim_end_matches(" --force").to_string();
            match send_save_session(&session_id, &name, &description, force).await {
                Ok(confirmed) => {
                    let label = format!(" session saved as '{}' ", confirmed);
                    let dashes = chat_width.min(72).saturating_sub(visual_len(&label) + 1);
                    println!("\x1b[2m─{}{}\x1b[0m", label, "─".repeat(dashes));
                }
                Err(e) => println!("\x1b[31m✗\x1b[0m  /session save failed: {}", e),
            }
            continue;
        }

        // /session load <name>
        if let Some(name) = query.strip_prefix("/session load ").map(str::trim) {
            let force = name.ends_with(" --force");
            let name = name.trim_end_matches(" --force").trim().to_string();
            match send_load_session(&session_id, &name, force).await {
                Ok((loaded_name, banner)) => {
                    let label = format!(" resumed '{}' ", loaded_name);
                    let dashes = chat_width.min(72).saturating_sub(visual_len(&label) + 1);
                    println!("\x1b[2m─{}{}\x1b[0m", label, "─".repeat(dashes));
                    if !banner.is_empty() {
                        for line in banner.lines() {
                            println!("  \x1b[33m{}\x1b[0m", line);
                        }
                        println!();
                    }
                }
                Err(e) => println!("\x1b[31m✗\x1b[0m  /session load failed: {}", e),
            }
            continue;
        }

        // /session delete <name>
        if let Some(name) = query.strip_prefix("/session delete ").map(str::trim) {
            let name = name.to_string();
            match send_delete_saved_session(&name).await {
                Ok(()) => {
                    let label = format!(" session '{}' deleted ", name);
                    let dashes = chat_width.min(72).saturating_sub(visual_len(&label) + 1);
                    println!("\x1b[2m─{}{}\x1b[0m", label, "─".repeat(dashes));
                }
                Err(e) => println!("\x1b[31m✗\x1b[0m  /session delete failed: {}", e),
            }
            continue;
        }

        // /session rename <old> <new>
        if let Some(rest) = query.strip_prefix("/session rename ").map(str::trim) {
            match rest.split_once(' ') {
                Some((old, new)) => {
                    let old = old.trim().to_string();
                    let new = new.trim().to_string();
                    match send_rename_session(&old, &new).await {
                        Ok(()) => {
                            let label = format!(" session '{}' renamed to '{}' ", old, new);
                            let dashes = chat_width.min(72).saturating_sub(visual_len(&label) + 1);
                            println!("\x1b[2m─{}{}\x1b[0m", label, "─".repeat(dashes));
                        }
                        Err(e) => println!("\x1b[31m✗\x1b[0m  /session rename failed: {}", e),
                    }
                }
                None => println!("Usage: /session rename <old-name> <new-name>"),
            }
            continue;
        }

        // /session diff <name1> <name2>
        if let Some(rest) = query.strip_prefix("/session diff ").map(str::trim) {
            match rest.split_once(' ') {
                Some((n1, n2)) => {
                    let n1 = n1.trim().to_string();
                    let n2 = n2.trim().to_string();
                    println!("\x1b[2mComparing '{}' and '{}'…\x1b[0m", n1, n2);
                    match send_diff_sessions(&n1, &n2).await {
                        Ok(summary) => {
                            println!();
                            for line in summary.lines() {
                                println!("  {}", line);
                            }
                            println!();
                        }
                        Err(e) => println!("\x1b[31m✗\x1b[0m  /session diff failed: {}", e),
                    }
                }
                None => println!("Usage: /session diff <name1> <name2>"),
            }
            continue;
        }

        // /session tag <name> [description...] — alias for /session save
        if let Some(rest) = query.strip_prefix("/session tag ").map(str::trim) {
            let (name, description) = rest
                .split_once(' ')
                .map(|(n, d)| (n.trim(), d.trim()))
                .unwrap_or((rest, ""));
            let description = description.to_string();
            let force = name.ends_with(" --force") || description.ends_with("--force");
            let name = name.trim_end_matches(" --force").to_string();
            match send_save_session(&session_id, &name, &description, force).await {
                Ok(confirmed) => {
                    let label = format!(" session tagged as '{}' ", confirmed);
                    let dashes = chat_width.min(72).saturating_sub(visual_len(&label) + 1);
                    println!("\x1b[2m─{}{}\x1b[0m", label, "─".repeat(dashes));
                }
                Err(e) => println!("\x1b[31m✗\x1b[0m  /session tag failed: {}", e),
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

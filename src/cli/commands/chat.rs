use anyhow::Result;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::cli::input::*;
use crate::cli::render::*;
use crate::cli::render_ratatui::RatatuiRendererStdout;
use crate::config::Config;

use super::approval::SessionApproval;
use super::ipc_client::new_session_id;
use super::pane::resolve_target_pane;
use super::slash;
use super::stream::{AskTmuxCtx, QueryArgs, RatatuiQueryCtx, TokenCtx, ask_with_session_ratatui};

const HELP_TEXT: &str = "\
Commands:
  /help      show this list           /exit      quit
  /clear     reset session            /refresh   resync host context
  /model     list or switch model     /pane      list or pin target pane
  /approvals list/on/off/revoke       /prompt    list or switch system prompt
  /limits    show active limits       /session   save/load/list/delete/rename

Aliases: /models /panes /sessions /approval /new /quit (and help, ? open help).

At a tool-approval prompt, type a message instead of Y/A/N to redirect the agent.
Tool output is capped at 10 lines on screen (… N more lines); full output is kept in history.

Up/Down navigate the input; at the top/bottom edge they recall history.
";

pub(super) async fn run_chat_inner(session_override: Option<String>) -> Result<()> {
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
                let _ = crate::tmux::bounded_output(std::process::Command::new("tmux").args([
                    "new-window",
                    "-t",
                    sname,
                    "-n",
                    "chat",
                    &chat_cmd,
                ]));
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
    } else {
        chat_width = terminal_width();
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
        } // stable for SETTLE_MS — proceed
    }
    // Ask tmux to forward focus events to applications (DEC mode 1004) so the
    // chat process is notified (ESC[I) when the user switches back to this pane
    // and can re-pin the input box to the bottom.  Best-effort; harmless if tmux
    // is absent or the option is already set.
    if pane_id_opt.is_some() {
        let _ = crate::tmux::bounded_output(std::process::Command::new("tmux").args([
            "set",
            "-g",
            "focus-events",
            "on",
        ]));
    }

    // Ratatui path: create the inline-viewport renderer (enters raw mode
    // internally).
    let renderer = match RatatuiRendererStdout::new(start_time) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Failed to initialise ratatui renderer: {e}");
            return Err(e.into());
        }
    };

    let result = run_chat_inner_raw(
        InputHandles {
            state: &mut input_state,
            stdin: &stdin,
            sigwinch: &mut sigwinch,
        },
        TerminalCtx { chat_width },
        session_id,
        current_prompt,
        &mut approval,
        TmuxCtx {
            session: tmux_session,
            pane: target_pane,
        },
        renderer,
    )
    .await;
    // Ratatui renderer is consumed by run_chat_inner_raw (restores on exit).
    result
}

struct InputHandles<'a> {
    state: &'a mut InputState,
    stdin: &'a AsyncStdin,
    sigwinch: &'a mut tokio::signal::unix::Signal,
}

struct TerminalCtx {
    chat_width: usize,
}

struct TmuxCtx {
    session: Option<String>,
    pane: Option<String>,
}

struct RatatuiCtx<'a> {
    input_state: &'a mut InputState,
    stdin: &'a AsyncStdin,
    sigwinch: &'a mut tokio::signal::unix::Signal,
    renderer: RatatuiRendererStdout,
    chat_width: usize,
    session_id: String,
    current_prompt: Option<String>,
    approval: &'a mut SessionApproval,
    tmux_session: Option<String>,
    target_pane: Option<String>,
}

async fn run_chat_inner_raw(
    handles: InputHandles<'_>,
    term: TerminalCtx,
    session_id: String,
    current_prompt: Option<String>,
    approval: &mut SessionApproval,
    tmux: TmuxCtx,
    renderer: RatatuiRendererStdout,
) -> Result<()> {
    let InputHandles {
        state: input_state,
        stdin,
        sigwinch,
    } = handles;
    let TerminalCtx { chat_width } = term;
    let TmuxCtx {
        session: tmux_session,
        pane: target_pane,
    } = tmux;

    run_chat_ratatui(RatatuiCtx {
        input_state,
        stdin,
        sigwinch,
        renderer,
        chat_width,
        session_id,
        current_prompt,
        approval,
        tmux_session,
        target_pane,
    })
    .await
}
// ── Ratatui chat loop ──────────────────────────────────────────────
async fn run_chat_ratatui(ctx: RatatuiCtx<'_>) -> Result<()> {
    let RatatuiCtx {
        input_state,
        stdin,
        sigwinch,
        mut renderer,
        mut chat_width,
        mut session_id,
        mut current_prompt,
        approval,
        tmux_session,
        mut target_pane,
    } = ctx;
    let mut last_ctrl_c: Option<std::time::Instant> = None;
    let mut daemon_up = false;
    let mut prompt_tokens: u32 = 0;
    let mut cost_usd: f64 = 0.0;
    let mut has_untracked: bool = false;
    let config = Config::load().unwrap_or_default();
    let mut context_window = config.resolve_model(None).context_window();
    let mut model = config.resolve_model(None).model.clone();

    // Wait for tmux client attachment.
    loop {
        let attached = crate::tmux::bounded_output(std::process::Command::new("tmux").args([
            "display-message",
            "-p",
            "#{session_attached}",
        ]))
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(1);
        if attached > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    // Helper: re-draw the live region.
    let redraw = |r: &mut RatatuiRendererStdout,
                  is: &InputState,
                  sid: &str,
                  ap: &SessionApproval,
                  mdl: &str,
                  pt: u32,
                  cw: u32,
                  du: bool,
                  cost: f64,
                  hu: bool| {
        let sb = StatusBarState {
            session_id: sid,
            approval_hint: &ap.hint(),
            model: mdl,
            prompt_tokens: pt,
            context_window: cw,
            daemon_up: du,
            tools_total: 0,
            cost_usd: cost,
            has_untracked: hu,
        };
        let _ = r.draw(is.current_line(), &sb);
    };

    redraw(
        &mut renderer,
        input_state,
        &session_id,
        approval,
        &model,
        prompt_tokens,
        context_window,
        daemon_up,
        0.0,
        false,
    );

    let _ = renderer.commit_styled(&banner_lines(
        chat_width,
        &crate::cli::palette::Palette::from_env(),
    ));

    // Send the greeting query — minimal rendering via ratatui commit path.
    {
        let cw = chat_width;
        match ask_with_session_ratatui(
            QueryArgs {
                query: "Hello!".to_string(),
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
            RatatuiQueryCtx {
                chat_width: Some(cw),
                session_cost: &mut cost_usd,
                session_has_untracked: &mut has_untracked,
                renderer: &mut renderer,
                model: &model,
                stdin,
            },
        )
        .await
        {
            Ok(()) => {
                daemon_up = true;
            }
            Err(e) => {
                eprintln!("\x1b[31m✗\x1b[0m Could not reach the daemon: {}", e);
                eprintln!("  Make sure it is running:  \x1b[1mdaemoneye daemon --console\x1b[0m");
                eprintln!("  \x1b[2mWaiting for your input…\x1b[0m");
            }
        }
    }

    redraw(
        &mut renderer,
        input_state,
        &session_id,
        approval,
        &model,
        prompt_tokens,
        context_window,
        daemon_up,
        0.0,
        false,
    );

    // Help text for /help command.
    let help_text = HELP_TEXT;

    let user_host = local_user_host();

    loop {
        // Read input using the existing key handler but render via ratatui.
        let line_opt = read_input_line_inner_ratatui(RatatuiInputCtx {
            state: input_state,
            stdin,
            sigwinch,
            renderer: &mut renderer,
            chat_width: &mut chat_width,
            session_id: &session_id,
            approval,
            model: &model,
            prompt_tokens,
            context_window,
            daemon_up,
            cost_usd,
            has_untracked,
            last_ctrl_c: &mut last_ctrl_c,
        })
        .await?;

        let Some(line) = line_opt else { break };

        let query = line.trim().to_string();
        if query.is_empty() {
            continue;
        }

        input_state.push_history(query.clone());

        if query == "/exit" || query == "/quit" || query == "exit" || query == "quit" {
            break;
        }
        if query == "/help" || query == "help" || query == "?" || query == "/?" {
            let _ = renderer.commit(help_text);
            let _ = renderer.commit("\n");
            redraw(
                &mut renderer,
                input_state,
                &session_id,
                approval,
                &model,
                prompt_tokens,
                context_window,
                daemon_up,
                cost_usd,
                has_untracked,
            );
            continue;
        }
        if query == "/clear" || query == "/new" {
            // Start a fresh conversation: new session id, reset counters.
            session_id = uuid::Uuid::new_v4().to_string();
            prompt_tokens = 0;
            cost_usd = 0.0;
            has_untracked = false;
            let _ = renderer.commit("  ✓ started a fresh session\n");
            redraw(
                &mut renderer,
                input_state,
                &session_id,
                approval,
                &model,
                prompt_tokens,
                context_window,
                daemon_up,
                cost_usd,
                has_untracked,
            );
            continue;
        }

        // All other slash commands map to a daemon round-trip or client-side
        // state change.  Command-shaped tokens that aren't recognized are
        // rejected (not sent to the LLM); prose that merely starts with '/'
        // (e.g. a path) falls through to the model.
        if query.starts_with('/') {
            let outcome = slash::handle_slash(
                slash::SlashCtx {
                    renderer: &mut renderer,
                    session_id: &session_id,
                    approval: &mut *approval,
                    model: &mut model,
                    context_window: &mut context_window,
                    current_prompt: &mut current_prompt,
                    target_pane: &mut target_pane,
                },
                &query,
            )
            .await;
            match outcome {
                slash::SlashOutcome::Handled => {
                    redraw(
                        &mut renderer,
                        input_state,
                        &session_id,
                        approval,
                        &model,
                        prompt_tokens,
                        context_window,
                        daemon_up,
                        cost_usd,
                        has_untracked,
                    );
                    continue;
                }
                slash::SlashOutcome::NotACommand => {
                    let first = query.split_whitespace().next().unwrap_or("");
                    if slash::is_command_shaped(first) {
                        let _ =
                            renderer.commit(&format!("  ✗ unknown command: {first} (try /help)\n"));
                        redraw(
                            &mut renderer,
                            input_state,
                            &session_id,
                            approval,
                            &model,
                            prompt_tokens,
                            context_window,
                            daemon_up,
                            cost_usd,
                            has_untracked,
                        );
                        continue;
                    }
                    // Prose starting with '/': fall through to the LLM.
                }
            }
        }

        // Echo the user's words into scrollback so the transcript reads as a
        // conversation. Same element as tool output — one visual grammar for
        // everything committed above the live region.
        if should_echo(&query) {
            let echo_body: Vec<String> = echo_body(&query);
            let _ = renderer.commit_panel(&user_host, &echo_body, false);
        }

        // ── Send the user query ─────────────────────────────────────────
        {
            let cw = chat_width;
            let mut prompt_tokens_copy = prompt_tokens;
            match ask_with_session_ratatui(
                QueryArgs {
                    query,
                    prompt_override: current_prompt.as_deref(),
                },
                Some(&session_id),
                approval,
                AskTmuxCtx {
                    session: tmux_session.as_deref(),
                    pane: target_pane.as_deref(),
                },
                TokenCtx {
                    prompt_tokens: &mut prompt_tokens_copy,
                    context_window,
                },
                RatatuiQueryCtx {
                    chat_width: Some(cw),
                    session_cost: &mut cost_usd,
                    session_has_untracked: &mut has_untracked,
                    renderer: &mut renderer,
                    model: &model,
                    stdin,
                },
            )
            .await
            {
                Ok(()) => {}
                Err(e) => {
                    eprintln!("\x1b[31m✗\x1b[0m {}", e);
                }
            }
            prompt_tokens = prompt_tokens_copy;
        }

        redraw(
            &mut renderer,
            input_state,
            &session_id,
            approval,
            &model,
            prompt_tokens,
            context_window,
            daemon_up,
            cost_usd,
            has_untracked,
        );
    }

    renderer.restore();
    println!("\n\x1b[2mGoodbye.\x1b[0m");
    Ok(())
}

// Read one line of input using the ratatui renderer for display.
struct RatatuiInputCtx<'a> {
    state: &'a mut InputState,
    stdin: &'a AsyncStdin,
    sigwinch: &'a mut tokio::signal::unix::Signal,
    renderer: &'a mut RatatuiRendererStdout,
    chat_width: &'a mut usize,
    session_id: &'a str,
    approval: &'a SessionApproval,
    model: &'a str,
    prompt_tokens: u32,
    context_window: u32,
    daemon_up: bool,
    cost_usd: f64,
    has_untracked: bool,
    last_ctrl_c: &'a mut Option<std::time::Instant>,
}

async fn read_input_line_inner_ratatui(ctx: RatatuiInputCtx<'_>) -> anyhow::Result<Option<String>> {
    let RatatuiInputCtx {
        state,
        stdin,
        sigwinch,
        renderer,
        chat_width,
        session_id,
        approval,
        model,
        prompt_tokens,
        context_window,
        daemon_up,
        cost_usd,
        has_untracked,
        last_ctrl_c,
    } = ctx;
    loop {
        tokio::select! {
            _ = sigwinch.recv() => {
                *chat_width = terminal_width();
                let sb = StatusBarState {
                    session_id,
                    approval_hint: &approval.hint(),
                    model,
                    prompt_tokens,
                    context_window,
                    daemon_up,
                    tools_total: 0,
                    cost_usd,
                    has_untracked,
                };
                let _ = renderer.draw(state.current_line(), &sb);
            }
            key = read_key(stdin) => {
                let Some(key) = key else {
                    return Ok(None);
                };
                match key {
                    Key::Enter => {
                        let s = state.current_line().as_str();
                        return Ok(Some(s));
                    }
                    Key::CtrlJ => {
                        // Ctrl+J or Alt+Enter: insert newline without submitting
                        state.current_line_mut().insert_newline();
                    }
                    Key::CtrlD => {
                        if state.current_line().as_str().is_empty() {
                            return Ok(None);
                        }
                        state.current_line_mut().delete();
                    }
                    Key::CtrlC => {
                        if let Some(t) = last_ctrl_c.as_ref()
                            && t.elapsed() < std::time::Duration::from_millis(1000)
                        {
                            return Ok(None);
                        }
                        *last_ctrl_c = Some(std::time::Instant::now());
                        *state.current_line_mut() = InputLine::new();
                        state.clear_history_nav();
                    }
                    Key::Char(c) if c != '\0' => { state.current_line_mut().insert(c); }
                    Key::Backspace => { state.current_line_mut().backspace(); }
                    Key::Delete => { state.current_line_mut().delete(); }
                    Key::Left => { state.current_line_mut().move_left(); }
                    Key::Right => { state.current_line_mut().move_right(); }
                    Key::Up => {
                        // Move the cursor up within the buffer; only recall the
                        // previous history entry when already on the first row.
                        let content_width = chat_width.saturating_sub(2);
                        if state.current_line().cursor_on_first_visual_row(content_width) {
                            if state.has_history() {
                                state.history_up();
                            }
                        } else {
                            state.current_line_mut().move_up(content_width);
                        }
                    }
                    Key::Down => {
                        // Move the cursor down within the buffer; only recall the
                        // next history entry when already on the last row.
                        let content_width = chat_width.saturating_sub(2);
                        if state.current_line().cursor_on_last_visual_row(content_width) {
                            if state.has_history() {
                                state.history_down();
                            }
                        } else {
                            state.current_line_mut().move_down(content_width);
                        }
                    }
                    Key::FocusGained => {
                        // Returning to this tmux pane: re-pin the input box to the
                        // bottom in case the terminal was repainted while away.
                        renderer.reanchor();
                    }
                    Key::FocusLost => {}
                    Key::Home => { state.current_line_mut().move_home(); }
                    Key::End => { state.current_line_mut().move_end(); }
                    Key::CtrlA => { state.current_line_mut().move_home(); }
                    Key::CtrlE => { state.current_line_mut().move_end(); }
                    Key::CtrlK => { state.current_line_mut().kill_to_end(); }
                    Key::CtrlU => { state.current_line_mut().kill_to_start(); }
                    Key::Paste(text) => {
                        state.current_line_mut().insert_str(&text);
                    }
                    _ => {}
                }
                let sb = StatusBarState {
                    session_id,
                    approval_hint: &approval.hint(),
                    model,
                    prompt_tokens,
                    context_window,
                    daemon_up,
                    tools_total: 0,
                    cost_usd,
                    has_untracked,
                };
                let _ = renderer.draw(state.current_line(), &sb);
            }
        }
    }
}

fn banner_lines(chat_width: usize, palette: &crate::cli::palette::Palette) -> Vec<Line<'static>> {
    let logo: &[&str] = &[
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
    let subtitle = "AGENTIC OPERATOR";

    let logo_w = logo.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    let pad = " ".repeat((chat_width.saturating_sub(logo_w)) / 2);

    let blood_red = Style::default()
        .fg(palette.red())
        .add_modifier(Modifier::BOLD);
    let deep_yellow = Style::default().fg(palette.yellow());
    let title_style = Style::default()
        .fg(Color::White)
        .add_modifier(Modifier::BOLD);
    let dim = Style::default().add_modifier(Modifier::DIM);

    // Eye highlights: which string to color per line index.
    let eye_markers: &[(usize, &str)] = &[(6, "▄██▄"), (7, "███ ██"), (8, "▀████▀"), (9, "▀██▀")];

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(logo.len() + 4);

    lines.push(Line::raw(""));

    for (i, &raw) in logo.iter().enumerate() {
        let eye_opt = eye_markers
            .iter()
            .find(|&&(idx, _)| idx == i)
            .map(|&(_, s)| s);

        let spans: Vec<Span<'static>> = if let Some(eye) = eye_opt {
            // Line has an eye highlight: split into pre/eye/post.
            if let Some(byte_pos) = raw.find(eye) {
                let pre = raw[..byte_pos].to_owned();
                let post = raw[byte_pos + eye.len()..].to_owned();
                vec![
                    Span::styled(pad.clone(), Style::default()),
                    Span::styled(pre, blood_red),
                    Span::styled(eye.to_owned(), deep_yellow),
                    Span::styled(post, blood_red),
                ]
            } else {
                vec![
                    Span::styled(pad.clone(), Style::default()),
                    Span::styled(raw.to_owned(), blood_red),
                ]
            }
        } else if i >= 18 {
            // DAEMONEYE word-mark lines.
            vec![
                Span::styled(pad.clone(), Style::default()),
                Span::styled(raw.to_owned(), title_style),
            ]
        } else {
            vec![
                Span::styled(pad.clone(), Style::default()),
                Span::styled(raw.to_owned(), blood_red),
            ]
        };

        lines.push(Line::from(spans));
    }

    // Subtitle: same block offset as logo lines, then centered within logo_w.
    let sub_pad = pad.clone() + &" ".repeat(logo_w.saturating_sub(subtitle.chars().count()) / 2);
    lines.push(Line::from(vec![
        Span::raw(sub_pad),
        Span::styled(subtitle, dim),
    ]));

    // Blank line then slash-command hint.
    lines.push(Line::raw(""));

    let hint_plain = "/help  — list commands      /exit  — quit";
    let hint_pad = " ".repeat((chat_width.saturating_sub(hint_plain.chars().count())) / 2);
    lines.push(Line::from(vec![
        Span::raw(hint_pad),
        Span::styled("/help".to_owned(), Style::default().fg(Color::Cyan)),
        Span::styled("  — list commands      ".to_owned(), dim),
        Span::styled("/exit".to_owned(), Style::default().fg(Color::Cyan)),
        Span::styled("  — quit".to_owned(), dim),
    ]));

    lines.push(Line::raw(""));

    lines
}

/// Build the chat-history attribution label from identity parts.
/// `host` is domain-stripped (`pinky.home.planetfoo.org` → `pinky`).
/// A missing/`"unknown"` host degrades to the bare user; a missing user
/// degrades to the literal `you`.
fn user_host_label(user: Option<&str>, host: Option<&str>) -> String {
    let Some(user) = user.filter(|u| !u.is_empty()) else {
        return "you".to_string();
    };
    let short = host
        .map(|h| h.split('.').next().unwrap_or("").to_string())
        .unwrap_or_default();
    if short.is_empty() || short == "unknown" {
        user.to_string()
    } else {
        format!("{user}@{short}")
    }
}

/// The label for this CLI process's host — the machine `daemoneye chat`
/// runs on, which can differ from the daemon host.
fn local_user_host() -> String {
    let user = std::env::var("USER").ok();
    let host = crate::daemon::utils::daemon_hostname();
    user_host_label(user.as_deref(), Some(&host))
}

fn echo_body(query: &str) -> Vec<String> {
    query.lines().map(str::to_string).collect()
}

/// Return `true` when a query should be echoed into scrollback.
///
/// Client-only commands (`/exit`, `/help`, `/clear`, …) never reach the model
/// so they must not be echoed.  Prose that merely starts with `/` (a file path)
/// or with a keyword like `help` or `clear` *does* reach the model and is
/// echoed.
fn should_echo(query: &str) -> bool {
    !matches!(
        query,
        "/exit" | "/quit" | "exit" | "quit" | "/help" | "help" | "?" | "/?" | "/clear" | "/new"
    )
}

#[cfg(test)]
mod tests {
    use super::HELP_TEXT;

    #[test]
    fn help_text_documents_aliases_and_behaviors() {
        // Aliases
        for alias in [
            "/models",
            "/panes",
            "/sessions",
            "/approval",
            "/new",
            "/quit",
        ] {
            assert!(
                HELP_TEXT.contains(alias),
                "HELP_TEXT must contain alias '{alias}'"
            );
        }
        // Approval-prompt redirect
        assert!(
            HELP_TEXT.contains("redirect"),
            "HELP_TEXT must document redirect behavior"
        );
        // Tool-output cap
        assert!(
            HELP_TEXT.contains("10 lines"),
            "HELP_TEXT must document the 10-line tool-output cap"
        );
    }

    #[test]
    fn echo_body_splits_multiline_query() {
        use super::echo_body;

        assert_eq!(echo_body("one line"), vec!["one line"]);

        let parts = echo_body("first\nsecond\nthird");
        assert_eq!(parts, vec!["first", "second", "third"]);

        assert_eq!(echo_body("trailing\n"), vec!["trailing"]);
    }

    #[test]
    fn echo_skips_client_only_commands() {
        use super::should_echo;

        // Must NOT echo — client-only commands
        for cmd in [
            "/exit", "/quit", "exit", "quit", "/help", "help", "?", "/?", "/clear", "/new",
        ] {
            assert!(!should_echo(cmd), "must not echo: {cmd}");
        }

        // Must echo — prose queries
        for q in [
            "what is this error",
            "/etc/hosts is missing",
            "help me debug this",
            "clearly this is wrong",
        ] {
            assert!(should_echo(q), "must echo: {q}");
        }
    }

    #[test]
    fn user_host_label_joins_user_and_shorthost() {
        use super::user_host_label;
        assert_eq!(
            user_host_label(Some("matt"), Some("pinky.home.planetfoo.org")),
            "matt@pinky"
        );
    }

    #[test]
    fn user_host_label_missing_user_falls_back_to_you() {
        use super::user_host_label;
        assert_eq!(user_host_label(None, Some("pinky")), "you");
        assert_eq!(user_host_label(Some(""), Some("pinky")), "you");
    }

    #[test]
    fn user_host_label_unknown_host_degrades_to_bare_user() {
        use super::user_host_label;
        assert_eq!(user_host_label(Some("matt"), Some("unknown")), "matt");
        assert_eq!(user_host_label(Some("matt"), Some("")), "matt");
    }
}

//! The ratatui chat loop and its line reader: `run_chat_ratatui` drives a
//! chat session turn by turn, `read_input_line_inner_ratatui` reads one line.

use super::*;

// ── Ratatui chat loop ──────────────────────────────────────────────
pub(super) async fn run_chat_ratatui(ctx: RatatuiCtx<'_>) -> Result<()> {
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
        parked: parked_presence,
    } = ctx;
    // The parked connection is kept whole for the loop's lifetime: the hold
    // ends when the loop drops it (stamping detach state), and the push
    // channel feeds the input loop. Splitting hold and receiver into separate
    // bindings let a by-value move discard the hold and kill the task before
    // the first prompt — bug-03b-1. A failed attach parks a detached
    // placeholder, so the loop still selects on a channel nobody sends to.
    let mut held_presence = super::chat_presence::chat_loop_setup(parked_presence);
    let pushes = held_presence.pushes();
    let mut last_ctrl_c: Option<std::time::Instant> = None;
    let mut daemon_up = false;
    let mut prompt_tokens: u32 = 0;
    let mut turn: usize = 0;
    let mut cost_usd: f64 = 0.0;
    let mut has_untracked: bool = false;
    let mut transcript = crate::cli::transcript::Transcript::new();
    let config = Config::load().unwrap_or_default();
    let mut context_window = config.resolve_model(None).context_window();
    let mut model = config.resolve_model(None).model.clone();

    // Wait for tmux client attachment.
    // Wait for tmux client attachment. Only meaningful inside tmux — outside
    // there is no client to attach, and waiting would hang chat forever.
    if tmux_session.is_some() {
        loop {
            let attached =
                crate::process::bounded_output(std::process::Command::new("tmux").args([
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
                transcript: &mut transcript,
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
            transcript: &transcript,
            pushes,
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
        if let Some(host) = shell_command(&query) {
            let opened = super::ipc_client::request_once(crate::ipc::Request::ShellOpen {
                host: host.map(str::to_string),
            })
            .await;
            match opened {
                Ok(crate::ipc::Response::ShellOpened { id }) => {
                    if let Err(e) =
                        crate::cli::attach::run_attach(stdin, sigwinch, &mut renderer, &id).await
                    {
                        let _ = renderer.commit(&format!("  ✗ attach {id}: {e}\n"));
                    }
                }
                Ok(crate::ipc::Response::Error(msg)) => {
                    let _ = renderer.commit(&format!("  ✗ shell: {msg}\n"));
                }
                Ok(other) => {
                    let _ =
                        renderer.commit(&format!("  ✗ shell: unexpected reply {}\n", other.kind()));
                }
                Err(e) => {
                    let _ = renderer.commit(&format!("  ✗ shell: {e}\n"));
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
                cost_usd,
                has_untracked,
            );
            continue;
        }
        if let Some(id) = attach_target(&query) {
            match id {
                Some(id) => {
                    if let Err(e) =
                        crate::cli::attach::run_attach(stdin, sigwinch, &mut renderer, id).await
                    {
                        let _ = renderer.commit(&format!("  ✗ attach {id}: {e}\n"));
                    }
                }
                None => {
                    let hint = approval.hint();
                    let status = StatusBarState {
                        session_id: &session_id,
                        approval_hint: &hint,
                        model: &model,
                        prompt_tokens,
                        context_window,
                        daemon_up,
                        tools_total: 0,
                        cost_usd,
                        has_untracked,
                    };
                    if let Err(e) =
                        super::agents::run_agents(stdin, sigwinch, &mut renderer, &status).await
                    {
                        let _ = renderer.commit(&format!("  ✗ agents: {e}\n"));
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
                cost_usd,
                has_untracked,
            );
            continue;
        }
        if let Some(id) = log_command(&query) {
            match id {
                Some(id) => {
                    if let Err(e) =
                        crate::cli::tail::run_log_viewer(stdin, sigwinch, &mut renderer, id).await
                    {
                        let _ = renderer.commit(&format!("  ✗ log {id}: {e}\n"));
                    }
                }
                None => {
                    let _ = renderer.commit("  usage: /log <log id>  (see /logs)\n");
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
                cost_usd,
                has_untracked,
            );
            continue;
        }
        if query == "/tail"
            || query == "/watch"
            || query.starts_with("/tail ")
            || query.starts_with("/watch ")
        {
            match query.split_whitespace().nth(1) {
                Some(id) => {
                    if let Err(e) =
                        crate::cli::tail::run_tail_viewer(stdin, sigwinch, &mut renderer, id).await
                    {
                        let _ = renderer.commit(&format!("  ✗ tail {id}: {e}\n"));
                    }
                }
                None => {
                    let _ = renderer.commit("  usage: /tail <shell id>  (see /shells)\n");
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
                    transcript: &mut transcript,
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
        turn += 1;
        if should_echo(&query) {
            let echo_body: Vec<String> = echo_body(&query);
            let label = turn_budget_label(turn, prompt_tokens, context_window);
            let _ = renderer.commit_panel_labeled(&user_host, &echo_body, false, Some(&label));
            transcript.push(crate::cli::transcript::Block::UserTurn {
                label: user_host.clone(),
                text: query.clone(),
            });
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
                    transcript: &mut transcript,
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
    println!("{}", super::resume_hint(&session_id));
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
    transcript: &'a crate::cli::transcript::Transcript,
    /// Pushes from the parked presence connection (M27 phase 03b). Rendered
    /// here — not in the background task — because the renderer owns the
    /// screen and only this loop may write to it.
    pushes: &'a mut tokio::sync::mpsc::UnboundedReceiver<String>,
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
        transcript,
        pushes,
    } = ctx;
    loop {
        tokio::select! {
            Some(msg) = pushes.recv() => {
                // An alert pushed over the presence connection. Rendered here,
                // by the loop that owns the renderer — never from the
                // background task (a stray stdout write would corrupt the
                // ratatui screen).
                let _ = renderer.commit(&format!("\n⚙ {msg}\n"));
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
            _ = sigwinch.recv() => {
                *chat_width = terminal_width();
                renderer.reanchor();
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
                        let content_width = renderer.input_content_width();
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
                        let content_width = renderer.input_content_width();
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
                    Key::CtrlO => {
                        if let Err(e) = crate::cli::viewer::run_transcript_viewer(
                            stdin,
                            sigwinch,
                            renderer,
                            transcript,
                        )
                        .await
                        {
                            eprintln!("\x1b[31m✗\x1b[0m viewer: {}", e);
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

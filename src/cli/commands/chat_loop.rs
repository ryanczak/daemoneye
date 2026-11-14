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
        turn_seed,
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
    let mut transcript = crate::cli::transcript::Transcript::new();
    let config = Config::load().unwrap_or_default();
    let mut context_window = config.resolve_model(None).context_window();
    let mut model = config.resolve_model(None).model.clone();
    // The status bar's one model (M37 02a): everything the bar reads lives
    // here and every draw goes through it.
    let glyphs = crate::cli::status_bar::glyph_set_for_locale(
        config
            .chat
            .glyph_set()
            .unwrap_or(&crate::cli::status_glyphs::EMOJI),
        crate::cli::status_bar::locale_is_utf8(
            std::env::var("LANG").ok().as_deref(),
            std::env::var("LC_ALL").ok().as_deref(),
            std::env::var("LC_CTYPE").ok().as_deref(),
        ),
    );
    let mut sb = crate::cli::status_bar::StatusModel {
        session_id: session_id.clone(),
        model: model.clone(),
        context_window,
        turn: turn_seed,
        glyphs,
        ..Default::default()
    };
    // Text typed during the last turn and submitted with Enter (M32 #18):
    // consumed at the top of the loop as the next query.
    let mut queued_message: Option<String> = None;

    // The OSC 7501 emitter (M36 21c): stdout, gated on being a terminal and
    // `$TMUX`. Created once for the loop; its clear rides the renderer's
    // terminal-restore guard, and inside tmux the pane option is unset there.
    let emit_status = crate::cli::program_status_emit::Emitter::for_stdout();

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

    // Helper: re-draw the live region from the one status model.
    let redraw = |r: &mut RatatuiRendererStdout,
                  is: &InputState,
                  ap: &SessionApproval,
                  sb: &mut crate::cli::status_bar::StatusModel| {
        sb.approval_hint = ap.hint();
        let _ = r.draw(is.current_line(), sb);
    };

    redraw(&mut renderer, input_state, approval, &mut sb);

    let _ = renderer.commit_styled(&banner_lines(
        chat_width,
        &crate::cli::palette::Palette::from_env(),
    ));

    // Send the greeting query — minimal rendering via ratatui commit path.
    {
        let cw = chat_width;
        // Keys typed during the greeting settle exactly as a typed turn's do
        // (M32 #18): Enter submits the kept text as the first query, no
        // second Enter; otherwise the first input line starts pre-filled.
        let mut typeahead = Typeahead::default();
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
            RatatuiQueryCtx {
                chat_width: Some(cw),
                renderer: &mut renderer,
                stdin,
                transcript: &mut transcript,
                typeahead: &mut typeahead,
                status: &emit_status,
                sb: &mut sb,
            },
        )
        .await
        {
            Ok(()) => {
                sb.set_daemon_up(true);
            }
            Err(e) => {
                sb.set_daemon_up(false);
                eprintln!("\x1b[31m✗\x1b[0m Could not reach the daemon: {}", e);
                eprintln!("  Make sure it is running:  \x1b[1mdaemoneye daemon --console\x1b[0m");
                eprintln!("  \x1b[2mWaiting for your input…\x1b[0m");
            }
        }
        settle_typeahead(&typeahead, &mut queued_message, input_state);
    }

    redraw(&mut renderer, input_state, approval, &mut sb);

    // Help text for /help command.
    let help_text = HELP_TEXT;

    let user_host = local_user_host();

    loop {
        // A queued message (typed mid-turn and submitted with Enter, M32 #18)
        // becomes the next query without reading input or a second Enter.
        let query = if let Some(q) = queued_message.take() {
            q
        } else {
            // Read input using the existing key handler but render via ratatui.
            let line_opt = read_input_line_inner_ratatui(RatatuiInputCtx {
                state: input_state,
                stdin,
                sigwinch,
                renderer: &mut renderer,
                chat_width: &mut chat_width,
                approval,
                sb: &mut sb,
                last_ctrl_c: &mut last_ctrl_c,
                transcript: &transcript,
                pushes,
            })
            .await?;

            let Some(line) = line_opt else { break };

            let q = line.trim().to_string();
            if q.is_empty() {
                continue;
            }
            q
        };

        input_state.push_history(query.clone());

        if query == "/exit" || query == "/quit" || query == "exit" || query == "quit" {
            break;
        }
        if query == "/help" || query == "help" || query == "?" || query == "/?" {
            let _ = renderer.commit(help_text);
            let _ = renderer.commit("\n");
            redraw(&mut renderer, input_state, approval, &mut sb);
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
            redraw(&mut renderer, input_state, approval, &mut sb);
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
                    sb.approval_hint = approval.hint();
                    if let Err(e) =
                        super::agents::run_agents(stdin, sigwinch, &mut renderer, &sb).await
                    {
                        let _ = renderer.commit(&format!("  ✗ agents: {e}\n"));
                    }
                }
            }
            redraw(&mut renderer, input_state, approval, &mut sb);
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
            redraw(&mut renderer, input_state, approval, &mut sb);
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
            redraw(&mut renderer, input_state, approval, &mut sb);
            continue;
        }
        if query == "/clear" || query == "/new" {
            // Start a fresh conversation: new session id, reset counters.
            session_id = uuid::Uuid::new_v4().to_string();
            sb.prompt_tokens = 0;
            sb.cost_usd = 0.0;
            sb.has_untracked = false;
            sb.previous_prompt_tokens = None;
            sb.session_name = None;
            sb.session_dirty = false;
            let _ = renderer.commit("  ✓ started a fresh session\n");
            redraw(&mut renderer, input_state, approval, &mut sb);
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
                    sb: &mut sb,
                },
                &query,
            )
            .await;
            match outcome {
                slash::SlashOutcome::Handled => {
                    redraw(&mut renderer, input_state, approval, &mut sb);
                    continue;
                }
                slash::SlashOutcome::NotACommand => {
                    let first = query.split_whitespace().next().unwrap_or("");
                    if slash::is_command_shaped(first) {
                        let _ =
                            renderer.commit(&format!("  ✗ unknown command: {first} (try /help)\n"));
                        redraw(&mut renderer, input_state, approval, &mut sb);
                        continue;
                    }
                    // Prose starting with '/': fall through to the LLM.
                }
            }
        }

        // Echo the user's words into scrollback so the transcript reads as a
        // conversation. Same element as tool output — one visual grammar for
        // everything committed above the live region.
        sb.turn += 1;
        sb.begin_turn(std::time::Instant::now());
        if should_echo(&query) {
            let echo_body: Vec<String> = echo_body(&query);
            let label = turn_budget_label(sb.turn, sb.prompt_tokens, context_window);
            let _ = renderer.commit_panel_labeled(&user_host, &echo_body, false, Some(&label));
            transcript.push(crate::cli::transcript::Block::UserTurn {
                label: user_host.clone(),
                text: query.clone(),
            });
        }

        // ── Send the user query ─────────────────────────────────────────
        {
            let cw = chat_width;
            let mut typeahead = Typeahead::default();
            let result = ask_with_session_ratatui(
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
                RatatuiQueryCtx {
                    chat_width: Some(cw),
                    renderer: &mut renderer,
                    stdin,
                    transcript: &mut transcript,
                    typeahead: &mut typeahead,
                    status: &emit_status,
                    sb: &mut sb,
                },
            )
            .await;
            let reached_daemon = match result {
                Ok(()) => true,
                Err(e) => {
                    eprintln!("\x1b[31m✗\x1b[0m {}", e);
                    false
                }
            };

            settle_typeahead(&typeahead, &mut queued_message, input_state);
            // Turn finished: idle (or daemon-down when the ask failed), and
            // the last turn's duration feeds throughput.
            sb.end_turn(std::time::Instant::now(), reached_daemon);

            redraw(&mut renderer, input_state, approval, &mut sb);
        }
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
    approval: &'a SessionApproval,
    sb: &'a mut crate::cli::status_bar::StatusModel,
    last_ctrl_c: &'a mut Option<std::time::Instant>,
    transcript: &'a crate::cli::transcript::Transcript,
    /// Pushes from the parked presence connection (M27 phase 03b). Rendered
    /// here — not in the background task — because the renderer owns the
    /// screen and only this loop may write to it.
    pushes: &'a mut super::chat_presence::PresenceReceiver,
}

async fn read_input_line_inner_ratatui(ctx: RatatuiInputCtx<'_>) -> anyhow::Result<Option<String>> {
    let RatatuiInputCtx {
        state,
        stdin,
        sigwinch,
        renderer,
        chat_width,
        approval,
        sb,
        last_ctrl_c,
        transcript,
        pushes,
    } = ctx;
    loop {
        tokio::select! {
            Some(push) = pushes.recv() => {
                // Pushes from the parked presence connection. Rendered here,
                // by the loop that owns the renderer — never from the
                // background task (a stray stdout write would corrupt the
                // ratatui screen).
                match push {
                    super::chat_presence::PresencePush::Text(msg) => {
                        let _ = renderer.commit(&format!("\n⚙ {msg}\n"));
                        sb.presence_text(&msg);
                    }
                    super::chat_presence::PresencePush::Alerts(alerts) => {
                        sb.set_alerts(alerts, std::time::Instant::now());
                    }
                    super::chat_presence::PresencePush::Counters {
                        background,
                        ghosts,
                        shells,
                    } => sb.set_session_counters(background, ghosts, shells),
                }
                sb.approval_hint = approval.hint();
                let _ = renderer.draw(state.current_line(), sb);
            }
            _ = sigwinch.recv() => {
                *chat_width = terminal_width();
                renderer.reanchor();
                sb.approval_hint = approval.hint();
                let _ = renderer.draw(state.current_line(), sb);
            }
            _ = tokio::time::sleep_until(
                tokio::time::Instant::now() + std::time::Duration::from_secs(1),
                ), if sb.is_time_based(std::time::Instant::now()) => {
                // The bar carries something time-based (a running turn, a
                // firing alert, a live resolve flash): redraw once a second.
                // The arm's condition turns the timer off otherwise.
                sb.approval_hint = approval.hint();
                let _ = renderer.draw(state.current_line(), sb);
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
                        sb.approval_hint = approval.hint();
                        let _ = renderer.draw(state.current_line(), sb);
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
                        sb.approval_hint = approval.hint();
                        let _ = renderer.draw(state.current_line(), sb);
                    }
                    _ => {}
                }
                sb.approval_hint = approval.hint();
                let _ = renderer.draw(state.current_line(), sb);
            }
        }
    }
}

/// The turn number a resumed chat's counter starts from: the daemon's count
/// when its reply carries one, 0 otherwise — a failed or foreign reply is
/// today's behaviour, and a label must not stop the chat.
pub(super) fn resumed_turn(reply: anyhow::Result<crate::ipc::Response>) -> usize {
    match reply {
        Ok(crate::ipc::Response::LimitsInfo { turn_count, .. }) => turn_count,
        _ => 0,
    }
}

/// Settle a finished turn's typeahead (M32 #18): text the user kept while the
/// turn streamed is either submitted with Enter — it becomes the next query,
/// no second Enter — or pre-fills the input line, cursor at the end. Empty
/// kept text does nothing.
fn settle_typeahead(typeahead: &Typeahead, queued: &mut Option<String>, input: &mut InputState) {
    if typeahead.text().is_empty() {
        return;
    }
    if typeahead.enter_pressed() {
        *queued = Some(typeahead.text().to_string());
    } else {
        input.current_line_mut().insert_str(typeahead.text());
    }
}

#[cfg(test)]
mod tests {
    use super::{resumed_turn, settle_typeahead};
    use crate::cli::commands::stream::Typeahead;
    use crate::cli::input::{InputState, Key};
    use crate::ipc::{LimitsSummary, Response};

    #[test]
    fn a_resumed_chat_seeds_its_turn_from_the_daemon() {
        let limits = LimitsSummary::default();
        let ok = Ok(Response::LimitsInfo {
            limits,
            turn_count: 7,
            tool_calls_this_session: 0,
            history_len: 0,
        });
        assert_eq!(resumed_turn(ok), 7);
        assert_eq!(resumed_turn(Err(anyhow::anyhow!("connect failed"))), 0);
        assert_eq!(resumed_turn(Ok(Response::Error("no".to_string()))), 0);
    }

    #[test]
    fn settle_typeahead_queues_on_enter_and_prefills_otherwise() {
        // Enter mid-turn: the kept text becomes the next query, the input
        // line stays empty.
        let mut ta = Typeahead::default();
        for k in [Key::Char('h'), Key::Char('i'), Key::Enter] {
            ta.feed(&k);
        }
        let mut queued = None;
        let mut input = InputState::new();
        settle_typeahead(&ta, &mut queued, &mut input);
        assert_eq!(queued.as_deref(), Some("hi"), "Enter submits the kept text");
        assert!(
            input.current_line().as_str().is_empty(),
            "a queued message does not also pre-fill the line"
        );

        // No Enter: nothing is queued and the input line starts pre-filled.
        let mut ta = Typeahead::default();
        for k in [
            Key::Char('d'),
            Key::Char('r'),
            Key::Char('a'),
            Key::Char('f'),
            Key::Char('t'),
        ] {
            ta.feed(&k);
        }
        let mut queued = None;
        let mut input = InputState::new();
        settle_typeahead(&ta, &mut queued, &mut input);
        assert_eq!(queued, None, "without Enter nothing is queued");
        assert_eq!(
            input.current_line().as_str(),
            "draft",
            "without Enter the line is pre-filled"
        );

        // Empty kept text changes nothing either way.
        let mut ta = Typeahead::default();
        ta.feed(&Key::Backspace);
        let mut queued = Some("already".to_string());
        let mut input = InputState::new();
        input.current_line_mut().insert_str("kept");
        settle_typeahead(&ta, &mut queued, &mut input);
        assert_eq!(
            queued.as_deref(),
            Some("already"),
            "an empty typeahead leaves the queue alone"
        );
        assert_eq!(
            input.current_line().as_str(),
            "kept",
            "an empty typeahead leaves the line alone"
        );
    }
}

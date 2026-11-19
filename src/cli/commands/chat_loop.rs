//! The ratatui chat loop and its line reader: `run_chat_ratatui` drives a
//! chat session turn by turn, `read_input_line_inner_ratatui` reads one line.

use super::*;

fn redraw_chat(
    renderer: &mut RatatuiRendererStdout,
    input_state: &InputState,
    approval: &SessionApproval,
    sb: &mut crate::cli::status_bar::StatusModel,
) {
    sb.grants = approval.summary();
    let _ = renderer.draw(input_state.current_line(), sb);
}

async fn wait_for_tmux_attachment(tmux_session: Option<&str>) {
    if tmux_session.is_none() {
        return;
    }
    loop {
        let attached = crate::process::bounded_output(std::process::Command::new("tmux").args([
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

enum QueryDispatch {
    Exit,
    Handled,
    SendToModel,
}

async fn handle_shell_open_slash(
    host: Option<&str>,
    stdin: &AsyncStdin,
    sigwinch: &mut tokio::signal::unix::Signal,
    renderer: &mut RatatuiRendererStdout,
) {
    let opened = super::ipc_client::request_once(crate::ipc::Request::ShellOpen {
        host: host.map(str::to_string),
    })
    .await;
    match opened {
        Ok(crate::ipc::Response::ShellOpened { id }) => {
            if let Err(e) = crate::cli::attach::run_attach(stdin, sigwinch, renderer, &id).await {
                let _ = renderer.commit(&format!("  ✗ attach {id}: {e}\n"));
            }
        }
        Ok(crate::ipc::Response::Error(msg)) => {
            let _ = renderer.commit(&format!("  ✗ shell: {msg}\n"));
        }
        Ok(other) => {
            let _ = renderer.commit(&format!("  ✗ shell: unexpected reply {}\n", other.kind()));
        }
        Err(e) => {
            let _ = renderer.commit(&format!("  ✗ shell: {e}\n"));
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_local_or_slash(
    query: &str,
    stdin: &AsyncStdin,
    sigwinch: &mut tokio::signal::unix::Signal,
    renderer: &mut RatatuiRendererStdout,
    session_id: &mut String,
    approval: &mut SessionApproval,
    model: &mut String,
    context_window: &mut u32,
    current_prompt: &mut Option<String>,
    target_pane: &mut Option<String>,
    transcript: &mut crate::cli::transcript::Transcript,
    sb: &mut crate::cli::status_bar::StatusModel,
) -> QueryDispatch {
    if query == "/exit" || query == "/quit" || query == "exit" || query == "quit" {
        return QueryDispatch::Exit;
    }
    if query == "/help" || query == "help" || query == "?" || query == "/?" {
        let _ = renderer.commit(HELP_TEXT);
        let _ = renderer.commit("\n");
        return QueryDispatch::Handled;
    }
    if let Some(host) = shell_command(query) {
        handle_shell_open_slash(host, stdin, sigwinch, renderer).await;
        return QueryDispatch::Handled;
    }
    if let Some(id) = attach_target(query) {
        match id {
            Some(id) => {
                if let Err(e) = crate::cli::attach::run_attach(stdin, sigwinch, renderer, id).await
                {
                    let _ = renderer.commit(&format!("  ✗ attach {id}: {e}\n"));
                }
            }
            None => {
                sb.grants = approval.summary();
                if let Err(e) = super::agents::run_agents(stdin, sigwinch, renderer, sb).await {
                    let _ = renderer.commit(&format!("  ✗ agents: {e}\n"));
                }
            }
        }
        return QueryDispatch::Handled;
    }
    if let Some(id) = log_command(query) {
        match id {
            Some(id) => {
                if let Err(e) =
                    crate::cli::tail::run_log_viewer(stdin, sigwinch, renderer, id).await
                {
                    let _ = renderer.commit(&format!("  ✗ log {id}: {e}\n"));
                }
            }
            None => {
                let _ = renderer.commit("  usage: /log <log id>  (see /logs)\n");
            }
        }
        return QueryDispatch::Handled;
    }
    if query == "/tail"
        || query == "/watch"
        || query.starts_with("/tail ")
        || query.starts_with("/watch ")
    {
        match query.split_whitespace().nth(1) {
            Some(id) => {
                if let Err(e) =
                    crate::cli::tail::run_tail_viewer(stdin, sigwinch, renderer, id).await
                {
                    let _ = renderer.commit(&format!("  ✗ tail {id}: {e}\n"));
                }
            }
            None => {
                let _ = renderer.commit("  usage: /tail <shell id>  (see /shells)\n");
            }
        }
        return QueryDispatch::Handled;
    }
    if query == "/clear" || query == "/new" {
        *session_id = uuid::Uuid::new_v4().to_string();
        sb.prompt_tokens = 0;
        sb.cost_usd = 0.0;
        sb.has_untracked = false;
        sb.previous_prompt_tokens = None;
        sb.session_name = None;
        sb.session_dirty = false;
        let _ = renderer.commit("  ✓ started a fresh session\n");
        return QueryDispatch::Handled;
    }

    if query.starts_with('/') {
        let outcome = slash::handle_slash(
            slash::SlashCtx {
                renderer,
                session_id,
                approval,
                model,
                context_window,
                current_prompt,
                target_pane,
                transcript,
                sb,
            },
            query,
        )
        .await;
        match outcome {
            slash::SlashOutcome::Handled => return QueryDispatch::Handled,
            slash::SlashOutcome::NotACommand => {
                let first = query.split_whitespace().next().unwrap_or("");
                if slash::is_command_shaped(first) {
                    let _ = renderer.commit(&format!("  ✗ unknown command: {first} (try /help)\n"));
                    return QueryDispatch::Handled;
                }
            }
        }
    }
    QueryDispatch::SendToModel
}

fn init_chat_status_model(
    config: &Config,
    session_id: &str,
    turn_seed: usize,
) -> (String, u32, crate::cli::status_bar::StatusModel) {
    let resolved = config.resolve_model(None);
    let context_window = resolved.context_window();
    let model = resolved.model.clone();
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
    let sb = crate::cli::status_bar::StatusModel {
        session_id: session_id.to_string(),
        model: model.clone(),
        context_window,
        turn: turn_seed,
        glyphs,
        ..Default::default()
    };
    (model, context_window, sb)
}

#[allow(clippy::too_many_arguments)]
async fn run_greeting_turn(
    session_id: &str,
    current_prompt: Option<&str>,
    approval: &mut SessionApproval,
    tmux_session: Option<&str>,
    target_pane: Option<&str>,
    chat_width: usize,
    renderer: &mut RatatuiRendererStdout,
    stdin: &AsyncStdin,
    transcript: &mut crate::cli::transcript::Transcript,
    emit_status: &crate::cli::program_status_emit::Emitter,
    sb: &mut crate::cli::status_bar::StatusModel,
    queued_message: &mut Option<String>,
    input_state: &mut InputState,
) {
    let mut typeahead = Typeahead::default();
    match ask_with_session_ratatui(
        QueryArgs {
            query: "Hello!".to_string(),
            prompt_override: current_prompt,
        },
        Some(session_id),
        approval,
        AskTmuxCtx {
            session: tmux_session,
            pane: target_pane,
        },
        RatatuiQueryCtx {
            chat_width: Some(chat_width),
            renderer,
            stdin,
            transcript,
            typeahead: &mut typeahead,
            status: emit_status,
            sb,
        },
    )
    .await
    {
        Ok(()) => sb.set_daemon_up(true),
        Err(e) => {
            sb.set_daemon_up(false);
            eprintln!("\x1b[31m✗\x1b[0m Could not reach the daemon: {}", e);
            eprintln!("  Make sure it is running:  \x1b[1mdaemoneye daemon --console\x1b[0m");
            eprintln!("  \x1b[2mWaiting for your input…\x1b[0m");
        }
    }
    settle_typeahead(&typeahead, queued_message, input_state);
}

fn echo_user_turn(
    query: &str,
    user_host: &str,
    context_window: u32,
    renderer: &mut RatatuiRendererStdout,
    transcript: &mut crate::cli::transcript::Transcript,
    sb: &mut crate::cli::status_bar::StatusModel,
) {
    sb.turn += 1;
    sb.begin_turn(std::time::Instant::now());
    if should_echo(query) {
        let echo_body: Vec<String> = echo_body(query);
        let label = turn_budget_label(sb.turn, sb.prompt_tokens, context_window);
        let _ = renderer.commit_panel_labeled(user_host, &echo_body, false, Some(&label));
        transcript.push(crate::cli::transcript::Block::UserTurn {
            label: user_host.to_string(),
            text: query.to_string(),
        });
    }
}

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
    let mut held_presence = super::chat_presence::chat_loop_setup(parked_presence);
    let pushes = held_presence.pushes();
    let mut last_ctrl_c: Option<std::time::Instant> = None;
    let mut transcript = crate::cli::transcript::Transcript::new();
    let config = Config::load().unwrap_or_default();
    let (mut model, mut context_window, mut sb) =
        init_chat_status_model(&config, &session_id, turn_seed);
    let mut queued_message: Option<String> = None;
    let emit_status = crate::cli::program_status_emit::Emitter::for_stdout();

    wait_for_tmux_attachment(tmux_session.as_deref()).await;
    redraw_chat(&mut renderer, input_state, approval, &mut sb);
    let _ = renderer.commit_styled(&banner_lines(
        chat_width,
        &crate::cli::palette::Palette::from_env(),
    ));
    run_greeting_turn(
        &session_id,
        current_prompt.as_deref(),
        approval,
        tmux_session.as_deref(),
        target_pane.as_deref(),
        chat_width,
        &mut renderer,
        stdin,
        &mut transcript,
        &emit_status,
        &mut sb,
        &mut queued_message,
        input_state,
    )
    .await;
    redraw_chat(&mut renderer, input_state, approval, &mut sb);
    let user_host = local_user_host();
    loop {
        let query = if let Some(q) = queued_message.take() {
            q
        } else {
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
        match dispatch_local_or_slash(
            &query,
            stdin,
            sigwinch,
            &mut renderer,
            &mut session_id,
            approval,
            &mut model,
            &mut context_window,
            &mut current_prompt,
            &mut target_pane,
            &mut transcript,
            &mut sb,
        )
        .await
        {
            QueryDispatch::Exit => break,
            QueryDispatch::Handled => {
                redraw_chat(&mut renderer, input_state, approval, &mut sb);
                continue;
            }
            QueryDispatch::SendToModel => {}
        }
        echo_user_turn(
            &query,
            &user_host,
            context_window,
            &mut renderer,
            &mut transcript,
            &mut sb,
        );
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
                chat_width: Some(chat_width),
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
        sb.end_turn(std::time::Instant::now(), reached_daemon);
        redraw_chat(&mut renderer, input_state, approval, &mut sb);
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
    pushes: &'a mut super::chat_presence::PresenceReceiver,
}

#[allow(clippy::too_many_arguments)]
async fn handle_input_key(
    key: Key,
    state: &mut InputState,
    stdin: &AsyncStdin,
    sigwinch: &mut tokio::signal::unix::Signal,
    renderer: &mut RatatuiRendererStdout,
    approval: &SessionApproval,
    sb: &mut crate::cli::status_bar::StatusModel,
    last_ctrl_c: &mut Option<std::time::Instant>,
    transcript: &crate::cli::transcript::Transcript,
) -> Option<Option<String>> {
    match key {
        Key::Enter => return Some(Some(state.current_line().as_str())),
        Key::CtrlJ => state.current_line_mut().insert_newline(),
        Key::CtrlD => {
            if state.current_line().as_str().is_empty() {
                return Some(None);
            }
            state.current_line_mut().delete();
        }
        Key::CtrlC => {
            if let Some(t) = last_ctrl_c.as_ref()
                && t.elapsed() < std::time::Duration::from_millis(1000)
            {
                return Some(None);
            }
            *last_ctrl_c = Some(std::time::Instant::now());
            *state.current_line_mut() = InputLine::new();
            state.clear_history_nav();
        }
        Key::Char(c) if c != '\0' => state.current_line_mut().insert(c),
        Key::Backspace => state.current_line_mut().backspace(),
        Key::Delete => state.current_line_mut().delete(),
        Key::Left => state.current_line_mut().move_left(),
        Key::Right => state.current_line_mut().move_right(),
        Key::Up => {
            let content_width = renderer.input_content_width();
            if state
                .current_line()
                .cursor_on_first_visual_row(content_width)
            {
                if state.has_history() {
                    state.history_up();
                }
            } else {
                state.current_line_mut().move_up(content_width);
            }
        }
        Key::Down => {
            let content_width = renderer.input_content_width();
            if state
                .current_line()
                .cursor_on_last_visual_row(content_width)
            {
                if state.has_history() {
                    state.history_down();
                }
            } else {
                state.current_line_mut().move_down(content_width);
            }
        }
        Key::FocusGained => {
            renderer.reanchor();
        }
        Key::FocusLost => {}
        Key::Home | Key::CtrlA => state.current_line_mut().move_home(),
        Key::End | Key::CtrlE => state.current_line_mut().move_end(),
        Key::CtrlK => state.current_line_mut().kill_to_end(),
        Key::CtrlU => state.current_line_mut().kill_to_start(),
        Key::Paste(text) => state.current_line_mut().insert_str(&text),
        Key::CtrlO => {
            if let Err(e) =
                crate::cli::viewer::run_transcript_viewer(stdin, sigwinch, renderer, transcript)
                    .await
            {
                eprintln!("\x1b[31m✗\x1b[0m viewer: {}", e);
            }
        }
        _ => {}
    }
    redraw_chat(renderer, state, approval, sb);
    None
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
                redraw_chat(renderer, state, approval, sb);
            }
            _ = sigwinch.recv() => {
                *chat_width = terminal_width();
                renderer.reanchor();
                redraw_chat(renderer, state, approval, sb);
            }
            _ = tokio::time::sleep_until(
                tokio::time::Instant::now() + std::time::Duration::from_secs(1),
            ), if sb.is_time_based(std::time::Instant::now()) => {
                redraw_chat(renderer, state, approval, sb);
            }
            key = read_key(stdin) => {
                let Some(key) = key else {
                    return Ok(None);
                };
                if let Some(done) = handle_input_key(
                    key,
                    state,
                    stdin,
                    sigwinch,
                    renderer,
                    approval,
                    sb,
                    last_ctrl_c,
                    transcript,
                )
                .await
                {
                    return Ok(done);
                }
            }
        }
    }
}

/// The turn number a resumed chat's counter starts from: the daemon's count
/// when its reply carries one, 0 otherwise.
pub(super) fn resumed_turn(reply: anyhow::Result<crate::ipc::Response>) -> usize {
    match reply {
        Ok(crate::ipc::Response::LimitsInfo { turn_count, .. }) => turn_count,
        _ => 0,
    }
}

/// Settle a finished turn's typeahead (M32 #18): text the user kept while the
/// turn streamed is either submitted with Enter or pre-fills the input line.
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

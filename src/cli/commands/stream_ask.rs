//! `ask_with_session_ratatui`: one chat turn streamed through the ratatui
//! renderer, from the request to the final token.

use super::*;

/// The status bar each prompt in a turn draws over: the session's model,
/// approval hint, token count and cost. The daemon is shown as down and no
/// session id is named while a prompt holds the live region.
fn prompt_status<'a>(
    hint: &'a str,
    model: &'a str,
    prompt_tokens: u32,
    context_window: u32,
    cost_usd: f64,
    has_untracked: bool,
) -> crate::cli::render::StatusBarState<'a> {
    crate::cli::render::StatusBarState {
        session_id: "",
        approval_hint: hint,
        model,
        prompt_tokens,
        context_window,
        daemon_up: false,
        tools_total: 0,
        cost_usd,
        has_untracked,
    }
}

/// Stream the AI response through the ratatui renderer.
///
/// Before the first token, animates a spinner in the inline viewport.
/// Tokens are fed through markdown rendering and committed line-by-line
/// to scrollback as styled spans.  Tool calls are auto-denied.
pub(in super::super) async fn ask_with_session_ratatui(
    qa: QueryArgs<'_>,
    session_id: Option<&str>,
    approval: &mut SessionApproval,
    tmux: AskTmuxCtx<'_>,
    tok: TokenCtx<'_>,
    ctx: RatatuiQueryCtx<'_>,
) -> Result<()> {
    let QueryArgs {
        query,
        prompt_override,
        ..
    } = qa;
    let AskTmuxCtx {
        session: tmux_session,
        pane: target_pane,
    } = tmux;
    let TokenCtx {
        prompt_tokens,
        context_window,
    } = tok;
    let RatatuiQueryCtx {
        chat_width,
        session_cost,
        session_has_untracked,
        renderer,
        model,
        stdin,
        transcript,
    } = ctx;

    let stream = connect().await?;
    let (rx, mut tx) = stream.into_split();
    let mut rx = BufReader::new(rx);

    let chat_pane = std::env::var("TMUX_PANE").ok();
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
            model: None,
        },
    )
    .await?;

    // ── Spinner animation (Phase 1 — waiting for first content) ──
    const SPINNER: &[&str] = &["(─)", "(○)", "(◎)", "(◉)", "(◎)", "(○)"];
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
    let verb_offset = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as usize)
        % VERBS.len();
    let mut spin = verb_offset * TICKS_PER_VERB;
    let mut response_started = false;

    // Buffer for deferred tool panel (silent tools).
    let mut pending_tool: Option<(String, Vec<String>)> = None;

    // Interrupt state machine for this turn.
    let mut interrupt_state = InterruptState::new();

    // Markdown renderer for streaming — feeds tokens and produces styled lines.
    let mut md = MarkdownRenderer::new();
    let render_width = chat_width.map(|w| w - 2).unwrap_or(80).max(20);

    // Accumulator for a partially-read daemon line. Owned by the caller (not by
    // the read future), so an interrupted/timed-out read leaves its bytes here
    // and the next read completes the message — no daemon message is ever lost
    // when a keypress or spinner tick interrupts an in-flight read
    // (the bug-phase-11-1 / bug-phase-11-2 failure mode).
    let mut line_buf: Vec<u8> = Vec::new();

    // Timestamp of the most recent daemon message. In phase 2 the 120 s overall
    // timeout is measured from this instant (reset on every message, including
    // KeepAlive) rather than per-loop-iteration, so the spinner can keep ticking
    // between tokens without defeating the timeout.
    let mut last_msg_at = std::time::Instant::now();

    let mut sigwinch =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change())?;

    loop {
        // Both phases animate a spinner on an 80 ms tick so a mid-stream pause
        // (e.g. a tool round-trip or a slow model) never looks frozen. Both
        // phases carry a deadline measured from `last_msg_at`: phase 1 bounds
        // the pre-content silence at `PHASE1_SILENCE_TIMEOUT_SECS` (90 s — the
        // daemon signals liveness every 15 s, so 90 s of silence means it is
        // hung, not slow), phase 2 keeps the pre-existing 120 s deadline.
        let (tick_interval, overall_timeout) = {
            let budget = silence_budget(response_started);
            (
                std::time::Duration::from_millis(80),
                Some(budget.saturating_sub(last_msg_at.elapsed())),
            )
        };
        let outcome = select_stream(
            stdin,
            &mut rx,
            &mut line_buf,
            &mut interrupt_state,
            tick_interval,
            overall_timeout,
            &mut sigwinch,
        )
        .await;

        match outcome {
            StreamOutcome::Interrupted => {
                if let Some(sid) = session_id {
                    send_cancel(sid).await;
                }
                let _ = renderer.commit_panel("result", &["⊘ interrupted".to_string()], true);
                interrupt_state.reset();
                break;
            }
            StreamOutcome::Error(e) => {
                return Err(anyhow::anyhow!("Connection error: {}", e));
            }
            StreamOutcome::Deadline => {
                let msg = if !response_started {
                    format!(
                        "No response from the daemon for {PHASE1_SILENCE_TIMEOUT_SECS}s — \
                         it appears hung (a healthy daemon signals liveness every 15s \
                         even while the AI is thinking). Try `daemoneye status`, or check \
                         ~/.daemoneye/var/log/daemon.log."
                    )
                } else {
                    format!(
                        "Daemon went silent mid-response (no data or keep-alive for \
                         {PHASE2_SILENCE_TIMEOUT_SECS}s). Abandoning the connection; the \
                         daemon may still be running — check `daemoneye status`."
                    )
                };
                return Err(anyhow::anyhow!("Connection error: {}", msg));
            }
            StreamOutcome::Tick => {
                // Spinner tick — animate (phase 1 only).
                let verb = VERBS[(spin / TICKS_PER_VERB) % VERBS.len()];
                let dot_period = 18;
                let pos = (spin % TICKS_PER_VERB) % dot_period;
                let dot_count = if pos < 10 {
                    pos + 1
                } else {
                    dot_period - pos + 1
                };
                let sb = StatusBarState {
                    session_id: session_id.unwrap_or(""),
                    approval_hint: &approval.hint(),
                    model,
                    prompt_tokens: *prompt_tokens,
                    context_window,
                    daemon_up: false,
                    tools_total: 0,
                    cost_usd: 0.0,
                    has_untracked: false,
                };
                let _ = renderer.draw_spinner(SPINNER[spin % SPINNER.len()], verb, dot_count, &sb);
                spin = spin.wrapping_add(1);
                continue;
            }
            StreamOutcome::Reanchor => {
                renderer.reanchor();
                continue;
            }
            StreamOutcome::OpenViewer => {
                // Ctrl+O mid-turn. The viewer's teardown already re-pins the
                // inline viewport on exit; we re-pin again so the spinner
                // resumes over a correctly-anchored surface, then keep the
                // same connection and line_buf — no turn restart.
                if let Err(e) = crate::cli::viewer::run_transcript_viewer(
                    stdin,
                    &mut sigwinch,
                    renderer,
                    transcript,
                )
                .await
                {
                    eprintln!("\x1b[31m✗\x1b[0m viewer: {}", e);
                }
                renderer.reanchor();
                continue;
            }
            StreamOutcome::Warn => {
                // First interrupt press — show warning in live region.
                let sb = StatusBarState {
                    session_id: session_id.unwrap_or(""),
                    approval_hint: &approval.hint(),
                    model,
                    prompt_tokens: *prompt_tokens,
                    context_window,
                    daemon_up: false,
                    tools_total: 0,
                    cost_usd: 0.0,
                    has_untracked: false,
                };
                let _ = renderer.draw_spinner("⚡", "interrupt?", 0, &sb);
                continue;
            }
            StreamOutcome::Msg(_) => {
                // fall through to Response handling below
            }
        }

        let msg = match outcome {
            StreamOutcome::Msg(m) => *m,
            _ => unreachable!(),
        };
        // A real daemon message arrived — reset the phase-2 timeout deadline.
        last_msg_at = std::time::Instant::now();

        // Handle the message

        match msg {
            Response::KeepAlive => {
                // Animate spinner on each keepalive (phase 1).
                let verb = VERBS[(spin / TICKS_PER_VERB) % VERBS.len()];
                let dot_period = 18;
                let pos = (spin % TICKS_PER_VERB) % dot_period;
                let dot_count = if pos < 10 {
                    pos + 1
                } else {
                    dot_period - pos + 1
                };
                let sb = StatusBarState {
                    session_id: session_id.unwrap_or(""),
                    approval_hint: &approval.hint(),
                    model,
                    prompt_tokens: *prompt_tokens,
                    context_window,
                    daemon_up: false,
                    tools_total: 0,
                    cost_usd: 0.0,
                    has_untracked: false,
                };
                let _ = renderer.draw_spinner(SPINNER[spin % SPINNER.len()], verb, dot_count, &sb);
                spin = spin.wrapping_add(1);
                continue;
            }
            Response::Ok => {
                // Flush any remaining partial line to scrollback.
                let remaining = md.flush_to_lines(render_width);
                if !remaining.is_empty() {
                    let _ = renderer.commit_styled(&remaining);
                }
                break;
            }
            Response::Error(e) => {
                let _ = renderer.commit(&format!("\n✗ {}\n", e));
                break;
            }
            Response::Token(t) => {
                if !response_started {
                    response_started = true;
                    // First token arrived — switch to phase 2. The spinner keeps
                    // animating in the live region on any pause between tokens;
                    // the live region is redrawn normally after the turn completes.
                }
                // Record the raw token text before the markdown renderer consumes
                // it, so the transcript holds the lossless form.
                transcript.append_assistant(&t);
                // Stream token through markdown renderer, committing completed
                // styled lines to scrollback via the renderer.
                let lines = md.feed_to_lines(&t, render_width);
                if !lines.is_empty() {
                    let _ = renderer.commit_styled(&lines);
                }
            }
            Response::SessionInfo {
                session_cost_usd,
                has_untracked_cost,
                ..
            } => {
                *session_cost = session_cost_usd;
                *session_has_untracked = has_untracked_cost;
            }
            Response::UsageUpdate { prompt_tokens: pt } => {
                *prompt_tokens = pt;
            }
            Response::SystemMsg(msg) => {
                if !response_started {
                    response_started = true;
                }
                let _ = renderer.commit(&format!("\n⚙ {}\n", msg));
                transcript.push(crate::cli::transcript::Block::System { text: msg.clone() });
            }
            // Auto-deny tool calls — daemon will inform the AI and respond in text.
            Response::ToolCallPrompt {
                id,
                command,
                background,
                target_pane,
                classification,
                interactive,
            } => {
                let hint = approval.hint();
                let sb = prompt_status(
                    &hint,
                    ctx.model,
                    *prompt_tokens,
                    context_window,
                    *session_cost,
                    *session_has_untracked,
                );
                let (approved, user_message) = prompt_tool_call_ratatui(
                    renderer,
                    stdin,
                    &sb,
                    approval,
                    &command,
                    background,
                    target_pane.as_deref(),
                    &classification,
                    interactive,
                )
                .await?;
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
            Response::CredentialPrompt { id, prompt } => {
                let hint = approval.hint();
                let sb = prompt_status(
                    &hint,
                    ctx.model,
                    *prompt_tokens,
                    context_window,
                    *session_cost,
                    *session_has_untracked,
                );
                let credential = prompt_credential_ratatui(renderer, stdin, &sb, &prompt).await;
                send_request(&mut tx, Request::CredentialResponse { id, credential }).await?;
            }
            Response::PaneSelectPrompt { id, panes } => {
                let hint = approval.hint();
                let sb = prompt_status(
                    &hint,
                    ctx.model,
                    *prompt_tokens,
                    context_window,
                    *session_cost,
                    *session_has_untracked,
                );
                let pane_id = prompt_pane_select_ratatui(renderer, stdin, &sb, &panes).await;
                send_request(&mut tx, Request::PaneSelectResponse { id, pane_id }).await?;
            }
            Response::ScriptDeletePrompt { id, script_name } => {
                let hint = approval.hint();
                let sb = prompt_status(
                    &hint,
                    ctx.model,
                    *prompt_tokens,
                    context_window,
                    *session_cost,
                    *session_has_untracked,
                );
                let approved = prompt_yes_no_ratatui(
                    renderer,
                    stdin,
                    &sb,
                    &format!("AI wants to delete script: {}", script_name),
                    &format!("Approve deleting ~/.daemoneye/scripts/{}?", script_name),
                )
                .await;
                send_request(&mut tx, Request::ScriptDeleteResponse { id, approved }).await?;
            }
            Response::ScriptWritePrompt {
                id,
                script_name,
                content,
                existing_content,
            } => {
                let hint = approval.hint();
                let sb = prompt_status(
                    &hint,
                    ctx.model,
                    *prompt_tokens,
                    context_window,
                    *session_cost,
                    *session_has_untracked,
                );
                let approved = prompt_write_ratatui(
                    renderer,
                    stdin,
                    &sb,
                    approval,
                    &script_name,
                    &content,
                    existing_content.as_deref(),
                    "script",
                    |a: &mut super::approval::SessionApproval, name: &str| {
                        a.scripts.insert(name.to_string());
                    },
                    |a: &super::approval::SessionApproval| a.scripts_all,
                    |a: &super::approval::SessionApproval, name: &str| a.scripts.contains(name),
                )
                .await;
                send_request(&mut tx, Request::ScriptWriteResponse { id, approved }).await?;
            }
            Response::ScheduleWritePrompt {
                id,
                name,
                kind,
                action,
            } => {
                let hint = approval.hint();
                let sb = prompt_status(
                    &hint,
                    ctx.model,
                    *prompt_tokens,
                    context_window,
                    *session_cost,
                    *session_has_untracked,
                );
                let approved =
                    prompt_schedule_write_ratatui(renderer, stdin, &sb, &name, &kind, &action)
                        .await;
                send_request(&mut tx, Request::ScheduleWriteResponse { id, approved }).await?;
            }
            Response::RunbookWritePrompt {
                id,
                runbook_name,
                content,
                existing_content,
            } => {
                let hint = approval.hint();
                let sb = prompt_status(
                    &hint,
                    ctx.model,
                    *prompt_tokens,
                    context_window,
                    *session_cost,
                    *session_has_untracked,
                );
                let approved = prompt_write_ratatui(
                    renderer,
                    stdin,
                    &sb,
                    approval,
                    &runbook_name,
                    &content,
                    existing_content.as_deref(),
                    "runbook",
                    |a: &mut super::approval::SessionApproval, name: &str| {
                        a.runbooks.insert(name.to_string());
                    },
                    |a: &super::approval::SessionApproval| a.runbooks_all,
                    |a: &super::approval::SessionApproval, name: &str| a.runbooks.contains(name),
                )
                .await;
                send_request(&mut tx, Request::RunbookWriteResponse { id, approved }).await?;
            }
            Response::RunbookDeletePrompt {
                id,
                runbook_name,
                active_jobs,
            } => {
                let job_info = if active_jobs.is_empty() {
                    String::new()
                } else {
                    format!(
                        "\n  Active jobs referencing this runbook:\n  {}",
                        active_jobs.join(", ")
                    )
                };
                let hint = approval.hint();
                let sb = prompt_status(
                    &hint,
                    ctx.model,
                    *prompt_tokens,
                    context_window,
                    *session_cost,
                    *session_has_untracked,
                );
                let approved = prompt_yes_no_ratatui(
                    renderer,
                    stdin,
                    &sb,
                    &format!("AI wants to delete runbook: {}{}", runbook_name, job_info),
                    &format!("Approve deleting ~/.daemoneye/runbooks/{}?", runbook_name),
                )
                .await;
                send_request(&mut tx, Request::RunbookDeleteResponse { id, approved }).await?;
            }
            Response::EditFilePrompt {
                id,
                path,
                operation,
                existing_content,
                new_content,
                dest_path,
                sudo,
            } => {
                let hint = approval.hint();
                let sb = prompt_status(
                    &hint,
                    ctx.model,
                    *prompt_tokens,
                    context_window,
                    *session_cost,
                    *session_has_untracked,
                );
                let (approved, user_message) = prompt_edit_file_ratatui(
                    renderer,
                    stdin,
                    &sb,
                    approval,
                    &path,
                    &operation,
                    existing_content.as_deref(),
                    new_content.as_deref(),
                    dest_path.as_deref(),
                    sudo,
                )
                .await;
                send_request(
                    &mut tx,
                    Request::EditFileResponse {
                        id,
                        approved,
                        user_message,
                    },
                )
                .await?;
            }
            // Silent tool calls and results — accumulate for minimal display.
            Response::ToolStarted { tool, summary, .. } => {
                let body = if !summary.is_empty() {
                    vec![format!("▸ {}", summary)]
                } else {
                    vec!["▸ running".to_string()]
                };
                pending_tool = Some((tool, body));
            }
            Response::ToolFinished { ok, elapsed_ms, .. } => {
                let label = tool_runtime_label(ok, elapsed_ms);
                match pending_tool.take() {
                    Some((title, body)) => {
                        let _ = renderer.commit_panel_labeled(&title, &body, false, Some(&label));
                        transcript.push(crate::cli::transcript::Block::ToolPanel {
                            tool: title,
                            summary: body.join("\n"),
                            label: Some(label),
                        });
                    }
                    None => {
                        let _ = renderer.commit_panel_labeled(
                            "result",
                            std::slice::from_ref(&label),
                            true,
                            None,
                        );
                        transcript.push(crate::cli::transcript::Block::ToolPanel {
                            tool: "result".to_string(),
                            summary: label.clone(),
                            label: None,
                        });
                    }
                }
            }
            Response::ToolResult {
                tool_call_id,
                output,
            } => {
                let lines: Vec<String> = output.lines().map(|l| l.to_string()).collect();
                let total = lines.len();
                const MAX_LINES: usize = 10;
                let shown = if total > MAX_LINES {
                    MAX_LINES - 1
                } else {
                    total
                };
                let mut body: Vec<String> = lines[..shown].to_vec();
                if total > MAX_LINES {
                    body.push(output_footer(total, shown));
                } else if body.is_empty() {
                    body.push("(no output)".to_string());
                }
                let _ = renderer.commit_panel("output", &body, true);
                transcript.push(crate::cli::transcript::Block::Output {
                    tool_call_id,
                    full: output,
                    shown,
                });
            }
            // Ignore informational responses not relevant to minimal rendering.
            Response::ScheduleList { .. }
            | Response::ScriptList { .. }
            | Response::RunbookList { .. }
            | Response::DaemonStatus { .. }
            | Response::ModelChanged { .. }
            | Response::ModelList { .. }
            | Response::PaneChanged { .. }
            | Response::PaneList { .. }
            | Response::ShellList { .. }
            | Response::ShellLogList { .. }
            | Response::ShellLog { .. }
            | Response::ShellScreen { .. }
            | Response::ShellChunk { .. }
            | Response::ShellSignalled { .. }
            | Response::ShellOpened { .. }
            | Response::ShellExited
            | Response::ContainerStatus { .. }
            | Response::LimitsInfo { .. }
            | Response::SessionSaved { .. }
            | Response::SessionLoaded { .. }
            | Response::SavedSessionList { .. } => {}
        }
    }

    // Flush a started-but-never-finished tool so its panel is not lost.
    if let Some((title, body)) = pending_tool.take() {
        let body_clone = body.clone();
        let _ = renderer.commit_panel(&title, &body_clone, false);
        transcript.push(crate::cli::transcript::Block::ToolPanel {
            tool: title,
            summary: body_clone.join("\n"),
            label: None,
        });
    }

    // Turn completed normally — reset interrupt state for next turn.
    interrupt_state.reset();

    Ok(())
}

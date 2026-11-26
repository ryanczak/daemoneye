//! `ask_with_session_ratatui`: one chat turn streamed through the ratatui
//! renderer, from the request to the final token.

use super::*;
use crate::cli::render_ratatui::RatatuiRendererStdout;
use tokio::net::unix::OwnedWriteHalf;

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

fn draw_spinner_tick(
    renderer: &mut RatatuiRendererStdout,
    spin: &mut usize,
    approval: &SessionApproval,
    typeahead_text: &str,
    sb: &mut crate::cli::status_bar::StatusModel,
) {
    let s = *spin;
    let verb = VERBS[(s / TICKS_PER_VERB) % VERBS.len()];
    let dot_period = 18;
    let pos = (s % TICKS_PER_VERB) % dot_period;
    let dot_count = if pos < 10 {
        pos + 1
    } else {
        dot_period - pos + 1
    };
    sb.grants = approval.summary();
    let _ = renderer.draw_spinner(
        SPINNER[s % SPINNER.len()],
        verb,
        dot_count,
        typeahead_text,
        sb,
    );
    *spin = s.wrapping_add(1);
}

#[allow(clippy::too_many_arguments)]
async fn handle_tool_or_cred_prompt(
    msg: Response,
    tx: &mut OwnedWriteHalf,
    renderer: &mut RatatuiRendererStdout,
    stdin: &AsyncStdin,
    sb: &mut crate::cli::status_bar::StatusModel,
    approval: &mut SessionApproval,
    status: &crate::cli::program_status_emit::Emitter,
) -> Result<()> {
    status.blocked(crate::shell::program_status::Kind::Permission);
    sb.grants = approval.summary();
    sb.prompt_opened();
    match msg {
        Response::ToolCallPrompt {
            id,
            command,
            background,
            target_pane,
            classification,
            interactive,
            shell_label,
            sudo,
        } => {
            let (approved, user_message) = prompt_tool_call_ratatui(
                renderer,
                stdin,
                sb,
                approval,
                &command,
                background,
                target_pane.as_deref(),
                &classification,
                interactive,
                shell_label.as_deref(),
                sudo,
            )
            .await?;
            send_request(
                tx,
                Request::ToolCallResponse {
                    id,
                    approved,
                    user_message,
                },
            )
            .await?;
        }
        Response::CredentialPrompt { id, prompt } => {
            let credential = prompt_credential_ratatui(renderer, stdin, sb, &prompt).await;
            send_request(tx, Request::CredentialResponse { id, credential }).await?;
        }
        Response::PaneSelectPrompt { id, panes } => {
            let pane_id = prompt_pane_select_ratatui(renderer, stdin, sb, &panes).await;
            send_request(tx, Request::PaneSelectResponse { id, pane_id }).await?;
        }
        _ => {}
    }
    status.reply();
    sb.prompt_answered();
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_script_or_schedule_prompt(
    msg: Response,
    tx: &mut OwnedWriteHalf,
    renderer: &mut RatatuiRendererStdout,
    stdin: &AsyncStdin,
    sb: &mut crate::cli::status_bar::StatusModel,
    approval: &mut SessionApproval,
    status: &crate::cli::program_status_emit::Emitter,
) -> Result<()> {
    status.blocked(crate::shell::program_status::Kind::Permission);
    sb.grants = approval.summary();
    sb.prompt_opened();
    match msg {
        Response::ScriptDeletePrompt {
            id,
            script_name,
            agent,
        } => {
            let agent_name = agent_name_of(&script_name);
            let kind_label = if agent { "agent" } else { "script" };
            let delete_kind = if agent {
                super::approval::PromptKind::AgentDelete(&agent_name)
            } else {
                super::approval::PromptKind::ScriptDelete(&script_name)
            };
            let approved = prompt_delete_ratatui(
                renderer,
                stdin,
                sb,
                approval,
                delete_kind,
                &script_name,
                kind_label,
                &format!("~/.daemoneye/scripts/{script_name}"),
            )
            .await;
            send_request(tx, Request::ScriptDeleteResponse { id, approved }).await?;
        }
        Response::ScriptWritePrompt {
            id,
            script_name,
            content,
            existing_content,
            agent,
        } => {
            let kind = if agent { "agent" } else { "script" };
            let prompt_kind = if agent {
                super::approval::PromptKind::AgentWrite(&agent_name_of(&script_name))
            } else {
                super::approval::PromptKind::ScriptWrite(&script_name)
            };
            let approved = prompt_write_ratatui(
                renderer,
                stdin,
                sb,
                approval,
                &script_name,
                &content,
                existing_content.as_deref(),
                kind,
                prompt_kind,
                |a: &mut super::approval::SessionApproval, name: &str| {
                    if agent {
                        a.agents.insert(agent_name_of(name));
                    } else {
                        a.scripts.insert(name.to_string());
                    }
                },
            )
            .await;
            send_request(tx, Request::ScriptWriteResponse { id, approved }).await?;
        }
        Response::ScheduleWritePrompt {
            id,
            name,
            kind,
            action,
        } => {
            let approved =
                prompt_schedule_write_ratatui(renderer, stdin, sb, approval, &name, &kind, &action)
                    .await;
            send_request(tx, Request::ScheduleWriteResponse { id, approved }).await?;
        }
        Response::GhostSpawnPrompt {
            id,
            runbook,
            agent,
            host,
            message,
        } => {
            let approved = prompt_ghost_spawn_ratatui(
                renderer,
                stdin,
                sb,
                &runbook,
                agent.as_deref(),
                host.as_deref(),
                &message,
            )
            .await;
            send_request(tx, Request::GhostSpawnResponse { id, approved }).await?;
        }
        _ => {}
    }
    status.reply();
    sb.prompt_answered();
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_runbook_or_file_prompt(
    msg: Response,
    tx: &mut OwnedWriteHalf,
    renderer: &mut RatatuiRendererStdout,
    stdin: &AsyncStdin,
    sb: &mut crate::cli::status_bar::StatusModel,
    approval: &mut SessionApproval,
    status: &crate::cli::program_status_emit::Emitter,
) -> Result<()> {
    status.blocked(crate::shell::program_status::Kind::Permission);
    sb.grants = approval.summary();
    sb.prompt_opened();
    match msg {
        Response::RunbookWritePrompt {
            id,
            runbook_name,
            content,
            existing_content,
        } => {
            let approved = prompt_write_ratatui(
                renderer,
                stdin,
                sb,
                approval,
                &runbook_name,
                &content,
                existing_content.as_deref(),
                "runbook",
                super::approval::PromptKind::RunbookWrite(&runbook_name),
                |a: &mut super::approval::SessionApproval, name: &str| {
                    a.runbooks.insert(name.to_string());
                },
            )
            .await;
            send_request(tx, Request::RunbookWriteResponse { id, approved }).await?;
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
            let _ = renderer.commit(&format!(
                "\n  ⚙ AI wants to delete runbook: {}{}",
                runbook_name, job_info
            ));
            let approved = prompt_delete_ratatui(
                renderer,
                stdin,
                sb,
                approval,
                super::approval::PromptKind::RunbookDelete(&runbook_name),
                &runbook_name,
                "runbook",
                &format!("~/.daemoneye/runbooks/{runbook_name}.md"),
            )
            .await;
            send_request(tx, Request::RunbookDeleteResponse { id, approved }).await?;
        }
        Response::EditFilePrompt {
            id,
            path,
            operation,
            existing_content,
            new_content,
            dest_path,
            sudo,
            always_asked,
        } => {
            let (approved, user_message) = prompt_edit_file_ratatui(
                renderer,
                stdin,
                sb,
                approval,
                &path,
                &operation,
                existing_content.as_deref(),
                new_content.as_deref(),
                dest_path.as_deref(),
                sudo,
                always_asked,
            )
            .await;
            send_request(
                tx,
                Request::EditFileResponse {
                    id,
                    approved,
                    user_message,
                },
            )
            .await?;
        }
        _ => {}
    }
    status.reply();
    sb.prompt_answered();
    Ok(())
}

fn handle_tool_feedback_frame(
    msg: Response,
    renderer: &mut RatatuiRendererStdout,
    transcript: &mut crate::cli::transcript::Transcript,
    pending_tool: &mut Option<(String, Vec<String>)>,
) {
    match msg {
        Response::ToolStarted { tool, summary, .. } => {
            let body = if !summary.is_empty() {
                vec![format!("▸ {}", summary)]
            } else {
                vec!["▸ running".to_string()]
            };
            *pending_tool = Some((tool, body));
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
        _ => {}
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_stream_frame(
    msg: Response,
    tx: &mut OwnedWriteHalf,
    renderer: &mut RatatuiRendererStdout,
    stdin: &AsyncStdin,
    transcript: &mut crate::cli::transcript::Transcript,
    typeahead: &Typeahead,
    status: &crate::cli::program_status_emit::Emitter,
    sb: &mut crate::cli::status_bar::StatusModel,
    approval: &mut SessionApproval,
    md: &mut MarkdownRenderer,
    render_width: usize,
    spin: &mut usize,
    response_started: &mut bool,
    pending_tool: &mut Option<(String, Vec<String>)>,
) -> Result<bool> {
    status.frame(&msg);
    match msg {
        Response::KeepAlive => {
            draw_spinner_tick(renderer, spin, approval, typeahead.text(), sb);
            Ok(false)
        }
        Response::Ok => {
            let remaining = md.flush_to_lines(render_width);
            if !remaining.is_empty() {
                let _ = renderer.commit_styled(&remaining);
            }
            Ok(true)
        }
        Response::Error(e) => {
            let _ = renderer.commit(&format!("\n✗ {}\n", e));
            Ok(true)
        }
        Response::Token(t) => {
            *response_started = true;
            transcript.append_assistant(&t);
            let lines = md.feed_to_lines(&t, render_width);
            if !lines.is_empty() {
                let _ = renderer.commit_styled(&lines);
            }
            Ok(false)
        }
        Response::SessionInfo {
            session_cost_usd,
            has_untracked_cost,
            ..
        } => {
            sb.cost_usd = session_cost_usd;
            sb.has_untracked = has_untracked_cost;
            Ok(false)
        }
        Response::UsageUpdate {
            prompt_tokens: pt,
            output_tokens,
        } => {
            sb.prompt_tokens = pt;
            sb.last_output_tokens = output_tokens;
            Ok(false)
        }
        Response::SystemMsg(msg) => {
            *response_started = true;
            let _ = renderer.commit(&format!("\n⚙ {}\n", msg));
            transcript.push(crate::cli::transcript::Block::System { text: msg });
            Ok(false)
        }
        Response::ToolCallPrompt { .. }
        | Response::CredentialPrompt { .. }
        | Response::PaneSelectPrompt { .. } => {
            handle_tool_or_cred_prompt(msg, tx, renderer, stdin, sb, approval, status).await?;
            Ok(false)
        }
        Response::ScriptDeletePrompt { .. }
        | Response::ScriptWritePrompt { .. }
        | Response::ScheduleWritePrompt { .. }
        | Response::GhostSpawnPrompt { .. } => {
            handle_script_or_schedule_prompt(msg, tx, renderer, stdin, sb, approval, status)
                .await?;
            Ok(false)
        }
        Response::RunbookWritePrompt { .. }
        | Response::RunbookDeletePrompt { .. }
        | Response::EditFilePrompt { .. } => {
            handle_runbook_or_file_prompt(msg, tx, renderer, stdin, sb, approval, status).await?;
            Ok(false)
        }
        Response::ToolStarted { .. }
        | Response::ToolFinished { .. }
        | Response::ToolResult { .. } => {
            handle_tool_feedback_frame(msg, renderer, transcript, pending_tool);
            Ok(false)
        }
        _ => Ok(false),
    }
}

fn deadline_error(response_started: bool) -> anyhow::Error {
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
    anyhow::anyhow!("Connection error: {}", msg)
}

async fn send_initial_ask_request(
    tx: &mut OwnedWriteHalf,
    qa: QueryArgs<'_>,
    session_id: Option<&str>,
    chat_width: Option<usize>,
    tmux: AskTmuxCtx<'_>,
) -> Result<()> {
    let chat_pane = std::env::var("TMUX_PANE").ok();
    let tmux_pane = tmux
        .pane
        .map(|s| s.to_string())
        .or_else(|| std::env::var("TMUX_PANE").ok());

    send_request(
        tx,
        Request::Ask {
            query: qa.query,
            tmux_pane,
            session_id: session_id.map(|s| s.to_string()),
            chat_pane,
            prompt: qa.prompt_override.map(|s| s.to_string()),
            chat_width,
            tmux_session: tmux.session.map(|s| s.to_string()),
            target_pane: tmux.pane.map(|s| s.to_string()),
            model: None,
        },
    )
    .await
}

/// Stream the AI response through the ratatui renderer.
pub(in super::super) async fn ask_with_session_ratatui(
    qa: QueryArgs<'_>,
    session_id: Option<&str>,
    approval: &mut SessionApproval,
    tmux: AskTmuxCtx<'_>,
    ctx: RatatuiQueryCtx<'_>,
) -> Result<()> {
    let RatatuiQueryCtx {
        chat_width,
        renderer,
        stdin,
        transcript,
        typeahead,
        status,
        sb,
        ..
    } = ctx;

    let stream = connect().await?;
    let (rx, mut tx) = stream.into_split();
    let mut rx = BufReader::new(rx);

    status.working();
    let _idle_guard = status.idle_on_drop();
    send_initial_ask_request(&mut tx, qa, session_id, chat_width, tmux).await?;

    let verb_offset = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as usize)
        % VERBS.len();
    let mut spin = verb_offset * TICKS_PER_VERB;
    let mut response_started = false;
    let mut pending_tool: Option<(String, Vec<String>)> = None;
    let mut interrupt_state = InterruptState::new();
    let mut md = MarkdownRenderer::new();
    let render_width = chat_width.map(|w| w - 2).unwrap_or(80).max(20);
    let mut line_buf: Vec<u8> = Vec::new();
    let mut last_msg_at = std::time::Instant::now();
    let mut sigwinch =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change())?;

    loop {
        let budget = silence_budget(response_started);
        let outcome = select_stream(
            stdin,
            &mut rx,
            &mut line_buf,
            &mut interrupt_state,
            typeahead,
            std::time::Duration::from_millis(80),
            Some(budget.saturating_sub(last_msg_at.elapsed())),
            &mut sigwinch,
        )
        .await;

        let msg = match outcome {
            StreamOutcome::Interrupted => {
                if let Some(sid) = session_id {
                    send_cancel(sid).await;
                }
                let _ = renderer.commit_panel("result", &["⊘ interrupted".to_string()], true);
                interrupt_state.reset();
                break;
            }
            StreamOutcome::Error(e) => return Err(anyhow::anyhow!("Connection error: {}", e)),
            StreamOutcome::Deadline => return Err(deadline_error(response_started)),
            StreamOutcome::Tick => {
                draw_spinner_tick(renderer, &mut spin, approval, typeahead.text(), sb);
                continue;
            }
            StreamOutcome::Reanchor => {
                renderer.reanchor();
                continue;
            }
            StreamOutcome::OpenViewer => {
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
                sb.grants = approval.summary();
                let _ = renderer.draw_spinner("⚡", "interrupt?", 0, typeahead.text(), sb);
                continue;
            }
            StreamOutcome::Msg(m) => *m,
        };

        last_msg_at = std::time::Instant::now();
        if handle_stream_frame(
            msg,
            &mut tx,
            renderer,
            stdin,
            transcript,
            typeahead,
            status,
            sb,
            approval,
            &mut md,
            render_width,
            &mut spin,
            &mut response_started,
            &mut pending_tool,
        )
        .await?
        {
            break;
        }
    }

    if let Some((title, body)) = pending_tool.take() {
        let _ = renderer.commit_panel(&title, &body, false);
        transcript.push(crate::cli::transcript::Block::ToolPanel {
            tool: title,
            summary: body.join("\n"),
            label: None,
        });
    }

    interrupt_state.reset();
    Ok(())
}

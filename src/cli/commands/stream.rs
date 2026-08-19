//! Streaming response rendering for chat and ask flows.
//!
//! Owns `ask_with_session_ratatui`, the long-lived loop that consumes `Response`
//! events from the daemon and renders them via the ratatui inline-viewport
//! renderer (tokens, tool panels, approval prompts).

use anyhow::Result;
use tokio::io::BufReader;

use crate::cli::input::*;
use crate::cli::markdown::MarkdownRenderer;
use crate::cli::render::*;
use crate::ipc::{Request, Response};

use super::approval::SessionApproval;
use super::interrupt::{InterruptAction, InterruptState};
use super::ipc_client::{connect, send_request};

/// Client-side silence bounds, derived from the daemon's
/// `KEEPALIVE_PERIOD_SECS` (15 s) with >= 6x margin: while a turn is in
/// flight the daemon sends *something* at least every 15 s, so 90 s of
/// total silence before the first content means the daemon is hung, not
/// slow. Phase 2 keeps the pre-existing 120 s.
const PHASE1_SILENCE_TIMEOUT_SECS: u64 = 90;
const PHASE2_SILENCE_TIMEOUT_SECS: u64 = 120;

/// Outcome of the inner streaming loop (daemon message or user interrupt).
#[derive(Debug)]
enum StreamOutcome {
    /// A daemon message arrived.
    Msg(Box<Response>),
    /// Spinner tick — caller should animate.
    Tick,
    /// First interrupt press — caller should show a warning.
    Warn,
    /// User aborted the turn.
    Interrupted,
    /// A resize or focus-gain arrived mid-stream — caller must re-anchor.
    Reanchor,
    /// Daemon error (EOF, parse failure, timeout).
    Error(String),
    /// Deadline expired before a daemon message (phase-accurate timeout).
    Deadline,
}

fn silence_budget(response_started: bool) -> std::time::Duration {
    std::time::Duration::from_secs(if response_started {
        PHASE2_SILENCE_TIMEOUT_SECS
    } else {
        PHASE1_SILENCE_TIMEOUT_SECS
    })
}

/// Fire an out-of-band cancel at the daemon on a fresh connection, so the
/// streaming connection's reader is never touched. Best-effort: on failure
/// (daemon gone, timeout) the daemon still ends the turn via EPIPE as today.
async fn send_cancel(session_id: &str) {
    let fut = async {
        let mut stream =
            tokio::net::UnixStream::connect(crate::config::default_socket_path()).await?;
        let mut data = serde_json::to_vec(&crate::ipc::Request::Cancel {
            session_id: session_id.to_string(),
        })?;
        data.push(b'\n');
        use tokio::io::AsyncWriteExt;
        stream.write_all(&data).await?;
        anyhow::Ok(())
    };
    if let Err(e) = tokio::time::timeout(std::time::Duration::from_secs(2), fut)
        .await
        .unwrap_or_else(|_| Err(anyhow::anyhow!("cancel send timed out")))
    {
        log::warn!("failed to deliver cancel: {e}");
    }
}

// ── AI conversation ─────────────────────────────────────────────────────────

pub(super) struct QueryArgs<'a> {
    pub(super) query: String,
    pub(super) prompt_override: Option<&'a str>,
}

pub(super) struct AskTmuxCtx<'a> {
    pub(super) session: Option<&'a str>,
    pub(super) pane: Option<&'a str>,
}

pub(super) struct TokenCtx<'a> {
    pub(super) prompt_tokens: &'a mut u32,
    pub(super) context_window: u32,
}

/// Context for the ratatui-path query function.
pub(super) struct RatatuiQueryCtx<'a> {
    pub(super) chat_width: Option<usize>,
    pub(super) session_cost: &'a mut f64,
    pub(super) session_has_untracked: &'a mut bool,
    pub(super) renderer: &'a mut crate::cli::render_ratatui::RatatuiRendererStdout,
    pub(super) model: &'a str,
    pub(super) stdin: &'a AsyncStdin,
    pub(super) transcript: &'a mut crate::cli::transcript::Transcript,
}

/// Stream the AI response through the ratatui renderer.
///
/// Before the first token, animates a spinner in the inline viewport.
/// Tokens are fed through markdown rendering and committed line-by-line
/// to scrollback as styled spans.  Tool calls are auto-denied.
pub(super) async fn ask_with_session_ratatui(
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
            } => {
                let sb = crate::cli::render::StatusBarState {
                    session_id: "",
                    approval_hint: &approval.hint(),
                    model: ctx.model,
                    prompt_tokens: *prompt_tokens,
                    context_window,
                    daemon_up: false,
                    tools_total: 0,
                    cost_usd: *session_cost,
                    has_untracked: *session_has_untracked,
                };
                let (approved, user_message) = prompt_tool_call_ratatui(
                    renderer,
                    stdin,
                    &sb,
                    approval,
                    &command,
                    background,
                    target_pane.as_deref(),
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
                let sb = crate::cli::render::StatusBarState {
                    session_id: "",
                    approval_hint: &approval.hint(),
                    model: ctx.model,
                    prompt_tokens: *prompt_tokens,
                    context_window,
                    daemon_up: false,
                    tools_total: 0,
                    cost_usd: *session_cost,
                    has_untracked: *session_has_untracked,
                };
                let credential = prompt_credential_ratatui(renderer, stdin, &sb, &prompt).await;
                send_request(&mut tx, Request::CredentialResponse { id, credential }).await?;
            }
            Response::PaneSelectPrompt { id, panes } => {
                let sb = crate::cli::render::StatusBarState {
                    session_id: "",
                    approval_hint: &approval.hint(),
                    model: ctx.model,
                    prompt_tokens: *prompt_tokens,
                    context_window,
                    daemon_up: false,
                    tools_total: 0,
                    cost_usd: *session_cost,
                    has_untracked: *session_has_untracked,
                };
                let pane_id = prompt_pane_select_ratatui(renderer, stdin, &sb, &panes).await;
                send_request(&mut tx, Request::PaneSelectResponse { id, pane_id }).await?;
            }
            Response::ScriptDeletePrompt { id, script_name } => {
                let sb = crate::cli::render::StatusBarState {
                    session_id: "",
                    approval_hint: &approval.hint(),
                    model: ctx.model,
                    prompt_tokens: *prompt_tokens,
                    context_window,
                    daemon_up: false,
                    tools_total: 0,
                    cost_usd: *session_cost,
                    has_untracked: *session_has_untracked,
                };
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
                let sb = crate::cli::render::StatusBarState {
                    session_id: "",
                    approval_hint: &approval.hint(),
                    model: ctx.model,
                    prompt_tokens: *prompt_tokens,
                    context_window,
                    daemon_up: false,
                    tools_total: 0,
                    cost_usd: *session_cost,
                    has_untracked: *session_has_untracked,
                };
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
                let sb = crate::cli::render::StatusBarState {
                    session_id: "",
                    approval_hint: &approval.hint(),
                    model: ctx.model,
                    prompt_tokens: *prompt_tokens,
                    context_window,
                    daemon_up: false,
                    tools_total: 0,
                    cost_usd: *session_cost,
                    has_untracked: *session_has_untracked,
                };
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
                let sb = crate::cli::render::StatusBarState {
                    session_id: "",
                    approval_hint: &approval.hint(),
                    model: ctx.model,
                    prompt_tokens: *prompt_tokens,
                    context_window,
                    daemon_up: false,
                    tools_total: 0,
                    cost_usd: *session_cost,
                    has_untracked: *session_has_untracked,
                };
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
                let sb = crate::cli::render::StatusBarState {
                    session_id: "",
                    approval_hint: &approval.hint(),
                    model: ctx.model,
                    prompt_tokens: *prompt_tokens,
                    context_window,
                    daemon_up: false,
                    tools_total: 0,
                    cost_usd: *session_cost,
                    has_untracked: *session_has_untracked,
                };
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
            } => {
                let sb = crate::cli::render::StatusBarState {
                    session_id: "",
                    approval_hint: &approval.hint(),
                    model: ctx.model,
                    prompt_tokens: *prompt_tokens,
                    context_window,
                    daemon_up: false,
                    tools_total: 0,
                    cost_usd: *session_cost,
                    has_untracked: *session_has_untracked,
                };
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
                    body.push(format!("… {} more lines", total - shown));
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
            | Response::LimitsInfo { .. }
            | Response::SessionSaved { .. }
            | Response::SessionLoaded { .. }
            | Response::SavedSessionList { .. } => {}
        }
    }

    // Flush a started-but-never-finished tool so its panel is not lost.
    if let Some((title, body)) = pending_tool.take() {
        let _ = renderer.commit_panel(&title, &body, false);
    }

    // Turn completed normally — reset interrupt state for next turn.
    interrupt_state.reset();

    Ok(())
}

/// Read one newline-delimited `Response`, accumulating bytes into the
/// **caller-owned** `buf`.
///
/// `read_until` appends consumed bytes to `buf` *before* awaiting more, so if
/// this future is dropped mid-line (an interrupt key or spinner tick won the
/// `select!`), the partial bytes remain in `buf` and the next call continues
/// where it left off. This is what makes interrupting a streaming read
/// non-destructive (the bug-phase-11-1 / bug-phase-11-2 failure mode was a
/// dropped read that stranded bytes inside the future's own local buffer).
///
/// On a complete line, `buf` is cleared and the parsed `Response` returned.
async fn recv_line(
    rx: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
    buf: &mut Vec<u8>,
) -> anyhow::Result<Response> {
    use tokio::io::AsyncBufReadExt;
    let n = rx.read_until(b'\n', buf).await?;
    if n == 0 {
        anyhow::bail!("Daemon closed connection unexpectedly.");
    }
    let line = std::str::from_utf8(buf)?.trim();
    let response: Response = serde_json::from_str(line)?;
    buf.clear();
    Ok(response)
}

/// Race keyboard input against a daemon read whose partial state lives in the
/// caller-owned `buf` (see [`recv_line`]).
///
/// Returning `Warn` or `Tick` drops the in-flight `recv_line` future, but the
/// bytes it already read are preserved in `buf`, so the caller can re-enter
/// without losing a daemon message. Only `Msg`/`Error`/`Interrupted` end the
/// read for good.
///
/// `tick_interval` controls the spinner animation rate (80 ms for phase 1,
/// `Duration::MAX` to disable for phase 2). `overall_timeout` is an optional
/// total timeout for the daemon response (120 s for phase 2, `None` for phase 1).
async fn select_stream(
    stdin: &AsyncStdin,
    rx: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
    buf: &mut Vec<u8>,
    interrupt_state: &mut InterruptState,
    tick_interval: std::time::Duration,
    overall_timeout: Option<std::time::Duration>,
    sigwinch: &mut tokio::signal::unix::Signal,
) -> StreamOutcome {
    let mut timeout_fut = overall_timeout.map(|d| Box::pin(tokio::time::sleep(d)));

    loop {
        if let Some(ref mut to) = timeout_fut {
            tokio::select! {
                key = read_key(stdin) => {
                    if let Some(key) = key {
                        if let Some(outcome) = focus_outcome(&key) {
                            return outcome;
                        }
                        match interrupt_state.feed(&key) {
                            InterruptAction::Ignore => continue,
                            InterruptAction::Warn => return StreamOutcome::Warn,
                            InterruptAction::Abort => return StreamOutcome::Interrupted,
                        }
                    }
                    continue;
                }
                res = recv_line(rx, buf) => {
                    return match res {
                        Ok(response) => StreamOutcome::Msg(Box::new(response)),
                        Err(e) => StreamOutcome::Error(e.to_string()),
                    };
                }
                _ = to => {
                    return StreamOutcome::Deadline;
                }
                _ = tokio::time::sleep(tick_interval), if tick_interval != std::time::Duration::MAX => {
                    return StreamOutcome::Tick;
                }
                _ = sigwinch.recv() => {
                    return StreamOutcome::Reanchor;
                }
            }
        } else {
            tokio::select! {
                key = read_key(stdin) => {
                    if let Some(key) = key {
                        if let Some(outcome) = focus_outcome(&key) {
                            return outcome;
                        }
                        match interrupt_state.feed(&key) {
                            InterruptAction::Ignore => continue,
                            InterruptAction::Warn => return StreamOutcome::Warn,
                            InterruptAction::Abort => return StreamOutcome::Interrupted,
                        }
                    }
                    continue;
                }
                res = recv_line(rx, buf) => {
                    return match res {
                        Ok(response) => StreamOutcome::Msg(Box::new(response)),
                        Err(e) => StreamOutcome::Error(e.to_string()),
                    };
                }
                _ = tokio::time::sleep(tick_interval), if tick_interval != std::time::Duration::MAX => {
                    return StreamOutcome::Tick;
                }
                _ = sigwinch.recv() => {
                    return StreamOutcome::Reanchor;
                }
            }
        }
    }
}

// ── Ratatui interactive approval primitives ──────────────────────────────────

/// Map a key event to a stream outcome the interrupt filter must not
/// swallow. FocusGained (ESC [ I) means the user switched back to this
/// pane and the viewport may need re-pinning.
fn focus_outcome(key: &Key) -> Option<StreamOutcome> {
    match key {
        Key::FocusGained => Some(StreamOutcome::Reanchor),
        _ => None,
    }
}

/// Bottom-border label for a finished tool: "✓ 1.2s" / "✗ 0.5s".
fn tool_runtime_label(ok: bool, elapsed_ms: u64) -> String {
    let status = if ok { "✓" } else { "✗" };
    format!("{status} {:.1}s", elapsed_ms as f64 / 1000.0)
}

/// Build the canonical approval-prompt string shared by every approval flow.
/// Option order is fixed: [Y]es, [A]pprove for <label>, [N]o, then the
/// redirect affordance only where the flow supports it. Keeping all flows on
/// this one builder is what prevents the prompts from drifting apart again.
fn build_approval_prompt(session_label: &str, supports_redirect: bool) -> String {
    let redirect = if supports_redirect {
        "or type a message "
    } else {
        ""
    };
    format!("  Approve? [Y]es  [A]pprove for {session_label}  [N]o  {redirect}› ")
}

/// Parse a user response to a Y/N/A prompt.
fn parse_approval_response(input: &str) -> (bool, bool, Option<String>) {
    let trimmed = input.trim();
    let lower = trimmed.to_lowercase();
    match lower.as_str() {
        "y" | "yes" => (true, false, None),
        "n" | "no" | "" => (false, false, None),
        "a" => (true, true, None),
        _ => (false, false, Some(trimmed.to_string())),
    }
}

/// Read a single keypress or full line under crossterm raw mode, rendering
/// the prompt and input in the live region (not scrollback).
///
/// If the first character is Y, N, or A (case-insensitive), returns it
/// immediately.  Otherwise reads the full line using the input editor,
/// redrawing the live region on each keystroke so the user sees their
/// typed text in place.
async fn read_approval_input(
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    stdin: &AsyncStdin,
    prompt_text: &str,
    status: &crate::cli::render::StatusBarState<'_>,
) -> String {
    use crate::cli::input::InputLine;

    // Initial draw with empty input.
    let mut line = InputLine::new();
    let _ = renderer.draw_prompt(prompt_text, &line, status);

    // Read the first byte to decide: single-key shortcut vs. full line edit.
    if let Some(first) = stdin.read_byte().await {
        let ch = first as char;
        match ch.to_ascii_lowercase() {
            'y' | 'n' | 'a' => {
                // Show the key the user pressed in the input box, then return.
                line.insert(ch);
                let _ = renderer.draw_prompt(prompt_text, &line, status);
                ch.to_string()
            }
            '\r' | '\n' => {
                // Empty input (Enter pressed immediately)
                String::new()
            }
            _ => {
                // Start of a typed message — use the input editor.
                line.insert(ch);
                let _ = renderer.draw_prompt(prompt_text, &line, status);

                loop {
                    match stdin.read_byte().await {
                        Some(b'\r' | b'\n') => {
                            return line.as_str();
                        }
                        Some(b'\x7f' | b'\x08') => {
                            line.backspace();
                            let _ = renderer.draw_prompt(prompt_text, &line, status);
                        }
                        Some(b'\x03') => {
                            // Ctrl+C — cancel, return empty
                            return String::new();
                        }
                        Some(b'\x1b') => {
                            // Escape — cancel, return empty
                            return String::new();
                        }
                        Some(b) => {
                            if b >= 0x20 {
                                line.insert(b as char);
                                let _ = renderer.draw_prompt(prompt_text, &line, status);
                            }
                        }
                        None => return line.as_str(),
                    }
                }
            }
        }
    } else {
        String::new()
    }
}

/// `read_approval_input`, panel edition: identical key semantics, but every
/// redraw renders the themed approval panel instead of the plain prompt.
async fn read_approval_input_panel(
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    stdin: &AsyncStdin,
    title: &str,
    session_label: &str,
    status: &crate::cli::render::StatusBarState<'_>,
) -> String {
    use crate::cli::input::InputLine;

    // Initial draw with empty input.
    let mut line = InputLine::new();
    let _ = renderer.draw_approval_panel(title, session_label, &line, status);

    // Read the first byte to decide: single-key shortcut vs. full line edit.
    if let Some(first) = stdin.read_byte().await {
        let ch = first as char;
        match ch.to_ascii_lowercase() {
            'y' | 'n' | 'a' => {
                // Show the key the user pressed in the input box, then return.
                line.insert(ch);
                let _ = renderer.draw_approval_panel(title, session_label, &line, status);
                ch.to_string()
            }
            '\r' | '\n' => {
                // Empty input (Enter pressed immediately)
                String::new()
            }
            _ => {
                // Start of a typed message — use the input editor.
                line.insert(ch);
                let _ = renderer.draw_approval_panel(title, session_label, &line, status);

                loop {
                    match stdin.read_byte().await {
                        Some(b'\r' | b'\n') => {
                            return line.as_str();
                        }
                        Some(b'\x7f' | b'\x08') => {
                            line.backspace();
                            let _ =
                                renderer.draw_approval_panel(title, session_label, &line, status);
                        }
                        Some(b'\x03') => {
                            // Ctrl+C — cancel, return empty
                            return String::new();
                        }
                        Some(b'\x1b') => {
                            // Escape — cancel, return empty
                            return String::new();
                        }
                        Some(b) => {
                            if b >= 0x20 {
                                line.insert(b as char);
                                let _ = renderer.draw_approval_panel(
                                    title,
                                    session_label,
                                    &line,
                                    status,
                                );
                            }
                        }
                        None => return line.as_str(),
                    }
                }
            }
        }
    } else {
        String::new()
    }
}

/// Display info text and prompt Y/N/A with typed-message support.
/// Returns `(approved, is_approve_session, user_message)`.
async fn prompt_with_session_approve(
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    stdin: &AsyncStdin,
    status: &crate::cli::render::StatusBarState<'_>,
    info_lines: &[&str],
    prompt_text: &str,
) -> (bool, bool, Option<String>) {
    // Commit info lines to scrollback (these are permanent).
    for line in info_lines {
        let _ = renderer.commit(&format!("{}\n", line));
    }
    // Read the decision in the live region.
    let input = read_approval_input(renderer, stdin, prompt_text, status).await;
    parse_approval_response(&input)
}

// ── Ratatui prompt functions (called from ask_with_session_ratatui) ──────────

pub(super) async fn prompt_tool_call_ratatui(
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    stdin: &AsyncStdin,
    status: &crate::cli::render::StatusBarState<'_>,
    approval: &mut super::approval::SessionApproval,
    command: &str,
    background: bool,
    target_pane: Option<&str>,
) -> anyhow::Result<(bool, Option<String>)> {
    use crate::daemon::utils::command_has_sudo;

    let where_label = if background {
        "daemon · runs silently"
    } else {
        "terminal · visible to you"
    };

    let mut body = vec![format!("$ {}", command)];
    if let Some(tp) = target_pane {
        body.push(format!("→ target: {}", tp));
    }
    let _ = renderer.commit_panel(where_label, &body, false);

    let is_sudo = command_has_sudo(command);
    let auto_approved = if is_sudo {
        approval.sudo
    } else {
        approval.regular
    };

    if auto_approved {
        let _ = renderer.commit("  ✓ auto-approved (session)\n");
        return Ok((true, None));
    }

    let session_label = if is_sudo { "sudo session" } else { "session" };
    let input =
        read_approval_input_panel(renderer, stdin, "approve command", session_label, status).await;
    let (approved, is_session, user_msg) = parse_approval_response(&input);

    if approved {
        if is_session {
            if is_sudo {
                approval.sudo = true;
            } else {
                approval.regular = true;
            }
            let kind = if is_sudo { "sudo" } else { "regular" };
            let _ = renderer.commit(&format!(
                "  ✓ approved — all {} commands auto-approved for this session\n",
                kind
            ));
        } else {
            let _ = renderer.commit("  ✓ approved\n");
        }
        Ok((true, None))
    } else if let Some(msg) = user_msg {
        let _ = renderer.commit("  ↩ redirecting agent with your message…\n");
        Ok((false, Some(msg)))
    } else {
        let _ = renderer.commit("  ✗ skipped\n");
        Ok((false, None))
    }
}

pub(super) async fn prompt_credential_ratatui(
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    stdin: &AsyncStdin,
    status: &crate::cli::render::StatusBarState<'_>,
    prompt: &str,
) -> String {
    let _ = renderer.commit(&format!("\n⚠ {}\n", prompt));

    // Read the credential in the live region, showing • for each char.
    // Two buffers: cred_real holds the actual typed value; cred_display holds masked bullets.
    let prompt_text = "  Password: ";
    let mut cred_real = String::new();
    let mut cred_display = crate::cli::input::InputLine::new();
    let _ = renderer.draw_credential_panel("sudo password", prompt, &cred_display, status);

    while let Some(b) = stdin.read_byte().await {
        match b {
            b'\r' | b'\n' => break,
            b'\x7f' | b'\x08' => {
                cred_real.pop();
                cred_display.backspace();
                let _ =
                    renderer.draw_credential_panel("sudo password", prompt, &cred_display, status);
            }
            b'\x03' | b'\x1b' => {
                cred_real.clear();
                break;
            }
            c if c >= 0x20 => {
                cred_real.push(c as char);
                cred_display.insert('•');
                let _ =
                    renderer.draw_credential_panel("sudo password", prompt, &cred_display, status);
            }
            _ => {}
        }
    }

    // Commit the final masked line to scrollback.
    let _ = renderer.commit(&format!("{}\n", prompt_text));
    cred_real
}

pub(super) async fn prompt_pane_select_ratatui(
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    stdin: &AsyncStdin,
    status: &crate::cli::render::StatusBarState<'_>,
    panes: &[crate::ipc::PaneInfo],
) -> String {
    let _ = renderer.commit("\n  ⚙ Which pane should receive this command?\n");
    for (i, pane) in panes.iter().enumerate() {
        let _ = renderer.commit(&format!(
            "  [{}]  {} — {} — {}\n",
            i + 1,
            pane.id,
            pane.cmd,
            pane.preview
        ));
    }
    let prompt_text = "  Select pane › ";
    let input = read_approval_input(renderer, stdin, prompt_text, status).await;
    input
        .trim()
        .parse::<usize>()
        .ok()
        .and_then(|n| panes.get(n.saturating_sub(1)).map(|p| p.id.clone()))
        .unwrap_or_else(|| panes.first().map(|p| p.id.clone()).unwrap_or_default())
}

pub(super) async fn prompt_yes_no_ratatui(
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    stdin: &AsyncStdin,
    status: &crate::cli::render::StatusBarState<'_>,
    info: &str,
    prompt_text: &str,
) -> bool {
    let _ = renderer.commit(&format!("\n  ⚙ {}\n", info));
    let full_prompt = format!("  {} [y/N] › ", prompt_text);
    let input = read_approval_input(renderer, stdin, &full_prompt, status).await;
    input.trim().to_lowercase() == "y" || input.trim().to_lowercase() == "yes"
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn prompt_write_ratatui<FAll, FInsert, FContains>(
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    stdin: &AsyncStdin,
    status: &crate::cli::render::StatusBarState<'_>,
    approval: &mut super::approval::SessionApproval,
    name: &str,
    content: &str,
    existing_content: Option<&str>,
    kind: &str,
    insert_for_session: FInsert,
    is_all_approved: FAll,
    is_name_approved: FContains,
) -> bool
where
    FAll: Fn(&super::approval::SessionApproval) -> bool,
    FInsert: Fn(&mut super::approval::SessionApproval, &str),
    FContains: Fn(&super::approval::SessionApproval, &str) -> bool,
{
    let _ = renderer.commit(&format!("\n  ⚙ AI wants to write {}: {}\n", kind, name));

    // Render diff
    let diff_lines = crate::cli::diff::render_diff(name, existing_content, content);
    for line in &diff_lines {
        let _ = renderer.commit(&format!("  {}\n", line));
    }

    let all_approved = is_all_approved(approval);
    let name_approved = is_name_approved(approval, name);

    if all_approved || name_approved {
        let _ = renderer.commit("  ✓ auto-approved (session)\n");
        return true;
    }

    let prompt_text = build_approval_prompt("session", false);
    let (approved, is_session, _user_msg) =
        prompt_with_session_approve(renderer, stdin, status, &[], &prompt_text).await;

    if approved {
        if is_session {
            insert_for_session(approval, name);
            let _ = renderer.commit(&format!(
                "  ✓ approved — edits to '{}' auto-approved for this session\n",
                name
            ));
        } else {
            let _ = renderer.commit("  ✓ approved\n");
        }
        true
    } else {
        let _ = renderer.commit("  ✗ denied\n");
        false
    }
}

pub(super) async fn prompt_schedule_write_ratatui(
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    stdin: &AsyncStdin,
    status: &crate::cli::render::StatusBarState<'_>,
    name: &str,
    kind: &str,
    action: &str,
) -> bool {
    let _ = renderer.commit(&format!(
        "\n  ⚙ AI wants to schedule: {} ({})\n  Action: {}\n",
        name, kind, action
    ));
    prompt_yes_no_ratatui(renderer, stdin, status, "", "Approve?").await
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn prompt_edit_file_ratatui(
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    stdin: &AsyncStdin,
    status: &crate::cli::render::StatusBarState<'_>,
    approval: &mut super::approval::SessionApproval,
    path: &str,
    operation: &str,
    existing_content: Option<&str>,
    new_content: Option<&str>,
    dest_path: Option<&str>,
) -> (bool, Option<String>) {
    let op_label = match operation {
        "create" => "create file",
        "delete" => "delete file",
        "copy" => "copy file",
        _ => "edit file",
    };
    let _ = renderer.commit(&format!("\n  ⚙ AI wants to {}: {}\n", op_label, path));
    if operation == "copy"
        && let Some(dst) = dest_path
    {
        let _ = renderer.commit(&format!("  → destination: {}\n", dst));
    }

    // Render diff
    let diff_name = if operation == "copy" {
        dest_path.unwrap_or(path)
    } else {
        path
    };
    let diff_existing = existing_content;
    let diff_new = new_content;
    let diff_lines =
        crate::cli::diff::render_diff(diff_name, diff_existing, diff_new.unwrap_or(""));
    for line in &diff_lines {
        let _ = renderer.commit(&format!("  {}\n", line));
    }

    let all_approved = approval.file_edits_all;
    let path_approved = approval.file_edits.contains(path);

    if all_approved || path_approved {
        let _ = renderer.commit("  ✓ auto-approved (session)\n");
        return (true, None);
    }

    let prompt_text = build_approval_prompt("session", true);
    let (approved, is_session, user_msg) =
        prompt_with_session_approve(renderer, stdin, status, &[], &prompt_text).await;

    if approved {
        if is_session && !all_approved {
            approval.file_edits.insert(path.to_string());
            let _ = renderer.commit(&format!(
                "  ✓ approved — edits to '{}' auto-approved for this session\n",
                path
            ));
        } else {
            let _ = renderer.commit("  ✓ approved\n");
        }
        (true, None)
    } else if let Some(msg) = user_msg {
        let _ = renderer.commit("  ↩ redirecting agent with your message…\n");
        (false, Some(msg))
    } else {
        let _ = renderer.commit("  ✗ skipped\n");
        (false, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_approval_decision ─────────────────────────────────────────

    #[test]
    fn parse_approval_decision_y_approves() {
        let (approved, is_session, msg) = parse_approval_response("y");
        assert!(approved);
        assert!(!is_session);
        assert!(msg.is_none());
    }

    #[test]
    fn parse_approval_decision_yes_approves() {
        let (approved, is_session, msg) = parse_approval_response("yes");
        assert!(approved);
        assert!(!is_session);
        assert!(msg.is_none());
    }

    #[test]
    fn parse_approval_decision_y_uppercase_approves() {
        let (approved, is_session, msg) = parse_approval_response("Y");
        assert!(approved);
        assert!(!is_session);
        assert!(msg.is_none());
    }

    #[test]
    fn parse_approval_decision_n_denies() {
        let (approved, is_session, msg) = parse_approval_response("n");
        assert!(!approved);
        assert!(!is_session);
        assert!(msg.is_none());
    }

    #[test]
    fn parse_approval_decision_empty_denies() {
        let (approved, is_session, msg) = parse_approval_response("");
        assert!(!approved);
        assert!(!is_session);
        assert!(msg.is_none());
    }

    #[test]
    fn parse_approval_decision_a_approves_session() {
        let (approved, is_session, msg) = parse_approval_response("a");
        assert!(approved);
        assert!(is_session);
        assert!(msg.is_none());
    }

    #[test]
    fn parse_approval_decision_typed_message_redirects() {
        let (approved, is_session, msg) = parse_approval_response("do X instead");
        assert!(!approved);
        assert!(!is_session);
        assert_eq!(msg, Some("do X instead".to_string()));
    }

    #[test]
    fn parse_approval_decision_typed_message_preserves_case() {
        let (approved, is_session, msg) = parse_approval_response("Fix the path please");
        assert!(!approved);
        assert!(!is_session);
        assert_eq!(msg, Some("Fix the path please".to_string()));
    }

    #[test]
    fn build_approval_prompt_session_with_redirect() {
        assert_eq!(
            build_approval_prompt("session", true),
            "  Approve? [Y]es  [A]pprove for session  [N]o  or type a message › "
        );
    }

    #[test]
    fn build_approval_prompt_sudo_session_with_redirect() {
        assert_eq!(
            build_approval_prompt("sudo session", true),
            "  Approve? [Y]es  [A]pprove for sudo session  [N]o  or type a message › "
        );
    }

    #[test]
    fn build_approval_prompt_session_without_redirect() {
        assert_eq!(
            build_approval_prompt("session", false),
            "  Approve? [Y]es  [A]pprove for session  [N]o  › "
        );
    }

    // ── silence_budget ─────────────────────────────────────────────────

    #[test]
    fn silence_budget_phase1_is_90s() {
        assert_eq!(
            silence_budget(false),
            std::time::Duration::from_secs(PHASE1_SILENCE_TIMEOUT_SECS)
        );
    }

    #[test]
    fn silence_budget_phase2_is_120s() {
        assert_eq!(
            silence_budget(true),
            std::time::Duration::from_secs(PHASE2_SILENCE_TIMEOUT_SECS)
        );
    }
}

#[cfg(test)]
mod stream_seam_tests {
    use super::*;
    use std::os::unix::io::{AsRawFd, FromRawFd};
    use tokio::net::UnixStream;

    /// A pipe-backed `AsyncStdin` plus the write end (as a `File`) to feed bytes.
    fn make_pipe_stdin() -> (AsyncStdin, std::fs::File) {
        let mut fds: [libc::c_int; 2] = [-1, -1];
        let ret = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_NONBLOCK) };
        assert_eq!(ret, 0, "pipe2 failed: {}", std::io::Error::last_os_error());
        let stdin = AsyncStdin::from_raw_fd(fds[0]).expect("from_raw_fd");
        let write_file = unsafe { std::fs::File::from_raw_fd(fds[1]) };
        (stdin, write_file)
    }

    async fn write_bytes(file: &std::fs::File, bytes: &[u8]) {
        let fd = file.as_raw_fd();
        let mut remaining = bytes;
        while !remaining.is_empty() {
            let n = unsafe {
                libc::write(
                    fd,
                    remaining.as_ptr() as *const libc::c_void,
                    remaining.len(),
                )
            };
            if n > 0 {
                remaining = &remaining[n as usize..];
            } else {
                // A short write on this pipe can only mean EAGAIN; anything else is
                // a real bug and must fail loudly rather than spin forever.
                let err = std::io::Error::last_os_error();
                assert_eq!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock,
                    "write to test pipe failed: {err}"
                );
                tokio::task::yield_now().await;
            }
        }
    }

    /// The regression guard for bug-phase-11-1 / bug-phase-11-2: a daemon read
    /// that is interrupted (dropped) mid-line must NOT lose the partial bytes —
    /// the next read completes the same message intact.
    #[tokio::test]
    async fn recv_line_preserves_partial_bytes_across_a_dropped_read() {
        let (client, server) = UnixStream::pair().unwrap();
        let (read_half, _write_half) = client.into_split();
        let mut rx = BufReader::new(read_half);
        let (_srv_r, mut srv_w) = server.into_split();

        // The full wire line is one serialized Response + '\n'.
        let wire = serde_json::to_string(&Response::Token("hello".to_string())).unwrap();
        let first_half = &wire[..wire.len() / 2];
        let second_half = &wire[wire.len() / 2..];

        let mut buf: Vec<u8> = Vec::new();

        // Send only the first half — no newline yet.
        use tokio::io::AsyncWriteExt;
        srv_w.write_all(first_half.as_bytes()).await.unwrap();

        // Drive recv_line until it has consumed the partial bytes, then drop it
        // by timing out — exactly what a Warn/Tick return does in select_stream.
        let dropped = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            recv_line(&mut rx, &mut buf),
        )
        .await;
        assert!(
            dropped.is_err(),
            "read should still be pending (no newline yet)"
        );
        assert!(
            !buf.is_empty(),
            "partial bytes must survive the dropped read, got empty buf"
        );

        // Now send the rest + newline; the next read must complete the message.
        srv_w.write_all(second_half.as_bytes()).await.unwrap();
        srv_w.write_all(b"\n").await.unwrap();

        let msg = recv_line(&mut rx, &mut buf).await.expect("recv_line ok");
        match msg {
            Response::Token(t) => assert_eq!(t, "hello", "message reassembled intact"),
            other => panic!("expected Token, got {other:?}"),
        }
        assert!(buf.is_empty(), "buf cleared after a complete line");
    }

    /// With the daemon idle (read pending) and an interrupt key queued,
    /// `select_stream` returns `Warn` on the first press without touching the
    /// stream — and the buffer is untouched.
    #[tokio::test]
    async fn select_stream_first_interrupt_press_warns() {
        let (client, _server) = UnixStream::pair().unwrap();
        let (read_half, _w) = client.into_split();
        let mut rx = BufReader::new(read_half);
        let mut buf: Vec<u8> = Vec::new();
        let mut state = InterruptState::new();

        let (stdin, wf) = make_pipe_stdin();
        write_bytes(&wf, &[0x03]).await; // Ctrl+C

        let mut sigwinch =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change()).unwrap();

        let outcome = select_stream(
            &stdin,
            &mut rx,
            &mut buf,
            &mut state,
            std::time::Duration::MAX, // no spinner tick
            Some(std::time::Duration::from_secs(120)),
            &mut sigwinch,
        )
        .await;

        assert!(matches!(outcome, StreamOutcome::Warn), "got {outcome:?}");
        assert!(state.is_armed());
        assert!(buf.is_empty());
    }

    /// With the keyboard idle and a full daemon line ready, `select_stream`
    /// returns the parsed `Msg`.
    #[tokio::test]
    async fn select_stream_delivers_a_full_daemon_message() {
        let (client, server) = UnixStream::pair().unwrap();
        let (read_half, _w) = client.into_split();
        let mut rx = BufReader::new(read_half);
        let (_srv_r, mut srv_w) = server.into_split();
        let mut buf: Vec<u8> = Vec::new();
        let mut state = InterruptState::new();

        let wire = serde_json::to_string(&Response::Token("hi".to_string())).unwrap();
        use tokio::io::AsyncWriteExt;
        srv_w.write_all(wire.as_bytes()).await.unwrap();
        srv_w.write_all(b"\n").await.unwrap();

        // stdin write end stays open with no bytes → read_key parks forever, so
        // only the daemon-read branch can fire.
        let (stdin, _wf) = make_pipe_stdin();

        let mut sigwinch =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change()).unwrap();

        let outcome = select_stream(
            &stdin,
            &mut rx,
            &mut buf,
            &mut state,
            std::time::Duration::MAX,
            Some(std::time::Duration::from_secs(120)),
            &mut sigwinch,
        )
        .await;

        match outcome {
            StreamOutcome::Msg(m) => match *m {
                Response::Token(t) => assert_eq!(t, "hi"),
                other => panic!("expected Token, got {other:?}"),
            },
            other => panic!("expected Msg, got {other:?}"),
        }
    }

    // ── tool_runtime_label ──────────────────────────────────────────────

    #[test]
    fn tool_runtime_label_formats_ok_and_err() {
        assert_eq!(tool_runtime_label(true, 1234), "✓ 1.2s");
        assert_eq!(tool_runtime_label(false, 450), "✗ 0.5s");
    }

    // ── focus_outcome ───────────────────────────────────────────────────

    #[test]
    fn focus_outcome_maps_focus_gained_to_reanchor() {
        let result = focus_outcome(&Key::FocusGained);
        assert!(matches!(result, Some(StreamOutcome::Reanchor)));
        assert!(focus_outcome(&Key::Char('x')).is_none());
    }

    // ── select_stream focus / sigwinch ──────────────────────────────────

    #[tokio::test]
    async fn select_stream_focus_gained_returns_reanchor() {
        let (client, _server) = UnixStream::pair().unwrap();
        let (read_half, _w) = client.into_split();
        let mut rx = BufReader::new(read_half);
        let mut buf: Vec<u8> = Vec::new();
        let mut state = InterruptState::new();

        let (stdin, wf) = make_pipe_stdin();
        // ESC [ I = focus gained
        write_bytes(&wf, b"\x1b[I").await;

        let mut sigwinch =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change()).unwrap();

        let outcome = select_stream(
            &stdin,
            &mut rx,
            &mut buf,
            &mut state,
            std::time::Duration::MAX,
            Some(std::time::Duration::from_secs(120)),
            &mut sigwinch,
        )
        .await;

        match outcome {
            StreamOutcome::Reanchor => {} // expected
            other => panic!("expected Reanchor, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn select_stream_sigwinch_returns_reanchor() {
        let (client, _server) = UnixStream::pair().unwrap();
        let (read_half, _w) = client.into_split();
        let mut rx = BufReader::new(read_half);
        let mut buf: Vec<u8> = Vec::new();
        let mut state = InterruptState::new();

        // stdin write end stays open with no bytes → read_key parks forever
        let (stdin, _wf) = make_pipe_stdin();

        let mut sigwinch =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change()).unwrap();

        // Spawn the select so it can receive the signal
        let select_handle = tokio::spawn(async move {
            select_stream(
                &stdin,
                &mut rx,
                &mut buf,
                &mut state,
                std::time::Duration::MAX,
                Some(std::time::Duration::from_secs(120)),
                &mut sigwinch,
            )
            .await
        });

        // Small delay to let the select settle
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Raise SIGWINCH to the current process
        unsafe { libc::raise(libc::SIGWINCH) };

        let outcome = select_handle.await.unwrap();
        match outcome {
            StreamOutcome::Reanchor => {} // expected
            other => panic!("expected Reanchor, got {other:?}"),
        }
    }
}

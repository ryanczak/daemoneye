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
use crate::config::Config;
use crate::ipc::{Request, Response};

use super::approval::SessionApproval;
use super::interrupt::{InterruptAction, InterruptState};
use super::ipc_client::{connect, send_request};

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
    /// Daemon error (EOF, parse failure, timeout).
    Error(String),
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

    loop {
        // Both phases animate a spinner on an 80 ms tick so a mid-stream pause
        // (e.g. a tool round-trip or a slow model) never looks frozen. Phase 1
        // (before the first content) has no overall timeout; phase 2 keeps a
        // 120 s deadline measured from the last message via `last_msg_at`.
        let (tick_interval, overall_timeout) = if !response_started {
            (std::time::Duration::from_millis(80), None)
        } else {
            let remaining =
                std::time::Duration::from_secs(120).saturating_sub(last_msg_at.elapsed());
            (std::time::Duration::from_millis(80), Some(remaining))
        };
        let outcome = select_stream(
            stdin,
            &mut rx,
            &mut line_buf,
            &mut interrupt_state,
            tick_interval,
            overall_timeout,
        )
        .await;

        match outcome {
            StreamOutcome::Interrupted => {
                let _ = renderer.commit_panel("result", &["⊘ interrupted".to_string()], true);
                interrupt_state.reset();
                break;
            }
            StreamOutcome::Error(e) => {
                return Err(anyhow::anyhow!("Connection error: {}", e));
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
                if !summary.is_empty() {
                    let _ = renderer.commit_panel(&tool, &[format!("▸ {}", summary)], false);
                } else {
                    let _ = renderer.commit_panel(&tool, &["▸ running".to_string()], false);
                }
            }
            Response::ToolFinished { ok, elapsed_ms, .. } => {
                let status = if ok { "✓" } else { "✗" };
                let secs = elapsed_ms as f64 / 1000.0;
                let _ =
                    renderer.commit_panel("result", &[format!("{} ({:.1}s)", status, secs)], true);
            }
            Response::ToolResult(output) => {
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

    // Update approval from config in case it changed during the turn.
    {
        let cfg = Config::load().unwrap_or_default();
        *approval = SessionApproval::from_config(&cfg.approvals);
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
) -> StreamOutcome {
    let mut timeout_fut = overall_timeout.map(|d| Box::pin(tokio::time::sleep(d)));

    loop {
        if let Some(ref mut to) = timeout_fut {
            tokio::select! {
                key = read_key(stdin) => {
                    if let Some(key) = key {
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
                    return StreamOutcome::Error("Daemon stopped responding (120 s timeout)".to_string());
                }
                _ = tokio::time::sleep(tick_interval), if tick_interval != std::time::Duration::MAX => {
                    return StreamOutcome::Tick;
                }
            }
        } else {
            tokio::select! {
                key = read_key(stdin) => {
                    if let Some(key) = key {
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
            }
        }
    }
}

// ── Ratatui interactive approval primitives ──────────────────────────────────

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
    let prompt_text = format!(
        "  Approve? [Y]es  [N]o  [A]pprove for {}  or type a message › ",
        session_label
    );
    let (approved, is_session, user_msg) =
        prompt_with_session_approve(renderer, stdin, status, &[], &prompt_text).await;

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
    let _ = renderer.draw_prompt(prompt_text, &cred_display, status);

    while let Some(b) = stdin.read_byte().await {
        match b {
            b'\r' | b'\n' => break,
            b'\x7f' | b'\x08' => {
                cred_real.pop();
                cred_display.backspace();
                let _ = renderer.draw_prompt(prompt_text, &cred_display, status);
            }
            b'\x03' | b'\x1b' => {
                cred_real.clear();
                break;
            }
            c if c >= 0x20 => {
                cred_real.push(c as char);
                cred_display.insert('•');
                let _ = renderer.draw_prompt(prompt_text, &cred_display, status);
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
            pane.current_cmd,
            pane.summary
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

    let has_a = !all_approved;
    let prompt_text = if has_a {
        "  Approve? [Y]es  [A]pprove for session  [N]o  › ".to_string()
    } else {
        "  Approve? [Y]es  [N]o  › ".to_string()
    };
    let (approved, is_session, _user_msg) =
        prompt_with_session_approve(renderer, stdin, status, &[], &prompt_text).await;

    if approved {
        if is_session && has_a {
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

    let prompt_text = if all_approved {
        "  Approve? [Y]es  [N]o  › ".to_string()
    } else {
        "  Approve? [Y]es  [A]pprove for session  [N]o  or type a message › ".to_string()
    };
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
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
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

        let outcome = select_stream(
            &stdin,
            &mut rx,
            &mut buf,
            &mut state,
            std::time::Duration::MAX, // no spinner tick
            Some(std::time::Duration::from_secs(120)),
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

        let outcome = select_stream(
            &stdin,
            &mut rx,
            &mut buf,
            &mut state,
            std::time::Duration::MAX,
            Some(std::time::Duration::from_secs(120)),
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
}

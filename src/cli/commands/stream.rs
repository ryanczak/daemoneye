//! Streaming response rendering for chat and ask flows.
//!
//! Owns `ask_with_session`, the long-lived loop that consumes `Response`
//! events from the daemon and renders them (tokens, tool panels, approval
//! prompts) while handling SIGWINCH-driven resizes and the status bar.

use anyhow::Result;
use tokio::io::BufReader;

use crate::cli::input::*;
use crate::cli::render::*;
use crate::config::Config;
use crate::ipc::{Request, Response};

use super::approval::SessionApproval;
use super::ipc_client::{connect, recv, send_request};

// ── AI conversation ─────────────────────────────────────────────────────────

/// Context for SIGWINCH handling during streaming in `ask_with_session`.
pub(super) struct StreamResizeDims<'a> {
    pub(super) width: &'a mut usize,
    pub(super) height: &'a mut usize,
    pub(super) start: std::time::Instant,
    pub(super) model: String,
    pub(super) daemon_up: bool,
    /// True when the input frame (borders + status bar) is currently drawn.
    /// When false, only dimensions are updated; caller redraws after streaming.
    pub(super) has_frame: bool,
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

pub(super) struct QueryArgs<'a> {
    pub(super) query: String,
    pub(super) display_query: &'a str,
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

pub(super) struct StreamCtx<'a> {
    pub(super) stdin: &'a AsyncStdin,
    pub(super) chat_width: Option<usize>,
    pub(super) old_termios: libc::termios,
    pub(super) sigwinch: Option<&'a mut tokio::signal::unix::Signal>,
    pub(super) resize: Option<StreamResizeDims<'a>>,
}

pub(super) async fn ask_with_session(
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
            model: None,
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
                            let revoke_cfg = Config::load().unwrap_or_default();
                            *approval = SessionApproval::from_config(&revoke_cfg.approvals);
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
                            let revoke_cfg = Config::load().unwrap_or_default();
                            *approval = SessionApproval::from_config(&revoke_cfg.approvals);
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
                let (approved, user_message) = super::approval_ui::prompt_tool_call(
                    super::approval_ui::PromptCtx {
                        stdin,
                        old_termios,
                        md: &mut md,
                        response_started: &mut response_started,
                        approval,
                        resize: &resize,
                        session_id,
                        prompt_tokens: *prompt_tokens,
                        context_window,
                    },
                    command,
                    background,
                    target_pane,
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
                let credential = super::approval_ui::prompt_credential(&mut md, &prompt);
                send_request(&mut tx, Request::CredentialResponse { id, credential }).await?;
            }
            Response::PaneSelectPrompt { id, panes } => {
                let pane_id = super::approval_ui::prompt_pane_select(
                    super::approval_ui::PromptCtx {
                        stdin,
                        old_termios,
                        md: &mut md,
                        response_started: &mut response_started,
                        approval,
                        resize: &resize,
                        session_id,
                        prompt_tokens: *prompt_tokens,
                        context_window,
                    },
                    panes,
                )
                .await?;
                send_request(&mut tx, Request::PaneSelectResponse { id, pane_id }).await?;
            }
            Response::ScriptDeletePrompt { id, script_name } => {
                let approved = super::approval_ui::prompt_script_delete(
                    super::approval_ui::PromptCtx {
                        stdin,
                        old_termios,
                        md: &mut md,
                        response_started: &mut response_started,
                        approval,
                        resize: &resize,
                        session_id,
                        prompt_tokens: *prompt_tokens,
                        context_window,
                    },
                    &script_name,
                )
                .await?;
                send_request(&mut tx, Request::ScriptDeleteResponse { id, approved }).await?;
            }
            Response::ScriptWritePrompt {
                id,
                script_name,
                content,
                existing_content,
            } => {
                let approved = super::approval_ui::prompt_script_write(
                    super::approval_ui::PromptCtx {
                        stdin,
                        old_termios,
                        md: &mut md,
                        response_started: &mut response_started,
                        approval,
                        resize: &resize,
                        session_id,
                        prompt_tokens: *prompt_tokens,
                        context_window,
                    },
                    &script_name,
                    &content,
                    existing_content.as_deref(),
                )
                .await?;
                send_request(&mut tx, Request::ScriptWriteResponse { id, approved }).await?;
            }
            Response::ScheduleWritePrompt {
                id,
                name,
                kind,
                action,
            } => {
                let approved = super::approval_ui::prompt_schedule_write(
                    super::approval_ui::PromptCtx {
                        stdin,
                        old_termios,
                        md: &mut md,
                        response_started: &mut response_started,
                        approval,
                        resize: &resize,
                        session_id,
                        prompt_tokens: *prompt_tokens,
                        context_window,
                    },
                    &name,
                    &kind,
                    &action,
                )
                .await?;
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
                existing_content,
            } => {
                let approved = super::approval_ui::prompt_runbook_write(
                    super::approval_ui::PromptCtx {
                        stdin,
                        old_termios,
                        md: &mut md,
                        response_started: &mut response_started,
                        approval,
                        resize: &resize,
                        session_id,
                        prompt_tokens: *prompt_tokens,
                        context_window,
                    },
                    &runbook_name,
                    &content,
                    existing_content.as_deref(),
                )
                .await?;
                send_request(&mut tx, Request::RunbookWriteResponse { id, approved }).await?;
            }
            Response::EditFilePrompt {
                id,
                path,
                operation,
                existing_content,
                new_content,
                dest_path,
            } => {
                let (approved, user_message) = super::approval_ui::prompt_edit_file(
                    super::approval_ui::PromptCtx {
                        stdin,
                        old_termios,
                        md: &mut md,
                        response_started: &mut response_started,
                        approval,
                        resize: &resize,
                        session_id,
                        prompt_tokens: *prompt_tokens,
                        context_window,
                    },
                    &path,
                    &operation,
                    existing_content.as_deref(),
                    new_content.as_deref(),
                    dest_path.as_deref(),
                )
                .await?;
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
            Response::RunbookDeletePrompt {
                id,
                runbook_name,
                active_jobs,
            } => {
                let approved = super::approval_ui::prompt_runbook_delete(
                    super::approval_ui::PromptCtx {
                        stdin,
                        old_termios,
                        md: &mut md,
                        response_started: &mut response_started,
                        approval,
                        resize: &resize,
                        session_id,
                        prompt_tokens: *prompt_tokens,
                        context_window,
                    },
                    &runbook_name,
                    &active_jobs,
                )
                .await?;
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

            Response::DaemonStatus { .. } => {
                // Not expected in the AI streaming loop; ignore.
            }
            Response::ModelChanged { model } => {
                if !response_started {
                    print!("\r\x1b[K");
                    response_started = true;
                }
                md.flush();
                println!(
                    "\n  \x1b[32m✓\x1b[0m Active model switched to \x1b[96m{}\x1b[0m",
                    model
                );
                println!();
                md.reset();
            }
            Response::ModelList { models, active } => {
                if !response_started {
                    print!("\r\x1b[K");
                    response_started = true;
                }
                md.flush();
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
                md.reset();
            }
            Response::PaneChanged { .. } | Response::PaneList { .. } => {
                // These are handled synchronously by the /pane slash command
                // path and should not arrive during a streaming AI turn.
            }
            Response::LimitsInfo { .. }
            | Response::SessionSaved { .. }
            | Response::SessionLoaded { .. }
            | Response::SavedSessionList { .. }
            | Response::SessionDiff { .. } => {}
        }
    }

    Ok(())
}

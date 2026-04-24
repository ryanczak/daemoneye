//! `daemoneye ask` — one-shot query to the daemon. Two modes:
//!
//! * default: spinner + styled streaming output via `ask_with_session` (shares the
//!   chat rendering pipeline).
//! * `--raw`: plain-stdout streaming, auto-denies all interactive prompts.
//!   Intended for scripting and piping.

use anyhow::Result;
use tokio::io::BufReader;

use crate::cli::input::{AsyncStdin, set_raw_mode};
use crate::cli::render::terminal_width;
use crate::config::Config;
use crate::ipc::{Request, Response};

use super::approval::SessionApproval;
use super::ipc_client::{connect, recv, send_request};
use super::stream::{AskTmuxCtx, QueryArgs, StreamCtx, TokenCtx, ask_with_session};

pub async fn run_ask(query: String, raw: bool) -> Result<()> {
    if raw {
        return run_ask_raw(query).await;
    }

    let stdin = AsyncStdin::new()?;
    let ask_config = Config::load().unwrap_or_default();
    let mut approval = SessionApproval::from_config(&ask_config.approvals);

    let old = set_raw_mode()?;
    let tmux_session = crate::tmux::current_session_name();
    // For one-shot asks the user's current pane IS the working pane; no split/discovery needed.
    let result = ask_with_session(
        QueryArgs {
            query: query.clone(),
            display_query: &query,
            prompt_override: None,
        },
        None,
        &mut approval,
        AskTmuxCtx {
            session: tmux_session.as_deref(),
            pane: None,
        },
        TokenCtx {
            prompt_tokens: &mut 0,
            context_window: 0,
        },
        StreamCtx {
            stdin: &stdin,
            chat_width: Some(terminal_width()),
            old_termios: old,
            sigwinch: None,
            resize: None,
        },
    )
    .await;
    crate::cli::input::restore_termios(old);
    result
}

/// Minimal ask: sends the query, prints only the agent's response tokens to stdout,
/// and auto-denies any tool calls or interactive prompts. No spinner, no decorations.
/// Intended for scripting and piping.
async fn run_ask_raw(query: String) -> Result<()> {
    use std::io::Write;
    use std::time::Duration;

    let tmux_session = crate::tmux::current_session_name();
    let tmux_pane = std::env::var("TMUX_PANE").ok();
    let chat_pane = tmux_pane.clone();

    let stream = connect().await?;
    let (rx, mut tx) = stream.into_split();
    let mut rx = BufReader::new(rx);

    send_request(
        &mut tx,
        Request::Ask {
            query,
            tmux_pane,
            session_id: None,
            chat_pane,
            prompt: None,
            chat_width: None,
            tmux_session: tmux_session.map(|s| s.to_string()),
            target_pane: None,
            model: None,
        },
    )
    .await?;

    loop {
        let msg = tokio::time::timeout(Duration::from_secs(120), recv(&mut rx))
            .await
            .map_err(|_| anyhow::anyhow!("Daemon stopped responding (120 s timeout)"))?
            .map_err(|e| anyhow::anyhow!("Connection error: {}", e))?;

        match msg {
            Response::KeepAlive => continue,
            Response::Ok => {
                println!();
                break;
            }
            Response::Error(e) => {
                eprintln!("{}", e);
                anyhow::bail!("{}", e);
            }
            Response::Token(t) => {
                print!("{}", t);
                std::io::stdout().flush()?;
            }
            // Auto-deny tool calls — daemon will inform the AI and it will respond in text.
            Response::ToolCallPrompt { id, .. } => {
                send_request(
                    &mut tx,
                    Request::ToolCallResponse {
                        id,
                        approved: false,
                        user_message: None,
                    },
                )
                .await?;
            }
            // Auto-deny all other interactive prompts.
            Response::CredentialPrompt { id, .. } => {
                send_request(
                    &mut tx,
                    Request::CredentialResponse {
                        id,
                        credential: String::new(),
                    },
                )
                .await?;
            }
            Response::PaneSelectPrompt { id, panes } => {
                let pane_id = panes.into_iter().next().map(|p| p.id).unwrap_or_default();
                send_request(&mut tx, Request::PaneSelectResponse { id, pane_id }).await?;
            }
            Response::ScriptDeletePrompt { id, .. } => {
                send_request(
                    &mut tx,
                    Request::ScriptDeleteResponse {
                        id,
                        approved: false,
                    },
                )
                .await?;
            }
            Response::ScriptWritePrompt { id, .. } => {
                send_request(
                    &mut tx,
                    Request::ScriptWriteResponse {
                        id,
                        approved: false,
                    },
                )
                .await?;
            }
            Response::ScheduleWritePrompt { id, .. } => {
                send_request(
                    &mut tx,
                    Request::ScheduleWriteResponse {
                        id,
                        approved: false,
                    },
                )
                .await?;
            }
            Response::RunbookWritePrompt { id, .. } => {
                send_request(
                    &mut tx,
                    Request::RunbookWriteResponse {
                        id,
                        approved: false,
                    },
                )
                .await?;
            }
            Response::EditFilePrompt { id, .. } => {
                send_request(
                    &mut tx,
                    Request::EditFileResponse {
                        id,
                        approved: false,
                        user_message: None,
                    },
                )
                .await?;
            }
            Response::RunbookDeletePrompt { id, .. } => {
                send_request(
                    &mut tx,
                    Request::RunbookDeleteResponse {
                        id,
                        approved: false,
                    },
                )
                .await?;
            }
            // Informational responses — silently skip.
            Response::SessionInfo { .. }
            | Response::UsageUpdate { .. }
            | Response::SystemMsg(_)
            | Response::ToolResult(_)
            | Response::ScheduleList { .. }
            | Response::ScriptList { .. }
            | Response::RunbookList { .. }
            | Response::ModelChanged { .. }
            | Response::ModelList { .. }
            | Response::PaneChanged { .. }
            | Response::PaneList { .. }
            | Response::DaemonStatus { .. }
            | Response::LimitsInfo { .. }
            | Response::SessionSaved { .. }
            | Response::SessionLoaded { .. }
            | Response::SavedSessionList { .. }
            | Response::SessionDiff { .. } => {}
        }
    }

    Ok(())
}

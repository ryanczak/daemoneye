use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::BufReader;
use crate::util::UnpoisonExt;

use crate::ai::{AiEvent, Message, PendingCall, ToolResult, make_client};
use crate::config::{Config, load_named_prompt};
use crate::daemon::session::{SessionEntry, SessionStore, append_session_message};
use crate::daemon::utils::daemon_hostname;
use crate::runbook::Runbook;
use crate::scheduler::ScheduleStore;
use crate::sys_context::get_or_init_sys_context;
use crate::tmux;
use crate::tmux::cache::SessionCache;
use crate::tmux::ensure_incident_session;

/// Return `true` if another ghost shell may be started without exceeding the
/// configured concurrency limit.
///
/// A `max_concurrent_ghosts` of 0 disables the cap entirely (always returns `true`).
pub fn check_ghost_capacity(config: &crate::config::Config) -> bool {
    let max = config.ghost.max_concurrent_ghosts;
    if max == 0 {
        return true;
    }
    crate::daemon::stats::get_ghosts_active() < max
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_zero_disables_cap() {
        let mut config = crate::config::Config::default();
        config.ghost.max_concurrent_ghosts = 0;
        // Even with many active ghosts, should always allow.
        assert!(check_ghost_capacity(&config));
    }

    #[test]
    fn capacity_allows_when_under_limit() {
        let mut config = crate::config::Config::default();
        config.ghost.max_concurrent_ghosts = 100; // very high ceiling
        // Active count starts at 0, so we're well under the limit.
        assert!(check_ghost_capacity(&config));
    }
}

/// Orchestrates the lifecycle of an autonomous Ghost Shell.
pub struct GhostManager;

impl GhostManager {
    /// Start a new Ghost Shell for a specific alert and runbook.
    ///
    /// 1. Ensures a host tmux session exists (active or detached).
    /// 2. Initializes a new ghost `SessionEntry` with the alert as the first user turn.
    ///    Background windows are created lazily on the first tool call, prefixed with
    ///    `bg_prefix` (e.g. `GS_BG_WINDOW_PREFIX` for webhook/interactive ghosts,
    ///    `GS_SCHED_WINDOW_PREFIX` for scheduler-triggered ghosts).
    /// 3. Returns the session ID for use by `trigger_ghost_turn`.
    pub async fn start_session(
        sessions: SessionStore,
        runbook: &Runbook,
        alert_msg: &str,
        bg_prefix: &'static str,
    ) -> Result<String> {
        let alert_name = &runbook.name;
        
        // 1. Ensure host tmux session exists (active or detached)
        let tmux_session = ensure_incident_session()
            .context("GhostManager: failed to ensure incident session")?;
        
        // 2. Initialize ghost shell entry
        let session_id = format!("ghost-{}-{}", alert_name, uuid::Uuid::new_v4().simple());
        
        let mut messages = Vec::new();

        // The alert payload is the first user turn.  Ghost behavioral instructions
        // (autonomous mode, background-only execution, no human present) live in the
        // system prompt assembled by `trigger_ghost_turn`, not here.  Putting them in
        // an assistant-role message causes the Anthropic API to reject the request
        // because conversations must begin with a user turn.
        let user_msg = Message {
            role: "user".to_string(),
            content: format!("Incoming alert:\n{}", alert_msg),
            tool_calls: None,
            tool_results: None,
        };
        messages.push(user_msg);

        let entry = SessionEntry {
            messages,
            last_accessed: Instant::now(),
            chat_pane: None,
            default_target_pane: None, // Ghost shells use background windows exclusively
            bg_windows: Vec::new(),
            last_prompt_tokens: 0,
            tmux_session: tmux_session.clone(),
            last_detach: None,
            messages_at_detach: 0,
            pipe_source_pane: None,
            is_ghost: true,
            ghost_config: Some(runbook.ghost_config.clone()),
            ghost_bg_prefix: bg_prefix,
        };

        {
            let mut store = sessions.lock().unwrap_or_log();
            store.insert(session_id.clone(), entry);
        }

        crate::daemon::stats::inc_ghosts_launched();

        log::info!(
            "Ghost Shell started: {} (alert: {}, session: {})",
            session_id,
            alert_name,
            tmux_session
        );

        Ok(session_id)
    }
}
/// Trigger a headless AI turn for a Ghost Shell.
///
/// This simulates a user's `Ask` request but without an attached terminal.
/// Results and tool outcomes are persisted to the session file.
pub async fn trigger_ghost_turn(
    session_id: &str,
    sessions: &SessionStore,
    config: &Config,
    cache: &Arc<SessionCache>,
    schedule_store: &Arc<ScheduleStore>,
) -> Result<()> {
    let (_messages, _ghost_config, tmux_session, _target_pane) = {
        let store = sessions.lock().unwrap_or_log();
        let Some(entry) = store.get(session_id) else {
            anyhow::bail!("Ghost Shell '{}' not found", session_id);
        };
        (
            entry.messages.clone(),
            entry.ghost_config.clone(),
            entry.tmux_session.clone(),
            entry.default_target_pane.clone(),
        )
    };

    let prompt_name = config.ai.prompt.clone();
    let system_base = load_named_prompt(&prompt_name).system;
    let sys_context = get_or_init_sys_context();

    let daemon_ceiling = config.ghost.max_ghost_turns;
    let (approved_scripts, run_with_sudo, max_ghost_turns, ssh_target) = {
        let store = sessions.lock().unwrap_or_log();
        store.get(session_id).and_then(|e| e.ghost_config.as_ref()).map(|gc| {
            let scripts = if gc.auto_approve_scripts.is_empty() {
                "none".to_string()
            } else {
                gc.auto_approve_scripts.join(", ")
            };
            let turns = if gc.max_ghost_turns > 0 {
                gc.max_ghost_turns.min(daemon_ceiling)
            } else {
                daemon_ceiling
            };
            (scripts, gc.run_with_sudo, turns, gc.ssh_target.clone())
        }).unwrap_or_else(|| ("none".to_string(), false, daemon_ceiling, None))
    };
    let remote_line = if let Some(ref target) = ssh_target {
        format!(
            "Remote SSH Target: {} — all commands are automatically wrapped in \
             `ssh {}` and executed on this host. \
             Do NOT manually SSH to the target; call run_terminal_command with the \
             command directly and the daemon handles SSH transparently.\n         ",
            target, target
        )
    } else {
        String::new()
    };
    let system = format!(
        "{}\n\n\
         ## Ghost Shell Execution Context\n\
         You are operating autonomously — no human user is present.\n\
         All terminal commands MUST use background mode (they run in de-gs-bg-* or de-gs-sj-* windows).\n\
         Do NOT ask questions or wait for user input.\n\
         Daemon Host: {}\n\
         Tmux Session: {}\n\
         {}Command Policy: non-sudo commands run freely (OS permissions are the boundary). \
         Sudo commands require a pre-approved script via install-sudoers.\n\
         Pre-approved Sudo Scripts: {}{}\n\
         Turn Budget: {} (hard limit — shell will be stopped when reached)\n\n\
         {}",
        system_base,
        daemon_hostname(),
        tmux_session,
        remote_line,
        approved_scripts,
        if run_with_sudo { " (executed with sudo)" } else { "" },
        max_ghost_turns,
        sys_context.format_for_ai()
    );

    if !tmux::session_exists(&tmux_session) {
        anyhow::bail!(
            "Ghost Shell {}: tmux session '{}' no longer exists",
            session_id,
            tmux_session
        );
    }

    let (tx_duplex, _rx_duplex) = tokio::io::duplex(4096);
    let (rx_half, tx_half) = tokio::io::split(tx_duplex);
    let mut tx = tx_half;
    let mut rx = BufReader::new(rx_half);

    let api_key = config.ai.resolve_api_key();
    let client: Arc<Box<dyn crate::ai::AiClient>> = Arc::new(make_client(
        &config.ai.provider,
        api_key,
        config.ai.model.clone(),
        config.ai.effective_base_url(),
    ));

    const GHOST_TURN_TIMEOUT_SECS: u64 = 300;

    let mut turn = 0usize;
    loop {
        if turn >= max_ghost_turns {
            log::warn!(
                "Ghost Shell {}: reached max turns ({}), stopping",
                session_id, max_ghost_turns
            );
            break;
        }
        turn += 1;

        let chat_messages = {
            let store = sessions.lock().unwrap_or_log();
            let Some(entry) = store.get(session_id) else { break; };
            entry.messages.clone()
        };

        let client_clone = Arc::clone(&client);
        let system_clone = system.clone();

        let (ai_tx, mut ai_rx) = tokio::sync::mpsc::unbounded_channel::<AiEvent>();

        tokio::spawn(async move {
            if let Err(e) = client_clone.chat(&system_clone, chat_messages, ai_tx, true).await {
                log::error!("Ghost Shell AI error: {}", e);
            }
        });

        let mut assistant_content = String::new();
        let mut pending_calls = Vec::new();

        let deadline = tokio::time::Instant::now() + Duration::from_secs(GHOST_TURN_TIMEOUT_SECS);

        loop {
            match tokio::time::timeout_at(deadline, ai_rx.recv()).await {
                Err(_elapsed) => {
                    log::error!(
                        "Ghost Shell {}: turn {} timed out after {}s",
                        session_id, turn, GHOST_TURN_TIMEOUT_SECS
                    );
                    anyhow::bail!("ghost turn timed out");
                }
                Ok(None) => break,
                Ok(Some(ev)) => match ev {
                    AiEvent::Token(t) => {
                        assistant_content.push_str(&t);
                    }
                    AiEvent::ToolCall(id, command, _background, _target_pane, retry_in_pane, thought_signature) => {
                        pending_calls.push(PendingCall::Background {
                            id,
                            cmd: command,
                            thought_signature,
                            _credential: None,
                            retry_pane: retry_in_pane,
                        });
                    }
                    AiEvent::ListRunbooks { id, thought_signature } => {
                        pending_calls.push(PendingCall::ListRunbooks { id, thought_signature });
                    }
                    AiEvent::ReadRunbook { id, thought_signature, name } => {
                        pending_calls.push(PendingCall::ReadRunbook { id, thought_signature, name });
                    }
                    AiEvent::SearchRepository { id, thought_signature, query, kind } => {
                        pending_calls.push(PendingCall::SearchRepository { id, thought_signature, query, kind });
                    }
                    AiEvent::ListMemories { id, thought_signature, category } => {
                        pending_calls.push(PendingCall::ListMemories { id, thought_signature, category });
                    }
                    AiEvent::ReadMemory { id, thought_signature, key, category } => {
                        pending_calls.push(PendingCall::ReadMemory { id, thought_signature, key, category });
                    }
                    AiEvent::GetTerminalContext { id, thought_signature } => {
                        pending_calls.push(PendingCall::GetTerminalContext { id, thought_signature });
                    }
                    AiEvent::ListPanes { id, thought_signature } => {
                        pending_calls.push(PendingCall::ListPanes { id, thought_signature });
                    }
                    AiEvent::WriteRunbook { id, thought_signature, name, content } => {
                        pending_calls.push(PendingCall::WriteRunbook { id, thought_signature, name, content });
                    }
                    AiEvent::DeleteRunbook { id, thought_signature, name } => {
                        pending_calls.push(PendingCall::DeleteRunbook { id, thought_signature, name });
                    }
                    AiEvent::WriteScript { id, thought_signature, script_name, content } => {
                        pending_calls.push(PendingCall::WriteScript { id, thought_signature, script_name, content });
                    }
                    AiEvent::DeleteScript { id, thought_signature, script_name } => {
                        pending_calls.push(PendingCall::DeleteScript { id, thought_signature, script_name });
                    }
                    AiEvent::ScheduleCommand { id, thought_signature, name, command, is_script, run_at, interval, runbook, ghost_runbook, cron } => {
                        pending_calls.push(PendingCall::ScheduleCommand {
                            id, thought_signature, name, command, is_script, run_at, interval, runbook, ghost_runbook, cron
                        });
                    }
                    AiEvent::EditFile { id, thought_signature, path, old_string, new_string, target_pane } => {
                        pending_calls.push(PendingCall::EditFile { id, thought_signature, path, old_string, new_string, target_pane });
                    }
                    AiEvent::SpawnGhost { id, runbook, message, thought_signature } => {
                        pending_calls.push(PendingCall::SpawnGhost { id, thought_signature, runbook, message });
                    }
                    AiEvent::Done(_) => break,
                    AiEvent::Error(e) => anyhow::bail!("AI error: {}", e),
                    _ => {}
                },
            }
        }

        let mut tool_results: Vec<ToolResult> = Vec::new();

        for call in &pending_calls {
            let outcome = crate::daemon::executor::execute_tool_call(
                call,
                &mut tx,
                &mut rx,
                Some(session_id),
                &tmux_session,
                None,
                cache,
                sessions,
                schedule_store,
            ).await?;

            match outcome {
                crate::daemon::executor::ToolCallOutcome::Result(r) => {
                    tool_results.push(ToolResult {
                        tool_call_id: call.id().to_string(),
                        tool_name: call.tool_name().to_string(),
                        content: r,
                    });
                }
                crate::daemon::executor::ToolCallOutcome::SpawnGhostSession {
                    session_id: ghost_sid,
                    runbook_name: _,
                    tool_result,
                } => {
                    let sessions2 = sessions.clone();
                    let cache2 = Arc::clone(cache);
                    let store2 = Arc::clone(schedule_store);
                    let config2 = config.clone();
                    match Box::pin(trigger_ghost_turn(&ghost_sid, &sessions2, &config2, &cache2, &store2)).await {
                        Ok(()) => {}
                        Err(e) => log::error!("nested SpawnGhost failed for {}: {}", ghost_sid, e),
                    }
                    tool_results.push(ToolResult {
                        tool_call_id: call.id().to_string(),
                        tool_name: call.tool_name().to_string(),
                        content: tool_result,
                    });
                }
                crate::daemon::executor::ToolCallOutcome::UserMessage(_) => {}
            }
        }

        if !tool_results.is_empty()
            && tool_results
                .iter()
                .all(|r| r.content.starts_with("Command denied by Ghost Policy"))
        {
            log::warn!(
                "Ghost Shell {}: all {} tool call(s) denied by ghost policy on turn {} — \
                 runbook may need auto_approve_scripts or auto_approve_read_only: true",
                session_id,
                tool_results.len(),
                turn,
            );
        }

        let assistant_msg = Message {
            role: "assistant".to_string(),
            content: assistant_content,
            tool_calls: if pending_calls.is_empty() { None } else { Some(pending_calls.iter().map(|c| c.to_tool_call()).collect()) },
            tool_results: if tool_results.is_empty() { None } else { Some(tool_results) },
        };

        append_session_message(session_id, &assistant_msg);
        {
            let mut store = sessions.lock().unwrap_or_log();
            if let Some(entry) = store.get_mut(session_id) {
                entry.messages.push(assistant_msg);
                entry.last_accessed = Instant::now();
            }
        }

        if pending_calls.is_empty() {
            break;
        }
    }

    log::info!("Ghost Turn completed for session {}", session_id);
    crate::daemon::stats::inc_ghosts_completed();
    Ok(())
}

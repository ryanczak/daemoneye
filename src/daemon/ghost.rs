use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::BufReader;

use crate::ai::{AiEvent, Message, PendingCall, ToolResult, make_client};
use crate::config::{Config, PricingSource, load_named_prompt};
use crate::cost::{CostRecord, compute_cost};
use crate::daemon::session::{
    SessionEntry, SessionStore, append_session_message, with_sessions, write_session_file,
};
use crate::daemon::utils::daemon_hostname;
use crate::runbook::Runbook;
use crate::scheduler::ScheduleStore;
use crate::sys_context::get_or_init_sys_context;
use crate::tmux;
use crate::tmux::cache::SessionCache;
use crate::tmux::ensure_incident_session;

/// Static ghost shell operating rules, appended to the ghost system prompt.
const GHOST_SHELL_RULES: &str = include_str!("../../assets/prompts/ghost-shell.txt");

/// Write a mailbox entry when a ghost shell exits (clean or error).
/// Best-effort: failures are logged but do not affect the caller.
async fn write_mailbox_on_exit(
    session_id: &str,
    sessions: &SessionStore,
    error: Option<&anyhow::Error>,
) {
    let Some((agent_name, ghost_config)) = with_sessions(sessions, |store| {
        let entry = store.get(session_id)?;
        let gc = entry.ghost_config.clone();
        let agent = gc.as_ref().and_then(|g| g.agent.clone());
        Some((agent, gc))
    }) else {
        return;
    };
    let Some(agent_name) = agent_name else {
        return;
    };

    let last_content = with_sessions(sessions, |store| {
        store
            .get(session_id)
            .and_then(|e| e.messages.last())
            .filter(|m| m.role == "assistant")
            .map(|m| m.content.clone())
            .unwrap_or_default()
    });

    let (status, err_text, result_text) = if let Some(e) = error {
        (
            crate::agents::mailbox::MailboxStatus::Failed,
            Some(e.to_string()),
            if last_content.is_empty() {
                None
            } else {
                Some(last_content)
            },
        )
    } else {
        (
            crate::agents::mailbox::MailboxStatus::Complete,
            None,
            if last_content.is_empty() {
                None
            } else {
                Some(last_content)
            },
        )
    };

    let completed_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let spawn_depth = ghost_config.as_ref().map(|g| g.spawn_depth).unwrap_or(0);
    let parent_job_id = ghost_config.as_ref().and_then(|g| g.parent_job_id.clone());

    let task_desc = with_sessions(sessions, |store| {
        store
            .get(session_id)
            .and_then(|e| e.ghost_task_message.clone())
            .unwrap_or_else(|| match &parent_job_id {
                Some(pid) => format!(
                    "ghost shell for session {} (depth {}, parent: {})",
                    session_id, spawn_depth, pid
                ),
                None => format!(
                    "ghost shell for session {} (depth {})",
                    session_id, spawn_depth
                ),
            })
    });

    let mailbox_entry = crate::agents::mailbox::MailboxResult {
        job_id: session_id.to_string(),
        agent: agent_name.clone(),
        task: task_desc,
        status,
        result: result_text,
        error: err_text,
        completed_at: Some(completed_at),
    };

    if let Err(e) = crate::agents::mailbox::write_mailbox(&agent_name, &mailbox_entry) {
        log::warn!(
            "Failed to write mailbox result for agent '{}': {}",
            agent_name,
            e
        );
    } else {
        log::info!(
            "Mailbox result written for agent '{}' (job_id: {})",
            agent_name,
            session_id
        );
    }
}

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
        // Seed from config.approvals.ghost_commands — OR-ed with per-runbook frontmatter.
        ghost_commands_default: bool,
    ) -> Result<String> {
        Self::start_session_with_config(
            sessions,
            runbook,
            &runbook.ghost_config,
            alert_msg,
            bg_prefix,
            ghost_commands_default,
        )
        .await
    }

    /// Start a new Ghost Shell with a pre-merged `GhostConfig`.
    ///
    /// Used when an agent config or runbook `agent:` field overrides the
    /// runbook's own ghost config. The `ghost_config` parameter is the
    /// fully merged config (runbook + agent, with runbook winning conflicts).
    pub async fn start_session_with_config(
        sessions: SessionStore,
        runbook: &Runbook,
        ghost_config: &crate::ipc::GhostConfig,
        alert_msg: &str,
        bg_prefix: &'static str,
        ghost_commands_default: bool,
    ) -> Result<String> {
        let alert_name = &runbook.name;

        // 1. Ensure host tmux session exists (active or detached)
        let tmux_session =
            ensure_incident_session().context("GhostManager: failed to ensure incident session")?;

        // 2. Initialize ghost shell entry
        let session_id = format!("ghost-{}-{}", alert_name, uuid::Uuid::new_v4().simple());

        let mut messages = Vec::new();

        // The alert payload plus the full runbook body form the first user turn.
        // Including the runbook content here ensures the ghost AI has its instructions
        // from the start regardless of the trigger path — a scheduled job fires with
        // only a terse "job fired" message, so without this the ghost would produce a
        // text-only response with no tool calls and exit after one turn.
        //
        // Ghost behavioral instructions (autonomous mode, background-only execution,
        // no human present) live in the system prompt assembled by `trigger_ghost_turn`,
        // not here.  Putting them in an assistant-role message causes the Anthropic API
        // to reject the request because conversations must begin with a user turn.
        let prior = crate::daemon::situational::assemble_incident_context(alert_msg)
            .map(|b| format!("\n\n{}", b))
            .unwrap_or_default();
        let user_msg = Message {
            role: "user".to_string(),
            content: format!(
                "Incoming alert:\n{}\n\nRunbook: {}\n\n{}{}",
                alert_msg, runbook.name, runbook.content, prior,
            ),
            tool_calls: None,
            tool_results: None,
            turn: Some(1),
        };
        // Write initial message to session file immediately so the file exists
        // even if the ghost shell fails before completing its first turn.
        crate::daemon::session::append_session_message(&session_id, &user_msg);
        messages.push(user_msg);

        let mut gc = ghost_config.clone();
        // Merge daemon-wide default: if either source enables the flag, the ghost gets it.
        gc.auto_approve_commands |= ghost_commands_default;

        let entry = SessionEntry {
            messages,
            last_accessed: Instant::now(),
            chat_pane: None,
            default_target_pane: None, // Ghost shells use background windows exclusively
            bg_windows: Vec::new(),
            last_prompt_tokens: 0,
            tmux_session: tmux_session.clone(),
            last_detach: None,
            detach_time_utc: None,
            messages_at_detach: 0,
            pipe_source_pane: None,
            is_ghost: true,
            ghost_config: Some(gc),
            ghost_bg_prefix: bg_prefix,
            started_at: chrono::Utc::now(),
            turn_count: 0,
            tool_calls_this_session: 0,
            active_model: ghost_config.model.clone(),
            last_snapshot_activity: 0,
            saved_name: None,
            dirty: false,
            artifacts_created: Vec::new(),
            auto_name_suggested: false,
            ghost_task_message: None,
            loaded_tools: std::collections::HashSet::new(),
            cost_usd: 0.0,
            cost_by_agent: std::collections::HashMap::new(),
            has_untracked_cost: false,
            token_scale: 1.5,
            compaction_in_flight: false,
            pending_compaction_notice: None,
        };

        with_sessions(&sessions, |store| {
            store.insert(session_id.clone(), entry);
        });

        crate::daemon::stats::inc_ghosts_launched();

        log::info!(
            "Ghost Shell started: {} (alert: {}, tmux_session: {}, bg_prefix: {})",
            session_id,
            alert_name,
            tmux_session,
            bg_prefix,
        );
        crate::daemon::utils::log_event(
            "ghost_start",
            serde_json::json!({
                "session_id": session_id,
                "alert_name": alert_name,
                "tmux_session": tmux_session,
                "trigger": bg_prefix,
                "spawn_depth": ghost_config.spawn_depth,
                "parent_job_id": ghost_config.parent_job_id,
            }),
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
    let result = do_ghost_turn(session_id, sessions, config, cache, schedule_store).await;
    write_mailbox_on_exit(session_id, sessions, result.as_ref().err()).await;
    result
}

async fn do_ghost_turn(
    session_id: &str,
    sessions: &SessionStore,
    config: &Config,
    cache: &Arc<SessionCache>,
    schedule_store: &Arc<ScheduleStore>,
) -> Result<()> {
    let Some((_messages, ghost_config, tmux_session, _target_pane, ghost_active_model)) =
        with_sessions(sessions, |store| {
            let entry = store.get(session_id)?;
            Some((
                entry.messages.clone(),
                entry.ghost_config.clone(),
                entry.tmux_session.clone(),
                entry.default_target_pane.clone(),
                entry.active_model.clone(),
            ))
        })
    else {
        anyhow::bail!("Ghost Shell '{}' not found", session_id);
    };

    let prompt_name = config.ai.prompt.clone();
    let system_base = load_named_prompt(&prompt_name).system;
    let sys_context = get_or_init_sys_context();

    let daemon_ceiling = config.ghost.max_ghost_turns;
    let (approved_scripts, run_with_sudo, max_ghost_turns, ssh_target, auto_approve_commands) =
        with_sessions(sessions, |store| {
            store
                .get(session_id)
                .and_then(|e| e.ghost_config.as_ref())
                .map(|gc| {
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
                    (
                        scripts,
                        gc.run_with_sudo,
                        turns,
                        gc.ssh_target.clone(),
                        gc.auto_approve_commands,
                    )
                })
                .unwrap_or_else(|| ("none".to_string(), false, daemon_ceiling, None, false))
        });
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
    let agents_block = crate::daemon::prompt::format_available_agents();
    let system = format!(
        "{}\n\n\
         ## Ghost Shell Execution Context\n\
         Daemon Host: {}\n\
         Tmux Session: {}\n\
         {}Command Policy: non-sudo commands run freely (OS permissions are the boundary){}. \
         {}\n\
         Pre-approved Sudo Scripts: {}{}\n\
         Turn Budget: {} (hard limit — shell will be stopped when reached)\n\n\
         {}\n\n\
         {}\
         {}",
        system_base,
        daemon_hostname(),
        tmux_session,
        remote_line,
        if auto_approve_commands {
            " — explicitly approved for investigation commands"
        } else {
            ""
        },
        if run_with_sudo {
            "Sudo commands are freely allowed (run_with_sudo is enabled for this runbook)."
                .to_string()
        } else {
            "Sudo commands require a pre-approved script via install-sudoers.".to_string()
        },
        approved_scripts,
        if run_with_sudo {
            " (executed with sudo)"
        } else {
            ""
        },
        max_ghost_turns,
        sys_context.format_for_ai(),
        GHOST_SHELL_RULES,
        agents_block,
    );

    let s = tmux_session.clone();
    let pane_alive = tmux::off_runtime("session-exists", move || tmux::session_exists(&s))
        .await
        .unwrap_or(false);
    if !pane_alive {
        anyhow::bail!(
            "Ghost Shell {}: tmux session '{}' no longer exists",
            session_id,
            tmux_session
        );
    }

    // Ghost shells never have a real IPC client — use sink/empty so that
    // send_response_split writes never block (they're discarded) and reads
    // immediately return EOF (ghost policy bypasses all approval prompts anyway).
    let mut tx = tokio::io::sink();
    let mut rx = BufReader::new(tokio::io::empty());

    let model_entry = config.resolve_model(ghost_active_model.as_deref());
    if let Some(ref name) = ghost_active_model {
        log::info!("Ghost Shell {}: using model '{}'", session_id, name);
    }
    let client: Arc<Box<dyn crate::ai::AiClient>> = Arc::new(make_client(
        &model_entry.provider,
        model_entry.resolve_api_key(),
        model_entry.model.clone(),
        model_entry.effective_base_url(),
        model_entry.effective_max_tokens(),
    ));

    const GHOST_TURN_TIMEOUT_SECS: u64 = 300;

    let mut turn = 0usize;
    // Tracks whether the wrap-up turn has already been injected so we run it
    // exactly once before breaking the loop.
    let mut wrap_up_turn = false;
    loop {
        if wrap_up_turn {
            // The previous iteration was the wrap-up turn — stop now.
            log::warn!(
                "Ghost Shell {}: wrap-up turn complete, stopping after {} turns (limit {})",
                session_id,
                turn,
                max_ghost_turns
            );
            break;
        }
        if turn >= max_ghost_turns {
            // Inject a synthetic user message asking the agent to wrap up, then
            // run one final turn so it can leave a clean handoff rather than
            // stopping mid-thought.
            log::warn!(
                "Ghost Shell {}: reached max turns ({}), running wrap-up turn",
                session_id,
                max_ghost_turns
            );
            let wrap_up = crate::ai::Message {
                role: "user".to_string(),
                content: format!(
                    "[BUDGET EXHAUSTED — turn {0}/{0} reached. This is your final turn. \
                     Summarize what was accomplished, record any critical state or follow-ups \
                     to memory (add_memory), and stop. Do not call run_terminal_command, \
                     edit_file, write_script, schedule_command, or spawn_ghost_shell in this turn.]",
                    max_ghost_turns
                ),
                tool_calls: None,
                tool_results: None,
                turn: Some(turn + 1),
            };
            let pushed = with_sessions(sessions, |store| {
                if let Some(entry) = store.get_mut(session_id) {
                    entry.messages.push(wrap_up.clone());
                    true
                } else {
                    false
                }
            });
            if pushed {
                crate::daemon::session::append_session_message(session_id, &wrap_up);
            }
            wrap_up_turn = true;
        }
        turn += 1;

        log::info!(
            "Ghost Shell {}: starting turn {}/{}{}",
            session_id,
            turn,
            max_ghost_turns,
            if wrap_up_turn { " (wrap-up)" } else { "" }
        );

        let Some((messages, loaded_tools, token_scale, started_at)) =
            with_sessions(sessions, |store| {
                let entry = store.get(session_id)?;
                Some((
                    entry.messages.clone(),
                    entry.loaded_tools.iter().cloned().collect::<Vec<String>>(),
                    entry.token_scale,
                    entry.started_at,
                ))
            })
        else {
            break;
        };

        let (chat_messages, compacted) =
            crate::daemon::context::ghost_ws::enforce_ghost_working_set(
                session_id,
                messages,
                token_scale,
                started_at,
                model_entry.context_window(),
                config,
            );
        if compacted {
            with_sessions(sessions, |store| {
                if let Some(entry) = store.get_mut(session_id) {
                    entry.messages = chat_messages.clone();
                }
            });
            write_session_file(session_id, &chat_messages);
        }

        let client_clone = Arc::clone(&client);
        let system_clone = system.clone();

        let (ai_tx, mut ai_rx) = tokio::sync::mpsc::unbounded_channel::<AiEvent>();

        tokio::spawn(async move {
            if let Err(e) = client_clone
                .chat(&system_clone, chat_messages, ai_tx, true, loaded_tools)
                .await
            {
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
                        session_id,
                        turn,
                        GHOST_TURN_TIMEOUT_SECS
                    );
                    crate::daemon::utils::log_event(
                        "ghost_error",
                        serde_json::json!({
                            "session_id": session_id,
                            "turn": turn,
                            "error": format!("turn timed out after {}s", GHOST_TURN_TIMEOUT_SECS),
                        }),
                    );
                    anyhow::bail!("ghost turn timed out");
                }
                Ok(None) => break,
                Ok(Some(ev)) => match ev {
                    AiEvent::Token(t) => {
                        assistant_content.push_str(&t);
                    }
                    AiEvent::ToolCall(
                        id,
                        command,
                        _background,
                        _target_pane,
                        retry_in_pane,
                        thought_signature,
                    ) => {
                        pending_calls.push(PendingCall::Background {
                            id,
                            cmd: command,
                            thought_signature,
                            _credential: None,
                            retry_pane: retry_in_pane,
                        });
                    }
                    AiEvent::ListRunbooks {
                        id,
                        thought_signature,
                    } => {
                        pending_calls.push(PendingCall::ListRunbooks {
                            id,
                            thought_signature,
                        });
                    }
                    AiEvent::ReadRunbook {
                        id,
                        thought_signature,
                        name,
                    } => {
                        pending_calls.push(PendingCall::ReadRunbook {
                            id,
                            thought_signature,
                            name,
                        });
                    }
                    AiEvent::SearchRepository {
                        id,
                        thought_signature,
                        query,
                        kind,
                    } => {
                        pending_calls.push(PendingCall::SearchRepository {
                            id,
                            thought_signature,
                            query,
                            kind,
                        });
                    }
                    AiEvent::RecallContext {
                        id,
                        thought_signature,
                        query,
                        turn_start,
                        turn_end,
                        scope,
                    } => {
                        pending_calls.push(PendingCall::RecallContext {
                            id,
                            thought_signature,
                            query,
                            turn_start,
                            turn_end,
                            scope,
                        });
                    }
                    AiEvent::ListMemories {
                        id,
                        thought_signature,
                        category,
                    } => {
                        pending_calls.push(PendingCall::ListMemories {
                            id,
                            thought_signature,
                            category,
                        });
                    }
                    AiEvent::ReadMemory {
                        id,
                        thought_signature,
                        key,
                        category,
                    } => {
                        pending_calls.push(PendingCall::ReadMemory {
                            id,
                            thought_signature,
                            key,
                            category,
                        });
                    }
                    AiEvent::UpdateMemory {
                        id,
                        key,
                        category,
                        body,
                        append,
                        tags,
                        summary,
                        relates_to,
                        expires,
                        thought_signature,
                    } => {
                        pending_calls.push(PendingCall::UpdateMemory {
                            id,
                            thought_signature,
                            key,
                            category,
                            body,
                            append,
                            tags,
                            summary,
                            relates_to,
                            expires,
                        });
                    }
                    AiEvent::GetTerminalContext {
                        id,
                        thought_signature,
                        scope,
                    } => {
                        pending_calls.push(PendingCall::GetTerminalContext {
                            id,
                            thought_signature,
                            scope,
                        });
                    }
                    AiEvent::ListPanes {
                        id,
                        thought_signature,
                    } => {
                        pending_calls.push(PendingCall::ListPanes {
                            id,
                            thought_signature,
                        });
                    }
                    AiEvent::WriteRunbook {
                        id,
                        thought_signature,
                        name,
                        content,
                    } => {
                        pending_calls.push(PendingCall::WriteRunbook {
                            id,
                            thought_signature,
                            name,
                            content,
                        });
                    }
                    AiEvent::DeleteRunbook {
                        id,
                        thought_signature,
                        name,
                    } => {
                        pending_calls.push(PendingCall::DeleteRunbook {
                            id,
                            thought_signature,
                            name,
                        });
                    }
                    AiEvent::WriteScript {
                        id,
                        thought_signature,
                        script_name,
                        content,
                    } => {
                        pending_calls.push(PendingCall::WriteScript {
                            id,
                            thought_signature,
                            script_name,
                            content,
                        });
                    }
                    AiEvent::DeleteScript {
                        id,
                        thought_signature,
                        script_name,
                    } => {
                        pending_calls.push(PendingCall::DeleteScript {
                            id,
                            thought_signature,
                            script_name,
                        });
                    }
                    AiEvent::ScheduleCommand {
                        id,
                        thought_signature,
                        name,
                        command,
                        is_script,
                        run_at,
                        interval,
                        runbook,
                        ghost_runbook,
                        cron,
                    } => {
                        pending_calls.push(PendingCall::ScheduleCommand {
                            id,
                            thought_signature,
                            name,
                            command,
                            is_script,
                            run_at,
                            interval,
                            runbook,
                            ghost_runbook,
                            cron,
                        });
                    }
                    AiEvent::EditFile {
                        id,
                        thought_signature,
                        path,
                        operation,
                        old_string,
                        new_string,
                        content,
                        dest_path,
                        target_pane,
                    } => {
                        pending_calls.push(PendingCall::EditFile {
                            id,
                            thought_signature,
                            path,
                            operation,
                            old_string,
                            new_string,
                            content,
                            dest_path,
                            target_pane,
                        });
                    }
                    AiEvent::SpawnGhost {
                        id,
                        runbook,
                        message,
                        agent,
                        thought_signature,
                    } => {
                        pending_calls.push(PendingCall::SpawnGhost {
                            id,
                            thought_signature,
                            runbook,
                            message,
                            agent,
                        });
                    }
                    AiEvent::AwaitAgentResult {
                        id,
                        job_id,
                        agent_name,
                        timeout_secs,
                        thought_signature,
                    } => {
                        pending_calls.push(PendingCall::AwaitAgentResult {
                            id,
                            thought_signature,
                            job_id,
                            agent_name,
                            timeout_secs,
                        });
                    }
                    AiEvent::Done(usage) => {
                        let pricing = model_entry.pricing().unwrap_or(crate::config::Pricing {
                            input_per_mtok: 0.0,
                            output_per_mtok: 0.0,
                            cache_read_per_mtok: 0.0,
                            cache_write_per_mtok: 0.0,
                            source: PricingSource::Unknown,
                        });
                        let cost = compute_cost(&usage, &pricing);
                        let attribution =
                            crate::cost::CostAttribution::from_ghost_config(ghost_config.as_ref());
                        let record = CostRecord {
                            timestamp: chrono::Utc::now(),
                            session_id: session_id.to_string(),
                            agent_name: attribution.agent_name,
                            is_ghost: attribution.is_ghost,
                            parent_job_id: attribution.parent_job_id,
                            provider: model_entry.provider.clone(),
                            model: model_entry.model.clone(),
                            tokens: usage,
                            cost,
                            pricing_source: pricing.source,
                        };
                        crate::daemon::utils::log_event(
                            "ai_cost",
                            // INVARIANT: CostRecord derives Serialize; serde_json::to_value never fails for it
                            serde_json::to_value(&record)
                                .expect("CostRecord serialization is infallible"),
                        );

                        // Accumulate cost on the session entry.
                        with_sessions(sessions, |store| {
                            if let Some(entry) = store.get_mut(session_id) {
                                entry.cost_usd += record.cost.total_cost_usd;
                                *entry
                                    .cost_by_agent
                                    .entry(record.agent_name.clone())
                                    .or_insert(0.0) += record.cost.total_cost_usd;
                                if record.pricing_source == PricingSource::Unknown {
                                    entry.has_untracked_cost = true;
                                }
                            }
                        });
                        break;
                    }
                    AiEvent::Error(e) => {
                        crate::daemon::utils::log_event(
                            "ghost_error",
                            serde_json::json!({
                                "session_id": session_id,
                                "turn": turn,
                                "error": format!("AI error: {e}"),
                            }),
                        );
                        anyhow::bail!("AI error: {}", e);
                    }
                    _ => {}
                },
            }
        }

        // Log what tools this turn will execute before dispatching.
        if !pending_calls.is_empty() {
            for call in &pending_calls {
                let detail = match call {
                    PendingCall::Background { cmd, .. } => format!(": {}", cmd),
                    _ => String::new(),
                };
                log::info!(
                    "Ghost Shell {}: turn {} dispatching '{}'{detail}",
                    session_id,
                    turn,
                    call.tool_name(),
                );
            }
            crate::daemon::utils::log_event(
                "ghost_turn",
                serde_json::json!({
                    "session_id": session_id,
                    "turn": turn,
                    "tool_count": pending_calls.len(),
                    "tools": pending_calls.iter().map(|c| {
                        let mut obj = serde_json::Map::new();
                        obj.insert("name".to_string(), serde_json::Value::String(c.tool_name().to_string()));
                        if let PendingCall::Background { cmd, .. } = c {
                            obj.insert("cmd".to_string(), serde_json::Value::String(cmd.clone()));
                        }
                        serde_json::Value::Object(obj)
                    }).collect::<Vec<_>>(),
                }),
            );
        }

        let mut tool_results: Vec<ToolResult> = Vec::new();

        for call in &pending_calls {
            let outcome = crate::daemon::executor::execute_tool_call(
                call,
                &mut tx,
                &mut rx,
                crate::daemon::executor::SessionCtx {
                    session_id: Some(session_id),
                    session_name: &tmux_session,
                    chat_pane: None,
                    sessions,
                },
                cache,
                schedule_store,
            )
            .await?;

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
                    job_id: _,
                } => {
                    let sessions2 = sessions.clone();
                    let cache2 = Arc::clone(cache);
                    let store2 = Arc::clone(schedule_store);
                    let config2 = config.clone();
                    match Box::pin(trigger_ghost_turn(
                        &ghost_sid, &sessions2, &config2, &cache2, &store2,
                    ))
                    .await
                    {
                        Ok(()) => {}
                        Err(e) => {
                            log::error!("nested SpawnGhost failed for {}: {}", ghost_sid, e);
                            crate::daemon::utils::log_event(
                                "ghost_error",
                                serde_json::json!({
                                    "session_id": session_id,
                                    "turn": turn,
                                    "error": format!("nested SpawnGhost failed for {ghost_sid}: {e}"),
                                }),
                            );
                        }
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
                 runbook may need auto_approve_scripts (for sudo) or auto_approve_commands: true (for non-sudo investigation commands)",
                session_id,
                tool_results.len(),
                turn,
            );
        }

        let assistant_msg = Message {
            role: "assistant".to_string(),
            content: assistant_content,
            tool_calls: if pending_calls.is_empty() {
                None
            } else {
                Some(pending_calls.iter().map(|c| c.to_tool_call()).collect())
            },
            tool_results: if tool_results.is_empty() {
                None
            } else {
                Some(tool_results)
            },
            turn: Some(turn),
        };

        append_session_message(session_id, &assistant_msg);
        with_sessions(sessions, |store| {
            if let Some(entry) = store.get_mut(session_id) {
                entry.messages.push(assistant_msg);
                entry.last_accessed = Instant::now();
            }
        });

        if pending_calls.is_empty() {
            break;
        }
    }

    log::info!("Ghost Shell {}: completed in {} turn(s)", session_id, turn);
    let (spawn_depth, parent_job_id) = ghost_config
        .as_ref()
        .map(|gc| (gc.spawn_depth, gc.parent_job_id.clone()))
        .unwrap_or((0, None));
    crate::daemon::utils::log_event(
        "ghost_complete",
        serde_json::json!({
            "session_id": session_id,
            "turns_used": turn,
            "spawn_depth": spawn_depth,
            "parent_job_id": parent_job_id,
        }),
    );
    crate::daemon::stats::inc_ghosts_completed();

    // Briefing generation: best-effort summary for the next invocation.
    if let Some(ref gc) = ghost_config
        && let Some(ref agent_name) = gc.agent
    {
        crate::daemon::briefing::generate_and_save_briefing(
            agent_name, session_id, sessions, config,
        )
        .await;
    }

    Ok(())
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

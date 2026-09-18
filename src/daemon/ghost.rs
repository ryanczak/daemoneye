use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::BufReader;

use crate::ai::{AiEvent, Message, PendingCall, ToolResult, make_client};
use crate::config::{Config, PricingSource, load_named_prompt};
use crate::cost::{CostRecord, compute_cost};
use crate::daemon::GS_SCHED_WINDOW_PREFIX;
use crate::daemon::run::{
    Briefing, BudgetDimension, RunId, RunKind, RunLogGuard, RunState, TurnLogEvent, TurnLogRecord,
    append_turn_event,
};
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
    run: Option<crate::daemon::run::RunResult>,
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
        run,
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
            ghost_runbook_name: Some(runbook.name.clone()),
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

/// The `RunKind` a ghost session runs as. A ghost spawned by another ghost is
/// `Delegated` and names its parent; everything else is a `Ghost` naming its
/// runbook and what set it off.
pub fn ghost_run_kind(
    runbook: Option<&str>,
    agent: Option<&str>,
    parent_job_id: Option<&str>,
    bg_prefix: &str,
) -> RunKind {
    if let (Some(agent), Some(parent)) = (agent, parent_job_id)
        && let Ok(parent_id) = RunId::parse(parent)
    {
        return RunKind::Delegated {
            agent: agent.to_string(),
            parent: parent_id,
        };
    }
    let trigger = if bg_prefix == GS_SCHED_WINDOW_PREFIX {
        "scheduled"
    } else {
        "webhook"
    };
    RunKind::Ghost {
        runbook: runbook.unwrap_or("unknown").to_string(),
        trigger: trigger.to_string(),
    }
}

/// Append one `TurnStarted { turn }` record to the run's turn log. The turn
/// log is diagnostic: a write problem is logged inside `append_turn_event`,
/// never a failure of the ghost turn itself.
fn emit_turn_started(run_id: &RunId, turn: usize) {
    append_turn_event(&TurnLogRecord {
        timestamp: chrono::Utc::now().to_rfc3339(),
        run_id: run_id.clone(),
        event: TurnLogEvent::TurnStarted { turn: turn as u32 },
    });
}

/// What the ghost turn loop reports back about how it ended. `turns` is the
/// count the loop actually ran, which `RunState::Cancelled` and the
/// `RunResult` both need.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct GhostLoopOutcome {
    pub turns: u32,
    pub budget_exhausted: bool,
    pub cancelled: bool,
}

/// The terminal [`RunState`] a ghost run records, from the loop's outcome and
/// its [`Result`]. The order is the contract: an error is the most specific
/// outcome even when the run was also cancelled or out of budget;
/// cancellation beats an exhausted budget.
fn ghost_terminal_state(
    result: &Result<()>,
    outcome: &GhostLoopOutcome,
    stage: String,
    session_id: &str,
) -> RunState {
    match result {
        Err(e) => RunState::HardFail(Briefing::new(
            format!("ghost shell {session_id}"),
            None,
            Vec::new(),
            Vec::new(),
            Some(e.to_string()),
            None,
        )),
        _ if outcome.cancelled => RunState::Cancelled {
            stage,
            turns: outcome.turns,
        },
        _ if outcome.budget_exhausted => RunState::BudgetExceeded(Briefing::new(
            format!("ghost shell {session_id}"),
            None,
            Vec::new(),
            Vec::new(),
            Some("ghost turn budget exhausted".to_string()),
            Some(BudgetDimension::Turns),
        )),
        _ => RunState::Complete,
    }
}

/// The literal a ghost must emit to mark a seeded step resolved.
pub const STEP_MARKER_PREFIX: &str = "STEP";

/// The 1-based indices of seeded steps with no `STEP <n>: DONE|SKIPPED`
/// marker anywhere in `transcript`. Empty when every step is covered.
///
/// Pure: no I/O, no session access. A step is covered only by a line whose
/// trimmed, uppercased form is `STEP <n>:` followed by `DONE` or `SKIPPED` —
/// prose describing the step marks nothing (the lesson of `parse_ghost_trigger`'s
/// removed `ALERT` heuristic: remarks about a condition are not the condition).
pub fn uncovered_steps(transcript: &str, step_count: usize) -> Vec<usize> {
    if step_count == 0 {
        return Vec::new();
    }
    let marker_line = |line: &str| -> Option<usize> {
        let trimmed = line.trim();
        let trimmed = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .unwrap_or(trimmed)
            .trim();
        let upper = trimmed.to_uppercase();
        let rest = upper.strip_prefix(&format!("{STEP_MARKER_PREFIX} "))?;
        let (index, verdict) = rest.split_once(':')?;
        let index: usize = index.trim().parse().ok()?;
        let verdict = verdict.trim();
        if verdict.starts_with("DONE") || verdict.starts_with("SKIPPED") {
            Some(index)
        } else {
            None
        }
    };
    let marked: std::collections::BTreeSet<usize> =
        transcript.lines().filter_map(marker_line).collect();
    (1..=step_count).filter(|n| !marked.contains(n)).collect()
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
    let kind = with_sessions(sessions, |store| {
        let entry = store.get(session_id)?;
        let gc = entry.ghost_config.clone().unwrap_or_default();
        Some(ghost_run_kind(
            entry.ghost_runbook_name.as_deref(),
            gc.agent.as_deref(),
            gc.parent_job_id.as_deref(),
            entry.ghost_bg_prefix,
        ))
    })
    .unwrap_or_else(|| ghost_run_kind(None, None, None, ""));
    let mut run_log = RunLogGuard::start(RunId::new(), kind.clone());
    let (_cancel_guard, cancel_signal) = crate::daemon::run::register_run(run_log.run_id());
    tokio::spawn(crate::daemon::run::watch_stop_sentinel(
        run_log.run_id().clone(),
        crate::daemon::run::STOP_POLL_INTERVAL,
    ));
    let mut outcome = GhostLoopOutcome::default();
    let result = do_ghost_turn(
        session_id,
        sessions,
        config,
        cache,
        schedule_store,
        run_log.run_id(),
        &cancel_signal,
        &mut outcome,
    )
    .await;
    let state = ghost_terminal_state(
        &result,
        &outcome,
        cancel_stage(run_log.run_id()),
        session_id,
    );
    run_log.finish(state.clone());
    let summary = with_sessions(sessions, |store| {
        store
            .get(session_id)
            .and_then(|e| e.messages.last())
            .filter(|m| m.role == "assistant")
            .map(|m| m.content.clone())
            .unwrap_or_default()
    });
    write_mailbox_on_exit(
        session_id,
        sessions,
        result.as_ref().err(),
        crate::daemon::run::RunResult::from_state(
            run_log.run_id().clone(),
            kind,
            state,
            summary,
            outcome.turns,
            crate::daemon::run::Budget::unlimited(),
        )
        .ok(),
    )
    .await;
    let (sbx, sid) = (config.sandbox.clone(), session_id.to_string());
    let _ = tokio::task::spawn_blocking(move || {
        crate::daemon::executor::container::teardown_ghost_containers(&sbx, &sid)
    })
    .await;
    result
}

fn cancel_stage(run_id: &crate::daemon::run::RunId) -> String {
    crate::daemon::run::take_cancel_door(run_id)
        .map(crate::daemon::run::CancelDoor::stage)
        .unwrap_or("unmarked_cancel")
        .to_string()
}

#[allow(clippy::too_many_arguments)]
async fn do_ghost_turn(
    session_id: &str,
    sessions: &SessionStore,
    config: &Config,
    cache: &Arc<SessionCache>,
    schedule_store: &Arc<ScheduleStore>,
    run_id: &RunId,
    cancel: &crate::daemon::run::RunCancelSignal,
    outcome: &mut GhostLoopOutcome,
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
    let steps: Vec<String> = with_sessions(sessions, |store| {
        store
            .get(session_id)
            .and_then(|e| e.ghost_runbook_name.clone())
    })
    .and_then(|name| match crate::runbook::load_runbook(&name) {
        Ok(rb) => Some(crate::runbook::parse_steps(&rb.content)),
        Err(e) => {
            log::warn!(
                "Ghost Shell {}: runbook '{}' unreadable, no seeded steps ({}): {}",
                session_id,
                name,
                e,
                e
            );
            None
        }
    })
    .unwrap_or_default();
    let steps_block = if steps.is_empty() {
        String::new()
    } else {
        let listed: String = steps
            .iter()
            .enumerate()
            .map(|(i, text)| format!("  {}. {}\n", i + 1, text))
            .collect();
        format!(
            "## Seeded Steps ({})\n\
             This runbook defines {} steps. You may not finish until every one is marked.\n\
             Emit, on its own line, one of:\n\
               {} <n>: DONE\n  \
               {} <n>: SKIPPED <reason>\n\
             for each step 1..={}. A step is only counted when that exact line appears —\n\
             describing the step in prose does not mark it.\n\
             {}",
            steps.len(),
            steps.len(),
            STEP_MARKER_PREFIX,
            STEP_MARKER_PREFIX,
            steps.len(),
            listed
        )
    };
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
         {}{}",
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
        steps_block,
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
        if cancel.is_cancelled() {
            log::info!(
                "Ghost Shell {session_id}: cancelled, stopping before turn {}",
                turn + 1
            );
            outcome.cancelled = true;
            break;
        }
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
            outcome.budget_exhausted = true;
        }
        turn += 1;
        outcome.turns = turn as u32;
        emit_turn_started(run_id, turn);

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
                        _wait,
                        thought_signature,
                        _shell,
                        _timeout_secs,
                    ) => {
                        pending_calls.push(PendingCall::Background {
                            id,
                            cmd: command,
                            thought_signature,
                            _credential: None,
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
                    AiEvent::GetShellContext {
                        id,
                        thought_signature,
                        scope,
                    } => {
                        pending_calls.push(PendingCall::GetShellContext {
                            id,
                            thought_signature,
                            scope,
                        });
                    }
                    AiEvent::ListShells {
                        id,
                        thought_signature,
                        scope,
                    } => {
                        pending_calls.push(PendingCall::ListShells {
                            id,
                            thought_signature,
                            scope,
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
            if !steps.is_empty() {
                let transcript = with_sessions(sessions, |store| {
                    store
                        .get(session_id)
                        .map(|entry| {
                            entry
                                .messages
                                .iter()
                                .filter(|m| m.role == "assistant")
                                .map(|m| m.content.clone())
                                .collect::<Vec<String>>()
                                .join("\n")
                        })
                        .unwrap_or_default()
                });
                let uncovered = uncovered_steps(&transcript, steps.len());
                if !uncovered.is_empty() {
                    let names: Vec<String> = uncovered.iter().map(|n| n.to_string()).collect();
                    let nudge = crate::ai::Message {
                        role: "user".to_string(),
                        content: format!(
                            "[SEEDED STEPS INCOMPLETE — steps {} have no {} <n>: DONE or \
                             {} <n>: SKIPPED <reason> line. Emit one for each, then finish. \
                             Do not re-run work already done.]",
                            names.join(", "),
                            STEP_MARKER_PREFIX,
                            STEP_MARKER_PREFIX
                        ),
                        tool_calls: None,
                        tool_results: None,
                        turn: Some(turn + 1),
                    };
                    append_session_message(session_id, &nudge);
                    with_sessions(sessions, |store| {
                        if let Some(entry) = store.get_mut(session_id) {
                            entry.messages.push(nudge);
                            entry.last_accessed = Instant::now();
                        }
                    });
                    log::warn!(
                        "Ghost Shell {}: steps {} uncovered, nudging instead of completing",
                        session_id,
                        names.join(", ")
                    );
                    continue;
                }
            }
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
    fn uncovered_steps_all_marked_done_is_empty() {
        let transcript = "working on it\nSTEP 1: DONE\nSTEP 2: DONE\n";
        assert!(
            uncovered_steps(transcript, 2).is_empty(),
            "a marker for every seeded step means nothing is uncovered"
        );
    }

    #[test]
    fn uncovered_steps_skipped_counts_as_covered() {
        let transcript = "STEP 1: DONE\nSTEP 2: SKIPPED disk was already clean\n";
        assert!(
            uncovered_steps(transcript, 2).is_empty(),
            "SKIPPED is a legitimate verdir for covering a step"
        );
    }

    #[test]
    fn uncovered_steps_reports_the_gaps() {
        let transcript = "STEP 1: DONE\nSTEP 3: DONE\n";
        assert_eq!(
            uncovered_steps(transcript, 4),
            vec![2, 4],
            "steps 1 and 3 are marked, so 2 and 4 are the uncovered gaps, in order"
        );
    }

    #[test]
    fn uncovered_steps_is_case_insensitive() {
        let transcript = "step 1: done\nstep 2: skipped reason\n";
        assert!(
            uncovered_steps(transcript, 2).is_empty(),
            "lower-case markers cover their steps, as parse_ghost_trigger uppercases before matching"
        );
    }

    #[test]
    fn uncovered_steps_accepts_a_bulleted_marker() {
        let transcript = "- STEP 1: DONE\n* STEP 2: SKIPPED not needed\n";
        assert!(
            uncovered_steps(transcript, 2).is_empty(),
            "a model writing markdown will bullet the marker line; it still covers the step"
        );
    }

    #[test]
    fn uncovered_steps_prose_about_a_step_covers_nothing() {
        let transcript = "I could not complete step 2 to my satisfaction.\n";
        assert_eq!(
            uncovered_steps(transcript, 3),
            vec![1, 2, 3],
            "prose about a step is not a marker — only a literal STEP <n>: DONE|SKIPPED line \
             covers it, exactly as a bare ALERT remark stopped triggering in H3"
        );
    }

    #[test]
    fn uncovered_steps_in_progress_is_not_covered() {
        let transcript = "STEP 1: DONE\nSTEP 2: IN PROGRESS\n";
        assert_eq!(
            uncovered_steps(transcript, 2),
            vec![2],
            "neither DONE nor SKIPPED means the step is still open"
        );
    }

    #[test]
    fn uncovered_steps_ignores_an_out_of_range_index() {
        let transcript = "STEP 20: DONE\n";
        assert_eq!(
            uncovered_steps(transcript, 3),
            vec![1, 2, 3],
            "an index beyond step_count covers nothing and must not alias step 2"
        );
    }

    #[test]
    fn uncovered_steps_zero_steps_is_empty() {
        let transcript = "STEP 1: DONE\n";
        assert!(
            uncovered_steps(transcript, 0).is_empty(),
            "no seeded steps means nothing can be uncovered"
        );
    }

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

    #[test]
    fn ghost_run_kind_names_the_runbook_and_a_webhook_trigger() {
        let kind = ghost_run_kind(
            Some("investigate-disk"),
            None,
            None,
            crate::daemon::GS_BG_WINDOW_PREFIX,
        );
        assert_eq!(
            kind,
            RunKind::Ghost {
                runbook: "investigate-disk".into(),
                trigger: "webhook".into()
            }
        );
    }

    #[test]
    fn ghost_run_kind_names_a_scheduled_trigger_from_the_sched_prefix() {
        let kind = ghost_run_kind(
            Some("nightly-sweep"),
            None,
            None,
            crate::daemon::GS_SCHED_WINDOW_PREFIX,
        );
        let trigger = match &kind {
            RunKind::Ghost { trigger, .. } => trigger.clone(),
            other => panic!("expected Ghost, got {other:?}"),
        };
        assert_eq!(trigger, "scheduled");
    }

    #[test]
    fn ghost_run_kind_is_delegated_when_a_parent_run_id_and_agent_are_present() {
        let parent = RunId::parse("2190b361c94e7a45").expect("a valid 16-hex id");
        let kind = ghost_run_kind(
            Some("investigate-disk"),
            Some("analyst"),
            Some(parent.as_str()),
            crate::daemon::GS_BG_WINDOW_PREFIX,
        );
        assert_eq!(
            kind,
            RunKind::Delegated {
                agent: "analyst".into(),
                parent
            }
        );
    }

    #[test]
    fn ghost_run_kind_falls_back_to_ghost_when_the_parent_is_not_a_run_id() {
        let kind = ghost_run_kind(
            Some("investigate-disk"),
            Some("analyst"),
            Some("ghost-disk-full-0123456789abcdef0123456789abcdef"),
            crate::daemon::GS_BG_WINDOW_PREFIX,
        );
        assert_ne!(
            kind,
            RunKind::Delegated {
                agent: "analyst".into(),
                parent: RunId::parse("2190b361c94e7a45").expect("fixture id"),
            },
            "a parent that is not a run id must stay a Ghost, not a Delegated run"
        );
        let (runbook, trigger) = match kind {
            RunKind::Ghost { runbook, trigger } => (runbook, trigger),
            other => panic!("expected Ghost for the non-run-id parent, got {other:?}"),
        };
        assert_eq!(runbook, "investigate-disk");
        assert_eq!(trigger, "webhook");
    }

    #[test]
    fn ghost_run_kind_without_a_runbook_says_unknown() {
        let kind = ghost_run_kind(None, None, None, crate::daemon::GS_BG_WINDOW_PREFIX);
        match kind {
            RunKind::Ghost { runbook, .. } => {
                assert_eq!(runbook, "unknown", "a nameless runbook must read 'unknown'")
            }
            other => panic!("expected Ghost, got {other:?}"),
        }
    }

    #[test]
    fn ghost_turn_log_records_a_turn_started_for_each_turn() {
        use crate::daemon::run::cancel::test_support::{HomeGuard, fresh_run_id};
        use crate::daemon::run::{TurnLogEvent, read_turn_log};
        let _home = HomeGuard::new();
        let id = fresh_run_id();
        emit_turn_started(&id, 1);
        emit_turn_started(&id, 2);
        let records = read_turn_log(&id);
        let turns: Vec<u32> = records
            .iter()
            .filter_map(|r| match &r.event {
                TurnLogEvent::TurnStarted { turn } => Some(*turn),
                _ => None,
            })
            .collect();
        assert_eq!(
            turns,
            vec![1, 2],
            "each emit must land exactly one TurnStarted record, in order"
        );
        assert!(records.iter().all(|r| r.run_id == id));
    }

    #[test]
    fn ghost_loop_outcome_defaults_to_zero_turns_and_no_flags() {
        let outcome = GhostLoopOutcome::default();
        assert_eq!(
            outcome,
            GhostLoopOutcome {
                turns: 0,
                budget_exhausted: false,
                cancelled: false,
            },
            "Default must give zero turns and no flags — the wrapper relies on \
             starting from a clean outcome"
        );
        assert_eq!(outcome.turns, 0, "Default turns must be 0");
        assert!(!outcome.budget_exhausted, "Default budget flag must be off");
        assert!(!outcome.cancelled, "Default cancel flag must be off");
    }

    #[test]
    fn ghost_terminal_state_prefers_hard_fail_over_cancel_and_budget() {
        let outcome = GhostLoopOutcome {
            turns: 3,
            budget_exhausted: true,
            cancelled: true,
        };
        let state = ghost_terminal_state(
            &Err(anyhow::anyhow!("model exploded")),
            &outcome,
            "stop_sentinel".to_string(),
            "gs-session-1",
        );
        match state {
            RunState::HardFail(briefing) => {
                assert!(
                    briefing
                        .blocker
                        .as_deref()
                        .is_some_and(|b| b.contains("model exploded")),
                    "the error must land in the briefing blocker, got {:?}",
                    briefing.blocker
                );
            }
            other => panic!("an Err with both flags set must stay HardFail, got {other:?}"),
        }
    }

    #[test]
    fn ghost_terminal_state_prefers_cancel_over_budget() {
        let outcome = GhostLoopOutcome {
            turns: 5,
            budget_exhausted: true,
            cancelled: true,
        };
        let state = ghost_terminal_state(&Ok(()), &outcome, "chat_cancel".to_string(), "gs-2");
        match state {
            RunState::Cancelled { stage, turns } => {
                assert_eq!(
                    stage, "chat_cancel",
                    "the stage must be the door mark, not a default"
                );
                assert_eq!(turns, 5, "Cancelled.turns must be the loop's actual count");
            }
            other => panic!("Ok with both flags must read Cancelled, got {other:?}"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn ghost_cancel_signal_stops_the_run_and_names_the_sentinel_door() {
        use crate::daemon::run::cancel::test_support::{HomeGuard, fresh_run_id};
        use std::time::Duration;
        let _home = HomeGuard::new();
        let id = fresh_run_id();
        let (guard, signal) = crate::daemon::run::register_run(&id);
        crate::daemon::run::request_stop(id.as_str()).expect("a valid run id writes its sentinel");
        let watcher = tokio::spawn(crate::daemon::run::watch_stop_sentinel(
            id.clone(),
            Duration::from_millis(10),
        ));
        tokio::task::yield_now().await;
        for _ in 0..3 {
            tokio::time::advance(Duration::from_millis(10)).await;
            tokio::task::yield_now().await;
        }
        drop(watcher);
        assert!(
            signal.is_cancelled(),
            "the sentinel the watcher fired must flip the ghost's registered signal"
        );
        drop(guard);
        let stage = crate::daemon::run::take_cancel_door(&id);
        assert_eq!(
            stage,
            Some(crate::daemon::run::CancelDoor::Sentinel),
            "the door mark must name the sentinel door, not the chat door and not \
             the sweep-on-drop default"
        );
    }
}

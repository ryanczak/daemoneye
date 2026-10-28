use anyhow::Result;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::BufReader;

use crate::ai::{AiEvent, Message, PendingCall, ToolResult, make_client};
use crate::config::{Config, PricingSource, load_named_prompt};
use crate::daemon::GS_SCHED_WINDOW_PREFIX;
use crate::daemon::run::{
    Briefing, Budget, BudgetDimension, RunId, RunKind, RunLogGuard, RunMetrics, RunState,
    TurnLogEvent, TurnLogRecord, append_turn_event, emit_turn_started,
};
use crate::daemon::session::{
    SessionEntry, SessionStore, append_session_message, with_sessions, write_session_file,
};
use crate::daemon::utils::daemon_hostname;
use crate::runbook::Runbook;
use crate::scheduler::ScheduleStore;
use crate::sys_context::get_or_init_sys_context;
use crate::tmux::cache::SessionCache;

mod prompt;
mod turn;

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

/// Resolve a ghost's `ssh_target` against `[hosts]` and record the outcome.
///
/// Runs **once, at ghost start** — `start_session_with_config` is the one
/// funnel every ghost start passes through — so the session's `GhostConfig`
/// carries the resolution and `GhostPolicy::from_config` copies it into the
/// policy like any other field, instead of the per-tool-call policy path
/// doing file I/O under the sessions lock.
///
/// `None` (no `ssh_target`) changes nothing and logs nothing: every ghost
/// without a remote target keeps today's behaviour exactly.
pub(crate) fn apply_ssh_target_resolution(
    gc: &mut crate::ipc::GhostConfig,
    hosts: &std::collections::HashMap<String, crate::config::HostEntry>,
    session_id: &str,
) -> Option<crate::host::SshTargetResolution> {
    let target = gc.ssh_target.as_deref()?;
    let resolution = crate::host::resolve_ssh_target(target, hosts);
    let resolution_label;
    let host;
    let candidates: Vec<String>;
    match &resolution {
        crate::host::SshTargetResolution::Configured(key) => {
            gc.ssh_target_host = Some(key.clone());
            gc.ssh_target_sudo_scripts = hosts
                .get(key)
                .map(|entry| entry.sudo_scripts.clone())
                .unwrap_or_default();
            resolution_label = "configured";
            host = serde_json::json!(key);
            candidates = Vec::new();
        }
        crate::host::SshTargetResolution::Unconfigured => {
            gc.ssh_target_host = None;
            gc.ssh_target_sudo_scripts = Vec::new();
            resolution_label = "unconfigured";
            host = serde_json::Value::Null;
            candidates = Vec::new();
        }
        crate::host::SshTargetResolution::Ambiguous(keys) => {
            gc.ssh_target_host = None;
            gc.ssh_target_sudo_scripts = Vec::new();
            resolution_label = "ambiguous";
            host = serde_json::Value::Null;
            candidates = keys.clone();
        }
    }
    crate::daemon::utils::log_event(
        "ghost_ssh_target_resolved",
        serde_json::json!({
            "session": session_id,
            "ssh_target": target,
            "resolution": resolution_label,
            "host": host,
            "candidates": candidates,
        }),
    );
    Some(resolution)
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

        // A ghost owns no tmux session (phase 05): its identity is its session
        // id, and its commands run in PTY shells.
        let tmux_session = String::new();

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
        apply_ssh_target_resolution(
            &mut gc,
            &crate::config::Config::load().unwrap_or_default().hosts,
            &session_id,
        );

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
            "Ghost Shell started: {} (alert: {}, bg_prefix: {})",
            session_id,
            alert_name,
            bg_prefix,
        );
        crate::daemon::utils::log_event(
            "ghost_start",
            serde_json::json!({
                "session_id": session_id,
                "alert_name": alert_name,
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

/// What the ghost turn loop reports back about how it ended. `turns` is the
/// count the loop actually ran, which `RunState::Cancelled` and the
/// `RunResult` both need.
#[derive(Debug, Default, Clone)]
pub struct GhostLoopOutcome {
    pub turns: u32,
    pub budget_exhausted: bool,
    pub cancelled: bool,
    /// The daily-cap gate's refusal message, set when the cap stopped a
    /// turn from being made at all (no wrap-up turn).
    pub cost_refused: Option<String>,
}

/// The terminal [`RunState`] a ghost run records, from the loop's outcome and
/// its [`Result`]. The order is the contract: an error is the most specific
/// outcome even when the run was also cancelled or out of budget;
/// cancellation beats an exhausted budget, and the cost cap beats the turn
/// budget.
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
        _ if outcome.cost_refused.is_some() => RunState::BudgetExceeded(Briefing::new(
            format!("ghost shell {session_id}"),
            None,
            Vec::new(),
            Vec::new(),
            outcome.cost_refused.clone(),
            Some(BudgetDimension::Usd),
        )),
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
    if let Some(ghost_gc) = with_sessions(sessions, |store| {
        store.get(session_id).and_then(|e| e.ghost_config.clone())
    }) {
        if let Some(agent) = ghost_gc.agent.clone() {
            run_log.tag("agent", agent);
        }
        if let Some(host) = ghost_gc.ssh_target.clone() {
            run_log.tag("host", host);
        }
    }
    let (_cancel_guard, cancel_signal, _active_guard) =
        register_ghost_run(session_id, run_log.run_id());
    tokio::spawn(crate::daemon::run::watch_stop_sentinel(
        run_log.run_id().clone(),
        crate::daemon::run::STOP_POLL_INTERVAL,
    ));
    let mut outcome = GhostLoopOutcome::default();
    let run_id = run_log.run_id().clone();
    let result = turn::do_ghost_turn(
        session_id,
        sessions,
        config,
        cache,
        schedule_store,
        &run_id,
        &cancel_signal,
        &mut outcome,
        run_log.metrics_mut(),
    )
    .await;
    let state = ghost_terminal_state(
        &result,
        &outcome,
        cancel_stage(run_log.run_id()),
        session_id,
    );
    // 08b: record the sandbox's egress while the log is still this sandbox's
    // own — the log survives `rm` and accumulates across sandboxes that reuse
    // a name. A created set miss (the ghost never ran a sandboxed command)
    // keeps the "no network requests" note out of runs that had no sandbox.
    if config.sandbox.enabled {
        let name = crate::daemon::executor::sbx::sandbox_name(
            crate::daemon::executor::sbx::SandboxKind::Ghost,
            session_id,
        );
        if crate::daemon::executor::ghost_sandbox::is_created(&name) {
            let program = crate::shell::which_sbx().unwrap_or_else(|_| "sbx".to_string());
            if let Some(audit) = crate::daemon::executor::sbx_lifecycle::read_egress(
                &crate::shell::real_runner(program),
                &name,
            )
            .await
            {
                let run_id = run_log.run_id().clone();
                append_turn_event(&TurnLogRecord {
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    run_id,
                    event: TurnLogEvent::Note {
                        text: crate::daemon::executor::sbx::egress_summary(&audit),
                    },
                });
                crate::daemon::utils::log_event(
                    "sandbox_egress",
                    crate::daemon::executor::sbx::egress_event(&name, &audit),
                );
            }
        }
    }
    run_log.finish(state.clone());
    let summary = with_sessions(sessions, |store| {
        store
            .get(session_id)
            .and_then(|e| e.messages.last())
            .filter(|m| m.role == "assistant")
            .map(|m| m.content.clone())
            .unwrap_or_default()
    });
    // The ghost's `Budget` says what the run actually spent: `max_turns` is
    // the effective turn cap the loop enforced, the other four limits are 0
    // (no other cap exists yet), and the tally folds its spend in. The wall
    // clock is read off the guard's `Instant`, which covers the whole run.
    let effective_cap = with_sessions(sessions, |store| {
        let daemon_ceiling = config.ghost.max_ghost_turns as u64;
        store
            .get(session_id)
            .and_then(|e| e.ghost_config.as_ref())
            .map(|gc| {
                if gc.max_ghost_turns > 0 {
                    (gc.max_ghost_turns as u64).min(daemon_ceiling)
                } else {
                    daemon_ceiling
                }
            })
            .unwrap_or(daemon_ceiling)
    });
    let mut budget = Budget::from_limits(effective_cap, 0, 0, 0, 0);
    run_log
        .metrics()
        .fold_into_budget(&mut budget, run_log.elapsed_secs());
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
            budget,
        )
        .ok(),
    )
    .await;
    // 08c: the ghost's shells die with the ghost — every run's commands share
    // one `Owner::Ghost(session)` shell per host, and a scheduled ghost would
    // otherwise leave an idle ssh shell host behind per run.
    let owner = crate::shell::Owner::Ghost(session_id.to_string());
    let closed = crate::daemon::executor::knowledge::close_owner_shells(
        crate::shell::registry(),
        &owner,
        Duration::from_secs(5),
    )
    .await;
    if !closed.is_empty() {
        crate::daemon::utils::log_event(
            "ghost_shells_closed",
            serde_json::json!({
                "session": session_id,
                "shells": closed.iter().map(|(id, _)| id).collect::<Vec<_>>(),
            }),
        );
    }
    // 06c: the ghost's sbx sandbox dies with the ghost, after its shells do.
    // A ghost that never ran a sandboxed command gets `rm` of a missing
    // sandbox, which reads as `Removed`, so this is harmless for it.
    if config.sandbox.enabled {
        let program = crate::shell::which_sbx().unwrap_or_else(|_| "sbx".to_string());
        let outcome = crate::daemon::executor::ghost_sandbox::teardown_ghost_sandbox(
            &crate::shell::real_runner(program),
            session_id,
        )
        .await;
        crate::daemon::utils::log_event(
            "ghost_sandbox_teardown",
            serde_json::json!({
                "session": session_id,
                "outcome": format!("{outcome:?}"),
            }),
        );
    }
    result
}

/// Register a ghost run for cancellation: by run id, which the stop sentinel
/// fires, and by the ghost's session id, so `Ctrl-c` while attached to one of
/// its shells (owner `ghost:<session>`) reaches it too.
fn register_ghost_run(
    session_id: &str,
    run_id: &RunId,
) -> (
    crate::daemon::run::RunCancelGuard,
    crate::daemon::run::RunCancelSignal,
    crate::daemon::run::ActiveRunGuard,
) {
    let (cancel_guard, signal) = crate::daemon::run::register_run(run_id);
    let active_guard = crate::daemon::run::register_active(session_id, run_id);
    (cancel_guard, signal, active_guard)
}

/// How often a parked ghost checks whether the user has resumed it.
const PAUSE_POLL: Duration = Duration::from_millis(250);

/// Park at a turn boundary while the user has one of this ghost's shells
/// paused from attached mode. Returns the line to tell the model once it has
/// been resumed, or `None` when it was not paused or was cancelled while
/// parked. Both edges are noted in the run log.
async fn park_while_paused(
    run_id: &RunId,
    cancel: &crate::daemon::run::RunCancelSignal,
) -> Option<String> {
    let note = |text: String| {
        append_turn_event(&TurnLogRecord {
            timestamp: chrono::Utc::now().to_rfc3339(),
            run_id: run_id.clone(),
            event: TurnLogEvent::Note { text },
        })
    };
    if crate::daemon::run::is_paused(run_id) {
        note("paused by the user from attached mode".to_string());
        while crate::daemon::run::is_paused(run_id) && !cancel.is_cancelled() {
            tokio::time::sleep(PAUSE_POLL).await;
        }
    }
    if cancel.is_cancelled() {
        return None;
    }
    let (shell, lasted) = crate::daemon::run::take_pause_notice(run_id)?;
    let secs = lasted.as_secs();
    note(format!("resumed after {secs} s"));
    Some(format!(
        "[The user paused shell {shell} for {secs} s from attached mode. It is running again.]"
    ))
}

/// What to tell the model when the user typed into one of this ghost's shells
/// from attached mode since its last turn, or `None` when nothing was typed.
fn typed_notice(run_id: &RunId) -> Option<String> {
    let (shell, bytes) = crate::daemon::run::take_typed_notice(run_id)?;
    Some(format!(
        "[The user typed {bytes} bytes into shell {shell} from attached mode. \
         Read its recent output before relying on what you last saw.]"
    ))
}

/// Add `text` to the ghost's conversation as a user message for `turn`, and
/// persist it, as the wrap-up message is.
fn push_user_note(session_id: &str, sessions: &SessionStore, text: String, turn: usize) {
    let message = crate::ai::Message {
        role: "user".to_string(),
        content: text,
        tool_calls: None,
        tool_results: None,
        turn: Some(turn),
    };
    let pushed = with_sessions(sessions, |store| {
        if let Some(entry) = store.get_mut(session_id) {
            entry.messages.push(message.clone());
            true
        } else {
            false
        }
    });
    if pushed {
        crate::daemon::session::append_session_message(session_id, &message);
    }
}

/// The `PendingCall` a ghost's turn loop collects from an `AiEvent::ToolCall`
/// (F9): ghosts are background-only, so the call becomes `Background` — but a
/// call asking for the interactive launch path is refused as invalid args,
/// with the reason, instead of being silently backgrounded.
fn ghost_pending_for_tool_call(
    id: String,
    cmd: String,
    interactive: bool,
    thought_signature: Option<String>,
    host: Option<String>,
) -> PendingCall {
    if interactive {
        PendingCall::InvalidArgs {
            id,
            name: "run_terminal_command",
            arguments: serde_json::json!({
                "command": cmd,
                "background": false,
                "interactive": true,
                "host": host
            })
            .to_string(),
            error: "interactive: true is not available to a ghost: ghosts run \
                    every command in the background and wait for it to exit, so \
                    a program that never exits (a TUI) cannot be launched. Run a \
                    command that exits."
                .to_string(),
            thought_signature,
        }
    } else {
        PendingCall::Background {
            id,
            cmd,
            thought_signature,
            _credential: None,
            host,
        }
    }
}

fn cancel_stage(run_id: &crate::daemon::run::RunId) -> String {
    crate::daemon::run::take_cancel_door(run_id)
        .map(crate::daemon::run::CancelDoor::stage)
        .unwrap_or("unmarked_cancel")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// F9: a ghost is background-only, so a tool call asking for the
    /// interactive launch path is refused as invalid args with the reason —
    /// never silently backgrounded. The call is otherwise unchanged.
    #[test]
    fn a_ghost_interactive_call_is_refused_with_the_reason() {
        let call = ghost_pending_for_tool_call(
            "tc_1".to_string(),
            "claude".to_string(),
            true,
            Some("sig".to_string()),
            Some("pinky".to_string()),
        );
        let PendingCall::InvalidArgs {
            id,
            name,
            arguments,
            error,
            thought_signature,
        } = call
        else {
            panic!("expected PendingCall::InvalidArgs for the interactive call")
        };
        assert_eq!(id, "tc_1");
        assert_eq!(name, "run_terminal_command");
        assert_eq!(thought_signature, Some("sig".to_string()));
        assert!(
            error.contains("not available to a ghost"),
            "the refusal says why interactive is refused: {error}"
        );
        assert!(
            error.contains("Run a command that exits"),
            "the refusal says what to do instead: {error}"
        );
        let args: serde_json::Value =
            serde_json::from_str(&arguments).expect("the raw arguments parse as JSON");
        assert_eq!(
            args.get("command").and_then(|c| c.as_str()),
            Some("claude"),
            "the raw arguments are kept for history: {arguments}"
        );
        assert_eq!(
            args.get("interactive").and_then(|i| i.as_bool()),
            Some(true),
            "the arguments show the call as the model made it: {arguments}"
        );

        let call = ghost_pending_for_tool_call(
            "tc_2".to_string(),
            "echo hi".to_string(),
            false,
            None,
            None,
        );
        let PendingCall::Background { cmd, .. } = call else {
            panic!("expected PendingCall::Background for a call without interactive")
        };
        assert_eq!(
            cmd, "echo hi",
            "a call without interactive stays a background run"
        );
    }

    #[test]
    fn ghost_start_records_how_its_ssh_target_resolved() {
        let _lock = crate::test_home_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let saved = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", tmp.path()) };
        crate::config::Config::ensure_dirs().expect("ensure dirs");

        let hosts = std::collections::HashMap::from([(
            "db01".to_string(),
            crate::config::HostEntry {
                ssh: "matt@db01.lan".to_string(),
                sudo_scripts: vec!["allowed.sh".to_string()],
                ..Default::default()
            },
        )]);
        let mut gc = crate::ipc::GhostConfig {
            ssh_target: Some("db01".to_string()),
            ..Default::default()
        };

        let resolution = apply_ssh_target_resolution(&mut gc, &hosts, "ghost-resolved");

        assert_eq!(
            resolution,
            Some(crate::host::SshTargetResolution::Configured(
                "db01".to_string()
            )),
            "a target naming a configured host must resolve to it"
        );
        assert_eq!(
            gc.ssh_target_host,
            Some("db01".to_string()),
            "the GhostConfig must carry the resolved host"
        );
        assert_eq!(
            gc.ssh_target_sudo_scripts,
            vec!["allowed.sh".to_string()],
            "the GhostConfig must carry the host's script whitelist"
        );

        let seg = crate::config::current_event_segment_path();
        let content = std::fs::read_to_string(&seg).expect("read the event segment");
        let records: Vec<serde_json::Value> = content
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter(|v| {
                v.get("event").and_then(|e| e.as_str()) == Some("ghost_ssh_target_resolved")
            })
            .collect();
        assert_eq!(
            records.len(),
            1,
            "exactly one resolution event per ghost start:\n{content}"
        );
        let record = &records[0];
        assert_eq!(
            record.get("resolution").and_then(|r| r.as_str()),
            Some("configured"),
            "the event must say which way the value resolved"
        );
        assert_eq!(
            record.get("host").and_then(|h| h.as_str()),
            Some("db01"),
            "the event must carry the resolved host key"
        );
        assert_eq!(
            record
                .get("candidates")
                .and_then(|c| c.as_array())
                .map(Vec::len),
            Some(0),
            "a configured resolution carries no candidates"
        );
        assert_eq!(
            record.get("ssh_target").and_then(|t| t.as_str()),
            Some("db01"),
            "the event must carry the raw ssh_target"
        );
        assert_eq!(
            record.get("session").and_then(|s| s.as_str()),
            Some("ghost-resolved"),
            "the event must carry the ghost session id"
        );

        unsafe {
            match saved {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
    }

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
            (
                outcome.turns,
                outcome.budget_exhausted,
                outcome.cancelled,
                outcome.cost_refused.is_none()
            ),
            (0u32, false, false, true),
            "Default must give zero turns, no flags and no refusal — the wrapper \
             relies on starting from a clean outcome"
        );
    }

    #[test]
    fn ghost_terminal_state_prefers_hard_fail_over_cancel_and_budget() {
        let outcome = GhostLoopOutcome {
            turns: 3,
            budget_exhausted: true,
            cancelled: true,
            cost_refused: None,
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
            cost_refused: None,
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

    #[test]
    fn a_cost_cap_ends_the_ghost_budget_exceeded_usd() {
        let outcome = GhostLoopOutcome {
            turns: 2,
            budget_exhausted: false,
            cancelled: false,
            cost_refused: Some(crate::daemon::spend::refusal_message(5.0, 5.0)),
        };
        let state =
            ghost_terminal_state(&Ok(()), &outcome, "unmarked_cancel".to_string(), "gs-cap");
        match state {
            RunState::BudgetExceeded(briefing) => {
                assert_eq!(
                    briefing.budget_left,
                    Some(BudgetDimension::Usd),
                    "the cost cap ends the run on the Usd dimension"
                );
                assert!(
                    briefing
                        .blocker
                        .as_deref()
                        .is_some_and(|b| b.contains("$5.00") && b.contains("midnight")),
                    "the blocker must be the gate's refusal message, got {:?}",
                    briefing.blocker
                );
            }
            other => panic!("a cost refusal must read BudgetExceeded, got {other:?}"),
        }
    }

    #[test]
    fn cancellation_beats_the_cost_cap() {
        let outcome = GhostLoopOutcome {
            turns: 3,
            budget_exhausted: false,
            cancelled: true,
            cost_refused: Some(crate::daemon::spend::refusal_message(5.0, 5.0)),
        };
        let state = ghost_terminal_state(
            &Ok(()),
            &outcome,
            "stop_sentinel".to_string(),
            "gs-cap-cancel",
        );
        match state {
            RunState::Cancelled { stage, turns } => {
                assert_eq!(
                    stage, "stop_sentinel",
                    "a cancellation and a cost refusal must still read Cancelled"
                );
                assert_eq!(turns, 3);
            }
            other => panic!("cancelled beats the cost cap, got {other:?}"),
        }
    }

    #[test]
    fn the_cost_cap_beats_the_turn_budget() {
        let outcome = GhostLoopOutcome {
            turns: 9,
            budget_exhausted: true,
            cancelled: false,
            cost_refused: Some(crate::daemon::spend::refusal_message(5.0, 5.0)),
        };
        let state = ghost_terminal_state(
            &Ok(()),
            &outcome,
            "unmarked_cancel".to_string(),
            "gs-cap-turns",
        );
        match state {
            RunState::BudgetExceeded(briefing) => {
                assert_eq!(
                    briefing.budget_left,
                    Some(BudgetDimension::Usd),
                    "the cost cap must beat the turn budget: both flags set, the \
                     briefing names the cost cap's dimension"
                );
                assert!(
                    briefing
                        .blocker
                        .as_deref()
                        .is_some_and(|b| b.contains("midnight")),
                    "the blocker must be the cost refusal, not the turn-budget one"
                );
            }
            other => panic!("the cost cap beats the turn budget, got {other:?}"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn park_while_paused_waits_for_the_resume_and_says_how_long() {
        use crate::daemon::run::cancel::test_support::{HomeGuard, fresh_run_id};
        use crate::daemon::run::{CancelReason, cancel_run, pause_run, register_run, resume_run};
        let _home = HomeGuard::new();
        let id = fresh_run_id();
        let (_cancel_guard, signal) = register_run(&id);
        assert_eq!(
            park_while_paused(&id, &signal).await,
            None,
            "a run that is not paused does not wait"
        );

        pause_run(&id, "s5");
        let resumer = tokio::spawn({
            let id = id.clone();
            async move {
                tokio::time::sleep(Duration::from_secs(40)).await;
                resume_run(&id);
            }
        });
        assert_eq!(
            park_while_paused(&id, &signal).await.as_deref(),
            Some("[The user paused shell s5 for 40 s from attached mode. It is running again.]"),
            "it waits for the resume, then says which shell and how long"
        );
        resumer.await.unwrap();

        pause_run(&id, "s5");
        let canceller = tokio::spawn({
            let id = id.clone();
            async move {
                tokio::time::sleep(Duration::from_secs(1)).await;
                cancel_run(&id, CancelReason::UserStop);
            }
        });
        assert_eq!(
            park_while_paused(&id, &signal).await,
            None,
            "a cancel ends the wait, and a cancelled run is told nothing"
        );
        canceller.await.unwrap();
    }

    #[test]
    fn typed_notice_tells_the_ghost_where_the_user_typed() {
        use crate::daemon::run::cancel::test_support::fresh_run_id;
        let id = fresh_run_id();
        assert_eq!(typed_notice(&id), None, "nothing typed, nothing said");
        crate::daemon::run::note_typed(&id, "s5", 12);
        assert_eq!(
            typed_notice(&id).as_deref(),
            Some(
                "[The user typed 12 bytes into shell s5 from attached mode. \
                 Read its recent output before relying on what you last saw.]"
            )
        );
        assert_eq!(typed_notice(&id), None, "said once");
    }

    #[test]
    fn ghost_run_is_cancellable_from_an_attached_shell() {
        use crate::daemon::run::cancel::test_support::{HomeGuard, fresh_run_id};
        let _home = HomeGuard::new();
        let id = fresh_run_id();
        let session = "gs-attach-cancel-1";
        let (_cancel_guard, signal, _active_guard) = register_ghost_run(session, &id);
        assert_eq!(
            crate::daemon::run::cancel_owner_run(
                &format!("ghost:{session}"),
                crate::daemon::run::CancelReason::UserStop
            ),
            Some(id.clone()),
            "the ghost's shell owner key reaches the ghost's run"
        );
        assert!(signal.is_cancelled());
        assert_eq!(cancel_stage(&id), "user_attached");
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

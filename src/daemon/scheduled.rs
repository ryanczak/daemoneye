use crate::ai::{AiEvent, Message};
use crate::config::Config;
use crate::daemon::ghost::{GhostManager, check_ghost_capacity, trigger_ghost_turn};
use crate::daemon::run::{Briefing, RunId, RunKind, RunLogGuard, RunState};
use crate::daemon::session::*;
use crate::daemon::utils::*;
use crate::ipc::Response;
use crate::scheduler::{ActionOn, JobStatus, ScheduleStore, ScheduledJob};
use crate::scripts;
use crate::shell::{Owner, ShellRegistry};
use crate::webhook::inject_ghost_event;
use std::sync::Arc;
use std::time::Duration;

/// The terminal [`RunState`] a scheduled job's run records, from the status the
/// store holds after the job ran. Pure: takes the status and the job name, not
/// the store, so its tests need no filesystem.
///
/// Note that a successful **`Every`** job does *not* read back as `Succeeded`:
/// the store's `mark_done` advances it and it returns to `Pending`. `Pending`
/// therefore maps to [`RunState::Complete`], not to "never finished".
fn scheduled_terminal_state(status: Option<&JobStatus>, job_name: &str) -> RunState {
    let hard_fail = |blocker: String| {
        RunState::HardFail(Briefing::new(
            format!("scheduled job {job_name}"),
            None,
            Vec::new(),
            Vec::new(),
            Some(blocker),
            None,
        ))
    };
    match status {
        // A successful recurring (`Every`/`Cron`) job lands here too, because
        // it reschedules itself back to Pending as soon as it is marked done.
        Some(JobStatus::Pending) | Some(JobStatus::Succeeded) => RunState::Complete,
        Some(JobStatus::Failed(msg)) => hard_fail(msg.clone()),
        Some(JobStatus::Running) => hard_fail("the job never recorded an outcome".to_string()),
        Some(JobStatus::Cancelled) => RunState::Cancelled {
            stage: "job_cancelled".into(),
            turns: 0,
        },
        None => hard_fail("the job left the store before it finished".to_string()),
    }
}

/// The terminal [`RunState`] a scheduled job's run records: a watchdog
/// refusal (`Some`) ends the run `BudgetExceeded` with the message as the
/// blocker and the `Usd` dimension; without one (`None`) the job's status
/// alone decides. Pure — the shape [`run_scheduled_job`] applies, so its
/// test runs the same mapping the caller does.
fn scheduled_run_state(
    refusal: Option<String>,
    status: Option<&JobStatus>,
    job_name: &str,
) -> RunState {
    match refusal {
        Some(message) => RunState::BudgetExceeded(Briefing::new(
            format!("scheduled job {job_name}"),
            None,
            Vec::new(),
            Vec::new(),
            Some(message),
            Some(crate::daemon::run::BudgetDimension::Usd),
        )),
        None => scheduled_terminal_state(status, job_name),
    }
}

/// Why a `Command` job refuses to fire. One string, so the `log_event` field
/// and the failure notification name the same migration sentence.
const COMMAND_REFUSAL_REASON: &str =
    "ActionOn::Command is no longer supported; migrate the job to ActionOn::Script";

/// The failure reason a job records when its script cannot be resolved.
fn resolve_failure(job_name: &str, reason: &str) -> String {
    format!("Scheduled job '{job_name}' failed: {reason}")
}

/// Send the job's result to any listening chat client and fire
/// `[notifications] on_alert`. The one notify tail a scheduled job has, so
/// the success, failure and refusal paths cannot diverge on it. The message
/// is also broadcast to every attached chat over the presence connection
/// (M27 phase 03b); delivering to nobody is the normal no-chat case.
async fn notify_job_result(
    job_name: &str,
    msg: String,
    config: &Config,
    notify_tx: &Option<tokio::sync::mpsc::UnboundedSender<Response>>,
) {
    if let Some(tx) = notify_tx.as_ref()
        && let Err(e) = tx.send(Response::SystemMsg(msg.clone()))
    {
        log::debug!(
            "scheduled job '{}': dropped notification (no receiver): {}",
            job_name,
            e
        );
    }
    presence_broadcast(&msg);
    fire_notification(job_name, &msg, config);
}

/// Mark the job failed, notify, and return. The one failure tail, shared by
/// every path that cannot run the script.
async fn fail_job(
    job: &ScheduledJob,
    store: &Arc<ScheduleStore>,
    config: &Config,
    notify_tx: &Option<tokio::sync::mpsc::UnboundedSender<Response>>,
    reason: String,
) {
    store.mark_done(&job.id, false, Some(reason.clone()));
    notify_job_result(&job.name, reason, config, notify_tx).await;
}

/// Jobs due this tick, whatever the daemon's adopted session name is.
///
/// An empty session name is normal: the daemon can run without any chat
/// client connected, and script/ghost jobs fire anyway (a live rehearsal on
/// pinky proved the webhook ghost path does).
pub(crate) fn due_jobs_for_tick(store: &ScheduleStore, _session: &str) -> Vec<ScheduledJob> {
    store.take_due()
}

/// Run a single scheduled job.
///
/// - `ActionOn::Ghost`: spawns a full Ghost Shell session using the named runbook.
/// - `ActionOn::Alert`: emits a `SystemMsg` notification only.
/// - `ActionOn::Script`: runs the resolved script in a shell owned by
///   [`Owner::Scheduler`] on the daemon host — a PTY shell, no tmux — and
///   optionally runs watchdog analysis against a runbook.
/// - `ActionOn::Command` (removed): loads from old `schedules.json` files but
///   refuses to fire, with a logged reason and a failure notification.
pub async fn run_scheduled_job(
    job: ScheduledJob,
    store: Arc<ScheduleStore>,
    session: String,
    sessions: SessionStore,
    config: Config,
    cache: Arc<crate::tmux::cache::SessionCache>,
    notify_tx: Option<tokio::sync::mpsc::UnboundedSender<Response>>,
) -> Result<(), String> {
    let job_id = job.id.clone();
    let job_name = job.name.clone();
    let mut run_log = RunLogGuard::start(
        RunId::new(),
        RunKind::Scheduled {
            job: job_name.clone(),
        },
    );
    let refusal = run_scheduled_job_inner(
        job,
        store.clone(),
        session,
        sessions,
        config,
        cache,
        notify_tx,
    )
    .await;
    let status = store
        .list()
        .into_iter()
        .find(|j| j.id == job_id)
        .map(|j| j.status);
    // A refused watchdog analysis ends the scheduled run `BudgetExceeded` in
    // place of the state the job status alone would give — the job itself
    // still ran and is still marked done as now.
    let state = scheduled_run_state(refusal, status.as_ref(), &job_name);
    run_log.finish(state);
    Ok(())
}

async fn run_scheduled_job_inner(
    job: ScheduledJob,
    store: Arc<ScheduleStore>,
    _session: String,
    sessions: SessionStore,
    config: Config,
    cache: Arc<crate::tmux::cache::SessionCache>,
    notify_tx: Option<tokio::sync::mpsc::UnboundedSender<Response>>,
) -> Option<String> {
    crate::daemon::stats::inc_schedules_executed();

    // Ghost-mode: hand off entirely to the ghost shell infrastructure.
    if let ActionOn::Ghost { runbook: rb_name } = &job.action {
        if !check_ghost_capacity(&config) {
            log::warn!(
                "Scheduled ghost job '{}': skipped — concurrency limit ({}) reached",
                job.name,
                config.ghost.max_concurrent_ghosts
            );
            inject_ghost_event(
                &sessions,
                &format!(
                    "[Ghost Shell Skipped] Scheduled job '{}' skipped — concurrency limit reached",
                    job.name
                ),
            )
            .await;
            store.mark_done(
                &job.id,
                false,
                Some("ghost concurrency limit reached".to_string()),
            );
            return None;
        }

        let alert_msg = format!(
            "Scheduled job '{}' fired ({})",
            job.name,
            job.kind.describe()
        );
        match crate::runbook::load_runbook(rb_name) {
            Err(e) => {
                let msg = format!(
                    "Scheduled ghost job '{}': failed to load runbook '{}': {}",
                    job.name, rb_name, e
                );
                log::error!("{}", msg);
                store.mark_done(&job.id, false, Some(msg));
            }
            Ok(rb) => {
                let merged_config = crate::agents::merge_runbook_ghost_config(&rb);
                match GhostManager::start_session_with_config(
                    sessions.clone(),
                    &rb,
                    &merged_config,
                    &alert_msg,
                    crate::daemon::GS_SCHED_WINDOW_PREFIX,
                    config.approvals.ghost_commands,
                )
                .await
                {
                    Err(e) => {
                        let msg = format!(
                            "Scheduled ghost job '{}': failed to start session: {}",
                            job.name, e
                        );
                        log::error!("{}", msg);
                        inject_ghost_event(
                            &sessions,
                            &format!(
                                "[Ghost Shell Failed] Scheduled job '{}' could not start: {}",
                                job.name, e
                            ),
                        )
                        .await;
                        store.mark_done(&job.id, false, Some(msg));
                    }
                    Ok(sid) => {
                        let session_log = crate::daemon::session::session_file(&sid)
                            .display()
                            .to_string();
                        inject_ghost_event(
                            &sessions,
                            &format!(
                                "[Ghost Shell Started] Scheduled job '{}' started autonomous session — session log: {}",
                                job.name, session_log
                            ),
                        )
                        .await;
                        let result = trigger_ghost_turn(
                            &sid,
                            &sessions,
                            &config,
                            &cache,
                            &Arc::clone(&store),
                        )
                        .await;
                        match result {
                            Ok(()) => {
                                inject_ghost_event(
                                    &sessions,
                                    &format!(
                                        "[Ghost Shell Completed] Scheduled job '{}' finished — session log: {}",
                                        job.name, session_log
                                    ),
                                )
                                .await;
                                store.mark_done(&job.id, true, None);
                            }
                            Err(e) => {
                                log::error!("Scheduled ghost job '{}' failed: {}", job.name, e);
                                inject_ghost_event(
                                    &sessions,
                                    &format!(
                                        "[Ghost Shell Failed] Scheduled job '{}' error: {} — session log: {}",
                                        job.name, e, session_log
                                    ),
                                )
                                .await;
                                store.mark_done(
                                    &job.id,
                                    false,
                                    Some(format!("ghost error: {}", e)),
                                );
                            }
                        }
                    }
                }
            }
        }
        return None;
    }

    if matches!(job.action, ActionOn::Script(_)) {
        crate::daemon::stats::inc_scripts_executed();
    }

    match &job.action {
        ActionOn::Alert => {
            // Pure alert: no command to run.
            store.mark_done(&job.id, true, None);
            let msg = format!("Watchdog alert: {}", job.name);
            notify_job_result(&job.name, msg, &config, &notify_tx).await;
        }
        ActionOn::Command(_) => {
            // Removed: the job still loads from old schedules.json files but
            // never fires. The reason is logged and notified so the operator
            // sees why the job now fails.
            log::warn!(
                "Scheduled job '{}' refused to fire: {}",
                job.name,
                COMMAND_REFUSAL_REASON
            );
            log_event(
                "scheduled_job_refused",
                serde_json::json!({
                    "job_id": job.id,
                    "job_name": job.name,
                    "reason": COMMAND_REFUSAL_REASON,
                }),
            );
            fail_job(
                &job,
                &store,
                &config,
                &notify_tx,
                format!("refused: {}", COMMAND_REFUSAL_REASON),
            )
            .await;
        }
        ActionOn::Script(s) => {
            let host_path = match scripts::resolve_script(s) {
                Ok(path) => path,
                Err(e) => {
                    let msg = resolve_failure(&job.name, &format!("{e:#}"));
                    log::error!("{}", msg);
                    fail_job(&job, &store, &config, &notify_tx, msg).await;
                    return None;
                }
            };
            run_script_job(
                &job,
                &host_path.to_string_lossy(),
                &store,
                &config,
                &notify_tx,
            )
            .await;
        }
        ActionOn::Ghost { .. } => {}
    }
    None
}

/// Run an `ActionOn::Script` job: resolve, then run the script in a
/// scheduler-owned PTY shell on the daemon host — inside a sandbox when
/// `[sandbox] enabled` — and record the job's result from the real exit code
/// the PTY gives back.
async fn run_script_job(
    job: &ScheduledJob,
    host_path: &str,
    store: &Arc<ScheduleStore>,
    config: &Config,
    notify_tx: &Option<tokio::sync::mpsc::UnboundedSender<Response>>,
) -> Option<String> {
    let owner = Owner::Scheduler(job.id.clone());
    let reg = crate::shell::registry();
    let script_name = std::path::Path::new(host_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(host_path)
        .to_string();

    let cmd_id = crate::daemon::stats::start_command(host_path, "scheduled");
    let (output, exit_code) = if config.sandbox.enabled {
        run_script_in_sandbox(reg, &owner, job, host_path, &script_name, config).await
    } else {
        run_script_on_host(reg, owner.clone(), host_path).await
    };
    crate::daemon::stats::finish_command(cmd_id, exit_code);

    // The job's shells and sandbox die with the run, as a ghost's do.
    let closed =
        crate::daemon::executor::knowledge::close_owner_shells(reg, &owner, Duration::from_secs(5))
            .await;
    if !closed.is_empty() {
        log_event(
            "scheduled_shells_closed",
            serde_json::json!({
                "job_id": job.id,
                "shells": closed.iter().map(|(id, _)| id).collect::<Vec<_>>(),
            }),
        );
    }
    if config.sandbox.enabled {
        let program = crate::shell::which_sbx().unwrap_or_else(|_| "sbx".to_string());
        let name = crate::daemon::executor::sbx::sandbox_name(
            crate::daemon::executor::sbx::SandboxKind::Ghost,
            &job.id,
        );
        let outcome = crate::daemon::executor::ghost_sandbox::forget_and_teardown(
            &crate::shell::real_runner(program),
            &name,
        )
        .await;
        log_event(
            "scheduled_sandbox_teardown",
            serde_json::json!({
                "job_id": job.id,
                "outcome": format!("{outcome:?}"),
            }),
        );
    }

    let normalized = normalize_output(&output);
    let success = exit_code == 0;
    let watchdog_refusal = run_watchdog(job, &normalized, config, notify_tx).await;
    store.mark_done(
        &job.id,
        success,
        if success {
            None
        } else {
            Some(format!("exit code {}", exit_code))
        },
    );
    let status_word = if success { "succeeded" } else { "failed" };
    let msg = format!(
        "`{}` {} (exit {})\n<output>\n{}\n</output>",
        script_name, status_word, exit_code, normalized
    );
    notify_job_result(&job.name, msg, config, notify_tx).await;
    watchdog_refusal.err()
}

/// The PTY run: the resolved script in the job's own shell on the daemon
/// host, with the ghost helper's shape — reuse the owner's shell, real exit
/// code back. [`run_script_on_host_with`] is the seam the tests drive;
/// [`run_script_on_host`] is the production call.
///
/// The host path is [`run_ghost_remote`] with `HostId::Local`: a **host**
/// shell's `sandbox` stays `None`, so the marker protocol uses the login
/// shell's exit-variable syntax rather than a sandbox's pinned `sh`. An
/// empty sandbox name through [`run_ghost_in_sandbox`] would register the
/// shell as `sandbox: Some("")` and get the wrong marker shell — bug-02-1.
async fn run_script_on_host_with<F, Fut>(
    reg: &ShellRegistry,
    owner: Owner,
    cmd: &str,
    spawn: F,
) -> (String, i32)
where
    F: FnOnce(crate::shell::ShellId) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    crate::daemon::executor::ghost_remote::run_ghost_remote(
        reg,
        owner,
        crate::host::HostId::Local,
        cmd,
        "scheduled",
        spawn,
    )
    .await
}

/// The production PTY run: a detached `daemoneye shell-host` (the same spawn
/// `run_background`'s local-ghost PTY branch passes) in the job's own shell.
/// `spawn_detached_host` resolves `current_exe()`, so it cannot run under a
/// test — the seam is what makes the host path testable at all.
async fn run_script_on_host(reg: &ShellRegistry, owner: Owner, cmd: &str) -> (String, i32) {
    run_script_on_host_with(reg, owner, cmd, |shell_id| {
        let (rows, cols) = crate::shell::spawn_size();
        crate::shell::spawn_detached_host(shell_id, &[], rows, cols)
    })
    .await
}

/// The sandboxed run: the ghost sandbox lifecycle, keyed by the job id, with
/// the resolved script staged in and the reason carried on failure. The spawn
/// step is the test seam, as [`run_script_on_host_with`]'s is.
async fn run_script_in_sandbox(
    reg: &ShellRegistry,
    owner: &Owner,
    job: &ScheduledJob,
    host_path: &str,
    script_name: &str,
    config: &Config,
) -> (String, i32) {
    let program = crate::shell::which_sbx().unwrap_or_else(|_| "sbx".to_string());
    let run = crate::shell::real_runner(program);
    let ensured = crate::daemon::executor::ghost_sandbox::ensure_sandbox(
        &run,
        &config.sandbox,
        crate::daemon::executor::sbx::SandboxKind::Ghost,
        &job.id,
        None,
    )
    .await;
    let sandbox = match ensured {
        Ok(name) => name,
        Err(reason) => return (reason, 1),
    };
    if let Err(reason) =
        crate::daemon::executor::ghost_sandbox::stage_script(&run, &sandbox, script_name, host_path)
            .await
    {
        return (reason, 1);
    }
    let cmd_to_run = crate::daemon::executor::sbx::staged_script_command(script_name, "");
    crate::daemon::executor::ghost_sandbox::run_ghost_in_sandbox(
        reg,
        owner.clone(),
        &sandbox,
        &cmd_to_run,
        "scheduled",
        |shell_id| {
            let (rows, cols) = crate::shell::spawn_size();
            let s = sandbox.clone();
            async move { crate::shell::spawn_sbx_host(shell_id, &s, rows, cols).await }
        },
    )
    .await
}

/// The job's runbook watchdog analysis, on the PTY output instead of a
/// captured pane. Unchanged from the window path except for its input.
///
/// The daily-cap gate runs before the model call: over
/// `[budget] max_cost_per_day` the analysis is skipped and logged, the job
/// itself still runs and is still marked done, and the caller ends the
/// scheduled run `BudgetExceeded` in place of the state
/// [`scheduled_terminal_state`] would give.
async fn run_watchdog(
    job: &ScheduledJob,
    output: &str,
    config: &Config,
    notify_tx: &Option<tokio::sync::mpsc::UnboundedSender<Response>>,
) -> Result<(), String> {
    let Some(rb_name) = job.runbook.as_ref() else {
        return Ok(());
    };
    let Ok(rb) = crate::runbook::load_runbook(rb_name) else {
        return Ok(());
    };
    if let Err(message) =
        crate::daemon::spend::cost_gate(config, crate::daemon::spend::SpendPath::Scheduled)
    {
        log::warn!(
            "Scheduled job '{}': watchdog analysis skipped — daily cost cap: {message}",
            job.name
        );
        return Err(message);
    }
    let model_entry = config.resolve_model(None);
    let client = crate::ai::make_client(
        &model_entry.provider,
        model_entry.resolve_api_key(),
        model_entry.model.clone(),
        model_entry.effective_base_url(),
        model_entry.effective_max_tokens(),
    );
    let system = crate::runbook::watchdog_system_prompt(&rb);
    let msgs = vec![Message {
        role: "user".to_string(),
        content: format!("Command output:\n```\n{output}\n```"),
        tool_calls: None,
        tool_results: None,
        turn: None,
    }];
    let (ai_tx, mut ai_rx) = tokio::sync::mpsc::unbounded_channel::<AiEvent>();
    let api_err = client
        .chat(&system, msgs, ai_tx, false, Vec::new())
        .await
        .is_err();
    let mut ai_response = String::new();
    while let Some(ev) = ai_rx.recv().await {
        match ev {
            AiEvent::Token(t) => ai_response.push_str(&t),
            AiEvent::Done(usage) => {
                crate::cost::emit_cost(crate::cost::CostCall {
                    model_entry,
                    usage: &usage,
                    attribution: &crate::cost::side_call_attribution("watchdog"),
                    session_id: "-",
                    run_id: "",
                    now: chrono::Utc::now(),
                });
            }
            _ => {}
        }
    }
    let (should_act, trigger_reason) =
        crate::webhook::evaluate_watchdog_response(&ai_response, api_err);
    log::info!(
        "Scheduler watchdog for '{}': should_act={} reason='{}'",
        job.name,
        should_act,
        trigger_reason
    );
    if should_act {
        let msg = format!("[Watchdog] {}: {}", job.name, ai_response.trim());
        notify_job_result(&job.name, msg, config, notify_tx).await;
    }
    Ok(())
}

#[cfg(test)]
#[path = "scheduled_tests.rs"]
mod tests;

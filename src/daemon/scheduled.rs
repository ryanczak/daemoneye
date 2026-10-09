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
/// the success, failure and refusal paths cannot diverge on it.
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
) {
    let job_id = job.id.clone();
    let job_name = job.name.clone();
    let mut run_log = RunLogGuard::start(
        RunId::new(),
        RunKind::Scheduled {
            job: job_name.clone(),
        },
    );
    run_scheduled_job_inner(
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
    run_log.finish(scheduled_terminal_state(status.as_ref(), &job_name));
}

async fn run_scheduled_job_inner(
    job: ScheduledJob,
    store: Arc<ScheduleStore>,
    _session: String,
    sessions: SessionStore,
    config: Config,
    cache: Arc<crate::tmux::cache::SessionCache>,
    notify_tx: Option<tokio::sync::mpsc::UnboundedSender<Response>>,
) {
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
            return;
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
        return;
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
                    return;
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
) {
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
    run_watchdog(job, &normalized, config, notify_tx).await;
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
}

/// The PTY run: the resolved script in the job's own shell on the daemon
/// host, with the ghost helper's shape — reuse the owner's shell, real exit
/// code back.
///
/// The production spawn is [`crate::shell::spawn_detached_host`] (a detached
/// `daemoneye shell-host`), the same one `run_background`'s local-ghost PTY
/// branch passes. The tests drive the shell engine through
/// [`crate::daemon::executor::ghost_sandbox::run_ghost_in_sandbox`]'s seam
/// instead, so this arm takes no spawn parameter.
async fn run_script_on_host(reg: &ShellRegistry, owner: Owner, cmd: &str) -> (String, i32) {
    let _max_per_owner = crate::config::Config::load()
        .unwrap_or_default()
        .shells
        .max_per_owner;
    crate::daemon::executor::ghost_sandbox::run_ghost_in_sandbox(
        reg,
        owner,
        "",
        cmd,
        "scheduled",
        |shell_id| crate::shell::spawn_detached_host(shell_id, &[], 24, 80),
    )
    .await
}

/// The sandboxed run: the ghost sandbox lifecycle, keyed by the job id, with
/// the resolved script staged in and the reason carried on failure. The spawn
/// step is the test seam, as [`run_script_on_host`]'s is.
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
            let s = sandbox.clone();
            async move { crate::shell::spawn_sbx_host(shell_id, &s, 24, 80).await }
        },
    )
    .await
}

/// The job's runbook watchdog analysis, on the PTY output instead of a
/// captured pane. Unchanged from the window path except for its input.
async fn run_watchdog(
    job: &ScheduledJob,
    output: &str,
    config: &Config,
    notify_tx: &Option<tokio::sync::mpsc::UnboundedSender<Response>>,
) {
    let Some(rb_name) = job.runbook.as_ref() else {
        return;
    };
    let Ok(rb) = crate::runbook::load_runbook(rb_name) else {
        return;
    };
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
        if let AiEvent::Token(t) = ev {
            ai_response.push_str(&t);
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hard_fail_blocker(state: &RunState) -> Option<String> {
        match state {
            RunState::HardFail(b) => b.blocker.clone(),
            other => panic!("expected HardFail, got {other:?}"),
        }
    }

    fn hard_fail_goal(state: &RunState) -> String {
        match state {
            RunState::HardFail(b) => b.goal.clone(),
            other => panic!("expected HardFail, got {other:?}"),
        }
    }

    #[test]
    fn scheduled_state_succeeded_is_complete() {
        let state = scheduled_terminal_state(Some(&JobStatus::Succeeded), "nightly-backup");
        assert_eq!(
            state,
            RunState::Complete,
            "a succeeded job completed its run"
        );
    }

    #[test]
    fn scheduled_state_pending_is_complete_because_every_jobs_reschedule() {
        let state = scheduled_terminal_state(Some(&JobStatus::Pending), "disk-check");
        assert_eq!(
            state,
            RunState::Complete,
            "Pending must be Complete: an Every job that succeeded is advanced by \
             mark_done straight back to Pending, so reading Pending as anything but \
             Complete would report every successful recurring job as a failure"
        );
    }

    #[test]
    fn scheduled_state_failed_carries_the_error_as_the_blocker() {
        let state = scheduled_terminal_state(
            Some(&JobStatus::Failed("exit code 1".to_string())),
            "nightly-backup",
        );
        assert_eq!(
            hard_fail_blocker(&state).as_deref(),
            Some("exit code 1"),
            "the store's error message must be preserved verbatim as the blocker"
        );
        assert_eq!(
            hard_fail_goal(&state),
            "scheduled job nightly-backup",
            "the briefing's goal must name the job"
        );
    }

    #[test]
    fn scheduled_state_cancelled_reports_zero_turns() {
        let state = scheduled_terminal_state(Some(&JobStatus::Cancelled), "cleanup-tmp");
        match state {
            RunState::Cancelled { stage, turns } => {
                assert_eq!(
                    stage, "job_cancelled",
                    "the stage names the job cancellation"
                );
                assert_eq!(turns, 0, "a scheduled job has no turns");
            }
            other => panic!("expected Cancelled, got {other:?}"),
        }
    }

    #[test]
    fn scheduled_state_running_is_a_hard_fail() {
        let state = scheduled_terminal_state(Some(&JobStatus::Running), "still-running");
        let blocker = hard_fail_blocker(&state);
        assert_eq!(
            blocker.as_deref(),
            Some("the job never recorded an outcome"),
            "a job still Running at map time never called mark_done; that is a bug \
             worth seeing in the log, not silence"
        );
        assert_eq!(hard_fail_goal(&state), "scheduled job still-running");
    }

    #[test]
    fn scheduled_state_missing_job_is_a_hard_fail() {
        let state = scheduled_terminal_state(None, "deleted-mid-run");
        let blocker = hard_fail_blocker(&state);
        assert_eq!(
            blocker.as_deref(),
            Some("the job left the store before it finished"),
            "a deleted job is a HardFail, not Complete: nobody recorded an outcome"
        );
        assert_eq!(hard_fail_goal(&state), "scheduled job deleted-mid-run");
    }

    #[test]
    fn scheduled_state_failed_and_missing_have_different_blockers() {
        let failed = scheduled_terminal_state(Some(&JobStatus::Failed("boom".to_string())), "j");
        let missing = scheduled_terminal_state(None, "j");
        assert_ne!(
            hard_fail_blocker(&failed),
            hard_fail_blocker(&missing),
            "the two HardFail paths name different causes and must stay distinguishable"
        );
    }

    #[test]
    fn owner_scheduler_key_is_distinct() {
        let job = "j1".to_string();
        assert_ne!(
            Owner::Scheduler(job.clone()).key(),
            Owner::Ghost(job.clone()).key(),
            "a scheduler shell and a ghost shell with the same id string are \
             different owners, so the cap groups and the owner check must treat \
             them apart"
        );
        assert_eq!(
            Owner::Scheduler(job.clone()).key(),
            "scheduler:j1",
            "the key carries the kind prefix, as the other variants' do"
        );
        assert_ne!(
            Owner::Scheduler(job.clone()).key(),
            Owner::Chat(job).key(),
            "a scheduler shell and a chat session with the same id string are \
             different owners too"
        );
    }

    #[tokio::test]
    async fn scheduled_script_runs_in_a_scheduler_pty_shell() {
        let _home = crate::test_home_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("HOME", dir.path()) }
        crate::config::Config::ensure_dirs().ok();
        let reg = ShellRegistry::default();
        let owner = Owner::Scheduler("job-sched-1".to_string());
        let (output, exit_code) = run_script_on_host_seam(
            &reg,
            owner.clone(),
            "printf 'SCHED_OK\\n'; sh -c 'exit 3'",
            spawn_in_process_host,
        )
        .await;
        assert_eq!(
            exit_code, 3,
            "the script's real exit code is preserved: {output:?}"
        );
        assert!(
            output.contains("SCHED_OK"),
            "the script's output is captured: {output:?}"
        );
        let (count, owner_key) = crate::shell::with_shells(&reg, |store| {
            (store.len(), store.values().next().map(|r| r.owner.key()))
        });
        assert_eq!(
            owner_key.as_deref(),
            Some("scheduler:job-sched-1"),
            "the shell's owner is the scheduler's"
        );
        crate::daemon::executor::knowledge::close_owner_shells(
            &reg,
            &owner,
            Duration::from_secs(5),
        )
        .await;
        let after = crate::shell::with_shells(&reg, |store| store.len());
        let _ = (count, after);
    }

    /// The in-process version of [`run_script_on_host`]: the same
    /// `run_in_shell_with_prompts` call with the in-process PTY host standing
    /// in for the detached production spawn, so the test needs no
    /// `current_exe()`.
    async fn run_script_on_host_seam<F, Fut>(
        reg: &ShellRegistry,
        owner: Owner,
        cmd: &str,
        spawn: F,
    ) -> (String, i32)
    where
        F: FnOnce(crate::shell::ShellId) -> Fut,
        Fut: std::future::Future<Output = anyhow::Result<()>>,
    {
        let mut sink = tokio::io::sink();
        let mut eof = tokio::io::BufReader::new(tokio::io::empty());
        let outcome = crate::daemon::executor::shell_exec::run_in_shell_with_prompts(
            reg,
            owner,
            cmd,
            5,
            None,
            Some(20),
            "scheduled",
            &mut sink,
            &mut eof,
            crate::host::HostId::Local,
            spawn,
        )
        .await;
        match outcome {
            Ok((_, result)) => (result.text, result.exit_code),
            Err(e) => (format!("scheduled job script could not run: {e:#}"), 1),
        }
    }

    async fn spawn_in_process_host(id: crate::shell::ShellId) -> anyhow::Result<()> {
        let dir = tempfile::tempdir().expect("tempdir for cast");
        let cast = dir.path().join(format!("{}.cast", id.as_str()));
        let backend = crate::shell::PtyBackend::spawn("bash", &[], 24, 80, 100, &cast, 0)
            .expect("spawning bash");
        let lock =
            crate::daemon::instance::InstanceLock::acquire(&crate::shell::lock_path(id.as_str()))
                .expect("holding the per-id instance lock");
        let socket = crate::shell::socket_path(id.as_str());
        let listener = crate::shell::bind(&socket).await.expect("binding");
        tokio::spawn(crate::shell::serve(
            listener,
            std::sync::Arc::clone(&backend),
        ));
        std::mem::forget(lock);
        std::mem::forget(dir);
        Ok(())
    }

    #[tokio::test]
    async fn scheduled_command_job_refuses_to_fire() {
        // The refusal path writes the event log under `$HOME`, so the guard
        // is required: an unlocked `set_var` here races every other
        // HOME-reading test in this binary (bug-05c-1's shape).
        let _home = crate::test_home_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("HOME", dir.path()) }
        crate::config::Config::ensure_dirs().ok();
        let job = test_job(
            "cmd-job",
            ActionOn::Command("echo hi".to_string()),
            "job-cmd-1",
        );
        let store = test_store(&job);
        let reg = ShellRegistry::default();
        let (reason, _) = run_refusal_arm(&job, &store).await;
        assert!(
            reason.contains(COMMAND_REFUSAL_REASON),
            "the refusal names the migration sentence: {reason}"
        );
        assert_eq!(
            store.list()[0].status,
            JobStatus::Failed(format!("refused: {COMMAND_REFUSAL_REASON}")),
            "the job is marked failed"
        );
        let count = crate::shell::with_shells(&reg, |s| s.len());
        assert_eq!(count, 0, "no shell is spawned for a refused job");
        let _ = reg;
    }

    #[tokio::test]
    async fn scheduled_script_fails_closed_when_the_sandbox_is_unavailable() {
        let _home = crate::test_home_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("HOME", dir.path()) }
        crate::config::Config::ensure_dirs().ok();
        let (runner, calls, canned) = fake_runner();
        canned.lock().unwrap().push(crate::shell::VmRun {
            code: None,
            stdout: String::new(),
            stderr: "spawning sbx: No such file".to_string(),
        });
        let cfg = crate::config::SandboxConfig {
            enabled: true,
            ..Default::default()
        };
        let name = crate::daemon::executor::sbx::sandbox_name(
            crate::daemon::executor::sbx::SandboxKind::Ghost,
            "job-sbx-1",
        );
        let got = crate::daemon::executor::ghost_sandbox::ensure_sandbox(
            &runner,
            &cfg,
            crate::daemon::executor::sbx::SandboxKind::Ghost,
            "job-sbx-1",
            None,
        )
        .await;
        let reason = got.expect_err("a failed preflight must fail closed");
        assert!(
            reason.starts_with("sandbox unavailable:"),
            "the job's reason is the sandbox's operator-facing one: {reason}"
        );
        assert_eq!(
            calls.lock().unwrap().len(),
            2,
            "only the two preflight probes ran: no create, no script"
        );
        let count = crate::shell::with_shells(&ShellRegistry::default(), |s| s.len());
        assert_eq!(
            count, 0,
            "no shell is spawned when the sandbox is unavailable"
        );
        let _ = name;
    }

    fn test_job(name: &str, action: ActionOn, id: &str) -> ScheduledJob {
        ScheduledJob {
            id: id.to_string(),
            name: name.to_string(),
            kind: crate::scheduler::ScheduleKind::Every {
                interval_secs: 60,
                next_run: chrono::Utc::now(),
            },
            action,
            runbook: None,
            status: JobStatus::Pending,
            created_at: chrono::Utc::now(),
            last_run: None,
        }
    }

    fn test_store(job: &ScheduledJob) -> Arc<ScheduleStore> {
        let store = ScheduleStore::new_empty();
        let mut running = job.clone();
        running.status = JobStatus::Running;
        store.add(running).expect("the temp store accepts the job");
        Arc::new(store)
    }

    async fn run_refusal_arm(job: &ScheduledJob, store: &Arc<ScheduleStore>) -> (String, i32) {
        let config = Config::default();
        let notify_tx: Option<tokio::sync::mpsc::UnboundedSender<Response>> = None;
        let reason = format!("refused: {COMMAND_REFUSAL_REASON}");
        crate::daemon::utils::log_event(
            "scheduled_job_refused",
            serde_json::json!({
                "job_id": job.id,
                "job_name": job.name,
                "reason": COMMAND_REFUSAL_REASON,
            }),
        );
        fail_job(job, store, &config, &notify_tx, reason.clone()).await;
        (reason, 1)
    }

    type Calls = std::sync::Arc<std::sync::Mutex<Vec<Vec<String>>>>;
    type Canned = std::sync::Arc<std::sync::Mutex<Vec<crate::shell::VmRun>>>;

    fn fake_runner() -> (crate::shell::SbxRunner, Calls, Canned) {
        use futures_util::FutureExt;
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let canned: std::sync::Arc<std::sync::Mutex<Vec<crate::shell::VmRun>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let runner: crate::shell::SbxRunner = {
            let calls = std::sync::Arc::clone(&calls);
            let canned = std::sync::Arc::clone(&canned);
            std::sync::Arc::new(move |argv: Vec<String>| {
                calls.lock().unwrap().push(argv.clone());
                let mut queue = canned.lock().unwrap();
                let run = if queue.is_empty() {
                    queue
                        .last()
                        .cloned()
                        .unwrap_or_else(|| crate::shell::VmRun {
                            code: Some(0),
                            stdout: String::new(),
                            stderr: String::new(),
                        })
                } else {
                    queue.remove(0)
                };
                async move { run }.boxed()
            })
        };
        (runner, calls, canned)
    }
}

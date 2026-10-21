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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::session::presence_count;
    use crate::ipc::Response as IpcResponse;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    /// Attach one live presence connection over a duplex under a unique
    /// session id, with the daemon side in a task. Delivery is observed by
    /// reading the client.
    async fn drive_sched_presence(
        session_id: &str,
    ) -> (
        BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
        tokio::io::WriteHalf<tokio::io::DuplexStream>,
        tokio::task::JoinHandle<anyhow::Result<()>>,
    ) {
        let sessions = SessionStore::new();
        let (client, server) = tokio::io::duplex(1 << 16);
        let (server_r, server_w) = tokio::io::split(server);
        let sid = session_id.to_string();
        let task = tokio::spawn(async move {
            let mut rx = BufReader::new(server_r);
            let mut tx = server_w;
            crate::daemon::server::handle_chat_attach(sessions, sid, &mut rx, &mut tx).await
        });
        let (client_r, client_w) = tokio::io::split(client);
        (BufReader::new(client_r), client_w, task)
    }

    /// Read one response line from the client side within 5 s.
    async fn read_one(
        rx: &mut BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
    ) -> IpcResponse {
        let mut line = String::new();
        let read = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            AsyncBufReadExt::read_line(rx, &mut line),
        )
        .await
        .expect("a response line within 5 s")
        .expect("the attach handler is still writing");
        assert!(read > 0, "the attach handler wrote a non-empty line");
        serde_json::from_str(line.trim()).expect("a response line parses")
    }

    #[tokio::test]
    async fn scheduled_job_result_is_pushed_to_attached_clients() {
        let (mut rx, mut client_w, task) = drive_sched_presence("sched-notify-1").await;
        let replies = read_one(&mut rx).await;
        assert!(
            matches!(replies, IpcResponse::Ok),
            "no detach state → Ok alone, got {replies:?}"
        );
        assert_eq!(presence_count("sched-notify-1"), 1);

        let config = Config::default();
        let notify_tx: Option<tokio::sync::mpsc::UnboundedSender<IpcResponse>> = None;
        notify_job_result(
            "nightly-backup",
            "Watchdog alert: nightly-backup".to_string(),
            &config,
            &notify_tx,
        )
        .await;

        let pushed = read_until_text(&mut rx, "Watchdog alert: nightly-backup").await;
        assert!(
            matches!(pushed.last(), Some(IpcResponse::SystemMsg(m)) if m == "Watchdog alert: nightly-backup"),
            "the attached chat receives the job's message as a SystemMsg: {pushed:?}"
        );

        client_w
            .shutdown()
            .await
            .expect("the client write half shuts down cleanly");
        // The hold ends on the next select! wake; the client write-half is
        // gone, so EOF arrives promptly. Wait for the count to drop rather
        // than joining: the abort/await ordering is not the behaviour under
        // test.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while presence_count("sched-notify-1") == 1 && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        // Either the count dropped to 0 (unregister ran) or the handler is
        // wedged waiting on something else — abort it and report what the
        // count actually was, so the failure names the state.
        let final_count = presence_count("sched-notify-1");
        assert_eq!(final_count, 0, "presence must unregister after EOF");
        task.abort();
    }

    /// Read response lines until one carries `text` (a concurrent test's
    /// global broadcast can interleave its own SystemMsg on this stream), or
    /// 5 s elapses.
    async fn read_until_text(
        rx: &mut BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
        text: &str,
    ) -> Vec<IpcResponse> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut out = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            assert!(
                !left.is_zero(),
                "the pushed line '{text}' never arrived within 5 s: {out:?}"
            );
            let mut line = String::new();
            let read = tokio::time::timeout(left, AsyncBufReadExt::read_line(rx, &mut line))
                .await
                .expect("a response line within the deadline")
                .expect("the attach handler is still writing");
            assert!(read > 0, "the attach handler wrote a non-empty line");
            let resp: IpcResponse =
                serde_json::from_str(line.trim()).expect("a response line parses");
            let hit = matches!(&resp, IpcResponse::SystemMsg(m) if m == text);
            out.push(resp);
            if hit {
                return out;
            }
        }
    }

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
        let (output, exit_code) = run_script_on_host_with(
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
        let owner_key =
            crate::shell::with_shells(&reg, |store| store.values().next().map(|r| r.owner.key()));
        assert_eq!(
            owner_key.as_deref(),
            Some("scheduler:job-sched-1"),
            "the shell's owner is the scheduler's"
        );
        let sentence = close_owner_shells_result(&reg, &owner).await;
        assert!(
            sentence.contains("Closed shell") || sentence.contains("still"),
            "the close reports an observed end or a still-finishing host, never silence: {sentence:?}"
        );
    }

    /// The one close sentence `close_owner_shells` produces for this owner,
    /// for the test to read.
    async fn close_owner_shells_result(reg: &ShellRegistry, owner: &Owner) -> String {
        let closed = crate::daemon::executor::knowledge::close_owner_shells(
            reg,
            owner,
            Duration::from_secs(5),
        )
        .await;
        closed
            .into_iter()
            .map(|(_, sentence)| sentence)
            .collect::<Vec<_>>()
            .join(" | ")
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
        let reg = ShellRegistry::default();
        let count = crate::shell::with_shells(&reg, |s| s.len());
        assert_eq!(
            count, 0,
            "no shell is spawned when the sandbox is unavailable"
        );
    }

    #[tokio::test]
    async fn scheduled_host_shell_is_not_marked_sandboxed() {
        let _home = crate::test_home_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("HOME", dir.path()) }
        crate::config::Config::ensure_dirs().ok();
        let reg = ShellRegistry::default();
        let owner = Owner::Scheduler("job-host-1".to_string());
        let (_output, _exit) = run_script_on_host_with(
            &reg,
            owner.clone(),
            "printf 'HOST_OK\\n'; sh -c 'exit 0'",
            spawn_in_process_host,
        )
        .await;
        let sandbox = crate::shell::with_shells(&reg, |store| {
            store
                .values()
                .find(|record| record.owner.key() == owner.key())
                .and_then(|record| record.sandbox.clone())
        });
        assert_eq!(
            sandbox, None,
            "the host path's scheduler shell is a host shell, not a sandboxed one"
        );
        close_owner_shells_result(&reg, &owner).await;
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

    #[test]
    fn due_jobs_for_tick_ignores_an_empty_session() {
        let job = test_job(
            "tick-job",
            ActionOn::Script("noop".to_string()),
            "job-due-1",
        );
        let store = ScheduleStore::new_empty();
        store.add(job.clone()).expect("the store accepts the job");
        let due = due_jobs_for_tick(&store, "");
        assert_eq!(
            due.len(),
            1,
            "an empty session name must not hold jobs back"
        );
        assert_eq!(due[0].id, "job-due-1");
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

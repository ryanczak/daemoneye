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
async fn read_one(rx: &mut BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>) -> IpcResponse {
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
    // A throwaway HOME: the attach path stamps `chat_last_seen`, and the
    // guard serializes every attach-driving test so no other chat is live in
    // the process-global presence map while this one runs.
    let (_home, _dir) = crate::test_home();
    let (mut rx, mut client_w, task) = drive_sched_presence("sched-notify-1").await;
    let replies = read_one(&mut rx).await;
    assert!(
        matches!(replies, IpcResponse::Ok),
        "no detach state → Ok alone, got {replies:?}"
    );
    assert_eq!(presence_count("sched-notify-1"), 1);

    let sessions = SessionStore::new();
    let bus = crate::daemon::notice_sinks::test_bus(sessions, crate::bus::Severity::Warning);
    notify_job_result(
        bus,
        alert_job_notice(
            &test_job("nightly-backup", ActionOn::Alert, "job-notify-1"),
            "Watchdog alert: nightly-backup",
        ),
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
        let resp: IpcResponse = serde_json::from_str(line.trim()).expect("a response line parses");
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
    let state = scheduled_terminal_state(Some(&(JobStatus::Succeeded, None)), "nightly-backup");
    assert_eq!(
        state,
        RunState::Complete,
        "a succeeded job completed its run"
    );
}

#[test]
fn a_script_run_records_a_finished_event() {
    // Pure: the record shape the event log will hold for one run. `Complete`
    // is `succeeded`; any other terminal state is `failed`. A Ghost job
    // injects its own lifecycle events and a deprecated Command job is
    // refused before it runs, so neither emits anything.
    let ok = scheduled_finished_record(
        "tmp-cast-purge",
        &ActionOn::Script("tmp-cast-purge".to_string()),
        &RunState::Complete,
    )
    .expect("a script run emits a record");
    assert_eq!(ok["action"], "script", "the record names the action");
    assert_eq!(
        ok["outcome"], "succeeded",
        "RunState::Complete reads as succeeded"
    );
    assert_eq!(ok["job_name"], "tmp-cast-purge", "the record names the job");

    let hard_fail = scheduled_finished_record(
        "backup",
        &ActionOn::Script("backup".to_string()),
        &RunState::HardFail(Briefing {
            blocker: Some("disk full".to_string()),
            goal: "back up".to_string(),
            attempts: Vec::new(),
            working_files: Vec::new(),
            criteria: None,
            budget_left: None,
        }),
    )
    .expect("a failed script run still emits a record");
    assert_eq!(
        hard_fail["outcome"], "failed",
        "any non-Complete end state reads as failed"
    );

    let alert = scheduled_finished_record("disk-high", &ActionOn::Alert, &RunState::Complete)
        .expect("an alert job run emits a record");
    assert_eq!(
        alert["action"], "alert",
        "an Alert job records action alert"
    );

    assert!(
        scheduled_finished_record(
            "ghost-job",
            &ActionOn::Ghost {
                runbook: "rb".to_string()
            },
            &RunState::Complete,
        )
        .is_none(),
        "a Ghost job injects its own lifecycle events and emits nothing"
    );
    assert!(
        scheduled_finished_record(
            "old-cmd",
            &ActionOn::Command("echo hi".to_string()),
            &RunState::Complete,
        )
        .is_none(),
        "a deprecated Command job is refused and never runs, so emits nothing"
    );
}

#[test]
fn scheduled_state_pending_is_complete_because_every_jobs_reschedule() {
    let state = scheduled_terminal_state(Some(&(JobStatus::Pending, None)), "disk-check");
    assert_eq!(
        state,
        RunState::Complete,
        "Pending must be Complete: an Every job that succeeded is advanced by \
             mark_done straight back to Pending, so reading Pending as anything but \
             Complete would report every successful recurring job as a failure"
    );
}

#[test]
fn a_failed_recurring_run_still_ends_hard_fail_with_its_error() {
    // A recurring job's failed run reads back Pending (mark_done reschedules
    // it), so the failure reaches this mapping through last_error.
    let failed = scheduled_terminal_state(
        Some(&(JobStatus::Pending, Some("boom".to_string()))),
        "disk-check",
    );
    assert_eq!(
        hard_fail_blocker(&failed).as_deref(),
        Some("boom"),
        "a failed recurring run must end hard_fail with its error as the blocker, \
             not as a reported success"
    );

    let succeeded = scheduled_terminal_state(Some(&(JobStatus::Pending, None)), "disk-check");
    assert_eq!(
        succeeded,
        RunState::Complete,
        "a recurring run with no error is still Complete"
    );
}

#[test]
fn scheduled_state_failed_carries_the_error_as_the_blocker() {
    let state = scheduled_terminal_state(
        Some(&(JobStatus::Failed("exit code 1".to_string()), None)),
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
    let state = scheduled_terminal_state(Some(&(JobStatus::Cancelled, None)), "cleanup-tmp");
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
    let state = scheduled_terminal_state(Some(&(JobStatus::Running, None)), "still-running");
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
    let failed =
        scheduled_terminal_state(Some(&(JobStatus::Failed("boom".to_string()), None)), "j");
    let missing = scheduled_terminal_state(None, "j");
    assert_ne!(
        hard_fail_blocker(&failed),
        hard_fail_blocker(&missing),
        "the two HardFail paths name different causes and must stay distinguishable"
    );
}

#[test]
fn a_refused_watchdog_ends_the_scheduled_run_budget_exceeded() {
    // The refusal the watchdog's cost gate returns, mapped by the same
    // pure function `run_scheduled_job` calls: it replaces whatever
    // state the job status alone would give, with the message as the
    // blocker and the `Usd` dimension on the briefing.
    let message = crate::daemon::spend::refusal_message(3.0, 2.0);
    let state = scheduled_run_state(
        Some(message.clone()),
        Some(&(JobStatus::Succeeded, None)),
        "nightly-backup",
    );
    match state {
        crate::daemon::run::RunState::BudgetExceeded(briefing) => {
            assert_eq!(
                briefing.budget_left,
                Some(crate::daemon::run::BudgetDimension::Usd),
                "the cost cap ends the scheduled run on the Usd dimension"
            );
            assert_eq!(
                briefing.blocker.as_deref(),
                Some(message.as_str()),
                "the blocker must be the gate's refusal message verbatim"
            );
            assert_eq!(
                briefing.goal, "scheduled job nightly-backup",
                "the briefing's goal must name the job"
            );
        }
        other => panic!("a refused watchdog must end BudgetExceeded, got {other:?}"),
    }
    // Without the refusal, the status alone decides: a succeeded job is
    // Complete, so the refusal replaces this state rather than changing
    // how the job was marked.
    let unrefused =
        scheduled_run_state(None, Some(&(JobStatus::Succeeded, None)), "nightly-backup");
    assert_eq!(
        unrefused,
        scheduled_terminal_state(Some(&(JobStatus::Succeeded, None)), "nightly-backup"),
        "no refusal means the state `scheduled_terminal_state` would have given"
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
    let closed =
        crate::daemon::executor::knowledge::close_owner_shells(reg, owner, Duration::from_secs(5))
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
    let backend =
        crate::shell::PtyBackend::spawn("bash", &[], 24, 80, 100, &cast, 0).expect("spawning bash");
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
        last_error: None,
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
    let reason = format!("refused: {COMMAND_REFUSAL_REASON}");
    crate::daemon::utils::log_event(
        "scheduled_job_refused",
        serde_json::json!({
            "job_id": job.id,
            "job_name": job.name,
            "reason": COMMAND_REFUSAL_REASON,
        }),
    );
    let bus = crate::bus::Bus::with_journal(|_| {});
    let leaked: &'static crate::bus::Bus = Box::leak(Box::new(bus));
    fail_job_terminal(job, store, leaked, reason.clone()).await;
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

// ── M38 phase 04: the job notices the bus producers publish ───────────────

/// A recording journal's sink, so a test can read the notices a producer
/// published without registering any consumer. Leaked to `&'static Bus`,
/// the shape `run_scheduled_job` and every helper take.
fn recording_bus() -> (
    &'static crate::bus::Bus,
    std::sync::Arc<std::sync::Mutex<Vec<crate::bus::Notice>>>,
) {
    let journal: std::sync::Arc<std::sync::Mutex<Vec<crate::bus::Notice>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = std::sync::Arc::clone(&journal);
    let bus: &'static crate::bus::Bus =
        Box::leak(Box::new(crate::bus::Bus::with_journal(move |n| {
            sink.lock().unwrap().push(n.clone())
        })));
    (bus, journal)
}

#[test]
fn alert_job_notice_is_a_scheduler_warning() {
    let job = test_job("disk-high", ActionOn::Alert, "job-alert-1");
    let n = alert_job_notice(&job, "Watchdog alert: disk-high");
    assert_eq!(n.source, crate::bus::Source::Scheduler);
    assert_eq!(n.severity, crate::bus::Severity::Warning);
    assert_eq!(n.name, "disk-high");
    assert_eq!(n.source_id, "job-alert-1");
    assert_eq!(n.labels.get("kind").map(String::as_str), Some("alert"));
    assert_eq!(n.title, "Watchdog alert: disk-high");
    assert_eq!(n.body, "", "a one-line alert message has no body");
}

#[test]
fn script_job_notice_severity_follows_the_exit_code() {
    let job = test_job(
        "nightly-backup",
        ActionOn::Script("backup.sh".into()),
        "job-scr-1",
    );
    let msg = "`backup.sh` failed (exit 1)\n<output>\nboom\n</output>";
    let failed = script_job_notice(&job, "backup.sh", msg, 1);
    assert_eq!(
        failed.severity,
        crate::bus::Severity::Error,
        "a non-zero exit is an Error"
    );
    assert_eq!(failed.title, "`backup.sh` failed (exit 1)");
    assert_eq!(failed.body, "<output>\nboom\n</output>");
    assert_eq!(failed.source, crate::bus::Source::Scheduler);
    assert_eq!(failed.source_id, "job-scr-1");
    assert_eq!(failed.name, "nightly-backup");
    assert_eq!(
        failed.labels.get("kind").map(String::as_str),
        Some("script")
    );

    let ok = script_job_notice(
        &job,
        "backup.sh",
        "`backup.sh` succeeded (exit 0)\n<output>\n\n</output>",
        0,
    );
    assert_eq!(ok.severity, crate::bus::Severity::Info, "exit 0 is Info");
    assert_eq!(ok.title, "`backup.sh` succeeded (exit 0)");
}

#[test]
fn failure_notice_is_a_scheduler_error() {
    let job = test_job(
        "nightly-backup",
        ActionOn::Script("backup.sh".into()),
        "job-fail-1",
    );
    let n = failure_notice(&job, "refused: the command action moved to scripts");
    assert_eq!(n.source, crate::bus::Source::Scheduler);
    assert_eq!(n.severity, crate::bus::Severity::Error);
    assert_eq!(n.name, "nightly-backup");
    assert_eq!(n.source_id, "job-fail-1");
    assert_eq!(n.labels.get("kind").map(String::as_str), Some("failure"));
    assert_eq!(n.title, "refused: the command action moved to scripts");
}

#[test]
fn watchdog_notice_is_a_watchdog_warning() {
    let job = test_job(
        "disk-check",
        ActionOn::Script("check.sh".into()),
        "job-wd-1",
    );
    let n = watchdog_notice(&job, "Disk at 92%\nGHOST_TRIGGER: NO");
    assert_eq!(n.source, crate::bus::Source::Watchdog);
    assert_eq!(n.severity, crate::bus::Severity::Warning);
    assert_eq!(n.name, "disk-check");
    assert_eq!(n.source_id, "job-wd-1");
    assert_eq!(n.labels.get("kind").map(String::as_str), Some("watchdog"));
    assert_eq!(n.title, "[Watchdog] disk-check: Disk at 92%");
    assert_eq!(n.body, "GHOST_TRIGGER: NO");
}

#[tokio::test]
async fn fail_job_publishes_one_error_notice() {
    let job = test_job(
        "nightly-backup",
        ActionOn::Script("backup.sh".into()),
        "job-fj-1",
    );
    let store = test_store(&job);
    let (bus, journal) = recording_bus();
    fail_job(&job, &store, bus, "boom: exit 7".to_string()).await;

    let notices = journal.lock().unwrap();
    assert_eq!(
        notices.len(),
        1,
        "a failed job publishes exactly one notice: {notices:?}"
    );
    let n = &notices[0];
    assert_eq!(n.source, crate::bus::Source::Scheduler);
    assert_eq!(n.severity, crate::bus::Severity::Error);
    assert_eq!(n.name, "nightly-backup");
    assert_eq!(n.source_id, "job-fj-1");
    assert_eq!(n.labels.get("kind").map(String::as_str), Some("failure"));
    drop(notices);

    let status = store
        .list()
        .into_iter()
        .find(|j| j.id == "job-fj-1")
        .map(|j| j.status);
    assert!(
        matches!(
            status,
            Some(JobStatus::Failed(_)) | Some(JobStatus::Pending)
        ),
        "the failed run is recorded: {status:?}"
    );
    let last_error = store
        .list()
        .into_iter()
        .find(|j| j.id == "job-fj-1")
        .and_then(|j| j.last_error);
    assert_eq!(
        last_error.as_deref(),
        Some("boom: exit 7"),
        "the job's failure reason is recorded"
    );
}

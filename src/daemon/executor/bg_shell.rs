//! A chat turn's `run_terminal_command(wait: false)` on the PTY shell
//! substrate (M24 phase 05b): start the command on a **dedicated** shell and
//! return immediately, then deliver its real exit code later as a
//! `[Background Task Completed]` message injected into the session.
//!
//! The tmux background path (`foreground::run_background` → `de-bg-*`), for
//! a ghost session or `backend = "tmux"` (the opt-out), keeps using it. This module is the
//! `backend = "pty"` branch's engine — the default — and, like `shell_exec`,
//! takes no `tx`/`rx`, so every part of it is testable against a real
//! in-process PTY host.

use anyhow::Context;
use std::time::Duration;

use super::shell_exec;
use crate::ai::Message;
use crate::daemon::run::{
    RunId, RunKind, RunState, TurnLogEvent, TurnLogRecord, append_turn_event,
};
use crate::daemon::session::{SessionStore, append_session_message, with_sessions};
use crate::daemon::utils::log_shell_command;
use crate::shell::{
    CommandHandle, CommandOutcome, Owner, ShellId, ShellRegistry, login_shell, spawn_command,
    spawn_dedicated_for, with_shells,
};

/// How long a started background command may run before its reader task gives
/// up and reports it incomplete. Deliberately **not** the 45-second
/// foreground timeout: a background command is *expected* to outlive the turn
/// that started it — a build, a test suite, a data copy. The ceiling exists
/// only so a wedged command cannot leak its reader task forever; the outcome
/// it would have carried is reported as "did not complete" with the
/// may-still-be-running warning.
pub(super) const BACKGROUND_CMD_TIMEOUT: Duration = Duration::from_secs(3600);

/// The ids a started background run hands back to the model, plus what
/// `await_and_notify` needs to index the outcome: the run's dedicated
/// shell's registry and the cast length the snapshot was taken at.
#[derive(Debug, Clone)]
pub(super) struct BackgroundStart {
    pub run_id: RunId,
    pub shell_id: ShellId,
    pub reg: ShellRegistry,
    pub before: Option<u64>,
}

/// Start `cmd` on a **dedicated** shell for `owner` and return without
/// awaiting it. Registers the run and writes `RunStarted` to its turn log.
///
/// The spawn step is injected: `spawn_detached_host` re-invokes
/// `current_exe()` and cannot run under a test binary (07b F4).
///
/// An interactive command (`ssh`, `mosh`, …) is refused here, not started:
/// a background ssh/mosh session can never be typed into, so starting it
/// would strand an open session with no input path. Run it in the foreground
/// instead.
pub(super) async fn start_background_with<F, Fut>(
    reg: &ShellRegistry,
    owner: Owner,
    cmd: &str,
    max_per_owner: u32,
    spawn: F,
) -> anyhow::Result<(BackgroundStart, CommandHandle)>
where
    F: FnOnce(ShellId) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    if crate::daemon::utils::is_interactive_command(cmd) {
        anyhow::bail!(
            "`{cmd}` is an interactive command and cannot run with `wait: false` — \
             a background ssh/mosh session can never be typed into. Run it \
             without `wait: false` instead."
        );
    }
    let run_id = RunId::new();
    // A cap error propagates unchanged: a background command that would
    // exceed `[shells] max_per_owner` must fail loudly, not silently borrow
    // the owner's foreground shell.
    let (shell_id, socket) = spawn_dedicated_for(
        reg,
        owner.clone(),
        max_per_owner,
        shell_exec::SHELL_READY_TIMEOUT,
        spawn,
    )
    .await?;
    finish_start(reg, owner, run_id, shell_id, socket, cmd).await
}

/// Start `cmd` on a dedicated shell in the chat session's own sbx sandbox,
/// and return without awaiting it. The sandbox is created behind a
/// fail-closed preflight (a failure is an `Err` and the command never runs on
/// the host), a staged-script invocation is re-staged from the operator's
/// copy on every call, and the shell's child runs `sbx exec -it <sandbox>`
/// so the command executes inside the VM.
///
/// An interactive command (`ssh`, `mosh`, …) is refused here for the same
/// reason [`start_background_with`] refuses it, before any spawn: the shell
/// and the sandbox are never created for a command that cannot run.
pub(super) async fn start_background_sandboxed(
    reg: &ShellRegistry,
    owner: Owner,
    session: &str,
    cmd: &str,
    max_per_owner: u32,
) -> anyhow::Result<(BackgroundStart, CommandHandle)> {
    let cfg = crate::config::Config::load().unwrap_or_default().sandbox;
    let program = crate::shell::which_sbx().unwrap_or_else(|_| "sbx".to_string());
    let run = crate::shell::real_runner(program);
    let sandbox = super::chat_sandbox::ensure_chat_sandbox(
        &run,
        &cfg,
        session,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    )
    .await
    .map_err(anyhow::Error::msg)?;
    let cmd_to_run = match crate::daemon::executor::sbx::script_invocation(cmd, |n| {
        crate::scripts::resolve_script(n).is_ok()
    }) {
        Some((name, args_tail)) => {
            let host_path = crate::config::scripts_dir().join(&name);
            super::ghost_sandbox::stage_script(&run, &sandbox, &name, &host_path.to_string_lossy())
                .await
                .map(|_| crate::daemon::executor::sbx::staged_script_command(&name, &args_tail))
                .map_err(anyhow::Error::msg)?
        }
        None => cmd.to_string(),
    };
    start_background_in_sandbox_with(
        reg,
        owner,
        &cmd_to_run,
        max_per_owner,
        &sandbox,
        |shell_id| {
            let (rows, cols) = crate::shell::spawn_size();
            let s = sandbox.clone();
            async move { crate::shell::spawn_sbx_host(shell_id, &s, rows, cols).await }
        },
    )
    .await
}

/// Start `cmd` on a **dedicated** shell inside the sbx sandbox `sandbox`, and
/// return without awaiting it. The steps of [`start_background_with`] with
/// `spawn_for_in_sandbox` in place of `spawn_for` and the same
/// spawn-then-ready-with-rollback shape: a record for a host that never came
/// up is removed before the error is returned.
pub(super) async fn start_background_in_sandbox_with<F, Fut>(
    reg: &ShellRegistry,
    owner: Owner,
    cmd: &str,
    max_per_owner: u32,
    sandbox: &str,
    spawn: F,
) -> anyhow::Result<(BackgroundStart, CommandHandle)>
where
    F: FnOnce(ShellId) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    if crate::daemon::utils::is_interactive_command(cmd) {
        anyhow::bail!(
            "`{cmd}` is an interactive command and cannot run with `wait: false` — \
             a background ssh/mosh session can never be typed into. Run it \
             without `wait: false` instead."
        );
    }
    let run_id = RunId::new();
    let id = crate::shell::spawn_for_in_sandbox(
        reg,
        owner.clone(),
        max_per_owner,
        sandbox.to_string(),
        true,
        spawn,
    )
    .await?;
    let socket = crate::shell::socket_path(id.as_str());
    if let Err(e) = crate::shell::await_ready(
        &socket,
        std::time::Instant::now() + shell_exec::SHELL_READY_TIMEOUT,
    )
    .await
    {
        with_shells(reg, |store| {
            store.remove(id.as_str());
        });
        return Err(e).with_context(|| {
            format!(
                "shell {} never became ready at {} within {:?}",
                id.as_str(),
                socket.display(),
                shell_exec::SHELL_READY_TIMEOUT
            )
        });
    }
    finish_start(reg, owner, run_id, id, socket, cmd).await
}

/// The tail both starts share: the `RunStarted` record, the marker name the
/// record dictates, and the command's dispatch. Takes the marker from
/// [`crate::shell::shell_marker_name`] so a sandbox record's marker is POSIX
/// `$?` while a host record's is today's value.
async fn finish_start(
    reg: &ShellRegistry,
    owner: Owner,
    run_id: RunId,
    shell_id: ShellId,
    socket: std::path::PathBuf,
    cmd: &str,
) -> anyhow::Result<(BackgroundStart, CommandHandle)> {
    append_turn_event(&TurnLogRecord {
        timestamp: chrono::Utc::now().to_rfc3339(),
        run_id: run_id.clone(),
        event: TurnLogEvent::RunStarted {
            kind: RunKind::Chat {
                session: owner.key(),
            },
        },
    });
    let marker_name = with_shells(reg, |store| {
        store
            .get(shell_id.as_str())
            .map(crate::shell::shell_marker_name)
    })
    .unwrap_or_else(login_shell);
    // The cast length before the command runs — the producer window's left
    // edge, the same snapshot the foreground path takes. Read before
    // `spawn_command`: once the completed command is in hand the pump has
    // usually flushed the whole command into the cast.
    let started_unix = with_shells(reg, |store| {
        store
            .get(shell_id.as_str())
            .map(|record| record.started_unix)
            .unwrap_or(0)
    });
    let before = std::fs::metadata(crate::shell::resolve_cast(shell_id.as_str(), started_unix))
        .map(|m| m.len())
        .ok();
    let handle = spawn_command(&socket, cmd, &marker_name, BACKGROUND_CMD_TIMEOUT).await?;
    Ok((
        BackgroundStart {
            run_id,
            shell_id,
            reg: reg.clone(),
            before,
        },
        handle,
    ))
}

/// The production form: [`start_background_with`] with the real spawn. The
/// PTY geometry is `[shells] rows`/`cols` via [`crate::shell::spawn_size`],
/// the same geometry `shell_exec`'s host spawn reads.
pub(super) async fn start_background(
    reg: &ShellRegistry,
    owner: Owner,
    cmd: &str,
    max_per_owner: u32,
) -> anyhow::Result<(BackgroundStart, CommandHandle)> {
    start_background_with(reg, owner, cmd, max_per_owner, |id| {
        let (rows, cols) = crate::shell::spawn_size();
        crate::shell::spawn_detached_host(id, &[], rows, cols)
    })
    .await
}

/// The completion message of a run that passed its timeout and then exited
/// on the Ctrl-C the timeout typed: it has a real exit code, so nothing is
/// still running and the output is shown, as for a finished run.
fn interrupted_message(
    cmd: &str,
    run_id: &RunId,
    shell_id: &ShellId,
    outcome: &CommandOutcome,
) -> String {
    format!(
        "Background command `{cmd}` (run {run_id}, shell {shell_id}) passed its \
         {BACKGROUND_CMD_TIMEOUT:?} timeout and was interrupted with Ctrl-C; it \
         exited with code {}. read_shell(shell: \"{shell_id}\") reads its \
         recorded output.\n{}",
        outcome.exit_code,
        output_block(outcome)
    )
}

/// A finished command's output as the model sees it. Same annotate → mask →
/// normalize order `format_for_model` applies (F3): the annotation strips
/// control bytes before masking, or an escape embedded mid-token could split
/// a secret past its regex.
fn output_block(o: &CommandOutcome) -> String {
    let text = shell_exec::format_for_model(&String::from_utf8_lossy(&o.output));
    if text.is_empty() {
        "(no output)".to_string()
    } else {
        format!("<output>\n{text}\n</output>")
    }
}

/// The `[Background Task Completed]` body for a finished background run.
/// Pure: no locks, no IO, no clock.
///
/// The caller prefixes `[Background Task Completed]\n` — that literal is what
/// `build_catchup_brief` scans for and must not drift.
pub(super) fn completion_message(
    cmd: &str,
    run_id: &RunId,
    shell_id: &ShellId,
    outcome: &Result<CommandOutcome, String>,
) -> String {
    let head = match outcome {
        Ok(o) => format!(
            "Background command `{cmd}` (run {run_id}, shell {shell_id}) finished with exit code {}.",
            o.exit_code
        ),
        Err(e) => format!(
            "Background command `{cmd}` (run {run_id}, shell {shell_id}) did not complete: {e}."
        ),
    };
    let body = match outcome {
        Ok(o) => output_block(o),
        Err(_) => format!(
            "The command may still be running in shell {shell_id} — do not assume it \
             failed outright; retrying could double-apply its effect. Its output so \
             far, if any, can be read from the shell's log."
        ),
    };
    format!("{head}\n{body}")
}

/// Await the run's outcome, write `RunFinished` to its turn log, and inject
/// the completion into its session. Runs detached; never returns to the turn.
///
/// `RunFinished` is `Complete` whenever the handle delivered an outcome —
/// whatever the exit code, a command that ran and exited 1 *completed* — and
/// `HardFail` when the handle resolved to `Err`.
///
/// `with_sessions` is not reentrant and must never be held across `.await`
/// and never nested: the lock discipline below is `notify_session`'s four
/// phases, minus the tmux `bg_windows` bookkeeping there is no window for. A
/// session entry that has disappeared before the await is a silent return,
/// exactly as `notify_session` does.
pub(super) async fn await_and_notify(
    sessions: SessionStore,
    session_id: String,
    cmd: String,
    start: BackgroundStart,
    handle: CommandHandle,
) {
    let BackgroundStart {
        run_id,
        shell_id,
        reg,
        before,
    } = start;
    // An interrupted run — past its timeout, then exited on the Ctrl-C — has
    // a real outcome. It is indexed and gets its own completion message, but
    // it did not finish on its own, so it still reports as a failure.
    let mut interrupted: Option<CommandOutcome> = None;
    let outcome: Result<CommandOutcome, String> = match handle.outcome.await {
        Ok(Ok(inner)) => {
            shell_exec::index_command_output(&reg, &cmd, &shell_id, &inner, before).await;
            Ok(inner)
        }
        Ok(Err(e)) => match e.downcast::<crate::shell::CommandTimedOut>() {
            Ok(timed_out) => {
                let text = timed_out.to_string();
                if let Some(inner) = timed_out.interrupted {
                    shell_exec::index_command_output(&reg, &cmd, &shell_id, &inner, before).await;
                    interrupted = Some(inner);
                }
                Err(text)
            }
            Err(e) => Err(format!("{e:#}")),
        },
        Err(_) => Err("the command's reader task ended without delivering an outcome".to_string()),
    };
    let state = report_state(&outcome);
    record_command_completion(&session_id, &cmd, &shell_id, &outcome);

    // Phase 1 (locked): the session entry disappearing before the await is a
    // silent return, exactly as `notify_session` does.
    let existed = with_sessions(&sessions, |store| store.contains_key(&session_id));
    if !existed {
        append_turn_event(&TurnLogRecord {
            timestamp: chrono::Utc::now().to_rfc3339(),
            run_id,
            event: TurnLogEvent::RunFinished { state },
        });
        return;
    }

    // Phase 2 (unlocked): pure formatting and the file append.
    let completion_msg = Message {
        role: "user".to_string(),
        content: format!(
            "[Background Task Completed]\n{}",
            match &interrupted {
                Some(o) => interrupted_message(&cmd, &run_id, &shell_id, o),
                None => completion_message(&cmd, &run_id, &shell_id, &outcome),
            }
        ),
        tool_calls: None,
        tool_results: None,
        turn: None,
    };
    append_session_message(&session_id, &completion_msg);

    // Phase 3 (locked): push the message into the in-memory history.
    with_sessions(&sessions, |store| {
        if let Some(entry) = store.get_mut(&session_id) {
            entry.push_event(completion_msg);
        }
    });

    // Phase 4 (unlocked): the run's own record.
    append_turn_event(&TurnLogRecord {
        timestamp: chrono::Utc::now().to_rfc3339(),
        run_id,
        event: TurnLogEvent::RunFinished { state },
    });
}

/// The `command` record of a finished background run: where the exit code is
/// first known. The launch record (`dispatch_background.rs`) carries no exit
/// code — it says the command was approved — and this record says what
/// actually happened. A run whose outcome is `Err` did not complete, so its
/// status is `failed` and no exit code is reported.
fn record_command_completion(
    session_id: &str,
    cmd: &str,
    shell_id: &ShellId,
    outcome: &Result<CommandOutcome, String>,
) {
    let (status, exit) = match outcome {
        Ok(o) => ("completed", Some(o.exit_code)),
        Err(_) => ("failed", None),
    };
    log_shell_command(
        Some(session_id),
        shell_id.as_str(),
        false,
        cmd,
        status,
        exit,
        "",
    );
}

/// [`await_and_notify`] to completion, then `close(shell_id)`: a sandboxed
/// background shell is single-use, and the command's completion message has
/// landed by the time this runs, so the shell is done for good. The close
/// comes **after** the message lands — closing first would hang the command
/// up before its reader has drained it.
pub(super) async fn await_notify_and_close<F, Fut>(
    sessions: SessionStore,
    session_id: String,
    cmd: String,
    start: BackgroundStart,
    handle: CommandHandle,
    close: F,
) where
    F: FnOnce(ShellId) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let shell_id = start.shell_id.clone();
    await_and_notify(sessions, session_id, cmd, start, handle).await;
    close(shell_id).await;
}

/// The `RunFinished` state a resolved (or dropped) handle reports.
fn report_state(outcome: &Result<CommandOutcome, String>) -> RunState {
    match outcome {
        Ok(_) => RunState::Complete,
        Err(e) => RunState::HardFail(crate::daemon::run::Briefing::new(
            format!("background command on the shell substrate did not complete: {e}"),
            None,
            Vec::new(),
            Vec::new(),
            Some(e.clone()),
            None,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::session::SessionEntry;
    use crate::shell::{Nonce, PtyBackend, bind, lock_path, serve, socket_path};
    use anyhow::Context;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    const ROWS: u16 = 24;
    const COLS: u16 = 80;

    /// The last line of today's event segment: the record the code under
    /// test just wrote. The `command` events land in the dated segment,
    /// which `log_event` creates on first write — so a missing segment
    /// means the record was never written.
    fn last_written_event_record() -> serde_json::Value {
        let seg = crate::config::current_event_segment_path();
        let content = std::fs::read_to_string(&seg)
            .expect("today's event segment exists — the record was never written");
        let last = content.lines().last().expect("at least one line");
        serde_json::from_str(last).expect("the last line is valid JSON")
    }

    /// A spawn substitute whose cast the producer can resolve through the
    /// ordinary path — `cast_path` under the test `HOME`'s `shell_logs_dir`,
    /// the shape `resolve_cast` + `meta_path_for` operate on in production,
    /// as `spawn_in_logs_dir` in `shell_exec_tests.rs` does.
    async fn spawn_in_logs_dir(id: ShellId) -> anyhow::Result<()> {
        let started = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let cast = crate::shell::cast_path(id.as_str(), started);
        std::fs::create_dir_all(cast.parent().context("cast has a parent")?)
            .context("creating the logs dir")?;
        let backend = PtyBackend::spawn("bash", &[], ROWS, COLS, 100, &cast, started)
            .context("spawning bash into the logs dir")?;
        let lock = crate::daemon::instance::InstanceLock::acquire(&lock_path(id.as_str()))
            .context("holding the per-id instance lock")?;
        let socket = socket_path(id.as_str());
        let listener = bind(&socket)
            .await
            .context("binding the in-process host's socket")?;
        tokio::spawn(serve(listener, Arc::clone(&backend)));
        std::mem::forget(lock);
        std::mem::forget(cast);
        Ok(())
    }

    /// The production spawn substitute: a real in-process PTY host serving
    /// the configured socket for `id`, a bash session whose `.cast` the
    /// returned path points at. `spawn_detached_host` cannot run under a test
    /// (07b F4).
    async fn spawn_with_cast(id: ShellId) -> anyhow::Result<PathBuf> {
        let dir = tempfile::tempdir().context("tempdir for cast")?;
        let cast = dir.path().join(format!("{}.cast", id.as_str()));
        let backend =
            PtyBackend::spawn("bash", &[], ROWS, COLS, 100, &cast, 0).context("spawning bash")?;
        let lock = crate::daemon::instance::InstanceLock::acquire(&lock_path(id.as_str()))
            .context("holding the per-id instance lock")?;
        let socket = socket_path(id.as_str());
        let listener = bind(&socket)
            .await
            .context("binding the in-process host's socket")?;
        tokio::spawn(serve(listener, Arc::clone(&backend)));
        std::mem::forget(lock);
        std::mem::forget(dir);
        Ok(cast)
    }

    /// The spawn step `start_background_with` takes in tests, paired with the
    /// host that serves the id's socket.
    async fn spawn_in_process_host(id: ShellId) -> anyhow::Result<()> {
        let cast = spawn_with_cast(id.clone()).await?;
        std::mem::forget(cast);
        Ok(())
    }

    /// A fresh `SessionStore` holding one live entry for `session_id`, built
    /// with every field the struct requires (there is no `Default`).
    fn store_with_entry(session_id: &str) -> SessionStore {
        let sessions = SessionStore::new();
        let entry = SessionEntry {
            messages: Vec::new(),
            last_accessed: std::time::Instant::now(),
            chat_pane: None,
            default_target_pane: None,
            bg_windows: Vec::new(),
            last_prompt_tokens: 0,
            tmux_session: "test".to_string(),
            last_detach: None,
            detach_time_utc: None,
            messages_at_detach: 0,
            is_ghost: false,
            ghost_config: None,
            ghost_runbook_name: None,
            ghost_bg_prefix: "",
            started_at: chrono::Utc::now(),
            turn_count: 0,
            tool_calls_this_session: 0,
            active_model: None,
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
            turn_inbox: None,
        };
        with_sessions(&sessions, |store| {
            store.insert(session_id.to_string(), entry);
        });
        sessions
    }

    // ── completion_message — pure, no host needed ────────────────────────────

    #[test]
    fn bg_completion_names_the_run_the_shell_and_the_exit_code() {
        let run_id = RunId::new();
        let shell_id = ShellId::from_index(3);
        let outcome = crate::shell::CommandOutcome {
            output: b"BODY\n".to_vec(),
            exit_code: 7,
            nonce: Nonce::new(),
        };
        let msg = completion_message("cargo build", &run_id, &shell_id, &Ok(outcome));
        let first = msg.lines().next().expect("at least one line");
        assert_eq!(
            first,
            &format!(
                "Background command `cargo build` (run {run_id}, shell {shell_id}) finished with exit code 7."
            ),
            "the first line must be the exact succeeded sentence"
        );
        assert!(
            !msg.contains("did not complete"),
            "a completed run's message must not carry the failure sentence: {msg:?}"
        );
        assert!(
            first.contains(&format!("run {run_id}")),
            "the run id must be in the first line: {first:?}"
        );
        assert!(
            first.contains(&format!("shell {shell_id}")),
            "the shell id must be in the first line: {first:?}"
        );
    }

    #[test]
    fn bg_completion_wraps_the_output_in_output_tags() {
        let run_id = RunId::new();
        let shell_id = ShellId::from_index(1);
        let outcome = crate::shell::CommandOutcome {
            output: b"BUILD SUCCEEDED\n".to_vec(),
            exit_code: 0,
            nonce: Nonce::new(),
        };
        let msg = completion_message("make", &run_id, &shell_id, &Ok(outcome));
        let start = msg
            .find("<output>")
            .expect("the output must be inside <output> tags");
        let end = msg
            .find("</output>")
            .expect("the closing tag must be present");
        assert!(start < end, "tags must wrap the output: {msg:?}");
        let inner = &msg[start + "<output>".len()..end];
        assert!(
            inner.contains("BUILD SUCCEEDED"),
            "the command's bytes must appear between the tags: {inner:?}"
        );
    }

    #[test]
    fn bg_completion_masks_a_secret_in_the_output() {
        let run_id = RunId::new();
        let shell_id = ShellId::from_index(2);
        let outcome = crate::shell::CommandOutcome {
            output: b"key=AKIAIOSFODNN7EXAMPLE\n".to_vec(),
            exit_code: 0,
            nonce: Nonce::new(),
        };
        let msg = completion_message("env", &run_id, &shell_id, &Ok(outcome));
        assert!(
            !msg.contains("AKIAIOSFODNN7EXAMPLE"),
            "a secret in the output must not reach the model: {msg:?}"
        );
        assert!(msg.contains("<AWS_KEY>"), "the mask marker must: {msg:?}");
    }

    #[test]
    fn bg_completion_on_failure_warns_the_command_may_still_run() {
        let run_id = RunId::new();
        let shell_id = ShellId::from_index(4);
        let msg = completion_message(
            "cargo test",
            &run_id,
            &shell_id,
            &Err("command timed out".to_string()),
        );
        let first = msg.lines().next().expect("at least one line");
        assert!(
            first.starts_with(&format!(
                "Background command `cargo test` (run {run_id}, shell {shell_id}) did not complete:"
            )),
            "the failure first line must say the command did not complete: {first:?}"
        );
        assert!(
            !first.contains("finished with exit code"),
            "a failure must not claim an exit code: {first:?}"
        );
        assert!(
            msg.contains("may still be running"),
            "the failure must carry the may-still-be-running warning: {msg:?}"
        );
    }

    // ── start_background_with — real PTY host ────────────────────────────────

    #[tokio::test]
    async fn bg_start_returns_before_the_command_finishes() {
        let (_home, _dir) = crate::test_home();
        let reg = ShellRegistry::default();
        let (start, mut handle) = start_background_with(
            &reg,
            Owner::Chat("bg-returns".into()),
            "sleep 1; printf 'LATE\\n'",
            5,
            spawn_in_process_host,
        )
        .await
        .expect("starting a normal command succeeds");
        let pending = handle
            .outcome
            .try_recv()
            .expect_err("a still-running command must not have resolved yet");
        assert!(
            matches!(pending, tokio::sync::oneshot::error::TryRecvError::Empty),
            "the call must return while the command still runs: {pending:?}"
        );

        let outcome = handle
            .outcome
            .await
            .expect("the reader task stays alive to deliver")
            .expect("the command finishes inside the timeout");
        let text = String::from_utf8_lossy(&outcome.output);
        assert!(
            text.contains("LATE"),
            "the late bytes must still arrive after the start returned: {text:?}"
        );
        assert!(
            start.shell_id.as_str().starts_with('s'),
            "a real shell id was allocated: {:?}",
            start.shell_id
        );
    }

    #[tokio::test]
    async fn bg_start_uses_a_dedicated_shell_not_the_owners_existing_one() {
        let (_home, _dir) = crate::test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("bg-dedicated".into());
        // Give the owner a live reusable shell first, exactly as the
        // foreground path would have. (This is *setup* — the whole point of
        // the test is that the engine does NOT reuse it.)
        let (foreground_id, _) = crate::shell::shell_for_owner(
            &reg,
            owner.clone(),
            crate::host::HostId::Local,
            5,
            shell_exec::SHELL_READY_TIMEOUT,
            spawn_in_process_host,
        )
        .await
        .expect("the owner's first shell comes up");

        let (start, _handle) = start_background_with(
            &reg,
            owner,
            "echo BACKGROUND_ONLY",
            5,
            spawn_in_process_host,
        )
        .await
        .expect("playing both roles is what a background command is");

        assert_ne!(
            foreground_id.as_str(),
            start.shell_id.as_str(),
            "the background command must get its own shell, not the owner's"
        );
        let guard = reg.try_lock().expect("registry test peek");
        let record = guard
            .get(start.shell_id.as_str())
            .expect("the background shell's record is registered");
        assert!(
            record.dedicated,
            "the fresh record must be marked dedicated"
        );
    }

    #[tokio::test]
    async fn bg_start_refuses_an_interactive_command() {
        let (_home, _dir) = crate::test_home();
        let reg = ShellRegistry::default();
        let result = start_background_with(
            &reg,
            Owner::Chat("bg-interactive".into()),
            "ssh nowhere.invalid",
            5,
            spawn_in_process_host,
        )
        .await;
        let err = result.expect_err("an interactive command must be refused, not started");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("without `wait: false`"),
            "the refusal must name the wait-free alternative: {msg}"
        );
        let guard = reg.try_lock().expect("registry test peek");
        assert!(
            guard.is_empty(),
            "no shell is allocated for a refused command"
        );
    }

    // ── await_and_notify — real PTY host + a real SessionStore ───────────────

    #[tokio::test]
    async fn bg_notify_pushes_a_completion_message_carrying_the_real_exit_code() {
        let (_home, _dir) = crate::test_home();
        let reg = ShellRegistry::default();
        let session_id = "bg-notify-session".to_string();
        let sessions = store_with_entry(&session_id);
        let (start, handle) = start_background_with(
            &reg,
            Owner::Chat(session_id.clone()),
            "printf 'DONE\\n'; sh -c 'exit 4'",
            5,
            spawn_in_process_host,
        )
        .await
        .expect("the command starts");

        await_and_notify(
            sessions.clone(),
            session_id.clone(),
            "printf 'DONE\\n'; sh -c 'exit 4'".to_string(),
            start,
            handle,
        )
        .await;

        let (role, content) = with_sessions(&sessions, |store| {
            let entry = store.get(&session_id).expect("the session entry is back");
            let last = entry.messages.last().expect("a completion was pushed");
            (last.role.clone(), last.content.clone())
        });
        assert_eq!(role, "user", "the completion is injected as the user role");
        assert!(
            content.starts_with("[Background Task Completed]"),
            "the prefixed literal leads: {content:?}"
        );
        assert!(
            content.contains("exit code 4"),
            "the real exit code from the marker must reach the session: {content:?}"
        );
        assert!(
            content.contains("DONE"),
            "the command's own output must reach the session: {content:?}"
        );
    }

    /// bug-16-1: a background run that passed its timeout and answered the
    /// Ctrl-C by exiting has a real exit code, so its completion message
    /// names that code and `read_shell`, and never carries the "may still be
    /// running" caveat a run with no outcome gets.
    #[tokio::test]
    async fn bg_interrupted_completion_names_the_exit_not_still_running() {
        let (_home, _dir) = crate::test_home();
        let session_id = "bg-interrupted-session".to_string();
        let sessions = store_with_entry(&session_id);
        let shell_id = ShellId::from_index(7);
        let (tx, rx) = tokio::sync::oneshot::channel();
        tx.send(Err(crate::shell::CommandTimedOut {
            timeout: Duration::from_secs(1),
            cmd: "trap 'exit 130' INT; sleep 30".to_string(),
            interrupted: Some(CommandOutcome {
                output: b"TRAPPED".to_vec(),
                exit_code: 130,
                nonce: crate::shell::Nonce::new(),
            }),
        }
        .into()))
            .expect("the receiver is alive");
        let start = BackgroundStart {
            run_id: RunId::new(),
            shell_id: shell_id.clone(),
            reg: ShellRegistry::default(),
            before: None,
        };
        let handle = CommandHandle {
            outcome: rx,
            nonce: crate::shell::Nonce::new(),
        };

        await_and_notify(
            sessions.clone(),
            session_id.clone(),
            "trap 'exit 130' INT; sleep 30".to_string(),
            start,
            handle,
        )
        .await;

        let content = with_sessions(&sessions, |store| {
            let entry = store.get(&session_id).expect("the session entry is back");
            entry
                .messages
                .last()
                .expect("a completion was pushed")
                .content
                .clone()
        });
        assert!(
            content.starts_with("[Background Task Completed]"),
            "the prefixed literal leads: {content:?}"
        );
        assert!(content.contains("130"), "the real exit code: {content:?}");
        assert!(content.contains("Ctrl-C"), "the interrupt: {content:?}");
        assert!(
            content.contains(&format!("read_shell(shell: \"{shell_id}\")")),
            "where the output is read: {content:?}"
        );
        assert!(
            !content.contains("may still be running"),
            "an exited command is not still running: {content:?}"
        );
        assert!(!content.contains(".."), "no doubled full stop: {content:?}");
    }

    #[tokio::test]
    async fn bg_completion_records_the_commands_exit_code() {
        let (_home, _dir) = crate::test_home();
        let reg = ShellRegistry::default();
        let session_id = "bg-record-session".to_string();
        let sessions = store_with_entry(&session_id);
        let (start, handle) = start_background_with(
            &reg,
            Owner::Chat(session_id.clone()),
            "printf 'DONE\\n'; sh -c 'exit 4'",
            5,
            spawn_in_process_host,
        )
        .await
        .expect("the command starts");

        await_and_notify(
            sessions.clone(),
            session_id.clone(),
            "printf 'DONE\\n'; sh -c 'exit 4'".to_string(),
            start,
            handle,
        )
        .await;

        let record = last_written_event_record();
        assert_eq!(
            record["event"], "command",
            "the completion writes a command record"
        );
        assert_eq!(record["status"], "completed", "a finished run is completed");
        assert_eq!(record["wait"], false, "the run was wait: false");
        assert_eq!(record["shell"], "s0", "the record names the shell");
        assert_eq!(record["exit"], 4, "the command's real exit code");
    }

    /// Hang the run's shell up before its outcome is awaited: the reader task
    /// then ends without an end marker and the handle resolves to `Err`, the
    /// shape of a completion that failed.
    async fn close_shell_in_process(shell_id: ShellId) {
        let socket = crate::shell::socket_path(shell_id.as_str());
        let _ = crate::shell::close_or_term(&socket, std::time::Duration::from_secs(5)).await;
    }

    /// A finished background run is indexed on its dedicated shell: the
    /// `.meta.json` gains one record with the run's real exit code, and
    /// `read_shell` returns what the command printed. This is the finding
    /// B9 fix -- before it, a `wait: false` run saved no index and its
    /// dedicated shell reported a missing command index on `read_shell`.
    #[tokio::test]
    async fn background_outcome_is_indexed() {
        let (_home, _dir) = crate::test_home();
        let reg = ShellRegistry::default();
        let session_id = "bg-indexed-session".to_string();
        let sessions = store_with_entry(&session_id);
        let (start, handle) = start_background_with(
            &reg,
            Owner::Chat(session_id.clone()),
            "echo hi",
            5,
            spawn_in_logs_dir,
        )
        .await
        .expect("the command starts");

        await_and_notify(
            sessions.clone(),
            session_id.clone(),
            "echo hi".to_string(),
            start.clone(),
            handle,
        )
        .await;

        let started = crate::shell::with_shells(&reg, |store| {
            store
                .get(start.shell_id.as_str())
                .map(|r| r.started_unix)
                .unwrap_or(0)
        });
        let cast = crate::shell::resolve_cast(start.shell_id.as_str(), started);
        let meta = crate::shell::MetaIndex::load(&crate::shell::meta_path_for(&cast))
            .expect("the background run was indexed");
        assert_eq!(
            meta.commands.len(),
            1,
            "one indexed record for the one background command: {:?}",
            meta.commands
        );
        assert_eq!(
            meta.commands[0].exit_code, 0,
            "the record carries the run's real exit code: {:?}",
            meta.commands[0]
        );
        let out = crate::daemon::executor::knowledge::read_shell_in(
            &reg,
            start.shell_id.as_str(),
            None,
            None,
            None,
        );
        assert!(
            out.contains("hi"),
            "read_shell returns the command's output: {out}"
        );
    }

    /// Hang the run's shell up before its outcome is awaited: the reader task
    /// then ends without an end marker and the handle resolves to `Err`, the
    /// shape of a completion that failed.
    #[tokio::test]
    async fn bg_completion_on_a_failed_outcome_records_status_failed() {
        let (_home, _dir) = crate::test_home();
        let reg = ShellRegistry::default();
        let session_id = "bg-record-failed-session".to_string();
        let sessions = store_with_entry(&session_id);
        let (start, handle) = start_background_with(
            &reg,
            Owner::Chat(session_id.clone()),
            "printf 'ORPHAN\\n'",
            5,
            spawn_in_process_host,
        )
        .await
        .expect("the command starts");
        with_sessions(&sessions, |store| {
            store.remove(&session_id);
        });
        close_shell_in_process(start.shell_id.clone()).await;

        await_and_notify(
            sessions.clone(),
            session_id.clone(),
            "printf 'ORPHAN\\n'".to_string(),
            start,
            handle,
        )
        .await;

        let record = last_written_event_record();
        assert_eq!(
            record["event"], "command",
            "the completion writes a command record"
        );
        assert_eq!(
            record["status"], "failed",
            "an Err outcome is a failed record"
        );
        assert_eq!(
            record["exit"],
            serde_json::Value::Null,
            "no exit code is known"
        );
    }

    #[tokio::test]
    async fn bg_notify_on_a_vanished_session_is_a_silent_return() {
        let (_home, _dir) = crate::test_home();
        let reg = ShellRegistry::default();
        let session_id = "bg-vanished-session".to_string();
        let sessions = store_with_entry(&session_id);
        let (start, handle) = start_background_with(
            &reg,
            Owner::Chat(session_id.clone()),
            "printf 'ORPHAN\\n'",
            5,
            spawn_in_process_host,
        )
        .await
        .expect("the command starts");
        with_sessions(&sessions, |store| {
            store.remove(&session_id);
        });

        await_and_notify(
            sessions.clone(),
            session_id.clone(),
            "printf 'ORPHAN\\n'".to_string(),
            start,
            handle,
        )
        .await;

        let count = with_sessions(&sessions, |store| store.len());
        assert_eq!(
            count, 0,
            "a vanished session must stay vanished — no entry resurrected"
        );
    }

    // ── the sandboxed background start — real PTY host ───────────────────────

    #[tokio::test]
    async fn bg_start_in_sandbox_places_a_dedicated_sandbox_shell() {
        let (_home, _dir) = crate::test_home();
        let reg = ShellRegistry::default();
        let (start, handle) = start_background_in_sandbox_with(
            &reg,
            Owner::Chat("bg-sbx".into()),
            "echo IN_SANDBOX",
            5,
            "de-chat-bg-sbx",
            spawn_in_process_host,
        )
        .await
        .expect("the sandbox shell starts");

        let record = with_shells(&reg, |store| {
            store
                .get(start.shell_id.as_str())
                .map(|r| (r.sandbox.clone(), r.dedicated, r.owner.key()))
        })
        .expect("the shell's record is registered");
        assert_eq!(
            record.0,
            Some("de-chat-bg-sbx".to_string()),
            "the record must name the sandbox it runs in"
        );
        assert!(record.1, "a background shell must be dedicated");
        assert_eq!(record.2, "chat:bg-sbx", "the shell belongs to the session");

        let outcome = handle
            .outcome
            .await
            .expect("the reader task stays alive to deliver")
            .expect("the command finishes inside the timeout");
        let text = String::from_utf8_lossy(&outcome.output);
        assert!(
            text.contains("IN_SANDBOX"),
            "the command's bytes must arrive through the sandbox shell: {text:?}"
        );
    }

    #[tokio::test]
    async fn bg_start_in_sandbox_refuses_an_interactive_command() {
        let (_home, _dir) = crate::test_home();
        let reg = ShellRegistry::default();
        let result = start_background_in_sandbox_with(
            &reg,
            Owner::Chat("bg-sbx-interactive".into()),
            "ssh example.com",
            5,
            "de-chat-bg-sbx-interactive",
            spawn_in_process_host,
        )
        .await;
        let err = result.expect_err("an interactive command must be refused, not started");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("interactive"),
            "the refusal must name the interactive problem: {msg}"
        );
        let guard = reg.try_lock().expect("registry test peek");
        assert!(
            guard.is_empty(),
            "the refusal comes before any spawn: no shell, no record"
        );
    }

    // ── await_notify_and_close — real PTY host + a real SessionStore ─────────

    #[tokio::test]
    async fn bg_notify_and_close_closes_after_the_message_lands() {
        let (_home, _dir) = crate::test_home();
        let reg = ShellRegistry::default();
        let session_id = "bg-close".to_string();
        let sessions = store_with_entry(&session_id);
        let (start, handle) = start_background_with(
            &reg,
            Owner::Chat(session_id.clone()),
            "echo DONE",
            5,
            spawn_in_process_host,
        )
        .await
        .expect("the command starts");

        let closed: Arc<Mutex<Vec<(String, bool)>>> = Arc::new(Mutex::new(Vec::new()));
        let close_log = Arc::clone(&closed);
        await_notify_and_close(
            sessions.clone(),
            session_id.clone(),
            "echo DONE".to_string(),
            start.clone(),
            handle,
            move |id: ShellId| {
                let close_log = Arc::clone(&close_log);
                async move {
                    let message_there = with_sessions(&sessions, |store| {
                        store
                            .get(&session_id)
                            .map(|entry| {
                                entry
                                    .messages
                                    .iter()
                                    .any(|m| m.content.starts_with("[Background Task Completed]"))
                            })
                            .unwrap_or(false)
                    });
                    close_log
                        .lock()
                        .expect("close log lock")
                        .push((id.as_str().to_string(), message_there));
                }
            },
        )
        .await;

        let log = closed.lock().expect("close log lock");
        assert_eq!(
            log.len(),
            1,
            "the close step runs exactly once, after the await: {log:?}"
        );
        assert_eq!(
            log[0].0,
            start.shell_id.as_str(),
            "the close step is handed the run's own shell id: {log:?}"
        );
        assert!(
            log[0].1,
            "the [Background Task Completed] message must already be in the \
             session when the close runs: {log:?}"
        );
    }
}

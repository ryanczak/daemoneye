//! A chat turn's `run_terminal_command(background=true)` on the PTY shell
//! substrate (M24 phase 05b): start the command on a **dedicated** shell and
//! return immediately, then deliver its real exit code later as a
//! `[Background Task Completed]` message injected into the session.
//!
//! The tmux background path (`foreground::run_background` → `de-bg-*`), for
//! a ghost session or `backend = "tmux"` (the opt-out), keeps using it. This module is the
//! `backend = "pty"` branch's engine — the default — and, like `shell_exec`,
//! takes no `tx`/`rx`, so every part of it is testable against a real
//! in-process PTY host.

use std::time::Duration;

use super::shell_exec;
use crate::ai::Message;
use crate::daemon::run::{
    RunId, RunKind, RunState, TurnLogEvent, TurnLogRecord, append_turn_event,
};
use crate::daemon::session::{SessionStore, append_session_message, with_sessions};
use crate::shell::{
    CommandHandle, CommandOutcome, Owner, ShellId, ShellRegistry, login_shell, spawn_command,
    spawn_dedicated_for,
};

/// How long a started background command may run before its reader task gives
/// up and reports it incomplete. Deliberately **not** the 45-second
/// foreground timeout: a background command is *expected* to outlive the turn
/// that started it — a build, a test suite, a data copy. The ceiling exists
/// only so a wedged command cannot leak its reader task forever; the outcome
/// it would have carried is reported as "did not complete" with the
/// may-still-be-running warning.
pub(super) const BACKGROUND_CMD_TIMEOUT: Duration = Duration::from_secs(3600);

/// The ids a started background run hands back to the model.
#[derive(Debug, Clone)]
pub(super) struct BackgroundStart {
    pub run_id: RunId,
    pub shell_id: ShellId,
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
            "`{cmd}` is an interactive command and cannot run with background=true — \
             a background ssh/mosh session can never be typed into. Run it in the \
             foreground instead (background=false)."
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
    append_turn_event(&TurnLogRecord {
        timestamp: chrono::Utc::now().to_rfc3339(),
        run_id: run_id.clone(),
        event: TurnLogEvent::RunStarted {
            kind: RunKind::Chat {
                session: owner.key(),
            },
        },
    });
    let handle = spawn_command(&socket, cmd, &login_shell(), BACKGROUND_CMD_TIMEOUT).await?;
    Ok((BackgroundStart { run_id, shell_id }, handle))
}

/// The production form: [`start_background_with`] with the real spawn. The
/// PTY geometry matches `shell_exec`'s host spawn (24×80).
pub(super) async fn start_background(
    reg: &ShellRegistry,
    owner: Owner,
    cmd: &str,
    max_per_owner: u32,
) -> anyhow::Result<(BackgroundStart, CommandHandle)> {
    start_background_with(reg, owner, cmd, max_per_owner, |id| {
        crate::shell::spawn_detached_host(id, 24, 80)
    })
    .await
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
        // Same annotate → mask → normalize order `format_for_model` applies
        // (F3): the annotation strips control bytes before masking, or an
        // escape embedded mid-token could split a secret past its regex.
        Ok(o) => {
            let text = shell_exec::format_for_model(&String::from_utf8_lossy(&o.output));
            if text.is_empty() {
                "(no output)".to_string()
            } else {
                format!("<output>\n{text}\n</output>")
            }
        }
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
    let BackgroundStart { run_id, shell_id } = start;
    let outcome: Result<CommandOutcome, String> = match handle.outcome.await {
        Ok(Ok(inner)) => Ok(inner),
        Ok(Err(e)) => Err(format!("{e:#}")),
        Err(_) => Err("the command's reader task ended without delivering an outcome".to_string()),
    };
    let state = report_state(&outcome);

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
            completion_message(&cmd, &run_id, &shell_id, &outcome)
        ),
        tool_calls: None,
        tool_results: None,
        turn: None,
    };
    append_session_message(&session_id, &completion_msg);

    // Phase 3 (locked): push the message into the in-memory history.
    with_sessions(&sessions, |store| {
        if let Some(entry) = store.get_mut(&session_id) {
            entry.messages.push(completion_msg);
        }
    });

    // Phase 4 (unlocked): the run's own record.
    append_turn_event(&TurnLogRecord {
        timestamp: chrono::Utc::now().to_rfc3339(),
        run_id,
        event: TurnLogEvent::RunFinished { state },
    });
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
    use crate::config::Config;
    use crate::daemon::session::SessionEntry;
    use crate::shell::{Nonce, PtyBackend, bind, lock_path, serve, socket_path};
    use anyhow::Context;
    use std::path::PathBuf;
    use std::sync::Arc;

    const ROWS: u16 = 24;
    const COLS: u16 = 80;

    /// Point `HOME` at a fresh temp dir for the duration of the test and
    /// serialise against other HOME-mutating tests (bug-05c-1).
    fn test_home() -> (crate::TestHomeGuard, tempfile::TempDir) {
        let guard = crate::test_home_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("HOME", dir.path()) }
        Config::ensure_dirs().ok();
        (guard, dir)
    }

    /// The production spawn substitute: a real in-process PTY host serving
    /// the configured socket for `id`, a bash session whose `.cast` the
    /// returned path points at. `spawn_detached_host` cannot run under a test
    /// (07b F4).
    async fn spawn_with_cast(id: ShellId) -> anyhow::Result<PathBuf> {
        let dir = tempfile::tempdir().context("tempdir for cast")?;
        let cast = dir.path().join(format!("{}.cast", id.as_str()));
        let backend =
            PtyBackend::spawn("bash", ROWS, COLS, 100, &cast, 0).context("spawning bash")?;
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
            pipe_source_pane: None,
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
        let (_home, _dir) = test_home();
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
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("bg-dedicated".into());
        // Give the owner a live reusable shell first, exactly as the
        // foreground path would have. (This is *setup* — the whole point of
        // the test is that the engine does NOT reuse it.)
        let (foreground_id, _) = crate::shell::shell_for_owner(
            &reg,
            owner.clone(),
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
        let (_home, _dir) = test_home();
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
            msg.contains("foreground"),
            "the refusal must name the foreground alternative: {msg}"
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
        let (_home, _dir) = test_home();
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

    #[tokio::test]
    async fn bg_notify_on_a_vanished_session_is_a_silent_return() {
        let (_home, _dir) = test_home();
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
}

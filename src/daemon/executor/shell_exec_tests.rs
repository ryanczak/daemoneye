use super::*;
use crate::shell::{ShellResponse, bind, lock_path, screen, send_input, serve, socket_path};
use anyhow::Context;
use std::sync::Arc;

#[test]
fn exit_status_annotation_unknown_is_silent() {
    assert!(exit_status_annotation(None).is_none());
}

#[test]
fn exit_status_annotation_zero_is_silent() {
    assert!(exit_status_annotation(Some(0)).is_none());
}

#[test]
fn exit_status_annotation_nonzero_notes_code() {
    let s = exit_status_annotation(Some(2));
    assert!(s.is_some());
    assert!(s.as_ref().unwrap().contains("2"));
}

/// The PTY geometry every shell host is spawned with, matching the
/// detached `spawn_detached_host` production spawn (24×80).
const ROWS: u16 = 24;
/// The PTY geometry every shell host is spawned with.
const COLS: u16 = 80;

/// A fake program that prompts like real sudo: it re-prompts on an empty
/// answer and prints the answered length. Spelled `f=su; f+=do; eval …`
/// so the defining command does not itself name the privilege tool and is
/// not routed through the prompt path.
const PROMPTING: &str = r#"f=su; f+=do; eval "$f() { local pw=; while [ -z \"\$pw\" ]; do read -rs -p \"\$SUDO_PROMPT\" pw || return 1; echo; done; echo GOT_\${#pw}; }""#;
/// A fake program that never prompts.
const SILENT: &str = r#"f=su; f+=do; eval "$f() { echo NOPROMPT; }""#;

/// The credential the "answered" transport holds, three
/// `Request::CredentialResponse` lines over a `Cursor`.
const CREDENTIAL: &str = "s3cret-answer-q";

/// The "attached" write half of [`answered_transport`]: records every
/// write the run path makes (the credential prompt goes here, not the
/// PTY) and discards them.
struct RecordingWriter {
    writes: Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
}

impl RecordingWriter {
    fn new(writes: Arc<std::sync::Mutex<Vec<Vec<u8>>>>) -> Self {
        Self { writes }
    }
}

impl tokio::io::AsyncWrite for RecordingWriter {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        self.writes
            .lock()
            .expect("write log lock")
            .push(buf.to_vec());
        std::task::Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

/// An "attached" transport: the write half records every write (the
/// credential prompt goes there, not the PTY), the read half answers
/// every prompt with [`CREDENTIAL`]. Over a `Cursor`, so the read half
/// is exhausted exactly when the three attempts are.
fn answered_transport() -> (
    Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
    impl tokio::io::AsyncBufRead + Unpin,
) {
    let writes = Arc::new(std::sync::Mutex::new(Vec::new()));
    let lines = (0..3)
        .map(|_| {
            let mut line = serde_json::to_string(&Request::CredentialResponse {
                id: "p".to_string(),
                credential: CREDENTIAL.to_string(),
            })
            .expect("CredentialResponse serialises");
            line.push('\n');
            line
        })
        .collect::<Vec<_>>()
        .join("");
    (writes, std::io::Cursor::new(lines.into_bytes()))
}

/// Run `cmd` in the shell a first (seed) command opened, with the given
/// transport halves — "nobody attached" is sink/EOF, "answered" is
/// [`answered_transport`]. The seed runs through the same run path with
/// sink/EOF, defining the fake program; the command under test then runs
/// in that same shell with the real transport. Returns the shell id, the
/// run result and the cast path, so the log can be read raw.
async fn run_with_transport<'p, W, R>(
    reg: &ShellRegistry,
    owner: Owner,
    seed_cmd: &str,
    cmd: &str,
    writer: &'p mut W,
    reader: &'p mut R,
) -> anyhow::Result<(ShellId, ShellRunResult, std::path::PathBuf)>
where
    W: tokio::io::AsyncWriteExt + Unpin + 'p,
    R: tokio::io::AsyncBufReadExt + Unpin + 'p,
{
    let cast_path: Arc<std::sync::Mutex<Option<std::path::PathBuf>>> =
        Arc::new(std::sync::Mutex::new(None));
    let cells = Arc::clone(&cast_path);
    let (id, _) = run_in_shell_with_prompts(
        reg,
        owner.clone(),
        seed_cmd,
        5,
        None,
        Some(20),
        "",
        &mut tokio::io::sink(),
        &mut tokio::io::empty(),
        crate::host::HostId::Local,
        move |id: ShellId| {
            let cells = Arc::clone(&cells);
            async move {
                let path = spawn_with_cast(id).await?;
                std::mem::forget(path.clone());
                *cells.lock().expect("cast cell lock") = Some(path);
                Ok(())
            }
        },
    )
    .await
    .context("the seed command runs")?;
    // The command under test, in the same shell the seed defined the fake
    // in — with the real transport this time.
    let (_same_id, out) = run_in_shell_with_prompts(
        reg,
        owner,
        cmd,
        5,
        Some(id.as_str()),
        Some(20),
        "",
        writer,
        reader,
        crate::host::HostId::Local,
        move |_id: ShellId| async move { Ok(()) },
    )
    .await?;
    let cast = cast_path
        .lock()
        .expect("cast cell lock")
        .clone()
        .expect("the spawn step recorded the host's cast path");
    Ok((id, out, cast))
}

/// `$SHELL` pointed at fish for the duration of one test, restored by
/// `Drop` even when an assertion fails mid-test — the guard form of the
/// session module's `with_fish_shell`, which cannot early-return.
pub(super) struct FishShellEnv;

impl FishShellEnv {
    pub(super) fn set() -> Self {
        unsafe {
            std::env::set_var("SHELL", "/usr/bin/fish");
        }
        FishShellEnv
    }
}

impl Drop for FishShellEnv {
    fn drop(&mut self) {
        unsafe {
            std::env::remove_var("SHELL");
        }
    }
}

/// The production spawn substitute: a real in-process PTY host serving the
/// configured socket for `id`, a bash session whose `.cast` the returned
/// path points at. `spawn_detached_host` cannot run under a test (07b F4).
async fn spawn_with_cast(id: ShellId) -> anyhow::Result<std::path::PathBuf> {
    let dir = tempfile::tempdir().context("tempdir for cast")?;
    let cast = dir.path().join(format!("{}.cast", id.as_str()));
    let backend = crate::shell::PtyBackend::spawn("bash", &[], ROWS, COLS, 100, &cast, 0)
        .context("spawning bash")?;
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

/// The spawn step `run_in_shell_with` takes, paired with the cast path the
/// host writes so a test can read the raw log afterwards.
pub(super) async fn spawn_in_process_host(id: ShellId) -> anyhow::Result<()> {
    let cast = spawn_with_cast(id.clone()).await?;
    std::mem::forget(cast);
    Ok(())
}

/// The historical no-transport run, the shape every pre-existing test in
/// this module exercises: `run_in_shell_with_prompts` with a sink write
/// half and an EOF read half, so the sudo branch is off and the answer
/// path can never be entered.
async fn run_in_shell_for_test<S, Fut>(
    reg: &ShellRegistry,
    owner: Owner,
    cmd: &str,
    max_per_owner: u32,
    shell: Option<&str>,
    timeout_secs: Option<u64>,
    spawn: S,
) -> anyhow::Result<(ShellId, ShellRunResult)>
where
    S: FnOnce(ShellId) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    run_in_shell_for_test_on_host(
        reg,
        owner,
        cmd,
        max_per_owner,
        shell,
        timeout_secs,
        crate::host::HostId::Local,
        spawn,
    )
    .await
}

/// `run_in_shell_for_test` with the host explicit — the shape the host
/// tests drive: the reuse and create steps are host-keyed, so a test that
/// names a host must pass one through rather than rely on the local
/// default.
#[allow(clippy::too_many_arguments)]
async fn run_in_shell_for_test_on_host<S, Fut>(
    reg: &ShellRegistry,
    owner: Owner,
    cmd: &str,
    max_per_owner: u32,
    shell: Option<&str>,
    timeout_secs: Option<u64>,
    host: crate::host::HostId,
    spawn: S,
) -> anyhow::Result<(ShellId, ShellRunResult)>
where
    S: FnOnce(ShellId) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    let mut sink = tokio::io::sink();
    let mut eof = tokio::io::empty();
    run_in_shell_with_prompts(
        reg,
        owner,
        cmd,
        max_per_owner,
        shell,
        timeout_secs,
        "",
        &mut sink,
        &mut eof,
        host,
        spawn,
    )
    .await
}

/// A spawn substitute whose cast the producer can resolve through the
/// ordinary path: `cast_path` under the test `HOME`'s `shell_logs_dir`,
/// the shape `resolve_cast` + `meta_path_for` operate on in production.
/// The per-id lock is leaked so the host reads as live.
async fn spawn_in_logs_dir(id: ShellId) -> anyhow::Result<()> {
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let cast = crate::shell::cast_path(id.as_str(), started);
    std::fs::create_dir_all(cast.parent().context("cast has a parent")?)
        .context("creating the logs dir")?;
    let backend = crate::shell::PtyBackend::spawn("bash", &[], ROWS, COLS, 100, &cast, started)
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

#[test]
fn resolve_shell_target_accepts_none_new_and_an_id() {
    assert!(matches!(resolve_shell_target(None), Ok(ShellTarget::Reuse)));
    assert!(matches!(
        resolve_shell_target(Some("new")),
        Ok(ShellTarget::New)
    ));
    match resolve_shell_target(Some("s7")) {
        Ok(ShellTarget::Existing(id)) => assert_eq!(id.as_str(), "s7"),
        other => panic!("Some(\"s7\") must resolve to Existing, got {other:?}"),
    }
}

#[test]
fn resolve_shell_target_rejects_a_bad_value() {
    let err =
        resolve_shell_target(Some("banana")).expect_err("a non-id, non-new value must be refused");
    assert!(
        err.contains("banana"),
        "the error must name the offending value: {err}"
    );
    assert!(
        err.contains("new") && err.contains("list_shells"),
        "the error must name both accepted forms: {err}"
    );
}

#[test]
fn resolve_shell_target_is_case_sensitive() {
    for bad in ["New", "NEW", "New "] {
        assert!(
            resolve_shell_target(Some(bad)).is_err(),
            "{bad} must not match \"new\" — the match is exact-case like \
                 list_shells' scope"
        );
    }
}

#[tokio::test]
async fn run_terminal_command_targets_the_named_shell() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let owner = Owner::Chat("tip".into());
    // The reuse rule's pick, opened first so it is the lowest-numbered one.
    let (reused, _) = shell_for_owner(
        &reg,
        owner.clone(),
        crate::host::HostId::Local,
        5,
        SHELL_READY_TIMEOUT,
        spawn_in_process_host,
    )
    .await
    .expect("the reuse shell comes up");
    // A second shell the reuse rule would never hand out: opening it
    // directly, exactly as `shelf: "new"` does, bypasses the reuse rule.
    let second = spawn_for_on_host(
        &reg,
        owner.clone(),
        5,
        crate::host::HostId::Local,
        false,
        spawn_in_process_host,
    )
    .await
    .expect("the second shell comes up");
    await_ready(
        &socket_path(second.as_str()),
        Instant::now() + SHELL_READY_TIMEOUT,
    )
    .await
    .expect("the second shell becomes ready");
    assert_ne!(reused.as_str(), second.as_str(), "fixture: two ids");

    // The reuse rule would pick `skipped`; naming `second` must win.
    let (id, result) = run_in_shell_for_test(
        &reg,
        owner,
        "echo TARGET_HIT",
        5,
        Some(second.as_str()),
        None,
        spawn_in_process_host,
    )
    .await
    .expect("the targeted command runs");
    assert_eq!(
        id.as_str(),
        second.as_str(),
        "shell: must win over the lowest-numbered reuse rule"
    );
    assert!(
        result.text.contains("TARGET_HIT"),
        "the command's output must come back: {:?}",
        result.text
    );
}

#[tokio::test]
async fn run_terminal_command_refuses_another_owners_shell() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    // Same inner string "a", different kind — the shape a comparison on
    // the inner string instead of `Owner::key()` would accept.
    let (other, _) = shell_for_owner(
        &reg,
        Owner::Ghost("a".into()),
        crate::host::HostId::Local,
        5,
        SHELL_READY_TIMEOUT,
        spawn_in_process_host,
    )
    .await
    .expect("the ghost's shell comes up");

    let err = run_in_shell_for_test(
        &reg,
        Owner::Chat("a".into()),
        "echo SENTINEL_NOT_FOR_GHOST",
        5,
        Some(other.as_str()),
        None,
        spawn_in_process_host,
    )
    .await
    .expect_err("another owner's shell must be refused");
    let msg = format!("{err:#}");
    assert!(
        msg.contains(other.as_str()),
        "the refusal must name the shell id: {msg}"
    );
    let recorded = crate::shell::run_command(
        &socket_path(other.as_str()),
        "echo RECORDED_STILL_ON",
        "bash",
        Duration::from_secs(10),
    )
    .await
    .expect("the ghost's shell still answers after the refusal");
    let text = String::from_utf8_lossy(&recorded.output);
    assert!(
        !text.contains("SENTINEL_NOT_FOR_GHOST"),
        "no byte of the refused command may run in the other owner's shell: {text:?}"
    );
}

#[tokio::test]
async fn run_terminal_command_custom_timeout_names_the_applied_value() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let owner = Owner::Chat("sleeper".into());
    let err = run_in_shell_for_test(
        &reg,
        owner,
        "sleep 4",
        5,
        None,
        Some(1),
        spawn_in_process_host,
    )
    .await
    .expect_err("a command outliving its timeout must be an error");
    let msg = format!("{err:#}");
    assert!(
        msg.contains('1') && msg.contains("limit"),
        "the error must name the applied timeout: {msg}"
    );
    assert!(
        !msg.contains("45s"),
        "the error must name the applied 1 s, not the 45 s default: {msg}"
    );
}

/// While a command runs, the record's `in_flight` names it; once the
/// call returns the guard has cleared it — the run path's Drop, not any
/// epilogue, is what leaves the shell free. The mid-run observation
/// comes from a side task polling the registry while the command runs.
#[tokio::test]
async fn in_flight_is_set_while_a_command_runs_and_cleared_after() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let in_reg = reg.clone();
    let owner = Owner::Chat("in-flight".into());
    let cmd = "sleep 1";
    let seen: Arc<std::sync::Mutex<Option<crate::shell::InFlight>>> =
        Arc::new(std::sync::Mutex::new(None));
    let seen_in = Arc::clone(&seen);
    tokio::spawn(async move {
        loop {
            let now = with_shells(&in_reg, |store| {
                store
                    .values()
                    .find(|record| record.in_flight.is_some())
                    .and_then(|record| record.in_flight.clone())
            });
            if let Some(flight) = now {
                *seen_in.lock().expect("seen lock") = Some(flight);
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
    let (id, _) = run_in_shell_for_test(&reg, owner, cmd, 5, None, None, spawn_in_process_host)
        .await
        .expect("the sleep runs");
    let watched = seen.lock().expect("seen lock").clone();
    assert!(
        watched.as_ref().is_some_and(|f| f.cmd == cmd),
        "in_flight must name the running command while it runs: {watched:?}"
    );
    let after = with_shells(&reg, |store| {
        store
            .get(id.as_str())
            .and_then(|record| record.in_flight.clone())
    });
    assert!(
        after.is_none(),
        "in_flight must be cleared once the run returns: {after:?}"
    );
}

/// A command that times out ends the run the ordinary way — through the
/// guard's Drop — so the shell must not be left marked busy by a failure.
#[tokio::test]
async fn in_flight_is_cleared_when_the_command_times_out() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let owner = Owner::Chat("in-flight-timeout".into());
    let err = run_in_shell_for_test(
        &reg,
        owner,
        "sleep 4",
        5,
        None,
        Some(1),
        spawn_in_process_host,
    )
    .await
    .expect_err("a command outliving its timeout must be an error");
    assert!(
        format!("{err:#}").contains("timed out"),
        "the failure is the timeout: {err:#}"
    );
    let still_marked: Vec<String> = with_shells(&reg, |store| {
        store
            .values()
            .filter(|record| record.in_flight.is_some())
            .map(|record| record.id.as_str().to_string())
            .collect()
    });
    assert!(
        still_marked.is_empty(),
        "no shell may stay marked in flight after a timeout: {still_marked:?}"
    );
}

#[tokio::test]
async fn shell_exec_returns_the_real_exit_code() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let (id, result) = run_in_shell_for_test(
        &reg,
        Owner::Chat("exit-code".into()),
        "printf 'HELLO\\n'; sh -c 'exit 3'",
        5,
        None,
        None,
        spawn_in_process_host,
    )
    .await
    .expect("an approved, printing, failing command resolves");
    assert_eq!(result.exit_code, 3, "the child's real exit code surfaces");
    assert!(
        result.text.contains("HELLO"),
        "the output must survive alongside the exit code: {:?}",
        result.text
    );
    assert!(
        result.text.contains("exited with status 3"),
        "the non-zero note must be present: {:?}",
        result.text
    );
    assert!(
        id.as_str().starts_with('s'),
        "a fresh id is allocated for the first command"
    );
}

/// F2: `finish_run` is reached only with a marker-terminated outcome, so
/// the shell is back at its prompt — and the record must say so, exactly
/// as `probe_foreground` would have stamped it.
#[tokio::test]
async fn a_finished_command_leaves_the_shell_at_its_prompt() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let started = now_unix();
    let (id, result) = run_in_shell_for_test(
        &reg,
        Owner::Chat("prompt-stamp".into()),
        "echo hi",
        5,
        None,
        None,
        spawn_in_process_host,
    )
    .await
    .expect("the command resolves");
    assert_eq!(result.exit_code, 0, "echo hi is clean");
    let finished = now_unix();
    let stored = with_shells(&reg, |store| {
        store
            .get(id.as_str())
            .and_then(|r| r.last_foreground.clone())
    });
    let (fg, at) = stored.expect("the record holds the stamp");
    assert_eq!(
        fg,
        Foreground::Prompt,
        "a finished waited command leaves the shell at its prompt"
    );
    assert!(
        (started..=finished).contains(&at),
        "the stamp's second {at} sits inside the test's own wall-clock window \
             [{started}, {finished}]"
    );
}

/// The refusal path never reaches `finish_run`, so a refused command
/// must not overwrite what the last real probe observed: a shell the
/// busy probe read `Program` stays `Program`.
#[tokio::test]
async fn a_busy_shell_is_not_marked_at_its_prompt() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let owner = Owner::Chat("busy-named-stamp".into());
    let (id, _) = shell_for_owner(
        &reg,
        owner.clone(),
        crate::host::HostId::Local,
        5,
        SHELL_READY_TIMEOUT,
        spawn_in_process_host,
    )
    .await
    .expect("the shell comes up");
    let busy_socket = socket_path(id.as_str());
    send_input(&busy_socket, b"sleep 30\n", Duration::from_secs(5))
        .await
        .expect("the shell accepts the input");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let fg = probe_foreground(&reg, &id, PROBE_TIMEOUT).await;
        if matches!(fg, Foreground::Program { .. }) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the shell never read busy: {fg:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let before = with_shells(&reg, |store| {
        store
            .get(id.as_str())
            .and_then(|r| r.last_foreground.clone())
    })
    .expect("the probe stamped the record");
    assert_ne!(before.1, 0, "the probe stamped a real unix second");

    let err = run_in_shell_for_test(
        &reg,
        owner,
        "echo BUSY_REFUSAL_MUST_NOT_RUN",
        5,
        Some(id.as_str()),
        None,
        spawn_in_process_host,
    )
    .await
    .expect_err("the busy shell refuses the command");
    assert!(
        format!("{err:#}").contains("is running sleep"),
        "the refusal is the busy answer: {err:#}"
    );
    let after = with_shells(&reg, |store| {
        store
            .get(id.as_str())
            .and_then(|r| r.last_foreground.clone())
    });
    assert_eq!(
        after.as_ref().map(|(fg, _)| fg),
        Some(&Foreground::Program {
            cmd: "sleep".to_string(),
            tui: false
        }),
        "a refused command must not stamp the record at its prompt: the \
             foreground is still the program the probe saw"
    );
}

#[tokio::test]
async fn shell_exec_masks_a_secret_that_stays_raw_in_the_cast() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let cast_path: Arc<std::sync::Mutex<Option<std::path::PathBuf>>> =
        Arc::new(std::sync::Mutex::new(None));
    let spawn_cast_path = Arc::clone(&cast_path);
    let (id, result) = run_in_shell_for_test(
        &reg,
        Owner::Chat("masking".into()),
        "echo AKIAIOSFODNN7EXAMPLE",
        5,
        None,
        None,
        move |id: ShellId| {
            let cells = Arc::clone(&spawn_cast_path);
            async move {
                let path = spawn_with_cast(id).await?;
                std::mem::forget(path.clone());
                *cells.lock().expect("cast cell lock") = Some(path);
                Ok(())
            }
        },
    )
    .await
    .expect("the seeded AWS key resolves");
    assert!(
        !result.text.contains("AKIAIOSFODNN7EXAMPLE"),
        "the secret must not reach the tool result: {:?}",
        result.text
    );
    assert!(
        result.text.contains("<AWS_KEY>"),
        "the mask marker must reach the tool result: {:?}",
        result.text
    );
    let cast = std::fs::read(
        cast_path
            .lock()
            .expect("cast cell lock")
            .clone()
            .expect("the spawn step recorded the host's cast path"),
    )
    .with_context(|| format!("reading the cast the host {} wrote", id.as_str()))
    .expect("the cast is readable");
    let cast = String::from_utf8_lossy(&cast);
    assert!(
        cast.contains("AKIAIOSFODNN7EXAMPLE"),
        "the log stays raw at rest — the M20 masking exit criterion is the pair"
    );
}

#[tokio::test]
async fn shell_exec_annotates_colour_and_strips_control_bytes() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let (_, result) = run_in_shell_for_test(
        &reg,
        Owner::Chat("colour".into()),
        "printf '\\033[31mred\\033[0m\\n\\033[2J\\033[Hclear\\n'",
        5,
        None,
        None,
        spawn_in_process_host,
    )
    .await
    .expect("the coloured command resolves");
    assert!(
        result.text.contains("[ERROR: red]"),
        "red becomes a semantic marker: {:?}",
        result.text
    );
    assert!(
        !result.text.contains('\x1b'),
        "no ESC byte may reach the model: {:?}",
        result.text
    );
}

#[tokio::test]
async fn shell_exec_reuses_the_owners_shell_across_calls() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let owner = Owner::Chat("reuse".into());
    let (id1, first) = run_in_shell_for_test(
        &reg,
        owner.clone(),
        "cd /tmp",
        5,
        None,
        None,
        spawn_in_process_host,
    )
    .await
    .expect("the first call runs");
    assert_eq!(first.exit_code, 0, "cd /tmp is clean");
    let (id2, second) = run_in_shell_for_test(
        &reg,
        owner.clone(),
        "pwd",
        5,
        None,
        None,
        spawn_in_process_host,
    )
    .await
    .expect("the second call runs");
    assert_eq!(
        id1.as_str(),
        id2.as_str(),
        "the same owner keeps one shell, so action persists: cd then pwd"
    );
    assert!(
        second.text.contains("/tmp"),
        "the working directory survived the first call: {:?}",
        second.text
    );
}

/// 08b's primary case, in the launch shape: an interactive command is
/// launched in the resolved shell — the bare command plus `\r`, no
/// markers — and returns `Ok` with the settled screen without waiting
/// for the session to end (D6). The ssh really runs and fails to resolve
/// the host; the screen shows that honestly, so nothing is asserted
/// about what ssh printed.
#[tokio::test]
async fn run_terminal_command_launches_an_interactive_command() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let start = std::time::Instant::now();
    let (id, result) = run_in_shell_for_test(
        &reg,
        Owner::Chat("interactive".into()),
        "ssh nowhere.invalid",
        5,
        None,
        None,
        spawn_in_process_host,
    )
    .await
    .expect("an interactive command launches instead of being refused");
    let elapsed = start.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "the launch must return without waiting for the session — \
             {elapsed:?} means it waited for ssh to exit"
    );
    assert!(
        result.text.starts_with("[Interactive session started]"),
        "the launch result opens with the launch sentence: {:?}",
        result.text
    );
    assert!(
        result.text.contains(id.as_str()),
        "the launch result names the shell the session holds: {id} — {:?}",
        result.text
    );
    assert_eq!(
        result.exit_code, 0,
        "the launch's exit code is the stats placeholder, not ssh's"
    );
    assert!(
        !result.text.contains("exit"),
        "the launch result names no exit code — nothing exited: {:?}",
        result.text
    );

    // The ssh really ran: the screen part is the host's render, not the
    // launch sentence. What it shows is ssh's own failure to resolve the
    // host — not asserted here, since it is the host's wording, not the
    // daemon's. The record's last observed foreground is whatever the
    // settle read saw, and `ssh nowhere.invalid` fails to resolve at
    // once, so the shell may already be idle again — `sleep 30`
    // (`launch_interactive_returns_the_settled_screen`) pins the busy
    // read.

    // The launch really wrote to the shell: end whatever held it with
    // the interrupt byte, then prove the shell is at its prompt and
    // still usable.
    // holder: end it with the byte that interrupts ssh, then prove the
    // shell is back to its prompt and still usable.
    crate::shell::send_input(
        &crate::shell::socket_path(id.as_str()),
        b"\x03",
        Duration::from_secs(5),
    )
    .await
    .expect("the shell accepts the interrupt");
}
/// The launch path itself: `launch_interactive` writes the bare command
/// plus `\r` into the shell, waits for the program to draw something (the
/// change wait, F1), then reads the settled screen, and returns the header
/// plus the masked screen (D6). `sleep 1; printf 'DRAWN'` draws nothing
/// for a second, so a launch that settled on the echoed command line would
/// miss it; the interrupt at the end is what ends the sleep.
#[tokio::test]
async fn launch_waits_for_a_slow_program_to_draw() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let owner = Owner::Chat("slow-draw".into());
    let (id, _) = run_in_shell_for_test(&reg, owner, "true", 5, None, None, spawn_in_process_host)
        .await
        .expect("the owner's shell comes up");

    let result = launch_interactive(
        &reg,
        &id,
        &socket_path(id.as_str()),
        "sleep 1; printf 'DR%sN\\n' AW",
    )
    .await
    .expect("the launch writes the command and reads the settled screen");
    assert!(
        result.text.contains("DRAWN"),
        "the printf only runs after the sleep, so the settled screen must \
             show the drawn word — a launch that settled on the echoed command \
             line returns before it: {:?}",
        result.text
    );
    crate::shell::send_input(&socket_path(id.as_str()), b"\x03", Duration::from_secs(5))
        .await
        .expect("the shell accepts the interrupt");
}

/// The launch path itself: `launch_interactive` writes the bare command
/// plus `\r` into the shell, waits for the screen to settle, and returns
/// the header plus the masked screen (D6). `sleep 30` really runs, so the
/// record's last observed foreground is the running program and the
/// interrupt at the end is what ends it.
#[tokio::test]
async fn launch_interactive_returns_the_settled_screen() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let owner = Owner::Chat("launch-screen".into());
    let (id, _) = run_in_shell_for_test(&reg, owner, "true", 5, None, None, spawn_in_process_host)
        .await
        .expect("the owner's shell comes up");
    let before = with_shells(&reg, |store| {
        store
            .get(id.as_str())
            .and_then(|r| r.last_foreground.clone())
            .map(|(fg, _)| fg)
    });
    assert_eq!(
        before,
        Some(Foreground::Prompt),
        "the setup command finished, so the record reads its prompt — the \
             launch is what must replace it with the program"
    );

    let result = launch_interactive(&reg, &id, &socket_path(id.as_str()), "sleep 30")
        .await
        .expect("the launch writes the command and reads the settled screen");
    assert!(
        result.text.starts_with("[Interactive session started]"),
        "the result opens with the launch sentence: {:?}",
        result.text
    );
    assert!(
        result.text.contains(id.as_str()),
        "the result names the shell the session holds: {id} — {:?}",
        result.text
    );
    assert!(
        result.text.lines().skip(1).any(|l| l.contains("sleep 30")),
        "the screen part shows the command that was launched: {:?}",
        result.text
    );
    assert_eq!(
        result.exit_code, 0,
        "the launch's exit code is the stats placeholder, not sleep's"
    );
    let fg = with_shells(&reg, |store| {
        store
            .get(id.as_str())
            .and_then(|r| r.last_foreground.clone())
            .map(|(fg, _)| fg)
    });
    assert_eq!(
        fg,
        Some(Foreground::Program {
            cmd: "sleep".into(),
            tui: false,
        }),
        "the settle read records the running program: {fg:?}"
    );

    crate::shell::send_input(&socket_path(id.as_str()), b"\x03", Duration::from_secs(5))
        .await
        .expect("the shell accepts the interrupt");
}

/// Task 3 recipe: `run_interactive_in_shell` launches a command
/// `is_interactive_command` does **not** recognise — `sleep 30` through
/// the waited path would hang on the marker protocol until the timeout,
/// and here it returns `Ok` within 10 s with the launch text. `\x03` at
/// the end ends the session so the shell does not outlive the test.
#[tokio::test]
async fn run_interactive_in_shell_launches_any_command() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let owner = Owner::Chat("interactive-param".into());
    let (id, result) = tokio::time::timeout(
        Duration::from_secs(10),
        run_interactive_in_shell(
            &reg,
            owner.clone(),
            "sleep 30",
            5,
            None,
            crate::host::HostId::Local,
            spawn_in_process_host,
        ),
    )
    .await
    .expect("an interactive launch returns within 10 s")
    .expect("an interactive launch of a non-recognised command succeeds");
    assert!(
        result.text.starts_with("[Interactive session started]"),
        "the launch text opens with the interactive banner: {:?}",
        &result.text[..result.text.len().min(120)]
    );
    let fg = with_shells(&reg, |store| {
        store
            .get(id.as_str())
            .and_then(|r| r.last_foreground.clone())
            .map(|(fg, _)| fg)
    });
    assert_eq!(
        fg,
        Some(Foreground::Program {
            cmd: "sleep".into(),
            tui: false,
        }),
        "the record's last_foreground is the running program: {fg:?}"
    );

    crate::shell::send_input(&socket_path(id.as_str()), b"\x03", Duration::from_secs(5))
        .await
        .expect("the shell accepts the interrupt");
}

/// The F2 regression guard, in the launch shape: an interactive launch
/// keeps its shell — the session holds it, so the reuse rule skips it
/// and the owner's next command lands in a **different** shell id.
/// Set against real in-process hosts.
#[tokio::test]
async fn an_interactive_launch_keeps_its_shell_and_the_next_command_gets_another() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let owner = Owner::Chat("marked".into());
    let (id, _) = run_in_shell_for_test(
        &reg,
        owner.clone(),
        "true",
        5,
        None,
        None,
        spawn_in_process_host,
    )
    .await
    .expect("the owner's shell comes up");

    let launched = launch_interactive(&reg, &id, &socket_path(id.as_str()), "sleep 30")
        .await
        .expect("the launch starts the session");
    assert!(
        launched.text.contains(id.as_str()),
        "the launch names the shell it holds: {:?}",
        launched.text
    );

    // The reuse rule sees the held shell busy, so the next command runs
    // in another shell.
    let (id2, second) = run_in_shell_for_test(
        &reg,
        owner.clone(),
        "echo ALIVE",
        5,
        None,
        None,
        spawn_in_process_host,
    )
    .await
    .expect("the next command must run in a fresh shell");
    assert_ne!(
        id2.as_str(),
        id.as_str(),
        "a shell holding a session is never handed to another command"
    );
    assert_eq!(second.exit_code, 0, "echo actually ran in the new shell");
    assert!(
        second.text.contains("ALIVE"),
        "the fresh shell produced the command's output: {:?}",
        second.text
    );

    // Name the held shell directly: the probe must refuse before any
    // byte is written — the session, not an absence, holds the terminal.
    let err = run_in_shell_for_test(
        &reg,
        owner.clone(),
        "echo HELD_REFUSAL_MUST_NOT_RUN",
        5,
        Some(id.as_str()),
        None,
        spawn_in_process_host,
    )
    .await
    .expect_err("the held shell is refused");
    let msg = format!("{err:#}");
    assert!(
        msg.contains(id.as_str()) && msg.contains("running sleep"),
        "the refusal names the shell and the session holding it: {msg}"
    );

    // End the session and prove the shell is still usable.
    crate::shell::send_input(
        &crate::shell::socket_path(id.as_str()),
        b"\x03",
        Duration::from_secs(5),
    )
    .await
    .expect("the shell accepts the interrupt");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let fg = probe_foreground(&reg, &id, PROBE_TIMEOUT).await;
        if matches!(fg, Foreground::Prompt) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the shell never came back to its prompt after the interrupt: {fg:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let (id3, third) = run_in_shell_for_test(
        &reg,
        owner,
        "echo ENDED_OK",
        5,
        Some(id.as_str()),
        None,
        spawn_in_process_host,
    )
    .await
    .expect("the interrupted shell is usable again");
    assert_eq!(
        id3.as_str(),
        id.as_str(),
        "ending the session returns the shell to the reuse rule"
    );
    assert!(
        third.text.contains("ENDED_OK"),
        "the interrupted shell produced the command's output: {:?}",
        third.text
    );
}

/// An ordinary command runs, then the owner's next one runs in the same
/// shell, both commands run on it, and the reuse filter is untouched.
#[tokio::test]
async fn shell_exec_non_interactive_still_runs_on_the_same_shell() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let owner = Owner::Chat("plain".into());
    let (id1, one) = run_in_shell_for_test(
        &reg,
        owner.clone(),
        "echo one",
        5,
        None,
        None,
        spawn_in_process_host,
    )
    .await
    .expect("the first command runs");
    assert_eq!(one.exit_code, 0, "echo one is clean");
    assert!(
        one.text.contains("one"),
        "the output surfaced: {:?}",
        one.text
    );
    let (id2, two) = run_in_shell_for_test(
        &reg,
        owner.clone(),
        "echo two",
        5,
        None,
        None,
        spawn_in_process_host,
    )
    .await
    .expect("the second command runs");
    assert_eq!(
        id1.as_str(),
        id2.as_str(),
        "no interactive command means the same shell is reused"
    );
    assert_eq!(two.exit_code, 0, "echo two is clean");
    assert!(
        two.text.contains("two"),
        "the output surfaced: {:?}",
        two.text
    );
}

#[test]
fn shell_exec_pipeline_is_annotate_then_mask_then_normalize() {
    assert_eq!(
        format_for_model("AKIAIOSFODNN7EXAMPLE\r\n\x1b[31mred line\x1b[0m\r\n"),
        "<AWS_KEY>\n[ERROR: red line]",
        "annotate strips ESC bytes before masking; normalize folds CRLF"
    );
}

#[test]
fn shell_exec_pipeline_leaves_a_clean_command_as_its_output() {
    assert_eq!(format_for_model("hello\r\n"), "hello");
    assert_eq!(format_for_model("\r\n\r\nhello\r\n\r\n"), "hello");
}

/// bug-07c-3's DoD. There must exist a test that is red against the
/// reversed pipeline, `annotate_output(&normalize_output(&mask_sensitive_(
/// raw)))`, so the annotate→mask order is observable. The input carries an
/// ESC byte mid-token: annotation-first deletes it before the mask regex
/// runs, mask-first leaves the key containing it intact and lets
/// annotate strip only the escape, so the two orderings disagree on the
/// exact string asserted below.
#[test]
fn shell_exec_ordering_guard_masks_a_secret_split_by_an_embedded_escape() {
    let dioxed = "AKIAIOSFOD\x1b[0mNN7EXAMPLE\r\n";
    let masked = format_for_model(dioxed);
    assert!(
        !masked.contains("AKIAIOSFOD"),
        "the secret must not reach the tool result even when a control byte sits mid-token: {masked:?}"
    );
    assert!(
        masked.contains("<AWS_KEY>"),
        "the real mask marker must be present: {masked:?}"
    );
    assert!(
        !masked.contains('\x1b'),
        "no escape may survive into text a model reads: {masked:?}"
    );
}

/// Task 5's call-site guard: the meta record's `command` field holds the
/// string the run path sent to the shell. A command without `sudo` must
/// arrive byte-for-byte as written — no `[de-sudo-prompt-N]` prefix may
/// appear — proving the plain path never allocates a sentinel or wraps.
#[tokio::test]
async fn non_sudo_command_takes_the_plain_path() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let (id, result) = run_in_shell_for_test(
        &reg,
        Owner::Chat("plain-path".into()),
        "echo PLAIN_ECHO_UNCHANGED",
        5,
        None,
        None,
        spawn_in_logs_dir,
    )
    .await
    .expect("the plain command resolves");
    let record_started_unix = with_shells(&reg, |store| {
        store.get(id.as_str()).map(|r| r.started_unix).unwrap_or(0)
    });
    let cast = crate::shell::resolve_cast(id.as_str(), record_started_unix);
    let meta = MetaIndex::load(&meta_path_for(&cast)).expect("the one plain command was indexed");
    let record = meta
        .commands
        .iter()
        .find(|r| r.command == "echo PLAIN_ECHO_UNCHANGED")
        .unwrap_or_else(|| {
            panic!(
                "the plain command must be indexed exactly as written, indexed: {:?}",
                meta.commands
                    .iter()
                    .map(|r| r.command.as_str())
                    .collect::<Vec<_>>()
            )
        });
    assert_eq!(record.exit_code, 0, "the plain command really ran");
    assert!(
        result.text.contains("PLAIN_ECHO_UNCHANGED"),
        "the plain path's output reaches the model: {:?}",
        result.text
    );
}

/// The full producer path: one command through `run_in_shell_with` ends up
/// as a `CommandRecord` in the shell's `.meta.json`, with the command's
/// text and its **real** exit code (3 here, so the assertion cannot pass on
/// a defaulted 0).
#[tokio::test]
async fn a_run_command_appends_a_meta_index_record() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let (id, result) = run_in_shell_for_test(
        &reg,
        Owner::Chat("index-producer".into()),
        "printf 'SENTINEL_ONE\\n'; sh -c 'exit 3'",
        5,
        None,
        None,
        spawn_in_logs_dir,
    )
    .await
    .expect("the command resolves");
    assert_eq!(result.exit_code, 3, "sanity: the command really failed");

    let record_started_unix = with_shells(&reg, |store| {
        store.get(id.as_str()).map(|r| r.started_unix).unwrap_or(0)
    });
    let cast = crate::shell::resolve_cast(id.as_str(), record_started_unix);
    let meta_path = meta_path_for(&cast);
    let meta = MetaIndex::load(&meta_path)
        .unwrap_or_else(|e| panic!("the command was indexed at {meta_path:?}: {e}"));
    assert_eq!(meta.commands.len(), 1, "exactly one command indexed");
    let record = &meta.commands[0];
    assert_eq!(record.index, 0, "the first recorded command is index 0");
    assert_eq!(record.command, "printf 'SENTINEL_ONE\\n'; sh -c 'exit 3'");
    assert_eq!(record.exit_code, 3, "the real exit code is recorded");
    assert_eq!(
        meta.owner.as_deref(),
        Some("chat:index-producer"),
        "the index records who owned the shell"
    );
    assert_eq!(meta.host.as_deref(), Some("local"), "and where it ran");
    assert!(
        record.first_byte < record.end_byte,
        "a non-empty clean range is stored: {record:?}"
    );
    let bytes = std::fs::read(&cast).expect("the cast is on disk");
    let start = record.first_byte as usize;
    // Decoded through the production reader: the stored range, read the
    // way `read_command_output` reads it, carries no marker text. (Raw
    // cast bytes are newline-delimited JSON, so a raw-byte check at the
    // boundary can only ever see `[` — it cannot discriminate anything.)
    let decoded = {
        let mut out = Vec::new();
        for line in String::from_utf8_lossy(&bytes[start..record.end_byte as usize]).lines() {
            crate::shell::extend_output_line(Some(line.to_owned()), &mut out);
        }
        out
    };
    assert!(
        !String::from_utf8_lossy(&decoded).contains("DE_"),
        "the stored range must not carry marker text: {record:?}"
    );
}

#[path = "shell_exec_hosted_tests.rs"]
mod hosted;

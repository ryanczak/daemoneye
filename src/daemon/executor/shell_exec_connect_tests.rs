use super::*;
use std::sync::Arc;

/// Serialises this module's PTY-spawning tests against each other. Each
/// test also holds `test_home_guard()` next to this lock, so a
/// HOME-mutating test cannot delete `HOME` out from under one of these
/// spawns (a spawn into a deleted `HOME` fails with `ENOENT`).
static PTY_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The credential the "attached" transport answers with; the fake ssh
/// checks for exactly this before declaring the login good.
const CREDENTIAL: &str = "right-pw";
/// The prompt id the round trip echoes; the fake never sees it.
const PROMPT_ID: &str = "c";

/// The "attached" write half: records every write (the credential prompt
/// goes here, not the PTY) so a test can count prompts.
struct RecordingWriter {
    writes: Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
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

/// The "attached" read half: `answers` `CredentialResponse` lines over a
/// `Cursor`, answered in order. Distinct credentials per test pass a
/// custom list.
fn answered_reader(credentials: &[&str]) -> impl tokio::io::AsyncBufRead + Unpin {
    let lines = credentials
        .iter()
        .map(|c| {
            let mut line = serde_json::to_string(&Request::CredentialResponse {
                id: PROMPT_ID.to_string(),
                credential: (*c).to_string(),
            })
            .expect("CredentialResponse serialises");
            line.push('\n');
            line
        })
        .collect::<Vec<_>>()
        .join("");
    std::io::Cursor::new(lines.into_bytes())
}

/// Count the credential prompts the transport saw.
fn prompt_count(writes: &Arc<std::sync::Mutex<Vec<Vec<u8>>>>) -> usize {
    writes
        .lock()
        .expect("write log lock")
        .iter()
        .filter(|w| {
            serde_json::from_slice::<Response>(w)
                .is_ok_and(|r| matches!(r, Response::CredentialPrompt { .. }))
        })
        .count()
}

/// Spawn the fake ssh (`sh -c script`) in a PTY through [`drive_connect`]
/// with the given transport halves, and time it. `connected` = the flag
/// file exists.
async fn drive_fake(
    script: &str,
    flag: &std::path::Path,
    tx: &mut (impl tokio::io::AsyncWriteExt + Unpin),
    rx: &mut (impl tokio::io::AsyncBufReadExt + Unpin),
) -> Result<(), String> {
    let flag_path = flag.to_path_buf();
    let argv = vec!["-c".to_string(), script.to_string()];
    drive_connect(
        "/bin/sh",
        &argv,
        || flag_path.exists(),
        "pinky",
        PROMPT_ID,
        tx,
        rx,
        Duration::from_secs(10),
    )
    .await
}

/// bug-08e-1's regression: the deadline is measured from the **last
/// answer**, not from spawn — a prompt answered after ~0.4 s that
/// connects ~0.5 s later is inside a 700 ms window of the answer, so a
/// user who took their time typing the credential still connects. The
/// never-answered half is pinned in the same test: a fake that says
/// nothing fails with the `did not connect within` wording.
#[tokio::test]
async fn an_answered_prompt_restarts_the_connect_timeout() {
    let _pty = PTY_TEST_LOCK.lock().await;
    let _home = crate::test_home_guard();
    let tmp = tempfile::tempdir().expect("tempdir");
    let flag = tmp.path().join("connected");
    let script = format!(
        "sleep 0.4; printf \"u@h's password: \"; read -r pw; sleep 0.5; \
             [ \"$pw\" = {CREDENTIAL} ] && touch {} && exit 0; exit 255",
        flag.display()
    );
    let writes = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut writer = RecordingWriter {
        writes: Arc::clone(&writes),
    };
    let mut reader = answered_reader(&[CREDENTIAL]);
    let flag_path = flag.to_path_buf();
    let argv = vec!["-c".to_string(), script.clone()];
    let outcome = drive_connect(
        "/bin/sh",
        &argv,
        || flag_path.exists(),
        "pinky",
        PROMPT_ID,
        &mut writer,
        &mut reader,
        Duration::from_millis(700),
    )
    .await;
    assert_eq!(
        outcome,
        Ok(()),
        "a login that finishes within the timeout of its answer succeeds: {outcome:?}"
    );
    assert!(flag.exists(), "the fake connected: {outcome:?}");

    // The never-answered half: no prompt, no answer, only sleep — the
    // deadline measured from spawn still fires with the pinned wording.
    let flag2 = tmp.path().join("never");
    let script2 = format!("sleep 2; touch {}; exit 0", flag2.display());
    let flag2_path = flag2.to_path_buf();
    let argv2 = vec!["-c".to_string(), script2];
    let started = std::time::Instant::now();
    let silent = drive_connect(
        "/bin/sh",
        &argv2,
        || flag2_path.exists(),
        "pinky",
        PROMPT_ID,
        &mut writer,
        &mut reader,
        Duration::from_millis(300),
    )
    .await;
    let msg = silent.expect_err("the silent fake must time out: {silent:?}");
    assert!(
        msg.contains("did not connect within 300ms"),
        "the never-answered deadline still fires from spawn: {msg:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the timeout kills the child at the deadline, not later: {msg:?}"
    );
}

/// Test 2: a host whose ssh asks nothing connects with no prompt round
/// trip at all — the connect step is transparent on key auth.
#[tokio::test]
async fn a_host_that_needs_no_credential_connects_without_a_prompt() {
    let _pty = PTY_TEST_LOCK.lock().await;
    let _home = crate::test_home_guard();
    let tmp = tempfile::tempdir().expect("tempdir");
    let flag = tmp.path().join("connected");
    let script = format!("sleep 0.2; touch {}; exit 0", flag.display());
    let writes = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut writer = RecordingWriter {
        writes: Arc::clone(&writes),
    };
    let mut reader = answered_reader(&[CREDENTIAL]);
    let outcome = drive_fake(&script, &flag, &mut writer, &mut reader).await;
    assert_eq!(outcome, Ok(()), "the fake connects: {outcome:?}");
    assert_eq!(
        prompt_count(&writes),
        0,
        "no prompt is asked when ssh connects without one"
    );
}

/// Test 3: the password prompt is answered once and the host connects —
/// the credential reaches the child's PTY and never the transport log.
#[tokio::test]
async fn a_password_prompt_is_answered_once_and_the_host_connects() {
    let _pty = PTY_TEST_LOCK.lock().await;
    let _home = crate::test_home_guard();
    let tmp = tempfile::tempdir().expect("tempdir");
    let flag = tmp.path().join("connected");
    // The `sleep 0.3` after the read keeps the child alive once the
    // answer is in — ssh's post-answer authentication latency — so the
    // classify-only-after-the-last-answer window (08f) is observable:
    // bug-08e-3's Mutation A re-applied turns this red (the stale prompt
    // is re-answered against the exhausted transport).
    let script = format!(
        "sleep 0.2; printf \"u@h's password: \"; read -r pw; sleep 0.3; \
             [ \"$pw\" = {CREDENTIAL} ] && touch {} && exit 0; exit 255",
        flag.display()
    );
    let writes = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut writer = RecordingWriter {
        writes: Arc::clone(&writes),
    };
    let mut reader = answered_reader(&[CREDENTIAL]);
    let outcome = drive_fake(&script, &flag, &mut writer, &mut reader).await;
    assert_eq!(outcome, Ok(()), "the fake connects: {outcome:?}");
    assert_eq!(
        prompt_count(&writes),
        1,
        "exactly one credential prompt was written: {:?}",
        writes.lock().expect("write log lock")
    );
}

/// Test 4: a wrong password is asked again, and the retry chunk — which
/// carries both `Permission denied` **and** a fresh prompt — does not
/// abort the login (the process-exit rule, not a text match).
#[tokio::test]
async fn a_wrong_password_is_asked_again_and_the_retry_line_does_not_abort() {
    let _pty = PTY_TEST_LOCK.lock().await;
    let _home = crate::test_home_guard();
    let tmp = tempfile::tempdir().expect("tempdir");
    let flag = tmp.path().join("connected");
    let script = format!(
        "sleep 0.2; printf \"u@h's password: \"; read -r pw; \
             if [ \"$pw\" = {CREDENTIAL} ]; then touch {}; exit 0; fi; \
             printf \"Permission denied, please try again.\\r\\nu@h2's password: \"; \
             read -r pw; \
             if [ \"$pw\" = {CREDENTIAL} ]; then touch {}; exit 0; fi; exit 255",
        flag.display(),
        flag.display()
    );
    let writes = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut writer = RecordingWriter {
        writes: Arc::clone(&writes),
    };
    let mut reader = answered_reader(&["wrong-pw", CREDENTIAL]);
    let outcome = drive_fake(&script, &flag, &mut writer, &mut reader).await;
    assert_eq!(outcome, Ok(()), "the retry connects: {outcome:?}");
    assert_eq!(
        prompt_count(&writes),
        2,
        "the wrong answer was asked again: {:?}",
        writes.lock().expect("write log lock")
    );
}

/// Test 5: nobody attached — the round trip gives up at once with 08f's
/// sentence, well inside any plausible timeout, and nothing is left
/// running.
#[tokio::test]
async fn an_unanswerable_connection_prompt_fails_fast_and_leaves_nothing_running() {
    let _pty = PTY_TEST_LOCK.lock().await;
    let _home = crate::test_home_guard();
    let tmp = tempfile::tempdir().expect("tempdir");
    let flag = tmp.path().join("connected");
    let script = format!(
        "sleep 0.2; printf \"u@h's password: \"; read -r pw; \
             [ \"$pw\" = {CREDENTIAL} ] && touch {} && exit 0; exit 255",
        flag.display()
    );
    let started = std::time::Instant::now();
    let outcome = drive_fake(
        &script,
        &flag,
        &mut tokio::io::sink(),
        &mut tokio::io::BufReader::new(tokio::io::empty()),
    )
    .await;
    let elapsed = started.elapsed();
    let msg = outcome.expect_err("the unanswerable prompt must fail: {outcome:?}");
    assert!(
        msg.contains("nobody is attached"),
        "the give-up names the missing reader: {msg:?}"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "the failure is fast, not a timeout: {elapsed:?}"
    );
    assert!(
        !flag.exists(),
        "the fake never reached its connected state: {msg:?}"
    );
}

/// Test 6: ssh's unknown-host-key question is refused — never answered
/// with a credential, and the error names the host.
#[tokio::test]
async fn an_unknown_host_key_is_refused() {
    let _pty = PTY_TEST_LOCK.lock().await;
    let _home = crate::test_home_guard();
    let tmp = tempfile::tempdir().expect("tempdir");
    let flag = tmp.path().join("connected");
    let script = "sleep 0.2; printf \"Are you sure you want to continue connecting \
                      (yes/no/[fingerprint])? \"; read -r a; exit 255"
        .to_string();
    let writes = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut writer = RecordingWriter {
        writes: Arc::clone(&writes),
    };
    let mut reader = answered_reader(&[CREDENTIAL]);
    let outcome = drive_fake(&script, &flag, &mut writer, &mut reader).await;
    let msg = outcome.expect_err("the host-key question must refuse: {outcome:?}");
    assert!(
        msg.contains("unknown host key") && msg.contains("pinky"),
        "the refusal names the host and the question: {msg:?}"
    );
    assert_eq!(
        prompt_count(&writes),
        0,
        "the host-key question is never answered with a credential"
    );
    assert!(!flag.exists(), "the fake never connected");
}

/// Live test 7: the configured host `pinky` — cold, the connect step
/// answers nothing and still connects; warm, the second call is a no-op
/// in milliseconds. Requires `pinky` reachable over ssh; ignored by
/// default, run with `-- --ignored --nocapture`.
#[tokio::test]
#[ignore = "requires the configured host `pinky` reachable over ssh"]
async fn a_cold_host_connects_first_and_a_warm_one_is_reused_on_pinky() {
    let (_home, dir) = crate::test_home();
    std::fs::write(
        dir.path().join(".daemoneye/etc/config.toml"),
        "[models.default]\nprovider = \"ollama\"\n\n[hosts.pinky]\nssh = \"pinky\"\n",
    )
    .expect("write config");
    let control = crate::config::ssh_dir();
    std::fs::create_dir_all(&control).expect("ssh dir");
    let entry = crate::shell::host_entry("pinky").expect("pinky is configured");
    let mut sink = tokio::io::sink();
    let mut eof = tokio::io::BufReader::new(tokio::io::empty());

    let cold_started = std::time::Instant::now();
    let cold = ensure_connected("pinky", false, PROMPT_ID, &mut sink, &mut eof).await;
    let cold_elapsed = cold_started.elapsed();
    println!("cold connect elapsed: {cold_elapsed:?}");
    assert_eq!(cold, Ok(()), "the cold connect succeeds: {cold:?}");

    let warm = master_running(&entry, &control);
    assert!(warm, "the master survives the connect step");

    let warm_started = std::time::Instant::now();
    let second = ensure_connected("pinky", false, PROMPT_ID, &mut sink, &mut eof).await;
    let warm_elapsed = warm_started.elapsed();
    println!("warm check elapsed: {warm_elapsed:?}");
    assert_eq!(second, Ok(()), "the warm check succeeds: {second:?}");
    assert!(
        warm_elapsed < Duration::from_millis(100),
        "the warm check is a no-op, not a fresh connect: {warm_elapsed:?}"
    );

    let _ = std::process::Command::new("ssh")
        .args([
            "-O",
            "exit",
            "-o",
            &format!("ControlPath={}/%C", control.display()),
            "pinky",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

// ── M26 phase-06b: the marker comes from the record ─────────────────────

use super::tests::{FishShellEnv, spawn_in_process_host};

/// A sandbox shell targeted by id gets the POSIX marker (`"sh"` → `$?`),
/// **not** the daemon's `$SHELL`. With the daemon's `$SHELL` fish, the
/// marker would ask for `$status`, which the sandbox's bash expands to
/// nothing, so the exit code never parses. Regression guard for phase-06b
/// Task 2: reverting to `marker_shell_name(&record_host)` (the host's
/// login shell, `/usr/bin/fish` here) makes this test time out.
#[tokio::test]
async fn a_sandbox_shell_targeted_by_id_uses_the_posix_marker() {
    let _pty = PTY_TEST_LOCK.lock().await;
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let owner = Owner::Chat("sandbox-marker".into());

    // A live sandbox shell: the record carries `sandbox`, its host is
    // `Local`. The in-process host runs a real `bash` behind the socket,
    // which stands in for the VM's bash — the marker protocol is the same.
    let id = crate::shell::spawn_for_in_sandbox(
        &reg,
        owner.clone(),
        5,
        "de-ghost-t".into(),
        false,
        spawn_in_process_host,
    )
    .await
    .expect("the sandbox shell's host comes up");
    crate::shell::await_ready(
        &socket_path(id.as_str()),
        std::time::Instant::now() + Duration::from_secs(15),
    )
    .await
    .expect("the sandbox shell's host must be alive, or the marker is not under test");

    // The daemon's `$SHELL` is fish for the duration of the call; the
    // guard restores it even when an assertion below fails.
    let _fish = FishShellEnv::set();

    let outcome = run_in_shell_with_prompts(
        &reg,
        owner,
        "sh -c 'exit 3'",
        5,
        Some(id.as_str()),
        Some(10),
        "",
        &mut tokio::io::sink(),
        &mut tokio::io::empty(),
        crate::host::HostId::Local,
        |_id: ShellId| async move { Ok(()) },
    )
    .await;

    // Drop the guard before asserting: an async-drop tail would be a
    // surprise, and the restore must not depend on the assertions.
    drop(_fish);

    let (_id, result) =
        outcome.expect("the POSIX marker must parse the exit code out of a sandbox shell");
    assert_eq!(
        result.exit_code, 3,
        "the sandbox's bash must report its child's exit code through the \
             POSIX marker: {result:?}"
    );
}

use super::*;
use crate::shell::proto::ShellSignal;
use core::num::NonZeroU16;
use std::time::Duration;

/// A unique cast path under a fresh temp dir, per test.
fn cast_path(dir: &tempfile::TempDir) -> std::path::PathBuf {
    dir.path().join("s9-0000000000-test.cast")
}

fn spawn_backend(dir: &tempfile::TempDir) -> Arc<PtyBackend> {
    let _home = crate::test_home_guard();
    PtyBackend::spawn("bash", &[], 24, 80, 100, &cast_path(dir), 0).expect("bash available")
}

fn recv_until(
    rx: &mut broadcast::Receiver<Vec<u8>>,
    deadline: std::time::Instant,
    mut pred: impl FnMut(&[u8]) -> bool,
) -> Option<Vec<u8>> {
    loop {
        if std::time::Instant::now() >= deadline {
            return None;
        }
        match rx.try_recv() {
            Ok(chunk) => {
                if pred(&chunk) {
                    return Some(chunk);
                }
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {
                // A chunk we never got could have carried the marker, but
                // the marker's *output* chunk — the one the command's own
                // stdout produced — is emitted later, so keep going; a
                // lagged subscriber is exactly the pump-survival case the
                // lagged test pins. Dropping out here would make this
                // fan-out test racy with channel capacity.
                continue;
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Closed) => return None,
        }
    }
}

/// Block only long enough to drain whatever output is in flight: return
/// once `try_recv` has come up empty for a short quiet period, so a
/// caller drains the current burst without waiting out the deadline.
fn quiet(rx: &mut broadcast::Receiver<Vec<u8>>, deadline: std::time::Instant) {
    let mut silent_since: Option<std::time::Instant> = None;
    loop {
        if std::time::Instant::now() >= deadline {
            return;
        }
        match rx.try_recv() {
            Ok(_) => silent_since = None,
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                let now = std::time::Instant::now();
                match silent_since {
                    None => silent_since = Some(now),
                    Some(t) if now.duration_since(t) >= Duration::from_millis(30) => return,
                    Some(_) => {}
                }
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) | Err(_) => {
                silent_since = None;
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Join the pump's output to raw bytes for grep-ability; also proves the
/// cast contains an `"o"` event carrying the bytes (the fan-out guarantee).
fn cast_output(_backend: &PtyBackend, dir: &tempfile::TempDir) -> String {
    std::fs::read_to_string(cast_path(dir)).expect("cast log readable on disk")
}

/// True if some `"o"` event payload contains `needle`.
fn cast_has_o_text(contents: &str, needle: &str) -> bool {
    for line in contents.lines().skip(1) {
        if !line.starts_with('[') {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(arr) = v.as_array() else {
            continue;
        };
        if arr.len() == 3
            && arr[1].as_str() == Some("o")
            && arr[2].as_str().is_some_and(|s| s.contains(needle))
        {
            return true;
        }
    }
    false
}

/// Assert `needle` appears in a cast-generated `"o"` event payload.
fn assert_cast_has_o(contents: &str, needle: &str, label: &str) {
    for line in contents.lines().skip(1) {
        if !line.starts_with('[') {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(line).unwrap_or(serde_json::Value::Null);
        let Some(arr) = v.as_array() else {
            continue;
        };
        if arr.len() == 3 && arr[1].as_str() == Some("o") {
            let Some(s) = arr[2].as_str() else {
                continue;
            };
            if s.contains(needle) {
                return;
            }
        }
    }
    panic!("{label}: expected an \"o\" event containing {needle:?} in cast:\n{contents}");
}

#[test]
fn backend_status_reports_raw_mode_under_stty_raw() {
    let dir = tempfile::tempdir().unwrap();
    let backend = spawn_backend(&dir);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    // Never assert on an idle shell: an interactive bash's readline
    // clears ICANON while it waits at the prompt, so an idle shell
    // reads Some(true) — correct, and the reason D3 only consults
    // raw_mode when the state is Running. A plain `sleep` keeps the
    // line discipline the shell set up: canonical.
    rt.block_on(backend.input(b"sleep 5\r\n"))
        .expect("write the sleep");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let raw = match rt.block_on(backend.status()) {
            ShellResponse::Status { raw_mode, .. } => raw_mode,
            other => panic!("expected a Status frame, got {other:?}"),
        };
        if raw == Some(false) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the shell to be canonical while a plain sleep runs within 10 s, last {raw:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    // `stty raw` switches the slave's line discipline off; the master's
    // tcgetattr then reads ICANON clear. The command restores sane mode
    // itself after the sleep: polled for below, since the tty's state
    // after `stty sane` depends on what bash's readline does next, and
    // that is not what this test pins.
    rt.block_on(backend.input(b"stty raw -echo; sleep 2; stty sane\r\n"))
        .expect("write the stty command");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let raw = match rt.block_on(backend.status()) {
            ShellResponse::Status { raw_mode, .. } => raw_mode,
            other => panic!("expected a Status frame, got {other:?}"),
        };
        if raw == Some(true) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the shell to be in raw mode within 10 s of `stty raw`, last {raw:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn backend_status_names_the_foreground_command() {
    let dir = tempfile::tempdir().unwrap();
    let backend = spawn_backend(&dir);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    // A fresh idle bash holds its own terminal.
    let idle = rt.block_on(backend.status());
    match &idle {
        ShellResponse::Status { foreground, .. } => assert_eq!(
            *foreground, None,
            "an idle shell's foreground group is the shell itself, got {idle:?}"
        ),
        other => panic!("expected a Status frame, got {other:?}"),
    }

    rt.block_on(backend.input(b"sleep 30\r\n"))
        .expect("write the sleep command");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let fg = match rt.block_on(backend.status()) {
            ShellResponse::Status { foreground, .. } => foreground,
            other => panic!("expected a Status frame, got {other:?}"),
        };
        if fg.as_deref() == Some("sleep") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the foreground group's leader to be named `sleep` within 10 s, last {fg:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    rt.block_on(backend.input(b"\x03"))
        .expect("end the sleep with ^C");
}

#[test]
fn backend_screen_text_marks_inverse_and_reports_alt_screen() {
    let dir = tempfile::tempdir().unwrap();
    let backend = spawn_backend(&dir);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(
        backend.input(b"printf '\\033[?1049h\\033[7mSELECTED\\033[0m plain\\n'; sleep 30\r\n"),
    )
    .expect("write the marking command");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let resp = rt
            .block_on(backend.screen_text(false, 0))
            .expect("screen_text");
        let ShellResponse::ScreenText {
            text, alt_screen, ..
        } = resp
        else {
            panic!("expected ScreenText, got {resp:?}");
        };
        if alt_screen && text.contains("[SEL: SELECTED]") {
            assert!(
                text.contains("plain"),
                "the unmarked part must survive: {text:?}"
            );
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the screen to show the inverse run and the alt screen within 10 s, last: {text:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    rt.block_on(backend.input(b"\x03"))
        .expect("end the sleep with ^C");
}

#[test]
fn backend_screen_text_reports_the_key_modes() {
    let dir = tempfile::tempdir().unwrap();
    let backend = spawn_backend(&dir);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(backend.input(b"printf '\\033[?2004h'; sleep 30\r\n"))
        .expect("write the mode command");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let resp = rt
            .block_on(backend.screen_text(false, 0))
            .expect("screen_text");
        let ShellResponse::ScreenText {
            bracketed_paste,
            application_cursor,
            ..
        } = resp
        else {
            panic!("expected ScreenText, got {resp:?}");
        };
        if bracketed_paste {
            assert!(
                !application_cursor,
                "only `?2004h` was sent; application cursor must stay off"
            );
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the ScreenText reply to report bracketed paste within 10 s"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    rt.block_on(backend.input(b"\x03"))
        .expect("end the sleep with ^C");
}

#[test]
fn backend_screen_text_prepends_scrollback_rows() {
    let dir = tempfile::tempdir().unwrap();
    let backend = spawn_backend(&dir);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(backend.input(b"for i in 1 2 3 4 5 6 7 8; do echo line$i; done\r\n"))
        .expect("write the loop");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let text = loop {
        let resp = rt
            .block_on(backend.screen_text(false, 100))
            .expect("screen_text");
        let ShellResponse::ScreenText { text, .. } = resp else {
            panic!("expected ScreenText");
        };
        if text.contains("line1") && text.contains("line8") {
            break text;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the frame to hold scrollback and screen within 10 s, last: {text:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(
        text.contains(
            "line1
line2"
        ),
        "the scrollback head must be prepended oldest first: {text:?}"
    );
    assert!(
        text.contains("line8"),
        "the viewport must still be there: {text:?}"
    );
}

#[test]
fn backend_fans_one_chunk_out_to_subscriber_log_and_screen() {
    let dir = tempfile::tempdir().unwrap();
    let backend = spawn_backend(&dir);
    let mut rx = backend.subscribe();

    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    // A uniquely-marked command so the assertion binds to this run's
    // output, not to the initial prompt. The marker is split with `''`
    // so the typed-command echo cannot forge it: the echo shows
    // `FAN''OUT_51_99`, only the actual output holds the contiguous
    // `FANOUT_51_99`.
    let marker = "FANOUT_51_99";
    let cmd = "printf '\\nFAN''OUT_51_99 out\\n'; sh -c 'exit 3'\n".to_string();
    tokio::runtime::Runtime::new()
        .expect("runtime")
        .block_on(backend.input(cmd.as_bytes()))
        .expect("write command");

    // 1. the subscriber receives the bytes
    let got = recv_until(&mut rx, deadline, |c| {
        c.windows(marker.len()).any(|w| w == marker.as_bytes())
    })
    .expect("subscriber must receive the marked output");
    let got_str = String::from_utf8_lossy(&got);
    assert!(
        got_str.contains(marker),
        "subscriber must receive the marker bytes: {got_str:?}"
    );

    // 2. the cast file contains them as an "o" event. The pump writes
    //    subscribers → log → screen per chunk, so the marker reaching
    //    the subscriber does not (yet) prove it reached the log — poll
    //    the cast to a bounded deadline, the way step 3 polls the screen.
    let cast_deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut cast;
    loop {
        cast = cast_output(&backend, &dir);
        if cast_has_o_text(&cast, marker) {
            break;
        }
        if std::time::Instant::now() >= cast_deadline {
            assert_cast_has_o(&cast, marker, "cast");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_cast_has_o(&cast, marker, "cast");

    // 3. status()'s screen reflects them: poll until the marker is
    //    visible on the live screen.
    let screen_deadline = std::time::Instant::now() + Duration::from_secs(10);
    let visible = loop {
        if backend.screen_contents().contains(marker) {
            break true;
        }
        if std::time::Instant::now() >= screen_deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(
        visible,
        "screen must show the fan-out marker, got: {:?}",
        backend.screen_contents()
    );
}

#[test]
fn backend_records_input_as_an_i_event() {
    let dir = tempfile::tempdir().unwrap();
    let backend = spawn_backend(&dir);
    let payload = "printf 'input-test\\n'\n";
    tokio::runtime::Runtime::new()
        .expect("runtime")
        .block_on(backend.input(payload.as_bytes()))
        .expect("write command");
    // Let the pump drain the fragment; the "i" event is written
    // synchronously by input(), before anything streams.
    std::thread::sleep(Duration::from_millis(50));
    let cast = cast_output(&backend, &dir);
    let found = cast.lines().skip(1).any(|line| {
        if !line.starts_with('[') {
            return false;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            return false;
        };
        v.as_array()
            .is_some_and(|a| a.len() == 3 && a[1].as_str() == Some("i"))
            && v.as_array().unwrap()[2]
                .as_str()
                .unwrap_or_default()
                .contains("input-test")
    });
    assert!(
        found,
        "cast must contain an \"i\" event for the input: {cast}"
    );
}

#[test]
fn looks_like_secret_prompt_matches_the_real_sudo_prompt() {
    assert!(
        looks_like_secret_prompt("[sudo] password for matt: "),
        "the real sudo prompt must arm the scrub"
    );
    for variant in ["Password:", "password:", "PASSWORD:"] {
        assert!(
            looks_like_secret_prompt(variant),
            "{variant} must arm the scrub — the case fold is load-bearing"
        );
    }
    assert!(
        looks_like_secret_prompt("Enter passphrase for key '/home/m/.ssh/id': "),
        "ssh's key-unlock prompt must arm the scrub"
    );
    assert!(
        looks_like_secret_prompt("de-sudo-prompt-0:"),
        "the 01a sentinel shape must arm the scrub"
    );
    assert!(
        looks_like_secret_prompt("Two-factor verification code: "),
        "a 2FA prompt must arm the scrub"
    );
    assert!(
        looks_like_secret_prompt("Enter OTP:"),
        "an otp prompt must arm the scrub"
    );
}

#[test]
fn looks_like_secret_prompt_ignores_ordinary_output() {
    assert!(
        !looks_like_secret_prompt("total 40\ndrwxr-xr-x 2 matt matt"),
        "ordinary listing output must not arm the scrub"
    );
    assert!(
        !looks_like_secret_prompt("12 files changed, 240 insertions(+)"),
        "git-style summary output must not arm the scrub"
    );
}

#[test]
fn an_ssh_password_prompt_arms_the_scrub() {
    let password = "matt@pinky.home.planetfoo.org's password: ";
    assert!(
        looks_like_secret_prompt(password),
        "the measured ssh login prompt must arm the scrub"
    );
    let passphrase = "Enter passphrase for key '/home/matt/.ssh/id_ed25519': ";
    assert!(
        looks_like_secret_prompt(passphrase),
        "the ssh key-passphrase prompt must arm the scrub"
    );
}

#[test]
fn the_host_key_question_does_not_arm_the_scrub() {
    let snap = "The authenticity of host 'pinky.home.planetfoo.org (172.16.50.75)' \
                    can't be established.\r\nED25519 key fingerprint is: \
                    SHA256:8H2tFe0tLZodJlVr7s88lqZFVQTt6eo4XiGLGm1q4GI\r\n\
                    This key is not known by any other names.\r\nAre you sure you want to \
                    continue connecting (yes/no/[fingerprint])? ";
    assert!(
        !looks_like_secret_prompt(snap),
        "the host-key question names no secret; its `yes` answer belongs in the log \
             in clear"
    );
}

#[test]
fn input_after_a_prompt_logs_a_redaction_marker() {
    const CREDENTIAL: &str = "hunter2-secret-test";
    let dir = tempfile::tempdir().unwrap();
    let backend = spawn_backend(&dir);
    let mut rx = backend.subscribe();
    quiet(&mut rx, std::time::Instant::now() + Duration::from_secs(10));

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    // `read -rs` is the fixture that owns the echo window the way a real
    // password prompt does: `-s` silences terminal echo around the
    // answer, so readline's prompt redraw cannot reflect the credential
    // back as an "o" event. The `''` splits keep the echoed command line
    // from arming the scrub (see `looks_like_secret_prompt`), so the flag
    // arms on `read`'s own prompt, printed after echo is off. `got N` in
    // the output marks the settled cast to poll for. Full reasoning:
    // `the_flag_clears_once_the_answer_is_submitted`.
    let prompt = "read -rs -p '[de-sudo-prompt''-77] Pass''word: ' de_secret; printf '\\n'; echo \"got ${#de_secret}\"\n";
    rt.block_on(backend.input(prompt.as_bytes()))
        .expect("emit prompt");
    // The pump is a thread; poll rather than sleep on a guess.
    for _ in 0..500 {
        if backend.awaiting_secret.load(Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        backend.awaiting_secret.load(Ordering::SeqCst),
        "the pump must have armed the scrub after the prompt chunk"
    );

    // The answer reaching the shell must never reach the cast file.
    rt.block_on(backend.input(format!("{CREDENTIAL}\n").as_bytes()))
        .expect("submit the credential");
    let done = format!("got {}", CREDENTIAL.len());
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let cast = loop {
        let cast = cast_output(&backend, &dir);
        if cast.contains("[redacted]") && cast_has_o_text(&cast, &done) {
            break cast;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "`[redacted]` and `{done}` never both reached the cast: {cast}"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(
        cast.contains("[redacted]"),
        "the marker must exist in the cast: {cast}"
    );
    assert!(
        !cast.contains(CREDENTIAL),
        "the credential must never reach the cast file: {cast}"
    );
}

#[test]
fn input_without_a_prompt_logs_the_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let backend = spawn_backend(&dir);
    assert!(
        !backend.awaiting_secret.load(Ordering::SeqCst),
        "a fresh shell must start unarmed"
    );
    let payload = "printf 'no-prompt-e2e-probe\\n'\n";
    tokio::runtime::Runtime::new()
        .expect("runtime")
        .block_on(backend.input(payload.as_bytes()))
        .expect("write command");
    // input() writes its "i" event synchronously.
    std::thread::sleep(Duration::from_millis(50));
    let cast = cast_output(&backend, &dir);
    assert!(
        cast.contains("no-prompt-e2e-probe"),
        "an unarmed input must be logged verbatim: {cast}"
    );
}

#[test]
fn ctrl_c_ends_secret_entry() {
    assert!(ends_secret_entry(b"x\n"), "a newline submits the answer");
    assert!(
        ends_secret_entry(b"x\r"),
        "a carriage return submits the answer"
    );
    assert!(ends_secret_entry(b"\x03"), "Ctrl-C cancels the prompt");
    assert!(
        !ends_secret_entry(b"abc"),
        "ordinary characters keep the scrub armed"
    );
    assert!(!ends_secret_entry(b""), "nothing ends what has not begun");
}

#[test]
fn the_flag_clears_once_the_answer_is_submitted() {
    const CREDENTIAL: &str = "hunter2-clear-target";
    let dir = tempfile::tempdir().unwrap();
    let backend = spawn_backend(&dir);
    let mut rx = backend.subscribe();
    quiet(&mut rx, std::time::Instant::now() + Duration::from_secs(10));
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    // `read -s` is the fixture that silences terminal echo *around the
    // answer*, the same window real sudo owns for its prompt — without
    // it readline's prompt-redraw flush reflects the answer back as an
    // "o" event and the whole-cast assertion below can never hold. What
    // the scrub itself must do (log `[redacted]`, clear on `\n`) is
    // exercised here; what the *shell program* must do (own the echo
    // window) is provided by the fixture. Both needles in the prompt are
    // split with `''` — the idiom `pty.rs::wrap_command` uses for the
    // marker — so the echoed command line matches neither `password` nor
    // `de-sudo-prompt-`; the flag can then arm only on the prompt that
    // `read -rs` itself emits, which it prints *after* echo is off.
    let echo = "read -rs -p '[de-sudo-prompt''-78] Pass''word: ' v; printf '\\n'\n";
    rt.block_on(backend.input(echo.as_bytes()))
        .expect("emit prompt");
    for _ in 0..500 {
        if backend.awaiting_secret.load(Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        backend.awaiting_secret.load(Ordering::SeqCst),
        "the flag must be armed before the answer is submitted"
    );

    rt.block_on(backend.input(format!("{CREDENTIAL}\n").as_bytes()))
        .expect("submit the answer");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if !backend.awaiting_secret.load(Ordering::SeqCst) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the flag must clear once the \\n-terminated answer is submitted"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    let followup_marker = "after-clear-probe";
    rt.block_on(backend.input(format!("echo {followup_marker}\n").as_bytes()))
        .expect("write the follow-up");
    std::thread::sleep(Duration::from_millis(50));
    let cast = cast_output(&backend, &dir);
    assert!(
        cast.contains(&format!(", \"i\", \"echo {followup_marker}")),
        "the follow-up input must be logged in clear after the scrub disarmed: {cast}"
    );
    // The credential must never appear anywhere in the cast file — the
    // whole-file negative the security property is written against.
    assert!(
        !cast.contains(CREDENTIAL),
        "the credential from the prompt phase must be absent from the whole cast: {cast}"
    );
}
#[test]
fn backend_streams_while_input_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let backend = spawn_backend(&dir);
    let mut rx = backend.subscribe();
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    // A command that streams output with a pause, so the second input is
    // written while the first command's output is still streaming.
    let command1 = "printf 'first-part\\n'; sleep 0.3; printf 'second-part\\n'\n";
    rt.block_on(backend.input(command1.as_bytes()))
        .expect("write command");

    // Wait for the stream to start, then write more input while it is
    // still streaming (before second-part appears). The marker is split
    // with `''` so the typed-command echo cannot forge it: the echo shows
    // `inter''leaved`, only the command's real output holds the
    // contiguous `interleaved`.
    recv_until(&mut rx, deadline, |c| {
        String::from_utf8_lossy(c).contains("first-part")
    })
    .expect("first frame must stream");
    let command2 = "printf 'inter''leaved\\n'\n";
    rt.block_on(backend.input(command2.as_bytes()))
        .expect("write while streaming");

    // Both the streaming command's tail and the interleaved input's
    // effect must reach the subscriber downstream.
    let got_second = recv_until(&mut rx, deadline, |c| {
        String::from_utf8_lossy(c).contains("second-part")
    })
    .expect("the streaming command's tail must not be lost");
    assert!(
        String::from_utf8_lossy(&got_second).contains("second-part"),
        "streaming tail reaches the subscriber"
    );
    let got_int = recv_until(
        &mut rx,
        std::time::Instant::now() + Duration::from_secs(10),
        |c| String::from_utf8_lossy(c).contains("interleaved"),
    )
    .expect("the interleaved input must reach the shell");
    assert!(
        String::from_utf8_lossy(&got_int).contains("interleaved"),
        "interleaved input effect reaches the subscriber"
    );
}

/// Assert that `rx` — a subscriber that read nothing during a flood —
/// reports `Lagged` on some read within a bounded wait, which is what the
/// pump's `continue` on `Lagged` exists to survive. The caller syncs on
/// the flood's own completion marker first, so by the time this runs the
/// subscriber is provably more than `STREAM_CAPACITY` chunks behind;
/// polling rather than a single `try_recv` keeps the read robust while a
/// late burst is still in flight.
fn assert_lagged(rx: &mut broadcast::Receiver<Vec<u8>>, label: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        match rx.try_recv() {
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(n)) => {
                assert!(n > 0, "{label} must report a nonzero lag");
                return;
            }
            Ok(_) | Err(_) => {
                if std::time::Instant::now() >= deadline {
                    panic!("{label}: never reported Lagged within the wait");
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

#[test]
fn backend_lagged_subscriber_does_not_break_the_pump() {
    let dir = tempfile::tempdir().unwrap();
    let backend = spawn_backend(&dir);
    let mut rx_slow = backend.subscribe();
    let mut rx_fast = backend.subscribe();

    // First, let the fast subscriber drain the initial prompt fragment.
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    quiet(&mut rx_fast, deadline);

    // Flood the shell with output so the slow subscriber — which never
    // reads — falls past STREAM_CAPACITY, while the fast subscriber is
    // drained continuously so it stays current.
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let flood = "printf 'FLOOD_START\\n'; yes | head -c 10000000\n";
    rt.block_on(backend.input(flood.as_bytes()))
        .expect("write flood command");

    // Drain the fast subscriber while the flood streams, so it never
    // lags; wait for the flood to settle back to the prompt.
    let _ = recv_until(&mut rx_fast, deadline, |c| {
        String::from_utf8_lossy(c).contains("FLOOD_START")
    });
    quiet(
        &mut rx_fast,
        std::time::Instant::now() + Duration::from_secs(20),
    );

    // The slow subscriber has been behind the whole flood: reading from
    // it now must report Lagged rather than silently drop or stall the
    // pump.
    assert_lagged(&mut rx_slow, "slow subscriber");

    // The fast subscriber still receives current chunks after the slow
    // one has lagged.
    let probe = "printf 'post-lag-probe\\n'\n";
    rt.block_on(backend.input(probe.as_bytes()))
        .expect("write probe");
    let got = recv_until(
        &mut rx_fast,
        std::time::Instant::now() + Duration::from_secs(20),
        |c| String::from_utf8_lossy(c).contains("post-lag-probe"),
    )
    .expect("fast subscriber must still receive current chunks");
    let got_str = String::from_utf8_lossy(&got);
    assert!(
        got_str.contains("post-lag-probe"),
        "fast subscriber must observe the probe output: {got_str:?}"
    );
}

#[test]
fn backend_resize_keeps_the_screen_contents() {
    let dir = tempfile::tempdir().unwrap();
    let backend = spawn_backend(&dir);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(backend.input(b"printf 'resize-marker\\n'\n"))
        .expect("input ok");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !backend.screen_contents().contains("resize-marker\n") {
        assert!(
            std::time::Instant::now() < deadline,
            "screen never showed resize-marker: {:?}",
            backend.screen_contents()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let before = rt.block_on(backend.status());
    match before {
        ShellResponse::Status { rows, cols, .. } => {
            assert_eq!((rows, cols), (24, 80), "initial dimensions");
        }
        other => panic!("expected status, got {other:?}"),
    }
    rt.block_on(backend.resize(40, 120)).expect("resize ok");
    let after = rt.block_on(backend.status());
    match after {
        ShellResponse::Status { rows, cols, .. } => {
            assert_eq!((rows, cols), (40, 120), "resized dimensions");
        }
        other => panic!("expected status, got {other:?}"),
    }
    assert!(
        backend.screen_contents().contains("resize-marker"),
        "the screen keeps its contents across a resize: {:?}",
        backend.screen_contents()
    );
}

#[test]
fn subscribe_with_screen_then_live_chunks_rebuild_the_screen() {
    let dir = tempfile::tempdir().unwrap();
    let backend = spawn_backend(&dir);
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    rt.block_on(backend.input(b"printf 'marker-one\\nmarker-two\\n'\n"))
        .expect("input ok");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !backend.screen_contents().contains("marker-two\n") {
        assert!(
            std::time::Instant::now() < deadline,
            "screen never showed marker-two: {:?}",
            backend.screen_contents()
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    let (snap, mut rx) = backend.subscribe_with_screen();
    let mut p = vt100::Parser::new(
        NonZeroU16::new(24).unwrap_or(NonZeroU16::MIN),
        NonZeroU16::new(80).unwrap_or(NonZeroU16::MIN),
        0,
    );
    p.process(&snap);

    rt.block_on(backend.input(b"printf 'marker-three\\n'\n"))
        .expect("input ok");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        while let Ok(chunk) = rx.try_recv() {
            p.process(&chunk);
        }
        let rebuilt = p.screen().contents();
        if backend.screen_contents().contains("marker-three\n")
            && rebuilt == backend.screen_contents()
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "rebuilt screen never matched\n--- rebuilt ---\n{rebuilt}\n--- host ---\n{}",
            backend.screen_contents()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        p.screen().contents(),
        backend.screen_contents(),
        "snapshot + live chunks rebuild the host's screen exactly"
    );
    assert!(
        p.screen().contents().contains("marker-one\n"),
        "the snapshot carried the pre-subscription screen"
    );

    rt.block_on(backend.signal(ShellSignal::Term)).ok();
}

#[test]
fn backend_signal_targets_the_foreground_group_not_the_shell() {
    let dir = tempfile::tempdir().unwrap();
    let backend = spawn_backend(&dir);
    let mut rx = backend.subscribe();
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    quiet(&mut rx, deadline);

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    // Start a long-lived foreground command.
    rt.block_on(backend.input(b"sleep 30\n"))
        .expect("start sleep");
    std::thread::sleep(Duration::from_millis(300));
    rt.block_on(backend.signal(ShellSignal::Int))
        .expect("Int ok");

    // The shell must still be alive: run a further command successfully.
    // The marker is split with `''` so the typed-command echo cannot
    // forge it: the echo shows `post-''signal-probe`, only the command's
    // real output holds the contiguous `post-signal-probe`.
    let probe = "printf 'post-''signal-probe\\n'\n";
    rt.block_on(backend.input(probe.as_bytes()))
        .expect("run after signal");
    std::thread::sleep(Duration::from_millis(300));
    // The foreground job is gone after `Int`, so the state is Idle; the
    // assertion that matters is that the shell itself is alive, which
    // `Exited` would deny.
    let status = rt.block_on(backend.status());
    match status {
        ShellResponse::Status {
            state: ShellState::Idle,
            ..
        } => {}
        other => panic!("shell must still be usable after Int, got {other:?}"),
    }
    let probe_marker = "post-signal-probe";
    let got = recv_until(
        &mut rx,
        std::time::Instant::now() + Duration::from_secs(10),
        |c| String::from_utf8_lossy(c).contains(probe_marker),
    )
    .expect("the post-signal command's output must stream");
    let got_str = String::from_utf8_lossy(&got);
    assert!(
        got_str.contains(probe_marker),
        "the post-signal output must reach the subscriber: {got_str:?}"
    );
}

#[test]
fn backend_status_reports_running_then_exited() {
    let dir = tempfile::tempdir().unwrap();
    let backend = spawn_backend(&dir);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    // The shell is alive with no job in the foreground: Idle (F1), not
    // Running — and decisively not Exited, which is the property this
    // test pins.
    match rt.block_on(backend.status()) {
        ShellResponse::Status {
            state: ShellState::Idle | ShellState::Running,
            ..
        } => {}
        other => panic!("expected the live state Idle, got {other:?}"),
    }
    // Kill the backend's shell child through the PtyShell, then wait for
    // the PTY to observe the exit (the reader thread drains EOF before
    // the pump gives up, so the shell is still alive briefly).
    {
        let mut shell = backend.shell.lock().unwrap_or_log();
        shell.kill().expect("kill shell");
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if !shell.is_alive().expect("liveness check") {
                break;
            }
            if std::time::Instant::now() >= deadline {
                panic!("shell child did not exit after kill");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let state = loop {
        match rt.block_on(backend.status()) {
            ShellResponse::Status {
                state: ShellState::Exited,
                ..
            } => break ShellState::Exited,
            _ if std::time::Instant::now() >= deadline => panic!("never Exited"),
            _ => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    assert_eq!(state, ShellState::Exited);
}

/// The pids whose PPID is `pid` — the shell's live children, found by a
/// bounded `/proc` walk, so panic-path cleanup can kill the exact job this
/// test started instead of any group it does not own.
fn child_pids_of(pid: Option<i64>) -> Vec<i64> {
    let Some(pid) = pid else {
        return Vec::new();
    };
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return found;
    };
    for entry in entries.flatten() {
        let Ok(raw) = entry.file_name().into_string() else {
            continue;
        };
        let Ok(tpid) = raw.parse::<i64>() else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{tpid}/stat")) else {
            continue;
        };
        let Some(fields) = stat.rsplit(')').next() else {
            continue;
        };
        let mut it = fields.split_whitespace();
        let _state = it.next();
        if it.next() == Some(&pid.to_string()) {
            found.push(tpid);
        }
    }
    found
}

/// Poll `backend.status()` until it reports `want`, or fail at the
/// deadline with the state actually observed.
fn status_until(backend: &Arc<PtyBackend>, want: ShellState) -> ShellState {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        match rt.block_on(backend.status()) {
            ShellResponse::Status { state, .. } if state == want => return state,
            other => {
                if std::time::Instant::now() >= deadline {
                    let (paused, fg, resumed) = {
                        let shell = backend.shell.lock().unwrap_or_log();
                        (
                            shell.paused_pgid(),
                            shell.has_foreground_job(),
                            shell.resumed_pgid_alive(),
                        )
                    };
                    panic!(
                        "never observed {want:?}, last state {other:?}, paused={paused:?}, fg={fg}, resumed={resumed:?}"
                    );
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

/// Panic-path cleanup for the status tests' `sleep 30`: resume whatever
/// the shell paused, then kill the shell's children and the shell by pid —
/// the section-G leak check counts every `sleep 30` this suite leaves
/// behind. Drop runs on unwind.
struct JobGuard {
    backend: Arc<PtyBackend>,
}
impl Drop for JobGuard {
    fn drop(&mut self) {
        let shell = self.backend.shell.lock().unwrap_or_log();
        for pgid in [shell.paused_pgid(), shell.resumed_pgid_alive()]
            .into_iter()
            .flatten()
        {
            let _ = std::process::Command::new("kill")
                .arg("-s")
                .arg("CONT")
                .arg("--")
                .arg(format!("-{pgid}"))
                .status();
        }
        let shell_pid = shell.pid().map(|p| p as i64);
        drop(shell);
        for pid in child_pids_of(shell_pid) {
            let _ = std::process::Command::new("kill")
                .arg("-9")
                .arg(format!("{pid}"))
                .status();
        }
        if let Some(pid) = shell_pid {
            let _ = std::process::Command::new("kill")
                .arg("-9")
                .arg(format!("{pid}"))
                .status();
        }
    }
}

/// Wait until the foreground group of `backend`'s shell belongs to the
/// exact job `argv` describes: `fg != shell pid` (F1) and the group
/// leader's `/proc/<fg>/cmdline` is `argv` NUL-joined. A shell's startup
/// rc (e.g. a `PROMPT_COMMAND` hook) can fork a transient child that
/// holds the terminal for seconds while the queued line is still
/// unread; the pty echoes a write as the line discipline receives it,
/// so the *echo* is not evidence the job started — only the leader's
/// cmdline is.
fn wait_for_fg_cmdline(backend: &Arc<PtyBackend>, argv: &[&str]) {
    let mut wanted: Vec<u8> = Vec::new();
    for (i, a) in argv.iter().enumerate() {
        if i > 0 {
            wanted.push(0);
        }
        wanted.extend_from_slice(a.as_bytes());
    }
    wanted.push(0);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let fg = {
            let shell = backend.shell.lock().unwrap_or_log();
            shell.current_foreground_pgid()
        };
        let hit = match fg {
            None => false,
            Some(fg) => {
                let pid = {
                    let shell = backend.shell.lock().unwrap_or_log();
                    shell.pid().map(i64::from)
                };
                Some(fg) != pid
                    && std::fs::read(format!("/proc/{fg}/cmdline"))
                        .map(|c| c == wanted)
                        .unwrap_or(false)
            }
        };
        if hit {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the job {argv:?} never took the foreground (last fg {fg:?})"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn backend_status_reports_paused_then_running() {
    // One state per assertion with its own message, in this order:
    // Running before STOP with a foreground job, Paused after STOP,
    // Running again after CONT.
    let dir = tempfile::tempdir().unwrap();
    let backend = spawn_backend(&dir);
    // The module-level JobGuard resumes and kills the exact job on any
    // assertion panic — the section-G leak check counts every `sleep 30`
    // this suite leaves behind.
    let _guard = JobGuard {
        backend: Arc::clone(&backend),
    };

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(backend.input(b"sleep 30\n"))
        .expect("start the foreground job");
    wait_for_fg_cmdline(&backend, &["sleep", "30"]);
    assert_eq!(
        status_until(&backend, ShellState::Running),
        ShellState::Running,
        "a foreground job must report Running after the fg wait loop proved has_foreground_job"
    );

    // Paused: STOP remembers the pgid and /proc/<pgid>/stat reads T.
    rt.block_on(backend.signal(ShellSignal::Stop))
        .expect("stop the job");
    assert_eq!(
        status_until(&backend, ShellState::Paused),
        ShellState::Paused,
        "a stopped foreground job must report Paused"
    );

    // Running again: CONT targets the remembered pgid (F2) and the job
    // resumes.
    rt.block_on(backend.signal(ShellSignal::Cont))
        .expect("resume the job");
    assert_eq!(
        status_until(&backend, ShellState::Running),
        ShellState::Running,
        "after CONT the resumed job must report Running again"
    );
    let _ = rt;
}

/// bug-08a-2's read-side companion: `Ctrl-p` is a toggle, so the state a
/// paused shell reports must go `Running → Paused → Running → Paused`
/// across STOP/CONT/STOP, with the second STOP reaching the job through
/// the remembered group even though bash holds the terminal after the
/// resume.
#[test]
fn backend_status_toggles_running_paused_more_than_once() {
    let dir = tempfile::tempdir().unwrap();
    let backend = spawn_backend(&dir);
    let _guard = JobGuard {
        backend: Arc::clone(&backend),
    };
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(backend.input(b"sleep 30\n"))
        .expect("start the foreground job");
    wait_for_fg_cmdline(&backend, &["sleep", "30"]);
    assert_eq!(
        status_until(&backend, ShellState::Running),
        ShellState::Running,
        "a foreground job must report Running before the first STOP"
    );
    rt.block_on(backend.signal(ShellSignal::Stop))
        .expect("the first STOP");
    assert_eq!(
        status_until(&backend, ShellState::Paused),
        ShellState::Paused,
        "the first STOP must report Paused"
    );
    rt.block_on(backend.signal(ShellSignal::Cont))
        .expect("the first CONT");
    assert_eq!(
        status_until(&backend, ShellState::Running),
        ShellState::Running,
        "the first CONT must report Running again"
    );
    rt.block_on(backend.signal(ShellSignal::Stop))
        .expect("the second STOP");
    assert_eq!(
        status_until(&backend, ShellState::Paused),
        ShellState::Paused,
        "the second STOP must report Paused again"
    );
    rt.block_on(backend.signal(ShellSignal::Cont))
        .expect("the second CONT");
    assert_eq!(
        status_until(&backend, ShellState::Running),
        ShellState::Running,
        "the second CONT must report Running again"
    );
}

#[test]
fn backend_status_is_idle_on_a_fresh_shell() {
    // Guards the rule-4 fallthrough: no job in the foreground, nothing
    // remembered, so the state a shell reports before anything has run
    // must be Idle, never Running.
    let dir = tempfile::tempdir().unwrap();
    let backend = spawn_backend(&dir);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        match rt.block_on(backend.status()) {
            ShellResponse::Status {
                state: ShellState::Idle,
                ..
            } => break,
            other => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "a fresh shell must settle to Idle, last state {other:?}"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

#[test]
fn close_hangs_up_a_shell_and_its_foreground_job() {
    // SIGHUP goes to the shell's pid, so it ends an idle shell *and* a
    // shell with a foreground job: the shell hangs its job up too.
    let dir = tempfile::tempdir().unwrap();
    let backend = spawn_backend(&dir);
    let _guard = JobGuard {
        backend: Arc::clone(&backend),
    };
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(backend.input(b"sleep 300\n"))
        .expect("start the foreground job");
    assert_eq!(
        status_until(&backend, ShellState::Running),
        ShellState::Running,
        "the foreground job must be Running before the hangup"
    );
    rt.block_on(backend.close())
        .expect("the hangup must succeed");
    assert_eq!(
        status_until(&backend, ShellState::Exited),
        ShellState::Exited,
        "the hangup must end the shell and its foreground job"
    );
}

/// The test runtime drives `input` itself: `block_on` inside a
/// `#[tokio::test]` would panic ("Cannot start a runtime from within a
/// runtime"), so the write must be awaited inline.
async fn handle_input(backend: &Arc<PtyBackend>, bytes: &[u8]) {
    backend.input(bytes).await.expect("writing into the shell");
}

#[tokio::test]
async fn serve_until_exit_returns_when_the_shell_exits() {
    // The host must return once the shell exits, so the process drops its
    // instance lock; against `serve` this handle never resolves.
    let dir = tempfile::tempdir().unwrap();
    let backend = spawn_backend(&dir);
    let sock = dir.path().join("s.sock");
    let listener = crate::shell::bind(&sock).await.expect("bind");
    let handle = tokio::spawn(crate::shell::serve_until_exit(
        listener,
        Arc::clone(&backend),
        Duration::from_millis(50),
    ));
    handle_input(&backend, b"exit\n").await;
    match tokio::time::timeout(Duration::from_secs(10), handle).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(e))) => panic!("serve_until_exit must end cleanly: {e}"),
        Ok(Err(e)) => panic!("serve_until_exit task panicked: {e}"),
        Err(_) => panic!("serve_until_exit must return once the shell exits"),
    }
}

use super::*;

// ── watch_shell (phase-04b) ──────────────────────────────────────────

/// Hold (and leak) the per-id instance lock so `host_is_alive` reads the
/// shell as live without a real host — what a pattern-arm test needs and
/// what a real host would otherwise provide.
fn hold_lock(id: &str) {
    let lock = crate::daemon::instance::InstanceLock::acquire(&crate::shell::lock_path(id))
        .expect("acquiring the per-id lock");
    std::mem::forget(lock);
}

/// A registry record for `index` under `Owner::Chat("watcher")`, written
/// inside a `lifecycle_home` scope (paths resolve through `HOME`).
fn insert_watch_record(reg: &ShellRegistry, index: u32, started_unix: u64) {
    insert(reg, index, Owner::Chat("watcher".into()), started_unix);
}

/// Append one pre-encoded cast event line to `path` (with its newline),
/// creating the logs dir the way a host's first write does.
fn append_event(path: &std::path::Path, t: f64, payload: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("creating the shell logs dir");
    }
    let line = format!(
        "[{}, \"o\", {}]\n",
        t,
        serde_json::to_string(payload).unwrap_or_default()
    );
    use std::io::Write as _;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .unwrap();
    f.write_all(line.as_bytes()).unwrap();
    f.flush().unwrap();
}

#[tokio::test]
async fn watch_shell_returns_when_the_pattern_appears() {
    let (_home, _dir) = lifecycle_home();
    let reg = ShellRegistry::default();
    insert_watch_record(&reg, 5, 1_788_470_005);
    append_event(&cast_path("s5", 1_788_470_005), 1.0, "warming up\n");
    hold_lock("s5");
    let path = cast_path("s5", 1_788_470_005);
    let writer = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        append_event(&path, 2.0, "READY to go\n");
    });
    let out = watch_shell_with(
        &reg,
        "s5",
        Some("READY"),
        5,
        Duration::from_millis(50),
        Duration::from_millis(0),
    )
    .await;
    writer.await.unwrap();
    assert!(
        out.contains("matched"),
        "a hit is reported as a match: {out}"
    );
    assert!(
        out.contains("READY to go"),
        "the matching line is shown: {out}"
    );
    assert!(
        out.contains("since this call started"),
        "the line number's meaning is named: {out}"
    );
}

#[tokio::test]
async fn watch_shell_ignores_output_written_before_the_call() {
    let (_home, _dir) = lifecycle_home();
    let reg = ShellRegistry::default();
    insert_watch_record(&reg, 6, 1_788_470_006);
    append_event(&cast_path("s6", 1_788_470_006), 1.0, "READY already\n");
    hold_lock("s6");
    let out = watch_shell_with(
        &reg,
        "s6",
        Some("READY"),
        1,
        Duration::from_millis(50),
        Duration::from_millis(0),
    )
    .await;
    assert!(
        out.contains("timed out"),
        "pre-call output must not match — the tool watches from now: {out}"
    );
    assert!(
        !out.contains("matched"),
        "the stale line is not what a match report shows: {out}"
    );
}

#[tokio::test]
async fn watch_shell_times_out_and_reports_what_it_saw() {
    let (_home, _dir) = lifecycle_home();
    let reg = ShellRegistry::default();
    insert_watch_record(&reg, 7, 1_788_470_007);
    hold_lock("s7");
    let path = cast_path("s7", 1_788_470_007);
    let writer = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        append_event(&path, 1.0, "progress 12%\n");
    });
    let out = watch_shell_with(
        &reg,
        "s7",
        Some("never-appears"),
        1,
        Duration::from_millis(50),
        Duration::from_millis(0),
    )
    .await;
    writer.await.unwrap();
    assert!(
        out.contains("timed out after 1 s"),
        "the timeout names the elapsed seconds: {out}"
    );
    assert!(
        out.contains("progress 12%"),
        "the tail shows what was actually seen: {out}"
    );
}

#[tokio::test]
async fn watch_shell_unknown_id_is_an_error_naming_live_shells() {
    let (_home, _dir) = lifecycle_home();
    let reg = ShellRegistry::default();
    insert_watch_record(&reg, 8, NOW);
    let out = watch_shell_with(
        &reg,
        "s99",
        Some("x"),
        5,
        Duration::from_millis(50),
        Duration::from_millis(0),
    )
    .await;
    assert!(
        out.starts_with("Error:"),
        "an unknown id is error text, returned as a String: {out}"
    );
    assert!(out.contains("s99"), "the bad id is named: {out}");
    assert!(
        out.contains("s8"),
        "the live ids are named so the model knows what to pass: {out}"
    );
    assert!(
        !out.contains("timed out"),
        "the refusal returns before any waiting: {out}"
    );
}

#[tokio::test]
async fn watch_shell_invalid_pattern_is_an_error() {
    let (_home, _dir) = lifecycle_home();
    let reg = ShellRegistry::default();
    insert_watch_record(&reg, 9, NOW);
    let early = std::time::Instant::now();
    let out = watch_shell_with(
        &reg,
        "s9",
        Some("[unclosed"),
        30,
        Duration::from_millis(50),
        Duration::from_millis(0),
    )
    .await;
    assert!(
        early.elapsed() < Duration::from_secs(1),
        "the refusal must return before the timeout, not after waiting: {out}"
    );
    assert!(
        out.starts_with("Error:"),
        "an invalid pattern is error text: {out}"
    );
    assert!(
        out.contains("[unclosed"),
        "the offending pattern is named: {out}"
    );
    assert!(
        out.contains("regex"),
        "the error says the pattern is applied as a regex: {out}"
    );
}

#[tokio::test]
async fn watch_shell_finds_the_cast_when_the_records_timestamp_is_stale() {
    let (_home, _dir) = lifecycle_home();
    let reg = ShellRegistry::default();
    // The record says the shell started at N, but the host stamped the
    // cast N+1 — a spawn that crossed a second boundary.
    insert_watch_record(&reg, 10, 1_788_470_010);
    let path = cast_path("s10", 1_788_470_011);
    append_event(&path, 1.0, "warm\n");
    hold_lock("s10");
    let writer = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        append_event(&path, 2.0, "FOUND IT\n");
    });
    let out = watch_shell_with(
        &reg,
        "s10",
        Some("FOUND IT"),
        5,
        Duration::from_millis(50),
        Duration::from_millis(0),
    )
    .await;
    writer.await.unwrap();
    assert!(
        out.contains("FOUND IT"),
        "the scan by parse_cast_name must find the fresh cast, \
             not the record's stale timestamp: {out}"
    );
}

#[tokio::test]
async fn watch_shell_stops_when_the_host_is_gone() {
    let (_home, _dir) = lifecycle_home();
    let reg = ShellRegistry::default();
    insert_watch_record(&reg, 11, 1_788_470_011);
    let path = cast_path("s11", 1_788_470_011);
    // A leased rather than leaked lock: dropping it makes the shell's
    // host look dead to `host_is_alive`.
    let leased = crate::daemon::instance::InstanceLock::acquire(&crate::shell::lock_path("s11"))
        .expect("acquiring the per-id lock for the test host");
    let writer = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        append_event(&path, 1.0, "left over\n");
        tokio::time::sleep(Duration::from_millis(150)).await;
        drop(leased);
    });
    let out = watch_shell_with(
        &reg,
        "s11",
        Some("never"),
        30,
        Duration::from_millis(50),
        Duration::from_millis(0),
    )
    .await;
    writer.await.expect("the writer task");
    assert!(
        out.contains("host is gone"),
        "the host-gone sentence is what comes back: {out}"
    );
    assert!(
        out.contains("left over"),
        "the tail of what was seen is reported: {out}"
    );
}

#[tokio::test]
async fn watch_shell_reports_that_nothing_was_running() {
    let (guard, _dir) = lifecycle_home();
    let reg = ShellRegistry::default();
    spawn_in_process_host(ShellId::from_index(12))
        .await
        .expect("the host fixture needs bash");
    insert_watch_record(&reg, 12, NOW);
    let out = watch_shell_with(
        &reg,
        "s12",
        None,
        5,
        Duration::from_millis(50),
        Duration::from_millis(100),
    )
    .await;
    drop(_dir);
    drop(guard);
    assert!(
        out.contains("nothing was running"),
        "an idle shell with no command is reported as such: {out}"
    );
    assert!(
        !out.contains("finished"),
        "an idle shell that never ran anything must not be called finished: {out}"
    );
    assert!(
        out.contains("start the command") || out.contains("Start the command"),
        "the sentence says what to do next: {out}"
    );
}

#[test]
fn visible_text_strips_ansi_escapes() {
    // The real prompt line from the measured production cast: OSC title
    // set, SGR colour onset and reset, around the visible text.
    let raw = "\u{1b}]0;matt@scrappy:~\u{7}\u{1b}]0;matt@scrappy: ~\u{7}\u{1b}[01;32mmatt@scrappy\u{1b}[00m:\u{1b}[01;34m~\u{1b}[00m$ ";
    let out = visible_text(raw);
    assert!(!out.contains('\u{1b}'), "no escape survives: {out:?}");
    assert!(
        out.contains("matt@scrappy") && out.contains('$'),
        "the plain text is what is left: {out:?}"
    );
}

#[test]
fn visible_text_keeps_the_text_after_the_last_carriage_return() {
    let out = visible_text("10%\r50%\r100% done\nnext");
    assert_eq!(
        out, "100% done\nnext",
        "the last write of a rewritten line wins: {out:?}"
    );
}

#[test]
fn input_target_note_names_each_foreground() {
    let prompt = input_target_note(&crate::shell::Foreground::Prompt);
    assert!(
        prompt.contains("prompt") && prompt.contains("shell command"),
        "Prompt must say the text will be run as a shell command: {prompt}"
    );
    let stdin_prog = input_target_note(&crate::shell::Foreground::Program {
        cmd: "grep".to_string(),
        tui: false,
    });
    assert!(
        stdin_prog.contains("grep") && stdin_prog.contains("stdin") && stdin_prog.contains("NOT"),
        "a non-TUI program must name the cmd and stdin: {stdin_prog}"
    );
    let tui = input_target_note(&crate::shell::Foreground::Program {
        cmd: "claude".to_string(),
        tui: true,
    });
    assert!(
        tui.contains("claude") && tui.contains("TUI"),
        "a TUI must name the cmd and the TUI: {tui}"
    );
    assert_ne!(
        prompt, stdin_prog,
        "a note that says the same thing in every state tells the approving \
             user nothing"
    );
    let paused = input_target_note(&crate::shell::Foreground::Paused {
        cmd: "sleep".to_string(),
    });
    assert!(
        paused.contains("sleep") && paused.contains("stopped"),
        "Paused names the stopped job: {paused}"
    );
    for fg in [
        crate::shell::Foreground::Exited,
        crate::shell::Foreground::Unknown,
    ] {
        let note = input_target_note(&fg);
        assert!(
            note.contains("gone"),
            "{fg} says the shell may be gone: {note}"
        );
    }
}

#[test]
fn screen_tail_keeps_the_last_rows_masked() {
    let screen = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\n\n\n\n";
    let out = screen_tail(screen, 5);
    assert_eq!(
        out, "four\nfive\nsix\nseven\neight",
        "8 rows + 3 blank ⇒ exactly rows 4–8, blanks dropped: {out:?}"
    );
    let secret = "AKIAIOSFODNN7EXAMPLE is on screen";
    let masked = screen_tail(&format!("row\n{secret}\nlast"), 5);
    assert!(
        !masked.contains("AKIAIOSFODNN7EXAMPLE") && masked.contains("<AWS_KEY>"),
        "the tail is masked before it reaches the panel: {masked:?}"
    );
}

#[test]
fn input_panel_shows_the_foreground_and_a_tui_screen_tail() {
    let input = composed("y", true);
    let screen = "top row\npick a mode\n> claude\n  1. opt\nAKIAIOSFODNN7EXAMPLE";
    let panel = input_panel(
        "s4",
        "y",
        &input,
        &crate::shell::Foreground::Program {
            cmd: "claude".to_string(),
            tui: true,
        },
        Some(screen),
    );
    assert!(
        panel.contains("foreground: running claude (TUI)"),
        "the panel names what holds the shell: {panel}"
    );
    assert!(
        panel.contains("screen (last 5 rows):") && panel.contains("> claude"),
        "a TUI panel shows the screen tail: {panel}"
    );
    assert!(
        panel.contains("<AWS_KEY>") && !panel.contains("AKIAIOSFODNN7EXAMPLE"),
        "the tail rows are masked: {panel}"
    );

    let prompt_panel = input_panel(
        "s4",
        "y",
        &input,
        &crate::shell::Foreground::Prompt,
        Some(screen),
    );
    assert!(
        prompt_panel.contains("foreground: shell prompt"),
        "the prompt panel names the prompt: {prompt_panel}"
    );
    assert!(
        !prompt_panel.contains("screen ("),
        "a prompt panel never shows a screen tail: {prompt_panel}"
    );
}

/// The composed input the older `send_input_with` tests send: the text,
/// no keys, the Enter flag, the default modes — the shape phase-02e's
/// new tests build explicitly.
fn composed(text: &str, enter: bool) -> ComposedInput {
    compose_input(text, None, &[], enter, crate::shell::ShellModes::default())
        .expect("plain text, no keys, a valid composition")
}

#[test]
fn compose_input_orders_text_keys_then_enter() {
    let keys = vec!["Up".to_string()];
    let input = compose_input("ls", None, &keys, true, crate::shell::ShellModes::default())
        .expect("a valid composition");
    assert_eq!(
        input.bytes,
        b"ls\x1b[A\r".to_vec(),
        "the text, then the encoded keys, then the Enter \\r: {:?}",
        input.bytes
    );
    assert_eq!(input.keys, keys, "the key names are carried verbatim");
    assert!(!input.bracketed, "plain text is never bracketed");
    assert!(input.enter, "the Enter flag is carried");

    let ctrl_c = vec!["C-c".to_string()];
    let input = compose_input(
        "",
        None,
        &ctrl_c,
        false,
        crate::shell::ShellModes::default(),
    )
    .expect("a keys-only send");
    assert_eq!(
        input.bytes,
        b"\x03".to_vec(),
        "an empty text with one control key sends just that key: {:?}",
        input.bytes
    );
    assert!(input.csi_u.is_empty(), "C-c has a legacy byte");
}

#[test]
fn compose_input_brackets_a_multiline_paste_when_the_shell_asked() {
    let on = crate::shell::ShellModes {
        bracketed_paste: true,
        ..crate::shell::ShellModes::default()
    };
    let input = compose_input("a\nb", None, &[], false, on).expect("the mode is on");
    assert_eq!(
        input.bytes,
        b"\x1b[200~a\nb\x1b[201~".to_vec(),
        "a newline with the mode on is wrapped in the paste brackets: {:?}",
        input.bytes
    );
    assert!(input.bracketed, "the wrap is reported to the panel/result");

    let off = crate::shell::ShellModes::default();
    let input = compose_input("a\nb", None, &[], false, off).expect("the mode is off");
    assert_eq!(
        input.bytes,
        b"a\nb".to_vec(),
        "with the mode off the raw bytes go out, never brackets: {:?}",
        input.bytes
    );
    assert!(!input.bracketed, "and it is reported as unbracketed");
}

#[test]
fn compose_input_refuses_bad_input_before_anything_is_sent() {
    let bad_key = vec!["Bogus".to_string()];
    let err = compose_input(
        "x",
        None,
        &bad_key,
        false,
        crate::shell::ShellModes::default(),
    )
    .expect_err("an unknown key is refused");
    assert!(
        err.to_string().contains("Bogus"),
        "the bad name is echoed: {err}"
    );

    let err = compose_input(
        "a\nb",
        Some(true),
        &[],
        false,
        crate::shell::ShellModes::default(),
    )
    .expect_err("paste: true with the mode off is refused");
    assert!(
        err.to_string().contains("bracketed paste"),
        "the error names the mode that is off: {err}"
    );

    let err = compose_input("", None, &[], false, crate::shell::ShellModes::default())
        .expect_err("an empty call is refused");
    assert!(
        err.to_string().contains("nothing to send"),
        "the empty call names its own error: {err}"
    );
}

#[tokio::test]
async fn send_input_result_names_csi_u_keys_and_the_paste() {
    let (_home, _dir) = lifecycle_home();
    let reg = ShellRegistry::default();
    insert(&reg, 3, Owner::Chat("caller".into()), NOW);
    let captured: std::sync::Mutex<Vec<Vec<u8>>> = std::sync::Mutex::new(Vec::new());
    let keys = vec!["S-Enter".to_string()];
    let input = compose_input("", None, &keys, false, crate::shell::ShellModes::default())
        .expect("S-Enter is in the vocabulary");
    let out = send_input_with(
        &reg,
        "s3",
        &input,
        &Owner::Chat("caller".into()),
        Duration::from_secs(5),
        |bytes| {
            let cap = &captured;
            async move {
                cap.lock().unwrap().push(bytes);
                Ok(())
            }
        },
    )
    .await;
    assert!(
        out.contains("S-Enter") && out.contains("CSI u"),
        "the result names the CSI u key and the encoding: {out}"
    );
    assert_eq!(
        captured.lock().unwrap().as_slice(),
        [input.bytes.as_slice()],
        "the bytes the injected send saw equal input.bytes"
    );

    captured.lock().unwrap().clear();
    let on = crate::shell::ShellModes {
        bracketed_paste: true,
        ..crate::shell::ShellModes::default()
    };
    let input = compose_input("a\nb", None, &[], false, on).expect("the mode is on");
    let out = send_input_with(
        &reg,
        "s3",
        &input,
        &Owner::Chat("caller".into()),
        Duration::from_secs(5),
        |bytes| {
            let cap = &captured;
            async move {
                cap.lock().unwrap().push(bytes);
                Ok(())
            }
        },
    )
    .await;
    assert!(
        out.contains("bracketed paste"),
        "the result says the text went out as a paste: {out}"
    );
    assert_eq!(
        captured.lock().unwrap().as_slice(),
        [input.bytes.as_slice()],
        "the bracketed bytes are what the injected send saw"
    );
}

#[tokio::test]
async fn send_input_enter_sends_a_carriage_return() {
    let (_home, _dir) = lifecycle_home();
    let reg = ShellRegistry::default();
    insert(&reg, 3, Owner::Chat("caller".into()), NOW);
    let captured: std::sync::Mutex<Vec<Vec<u8>>> = std::sync::Mutex::new(Vec::new());
    let input = compose_input(
        "hello",
        None,
        &[],
        true,
        crate::shell::ShellModes::default(),
    )
    .expect("plain text, no keys");
    let out = send_input_with(
        &reg,
        "s3",
        &input,
        &Owner::Chat("caller".into()),
        Duration::from_secs(5),
        |bytes| {
            let cap = &captured;
            async move {
                cap.lock().unwrap().push(bytes);
                Ok(())
            }
        },
    )
    .await;
    assert!(
        out.starts_with("Sent") && out.contains("s3"),
        "the report names the shell and what was sent: {out}"
    );
    assert!(out.contains("Enter sent"), "the Enter is reported: {out}");
    assert_eq!(
        captured.lock().unwrap().as_slice(),
        [b"hello\r".to_vec()],
        "enter: true appends exactly one \\r — the byte a real Enter sends"
    );
}

#[tokio::test]
async fn the_users_shell_is_visible_and_drivable_from_chat_only() {
    let (_home, _dir) = lifecycle_home();
    let reg = ShellRegistry::default();
    insert(&reg, 3, Owner::Chat("a".into()), NOW);
    insert(&reg, 7, Owner::User, NOW);
    insert(&reg, 8, Owner::Chat("b".into()), NOW);
    let mine = list_shells_in(&reg, None, &Owner::Chat("a".into()), NOW);
    assert!(mine.contains("s3") && mine.contains("s7"), "{mine}");
    assert!(
        !mine.contains("s8"),
        "another session's shell stays out: {mine}"
    );
    assert!(crate::shell::is_user_shell(&reg, "s7"));
    assert!(!crate::shell::is_user_shell(&reg, "s3"));

    let sent: std::sync::Mutex<Vec<Vec<u8>>> = std::sync::Mutex::new(Vec::new());
    let send = |bytes: Vec<u8>| {
        let sent = &sent;
        async move {
            sent.lock().unwrap().push(bytes);
            Ok(())
        }
    };
    let out = send_input_with(
        &reg,
        "s7",
        &composed("ls", true),
        &Owner::Chat("a".into()),
        Duration::from_secs(5),
        send,
    )
    .await;
    assert!(!out.contains("not yours"), "{out}");
    assert_eq!(
        sent.lock().unwrap().len(),
        1,
        "a chat session types into it"
    );

    let out = send_input_with(
        &reg,
        "s7",
        &composed("ls", true),
        &Owner::Ghost("g".into()),
        Duration::from_secs(5),
        send,
    )
    .await;
    assert!(out.contains("not yours"), "a ghost is refused: {out}");
    assert_eq!(sent.lock().unwrap().len(), 1, "nothing reached the shell");
}

#[tokio::test]
async fn send_input_without_enter_sends_no_newline() {
    let (_home, _dir) = lifecycle_home();
    let reg = ShellRegistry::default();
    insert(&reg, 3, Owner::Chat("caller".into()), NOW);
    let captured: std::sync::Mutex<Vec<Vec<u8>>> = std::sync::Mutex::new(Vec::new());
    let input = compose_input(
        "hello",
        None,
        &[],
        false,
        crate::shell::ShellModes::default(),
    )
    .expect("plain text, no keys");
    let out = send_input_with(
        &reg,
        "s3",
        &input,
        &Owner::Chat("caller".into()),
        Duration::from_secs(5),
        |bytes| {
            let cap = &captured;
            async move {
                cap.lock().unwrap().push(bytes);
                Ok(())
            }
        },
    )
    .await;
    assert!(
        out.contains("no Enter"),
        "the absence of the Enter is reported: {out}"
    );
    assert_eq!(
        captured.lock().unwrap().as_slice(),
        [b"hello".to_vec()],
        "enter: false sends the text alone, no terminator"
    );
}

#[tokio::test]
async fn send_input_refuses_another_owners_shell() {
    let (_home, _dir) = lifecycle_home();
    let reg = ShellRegistry::default();
    // Same inner string "a", different owner kind — the shape that breaks
    // a comparison on the inner string instead of `Owner::key()`.
    insert(&reg, 4, Owner::Ghost("a".into()), NOW);
    let sent: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
    let out = send_input_with(
        &reg,
        "s4",
        &composed("whoami", true),
        &Owner::Chat("a".into()),
        Duration::from_secs(5),
        |_bytes| {
            sent.store(true, Ordering::SeqCst);
            async { Ok(()) }
        },
    )
    .await;
    assert!(
        out.starts_with("Error:"),
        "another owner's shell is refused, as text not an Err: {out}"
    );
    assert!(out.contains("s4"), "the refused id is named: {out}");
    assert!(
        has_record(&reg, "s4"),
        "a refusal must not remove the shell from the registry"
    );
    assert!(
        !sent.load(Ordering::SeqCst),
        "the send must never have been attempted across owners"
    );
}

#[tokio::test]
async fn send_input_unknown_id_is_an_error_naming_live_shells() {
    let (_home, _dir) = lifecycle_home();
    let reg = ShellRegistry::default();
    insert(&reg, 5, Owner::Chat("caller".into()), NOW);
    let sent: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
    let out = send_input_with(
        &reg,
        "s99",
        &composed("echo hi", false),
        &Owner::Chat("caller".into()),
        Duration::from_secs(5),
        |_bytes| {
            sent.store(true, Ordering::SeqCst);
            async { Ok(()) }
        },
    )
    .await;
    assert!(
        out.starts_with("Error:"),
        "an unknown id is error text, not a panic and not an Err: {out}"
    );
    assert!(
        out.contains("s99") && out.contains("s5"),
        "the unknown id and the live ids are both named: {out}"
    );
    assert!(out.contains("list_shells"));
    assert!(!sent.load(Ordering::SeqCst));
}
#[tokio::test]
async fn send_input_text_never_reaches_read_shell() {
    let (_home, _dir) = lifecycle_home();
    let reg = ShellRegistry::default();
    spawn_in_process_host(ShellId::from_index(3))
        .await
        .expect("host binds the socket");
    insert(&reg, 3, Owner::Chat("caller".into()), NOW);
    let out = send_input_with(
        &reg,
        "s3",
        &composed("SENTINEL-NOT-IN-OUTPUT", false),
        &Owner::Chat("caller".into()),
        Duration::from_secs(5),
        |bytes| async move {
            crate::shell::send_input(&socket_path("s3"), &bytes, Duration::from_secs(5)).await
        },
    )
    .await;
    assert!(
        !out.starts_with("Error"),
        "the real path accepted the write: {out}"
    );
    // The host writes its `.cast` under the `HOME` it inherited at spawn
    // (see spawn_with_cast, whose tempdir homes both the PTY and the log);
    // the test's `lifecycle_home()` dir holds none of it by construction.
    // Production-path half of the safety property: the typed text really
    // reached a real host's cast as an `"i"` event. The read-side half —
    // that no reader surfaces it — is `…_in_a_reachable_cast` below,
    // which holds the cast the reader opens under the test's own `HOME`.
    let stray = find_stray_cast("SENTINEL-NOT-IN-OUTPUT");
    let recorded = std::fs::read_to_string(std::path::PathBuf::from(&stray))
        .unwrap_or_else(|_| panic!("reading the cast the input landed in: {stray}"));
    assert!(
        recorded.contains("SENTINEL-NOT-IN-OUTPUT"),
        "the \"i\" event is really written to the cast"
    );
}

/// The read-side half of the safety property, over a cast the read tools
/// can actually open. The production half above proves a real host writes
/// the `"i"` event; this proves a reader that stopped filtering on `"o"`
/// would surface the typed text through `read_shell`. The fixture builds
/// the cast directly (`CastWriter::create` + `write_output`/`write_input`)
/// in the module's own `fixture` shape — `.meta.json` included, the same
/// index the host screenshots through `mark` — so the reader really opens
/// the file, and a widened filter in `src/shell/log.rs` turns this red.
#[test]
fn send_input_text_never_reaches_read_shell_in_a_reachable_cast() {
    let (tmp, reg) = unreachable_cast_fixture();
    with_home(&tmp, || {
        let out = read_shell_in(&reg, "s12", None, None, None);
        assert!(
            out.contains("READY to go"),
            "the \"o\" output the fixture wrote is what the read tool returns: {out}"
        );
        assert!(
            !out.contains("SENTINEL-TYPED-BY-SEND-INPUT"),
            "typed input must never surface through the read tools: {out}"
        );
    });
}

/// A one-shell fixture whose cast holds an `"o"` event and an `"i"` event
/// and a `.meta.json` that names command 0 over a range covering both —
/// a future host that begins recording input inside a command's slice is
/// the shape this test defends against. Registry record `s12` under
/// `Owner::Chat("caller")`, `started_unix` 1_788_470_012 (index-derived,
/// so no other test's cast collides). Runs with `HOME` pointed at its own
/// tmp dir, the way `fixture` does, because `cast_path` resolves through
/// `HOME`.
fn unreachable_cast_fixture() -> (TmpHome, ShellRegistry) {
    let tmp = TmpHome::new();
    let reg = ShellRegistry::default();
    with_home(&tmp, || {
        let (id, started_unix) = ("s12", 1_788_470_012u64);
        let cast = cast_path(id, started_unix);
        std::fs::create_dir_all(cast.parent().unwrap()).unwrap();
        let mut w = crate::shell::CastWriter::create(&cast, 80, 24, started_unix).unwrap();
        w.mark(Duration::from_secs_f64(1.0), "probe --yes").unwrap();
        let first_byte = w.byte_len();
        w.write_output(Duration::from_secs_f64(1.5), b"READY to go\n")
            .unwrap();
        w.write_input(
            Duration::from_secs_f64(2.0),
            b"SENTINEL-TYPED-BY-SEND-INPUT\n",
        )
        .unwrap();
        let end_byte = w.byte_len();
        w.mark(Duration::from_secs_f64(2.5), "end 0").unwrap();
        drop(w);
        let meta = crate::shell::MetaIndex {
            owner: None,
            host: None,
            shell_id: id.into(),
            cast: cast.file_name().unwrap().to_string_lossy().into_owned(),
            started_unix,
            commands: vec![crate::shell::CommandRecord {
                index: 0,
                command: "probe --yes".into(),
                started: 1.0,
                ended: 2.5,
                exit_code: 0,
                first_byte,
                end_byte,
            }],
        };
        meta.save(&crate::shell::meta_path_for(&cast)).unwrap();
        insert(&reg, 12, Owner::Chat("caller".into()), started_unix);
    });
    (tmp, reg)
}

/// The most recent temp dir holding a `.cast` that records `sentinel`.
fn find_stray_cast(sentinel: &str) -> String {
    let mut candidates: Vec<(std::time::SystemTime, String)> = Vec::new();
    let _ = std::fs::read_dir("/tmp").map(|rd| {
        for entry in rd.flatten() {
            let p = entry.path().join("s3.cast");
            if let Ok(s) = std::fs::read_to_string(&p)
                && s.contains(sentinel)
            {
                let t = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                candidates.push((t, p.to_string_lossy().into_owned()));
            }
        }
    });
    // The assertion above guarantees at least one; pick the newest.
    candidates
        .into_iter()
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, p)| p)
        .unwrap_or_default()
}

// -----------------------------------------------------------------------
// list_shell_logs / read_shell_log (M22 phase-06b)
// -----------------------------------------------------------------------

/// A log directory under the temp `HOME` (where `shell_logs_dir()` points)
/// plus the dir path, for driving the `_in` seams directly. The whole
/// body must call the seams inside `with_home(&tmp, …)` so
/// `shell_logs_dir()` and the writing `meta_path_for` agree on the
/// directory.
fn logs_dir_fixture() -> (TmpHome, std::path::PathBuf) {
    let tmp = TmpHome::new();
    let dir = with_home(&tmp, || {
        let dir = crate::config::shell_logs_dir();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    });
    (tmp, dir)
}

/// Write one recording named `log_id` into `dir`: a `.cast` holding
/// `per_command` (one line of output per command) and a `.meta.json`
/// sidecar indexing those commands with the given exit codes. The
/// commands' text is `per_command` joined with " && " — so a row's hint
/// is that row's own commanded output, not a label shared across rows —
/// and the artifact is built with the real producer paths so the test
/// cannot disagree with the sweep or the id grammar.
fn write_recording(dir: &std::path::Path, log_id: &str, per_command: &[&str], exit_codes: &[i32]) {
    let cast = dir.join(format!("{log_id}.cast"));
    let started_unix: u64 = log_id
        .rsplit_once('-')
        .and_then(|(_, ts)| ts.parse().ok())
        .unwrap_or(0);
    let mut w = crate::shell::CastWriter::create(&cast, 80, 24, started_unix).unwrap();
    let mut records = Vec::new();
    let shared_command = per_command.join(" && ");
    for (i, out) in per_command.iter().enumerate() {
        let t = 1.0 + i as f64;
        let command = shared_command.clone();
        w.mark(Duration::from_secs_f64(t), &command).unwrap();
        let first_byte = w.byte_len();
        w.write_output(
            Duration::from_secs_f64(t + 0.5),
            format!("{out}\n").as_bytes(),
        )
        .unwrap();
        let end_byte = w.byte_len();
        w.mark(Duration::from_secs_f64(t + 1.0), &format!("end {i}"))
            .unwrap();
        records.push(crate::shell::CommandRecord {
            index: i as u32,
            command,
            started: t,
            ended: t + 0.5,
            exit_code: exit_codes.get(i).copied().unwrap_or(0),
            first_byte,
            end_byte,
        });
    }
    drop(w);
    let meta = crate::shell::MetaIndex {
        owner: None,
        host: None,
        shell_id: log_id
            .rsplit_once('-')
            .map(|(id, _)| id)
            .unwrap_or("")
            .into(),
        cast: cast.file_name().unwrap().to_string_lossy().into_owned(),
        started_unix,
        commands: records,
    };
    meta.save(&meta_path_for(&cast)).unwrap();
}

/// The row's hint is its own first command's text. The slices guard on
/// `min`/`max` because a row id can also appear in the "past the limit"
/// suffix; the assertion needs the listed rows only.
fn row_hint(out: &str, id: &str, next: Option<&str>) -> String {
    let start = out.find(id).expect("row listed");
    let end = match next {
        Some(n) => out[start + id.len()..]
            .find(n)
            .map(|i| start + id.len() + i)
            .unwrap_or(out.len()),
        None => out.len(),
    };
    out[start..end.min(out.len())].to_string()
}

#[test]
fn list_shell_logs_is_newest_first() {
    let (tmp, dir) = logs_dir_fixture();
    with_home(&tmp, || {
        write_recording(&dir, "s3-1788470000", &["older output"], &[0]);
        write_recording(&dir, "s9-1788480000", &["newer output"], &[0]);
        write_recording(&dir, "s1-1788460000", &["oldest output"], &[0]);
        let out = list_shell_logs_in(&dir, None, None, 1_788_480_100);
        let s9 = out.find("s9-1788480000").expect("s9 listed");
        let s3 = out.find("s3-1788470000").expect("s3 listed");
        let s1 = out.find("s1-1788460000").expect("s1 listed");
        assert!(
            s9 < s3 && s3 < s1,
            "rows are newest first by started_unix: {out}"
        );
        let s1_hint = row_hint(&out, "s1-1788460000", None);
        assert!(
            s1_hint.contains("oldest output"),
            "the oldest row's hint is its own first command: {out}"
        );
        let s9_hint = row_hint(&out, "s9-1788480000", Some("s3-1788470000"));
        assert!(
            s9_hint.contains("newer output"),
            "each row's hint is that row's own command: {out}"
        );
    });
}

#[test]
fn list_shell_logs_includes_a_cast_with_no_index() {
    let (tmp, dir) = logs_dir_fixture();
    with_home(&tmp, || {
        write_recording(&dir, "s4-1788470400", &["indexed output"], &[0]);
        let cast = dir.join("s5-1788470500.cast");
        std::fs::write(&cast, b"{\"version\": 2, \"width\": 80}").unwrap();
        let out = list_shell_logs_in(&dir, None, None, 1_788_480_100);
        let s5 = out
            .find("s5-1788470500")
            .expect("the unindexed cast is listed");
        assert!(
            out[s5..].contains("unknown"),
            "an unindexed recording shows an unknown command count, not 0: {out}"
        );
        assert!(
            !out[s5..].contains("0 command"),
            "unknown must never be rendered as 0 commands: {out}"
        );
        // The indexed sibling keeps its real count.
        let s4 = out
            .find("s4-1788470400")
            .expect("the indexed cast is listed");
        assert!(
            out[s4..].contains("1 command"),
            "a readable index shows its real count: {out}"
        );
    });
}

#[test]
fn list_shell_logs_since_and_limit_filter() {
    let (tmp, dir) = logs_dir_fixture();
    with_home(&tmp, || {
        write_recording(&dir, "s2-1788460000", &["old output"], &[0]);
        write_recording(&dir, "s4-1788470000", &["boundary output"], &[0]);
        write_recording(&dir, "s6-1788480000", &["new output"], &[0]);
        // `since` is inclusive: the boundary recording itself survives.
        let out = list_shell_logs_in(&dir, Some(1_788_470_000), None, 1_788_480_100);
        assert!(
            !out.contains("s2-1788460000"),
            "a recording older than `since` is excluded: {out}"
        );
        assert!(
            out.contains("s4-1788470000") && out.contains("s6-1788480000"),
            "recordings at or after `since` are included: {out}"
        );
        // `limit` caps the rows and the output says it clamped.
        let out = list_shell_logs_in(&dir, None, Some(900), 1_788_480_100);
        assert!(
            out.contains("clamped"),
            "a limit above the cap is reported as clamped: {out}"
        );
        let out = list_shell_logs_in(&dir, None, Some(2), 1_788_480_100);
        assert!(
            out.contains("1 more recording"),
            "the truncated suffix is named, not silently dropped: {out}"
        );
        // The listed rows are the newest pair; the id string asserts on
        // the rows only, so a listed id must not appear twice (once as a
        // row, once in the suffix). Split the output at the suffix and
        // assert on the rows.
        let rows_part = out.split("[1 more recording").next().unwrap_or(&out);
        assert!(
            !rows_part.contains("s2-1788460000"),
            "not every row survived the limit: {rows_part}"
        );
        assert!(
            rows_part.contains("s6-1788480000"),
            "the newest row survives a limit of 2: {rows_part}"
        );
        let without = list_shell_logs_in(&dir, Some(1_788_481_000), None, 1_788_480_100);
        assert!(
            without.contains("No shell recordings at or after"),
            "a `since` past every recording says so plainly: {without}"
        );
    });
}

#[test]
fn read_shell_log_refuses_a_bare_shell_id() {
    let (tmp, dir) = logs_dir_fixture();
    with_home(&tmp, || {
        write_recording(&dir, "s3-1788470000", &["the only s3 recording"], &[0]);
        let out = read_log_in(&dir, "s3", None, None, None);
        assert!(
            out.starts_with("Error:"),
            "a bare shell id is a refusal, not a read: {out}"
        );
        assert!(
            out.contains("s3"),
            "the refusal names the offending value: {out}"
        );
        assert!(
            out.contains("list_shell_logs"),
            "the refusal points at the listing tool: {out}"
        );
        assert!(
            !out.contains("the only s3 recording"),
            "no recording is read even though exactly one s3-* exists: {out}"
        );
    });
}

#[test]
fn read_shell_log_reads_one_command_with_its_exit_code() {
    let (tmp, dir) = logs_dir_fixture();
    with_home(&tmp, || {
        write_recording(
            &dir,
            "s7-1788477000",
            &["first output", "second output"],
            &[3, 0],
        );
        let out = read_log_in(&dir, "s7-1788477000", Some(1), None, None);
        assert!(
            out.contains("second output") && !out.contains("first output"),
            "command_index 1 returns the second command's output only: {out}"
        );
        assert!(
            out.contains("command 1"),
            "the header names the selected index: {out}"
        );
        assert!(
            out.contains("exit code 0"),
            "command 1's real exit code is cited, not a default: {out}"
        );
        let failing = read_log_in(&dir, "s7-1788477000", Some(0), None, None);
        assert!(
            failing.contains("exit code 3") && failing.contains("non-zero"),
            "command 0's real non-zero exit code is in the header, not a default: {failing}"
        );
        assert!(
            failing.contains("first output") && !failing.contains("second output"),
            "command_index 0 returns the first command's output only: {failing}"
        );
    });
}

#[test]
fn read_shell_log_unknown_log_is_an_error_naming_what_exists() {
    let (tmp, dir) = logs_dir_fixture();
    with_home(&tmp, || {
        write_recording(&dir, "s2-1788465000", &["real output"], &[0]);
        let out = read_log_in(&dir, "s9-1788499999", None, None, None);
        assert!(
            out.starts_with("Error:"),
            "an absent log id is an error, returned as a String: {out}"
        );
        assert!(
            out.contains("s9-1788499999"),
            "the error names what could not be found: {out}"
        );
        assert!(
            out.contains("s2-1788465000"),
            "the error lists the available log ids: {out}"
        );
    });
}

#[test]
fn read_shell_log_invalid_filter_is_an_error() {
    let (tmp, dir) = logs_dir_fixture();
    with_home(&tmp, || {
        write_recording(&dir, "s6-1788476000", &["real output"], &[0]);
        let out = read_log_in(&dir, "s6-1788476000", None, None, Some("[unclosed"));
        assert!(
            out.starts_with("Error:"),
            "an invalid filter regex is an error, not a literal match: {out}"
        );
        assert!(
            out.contains("[unclosed"),
            "the error names the offending pattern: {out}"
        );
        assert!(
            out.contains("regex"),
            "the error says the filter is applied as a regex: {out}"
        );
        assert!(
            !out.contains("real output"),
            "the pattern is rejected before any file is read: {out}"
        );
    });
}

// -----------------------------------------------------------------------
// read_shell_screen (M28 phase 01c)
// -----------------------------------------------------------------------

// -- screen waits (M28 phase 03a) --------------------------------------

#[test]
fn wait_bounds_clamp_the_timeout_and_the_settle() {
    let (t, s) = wait_bounds(None, None);
    assert_eq!(t, Duration::from_secs(30), "the default timeout is 30 s");
    assert_eq!(s, None, "no settle_ms means no settle wait");

    let (t, _) = wait_bounds(Some(0), None);
    assert_eq!(t, Duration::from_secs(1), "0 s clamps up to 1 s");
    let (t, _) = wait_bounds(Some(9999), None);
    assert_eq!(t, Duration::from_secs(300), "9999 s clamps down to 300 s");

    let (t, s) = wait_bounds(Some(10), Some(1));
    assert_eq!(t, Duration::from_secs(10));
    assert_eq!(s, Some(Duration::from_millis(50)), "1 ms clamps to 50 ms");
    let (_, s) = wait_bounds(None, Some(99999));
    assert_eq!(
        s,
        Some(Duration::from_millis(5000)),
        "99999 ms clamps to 5 s"
    );
}

#[tokio::test]
async fn wait_screen_change_returns_on_the_first_difference() {
    let seq: std::cell::RefCell<Vec<&'static str>> = std::cell::RefCell::new(vec![
        "$ claude", "$ claude", "UI", "UI", "UI", "UI", "UI", "UI", "UI", "UI", "UI", "UI",
    ]);
    let read = || {
        let mut q = seq.borrow_mut();
        let v = if q.is_empty() { "UI" } else { q.remove(0) };
        std::future::ready(Ok::<String, String>(v.to_string()))
    };
    assert!(
        wait_screen_change(
            read,
            "$ claude",
            Duration::from_millis(200),
            Duration::from_millis(10),
        )
        .await,
        "the third read shows the drawn UI, so the wait returns true"
    );

    let read = || std::future::ready(Ok::<String, String>("$ claude".to_string()));
    assert!(
        !wait_screen_change(
            read,
            "$ claude",
            Duration::from_millis(200),
            Duration::from_millis(10),
        )
        .await,
        "a reader that never changes is the launch's not-drawn-yet case: false"
    );
}

#[tokio::test]
async fn wait_screen_returns_when_the_pattern_appears() {
    let seq: std::cell::RefCell<Vec<&'static str>> = std::cell::RefCell::new(vec![
        "loading", "loading", "done $", "done $", "done $", "done $",
    ]);
    let read = || {
        let mut q = seq.borrow_mut();
        let v = q.remove(0);
        std::future::ready(Ok::<String, String>(v.to_string()))
    };
    let re = regex::Regex::new("done").unwrap();
    let out = wait_screen(
        read,
        Some(&re),
        None,
        Duration::from_millis(200),
        Duration::from_millis(10),
    )
    .await;
    assert!(
        matches!(out, WaitOutcome::Matched { .. }),
        "the third read matches, so the wait ends Matched: {out:?}"
    );
}

#[tokio::test]
async fn wait_screen_with_both_waits_for_the_pattern_then_settles() {
    let calls = std::rc::Rc::new(std::cell::Cell::new(0u32));
    let counter = std::rc::Rc::clone(&calls);
    let read = move || {
        let c = counter.get();
        counter.set(c + 1);
        let v = match c {
            0 => "loading",
            1 => "done 1",
            2 => "done 2",
            _ => "done 3",
        };
        std::future::ready(Ok::<String, String>(v.to_string()))
    };
    let re = regex::Regex::new("done").unwrap();
    let out = wait_screen(
        read,
        Some(&re),
        Some(Duration::from_millis(10)),
        Duration::from_millis(200),
        Duration::from_millis(10),
    )
    .await;
    assert!(
        matches!(out, WaitOutcome::Settled { .. }),
        "with both waits the pattern match hands over to the settle phase, \
             which needs two equal reads after `done 1`: {out:?}"
    );
    assert!(
        calls.get() > 2,
        "the reader was called past the first `done` read (called {} times)",
        calls.get()
    );
}

#[tokio::test]
async fn wait_screen_settles_when_two_reads_agree() {
    let seq: std::cell::RefCell<Vec<&'static str>> = std::cell::RefCell::new(vec![
        "a", "b", "c", "c", "c", "c", "c", "c", "c", "c", "c", "c", "c", "c", "c", "c", "c", "c",
        "c", "c",
    ]);
    let read = || {
        let mut q = seq.borrow_mut();
        let v = q.remove(0);
        std::future::ready(Ok::<String, String>(v.to_string()))
    };
    let out = wait_screen(
        read,
        None,
        Some(Duration::from_millis(10)),
        Duration::from_millis(200),
        Duration::from_millis(10),
    )
    .await;
    assert!(
        matches!(out, WaitOutcome::Settled { .. }),
        "reads three and four both say c, so the screen settled: {out:?}"
    );
}

#[tokio::test]
async fn wait_screen_that_never_settles_is_a_result_not_an_error() {
    let n = std::cell::Cell::new(0u64);
    let read = || {
        let v = n.get();
        n.set(v + 1);
        std::future::ready(Ok::<String, String>(format!("frame {v}")))
    };
    let out = wait_screen(
        read,
        None,
        Some(Duration::from_millis(10)),
        Duration::from_millis(200),
        Duration::from_millis(10),
    )
    .await;
    let WaitOutcome::NotSettled { timeout } = &out else {
        panic!("a counter screen never settles, so the wait is NotSettled: {out:?}")
    };
    assert_eq!(
        *timeout,
        Duration::from_millis(200),
        "the deadline is named"
    );
    let note = wait_note(&out, None).unwrap_or_default();
    assert!(
        note.contains("did not settle") && note.contains("wait_for"),
        "the note points the model at wait_for: {note}"
    );
}

#[tokio::test]
async fn wait_screen_never_matching_times_out() {
    let read = || std::future::ready(Ok::<String, String>("loading".to_string()));
    let re = regex::Regex::new("done").unwrap();
    let out = wait_screen(
        read,
        Some(&re),
        None,
        Duration::from_millis(200),
        Duration::from_millis(10),
    )
    .await;
    assert!(
        matches!(out, WaitOutcome::NotMatched { .. }),
        "a screen that never shows the pattern is NotMatched: {out:?}"
    );
    let note = wait_note(&out, Some(&re)).unwrap_or_default();
    assert!(
        note.contains("done"),
        "the note names the pattern it waited for: {note}"
    );

    let read = || std::future::ready(Err::<String, String>("gone".to_string()));
    let out = wait_screen(
        read,
        Some(&re),
        None,
        Duration::from_millis(200),
        Duration::from_millis(10),
    )
    .await;
    assert!(
        matches!(&out, WaitOutcome::Failed(msg) if msg == "gone"),
        "the read's error is passed through: {out:?}"
    );
    assert!(
        wait_note(&out, Some(&re)).is_none(),
        "a Failed outcome shows no screen"
    );
}

#[tokio::test]
async fn read_shell_screen_invalid_wait_for_is_an_error() {
    let (_home, _dir) = lifecycle_home();
    let reg = ShellRegistry::default();
    insert_watch_record(&reg, 5, 1_788_470_005);
    let out = read_shell_screen_in(
        &reg,
        "s5",
        None,
        None,
        None,
        false,
        Some("("),
        None,
        None,
        Duration::from_millis(200),
    )
    .await;
    assert!(
        out.starts_with("Error: invalid wait_for regex \"(\""),
        "an invalid wait_for regex is an error before any socket I/O: {out}"
    );
}

// -----------------------------------------------------------------------
// read_shell_screen (M28 phase 01c) — live-shell fixtures
// -----------------------------------------------------------------------

/// Open a real in-process shell host and return its id. The same path
/// `close_shell_ends_the_callers_shell` takes.
async fn open_screen_shell(reg: &ShellRegistry) -> String {
    let opened = open_shell_with(
        reg,
        &Owner::Chat("screen-reader".into()),
        crate::host::HostId::Local,
        5,
        Duration::from_millis(3000),
        spawn_in_process_host,
    )
    .await;
    assert!(
        opened.contains("Opened shell"),
        "the precondition is a real, ready shell: {opened}"
    );
    opened
        .split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_ascii_alphanumeric())
                .to_string()
        })
        .find(|w| w.starts_with('s') && w[1..].chars().all(|c| c.is_ascii_digit()))
        .expect("the success sentence names the id")
}

async fn type_into(id: &str, bytes: &[u8]) {
    crate::shell::send_input(&socket_path(id), bytes, Duration::from_secs(5))
        .await
        .expect("the shell accepts input");
}

/// Read the screen until `done` holds or 10 s pass; returns the last read.
async fn screen_until(
    reg: &ShellRegistry,
    id: &str,
    lines: Option<u64>,
    pattern: Option<&str>,
    done: impl Fn(&str) -> bool,
) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let out = read_shell_screen_in(
            reg,
            id,
            lines,
            None,
            pattern,
            false,
            None,
            None,
            None,
            Duration::from_secs(5),
        )
        .await;
        if done(&out) || Instant::now() >= deadline {
            return out;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The rows after the header line.
fn body_rows(out: &str) -> Vec<&str> {
    out.lines().skip(1).collect()
}

#[tokio::test]
async fn read_shell_screen_returns_the_screen_with_a_foreground_header() {
    let (_home, _dir) = lifecycle_home();
    let reg = ShellRegistry::default();
    let id = open_screen_shell(&reg).await;
    type_into(&id, b"echo SCREEN-MARKER-01C\n").await;
    let out = screen_until(&reg, &id, None, None, |o| {
        o.lines().skip(1).any(|l| l.trim() == "SCREEN-MARKER-01C") && o.contains("shell prompt")
    })
    .await;
    let header = out.lines().next().unwrap_or_default();
    assert!(
        header.starts_with(&format!("Shell {id} — shell prompt — 24x80, cursor ")),
        "the header names the shell, its foreground and its size: {out}"
    );
    assert!(
        body_rows(&out)
            .iter()
            .any(|l| l.trim() == "SCREEN-MARKER-01C"),
        "the body is the rendered screen: {out}"
    );
}

#[tokio::test]
async fn read_shell_screen_names_a_running_program() {
    let (_home, _dir) = lifecycle_home();
    let reg = ShellRegistry::default();
    let id = open_screen_shell(&reg).await;
    type_into(&id, b"sleep 30\n").await;
    let out = screen_until(&reg, &id, None, None, |o| o.contains("running sleep")).await;
    type_into(&id, b"\x03").await;
    assert!(
        out.lines()
            .next()
            .is_some_and(|h| h.contains("— running sleep —")),
        "a running job is named in the header: {out}"
    );
}

#[tokio::test]
async fn read_shell_screen_lines_tails_the_visible_rows() {
    let (_home, _dir) = lifecycle_home();
    let reg = ShellRegistry::default();
    let id = open_screen_shell(&reg).await;
    type_into(&id, b"printf 'row-a\\nrow-b\\nrow-c\\n'\n").await;
    // Wait for the output (a row that is exactly `row-c`, not the echoed
    let full = screen_until(&reg, &id, None, None, |o| {
        o.lines().any(|l| l.trim() == "row-c") && o.contains("shell prompt")
    })
    .await;
    let rows = body_rows(&full);
    let after_c = rows
        .iter()
        .rposition(|l| l.trim() == "row-c")
        .expect("row-c is on screen");
    let tail_len = rows.len() - after_c;
    let out = read_shell_screen_in(
        &reg,
        &id,
        Some(tail_len as u64),
        None,
        None,
        false,
        None,
        None,
        None,
        Duration::from_secs(5),
    )
    .await;
    let tailed = body_rows(&out);
    assert_eq!(tailed.len(), tail_len,);
    assert_eq!(tailed[0].trim(), "row-c", "the tail starts at row-c: {out}");
    assert!(
        !tailed.iter().any(|l| l.trim() == "row-a"),
        "rows above the tail are dropped: {out}"
    );
}
#[tokio::test]
async fn read_shell_screen_pattern_filters_rows() {
    let (_home, _dir) = lifecycle_home();
    let reg = ShellRegistry::default();
    let id = open_screen_shell(&reg).await;
    type_into(&id, b"printf 'row-a\\nrow-b\\nrow-c\\n'\n").await;
    let out = screen_until(&reg, &id, None, Some("^row-b$"), |o| {
        body_rows(o).contains(&"row-b")
    })
    .await;
    assert_eq!(
        body_rows(&out),
        vec!["row-b"],
        "only the row matching the pattern is returned, not the echoed command: {out}"
    );
    assert!(
        out.lines().next().is_some_and(|h| h.contains("/^row-b$/")),
        "{out}"
    );
}

#[tokio::test]
async fn read_shell_screen_invalid_pattern_is_an_error() {
    let (_home, _dir) = lifecycle_home();
    let reg = ShellRegistry::default();
    insert_watch_record(&reg, 5, 1_788_470_005);
    let out = read_shell_screen_in(
        &reg,
        "s5",
        None,
        None,
        Some("("),
        false,
        None,
        None,
        None,
        Duration::from_millis(200),
    )
    .await;
    assert!(
        out.starts_with("Error: invalid pattern regex \"(\""),
        "an invalid regex is an error before any socket I/O: {out}"
    );
}

#[tokio::test]
async fn read_shell_screen_unknown_shell_is_an_error_naming_live_shells() {
    let (_home, _dir) = lifecycle_home();
    let reg = ShellRegistry::default();
    insert_watch_record(&reg, 5, 1_788_470_005);
    let out = read_shell_screen_in(
        &reg,
        "s9",
        None,
        None,
        None,
        false,
        None,
        None,
        None,
        Duration::from_millis(200),
    )
    .await;
    assert_eq!(
        out,
        unknown_shell_error(&reg, "s9"),
        "the same sentence read_shell gives"
    );
    assert!(out.contains("s5"), "the live ids are named: {out}");
    assert!(out.contains("s5"), "the live ids are named: {out}");
}

#[tokio::test]
async fn read_shell_screen_records_last_foreground() {
    let (_home, _dir) = lifecycle_home();
    let reg = ShellRegistry::default();
    let id = open_screen_shell(&reg).await;
    let before = with_shells(&reg, |store| {
        store.get(&id).and_then(|r| r.last_foreground.clone())
    });
    assert_eq!(before, None, "nothing is observed before the first read");
    let out = screen_until(&reg, &id, None, None, |o| o.contains("shell prompt")).await;
    let after = with_shells(&reg, |store| {
        store.get(&id).and_then(|r| r.last_foreground.clone())
    });
    let (fg, at) = after.expect("the read stores what it observed");
    assert_eq!(fg, crate::shell::Foreground::Prompt, "{out}");
    assert!(at > 0, "the observation carries its time");
}

#[test]
fn scrollback_part_strips_the_visible_suffix() {
    assert_eq!(
        scrollback_part("old-1\nold-2\nnow-1\nnow-2", "now-1\nnow-2"),
        "old-1\nold-2"
    );
    assert_eq!(
        scrollback_part("old-1\nchanged", "now-1"),
        "old-1\nchanged",
        "a screen that changed between reads keeps the whole later read"
    );
    assert_eq!(
        scrollback_part("old-1", ""),
        "old-1",
        "an empty screen strips nothing"
    );
}

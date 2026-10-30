use super::*;

// ── run_terminal_command { host? } (M21 phase-04d) ──────────────────────

/// The module seam `run_in_shell_with_prompts` needs is the transport's
/// write/read halves, which `run_in_shell_for_test` already supplies;
/// what the host tests additionally need is a registry the host assertions
/// can read after the call. This helper exists because the tests hold
/// `reg` themselves — `run_in_shell_core` takes it, so no swap is needed.
///
/// Returns the id string the run's answer quoted (`text` embeds it via the
/// ordinary tool path is *not* used here — the raw result is returned), so
/// the host tests can find the shell the reuse rule chose.
async fn run_hosted<S, Fut>(
    reg: &ShellRegistry,
    owner: &Owner,
    cmd: &str,
    shell: Option<&str>,
    host: crate::host::HostId,
    spawn: S,
) -> anyhow::Result<(ShellId, ShellRunResult)>
where
    S: FnOnce(ShellId) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    run_in_shell_for_test_on_host(reg, owner.clone(), cmd, 5, shell, None, host, spawn).await
}

/// Task 1 row 2: `run_terminal_command { host: "web01" }` with no `shell`
/// reuses this session's lowest-numbered open shell **on that host**, and
/// creates one there when there is none. The record's `host` and the
/// remote `hostname` output are both checked, so this pins not just where
/// the record was stamped but where the command actually ran.
#[tokio::test]
async fn run_terminal_command_on_a_named_host_reuses_that_hosts_shell() {
    let (_home, dir) = crate::test_home();
    std::fs::write(
        dir.path().join(".daemoneye/etc/config.toml"),
        "[models.default]\nprovider = \"ollama\"\n\n[hosts.web01]\n\
             ssh = \"matt@web01.lan\"\n",
    )
    .expect("write config");
    let reg = ShellRegistry::default();
    let owner = Owner::Chat("remote-run".into());

    // A first command on web01: no shell of this owner exists anywhere, so
    // one is created **on web01**.
    let (id1, first) = run_hosted(
        &reg,
        &owner,
        "echo first-remote",
        None,
        crate::host::HostId::Named("web01".to_string()),
        spawn_in_process_host,
    )
    .await
    .expect("the remote shell resolves");
    assert_eq!(first.exit_code, 0, "the remote command ran cleanly");
    let record_host = with_shells(&reg, |store| {
        store.get(id1.as_str()).map(|r| r.host.clone())
    });
    assert_eq!(
        record_host,
        Some(crate::host::HostId::Named("web01".to_string())),
        "the record must carry the host the command was asked for"
    );

    // A second command on the same host with no `shell`: the reuse rule
    // must hand back the *same* shell, not open another one.
    let (id2, second) = run_hosted(
        &reg,
        &owner,
        "echo second-remote",
        None,
        crate::host::HostId::Named("web01".to_string()),
        spawn_in_process_host,
    )
    .await
    .expect("the reuse step resolves");
    assert_eq!(
        id2.as_str(),
        id1.as_str(),
        "the second command must reuse web01's shell, not open another"
    );
    assert_eq!(second.exit_code, 0, "the reused shell ran the command");

    // And the local rule is untouched by all of this: a third command on
    // the local host does not reach web01's shell — it opens its own.
    let (id3, third) = run_hosted(
        &reg,
        &owner,
        "echo local-run",
        None,
        crate::host::HostId::Local,
        spawn_in_process_host,
    )
    .await
    .expect("the local shell resolves");
    assert_ne!(
        id3.as_str(),
        id1.as_str(),
        "the local command must not be handed web01's remote shell"
    );
    let local_host = with_shells(&reg, |store| {
        store.get(id3.as_str()).map(|r| r.host.clone())
    });
    assert_eq!(
        local_host,
        Some(crate::host::HostId::Local),
        "the local command's record stays Local"
    );
    assert_eq!(third.exit_code, 0, "the local command ran");
}

/// Task 1 row 1 (preservation): with no `host` named, the command lands
/// on a `HostId::Local` record exactly as it did before hosts existed —
/// both for reuse and for a fresh shell.
#[tokio::test]
async fn run_terminal_command_without_a_host_stays_on_the_daemon_host() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let owner = Owner::Chat("local-run".into());

    let (id1, first) = run_in_shell_for_test(
        &reg,
        owner.clone(),
        "echo one",
        5,
        None,
        None,
        spawn_in_process_host,
    )
    .await
    .expect("the first command resolves");
    assert_eq!(first.exit_code, 0, "the first command ran");
    let host1 = with_shells(&reg, |store| {
        store.get(id1.as_str()).map(|r| r.host.clone())
    });
    assert_eq!(
        host1,
        Some(crate::host::HostId::Local),
        "the no-host reuse path keeps the local stamp"
    );

    let (id2, second) = run_in_shell_for_test(
        &reg,
        owner.clone(),
        "echo two",
        5,
        Some("new"),
        None,
        spawn_in_process_host,
    )
    .await
    .expect("the fresh shell resolves");
    assert_ne!(
        id2.as_str(),
        id1.as_str(),
        "shell: \"new\" opens its own shell"
    );
    let host2 = with_shells(&reg, |store| {
        store.get(id2.as_str()).map(|r| r.host.clone())
    });
    assert_eq!(
        host2,
        Some(crate::host::HostId::Local),
        "the no-host fresh-shell path keeps the local stamp"
    );
    assert_eq!(second.exit_code, 0, "the fresh shell ran the command");
}

/// Task 1's last row — the load-bearing negative: `shell: "s3"` naming a
/// shell on the **local** host while `host: "web01"` is requested is
/// refused, naming both hosts, and the command does **not** run in s3.
#[tokio::test]
async fn run_terminal_command_refuses_a_shell_whose_host_differs_from_the_named_host() {
    let (_home, dir) = crate::test_home();
    std::fs::write(
        dir.path().join(".daemoneye/etc/config.toml"),
        "[models.default]\nprovider = \"ollama\"\n\n[hosts.web01]\n\
             ssh = \"matt@web01.lan\"\n",
    )
    .expect("write config");
    let reg = ShellRegistry::default();
    let owner = Owner::Chat("mismatch".into());

    // s3 is opened **without** a host, so it is a local shell — the shell
    // whose relocation the row forbids.
    let (id, opened) = run_in_shell_for_test(
        &reg,
        owner.clone(),
        "echo opened",
        5,
        None,
        None,
        spawn_in_process_host,
    )
    .await
    .expect("the local shell opens");
    assert_eq!(opened.exit_code, 0, "the precondition command ran");

    let err = run_in_shell_for_test_on_host(
        &reg,
        owner.clone(),
        "echo must-not-run",
        5,
        Some(id.as_str()),
        None,
        crate::host::HostId::Named("web01".to_string()),
        spawn_in_process_host,
    )
    .await
    .expect_err("a shell on another host must be refused");
    let msg = err.to_string();
    assert!(
        msg.contains("local"),
        "the refusal must name the shell's real host: {msg}"
    );
    assert!(
        msg.contains("web01"),
        "the refusal must name the requested host: {msg}"
    );
    assert!(
        !msg.contains("never became ready") && !msg.contains("per-owner shell limit"),
        "a host mismatch is neither a readiness nor a limit problem: {msg}"
    );

    // The command did not run in the named shell: read the transcript with
    // a *different* command — the only `must-not-run` byte sequence a
    // re-read could contain would be this probe's own — and check the
    // shell's recorded command count instead. The refusal must have left
    // the shell with exactly the one command the precondition ran.
    let outcome = run_command(
        &socket_path(id.as_str()),
        "echo not-echoed-above",
        "bash",
        Duration::from_secs(10),
    )
    .await
    .expect("the shell is still live and answers");
    let recorded = String::from_utf8_lossy(&outcome.output).to_string();
    assert!(
        !recorded.contains("must-not-run"),
        "the refused command must not have run in the named shell: {recorded:?}"
    );
    let still_there = with_shells(&reg, |store| store.contains_key(id.as_str()));
    assert!(still_there, "a refused command must not close the shell");
}

/// Task 4's refusal, live: `shell: "<id>"` naming a shell that is busy
/// running `sleep 30` is refused **before any byte is written** — the
/// error names the shell and the running program, the `sleep` keeps
/// running, and the refused command's text never appears on the shell's
/// screen. A shell that was just interrupted can read `Running` for a
/// moment, so the run path re-probes; here the busy state is settled
/// before the call is made, so the refusal is the first answer.
#[tokio::test]
async fn run_terminal_command_refuses_a_busy_named_shell() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let owner = Owner::Chat("busy-named".into());
    let (id, _) = shell_for_owner(
        &reg,
        owner,
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
        if matches!(
            fg,
            Foreground::Program { ref cmd, .. } if cmd == "sleep"
        ) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the shell never read busy: {fg:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let err = run_in_shell_for_test(
        &reg,
        Owner::Chat("busy-named".into()),
        "echo BUSY_REFUSAL_MUST_NOT_RUN",
        5,
        Some(id.as_str()),
        None,
        spawn_in_process_host,
    )
    .await
    .expect_err("a busy named shell must be refused");
    let msg = format!("{err:#}");
    assert!(
        msg.contains(id.as_str()),
        "the refusal must name the shell id: {msg}"
    );
    assert!(
        msg.contains("running sleep"),
        "the refusal must name the running program: {msg}"
    );
    assert!(
        msg.contains("a command written there would be typed into it")
            && msg.contains("send_input")
            && msg.contains("shell: \"new\""),
        "the refusal must carry D5's remedy wording: {msg}"
    );

    // Nothing was written, so the refused command's text must never have
    // reached the shell: the status still reads `Running` and the screen
    // shows no trace of the command.
    let busy_socket = socket_path(id.as_str());
    let busy_status = crate::shell::status(&busy_socket, Duration::from_secs(5))
        .await
        .expect("the busy shell still answers status");
    let ShellResponse::Status { state, .. } = busy_status else {
        panic!("status must answer with a Status frame");
    };
    assert_eq!(
        state,
        crate::shell::ShellState::Running,
        "the sleep is still running after the refusal"
    );
    let screen_after = screen(&busy_socket, false, 0, Duration::from_secs(5))
        .await
        .expect("the busy shell still answers screen");
    let ShellResponse::ScreenText { text, .. } = screen_after else {
        panic!("screen must answer with a ScreenText frame");
    };
    assert!(
        !text.contains("BUSY_REFUSAL_MUST_NOT_RUN"),
        "the refused command must never have been typed into the shell: {text:?}"
    );

    // The refusal left the shell as it was: end the sleep and run the
    // command the ordinary way to prove the shell stayed usable.
    send_input(&busy_socket, b"\x03", Duration::from_secs(5))
        .await
        .expect("the shell accepts the cancel");
    let (ran_in, result) = run_in_shell_for_test(
        &reg,
        Owner::Chat("busy-named".into()),
        "echo AFTER_CANCEL_OK",
        5,
        Some(id.as_str()),
        None,
        spawn_in_process_host,
    )
    .await
    .expect("the interrupted shell is usable again");
    assert_eq!(
        ran_in.as_str(),
        id.as_str(),
        "the refusal must not have disturbed the shell"
    );
    assert!(
        result.text.contains("AFTER_CANCEL_OK"),
        "the command ran after the sleep was cancelled: {:?}",
        result.text
    );
}

/// Task 1's redundant-but-legal row: naming a `shell` whose host *is* the
/// requested one runs in that shell — the refusal must not be a blanket
/// one, or `shell` + `host` could never be combined at all.
#[tokio::test]
async fn run_terminal_command_with_a_matching_shell_and_host_is_not_an_error() {
    let (_home, dir) = crate::test_home();
    std::fs::write(
        dir.path().join(".daemoneye/etc/config.toml"),
        "[models.default]\nprovider = \"ollama\"\n\n[hosts.web01]\n\
             ssh = \"matt@web01.lan\"\n",
    )
    .expect("write config");
    let reg = ShellRegistry::default();
    let owner = Owner::Chat("match".into());

    let (id, opened) = run_in_shell_for_test_on_host(
        &reg,
        owner.clone(),
        "echo opened",
        5,
        None,
        None,
        crate::host::HostId::Named("web01".to_string()),
        spawn_in_process_host,
    )
    .await
    .expect("the remote shell opens");
    assert_eq!(opened.exit_code, 0, "the precondition command ran");

    let (got, result) = run_in_shell_for_test_on_host(
        &reg,
        owner.clone(),
        "echo matching",
        5,
        Some(id.as_str()),
        None,
        crate::host::HostId::Named("web01".to_string()),
        spawn_in_process_host,
    )
    .await
    .expect("a shell on the named host is a legal target");
    assert_eq!(
        got.as_str(),
        id.as_str(),
        "the named shell is the one that ran the command"
    );
    assert_eq!(result.exit_code, 0, "the command ran in it");
}
/// An unconfigured host is **not** refused by the run path any more: it
/// resolves through the permissive lookup to its own ssh destination,
/// behind the approval the caller has already given, and the shell it
/// opens carries the named host. Never a half-created shell on refusal —
/// the refusals that remain (`local`, empty-ssh) still fire before any
/// spawn.
#[tokio::test]
async fn run_terminal_command_an_unconfigured_host_resolves_to_its_own_destination() {
    let (_home, dir) = crate::test_home();
    std::fs::write(
        dir.path().join(".daemoneye/etc/config.toml"),
        "[models.default]\nprovider = \"ollama\"\n\n[hosts.web01]\n\
             ssh = \"matt@web01.lan\"\n",
    )
    .expect("write config");
    let reg = ShellRegistry::default();
    let owner = Owner::Chat("new-host".into());

    let (id, result) = run_in_shell_for_test_on_host(
        &reg,
        owner.clone(),
        "echo reachable",
        5,
        None,
        None,
        crate::host::HostId::Named("nope".to_string()),
        spawn_in_process_host,
    )
    .await
    .expect("an unconfigured host resolves behind its approval");
    assert_eq!(result.exit_code, 0, "the command ran in the shell");
    let record_host = with_shells(&reg, |store| store.get(id.as_str()).map(|r| r.host.clone()));
    assert_eq!(
        record_host,
        Some(crate::host::HostId::Named("nope".to_string())),
        "the record must carry the host the command was asked for"
    );
    // The destination the spawn reaches is the bare name — the same shape
    // a configured `[hosts.pinky] ssh = "pinky"` already has.
    let entry = crate::shell::host_entry_or_bare("nope").expect("the host resolves");
    assert_eq!(
        entry.ssh, "nope",
        "an unconfigured host becomes its own ssh destination"
    );
}

/// `host: "local"` behaves exactly as omitting the parameter: the same
/// reuse shell, the same local stamp. `HostId::parse` maps the word to
/// `Local` in any case, so this is the row that says the mapping reaches
/// the tool path, not just the parser.
#[tokio::test]
async fn run_terminal_command_host_local_is_the_daemon_host() {
    let (_home, _dir) = crate::test_home();

    let reg = ShellRegistry::default();
    let owner = Owner::Chat("local-word".into());

    let (id1, first) = run_in_shell_for_test(
        &reg,
        owner.clone(),
        "echo base",
        5,
        None,
        None,
        spawn_in_process_host,
    )
    .await
    .expect("the no-host shell opens");
    assert_eq!(first.exit_code, 0, "the precondition command ran");

    // The production path parses the name first (`mod.rs`), which is what
    // maps "local" to `Local`; the test mirrors that order rather than
    // handing `Named("local")` past the resolver, which is not a shape
    // production can produce.
    let host = crate::host::HostId::parse("local").expect("the word local parses");
    assert_eq!(host, crate::host::HostId::Local, "local maps to Local");
    let (id2, second) = run_in_shell_for_test_on_host(
        &reg,
        owner.clone(),
        "echo explicit",
        5,
        None,
        None,
        host,
        spawn_in_process_host,
    )
    .await
    .expect("host: \"local\" resolves");
    assert_eq!(
        id2.as_str(),
        id1.as_str(),
        "host: \"local\" reuses the same shell the omitted form would"
    );
    assert_eq!(second.exit_code, 0, "the command ran in it");
    let host2 = with_shells(&reg, |store| {
        store.get(id2.as_str()).map(|r| r.host.clone())
    });
    assert_eq!(
        host2,
        Some(crate::host::HostId::Local),
        "host: \"local\" stamps Local, not a name"
    );
}

#[tokio::test]
async fn read_shell_returns_clean_output_after_a_real_command() {
    let (_home, _dir) = crate::test_home();
    let reg = crate::shell::registry();
    let id = crate::shell::spawn_for_on_host(
        reg,
        Owner::Chat("read-shell-e2e".into()),
        5,
        crate::host::HostId::Local,
        false,
        spawn_in_logs_dir,
    )
    .await
    .expect("an ordinary shell spawns");
    let result = run_in_shell_for_test(
        reg,
        Owner::Chat("read-shell-e2e".into()),
        "printf 'SENTINEL_TWO\\n'",
        5,
        None,
        None,
        spawn_in_logs_dir,
    )
    .await
    .expect("the command resolves");
    assert_eq!(
        result.0.as_str(),
        id.as_str(),
        "the second call reuses that shell"
    );

    let out = crate::daemon::executor::knowledge::read_shell(id.as_str(), None, None, None);
    assert!(
        out.contains("SENTINEL_TWO"),
        "the command's output is recorded and readable: {out}"
    );
    assert!(
        !out.contains("DE_"),
        "no marker framing may leak into the recorded output: {out}"
    );
}

/// Each record's range is its own command's, not the whole cast. The check
/// is on the **index**, decoded with the raw `"o"` decoder and no windowing:
/// `read_shell` cannot see a too-wide left boundary, because the reader
/// re-windows the accumulated buffer and keeps only the last BEGIN..END, so
/// a producer anchoring every record at byte 0 would still read cleanly.
/// Decoding record 1's stored bytes directly is reader-independent and
/// deterministic: the first command's output is always on disk before the
/// second command is sent, so a range that reaches back must contain it.
#[tokio::test]
async fn read_shell_since_command_excludes_the_previous_commands_output() {
    let (_home, _dir) = crate::test_home();
    let reg = crate::shell::registry();
    let owner = Owner::Chat("since-command-e2e".into());
    let id = crate::shell::spawn_for(reg, owner.clone(), 5, false, spawn_in_logs_dir)
        .await
        .expect("an ordinary shell spawns");
    for cmd in ["printf 'SENTINEL_ALPHA\\n'", "printf 'SENTINEL_BETA\\n'"] {
        let (got, _) =
            run_in_shell_for_test(reg, owner.clone(), cmd, 5, None, None, spawn_in_logs_dir)
                .await
                .expect("the command resolves");
        assert_eq!(
            got.as_str(),
            id.as_str(),
            "both commands land in the one shell"
        );
    }
    let out = crate::daemon::executor::knowledge::read_shell(id.as_str(), None, Some(1), None);
    assert!(
        out.contains("SENTINEL_BETA"),
        "command 1's own output is readable from its index: {out}"
    );
    assert!(
        !out.contains("DE_"),
        "no marker framing may leak into the recorded output: {out}"
    );
    // The reader-independent half: record 1's stored bytes, decoded raw.
    let started = with_shells(reg, |st| {
        st.get(id.as_str()).map(|r| r.started_unix).unwrap_or(0)
    });
    let cast = crate::shell::resolve_cast(id.as_str(), started);
    let meta = MetaIndex::load(&meta_path_for(&cast)).expect("both commands were indexed");
    let rec = meta
        .commands
        .iter()
        .find(|c| c.index == 1)
        .expect("command 1 has a record");
    let bytes = std::fs::read(&cast).expect("the cast is on disk");
    let mut raw = Vec::new();
    for line in
        String::from_utf8_lossy(&bytes[rec.first_byte as usize..rec.end_byte as usize]).lines()
    {
        crate::shell::decode_payload(Some(line.to_owned()), &mut raw);
    }
    let raw = String::from_utf8_lossy(&raw);
    assert!(
        raw.contains("SENTINEL_BETA"),
        "record 1's range holds its own output: {raw:?}"
    );
    assert!(
        !raw.contains("SENTINEL_ALPHA"),
        "record 1's range must not reach back into command 0: {raw:?}"
    );
}

// ── shell-exit detection (M21 phase-08b) ─────────────────────────────────

/// The production spawn substitute whose program **exits immediately**:
/// `sh -c 'exit 0'` in place of `spawn_with_cast`'s bash — everything else
/// identical, so the run path sees a socket that answers status with
/// `Exited` while the PTY child is gone.
async fn spawn_with_exited_shell(id: ShellId) -> anyhow::Result<std::path::PathBuf> {
    let dir = tempfile::tempdir().context("tempdir for cast")?;
    let cast = dir.path().join(format!("{}.cast", id.as_str()));
    let backend = crate::shell::PtyBackend::spawn(
        "sh",
        &["-c".to_string(), "exit 0".to_string()],
        ROWS,
        COLS,
        100,
        &cast,
        0,
    )
    .context("spawning the exiting shell")?;
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

/// A dead shell's error, dressed exactly as the socket-close path leaves
/// it, must map to "exited before the command finished" — never to the
/// timeout wording the old inline closure gave it.
#[test]
fn socket_close_mid_command_reports_shell_exit() {
    let err = crate::shell::ShellExited {
        cmd: "sleep 60".to_string(),
    };
    let closed = anyhow::Error::new(err).context(
        "shell socket at /run/user/1000/daemoneye/shells/s9.sock closed while \
             running \"sleep 60\"",
    );
    let mapped = map_run_command_error(closed, Duration::from_secs(45), "s9");
    let text = format!("{mapped:#}");
    assert!(
        text.contains("exited before the command finished"),
        "the mapping says the shell exited: {text}"
    );
    assert!(
        !text.contains("exceeded"),
        "a closed socket exceeded no limit: {text}"
    );
    assert!(
        mapped
            .chain()
            .any(|c| c.downcast_ref::<crate::shell::ShellExited>().is_some()),
        "the typed error stays in the chain: {mapped:#?}"
    );

    let timeout_err = anyhow::anyhow!("timed out after 45s waiting for command output");
    let mapped = map_run_command_error(timeout_err, Duration::from_secs(45), "s9");
    let text = format!("{mapped:#}");
    assert!(
        text.contains("command exceeded the 45s limit in shell s9"),
        "a genuine timeout keeps its wording: {text}"
    );
}

/// An exited shell must be *reported as exited*, not dressed up as a
/// timeout: nothing exceeded a limit and nothing is still running. The
/// wording the model reads decides what it does next — retrying into a
/// dead shell.
#[tokio::test]
async fn an_exited_shell_is_reported_as_exited_not_as_a_timeout() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let owner = Owner::Chat("exited-e2e".into());
    let mut sink = tokio::io::sink();
    let mut eof = tokio::io::empty();
    let started = std::time::Instant::now();
    let err = run_in_shell_with_prompts(
        &reg,
        owner,
        "echo hi",
        5,
        None,
        Some(20),
        "",
        &mut sink,
        &mut eof,
        crate::host::HostId::Local,
        |id| async move { spawn_with_exited_shell(id).await.map(|_cast| ()) },
    )
    .await
    .expect_err("a dead shell must fail the run");
    let elapsed = started.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "the run must fail in seconds, not at its 20 s timeout: {elapsed:?}"
    );
    let msg = format!("{err:#}");
    assert!(
        msg.contains("exited"),
        "the failure must say the shell exited: {msg}"
    );
    assert!(
        !msg.contains("exceeded"),
        "a dead shell exceeded no limit: {msg}"
    );
}

/// Test 5: a command naming the privilege tool that **never prompts** is
/// not given up on — the old path classified on the shell's echo and
/// abandoned it with a false "could not be read" error.
#[tokio::test]
async fn a_password_prompt_that_never_appears_is_not_given_up() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let owner = Owner::Chat("never-prompt".into());
    let (id, result, _cast) = run_with_transport(
        &reg,
        owner,
        SILENT,
        "sudo true",
        &mut tokio::io::sink(),
        &mut tokio::io::empty(),
    )
    .await
    .expect("the never-prompting command resolves");
    assert_eq!(result.exit_code, 0, "the command ran cleanly");
    assert!(
        result.text.contains("NOPROMPT"),
        "the fake's output reaches the model: {:?}",
        result.text
    );
    assert!(
        id.as_str().starts_with('s'),
        "the run names its shell: {id:?}"
    );
}

/// Test 6: a correct password is answered **exactly once** — never again
/// by later ordinary output, which is what Defect B did: the extra
/// answers ran as shell commands in plaintext, and the cast held over a
/// dozen copies of the secret.
#[tokio::test]
async fn a_correct_password_is_sent_exactly_once() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let owner = Owner::Chat("answered-once".into());
    let (writes, mut reader) = answered_transport();
    let mut writer = RecordingWriter::new(Arc::clone(&writes));
    let outcome = run_with_transport(
        &reg,
        owner,
        PROMPTING,
        "sudo true; sleep 1; echo TAIL",
        &mut writer,
        &mut reader,
    )
    .await;
    if let Err(ref e) = outcome {
        let seen = writes.lock().expect("write log lock");
        panic!(
            "the answered run resolves: {e:#} — the transport saw {} writes: {:?}",
            seen.len(),
            *seen
        );
    }
    let (id, result, cast) = outcome.expect("checked above");
    assert_eq!(result.exit_code, 0, "the command ran cleanly: {result:?}");
    let text = result.text.clone();
    assert!(
        text.contains("GOT_15"),
        "the fake reports the answered length (15 = \"s3cret-answer-q\" + \\n): {text:?}"
    );
    assert!(
        text.contains("TAIL"),
        "the rest of the line ran after the prompt was answered: {text:?}"
    );
    assert!(
        id.as_str().starts_with('s'),
        "the run names its shell: {id:?}"
    );

    let raw = std::fs::read_to_string(&cast)
        .with_context(|| format!("reading the cast of shell {}", id.as_str()))
        .expect("the cast is readable");
    let secret_hits = raw.matches(CREDENTIAL).count();
    assert_eq!(
        secret_hits,
        0,
        "the credential must never reach the shell log: {secret_hits} copies in {}",
        cast.display()
    );
    assert_eq!(
        raw.matches("[redacted]").count(),
        1,
        "exactly one redacted entry — the answer: {raw}"
    );
    assert!(
        !raw.contains("command not found"),
        "a leaked answer runs as a shell command; none may appear: {raw}"
    );
}

/// Test 7: nobody attached (the ghost shape) → the prompt is given up
/// **fast and honestly**, and the same shell runs the next command.
#[tokio::test]
async fn an_unanswerable_password_prompt_gives_up_fast_and_leaves_the_shell_usable() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let owner = Owner::Chat("nobody-attached".into());
    let started = std::time::Instant::now();
    let (id, err) = match run_with_transport(
        &reg,
        owner.clone(),
        PROMPTING,
        "sudo true",
        &mut tokio::io::sink(),
        &mut tokio::io::empty(),
    )
    .await
    {
        Ok((spawned, _, _)) => {
            panic!("an unanswerable prompt must fail the run: shell {spawned:?}")
        }
        Err(e) => (None::<ShellId>, e),
    };
    let _ = id;
    let err = anyhow::anyhow!("{err:#}");
    // The failing run's error names its shell only in the context: pull it
    // out of the message so the next command can reuse the same shell.
    let id = ShellId::parse(
        err.to_string()
            .split("shell ")
            .nth(1)
            .and_then(|rest| rest.split(&[' ', '\'', ')'][..]).next())
            .expect("the failing run names its shell"),
    )
    .expect("the failing run names a valid shell");
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(5),
        "the give-up must be fast, not a timeout: {elapsed:?}"
    );
    let msg = format!("{err:#}");
    assert!(
        msg.contains("nobody is attached to answer it"),
        "the reason names the real problem: {msg}"
    );
    assert!(
        !msg.contains("did not complete within"),
        "a cancelled prompt did not wait out a limit: {msg}"
    );

    let next_started = std::time::Instant::now();
    let (same_id, next) = run_in_shell_with_prompts(
        &reg,
        owner,
        "echo AFTER",
        5,
        Some(id.as_str()),
        Some(10),
        "",
        &mut tokio::io::sink(),
        &mut tokio::io::empty(),
        crate::host::HostId::Local,
        move |_shell: ShellId| async move { Ok(()) },
    )
    .await
    .expect("the shell is still usable");
    let next_elapsed = next_started.elapsed();
    assert!(
        next_elapsed < Duration::from_secs(5),
        "the next command must not wait out its timeout: {next_elapsed:?}"
    );
    assert!(
        next.text.contains("AFTER"),
        "the next command ran: {:?}",
        next.text
    );
    assert!(
        same_id.as_str().starts_with('s'),
        "the reused shell is named: {same_id:?}"
    );
}

/// Test 8: a later command on the same line still sees the sentinel —
/// `export` (not a first-command-only prefix assignment) is what makes
/// the classification fire on the program that actually prompts.
#[tokio::test]
async fn a_later_command_on_the_line_still_gets_the_prompt_sentinel() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let owner = Owner::Chat("later-on-line".into());
    let (writes, mut reader) = answered_transport();
    let mut writer = RecordingWriter::new(Arc::clone(&writes));
    let (id, result, _cast) = run_with_transport(
        &reg,
        owner,
        PROMPTING,
        "cd /tmp && sudo true",
        &mut writer,
        &mut reader,
    )
    .await
    .expect("the later-on-line run resolves");
    assert_eq!(result.exit_code, 0, "the command ran cleanly: {result:?}");
    assert!(
        result.text.contains("GOT_15"),
        "the prompt on the later command was classified and answered: {:?}",
        result.text
    );
    assert!(
        id.as_str().starts_with('s'),
        "the run names its shell: {id:?}"
    );
}

/// M23 01b test 9: a user who answers **after** the command's timeout has
/// elapsed must not have the command killed as timed out — the wait for
/// the answer is added back to the deadline, so the command still gets
/// its own budget after the password was sent. The transport's answer
/// arrives 3 s after the prompt, past a 2 s timeout, and the run must
/// still complete with the fake's `GOT_15`.
/// Mutation A guard: with `deadline += asked.elapsed();` deleted in
/// `read_to_marker` this fails at "timed out after 2s".
#[tokio::test]
async fn an_answer_after_the_timeout_still_completes_the_command() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let owner = Owner::Chat("late-answer".into());
    let writes = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut writer = RecordingWriter::new(Arc::clone(&writes));
    // The "attached" read half whose answer is late: nothing for 3 s,
    // then one `CredentialResponse` for prompt id "p", then 30 s of
    // silence with the stream still open (the run must be answered, not
    // EOF'd).
    let (late_tx, late_rx) = tokio::io::duplex(4096);
    let mut reader = tokio::io::BufReader::new(late_rx);
    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        let mut late_tx = late_tx;
        tokio::time::sleep(Duration::from_secs(3)).await;
        let mut line = serde_json::to_string(&Request::CredentialResponse {
            id: "p".to_string(),
            credential: CREDENTIAL.to_string(),
        })
        .expect("CredentialResponse serialises");
        line.push('\n');
        late_tx
            .write_all(line.as_bytes())
            .await
            .expect("the late answer writes");
        tokio::time::sleep(Duration::from_secs(30)).await;
    });
    let mut write_sink = tokio::io::sink();
    let (id, _seed) = run_in_shell_with_prompts(
        &reg,
        owner.clone(),
        PROMPTING,
        5,
        None,
        Some(20),
        "",
        &mut write_sink,
        &mut tokio::io::empty(),
        crate::host::HostId::Local,
        |id| async move { spawn_with_cast(id).await.map(|_cast| ()) },
    )
    .await
    .context("the seed command runs")
    .expect("the seed command defines the fake");
    let outcome = run_in_shell_with_prompts(
        &reg,
        owner,
        "sudo true",
        5,
        Some(id.as_str()),
        Some(2),
        "p",
        &mut writer,
        &mut reader,
        crate::host::HostId::Local,
        move |_id: ShellId| async move { Ok(()) },
    )
    .await;
    if let Err(ref e) = outcome {
        let seen = writes.lock().expect("write log lock");
        panic!(
            "a late answer must still complete the command: {e:#} — the transport saw \
                 {} writes: {:?}",
            seen.len(),
            *seen
        );
    }
    let (_, result) = outcome.expect("checked above");
    assert_eq!(
        result.exit_code, 0,
        "the answered command ran cleanly: {result:?}"
    );
    assert!(
        result.text.contains("GOT_15"),
        "the fake reports the answered length after the late answer: {:?}",
        result.text
    );
}

/// M23 01b test 10: only the answer's wait is excluded — a command that
/// hangs **after** an instant answer still times out on schedule, so the
/// deadline extension cannot be read as an open-ended extension.
/// Mutation B guard: with `asked.elapsed()` replaced by
/// `Duration::from_secs(60)` the hang survives to its 30 s sleep instead
/// of timing out at 2 s, and the under-5-s assertion fails.
#[tokio::test]
async fn a_hang_after_an_answer_still_times_out() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let owner = Owner::Chat("hang-after-answer".into());
    let (writes, mut reader) = answered_transport();
    let mut writer = RecordingWriter::new(Arc::clone(&writes));
    let (id, _) = run_in_shell_with_prompts(
        &reg,
        owner.clone(),
        PROMPTING,
        5,
        None,
        Some(20),
        "",
        &mut tokio::io::sink(),
        &mut tokio::io::empty(),
        crate::host::HostId::Local,
        |id| async move { spawn_with_cast(id).await.map(|_cast| ()) },
    )
    .await
    .context("the seed command runs")
    .expect("the seed command defines the fake");
    let started = std::time::Instant::now();
    let outcome = run_in_shell_with_prompts(
        &reg,
        owner,
        "sudo true; sleep 30",
        5,
        Some(id.as_str()),
        Some(2),
        "p",
        &mut writer,
        &mut reader,
        crate::host::HostId::Local,
        move |_id: ShellId| async move { Ok(()) },
    )
    .await;
    let elapsed = started.elapsed();
    let err = match outcome {
        Ok((_, result)) => panic!(
            "a hang after an answer must time out, not complete: exit {} {:?}",
            result.exit_code, result.text
        ),
        Err(e) => e,
    };
    assert!(
        elapsed < Duration::from_secs(5),
        "the timeout must fire on the command's own budget: {elapsed:?}"
    );
    let msg = format!("{err:#}");
    assert!(
        msg.contains("timed out after 2s"),
        "the failure is the command's own 2 s limit, not the answer's wait: {msg}"
    );
}

/// Live test 11: the real privilege tool on `pinky` (sudo-rs 0.2.13) —
/// the only witness for the settle wait (Mutation D is green locally).
#[tokio::test]
#[ignore = "requires the configured host `pinky` reachable over ssh"]
async fn an_unanswerable_password_prompt_on_pinky_leaves_the_shell_usable() {
    let (_home, dir) = crate::test_home();
    std::fs::write(
        dir.path().join(".daemoneye/etc/config.toml"),
        "[models.default]\nprovider = \"ollama\"\n\n[hosts.pinky]\nssh = \"pinky\"\n",
    )
    .expect("write config");
    crate::config::ssh_dir();
    let reg = ShellRegistry::default();
    let owner = Owner::Chat("pinky-08f".into());
    // The spawn step: in-process, the real `ssh` argv with `-o
    // BatchMode=yes` inserted **before the destination** (the last argv
    // element — options after it become the remote command). In-process
    // rather than `spawn_detached` because a test binary cannot be
    // re-invoked as the shell host.
    let spawn = |id: ShellId| async move {
        let control = crate::config::ssh_dir();
        std::fs::create_dir_all(&control).context("creating the ssh control dir")?;
        let entry = crate::config::HostEntry {
            ssh: "pinky".into(),
            ..Default::default()
        };
        let mut argv = crate::host::ssh_argv(&entry, &control);
        let dest = argv.len() - 1;
        argv.insert(dest, "BatchMode=yes".to_string());
        argv.insert(dest, "-o".to_string());
        let program = crate::shell::which_ssh()?;
        let temp = tempfile::tempdir().context("tempdir for cast")?;
        let cast = temp.path().join(format!("{}.cast", id.as_str()));
        let backend = crate::shell::PtyBackend::spawn(&program, &argv, ROWS, COLS, 100, &cast, 0)
            .context("spawning ssh to pinky")?;
        let lock = crate::daemon::instance::InstanceLock::acquire(&lock_path(id.as_str()))
            .context("holding the per-id instance lock")?;
        let socket = socket_path(id.as_str());
        let listener = bind(&socket).await.context("binding the host's socket")?;
        tokio::spawn(serve(listener, Arc::clone(&backend)));
        std::mem::forget(lock);
        std::mem::forget(temp);
        Ok(())
    };
    let started = std::time::Instant::now();
    let (id, first) = run_in_shell_with_prompts(
        &reg,
        owner.clone(),
        "sudo -k",
        5,
        None,
        Some(20),
        "",
        &mut tokio::io::sink(),
        &mut tokio::io::empty(),
        crate::host::HostId::Named("pinky".to_string()),
        spawn,
    )
    .await
    .context("the never-prompting clear on pinky")
    .expect("the never-prompting clear on pinky resolves");
    let first_elapsed = started.elapsed();
    println!(
        "pinky `sudo -k`: {first_elapsed:?} (exit {})",
        first.exit_code
    );
    assert_eq!(first.exit_code, 0, "the clear ran cleanly");
    assert!(id.as_str().starts_with('s'), "the shell is named: {id:?}");

    let prompt_started = std::time::Instant::now();
    let mut sink = tokio::io::sink();
    let mut eof = tokio::io::empty();
    let err = run_in_shell_with_prompts(
        &reg,
        owner.clone(),
        "sudo whoami",
        5,
        Some(id.as_str()),
        Some(20),
        "",
        &mut sink,
        &mut eof,
        crate::host::HostId::Named("pinky".to_string()),
        spawn,
    )
    .await
    .expect_err("the unanswerable prompt must fail the run");
    let prompt_elapsed = prompt_started.elapsed();
    println!("pinky give-up: {prompt_elapsed:?}");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("nobody is attached"),
        "the reason names the real problem: {msg}"
    );
    assert!(
        prompt_elapsed < Duration::from_secs(5),
        "the give-up must be fast: {prompt_elapsed:?}"
    );

    let after_started = std::time::Instant::now();
    let (_, after) = run_in_shell_with_prompts(
        &reg,
        owner,
        "echo AFTER",
        5,
        Some(id.as_str()),
        Some(10),
        "",
        &mut tokio::io::sink(),
        &mut tokio::io::empty(),
        crate::host::HostId::Named("pinky".to_string()),
        spawn,
    )
    .await
    .expect("the shell is still usable");
    let after_elapsed = after_started.elapsed();
    println!("pinky next command: {after_elapsed:?}");
    assert!(
        after.text.contains("AFTER"),
        "the next command ran: {:?}",
        after.text
    );
    assert!(
        after_elapsed < Duration::from_secs(5),
        "the next command must not wait out its timeout: {after_elapsed:?}"
    );
}

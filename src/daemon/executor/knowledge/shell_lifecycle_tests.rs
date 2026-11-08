use super::*;

// ── open_shell ───────────────────────────────────────────────────────────

#[tokio::test]
async fn open_shell_returns_a_new_id() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let out = open_shell_with(
        &reg,
        &Owner::Chat("open".into()),
        crate::host::HostId::Local,
        5,
        Duration::from_millis(3000),
        spawn_in_process_host,
    )
    .await;
    let ids = with_shells(&reg, |store| {
        let mut ids: Vec<String> = store.values().map(|r| r.id.as_str().to_string()).collect();
        ids.sort();
        ids
    });
    assert_eq!(
        ids.len(),
        1,
        "one call must create exactly one record: {out}"
    );
    let id = &ids[0];
    assert!(
        out.contains(&format!("Opened shell {id}")),
        "the sentence must name the new id: {out}"
    );
    assert!(
        id.starts_with('s') && id[1..].chars().all(|c| c.is_ascii_digit()),
        "the id must be an sN shell id: {id}"
    );
    let dedicated = with_shells(&reg, |store| {
        store
            .values()
            .find(|r| r.id.as_str() == id)
            .map(|r| r.dedicated)
    });
    assert_eq!(
        dedicated,
        Some(false),
        "open_shell must create a non-dedicated shell — a dedicated one can \
             never be selected by the reuse rule"
    );
    assert!(
        out.contains("lowest-numbered"),
        "the reuse caveat must be in the sentence: {out}"
    );
}

#[tokio::test]
async fn open_shell_refuses_past_the_owner_cap() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    insert(&reg, 7, Owner::Chat("capped".into()), NOW);
    let ran: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
    let spawn = counting_spawn(ran, spawn_in_process_host);
    let out = open_shell_with(
        &reg,
        &Owner::Chat("capped".into()),
        crate::host::HostId::Local,
        1,
        Duration::from_millis(100),
        spawn,
    )
    .await;
    assert!(
        out.starts_with("Error:") && out.contains("cap"),
        "a breach is error text, not a panic and not an Err: {out}"
    );
    assert!(
        out.contains("limit is 1"),
        "the configured limit is named so the model can act: {out}"
    );
    assert_eq!(
        with_shells(&reg, |store| store.len()),
        1,
        "the pre-existing record is the only one"
    );
    assert!(
        !ran.load(Ordering::SeqCst),
        "the closure must not have been attempted"
    );
}

#[tokio::test]
async fn open_shell_removes_the_record_when_readiness_fails() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let out = open_shell_with(
        &reg,
        &Owner::Chat("stale".into()),
        crate::host::HostId::Local,
        5,
        Duration::from_millis(200),
        spawn_never_binds,
    )
    .await;
    assert!(
        out.starts_with("Error:") && out.contains("did not become ready"),
        "a readiness failure returns the error text: {out}"
    );
    assert_eq!(
        with_shells(&reg, |store| store.len()),
        0,
        "a host that never came up must not leave a record behind"
    );
}

// ── open_shell's host argument (M21 phase-04b) ───────────────────────────

/// § Current state 5 pinned as behaviour: a host problem is reported as a
/// host problem, never dressed up in per-owner shell-limit advice the
/// model cannot act on. `host_entry`'s refusal names the unknown host and
/// lists the configured ones; the cap sentence names neither.
#[tokio::test]
async fn open_shell_on_an_unknown_host_names_the_host_not_the_shell_limit() {
    let (_home, dir) = crate::test_home();
    std::fs::write(
        dir.path().join(".daemoneye/etc/config.toml"),
        "[models.default]\nprovider = \"ollama\"\n\n[hosts.web01]\n\
             ssh = \"matt@web01.lan\"\n\n[hosts.db01]\nssh = \"db01\"\n",
    )
    .expect("write config");
    let reg = ShellRegistry::default();
    let _swap = open_shell_test_seam::swap_in(&reg);
    let out = open_shell(&Owner::Chat("hosted".into()), Some("nope"), None, None).await;
    assert!(
        out.contains("nope"),
        "the refusal must name the host the model asked for: {out}"
    );
    assert!(
        out.contains("web01") && out.contains("db01"),
        "the refusal must list the configured hosts so the model can correct itself: {out}"
    );
    assert!(
        !out.contains("per-owner shell limit"),
        "a host problem must never arrive as shell-limit advice: {out}"
    );
    assert_eq!(
        with_shells(&reg, |store| store.len()),
        0,
        "a refused host must not leave a record behind"
    );
}

/// The host named at open time rides the record and reaches the model
/// again through `list_shells` — the plumbing's whole point.
#[tokio::test]
async fn open_shell_on_a_named_host_records_the_host() {
    let (_home, dir) = crate::test_home();
    std::fs::write(
        dir.path().join(".daemoneye/etc/config.toml"),
        "[models.default]\nprovider = \"ollama\"\n\n[hosts.web01]\n\
             ssh = \"matt@web01.lan\"\n",
    )
    .expect("write config");
    let reg = ShellRegistry::default();
    let out = open_shell_with(
        &reg,
        &Owner::Chat("web".into()),
        crate::host::HostId::Named("web01".to_string()),
        5,
        Duration::from_millis(3000),
        spawn_in_process_host,
    )
    .await;
    assert!(
        out.contains("Opened shell") && out.contains("web01"),
        "the precondition is a ready shell on web01: {out}"
    );
    let record_host = with_shells(&reg, |store| store.values().next().map(|r| r.host.clone()));
    assert_eq!(
        record_host,
        Some(crate::host::HostId::Named("web01".to_string())),
        "the record must carry the host open_shell_with was given"
    );
    let listed = list_shells_in(&reg, None, &Owner::Chat("web".into()), NOW);
    assert!(
        listed.contains("host:web01"),
        "list_shells must render the host so the model sees where the shell is: {listed}"
    );
}

/// No `host` naming is today's behaviour: the record stays stamped
/// `HostId::Local`, and because the renderer keeps the local token empty,
/// `list_shells` prints exactly as it did before hosts existed.
#[tokio::test]
async fn open_shell_without_a_host_records_the_local_host() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let _swap = open_shell_test_seam::swap_in(&reg);
    let out = open_shell(&Owner::Chat("local".into()), None, None, None).await;
    assert!(
        out.contains("Opened shell"),
        "the precondition is a ready local shell: {out}"
    );
    let record_host = with_shells(&reg, |store| store.values().next().map(|r| r.host.clone()));
    assert_eq!(
        record_host,
        Some(crate::host::HostId::Local),
        "the no-host form must keep the local stamp"
    );
    let listed = list_shells_in(&reg, None, &Owner::Chat("local".into()), NOW);
    assert!(
        !listed.contains("host:"),
        "an unhosted shell must render exactly as before hosts existed: {listed}"
    );
}

/// The no-host result sentence names the size the shell opened at. With
/// no `rows`/`cols` in the request and no `[shells]` table in
/// `lifecycle_home`'s config, that is the 03b default 40×120; the byte
/// contract now includes it.
#[tokio::test]
async fn open_shell_local_result_names_the_size() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let _swap = open_shell_test_seam::swap_in(&reg);
    let out = open_shell(&Owner::Chat("sentinel".into()), None, None, None).await;
    let id = out
        .split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_ascii_alphanumeric())
                .to_string()
        })
        .find(|w| w.starts_with('s') && w[1..].chars().all(|c| c.is_ascii_digit()))
        .expect("the success sentence names the id");
    assert_eq!(
        out,
        format!(
            "Opened shell {id}. It is now registered and readable — list_shells \
                 lists it, read_shell and find_in_shells read its recorded output, \
                 close_shell ends it. Its terminal is 40 rows × 120 columns. Note: \
                 a command still runs in this session's lowest-numbered open shell \
                 by default, so this shell sees work only when it is that one — \
                 pass shell: \"{id}\" to run_terminal_command to target it \
                 directly, or shell: \"new\" for one command's exclusive shell."
        ),
        "the no-host sentence names the size it opened at (40×120 here: no \
             request, no [shells] table)"
    );
}

/// A sized request resolves through `open_shell` the same way: the
/// sentence names the size the shell was opened at, `rows` first, with
/// the ` × ` between.
#[tokio::test]
async fn open_shell_names_the_requested_size() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let _swap = open_shell_test_seam::swap_in(&reg);
    let out = open_shell(&Owner::Chat("sized".into()), None, Some(50), Some(160)).await;
    assert!(
        out.contains("Opened shell"),
        "the precondition is a ready shell: {out}"
    );
    assert!(
        out.contains("Its terminal is 50 rows × 160 columns."),
        "the sentence names the requested size: {out}"
    );
}
/// A name outside `[A-Za-z0-9._-]` (or over 64 chars) is refused before
/// any spawn, with `HostId::parse`'s own message — not with shell-limit
/// advice.
#[tokio::test]
async fn open_shell_rejects_a_malformed_host_name_before_any_spawn() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let _swap = open_shell_test_seam::swap_in(&reg);
    let out = open_shell(&Owner::Chat("weird".into()), Some("bad name!"), None, None).await;
    assert!(
        out.starts_with("Error:"),
        "a malformed name is refused: {out}"
    );
    assert!(
        !out.contains("per-owner shell limit"),
        "a malformed name is a name problem, not a shell-limit problem: {out}"
    );
    assert_eq!(
        with_shells(&reg, |store| store.len()),
        0,
        "a refused name must not leave a record behind"
    );
}

/// Task 1's contract: the guard restores what `REPLACE` held *before* the
/// swap, so a nested swap's drop puts the outer swap back rather than
/// clearing to `None`.
///
/// `test_home_guard()` is here for its **lock**, not its `HOME` swap: the
/// `REPLACE` global below is shared with the five `open_shell` seam tests
/// (`#[test]`s run in parallel threads), and this test both reads it as a
/// precondition and mutates it twice. The other five serialize on
/// `crate::test_home()`, which bundles the same lock; this test needs no
/// `$HOME`, so it takes the lock by itself.
#[test]
fn swap_in_restores_the_previous_registry_on_drop() {
    let _home = crate::test_home_guard();
    let outer = ShellRegistry::default();
    let inner = ShellRegistry::default();
    assert!(
        open_shell_test_seam::swapped().is_none(),
        "precondition: no swap is in scope (this test must not depend on another)"
    );
    let _outer = open_shell_test_seam::swap_in(&outer);
    {
        let _inner = open_shell_test_seam::swap_in(&inner);
        assert!(
            open_shell_test_seam::swapped().is_some(),
            "the inner swap is in force while its guard lives"
        );
    }
    assert!(
        open_shell_test_seam::swapped().is_some(),
        "the inner drop restores the outer swap, it does not clear to none"
    );
    drop(_outer);
    assert!(
        open_shell_test_seam::swapped().is_none(),
        "the outer drop restores the empty state the test found"
    );
}

/// The case 04b's review named as the seam's latent flake: a caller that
/// goes through the public `open_shell` with **no** swap in scope must
/// resolve the daemon singleton — a fresh, empty registry — not whatever
/// an earlier test left in the seam. An unresolved (leaked or stale)
/// seam is what would make this test order-dependent.
#[tokio::test]
async fn open_shell_without_a_swap_uses_the_daemon_registry() {
    let (_home, _dir) = crate::test_home();
    assert!(
        open_shell_test_seam::swapped().is_none(),
        "precondition: no swap is in scope (this test must not depend on another)"
    );
    let out = open_shell(&Owner::Chat("no-swap".into()), None, None, None).await;
    assert!(
        out.contains("Opened shell"),
        "the precondition is a ready local shell: {out}"
    );
    // The record landed in the daemon singleton, and the seam is still
    // empty — nothing was installed.
    let singleton_ids = with_shells(crate::shell::registry(), |store| store.len());
    assert!(
        singleton_ids >= 1,
        "the record must land in the daemon registry, not a swapped one: {singleton_ids}"
    );
    assert!(
        open_shell_test_seam::swapped().is_none(),
        "the path must not install a registry of its own"
    );
}

// ── live host (ignored; run by hand) ─────────────────────────────────────

/// M21 phase-04b's end-to-end check: `open_shell { host: "pinky" }` really
/// runs on the remote host. requiring the live host `pinky` over ssh with
/// key auth (no prompt). It goes through `open_shell_with` — the tool
/// path, not a direct `ssh` — with the spawn step bound in-process
/// (the real `ssh` argv; `spawn_ssh_host` cannot run under a test binary
/// because `launch::spawn_detached` re-invokes `current_exe()`), then
/// drives the shell through the ordinary `run_terminal_command` protocol
/// and prints the four facts on one line.
#[tokio::test]
#[ignore = "requires the configured host `pinky` reachable over ssh"]
async fn open_shell_on_pinky_end_to_end() {
    let (_home, dir) = crate::test_home();
    std::fs::write(
        dir.path().join(".daemoneye/etc/config.toml"),
        "[models.default]\nprovider = \"ollama\"\n\n[hosts.pinky]\nssh = \"pinky\"\n",
    )
    .expect("write config");
    crate::config::ssh_dir();

    let reg = ShellRegistry::default();
    let owner = Owner::Chat("e2e-host".into());
    let out = open_shell_with(
        &reg,
        &owner,
        crate::host::HostId::Named("pinky".to_string()),
        5,
        Duration::from_secs(120),
        |id| async move { spawn_pinky_ssh(id).await },
    )
    .await;
    assert!(
        out.contains("Opened shell"),
        "the shell must open on pinky: {out}"
    );
    let id = out
        .split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_ascii_alphanumeric())
                .to_string()
        })
        .find(|w| w.starts_with('s') && w[1..].chars().all(|c| c.is_ascii_digit()))
        .expect("the success sentence names the id");

    let record_host = with_shells(&reg, |store| store.get(id.as_str()).map(|r| r.host.clone()));
    let record_host = match record_host {
        Some(crate::host::HostId::Named(name)) => name,
        other => panic!("the record must name pinky, got {other:?}"),
    };

    let listed = list_shells_in(&reg, None, &owner, NOW);
    assert!(
        listed.contains(&format!("host:{record_host}")),
        "list_shells must render the host: {listed}"
    );

    let hostname = run_command(
        &crate::shell::socket_path(&id),
        "hostname",
        &marker_shell_name(&crate::host::HostId::Named(record_host.clone())),
        Duration::from_secs(60),
    )
    .await
    .expect("the remote shell answers `hostname`");
    let hostname_out = String::from_utf8_lossy(&hostname.output).to_string();
    assert!(
        hostname_out.contains("pinky"),
        "`hostname` must say pinky: {hostname_out:?}"
    );

    let exit42 = run_command(
        &crate::shell::socket_path(&id),
        "(exit 42)",
        &marker_shell_name(&crate::host::HostId::Named(record_host.clone())),
        Duration::from_secs(60),
    )
    .await
    .expect("the remote shell answers `(exit 42)`");

    println!(
        "record_host={record_host} listed_host={record_host} remote_hostname={} remote_exit_code={}",
        hostname_out.trim().lines().last().unwrap_or(""),
        exit42.exit_code,
    );

    close_shell_with(&reg, &id, &owner, Duration::from_millis(3000)).await;
    let cp = crate::config::ssh_dir();
    let _ = std::process::Command::new("ssh")
        .args([
            "-O",
            "exit",
            "-o",
            &format!("ControlPath={}/%C", cp.display()),
            "pinky",
        ])
        .status();
}

/// `run_terminal_command`'s run path with the transport's write/read halves
/// bound to sink/EOF (no sudo) and the spawn step in-process over the real
/// `ssh` argv — the live-test shape [`open_shell_on_pinky_end_to_end`]
/// mirrors, but through the reuse-or-create step instead of `open_shell`.
async fn run_pinky_via_run_path(
    reg: &ShellRegistry,
    owner: Owner,
    cmd: &str,
) -> anyhow::Result<(ShellId, crate::daemon::executor::shell_exec::ShellRunResult)> {
    let mut sink = tokio::io::sink();
    let mut eof = tokio::io::empty();
    crate::daemon::executor::shell_exec::run_in_shell_with_prompts(
        reg,
        owner,
        cmd,
        5,
        None,
        Some(120),
        "",
        &mut sink,
        &mut eof,
        crate::host::HostId::Named("pinky".to_string()),
        |id| async move { spawn_pinky_ssh(id).await },
    )
    .await
}

/// M21 phase-04d's end-to-end check: `run_terminal_command { host: "pinky" }`
/// really runs on the remote host. Same preconditions as
/// [`open_shell_on_pinky_end_to_end`]: the live host `pinky` over ssh with
/// key auth (no prompt). The run path is [`run_in_shell_core`]'s host
/// parameter — the reuse-or-create step the tool drives — with the spawn
/// step bound in-process (the real `ssh` argv; `spawn_ssh_host` cannot run
/// under a test binary because `launch::spawn_detached` re-invokes
/// `current_exe()`), then the command's output and exit code are asserted
/// against the real remote and printed on one line.
#[tokio::test]
#[ignore = "requires the configured host `pinky` reachable over ssh"]
async fn run_terminal_command_on_pinky_end_to_end() {
    let (_home, dir) = crate::test_home();
    std::fs::write(
        dir.path().join(".daemoneye/etc/config.toml"),
        "[models.default]\nprovider = \"ollama\"\n\n[hosts.pinky]\nssh = \"pinky\"\n",
    )
    .expect("write config");
    crate::config::ssh_dir();

    let reg = ShellRegistry::default();
    let owner = Owner::Chat("e2e-rc-host".into());
    let (id, first) = run_pinky_via_run_path(&reg, owner.clone(), "hostname")
        .await
        .expect("the pinky run resolves");
    assert_eq!(first.exit_code, 0, "`hostname` ran cleanly on pinky");
    let hostname_out = first.text.trim().lines().last().unwrap_or("").to_string();
    assert!(
        hostname_out.contains("pinky"),
        "`hostname` must say pinky: {hostname_out:?}"
    );

    let record_host = with_shells(&reg, |store| store.get(id.as_str()).map(|r| r.host.clone()));
    assert_eq!(
        record_host,
        Some(crate::host::HostId::Named("pinky".to_string())),
        "the record must carry the host the command was asked for"
    );

    let (id2, second) = run_pinky_via_run_path(&reg, owner.clone(), "(exit 42)")
        .await
        .expect("the exit-42 run resolves");
    assert_eq!(id2.as_str(), id.as_str(), "the second command reused s*");
    assert_eq!(second.exit_code, 42, "the remote non-zero exit propagates");

    println!(
        "record_host=pinky reused={reused} remote_hostname={hostname_out} remote_exit_code={}",
        second.exit_code,
        reused = id2.as_str(),
    );

    close_shell_with(&reg, id.as_str(), &owner, Duration::from_millis(3000)).await;
    let cp = crate::config::ssh_dir();
    let _ = std::process::Command::new("ssh")
        .args([
            "-O",
            "exit",
            "-o",
            &format!("ControlPath={}/%C", cp.display()),
            "pinky",
        ])
        .status();
}

// ── close_shell ──────────────────────────────────────────────────────────

#[tokio::test]
async fn close_shell_ends_the_callers_shell() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let opened = open_shell_with(
        &reg,
        &Owner::Chat("closer".into()),
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
    let id = opened
        .split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_ascii_alphanumeric())
                .to_string()
        })
        .find(|w| w.starts_with('s') && w[1..].chars().all(|c| c.is_ascii_digit()))
        .expect("the success sentence names the id");

    // The signal is one-way and an idle bash ignores it, so the honest
    // outcome for a live host is the "still finishing" branch. Poll:
    // whichever of the two observation branches the tool reports, it must
    // be one of them and must name the id — never a bare `Ok` and never a
    // closure the host did not confirm. This measures the reporting
    // contract on the only signal this wire carries.
    let mut out = String::new();
    for _ in 0..3 {
        out = close_shell_with(
            &reg,
            &id,
            &Owner::Chat("closer".into()),
            Duration::from_millis(3000),
        )
        .await;
        if out.contains("Closed shell") || out.contains("Sent TERM") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        out.contains(&format!("shell {id}")),
        "either branch names the shell id: {out}"
    );
    if out.contains("Closed shell") {
        // Observed closure: the host must really be gone.
        assert!(
            !host_is_alive(&crate::shell::lock_path(&id)),
            "a 'Closed shell' answer must mean the lock is released: {out}"
        );
    } else {
        // The lingering branch: the report must not present the shell as
        // gone, and the host must still hold its lock — the exact
        // "possible still finishing" state the sentence describes.
        assert!(
            out.contains("Sent TERM") && out.contains("still"),
            "the lingering branch says what was sent and what remains: {out}"
        );
        assert!(
            host_is_alive(&crate::shell::lock_path(&id)),
            "the still-finishing answer must match a live host: {out}"
        );
    }
    // Either way the registry record stays: the reaper owns record
    // lifetime, close_shell never removes one.
    assert!(
        has_record(&reg, &id),
        "close_shell must not remove the record"
    );
}

#[tokio::test]
async fn close_shell_refuses_another_owners_shell() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    // Same inner string, different kind: the point of the `key()` check.
    insert(&reg, 4, Owner::Ghost("cross".into()), NOW);
    let out = close_shell_with(
        &reg,
        "s4",
        &Owner::Chat("cross".into()),
        Duration::from_millis(500),
    )
    .await;
    assert!(
        out.starts_with("Error:") && out.contains("s4"),
        "another kind's shell must be refused, the id named: {out}"
    );
    assert!(
        out.contains("not yours"),
        "the refusal sentence says whose shell it is: {out}"
    );
    assert!(
        has_record(&reg, "s4"),
        "a refused close must not have removed the record"
    );
}

#[tokio::test]
async fn close_shell_unknown_id_is_an_error() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let out = close_shell_with(
        &reg,
        "s99",
        &Owner::Chat("anon".into()),
        Duration::from_millis(500),
    )
    .await;
    assert!(
        out.starts_with("Error:"),
        "an unknown id is error text: {out}"
    );
    assert!(out.contains("s99"), "the unknown id is named: {out}");
    assert!(
        out.contains("list_shells"),
        "the model is told the way to get a fresh id: {out}"
    );
    assert_eq!(
        out,
        unknown_shell_error(&reg, "s99"),
        "close_shell words the refusal as every shell tool does (M31 #72)"
    );
}

/// 08c Task 4.1: closing an owner's shells ends **only** that owner's
/// shells. The hosts run `sh -c 'sleep 1000'`, not bash — an interactive
/// bash ignores TERM (measured at drafting, § Current state 3), so a
/// bash host would not report `Exited` on either side and the test would
/// pin nothing.
#[tokio::test]
async fn closing_an_owners_shells_ends_only_that_owners_shells() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let spawn_sleeping_host = |id: ShellId| async move {
        let dir = tempfile::tempdir().context("tempdir for cast")?;
        let cast = dir.path().join(format!("{}.cast", id.as_str()));
        let backend = crate::shell::PtyBackend::spawn(
            "sh",
            &["-c".to_string(), "sleep 1000".to_string()],
            TEST_ROWS,
            TEST_COLS,
            100,
            &cast,
            0,
        )
        .context("spawning sh -c 'sleep 1000'")?;
        let lock =
            crate::daemon::instance::InstanceLock::acquire(&crate::shell::lock_path(id.as_str()))
                .context("holding the per-id instance lock")?;
        let socket = socket_path(id.as_str());
        let listener = bind(&socket).await.context("binding the host's socket")?;
        tokio::spawn(serve(listener, std::sync::Arc::clone(&backend)));
        std::mem::forget(lock);
        std::mem::forget(dir);
        Ok(())
    };
    let owner_a = Owner::Ghost("a".into());
    let owner_b = Owner::Ghost("b".into());
    let id_a = spawn_for_on_host(
        &reg,
        owner_a.clone(),
        5,
        crate::host::HostId::Local,
        false,
        spawn_sleeping_host,
    )
    .await
    .expect("a's shell spawns");
    let id_b = spawn_for_on_host(
        &reg,
        owner_b.clone(),
        5,
        crate::host::HostId::Local,
        false,
        spawn_sleeping_host,
    )
    .await
    .expect("b's shell spawns");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    await_ready(&socket_path(id_a.as_str()), deadline)
        .await
        .expect("a's host is ready");
    await_ready(&socket_path(id_b.as_str()), deadline)
        .await
        .expect("b's host is ready");

    let closed = close_owner_shells(&reg, &owner_a, Duration::from_secs(3)).await;
    assert_eq!(
        closed.len(),
        1,
        "exactly one shell belongs to ghost:a: {closed:?}"
    );
    assert_eq!(
        closed[0].0,
        id_a.as_str(),
        "the closed shell is a's, not b's: {closed:?}"
    );

    let mut a_exited = false;
    let mut b_exited = false;
    for _ in 0..40 {
        let state_a = status(&socket_path(id_a.as_str()), Duration::from_millis(100)).await;
        let state_b = status(&socket_path(id_b.as_str()), Duration::from_millis(100)).await;
        a_exited = a_exited
            || matches!(state_a, Ok(ShellResponse::Exited { .. }))
            || matches!(&state_a,
                    Ok(ShellResponse::Status { state, .. }) if *state == ShellState::Exited);
        b_exited = b_exited
            || matches!(state_b, Ok(ShellResponse::Exited { .. }))
            || matches!(&state_b,
                    Ok(ShellResponse::Status { state, .. }) if *state == ShellState::Exited);
        if a_exited {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(a_exited, "a's shell reports Exited within 2 s of the close");
    assert!(!b_exited, "b's shell must not be touched by a's teardown");
}

// ── detach-close (M36 phase-17a) ─────────────────────────────────────────

/// A daemon-free `close_after_detach` exercise: the hosts are spawned
/// directly by `spawn_for_on_host`, so the assertions read the same locks a
/// real close observes. The shells run `sh -c 'sleep 1000'`, not bash — an
/// interactive bash ignores TERM (measured at M25 drafting, § Current state
/// 3), so a bash host would not report `Exited` and the hangup would pin
/// nothing.
async fn detach_close_spawn(id: ShellId) -> anyhow::Result<()> {
    let dir = tempfile::tempdir().context("tempdir for cast")?;
    let cast = dir.path().join(format!("{}.cast", id.as_str()));
    let backend = crate::shell::PtyBackend::spawn(
        "sh",
        &["-c".to_string(), "sleep 1000".to_string()],
        TEST_ROWS,
        TEST_COLS,
        100,
        &cast,
        0,
    )
    .context("spawning sh -c 'sleep 1000'")?;
    let lock =
        crate::daemon::instance::InstanceLock::acquire(&crate::shell::lock_path(id.as_str()))
            .context("holding the per-id instance lock")?;
    let socket = socket_path(id.as_str());
    let listener = bind(&socket).await.context("binding the host's socket")?;
    // Like a real host process: serve until the shell exits, then release
    // the lock — the test's `host_is_alive` reads false only once this task
    // has ended, and `serve` alone never returns.
    tokio::spawn(async move {
        let _ =
            crate::shell::serve_until_exit(listener, backend, std::time::Duration::from_millis(50))
                .await;
        drop(lock);
    });
    std::mem::forget(dir);
    Ok(())
}

#[tokio::test]
async fn last_detach_closes_the_sessions_shells_after_the_grace() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let owner = Owner::Chat("detach-1".into());
    let id_a = spawn_for_on_host(
        &reg,
        owner.clone(),
        5,
        crate::host::HostId::Local,
        false,
        detach_close_spawn,
    )
    .await
    .expect("shell a spawns");
    let id_b = spawn_for_on_host(
        &reg,
        owner.clone(),
        5,
        crate::host::HostId::Local,
        false,
        detach_close_spawn,
    )
    .await
    .expect("shell b spawns");
    assert!(host_is_alive(&crate::shell::lock_path(id_a.as_str())));
    assert!(host_is_alive(&crate::shell::lock_path(id_b.as_str())));

    close_after_detach(&reg, "detach-1", Duration::from_millis(100)).await;

    assert!(
        !host_is_alive(&crate::shell::lock_path(id_a.as_str())),
        "shell a's host is gone after the grace with no presence"
    );
    assert!(
        !host_is_alive(&crate::shell::lock_path(id_b.as_str())),
        "shell b's host is gone after the grace with no presence"
    );
}

#[tokio::test]
async fn a_reattach_within_the_grace_keeps_the_shells() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let owner = Owner::Chat("detach-2".into());
    let id = spawn_for_on_host(
        &reg,
        owner.clone(),
        5,
        crate::host::HostId::Local,
        false,
        detach_close_spawn,
    )
    .await
    .expect("the shell spawns");

    // The session re-attaches before the grace ends: the presence count is
    // positive when the grace expires, so nothing is closed.
    let (push_tx, _push_rx) = tokio::sync::mpsc::unbounded_channel();
    let conn = crate::daemon::session::PresenceConnection { push: push_tx };
    crate::daemon::session::presence_register("detach-2", conn.clone());
    assert_eq!(crate::daemon::session::presence_count("detach-2"), 1);

    close_after_detach(&reg, "detach-2", Duration::from_millis(300)).await;

    assert!(
        host_is_alive(&crate::shell::lock_path(id.as_str())),
        "a re-attach within the grace must leave the session's shell alone"
    );
    assert!(has_record(&reg, id.as_str()));
    let _ = crate::daemon::session::presence_unregister("detach-2", &conn);
}

#[tokio::test]
async fn detach_close_leaves_other_owners_and_dedicated_shells() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let session = Owner::Chat("detach-3".into());
    let user = Owner::User;

    let session_id = spawn_for_on_host(
        &reg,
        session.clone(),
        5,
        crate::host::HostId::Local,
        false,
        detach_close_spawn,
    )
    .await
    .expect("the session's shell spawns");
    let dedicated_id = spawn_for_on_host(
        &reg,
        session.clone(),
        5,
        crate::host::HostId::Local,
        true,
        detach_close_spawn,
    )
    .await
    .expect("the dedicated shell spawns");
    let user_id = spawn_for_on_host(
        &reg,
        user.clone(),
        5,
        crate::host::HostId::Local,
        false,
        detach_close_spawn,
    )
    .await
    .expect("the user shell spawns");
    let other_id = spawn_for_on_host(
        &reg,
        Owner::Chat("detach-other".into()),
        5,
        crate::host::HostId::Local,
        false,
        detach_close_spawn,
    )
    .await
    .expect("the other session's shell spawns");

    close_after_detach(&reg, "detach-3", Duration::from_millis(100)).await;

    assert!(
        !host_is_alive(&crate::shell::lock_path(session_id.as_str())),
        "the session's own non-dedicated shell is closed"
    );
    for (id, why) in [
        (
            dedicated_id.as_str(),
            "a dedicated shell carries a background run",
        ),
        (
            user_id.as_str(),
            "an Owner::User shell is not this path's target",
        ),
        (
            other_id.as_str(),
            "another session's shell is not this path's target",
        ),
    ] {
        assert!(
            host_is_alive(&crate::shell::lock_path(id)),
            "{why}: it must survive the session's detach"
        );
    }
}

// ── idle-reap (M36 phase-17c) ────────────────────────────────────────────

/// The probe timeout this section's live reads use.
#[cfg(test)]
const REAP_PROBE_TIMEOUT: Duration = crate::shell::PROBE_TIMEOUT;

/// A shell host whose recording the test controls — and whose `.cast` lands
/// under the **test HOME**'s `var/log/shells/` (the directory the real host
/// uses and the one `resolve_cast` scans), so the reap reads the same file
/// the host is writing. `lifecycle_test_support::spawn_with_cast` homes the
/// cast in an *unrelated* tempdir, so this section's own host is built here
/// instead: same bind/serve/leak shape, cast path `cast_path(id, started)`.
#[cfg(test)]
async fn idle_reap_spawn(id: ShellId) -> anyhow::Result<()> {
    use anyhow::Context as _;
    let started = crate::shell::now_unix();
    let cast = crate::shell::cast_path(id.as_str(), started);
    std::fs::create_dir_all(crate::config::shell_logs_dir())
        .context("creating the test cast dir")?;
    // `sh -c 'sleep 1000'`, not interactive bash (bug-17c-1): the close that
    // ends this shell sends HUP, and if the host's close ack races the
    // 5 s signal timeout the client falls back to TERM — which a live
    // interactive bash ignores, so the host would sit at its prompt holding
    // the lock past any bounded wait. A non-interactive `sh` child ends on
    // **both** signals, so the hangup-to-lock-released arc is bounded no
    // matter which signal the wire carried. The record still classifies
    // `Prompt` (the foreground group is the child's own), so every
    // precondition the section asserts is unchanged.
    let backend = crate::shell::PtyBackend::spawn(
        "sh",
        &["-c".to_string(), "sleep 1000".to_string()],
        lifecycle_test_support::ROWS,
        lifecycle_test_support::COLS,
        100,
        &cast,
        started,
    )
    .context("spawning sh -c 'sleep 1000'")?;
    let lock =
        crate::daemon::instance::InstanceLock::acquire(&crate::shell::lock_path(id.as_str()))
            .context("holding the per-id instance lock")?;
    let socket = crate::shell::socket_path(id.as_str());
    let listener = crate::shell::bind(&socket)
        .await
        .context("binding the in-process host's socket")?;
    tokio::spawn(async move {
        let _ = crate::shell::serve_until_exit(
            listener,
            backend,
            std::time::Duration::from_millis(100),
        )
        .await;
        drop(lock);
    });
    std::mem::forget(cast);
    Ok(())
}

/// `probe_foreground` through the same timeout every live read in this file
/// uses, so a test's precondition reads the same way the reap's own probe would.
#[cfg(test)]
async fn reap_probe(reg: &ShellRegistry, id: &ShellId) -> Foreground {
    crate::shell::probe_foreground(reg, id, REAP_PROBE_TIMEOUT).await
}

/// Type a `sleep 60` at `id`'s prompt and wait until the probe reads it as a
/// running program — the not-at-a-prompt precondition the skip test pins.
#[cfg(test)]
async fn reap_busy_until_running(reg: &ShellRegistry, id: &ShellId) -> Foreground {
    let socket = crate::shell::socket_path(id.as_str());
    crate::shell::send_input(&socket, b"sleep 60\n", Duration::from_secs(5))
        .await
        .expect("the shell accepts input");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let fg = reap_probe(reg, id).await;
        if matches!(fg, Foreground::Program { .. }) {
            return fg;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the shell never read busy: {fg:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// End the `sleep 60` a test typed, with the byte that cancels it.
#[cfg(test)]
async fn reap_cancel_busy(id: &ShellId) {
    let socket = crate::shell::socket_path(id.as_str());
    crate::shell::send_input(&socket, b"\x03", Duration::from_secs(5))
        .await
        .expect("the shell accepts the cancel");
}

/// Set the idle precondition explicitly: backdate the shell's cast mtime far
/// past any limit the reap is asked to judge, so a test's "idle" is a fact
/// written here, never a guess from spawn timing (bug-17c-1).
#[cfg(test)]
fn reap_backdate_cast(id: &ShellId, started: u64, seconds_old: u64) {
    let cast = crate::shell::resolve_cast(id.as_str(), started);
    let f = std::fs::OpenOptions::new()
        .write(true)
        .open(&cast)
        .expect("the host's cast exists");
    let old = std::time::SystemTime::now() - Duration::from_secs(seconds_old);
    f.set_times(std::fs::FileTimes::new().set_modified(old))
        .expect("backdate the cast's mtime");
}

#[tokio::test]
async fn reap_idle_closes_a_silent_adopted_shell_at_its_prompt() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let id = spawn_for_on_host(
        &reg,
        Owner::Adopted,
        5,
        crate::host::HostId::Local,
        false,
        idle_reap_spawn,
    )
    .await
    .expect("the adopted shell spawns");

    let prompt = reap_probe(&reg, &id).await;
    assert_eq!(prompt, Foreground::Prompt, "the precondition is a prompt");

    let started = with_shells(&reg, |store| store.get(id.as_str()).map(|r| r.started_unix))
        .expect("the record exists");
    reap_backdate_cast(&id, started, 300);
    let closed =
        crate::daemon::executor::knowledge::shell::reap_idle_shells(&reg, 30, u64::MAX).await;
    // `u64::MAX` makes the idle window unbounded: the backdated mtime is the
    // only condition that can hold the shell open here, which is the bug
    // report's point — the idle precondition is a fact this test wrote, not a
    // guess from spawn timing.

    assert!(
        closed.contains(&id.as_str().to_owned()),
        "a silent adopted shell at its prompt must be reaped: {closed:?}"
    );
    reap_host_gone_budget(&id, "a reaped shell's host must really be gone").await;
}

#[tokio::test]
async fn reap_idle_skips_a_user_shell() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let id = spawn_for_on_host(
        &reg,
        Owner::User,
        5,
        crate::host::HostId::Local,
        false,
        idle_reap_spawn,
    )
    .await
    .expect("the user shell spawns");

    let prompt = reap_probe(&reg, &id).await;
    assert_eq!(prompt, Foreground::Prompt, "the precondition is a prompt");

    let started = with_shells(&reg, |store| store.get(id.as_str()).map(|r| r.started_unix))
        .expect("the record exists");
    reap_backdate_cast(&id, started, 300);
    let now = crate::shell::now_unix();
    let closed = crate::daemon::executor::knowledge::shell::reap_idle_shells(&reg, 30, now).await;

    assert!(
        !closed.contains(&id.as_str().to_owned()),
        "the user's /shell shells are never reaped: {closed:?}"
    );
    assert!(
        host_is_alive(&crate::shell::lock_path(id.as_str())),
        "the user shell's host must survive the reap"
    );
}

#[tokio::test]
async fn reap_idle_skips_a_shell_with_a_command_in_flight() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let id = spawn_for_on_host(
        &reg,
        Owner::Chat("reap-flying".into()),
        5,
        crate::host::HostId::Local,
        false,
        idle_reap_spawn,
    )
    .await
    .expect("the shell spawns");

    let prompt = reap_probe(&reg, &id).await;
    assert_eq!(prompt, Foreground::Prompt, "the precondition is a prompt");
    let started = with_shells(&reg, |store| store.get(id.as_str()).map(|r| r.started_unix))
        .expect("the record exists");
    reap_backdate_cast(&id, started, 300);
    crate::shell::set_in_flight(&reg, &id, "true", crate::shell::now_unix());
    let now = crate::shell::now_unix();
    let closed = crate::daemon::executor::knowledge::shell::reap_idle_shells(&reg, 30, now).await;

    assert!(
        !closed.contains(&id.as_str().to_owned()),
        "a shell with an in-flight command is never reaped: {closed:?}"
    );
    assert!(
        host_is_alive(&crate::shell::lock_path(id.as_str())),
        "the in-flight shell's host must survive the reap"
    );
}

#[tokio::test]
async fn reap_idle_skips_a_present_chat_sessions_shell() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let session = "reap-present";
    let id = spawn_for_on_host(
        &reg,
        Owner::Chat(session.into()),
        5,
        crate::host::HostId::Local,
        false,
        idle_reap_spawn,
    )
    .await
    .expect("the shell spawns");

    let conn = crate::daemon::session::PresenceConnection::test_conn();
    crate::daemon::session::presence_register(session, conn.clone());
    let prompt = reap_probe(&reg, &id).await;
    assert_eq!(prompt, Foreground::Prompt, "the precondition is a prompt");
    let started = with_shells(&reg, |store| store.get(id.as_str()).map(|r| r.started_unix))
        .expect("the record exists");
    reap_backdate_cast(&id, started, 300);
    let now = crate::shell::now_unix();
    let closed = crate::daemon::executor::knowledge::shell::reap_idle_shells(&reg, 30, now).await;

    assert!(
        !closed.contains(&id.as_str().to_owned()),
        "a present session's shell is never reaped: {closed:?}"
    );
    assert!(
        host_is_alive(&crate::shell::lock_path(id.as_str())),
        "the present session's shell must survive the reap"
    );
    let _ = crate::daemon::session::presence_unregister(session, &conn);
}

#[tokio::test]
async fn reap_idle_closes_an_absent_chat_sessions_shell() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let session = "reap-absent";
    let id = spawn_for_on_host(
        &reg,
        Owner::Chat(session.into()),
        5,
        crate::host::HostId::Local,
        false,
        idle_reap_spawn,
    )
    .await
    .expect("the shell spawns");

    let prompt = reap_probe(&reg, &id).await;
    assert_eq!(prompt, Foreground::Prompt, "the precondition is a prompt");

    let started = with_shells(&reg, |store| store.get(id.as_str()).map(|r| r.started_unix))
        .expect("the record exists");
    reap_backdate_cast(&id, started, 300);
    let now = crate::shell::now_unix();
    let closed = crate::daemon::executor::knowledge::shell::reap_idle_shells(&reg, 30, now).await;

    assert!(
        closed.contains(&id.as_str().to_owned()),
        "a silent absent chat shell at its prompt must be reaped: {closed:?}"
    );
    // The close is synchronous — it hung up the session's PTY. But the *host
    // process* ends on its own after the hangup, and this test's host is
    // in-process. Its serve loop ends on its own, but the test cannot know
    // when; what the reap must guarantee is the registry record and its
    // hangup, which `closed` names.
    // The absence check that really matters lives below.
    reap_host_gone_budget(&id, "a reaped chat shell's host must really be gone").await;
}

/// A reaped shell's host must end on its own. In-process test hosts release
/// their per-id lock as `serve_until_exit` returns, which happens soon after
/// the hangup but on the host's own schedule, so poll — never silently: the
/// assert names the shell and what it waited for. The budget is generous
/// (bug-17c-1): a bash exits on SIGHUP within milliseconds, but an
/// intermittent 6–8 s delay between the HUP and the PTY child's reaping was
/// observed under parallel test load, and a short budget turned that
/// environment slowness into a false failure.
#[cfg(test)]
async fn reap_host_gone_budget(id: &ShellId, msg: &str) {
    let lock = crate::shell::lock_path(id.as_str());
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while std::time::Instant::now() < deadline {
        if !host_is_alive(&lock) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{msg}: shell {}'s lock still held after 20s", id.as_str());
}

#[tokio::test]
async fn reap_idle_skips_a_recently_written_shell() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let id = spawn_for_on_host(
        &reg,
        Owner::Adopted,
        5,
        crate::host::HostId::Local,
        false,
        idle_reap_spawn,
    )
    .await
    .expect("the shell spawns");

    let prompt = reap_probe(&reg, &id).await;
    assert_eq!(prompt, Foreground::Prompt, "the precondition is a prompt");

    let started = with_shells(&reg, |store| store.get(id.as_str()).map(|r| r.started_unix))
        .expect("the record exists");
    // Not idle: the cast the host is writing still bears (at worst) its spawn
    // instant, well inside the 5 s the reap is asked to judge.
    let touched = std::fs::OpenOptions::new()
        .write(true)
        .open(crate::shell::resolve_cast(id.as_str(), started))
        .expect("the cast the spawn wrote");
    touched
        .set_times(std::fs::FileTimes::new().set_modified(std::time::SystemTime::now()))
        .expect("freshen the cast's mtime");
    drop(touched);

    let now = crate::shell::now_unix();
    let closed =
        crate::daemon::executor::knowledge::shell::reap_idle_shells(&reg, 5, now + 1).await;

    assert!(
        closed.is_empty(),
        "a shell whose recording moved seconds ago is not idle: {closed:?}"
    );
    assert!(
        host_is_alive(&crate::shell::lock_path(id.as_str())),
        "a not-idle shell's host must survive the reap"
    );
}

#[tokio::test]
async fn reap_idle_skips_a_shell_not_at_its_prompt() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    // A bash host, not the section's idle `sh -c 'sleep 1000'` one: the
    // precondition is a *typed foreground program* (`sleep 60`) holding the
    // terminal, which only an interactive shell can run — and an interactive
    // bash survives this test's own ^C doodle, unlike the close path the
    // closing tests exercise.
    let id = spawn_for_on_host(
        &reg,
        Owner::Adopted,
        5,
        crate::host::HostId::Local,
        false,
        |id| async move {
            let started = crate::shell::now_unix();
            let cast = crate::shell::cast_path(id.as_str(), started);
            std::fs::create_dir_all(crate::config::shell_logs_dir())
                .context("creating the test cast dir")?;
            let backend = crate::shell::PtyBackend::spawn(
                "bash",
                &[],
                lifecycle_test_support::ROWS,
                lifecycle_test_support::COLS,
                100,
                &cast,
                started,
            )
            .context("spawning bash")?;
            let lock = crate::daemon::instance::InstanceLock::acquire(&crate::shell::lock_path(
                id.as_str(),
            ))
            .context("holding the per-id instance lock")?;
            let socket = crate::shell::socket_path(id.as_str());
            let listener = crate::shell::bind(&socket)
                .await
                .context("binding the in-process host's socket")?;
            tokio::spawn(async move {
                let _ = crate::shell::serve_until_exit(
                    listener,
                    backend,
                    std::time::Duration::from_millis(100),
                )
                .await;
                drop(lock);
            });
            std::mem::forget(cast);
            Ok(())
        },
    )
    .await
    .expect("the shell spawns");

    // A foreground program (here `sleep 60`) holds the terminal until the
    // probe reads it as `Program { .. }`. The pending `sleep 60` survives the
    // reap's CLOSE (the ^C row cancels it), so a wait afterward boxes it in.
    let fg = reap_busy_until_running(&reg, &id).await;
    assert!(
        matches!(fg, Foreground::Program { .. }),
        "the precondition is a running program, got {fg:?}"
    );

    let started = with_shells(&reg, |store| store.get(id.as_str()).map(|r| r.started_unix))
        .expect("the record exists");
    reap_backdate_cast(&id, started, 300);
    let now = crate::shell::now_unix();
    let closed = crate::daemon::executor::knowledge::shell::reap_idle_shells(&reg, 30, now).await;

    assert!(
        !closed.contains(&id.as_str().to_owned()),
        "a shell running a program is never reaped, silent or not: {closed:?}"
    );
    assert!(
        host_is_alive(&crate::shell::lock_path(id.as_str())),
        "the shell's host must survive the reap"
    );
    reap_cancel_busy(&id).await;
}

#[tokio::test]
async fn reap_idle_does_nothing_when_idle_secs_is_zero() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let id = spawn_for_on_host(
        &reg,
        Owner::Adopted,
        5,
        crate::host::HostId::Local,
        false,
        idle_reap_spawn,
    )
    .await
    .expect("the shell spawns");
    let prompt = reap_probe(&reg, &id).await;
    assert_eq!(prompt, Foreground::Prompt, "the precondition is a prompt");

    let closed =
        crate::daemon::executor::knowledge::shell::reap_idle_shells(&reg, 0, u64::MAX).await;
    assert!(
        closed.is_empty(),
        "0 = never reap, whatever the shell looks like: {closed:?}"
    );
    assert!(
        host_is_alive(&crate::shell::lock_path(id.as_str())),
        "0 = never reap, the shell's host must survive"
    );
}

use super::*;
use crate::daemon::executor::ToolCallOutcome;
use crate::daemon::executor::file_ops::run_in_host_shell;
use crate::ipc::Request;
use std::os::unix::fs::PermissionsExt as _;
use std::sync::Arc;

// ── Shared fixtures ───────────────────────────────────────────────────

/// A test `HOME` with `[hosts.web01]` configured, so the transport's
/// `host_entry_or_bare` resolves against something real.
pub(super) fn remote_write_test_home() -> (crate::TestHomeGuard, tempfile::TempDir) {
    let guard = crate::test_home_guard();
    let dir = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("HOME", dir.path());
    }
    crate::config::Config::ensure_dirs().ok();
    std::fs::write(
        dir.path().join(".daemoneye/etc/config.toml"),
        "[models.default]\nprovider = \"ollama\"\n\n[hosts.web01]\n\
         ssh = \"matt@web01.lan\"\n",
    )
    .unwrap();
    (guard, dir)
}

/// The in-process PTY host the transport tests drive, the same fixture
/// `read.rs`, `shell_exec.rs` and `bg_shell.rs` hold their own copies of:
/// a real bash session serving the id's socket, per-id instance lock held
/// so the shell reads as live. `spawn_ssh_host` re-invokes ssh, which a
/// test cannot use against a named host it cannot reach.
pub(super) async fn spawn_in_process_host(id: crate::shell::ShellId) -> anyhow::Result<()> {
    use anyhow::Context as _;

    let dir = tempfile::tempdir().context("tempdir for cast")?;
    let cast = dir.path().join(format!("{}.cast", id.as_str()));
    let backend = crate::shell::PtyBackend::spawn("bash", &[], 24, 80, 100, &cast, 0)
        .context("spawning bash")?;
    let lock =
        crate::daemon::instance::InstanceLock::acquire(&crate::shell::lock_path(id.as_str()))
            .context("holding the per-id instance lock")?;
    let socket = crate::shell::socket_path(id.as_str());
    let listener = crate::shell::bind(&socket)
        .await
        .context("binding the in-process host's socket")?;
    tokio::spawn(crate::shell::serve(listener, Arc::clone(&backend)));
    std::mem::forget(lock);
    std::mem::forget(dir);
    Ok(())
}

/// A two-pipe pump standing in for the client: the daemon's write half
/// feeds the pump the `EditFilePrompt`, the pump records it and writes an
/// approval back on the daemon's read half. Returns `(daemon_tx,
/// daemon_rx, prompts)`; the record is what test 10 reads — a host write
/// must open `EditFilePrompt`, not run unapproved.
pub(super) async fn auto_approve_channel() -> anyhow::Result<(
    tokio::io::DuplexStream,
    tokio::io::BufReader<tokio::io::DuplexStream>,
    Arc<std::sync::Mutex<Vec<String>>>,
)> {
    let (mut to_pump, daemon_tx) = tokio::io::duplex(64 * 1024);
    let (pump_out, daemon_rx) = tokio::io::duplex(64 * 1024);
    let prompts: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
    let prompt_ref = Arc::clone(&prompts);
    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        let mut pump_out = pump_out;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match tokio::io::AsyncReadExt::read(&mut to_pump, &mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let text = String::from_utf8_lossy(&buf[..n]).to_string();
                    for line in text.lines() {
                        if let Ok(Response::EditFilePrompt {
                            id,
                            path,
                            sudo,
                            always_asked,
                            existing_content,
                            new_content,
                            ..
                        }) = serde_json::from_str::<Response>(line)
                        {
                            let reply = Request::EditFileResponse {
                                id,
                                approved: true,
                                user_message: None,
                            };
                            prompt_ref.lock().unwrap().push(format!(
                                "edit:{path}:sudo={sudo}:always_asked={always_asked}\
                                 \nexisting:{existing_content:?}\nnew:{new_content:?}"
                            ));
                            let mut wire = serde_json::to_string(&reply).unwrap();
                            wire.push('\n');
                            let _ = pump_out.write_all(wire.as_bytes()).await;
                            continue;
                        }
                    }
                }
            }
        }
    });
    Ok((daemon_tx, tokio::io::BufReader::new(daemon_rx), prompts))
}

/// `approve_and_answer_channel`: the same two-pipe pump, but the client
/// also handles a `CredentialPrompt` — it records `"cred"` and then either
/// writes the credential back or, with `None`, drops its write half so the
/// daemon reads EOF (the "nobody is attached" case).
pub(super) async fn approve_and_answer_channel(
    credential: Option<&'static str>,
) -> anyhow::Result<(
    tokio::io::DuplexStream,
    tokio::io::BufReader<tokio::io::DuplexStream>,
    Arc<std::sync::Mutex<Vec<String>>>,
)> {
    let (mut to_pump, daemon_tx) = tokio::io::duplex(64 * 1024);
    let (pump_out, daemon_rx) = tokio::io::duplex(64 * 1024);
    let prompts: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
    let prompt_ref = Arc::clone(&prompts);
    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        let mut pump_out = pump_out;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match tokio::io::AsyncReadExt::read(&mut to_pump, &mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let text = String::from_utf8_lossy(&buf[..n]).to_string();
                    for line in text.lines() {
                        if let Ok(Response::EditFilePrompt {
                            id,
                            path,
                            sudo,
                            always_asked,
                            ..
                        }) = serde_json::from_str::<Response>(line)
                        {
                            let reply = Request::EditFileResponse {
                                id,
                                approved: true,
                                user_message: None,
                            };
                            prompt_ref.lock().unwrap().push(format!(
                                "edit:{path}:sudo={sudo}:always_asked={always_asked}"
                            ));
                            let mut wire = serde_json::to_string(&reply).unwrap();
                            wire.push('\n');
                            let _ = pump_out.write_all(wire.as_bytes()).await;
                            continue;
                        }
                        if let Ok(Response::CredentialPrompt { id, .. }) =
                            serde_json::from_str::<Response>(line)
                        {
                            prompt_ref.lock().unwrap().push("cred".to_string());
                            let Some(c) = credential else {
                                return;
                            };
                            let reply = Request::CredentialResponse {
                                id,
                                credential: c.to_string(),
                            };
                            let mut wire = serde_json::to_string(&reply).unwrap();
                            wire.push('\n');
                            let _ = pump_out.write_all(wire.as_bytes()).await;
                        }
                    }
                }
            }
        }
    });
    Ok((daemon_tx, tokio::io::BufReader::new(daemon_rx), prompts))
}

/// The Perl code is hex-encoded in the wire command, so check the source
/// for the File::Path/make_path call and File::Basename/dirname import.
/// `commit_local_edit_atomic` must not follow a symlink planted at the
/// predictable temp name: the write goes through an `O_EXCL` temp created
/// at a random name, so the planted link is ignored and the victim is
/// untouched.
#[test]
fn local_edit_ignores_a_planted_temp_symlink() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("f.txt");
    std::fs::write(&target, "old").unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o640)).unwrap();
    let victim = dir.path().join("victim");
    std::fs::write(&victim, "VICTIM").unwrap();
    std::os::unix::fs::symlink(&victim, dir.path().join("f.de_tmp")).unwrap();

    commit_local_edit_atomic(&target, target.to_str().unwrap(), "new")
        .unwrap_or_else(|e| panic!("the edit commits: {e}"));

    assert_eq!(
        std::fs::read_to_string(&victim).unwrap(),
        "VICTIM",
        "the planted symlink's target must be untouched"
    );
    let md = std::fs::symlink_metadata(&target).unwrap();
    assert!(
        !md.file_type().is_symlink(),
        "f.txt must be a regular file after the edit, not a symlink"
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
    assert_eq!(
        md.permissions().mode() & 0o7777,
        0o640,
        "the edit must keep the file's mode"
    );
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".de_tmp"))
        .collect();
    assert!(
        leftovers == ["f.de_tmp"],
        "no temp beyond the planted link may be left: {leftovers:?}"
    );
}

#[test]
fn remote_create_cmd_perl_branch_makes_parent_dirs() {
    let src = include_str!("ops.rs");
    assert!(
        src.contains("File::Path")
            && src.contains("make_path")
            && src.contains("File::Basename")
            && src.contains("dirname"),
        "Perl branch in source must contain File::Path/make_path and File::Basename/dirname"
    );
}
/// The Python code is hex-encoded in the wire command, so check the source
/// for the makedirs call.
#[test]
fn remote_create_cmd_python_branch_unchanged() {
    let src = include_str!("ops.rs");
    assert!(
        src.contains("makedirs") && src.contains("exist_ok=True"),
        "Python branch in source must still contain makedirs with exist_ok=True"
    );
}

/// The remote copy command must use `cp -n` (no-clobber) — a silent
/// overwrite would be a write the approval prompt never showed.
#[test]
fn remote_copy_cmd_is_no_clobber() {
    let src = include_str!("ops_copy.rs");
    assert!(
        src.contains("cp -n --"),
        "remote copy command must use cp -n (no-clobber)"
    );
}

/// Test 9, the retirement: `target_pane` is gone from `edit_file`'s
/// schema — and from the whole tool surface. This asserts against the
/// `ToolDef`, not a grep, so the test fails if the parameter comes back
/// under any spelling.
#[test]
fn edit_file_no_longer_accepts_target_pane() {
    let def = crate::ai::tools::TOOLS
        .iter()
        .find(|t| t.name == "edit_file")
        .unwrap_or_else(|| panic!("edit_file is in TOOLS"));
    let param_names: Vec<&str> = def.params.iter().map(|p| p.name).collect();
    assert!(
        !param_names.contains(&"target_pane"),
        "edit_file must not carry target_pane any more: {param_names:?}"
    );
    assert!(
        param_names.contains(&"host"),
        "edit_file carries host in its place: {param_names:?}"
    );
    let anywhere = crate::ai::tools::TOOLS
        .iter()
        .any(|t| t.params.iter().any(|p| p.name == "target_pane"));
    assert!(
        !anywhere,
        "target_pane is retired from the whole tool surface"
    );
}

/// The `DE_OK:` / `DE_ERROR:` sentinel contract, pinned for all four
/// command builders. Edit's and create's builders hex-encode the
/// Python/Perl payloads, so the sentinels live in the *source* (checked
/// with `include_str!`); delete's and copy's emit them inline, so the
/// built strings carry them (checked directly). Mutation B drifts one of
/// these — the scan and the builder are a contract.
#[test]
fn the_write_sentinels_are_unchanged() {
    let create_cmd = build_remote_create_cmd("/tmp/x", "hi");
    assert!(
        create_cmd.contains("echo '__DE_DONE__'"),
        "create's builder must still terminate with the done marker: {create_cmd:?}"
    );
    let edit_cmd = super::super::write::build_remote_edit_cmd("/tmp/x", "a", "b");
    assert!(
        edit_cmd.contains("echo '__DE_DONE__'"),
        "edit's builder must still terminate with the done marker: {edit_cmd:?}"
    );
    let src = concat!(
        include_str!("write.rs"),
        include_str!("ops.rs"),
        include_str!("ops_delete.rs"),
        include_str!("ops_copy.rs"),
    );
    for why in [
        "DE_ERROR: file already exists",
        "DE_OK: Created",
        "DE_OK: Edited",
        "DE_ERROR: file not found",
        "DE_OK: Deleted",
        "DE_ERROR: source not found",
        "DE_ERROR: destination already exists",
        "DE_OK: Copied",
        "DE_ERROR: cp failed",
        "DE_OK:",
        "DE_ERROR:",
    ] {
        assert!(src.contains(why), "the sentinel contract lost {why:?}");
    }
    // Both interpreter branches carry their own sentinel, so a drift in
    // one must halve the count, not hide behind the other's copy. The
    // builder's payload is hex-encoded, so decode it out of the built
    // command and check each branch's copy there.
    let decode_hex_from = |cmd: &str, what: &str| -> Option<String> {
        let start = cmd.find(what)? + what.len();
        let hex: String = cmd[start..]
            .chars()
            .take_while(|c| c.is_ascii_hexdigit())
            .collect();
        (hex.len().is_multiple_of(2) && !hex.is_empty())
            .then(|| {
                (0..hex.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(b'?'))
                    .collect::<Vec<u8>>()
            })
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    };
    for (label, payload) in [
        ("create python", decode_hex_from(&create_cmd, "fromhex('")),
        (
            "create perl",
            decode_hex_from(&create_cmd, "pack(\"H*\",\""),
        ),
        ("edit python", decode_hex_from(&edit_cmd, "fromhex('")),
        ("edit perl", decode_hex_from(&edit_cmd, "pack(\"H*\",\"")),
    ] {
        let payload = payload.unwrap_or_else(|| panic!("{label} branch's hex payload decoded"));
        assert!(
            payload.contains("DE_OK:") && payload.contains("DE_ERROR:"),
            "{label} branch must still emit both sentinels: {payload:?}"
        );
    }
}

/// A mode a file carried before an edit must still be on it after one,
/// on the local path: the temp file is chmod'd to the original's mode
/// before the rename, so a `0600` file does not come back `0644` and a
/// `0750` file keeps its execute bits under any umask.
#[tokio::test]
async fn a_local_edit_keeps_the_files_mode() {
    let (_home, dir) = remote_write_test_home();
    for (name, mode) in [("six.txt", 0o600), ("sevenfifty.txt", 0o750)] {
        let file = dir.path().join(name);
        std::fs::write(&file, "a\nold-line\nb\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
        let (mut tx, mut rx, _prompts) = auto_approve_channel().await.unwrap();
        let outcome = run_edit_with_spawn(
            RunEditArgs {
                id: "t09a",
                path: file.to_str().unwrap(),
                old_string: "old-line",
                new_string: "new-line",
                host: &crate::host::HostId::Local,
                sudo: false,
            },
            Some("sess-09a"),
            &mut tx,
            &mut rx,
            Box::new(|_id| Box::pin(async { Ok(()) })),
        )
        .await
        .unwrap_or_else(|e| panic!("run_edit must not error: {e}"));
        let ToolCallOutcome::Result(s) = outcome else {
            panic!("expected a Result outcome")
        };
        assert!(s.contains("Edited"), "the local edit really ran: {s:?}");
        let edited = std::fs::read_to_string(&file).unwrap_or_default();
        assert!(
            edited.contains("new-line"),
            "the edit landed on the file: {edited:?}"
        );
        let got = std::fs::metadata(&file).unwrap().permissions().mode() & 0o7777;
        assert_eq!(got, mode, "{name} must keep its mode across an edit: {s:?}");
    }
}

/// The same preservation on the remote path: the op builds its own
/// command and the in-process host shell runs the payload, so the chmod
/// the payload carries is what the file's mode reflects afterwards.
#[tokio::test]
async fn a_remote_edit_keeps_the_files_mode() {
    let (_home, dir) = remote_write_test_home();
    for (name, mode) in [("six.txt", 0o600), ("sevenfifty.txt", 0o750)] {
        let file = dir.path().join(name);
        std::fs::write(&file, "a\nold-line\nb\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
        let (mut tx, mut rx, _prompts) = auto_approve_channel().await.unwrap();
        let outcome = run_edit_with_spawn(
            RunEditArgs {
                id: "t09a",
                path: file.to_str().unwrap(),
                old_string: "old-line",
                new_string: "new-line",
                host: &crate::host::HostId::Named("web01".to_string()),
                sudo: false,
            },
            Some("sess-09a"),
            &mut tx,
            &mut rx,
            Box::new(|id| Box::pin(spawn_in_process_host(id))),
        )
        .await
        .unwrap_or_else(|e| panic!("run_edit must not error: {e}"));
        let ToolCallOutcome::Result(s) = outcome else {
            panic!("expected a Result outcome")
        };
        assert!(
            s.contains(&format!("Edited {} on host web01.", file.display())),
            "the approved host edit really ran: {s:?}"
        );
        let edited = std::fs::read_to_string(&file).unwrap_or_default();
        assert!(
            edited.contains("new-line"),
            "the edit landed on the host filesystem: {edited:?}"
        );
        let got = std::fs::metadata(&file).unwrap().permissions().mode() & 0o7777;
        assert_eq!(
            got, mode,
            "{name} must keep its mode across a host edit: {s:?}"
        );
    }
    let owner = Owner::Chat(owner_key());
    if let Some(id) = crate::shell::registry_lookup(&owner, |id| id.to_string()) {
        let _ = crate::daemon::executor::knowledge::close_shell_with(
            crate::shell::registry(),
            &id,
            &owner,
            std::time::Duration::from_millis(3000),
        )
        .await;
    }
}

/// The Python branch alone: the builder's payload is driven by a real
/// `python3`, with `PATH` narrowed so only that interpreter resolves —
/// the fallback would never be reached, so the assertion is about the
/// branch itself.
#[test]
fn the_python_branch_keeps_the_files_mode() {
    let which = |prog: &str| -> String {
        let out = std::process::Command::new("sh")
            .args(["-c", &format!("command -v {prog}")])
            .output()
            .unwrap_or_else(|e| panic!("resolving {prog}: {e}"));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    let bash = which("bash");
    let python3 = which("python3");
    assert!(
        !bash.is_empty() && !python3.is_empty(),
        "bash and python3 must resolve"
    );
    let bin = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(&python3, bin.path().join("python3")).unwrap();
    for (name, mode) in [("six.txt", 0o600), ("sevenfifty.txt", 0o750)] {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(name);
        std::fs::write(&file, "a\nold-line\nb\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
        let cmd = super::super::write::build_remote_edit_cmd(
            file.to_str().unwrap(),
            "old-line",
            "new-line",
        );
        let out = std::process::Command::new(&bash)
            .arg("-c")
            .arg(&cmd)
            .env("PATH", bin.path())
            .output()
            .unwrap_or_else(|e| panic!("running the python branch: {e}"));
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("DE_OK:"),
            "the python branch edited the file: {stdout:?}"
        );
        let edited = std::fs::read_to_string(&file).unwrap_or_default();
        assert!(edited.contains("new-line"), "the edit landed: {edited:?}");
        let got = std::fs::metadata(&file).unwrap().permissions().mode() & 0o7777;
        assert_eq!(
            got, mode,
            "{name} must keep its mode through the python branch: {stdout:?}"
        );
    }
}

/// The Perl branch alone: the same narrowed `PATH`, this time with only
/// a `perl` symlink, so `command -v python3` fails and the fallback runs.
#[test]
fn the_perl_branch_keeps_the_files_mode() {
    let which = |prog: &str| -> String {
        let out = std::process::Command::new("sh")
            .args(["-c", &format!("command -v {prog}")])
            .output()
            .unwrap_or_else(|e| panic!("resolving {prog}: {e}"));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    let bash = which("bash");
    let perl = which("perl");
    assert!(
        !bash.is_empty() && !perl.is_empty(),
        "bash and perl must resolve"
    );
    let bin = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(&perl, bin.path().join("perl")).unwrap();
    for (name, mode) in [("six.txt", 0o600), ("sevenfifty.txt", 0o750)] {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(name);
        std::fs::write(&file, "a\nold-line\nb\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
        let cmd = super::super::write::build_remote_edit_cmd(
            file.to_str().unwrap(),
            "old-line",
            "new-line",
        );
        let out = std::process::Command::new(&bash)
            .arg("-c")
            .arg(&cmd)
            .env("PATH", bin.path())
            .output()
            .unwrap_or_else(|e| panic!("running the perl branch: {e}"));
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("DE_OK:"),
            "the perl branch edited the file: {stdout:?}"
        );
        let edited = std::fs::read_to_string(&file).unwrap_or_default();
        assert!(edited.contains("new-line"), "the edit landed: {edited:?}");
        let got = std::fs::metadata(&file).unwrap().permissions().mode() & 0o7777;
        assert_eq!(
            got, mode,
            "{name} must keep its mode through the perl branch: {stdout:?}"
        );
    }
}

/// Both remote-edit branches must ignore a symlink planted at the
/// predictable temp name the old payloads used: the payload's temp is now
/// created exclusively at a random name, so the planted link and its
/// victim are untouched.
#[test]
fn remote_edit_ignores_a_planted_temp_symlink() {
    let which = |prog: &str| -> String {
        let out = std::process::Command::new("sh")
            .args(["-c", &format!("command -v {prog}")])
            .output()
            .unwrap_or_else(|e| panic!("resolving {prog}: {e}"));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    let bash = which("bash");
    assert!(!bash.is_empty(), "bash must resolve");
    for (interpreter, link) in [("python3", "python3"), ("perl", "perl")] {
        let resolved = which(interpreter);
        assert!(!resolved.is_empty(), "{interpreter} must resolve");
        let bin = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(&resolved, bin.path().join(link)).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("g.txt");
        std::fs::write(&file, "a\nold-line\nb\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o640)).unwrap();
        let victim = dir.path().join("victim");
        std::fs::write(&victim, "VICTIM").unwrap();
        std::os::unix::fs::symlink(&victim, dir.path().join(format!("g.txt{}", ".de_tmp")))
            .unwrap();
        let cmd = super::super::write::build_remote_edit_cmd(
            file.to_str().unwrap(),
            "old-line",
            "new-line",
        );
        let out = std::process::Command::new(&bash)
            .arg("-c")
            .arg(&cmd)
            .env("PATH", bin.path())
            .output()
            .unwrap_or_else(|e| panic!("running the {interpreter} branch: {e}"));
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("DE_OK:"),
            "the {interpreter} branch edited the file: {stdout:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&victim).unwrap(),
            "VICTIM",
            "the planted symlink's target must be untouched ({interpreter})"
        );
        let md = std::fs::symlink_metadata(&file).unwrap();
        assert!(
            !md.file_type().is_symlink(),
            "g.txt must be a regular file after the edit ({interpreter})"
        );
        assert!(
            std::fs::read_to_string(&file).unwrap().contains("new-line"),
            "the edit landed ({interpreter})"
        );
        assert_eq!(
            md.permissions().mode() & 0o7777,
            0o640,
            "g.txt keeps its mode through the {interpreter} branch"
        );
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".de_tmp"))
            .collect();
        assert!(
            leftovers == ["g.txt.de_tmp"],
            "no temp beyond the planted link may be left ({interpreter}): {leftovers:?}"
        );
    }
}

/// Both remote-create branches must ignore a symlink planted at the old
/// predictable temp name, and the created file keeps its umask-derived
/// mode (0644 under `umask 022`), not the temp's 0600.
#[test]
fn remote_create_ignores_a_planted_temp_symlink() {
    let which = |prog: &str| -> String {
        let out = std::process::Command::new("sh")
            .args(["-c", &format!("command -v {prog}")])
            .output()
            .unwrap_or_else(|e| panic!("resolving {prog}: {e}"));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    let bash = which("bash");
    assert!(!bash.is_empty(), "bash must resolve");
    for (interpreter, link) in [("python3", "python3"), ("perl", "perl")] {
        let resolved = which(interpreter);
        assert!(!resolved.is_empty(), "{interpreter} must resolve");
        let bin = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(&resolved, bin.path().join(link)).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("h.txt");
        let victim = dir.path().join("victim");
        std::fs::write(&victim, "VICTIM").unwrap();
        std::os::unix::fs::symlink(&victim, dir.path().join(format!("h.txt{}", ".de_tmp")))
            .unwrap();
        let cmd = super::super::ops::build_remote_create_cmd(file.to_str().unwrap(), "created");
        let out = std::process::Command::new(&bash)
            .arg("-c")
            .arg(format!("umask 022; {cmd}"))
            .env("PATH", bin.path())
            .output()
            .unwrap_or_else(|e| panic!("running the {interpreter} branch: {e}"));
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("DE_OK:"),
            "the {interpreter} branch created the file: {stdout:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&victim).unwrap(),
            "VICTIM",
            "the planted symlink's target must be untouched ({interpreter})"
        );
        let md = std::fs::symlink_metadata(&file).unwrap();
        assert!(
            !md.file_type().is_symlink(),
            "h.txt must be a regular file after the create ({interpreter})"
        );
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "created",
            "the content landed ({interpreter})"
        );
        assert_eq!(
            md.permissions().mode() & 0o7777,
            0o644,
            "the create keeps its umask-derived mode, not the temp's 0600 ({interpreter})"
        );
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".de_tmp"))
            .collect();
        assert!(
            leftovers == ["h.txt.de_tmp"],
            "no temp beyond the planted link may be left ({interpreter}): {leftovers:?}"
        );
    }
}

/// `commit_local_create` must ignore a symlink planted at the temp name
/// the old code used: the new file is written through an `O_EXCL` temp at
/// a random name, so the planted link and its target are untouched.
#[test]
fn local_create_ignores_a_planted_temp_symlink() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("n.txt");
    let victim = dir.path().join("victim");
    std::fs::write(&victim, "VICTIM").unwrap();
    std::os::unix::fs::symlink(&victim, dir.path().join("n.de_tmp")).unwrap();

    commit_local_create(&target, "created").unwrap_or_else(|e| panic!("the create commits: {e}"));

    assert_eq!(
        std::fs::read_to_string(&victim).unwrap(),
        "VICTIM",
        "the planted symlink's target must be untouched"
    );
    let md = std::fs::symlink_metadata(&target).unwrap();
    assert!(
        !md.file_type().is_symlink(),
        "n.txt must be a regular file after the create, not a symlink"
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "created");
    assert_eq!(
        md.permissions().mode() & 0o7777,
        0o600,
        "the created file keeps today's 0600 under the daemon's umask 077"
    );
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".de_tmp"))
        .collect();
    assert!(
        leftovers == ["n.de_tmp"],
        "no temp beyond the planted link may be left: {leftovers:?}"
    );
}

/// The exclusive temp opener: two calls for the same destination produce
/// different, private paths, and `create_new` refuses an existing path.
#[test]
fn exclusive_temp_is_private_and_fresh() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("f.txt");
    let (first_path, first_file) =
        exclusive_temp_file(&dest).unwrap_or_else(|e| panic!("the first temp opens: {e}"));
    let (second_path, second_file) =
        exclusive_temp_file(&dest).unwrap_or_else(|e| panic!("the second temp opens: {e}"));
    assert_ne!(first_path, second_path, "two calls must differ");
    for (path, _file) in [(&first_path, first_file), (&second_path, second_file)] {
        assert_eq!(path.parent(), Some(dir.path()), "beside the destination");
        assert!(
            path.to_str().unwrap().ends_with(".de_tmp"),
            "the temp keeps the .de_tmp suffix: {path:?}"
        );
        let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode & 0o077,
            0,
            "the temp is private to the owner: {path:?}"
        );
    }
    let again = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&first_path);
    assert_eq!(
        again.unwrap_err().kind(),
        std::io::ErrorKind::AlreadyExists,
        "create_new refuses an existing path"
    );
}

/// Owner restoration cannot be exercised without root (a non-root
/// writer's chown is a no-op by kernel rule), so the payload that does it
/// is pinned by decoding both hex payloads out of the builder, the way
/// `the_write_sentinels_are_unchanged` does. The mode tests above pin the
/// observable half.
#[test]
fn the_edit_payloads_restore_ownership() {
    let edit_cmd = super::super::write::build_remote_edit_cmd("/tmp/x", "a", "b");
    let decode_hex_from = |cmd: &str, what: &str| -> Option<String> {
        let start = cmd.find(what)? + what.len();
        let hex: String = cmd[start..]
            .chars()
            .take_while(|c| c.is_ascii_hexdigit())
            .collect();
        (hex.len().is_multiple_of(2) && !hex.is_empty())
            .then(|| {
                (0..hex.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(b'?'))
                    .collect::<Vec<u8>>()
            })
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    };
    let python = decode_hex_from(&edit_cmd, "fromhex('")
        .unwrap_or_else(|| panic!("the python branch's hex payload decoded"));
    assert!(
        python.contains("os.fchmod(fd,st.st_mode&0o7777)")
            && python.contains("os.fchown(fd,st.st_uid,st.st_gid)"),
        "the python branch must chmod the temp fd to the original's mode and chown it: {python:?}"
    );
    let perl = decode_hex_from(&edit_cmd, "pack(\"H*\",\"")
        .unwrap_or_else(|| panic!("the perl branch's hex payload decoded"));
    assert!(
        perl.contains("chmod($st[2]&07777,$g)") && perl.contains("chown($st[4],$st[5],$g)"),
        "the perl branch must chmod the temp filehandle to the original's mode and chown it: {perl:?}"
    );
}

/// Test 10, the security preservation: a **host** write is still gated by
/// `EditFilePrompt` before anything runs. The transport must not create an
/// unapproved write path.
#[tokio::test]
async fn a_remote_write_still_prompts_for_approval() {
    let (_home, _dir) = remote_write_test_home();
    let probe = std::env::temp_dir().join(format!("de-07b-approval-{}", std::process::id()));
    let _ = std::fs::remove_file(&probe);
    let (mut tx, mut rx, prompts) = auto_approve_channel().await.unwrap();

    let outcome = run_create_with_spawn(
        "t10",
        probe.to_str().unwrap(),
        Some("approved only\n"),
        &crate::host::HostId::Named("web01".to_string()),
        Some("sess-1"),
        &mut tx,
        &mut rx,
        Box::new(|id| Box::pin(spawn_in_process_host(id))),
    )
    .await
    .unwrap_or_else(|e| panic!("run_create must not error: {e}"));
    let got = prompts.lock().unwrap().clone();
    assert_eq!(
        got.len(),
        1,
        "exactly one EditFilePrompt must fire before the host run: {got:?}"
    );
    assert!(
        got[0].contains("(remote via host web01)"),
        "the prompt names the host: {:?}",
        got[0]
    );
    let ToolCallOutcome::Result(s) = outcome else {
        panic!("expected a Result outcome")
    };
    assert!(
        s.contains(&format!("Created {} on host web01.", probe.display())),
        "the approved host write really ran: {s:?}"
    );
    let written = std::fs::read_to_string(&probe).unwrap_or_default();
    assert!(
        written.contains("approved only"),
        "the file landed on the (in-process) host filesystem: {written:?}"
    );
    let _ = std::fs::remove_file(&probe);
}

/// Test 5, preservation: a local create produces byte-identical output to
/// before this phase — the same prompt (the client sees the plain path,
/// no host named). Approval is denied by construction here (`rx` is a pipe
/// nobody answers, so the await ends in the same "User denied execution" a
/// silent client produces); the *sentences*, not the write, are what this
/// test pins.
#[tokio::test]
async fn the_local_write_path_is_unchanged() {
    let (_home, _dir) = crate::test_home();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("fresh.txt");
    let (mut tx, mut rx, prompts) = auto_approve_channel().await.unwrap();
    let outcome = run_create(
        "t1",
        path.to_str().unwrap(),
        Some("hello local\n"),
        &crate::host::HostId::Local,
        Some("sess-1"),
        &mut tx,
        &mut rx,
    )
    .await
    .unwrap_or_else(|e| panic!("run_create must not error: {e}"));
    let prompt = prompts.lock().unwrap().join("\n");
    let ToolCallOutcome::Result(s) = outcome else {
        panic!("expected a Result outcome")
    };
    assert!(
        !s.contains("on host "),
        "a local create must not name a host: {s:?}"
    );
    assert!(
        !prompt.contains("(remote via host"),
        "a local create's prompt must not name a remote host: {prompt:?}"
    );
    assert!(
        prompt.contains(path.to_str().unwrap()),
        "the prompt names the plain path, as before: {prompt:?}"
    );
}

// ── Tests 1–4: each op runs end to end on a named host ────────────────

/// Test 1: a create on a named host runs through the shell — the op's
/// own command, the op's own scan, the file landing on the host
/// filesystem.
#[tokio::test]
async fn create_on_a_named_host_runs_through_the_shell() {
    let (_home, dir) = remote_write_test_home();
    let probe = dir.path().join("probe-create.txt");
    let (mut tx, mut rx, prompts) = auto_approve_channel().await.unwrap();
    let outcome = run_create_with_spawn(
        "t1",
        probe.to_str().unwrap(),
        Some("through the shell\n"),
        &crate::host::HostId::Named("web01".to_string()),
        Some("sess-1"),
        &mut tx,
        &mut rx,
        Box::new(|id| Box::pin(spawn_in_process_host(id))),
    )
    .await
    .unwrap_or_else(|e| panic!("run_create must not error: {e}"));
    let ToolCallOutcome::Result(s) = outcome else {
        panic!("expected a Result outcome")
    };
    assert!(
        s.contains(&format!("Created {} on host web01.", probe.display())),
        "the approved host create really ran: {s:?}"
    );
    let written = std::fs::read_to_string(&probe).unwrap_or_default();
    assert!(
        written.contains("through the shell"),
        "the file landed on the (in-process) host filesystem: {written:?}"
    );
    assert_eq!(
        prompts.lock().unwrap().len(),
        1,
        "the create is gated by exactly one EditFilePrompt"
    );
}

/// Test 2: a delete on a named host runs through the shell — the op
/// itself, not a retyped `rm`.
#[tokio::test]
async fn delete_on_a_named_host_runs_through_the_shell() {
    let (_home, dir) = remote_write_test_home();
    let probe = dir.path().join("probe-delete.txt");
    std::fs::write(&probe, "to be deleted").unwrap();
    let (mut tx, mut rx, prompts) = auto_approve_channel().await.unwrap();
    let outcome = run_delete_with_spawn(
        "t2",
        probe.to_str().unwrap(),
        &crate::host::HostId::Named("web01".to_string()),
        Some("sess-1"),
        &mut tx,
        &mut rx,
        Box::new(|id| Box::pin(spawn_in_process_host(id))),
    )
    .await
    .unwrap_or_else(|e| panic!("run_delete must not error: {e}"));
    let ToolCallOutcome::Result(s) = outcome else {
        panic!("expected a Result outcome")
    };
    assert!(
        s.contains(&format!("Deleted {} on host web01.", probe.display())),
        "the approved host delete really ran: {s:?}"
    );
    assert!(
        !probe.exists(),
        "the file really was deleted by the op's own command"
    );
    assert_eq!(
        prompts.lock().unwrap().len(),
        1,
        "the delete is gated by exactly one EditFilePrompt"
    );
}

/// M35 #40: a copy's temp path is unique per call and per destination.
#[test]
fn copy_tmp_path_is_unique_per_call_and_destination() {
    let b_txt = std::path::Path::new("/d/b.txt");
    let b_md = std::path::Path::new("/d/b.md");
    let first = copy_tmp_path(b_txt);
    let second = copy_tmp_path(b_txt);
    assert_ne!(
        first, second,
        "two calls for the same dest must differ: {first:?} vs {second:?}"
    );
    assert_ne!(
        first,
        copy_tmp_path(b_md),
        "dests with different extensions must not share a temp path: {first:?}"
    );
    for (dest, tmp) in [(b_txt, &first), (b_md, &copy_tmp_path(b_md))] {
        assert_eq!(
            tmp.parent(),
            Some(dest.parent().unwrap()),
            "the temp path stays next to {dest:?}: {tmp:?}"
        );
        assert!(
            tmp.to_str().unwrap().ends_with(".de_tmp"),
            "the temp path keeps the .de_tmp suffix: {tmp:?}"
        );
        assert_ne!(
            tmp, dest,
            "the temp path is never the destination itself: {tmp:?}"
        );
    }
}

/// M35 #37: the remote branch must report an existing destination exactly
/// as the local branch does — one source for the message, not two formats.
#[tokio::test]
async fn copy_on_a_named_host_reports_an_existing_destination_like_local() {
    let (_home, dir) = remote_write_test_home();
    let src = dir.path().join("probe-src.txt");
    let dst = dir.path().join("probe-dst.txt");
    std::fs::write(&src, "copy me").unwrap();
    std::fs::write(&dst, "already there").unwrap();
    let (mut tx, mut rx, prompts) = auto_approve_channel().await.unwrap();
    let outcome = run_copy_with_spawn(
        "t37",
        src.to_str().unwrap(),
        Some(dst.to_str().unwrap()),
        &crate::host::HostId::Named("web01".to_string()),
        Some("sess-1"),
        &mut tx,
        &mut rx,
        Box::new(|id| Box::pin(spawn_in_process_host(id))),
    )
    .await
    .unwrap_or_else(|e| panic!("run_copy must not error: {e}"));
    let ToolCallOutcome::Result(s) = outcome else {
        panic!("expected a Result outcome")
    };
    assert_eq!(
        s,
        format!(
            "Error: destination already exists: {}. Remove it first or choose a different path.",
            dst.display()
        ),
        "the remote branch reports the dest exactly as the local branch does: {s:?}"
    );
    assert_eq!(
        prompts.lock().unwrap().len(),
        1,
        "the copy is gated by exactly one EditFilePrompt"
    );
}

/// Test 3: a copy on a named host runs through the shell — the op
/// itself, not a retyped `cp`.
#[tokio::test]
async fn copy_on_a_named_host_runs_through_the_shell() {
    let (_home, dir) = remote_write_test_home();
    let src = dir.path().join("probe-src.txt");
    let dst = dir.path().join("probe-dst.txt");
    std::fs::write(&src, "copy me").unwrap();
    let (mut tx, mut rx, prompts) = auto_approve_channel().await.unwrap();
    let outcome = run_copy_with_spawn(
        "t3",
        src.to_str().unwrap(),
        Some(dst.to_str().unwrap()),
        &crate::host::HostId::Named("web01".to_string()),
        Some("sess-1"),
        &mut tx,
        &mut rx,
        Box::new(|id| Box::pin(spawn_in_process_host(id))),
    )
    .await
    .unwrap_or_else(|e| panic!("run_copy must not error: {e}"));
    let ToolCallOutcome::Result(s) = outcome else {
        panic!("expected a Result outcome")
    };
    assert!(
        s.contains(&format!(
            "Copied {} to {} on host web01.",
            src.display(),
            dst.display()
        )),
        "the approved host copy really ran: {s:?}"
    );
    let copied = std::fs::read_to_string(&dst).unwrap_or_default();
    assert!(
        copied.contains("copy me"),
        "the copy landed on the host filesystem: {copied:?}"
    );
    assert_eq!(
        prompts.lock().unwrap().len(),
        1,
        "the copy is gated by exactly one EditFilePrompt"
    );
}

/// Test 4: an edit on a named host runs through the shell — the op
/// itself, its own builder's payload, not a builder call bypassing the
/// branch.
#[tokio::test]
async fn edit_on_a_named_host_runs_through_the_shell() {
    let (_home, dir) = remote_write_test_home();
    let probe = dir.path().join("probe-edit.txt");
    std::fs::write(&probe, "the old string here").unwrap();
    let (mut tx, mut rx, prompts) = auto_approve_channel().await.unwrap();
    let outcome = run_edit_with_spawn(
        RunEditArgs {
            id: "t4",
            path: probe.to_str().unwrap(),
            old_string: "the old string",
            new_string: "the new string",
            host: &crate::host::HostId::Named("web01".to_string()),
            sudo: false,
        },
        Some("sess-1"),
        &mut tx,
        &mut rx,
        Box::new(|id| Box::pin(spawn_in_process_host(id))),
    )
    .await
    .unwrap_or_else(|e| panic!("run_edit must not error: {e}"));
    let ToolCallOutcome::Result(s) = outcome else {
        panic!("expected a Result outcome")
    };
    assert!(
        s.contains(&format!("Edited {} on host web01.", probe.display())),
        "the approved host edit really ran: {s:?}"
    );
    let edited = std::fs::read_to_string(&probe).unwrap_or_default();
    assert!(
        edited.contains("the new string"),
        "the edit landed on the host filesystem: {edited:?}"
    );
    assert_eq!(
        prompts.lock().unwrap().len(),
        1,
        "the edit is gated by exactly one EditFilePrompt"
    );
}

/// M32 #10: a create that fails on a path holding `DE_OK:` reports the
/// failure.
#[tokio::test]
async fn a_failed_create_on_a_sentinel_path_is_an_error() {
    let (_home, dir) = remote_write_test_home();
    let probe = dir.path().join("DE_OK:exists.txt");
    std::fs::write(&probe, "already here").unwrap();
    let (mut tx, mut rx, _prompts) = auto_approve_channel().await.unwrap();
    let outcome = run_create_with_spawn(
        "t10a",
        probe.to_str().unwrap(),
        Some("new content\n"),
        &crate::host::HostId::Named("web01".to_string()),
        Some("sess-1"),
        &mut tx,
        &mut rx,
        Box::new(|id| Box::pin(spawn_in_process_host(id))),
    )
    .await
    .unwrap_or_else(|e| panic!("run_create must not error: {e}"));
    let ToolCallOutcome::Result(s) = outcome else {
        panic!("expected a Result outcome")
    };
    assert!(
        s.starts_with("Error creating") && s.contains("file already exists"),
        "the failed create is reported as a failure: {s:?}"
    );
    assert_eq!(std::fs::read_to_string(&probe).unwrap(), "already here");
}

/// M32 #10: an edit that fails on a path holding `DE_OK:` reports the
/// failure, not `Edited`.
#[tokio::test]
async fn a_failed_edit_on_a_sentinel_path_is_an_error() {
    let (_home, dir) = remote_write_test_home();
    let probe = dir.path().join("DE_OK:probe.txt");
    std::fs::write(&probe, "nothing to match").unwrap();
    let (mut tx, mut rx, _prompts) = auto_approve_channel().await.unwrap();
    let outcome = run_edit_with_spawn(
        RunEditArgs {
            id: "t10b",
            path: probe.to_str().unwrap(),
            old_string: "an absent string",
            new_string: "replacement",
            host: &crate::host::HostId::Named("web01".to_string()),
            sudo: false,
        },
        Some("sess-1"),
        &mut tx,
        &mut rx,
        Box::new(|id| Box::pin(spawn_in_process_host(id))),
    )
    .await
    .unwrap_or_else(|e| panic!("run_edit must not error: {e}"));
    let ToolCallOutcome::Result(s) = outcome else {
        panic!("expected a Result outcome")
    };
    assert!(
        s.starts_with("Error editing") && s.contains("old_string not found"),
        "the failed edit is reported as a failure: {s:?}"
    );
    assert_eq!(std::fs::read_to_string(&probe).unwrap(), "nothing to match");
}

/// Test 7, the § Current state 3 negative: a remote write's success
/// sentence and its approval prompt both name the **host**, and neither
/// contains the word `pane`.
#[tokio::test]
async fn a_remote_write_names_the_host_not_a_pane() {
    let (_home, _dir) = remote_write_test_home();
    let probe = std::env::temp_dir().join(format!("de-07b-names-{}", std::process::id()));
    let _ = std::fs::remove_file(&probe);
    let (mut tx, mut rx, prompts) = auto_approve_channel().await.unwrap();

    let outcome = run_create_with_spawn(
        "t7",
        probe.to_str().unwrap(),
        Some("host named\n"),
        &crate::host::HostId::Named("web01".to_string()),
        Some("sess-1"),
        &mut tx,
        &mut rx,
        Box::new(|id| Box::pin(spawn_in_process_host(id))),
    )
    .await
    .unwrap_or_else(|e| panic!("run_create must not error: {e}"));
    let got = prompts.lock().unwrap().clone();
    let prompt = got.join("\n");
    let ToolCallOutcome::Result(s) = outcome else {
        panic!("expected a Result outcome")
    };
    assert!(
        s.contains(&format!("Created {} on host web01.", probe.display())),
        "the success sentence names the host: {s:?}"
    );
    assert!(!s.contains("pane"), "the sentence must not say pane: {s:?}");
    assert!(
        prompt.contains("(remote via host web01)"),
        "the prompt names the host: {prompt:?}"
    );
    assert!(
        !prompt.contains("pane"),
        "the prompt must not say pane: {prompt:?}"
    );
    let _ = std::fs::remove_file(&probe);
}

/// Test 6: a malformed host name is refused with `HostId::parse`'s own
/// message before anything is written and before any shell is opened.
#[tokio::test]
async fn a_malformed_host_is_refused_by_edit_file() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("never-written.txt");

    let outcome = super::super::run_edit_file(
        super::super::EditArgs {
            id: "t6",
            path: path.to_str().unwrap(),
            operation: "create",
            old_string: None,
            new_string: None,
            content: Some("must not land\n"),
            dest_path: None,
            host: Some("../etc"),
            sudo: false,
        },
        Some("sess-1"),
        super::super::super::GhostCtx {
            policy: None,
            is_ghost: false,
        },
        &mut tokio::io::sink(),
        &mut tokio::io::BufReader::new(tokio::io::empty()),
    )
    .await
    .unwrap_or_else(|e| panic!("run_edit_file must not error: {e}"));
    let ToolCallOutcome::Result(s) = outcome else {
        panic!("expected a Result outcome")
    };
    assert!(
        s.contains("Error:") && s.contains("../etc") && s.contains("A-Za-z0-9._-"),
        "HostId::parse's own message comes back: {s:?}"
    );
    assert!(
        !path.exists(),
        "nothing was written on a malformed host name"
    );
}

/// M37 phase-06: `resolve_edit_host` consults the self-host refusal.
#[tokio::test]
async fn resolve_edit_host_consults_the_self_host_refusal() {
    let (_guard, dir) = crate::test_home();
    let hostname = crate::daemon::utils::daemon_hostname();
    let cfg_text = format!("[hosts.selfentry]\nssh = \"{hostname}\"\n");
    let etc = dir.path().join(".daemoneye/etc");
    std::fs::create_dir_all(&etc).unwrap();
    std::fs::write(etc.join("config.toml"), &cfg_text).unwrap();

    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("never-written.txt");
    let outcome = super::super::run_edit_file(
        super::super::EditArgs {
            id: "t06-self",
            path: path.to_str().unwrap(),
            operation: "create",
            old_string: None,
            new_string: None,
            content: Some("must not land\n"),
            dest_path: None,
            host: Some("selfentry"),
            sudo: false,
        },
        Some("sess-06"),
        super::super::super::GhostCtx {
            policy: None,
            is_ghost: false,
        },
        &mut tokio::io::sink(),
        &mut tokio::io::BufReader::new(tokio::io::empty()),
    )
    .await
    .unwrap_or_else(|e| panic!("run_edit_file must not error: {e}"));
    let ToolCallOutcome::Result(s) = outcome else {
        panic!("expected a Result outcome")
    };
    assert!(
        s.contains("selfentry") && s.contains("is this machine") && s.contains("Omit `host`"),
        "the self-host refusal comes back before any prompt: {s:?}"
    );
    assert!(!path.exists(), "nothing was written on a refused self host");
}

/// `get_pane_remote_host` keeps its one live caller, the read-only tmux
/// context in `prompt.rs`.
#[test]
fn get_pane_remote_host_still_has_its_callers() {
    assert!(
        include_str!("../../prompt.rs").contains("get_pane_remote_host"),
        "prompt.rs keeps its live caller"
    );
    assert!(
        !include_str!("../../executor/ghost_exec.rs").contains("get_pane_remote_host"),
        "the tmux foreground caller was deleted in M27 phase 05"
    );
    let helper = include_str!("../../utils/host.rs");
    assert!(
        helper.contains("pub fn get_pane_remote_host"),
        "the helper itself still exists — deleting it was never this row's job"
    );
}

#[tokio::test]
#[ignore = "requires the configured host `pinky` reachable over ssh"]
async fn edit_file_on_pinky_end_to_end() {
    let (_home, dir) = {
        let g = crate::test_home_guard();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) }
        crate::config::Config::ensure_dirs().ok();
        (g, dir)
    };
    std::fs::write(
        dir.path().join(".daemoneye/etc/config.toml"),
        "[models.default]\nprovider = \"ollama\"\n\n[hosts.pinky]\nssh = \"pinky\"\n",
    )
    .expect("write config");
    crate::config::ssh_dir();

    let reg = crate::shell::ShellRegistry::default();
    let owner = Owner::Chat(owner_key());
    let probe = "/tmp/de-07b-probe.txt";

    let (mut tx, mut rx, prompts) = auto_approve_channel().await.unwrap();
    let outcome = run_create_with_spawn(
        "t-live",
        probe,
        Some("wrote-through-daemoneye\n"),
        &crate::host::HostId::Named("pinky".to_string()),
        Some("live07b"),
        &mut tx,
        &mut rx,
        Box::new(|id| Box::pin(crate::daemon::executor::knowledge::spawn_pinky_ssh(id))),
    )
    .await
    .expect("the pinky create resolves");
    let got = prompts.lock().unwrap().clone();
    assert_eq!(
        got.len(),
        1,
        "exactly one EditFilePrompt fired before the remote run: {got:?}"
    );
    assert!(
        got[0].contains("(remote via host pinky)"),
        "the prompt names the host: {:?}",
        got[0]
    );
    let ToolCallOutcome::Result(s) = outcome else {
        panic!("expected a Result outcome")
    };
    assert!(
        s.contains(&format!("Created {probe} on host pinky.")),
        "the success sentence names the host: {s:?}"
    );
    println!("PASTE MATCH >>>\nedit_file_on_pinky create sentence={s:?}\n<<<");

    let readback = run_in_host_shell(
        crate::shell::registry(),
        owner.clone(),
        crate::host::HostId::Named("pinky".to_string()),
        &format!("cat '{probe}'; rm -f '{probe}'"),
        REMOTE_WRITE_TIMEOUT_SECS,
        |id| std::boxed::Box::pin(crate::daemon::executor::knowledge::spawn_pinky_ssh(id)),
    )
    .await
    .expect("the pinky readback resolves");
    assert!(
        readback.contains("wrote-through-daemoneye"),
        "the file on pinky carries the content: {readback:?}"
    );
    println!("PASTE MATCH >>>\nedit_file_on_pinky readback={readback:?}\n<<<");

    let _ = crate::daemon::executor::knowledge::close_shell_with(
        &reg,
        crate::shell::registry_lookup(&owner, |id| id.to_string())
            .unwrap_or_default()
            .as_str(),
        &owner,
        std::time::Duration::from_millis(3000),
    )
    .await;
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
    let _ = dir;
}

#[tokio::test]
#[ignore = "requires the configured host `pinky` reachable over ssh"]
async fn a_remote_edit_keeps_mode_on_pinky() {
    let (_home, dir) = {
        let g = crate::test_home_guard();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) }
        crate::config::Config::ensure_dirs().ok();
        (g, dir)
    };
    std::fs::write(
        dir.path().join(".daemoneye/etc/config.toml"),
        "[models.default]\nprovider = \"ollama\"\n\n[hosts.pinky]\nssh = \"pinky\"\n",
    )
    .expect("write config");
    crate::config::ssh_dir();

    let reg = crate::shell::ShellRegistry::default();
    let owner = Owner::Chat(owner_key());
    let probe = "/tmp/de-09a-mode-probe";

    let _ = run_in_host_shell(
        crate::shell::registry(),
        owner.clone(),
        crate::host::HostId::Named("pinky".to_string()),
        "printf 'a\\nold-line\\nb\\n' > /tmp/de-09a-mode-probe && chmod 600 /tmp/de-09a-mode-probe && stat -c before=%a /tmp/de-09a-mode-probe",
        REMOTE_WRITE_TIMEOUT_SECS,
        |id| std::boxed::Box::pin(crate::daemon::executor::knowledge::spawn_pinky_ssh(id)),
    )
    .await
    .expect("the pinky probe setup resolves");

    let (mut tx, mut rx, _prompts) = auto_approve_channel().await.unwrap();
    let outcome = run_edit_with_spawn(
        RunEditArgs {
            id: "t09a-live",
            path: probe,
            old_string: "old-line",
            new_string: "new-line",
            host: &crate::host::HostId::Named("pinky".to_string()),
            sudo: false,
        },
        Some("live09a"),
        &mut tx,
        &mut rx,
        Box::new(|id| Box::pin(crate::daemon::executor::knowledge::spawn_pinky_ssh(id))),
    )
    .await
    .expect("the pinky edit resolves");
    let ToolCallOutcome::Result(s) = outcome else {
        panic!("expected a Result outcome")
    };
    assert!(
        s.contains(&format!("Edited {probe} on host pinky.")),
        "the approved host edit really ran: {s:?}"
    );

    let readback = run_in_host_shell(
        crate::shell::registry(),
        owner.clone(),
        crate::host::HostId::Named("pinky".to_string()),
        "stat -c after=%a /tmp/de-09a-mode-probe; rm -f /tmp/de-09a-mode-probe",
        REMOTE_WRITE_TIMEOUT_SECS,
        |id| std::boxed::Box::pin(crate::daemon::executor::knowledge::spawn_pinky_ssh(id)),
    )
    .await
    .expect("the pinky readback resolves");
    println!("before=600 (stat -c before=%a /tmp/de-09a-mode-probe)");
    println!("after: {readback:?}");
    assert!(
        readback.contains("after=600"),
        "the edited file keeps its mode on pinky: {readback:?}"
    );

    let _ = crate::daemon::executor::knowledge::close_shell_with(
        &reg,
        crate::shell::registry_lookup(&owner, |id| id.to_string())
            .unwrap_or_default()
            .as_str(),
        &owner,
        std::time::Duration::from_millis(3000),
    )
    .await;
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
    let _ = dir;
}

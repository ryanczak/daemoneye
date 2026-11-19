use super::tests::{
    approve_and_answer_channel, auto_approve_channel, remote_write_test_home, spawn_in_process_host,
};
use super::*;
use crate::daemon::executor::ToolCallOutcome;
use crate::daemon::executor::file_ops::run_in_host_shell;
use std::os::unix::fs::PermissionsExt as _;

// ── 09b: the root edit ─────────────────────────────────────────────────

/// A fake privilege tool the shell defines before the edit runs: it
/// prompts with `$SUDO_PROMPT`, accepts only `pw-xyz`, and then runs its
/// arguments. Spelled `f=su; f+=do; eval …` so the defining command does
/// not itself name the privilege tool and is not routed through the
/// prompt path — the same trick `shell_exec.rs`'s `PROMPTING` uses.
const FAKE_ROOT: &str = r#"f=su; f+=do; eval "$f() { local pw=; read -rs -p \"\$SUDO_PROMPT\" pw || return 1; echo; [ \"\$pw\" = pw-xyz ] || return 1; \"\$@\"; }""#;

/// Seed `FAKE_ROOT` into the shell the edit will reuse, so the spawned
/// in-process host's bash already holds the fake and the spawn closure is
/// never called again.
async fn seed_fake_root(host: &str) {
    run_in_host_shell(
        crate::shell::registry(),
        Owner::Chat(owner_key()),
        crate::host::HostId::Named(host.to_string()),
        FAKE_ROOT,
        REMOTE_WRITE_TIMEOUT_SECS,
        Box::new(|id| Box::pin(spawn_in_process_host(id))),
    )
    .await
    .expect("seed the fake privilege tool");
}

/// Close every shell the tool-keyed owner holds, so a leaked host does
/// not make unrelated PTY tests fail at random (09a's lesson).
async fn close_tool_shells() {
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

/// The answered path: the password is typed in the chat, the edit runs as
/// root, and the file's mode survives — the whole point of 09b.
#[tokio::test]
async fn a_root_edit_on_a_host_answers_the_password_prompt() {
    let (_home, dir) = remote_write_test_home();
    let probe = dir.path().join("probe-root.txt");
    std::fs::write(&probe, "a\nold-line\nb\n").unwrap();
    std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o640)).unwrap();
    seed_fake_root("web01").await;
    let (mut tx, mut rx, prompts) = approve_and_answer_channel(Some("pw-xyz")).await.unwrap();
    let outcome = run_edit_with_spawn(
        RunEditArgs {
            id: "t09b-1",
            path: probe.to_str().unwrap(),
            old_string: "old-line",
            new_string: "new-line",
            host: &crate::host::HostId::Named("web01".to_string()),
            sudo: true,
        },
        Some("sess-09b"),
        &mut tx,
        &mut rx,
        Box::new(|_id| Box::pin(async { Ok(()) })),
    )
    .await
    .expect("the root edit resolves");
    let ToolCallOutcome::Result(s) = outcome else {
        panic!("expected a Result outcome")
    };
    assert!(
        s.contains(&format!("Edited {} on host web01.", probe.display())),
        "the approved root edit really ran: {s:?}"
    );
    let edited = std::fs::read_to_string(&probe).unwrap_or_default();
    assert!(
        edited.contains("new-line"),
        "the edit landed on the file: {edited:?}"
    );
    let got = std::fs::metadata(&probe).unwrap().permissions().mode() & 0o7777;
    assert_eq!(got, 0o640, "the root edit keeps the file's mode: {s:?}");
    let got = prompts.lock().unwrap().clone();
    let edits: Vec<_> = got.iter().filter(|l| l.starts_with("edit:")).collect();
    let creds: Vec<_> = got.iter().filter(|l| l == &"cred").collect();
    assert_eq!(edits.len(), 1, "exactly one edit prompt fired: {got:?}");
    assert!(
        edits[0].contains("sudo=true") && edits[0].contains("as root via sudo"),
        "the prompt carries the root signal: {:?}",
        edits[0]
    );
    assert_eq!(
        creds.len(),
        1,
        "exactly one credential round trip fired: {got:?}"
    );
    close_tool_shells().await;
}

/// The give-up: nobody is attached, so the prompt is cancelled and the
/// file is untouched — the failure is loud, not "result was unclear".
#[tokio::test]
async fn an_unanswered_root_edit_leaves_the_file_unedited() {
    let (_home, dir) = remote_write_test_home();
    let probe = dir.path().join("probe-nobody.txt");
    std::fs::write(&probe, "a\nold-line\nb\n").unwrap();
    std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o640)).unwrap();
    seed_fake_root("web01").await;
    let (mut tx, mut rx, _prompts) = approve_and_answer_channel(None).await.unwrap();
    let started = std::time::Instant::now();
    let outcome = run_edit_with_spawn(
        RunEditArgs {
            id: "t09b-2",
            path: probe.to_str().unwrap(),
            old_string: "old-line",
            new_string: "new-line",
            host: &crate::host::HostId::Named("web01".to_string()),
            sudo: true,
        },
        Some("sess-09b"),
        &mut tx,
        &mut rx,
        Box::new(|_id| Box::pin(async { Ok(()) })),
    )
    .await
    .expect("the root edit resolves");
    let elapsed = started.elapsed();
    println!("root-edit give-up elapsed: {elapsed:?}");
    let ToolCallOutcome::Result(s) = outcome else {
        panic!("expected a Result outcome")
    };
    assert!(
        s.contains("Error") && s.contains("nobody is attached to answer it"),
        "the failure is loud, naming why: {s:?}"
    );
    assert!(
        !s.contains("result was unclear"),
        "a cancelled prompt is never \"result was unclear\": {s:?}"
    );
    let edited = std::fs::read_to_string(&probe).unwrap_or_default();
    assert!(
        edited.contains("old-line"),
        "the file is untouched: {edited:?}"
    );
    close_tool_shells().await;
}

/// A wrong password leaves no sentinel line at all — today's scan calls
/// that "result was unclear"; for a root edit it must be an `Error:`
/// saying the edit did not run.
#[tokio::test]
async fn a_wrong_password_root_edit_leaves_the_file_unedited() {
    let (_home, dir) = remote_write_test_home();
    let probe = dir.path().join("probe-wrong.txt");
    std::fs::write(&probe, "a\nold-line\nb\n").unwrap();
    std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o640)).unwrap();
    seed_fake_root("web01").await;
    let (mut tx, mut rx, _prompts) = approve_and_answer_channel(Some("wrong")).await.unwrap();
    let outcome = run_edit_with_spawn(
        RunEditArgs {
            id: "t09b-3",
            path: probe.to_str().unwrap(),
            old_string: "old-line",
            new_string: "new-line",
            host: &crate::host::HostId::Named("web01".to_string()),
            sudo: true,
        },
        Some("sess-09b"),
        &mut tx,
        &mut rx,
        Box::new(|_id| Box::pin(async { Ok(()) })),
    )
    .await
    .expect("the root edit resolves");
    let ToolCallOutcome::Result(s) = outcome else {
        panic!("expected a Result outcome")
    };
    assert!(
        s.starts_with("Error:") && s.contains("did not run"),
        "a refused password says the edit did not run: {s:?}"
    );
    assert!(
        !s.contains("unclear"),
        "never \"result was unclear\" on a root edit: {s:?}"
    );
    let edited = std::fs::read_to_string(&probe).unwrap_or_default();
    assert!(
        edited.contains("old-line"),
        "the file is untouched: {edited:?}"
    );
    close_tool_shells().await;
}

/// The scoping refusals, before any prompt: the root edit needs a
/// configured host and the `edit` operation, and nothing is offered for
/// approval in either refusal.
#[tokio::test]
async fn a_root_edit_needs_a_host_and_the_edit_operation() {
    let (_home, dir) = remote_write_test_home();
    let probe = dir.path().join("never-edited.txt");
    let mut out: Vec<u8> = Vec::new();
    let outcome = super::super::run_edit_file(
        super::super::EditArgs {
            id: "t09b-4",
            path: probe.to_str().unwrap(),
            operation: "edit",
            old_string: Some("old-line"),
            new_string: Some("new-line"),
            content: None,
            dest_path: None,
            host: None,
            sudo: true,
        },
        Some("sess-09b"),
        super::super::super::GhostCtx {
            policy: None,
            is_ghost: false,
        },
        &mut out,
        &mut tokio::io::BufReader::new(tokio::io::empty()),
    )
    .await
    .expect("the local root edit is refused");
    let ToolCallOutcome::Result(s) = outcome else {
        panic!("expected a Result outcome")
    };
    assert!(
        s.contains("runs on a configured host"),
        "the refusal names the missing host: {s:?}"
    );
    let outcome = super::super::run_edit_file(
        super::super::EditArgs {
            id: "t09b-4",
            path: probe.to_str().unwrap(),
            operation: "create",
            old_string: None,
            new_string: None,
            content: Some("nope\n"),
            dest_path: None,
            host: Some("web01"),
            sudo: true,
        },
        Some("sess-09b"),
        super::super::super::GhostCtx {
            policy: None,
            is_ghost: false,
        },
        &mut out,
        &mut tokio::io::BufReader::new(tokio::io::empty()),
    )
    .await
    .expect("the create root op is refused");
    let ToolCallOutcome::Result(s) = outcome else {
        panic!("expected a Result outcome")
    };
    assert!(
        s.contains("operation=\"edit\" only"),
        "the refusal names the only operation: {s:?}"
    );
    assert!(
        out.is_empty(),
        "no prompt reached the client before either refusal: {} bytes",
        out.len()
    );
}

/// The line is detected as a privilege-tool command (so the run path
/// turns the prompt handling on) and its payloads are the same bytes the
/// plain edit's builder produces — shared, not copied.
#[test]
fn the_root_edit_line_is_detected_and_reuses_the_payloads() {
    let cmd = super::super::write::build_remote_sudo_edit_cmd("/tmp/x", "a", "b");
    assert!(
        crate::daemon::utils::command_has_sudo(&cmd),
        "the run path must classify the line as a privilege-tool command: {cmd:?}"
    );
    assert!(
        cmd.contains("set -- python3 -c") && cmd.contains("set -- perl -e"),
        "the interpreter is chosen with set --, not inside the branch: {cmd:?}"
    );
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
    let plain = super::super::write::build_remote_edit_cmd("/tmp/x", "a", "b");
    for (what, label) in [("fromhex('", "python"), ("pack(\"H*\",\"", "perl")] {
        let a = decode_hex_from(&cmd, what)
            .unwrap_or_else(|| panic!("the root line's {label} payload decoded"));
        let b = decode_hex_from(&plain, what)
            .unwrap_or_else(|| panic!("the plain line's {label} payload decoded"));
        assert_eq!(
            a, b,
            "the root line's {label} payload is the plain edit's payload"
        );
        assert!(
            a.contains("DE_OK: Edited") && a.contains("DE_ERROR:"),
            "the {label} payload still carries the sentinel contract"
        );
    }
}

/// Both interpreter branches of the root line run a real edit.
#[test]
fn the_root_edit_line_runs_under_both_interpreters() {
    let which = |prog: &str| -> String {
        let out = std::process::Command::new("sh")
            .args(["-c", &format!("command -v {prog}")])
            .output()
            .unwrap_or_else(|e| panic!("resolving {prog}: {e}"));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    let bash = which("bash");
    assert!(!bash.is_empty(), "bash must resolve");
    for prog in ["python3", "perl"] {
        let interp = which(prog);
        assert!(!interp.is_empty(), "{prog} must resolve");
        let bin = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(&interp, bin.path().join(prog)).unwrap();
        let fake = bin.path().join("sudo");
        std::fs::write(&fake, "#!/bin/sh\nexec \"$@\"\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        for (name, mode) in [("six.txt", 0o600), ("sevenfifty.txt", 0o750)] {
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join(name);
            std::fs::write(&file, "a\nold-line\nb\n").unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
            let cmd = super::super::write::build_remote_sudo_edit_cmd(
                file.to_str().unwrap(),
                "old-line",
                "new-line",
            );
            assert!(
                cmd.contains(&format!("set -- {prog}")),
                "the {prog} branch was selected: {cmd:?}"
            );
            let out = std::process::Command::new(&bash)
                .arg("-c")
                .arg(&cmd)
                .env("PATH", bin.path())
                .output()
                .unwrap_or_else(|e| panic!("running the {prog} branch: {e}"));
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert!(
                stdout.contains("DE_OK:"),
                "the {prog} branch edited the file: {stdout:?}"
            );
            let edited = std::fs::read_to_string(&file).unwrap_or_default();
            assert!(edited.contains("new-line"), "the edit landed: {edited:?}");
            let got = std::fs::metadata(&file).unwrap().permissions().mode() & 0o7777;
            assert_eq!(
                got, mode,
                "{name} must keep its mode through the {prog} branch: {stdout:?}"
            );
        }
    }
}

#[tokio::test]
#[ignore = "requires the configured host `pinky` reachable over ssh"]
async fn a_root_edit_on_pinky_with_nobody_attached_fails_cleanly() {
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
    let probe = "/tmp/de-09b-probe";

    let _ = run_in_host_shell(
        crate::shell::registry(),
        owner.clone(),
        crate::host::HostId::Named("pinky".to_string()),
        "printf 'a\\nold-line\\nb\\n' > /tmp/de-09b-probe && chmod 640 /tmp/de-09b-probe",
        REMOTE_WRITE_TIMEOUT_SECS,
        |id| std::boxed::Box::pin(crate::daemon::executor::knowledge::spawn_pinky_ssh(id)),
    )
    .await
    .expect("the pinky probe setup resolves");

    let (mut tx, mut rx, _prompts) = approve_and_answer_channel(None).await.unwrap();
    let started = std::time::Instant::now();
    let outcome = run_edit_with_spawn(
        RunEditArgs {
            id: "t09b-live",
            path: probe,
            old_string: "old-line",
            new_string: "new-line",
            host: &crate::host::HostId::Named("pinky".to_string()),
            sudo: true,
        },
        Some("live09b"),
        &mut tx,
        &mut rx,
        Box::new(|id| Box::pin(crate::daemon::executor::knowledge::spawn_pinky_ssh(id))),
    )
    .await
    .expect("the pinky root edit resolves");
    let elapsed = started.elapsed();
    let ToolCallOutcome::Result(s) = outcome else {
        panic!("expected a Result outcome")
    };
    println!("root-edit-on-pinky result: {s:?}");
    println!("root-edit-on-pinky elapsed: {elapsed:?}");
    assert!(
        s.contains("nobody is attached to answer it"),
        "the give-up names the real problem: {s:?}"
    );

    let readback = run_in_host_shell(
        crate::shell::registry(),
        owner.clone(),
        crate::host::HostId::Named("pinky".to_string()),
        "cat /tmp/de-09b-probe; rm -f /tmp/de-09b-probe",
        REMOTE_WRITE_TIMEOUT_SECS,
        |id| std::boxed::Box::pin(crate::daemon::executor::knowledge::spawn_pinky_ssh(id)),
    )
    .await
    .expect("the pinky readback resolves");
    assert!(
        readback.contains("old-line"),
        "the file is untouched: {readback:?}"
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

fn gave_up_error() -> anyhow::Error {
    anyhow::Error::new(crate::shell::PromptGivenUp {
        reason: "nobody is attached to answer it — run it from a chat session.".to_string(),
    })
    .context("the password prompt in shell s7 was cancelled")
}

fn exited_error() -> anyhow::Error {
    anyhow::Error::new(crate::shell::ShellExited {
        cmd: "echo hi".to_string(),
    })
}

fn timeout_error() -> anyhow::Error {
    anyhow::anyhow!("timed out after 45s waiting for command output: \"sleep 60\"")
}

/// A failed sudo edit is worded for a file operation.
#[test]
fn a_root_edit_error_names_the_file_and_host() {
    for e in [gave_up_error(), exited_error(), timeout_error()] {
        let text = root_edit_error_text("/etc/x.conf", "pinky", &e);
        assert!(text.starts_with("Error: "), "it is an error report: {text}");
        assert!(text.contains("/etc/x.conf"), "it names the file: {text}");
        assert!(text.contains("pinky"), "it names the host: {text}");
        assert!(
            !text.contains("in shell edit_file"),
            "no pseudo shell id: {text}"
        );
        assert!(
            !text.contains("Omit `shell`"),
            "edit_file has no `shell` parameter: {text}"
        );
        assert!(!text.contains(".."), "no doubled full stop: {text}");
    }
}

/// Each branch keeps its meaning: give-up and exited are done — the file
/// was not changed; timeout is indeterminate — the edit may still land.
#[test]
fn a_root_edit_error_keeps_each_branch_meaning() {
    let give_up = root_edit_error_text("/etc/x.conf", "pinky", &gave_up_error());
    assert!(
        give_up.contains("nobody is attached") && give_up.contains("was not changed"),
        "give-up: done, not changed: {give_up}"
    );
    assert!(
        !give_up.contains("may still"),
        "give-up does not hedge: {give_up}"
    );

    let exited = root_edit_error_text("/etc/x.conf", "pinky", &exited_error());
    assert!(
        exited.contains("exited") && exited.contains("was not changed"),
        "exited: done, not changed: {exited}"
    );
    assert!(
        !exited.contains("may still"),
        "exited does not hedge: {exited}"
    );

    let timeout = root_edit_error_text("/etc/x.conf", "pinky", &timeout_error());
    assert!(
        timeout.contains("may still complete") && !timeout.contains("was not changed"),
        "timeout: indeterminate: {timeout}"
    );
}

// ── M36 phase-20a: the daemon's own config.toml is the admitted edit ──

/// A test `HOME` with the config holding a secret, so the masked-prompt
/// test drives a real file.
fn config_edit_test_home() -> (crate::TestHomeGuard, tempfile::TempDir) {
    let (guard, dir) = crate::test_home();
    std::fs::write(
        dir.path().join(".daemoneye/etc/config.toml"),
        "[models.default]\nprovider = \"anthropic\"\napi_key = \"sk-ant-secret-value-0001\"\n\
         model = \"claude\"\n\n[limits]\ntotal_tool_calls_per_turn = 0\n",
    )
    .unwrap();
    (guard, dir)
}

#[tokio::test]
async fn edit_file_config_refuses_create_delete_copy_host_and_sudo() {
    let (_home, dir) = config_edit_test_home();
    let cfg = dir.path().join(".daemoneye/etc/config.toml");
    let elsewhere = dir.path().join("elsewhere.toml");
    let refused = |args, out: &'static str| async move {
        let outcome = super::super::run_edit_file(
            args,
            Some("sess-20a"),
            super::super::super::GhostCtx {
                policy: None,
                is_ghost: false,
            },
            &mut tokio::io::sink(),
            &mut tokio::io::BufReader::new(tokio::io::empty()),
        )
        .await
        .unwrap_or_else(|e| panic!("{out} must resolve: {e}"));
        let ToolCallOutcome::Result(s) = outcome else {
            panic!("expected a Result outcome for {out}")
        };
        s
    };

    let create = refused(
        super::super::EditArgs {
            id: "t20a-c",
            path: cfg.to_str().unwrap(),
            operation: "create",
            old_string: None,
            new_string: None,
            content: Some("nope\n"),
            dest_path: None,
            host: None,
            sudo: false,
        },
        "the config create",
    )
    .await;
    assert!(
        create.contains("only operation=\"edit\""),
        "the config create is refused: {create:?}"
    );

    let delete = refused(
        super::super::EditArgs {
            id: "t20a-d",
            path: cfg.to_str().unwrap(),
            operation: "delete",
            old_string: None,
            new_string: None,
            content: None,
            dest_path: None,
            host: None,
            sudo: false,
        },
        "the config delete",
    )
    .await;
    assert!(
        delete.contains("only operation=\"edit\""),
        "the config delete is refused: {delete:?}"
    );

    let copy = refused(
        super::super::EditArgs {
            id: "t20a-p",
            path: cfg.to_str().unwrap(),
            operation: "copy",
            old_string: None,
            new_string: None,
            content: None,
            dest_path: Some(elsewhere.to_str().unwrap()),
            host: None,
            sudo: false,
        },
        "the config copy",
    )
    .await;
    assert!(
        copy.contains("only operation=\"edit\""),
        "the config copy is refused: {copy:?}"
    );

    let host = refused(
        super::super::EditArgs {
            id: "t20a-h",
            path: cfg.to_str().unwrap(),
            operation: "edit",
            old_string: Some("total_tool_calls_per_turn = 0"),
            new_string: Some("total_tool_calls_per_turn = 50"),
            content: None,
            dest_path: None,
            host: Some("de-test-unreachable.invalid"),
            sudo: false,
        },
        "the config edit with host",
    )
    .await;
    assert!(
        host.contains("daemon host's file"),
        "a config edit naming a host is refused: {host:?}"
    );
    assert!(
        !host.contains("EditFile"),
        "nothing is offered for approval: {host:?}"
    );

    let sudo_edit = refused(
        super::super::EditArgs {
            id: "t20a-s",
            path: cfg.to_str().unwrap(),
            operation: "edit",
            old_string: Some("total_tool_calls_per_turn = 0"),
            new_string: Some("total_tool_calls_per_turn = 50"),
            content: None,
            dest_path: None,
            host: None,
            sudo: true,
        },
        "the config sudo edit",
    )
    .await;
    assert!(
        sudo_edit.contains("sudo is not supported"),
        "a config edit as root is refused: {sudo_edit:?}"
    );

    let after = std::fs::read_to_string(&cfg).unwrap();
    assert!(
        after.contains("total_tool_calls_per_turn = 0"),
        "the file is unchanged: {after:?}"
    );
}

#[tokio::test]
async fn edit_file_still_refuses_other_daemoneye_paths() {
    let (_home, dir) = config_edit_test_home();
    let etc = dir.path().join(".daemoneye/etc");
    for name in ["other.toml", "config.toml.bak", "config.toml.de_tmp"] {
        let path = etc.join(name);
        std::fs::write(&path, "sibling\n").unwrap();
        let outcome = super::super::run_edit_file(
            super::super::EditArgs {
                id: "t20a-other",
                path: path.to_str().unwrap(),
                operation: "edit",
                old_string: Some("sibling"),
                new_string: Some("edited"),
                content: None,
                dest_path: None,
                host: None,
                sudo: false,
            },
            Some("sess-20a"),
            super::super::super::GhostCtx {
                policy: None,
                is_ghost: false,
            },
            &mut tokio::io::sink(),
            &mut tokio::io::BufReader::new(tokio::io::empty()),
        )
        .await
        .unwrap_or_else(|e| panic!("{name} must resolve: {e}"));
        let ToolCallOutcome::Result(s) = outcome else {
            panic!("expected a Result outcome for {name}")
        };
        assert!(
            s.contains("cannot access the daemoneye configuration"),
            "{name} stays refused: {s:?}"
        );
    }
}

#[tokio::test]
async fn edit_file_config_refuses_an_unparseable_result_and_leaves_the_file() {
    let (_home, dir) = config_edit_test_home();
    let cfg = dir.path().join(".daemoneye/etc/config.toml");
    let before = std::fs::read_to_string(&cfg).unwrap();
    let outcome = super::super::run_edit_file(
        super::super::EditArgs {
            id: "t20a-bad",
            path: cfg.to_str().unwrap(),
            operation: "edit",
            old_string: Some("total_tool_calls_per_turn = 0"),
            new_string: Some("limits = total [ [ [ broken"),
            content: None,
            dest_path: None,
            host: None,
            sudo: false,
        },
        Some("sess-20a"),
        super::super::super::GhostCtx {
            policy: None,
            is_ghost: false,
        },
        &mut tokio::io::sink(),
        &mut tokio::io::BufReader::new(tokio::io::empty()),
    )
    .await
    .unwrap_or_else(|e| panic!("the refusal resolves: {e}"));
    let ToolCallOutcome::Result(s) = outcome else {
        panic!("expected a Result outcome")
    };
    assert!(
        s.contains("unparseable"),
        "the refusal names the TOML failure: {s:?}"
    );
    assert!(
        !s.contains("EditFile"),
        "nothing is offered for approval: {s:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&cfg).unwrap(),
        before,
        "the file is byte-for-byte unchanged"
    );
}

#[tokio::test]
async fn edit_file_config_masked_placeholder_never_matches() {
    let (_home, dir) = config_edit_test_home();
    let cfg = dir.path().join(".daemoneye/etc/config.toml");
    let before = std::fs::read_to_string(&cfg).unwrap();
    let outcome = super::super::run_edit_file(
        super::super::EditArgs {
            id: "t20a-mask",
            path: cfg.to_str().unwrap(),
            operation: "edit",
            old_string: Some("api_key = \"<REDACTED>\"\n"),
            new_string: Some("api_key = \"<REDACTED>-edited\"\n"),
            content: None,
            dest_path: None,
            host: None,
            sudo: false,
        },
        Some("sess-20a"),
        super::super::super::GhostCtx {
            policy: None,
            is_ghost: false,
        },
        &mut tokio::io::sink(),
        &mut tokio::io::BufReader::new(tokio::io::empty()),
    )
    .await
    .unwrap_or_else(|e| panic!("the refusal resolves: {e}"));
    let ToolCallOutcome::Result(s) = outcome else {
        panic!("expected a Result outcome")
    };
    assert!(
        s.contains("old_string not found"),
        "a masked placeholder does not match the real contents: {s:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&cfg).unwrap(),
        before,
        "nothing was written"
    );
}

#[tokio::test]
async fn edit_file_config_prompt_is_masked_and_always_asked() {
    let (_home, dir) = config_edit_test_home();
    let cfg = dir.path().join(".daemoneye/etc/config.toml");
    let (mut tx, mut rx, prompts) = auto_approve_channel().await.unwrap();
    let outcome = run_edit_with_spawn(
        RunEditArgs {
            id: "t20a-prompt",
            path: cfg.to_str().unwrap(),
            old_string: "total_tool_calls_per_turn = 0",
            new_string: "total_tool_calls_per_turn = 50",
            host: &crate::host::HostId::Local,
            sudo: false,
        },
        Some("sess-20a"),
        &mut tx,
        &mut rx,
        Box::new(|_id| Box::pin(async { Ok(()) })),
    )
    .await
    .unwrap_or_else(|e| panic!("the approved write resolves: {e}"));
    let got = prompts.lock().unwrap().clone();
    assert_eq!(got.len(), 1, "exactly one EditFilePrompt fires: {got:?}");
    assert!(
        got[0].contains("always_asked=true"),
        "the prompt is never auto-approved on the wire: {got:?}"
    );
    assert!(
        !got[0].contains("sk-ant-secret-value-0001"),
        "the prompt never carries the secret value: {got:?}"
    );
    assert!(
        got[0].contains("REDACTED"),
        "the prompt's contents are masked: {got:?}"
    );
    let written = std::fs::read_to_string(&cfg).unwrap();
    assert!(
        written.contains("total_tool_calls_per_turn = 50"),
        "the write used the unmasked strings and landed: {written:?}"
    );
    let ToolCallOutcome::Result(s) = outcome else {
        panic!("expected a Result outcome")
    };
    assert!(s.contains("Edited"), "the write reports success: {s:?}");
    assert!(
        s.contains("The write succeeded, but the reload refused it"),
        "with no installed handle the result names the reload's error \
         without failing the write: {s:?}"
    );
    assert!(
        s.contains("no config handle is installed"),
        "the reload's own error comes back: {s:?}"
    );
}

#[tokio::test]
async fn edit_file_ordinary_edit_prompt_is_not_masked() {
    let (_home, dir) = crate::test_home();
    let file = dir.path().join("secrets.txt");
    std::fs::write(&file, "password = \"hunter2-real\"\n").unwrap();
    let (mut tx, mut rx, prompts) = auto_approve_channel().await.unwrap();
    let outcome = run_edit_with_spawn(
        RunEditArgs {
            id: "t20a-plain",
            path: file.to_str().unwrap(),
            old_string: "hunter2-real",
            new_string: "hunter2-rotated",
            host: &crate::host::HostId::Local,
            sudo: false,
        },
        Some("sess-20a"),
        &mut tx,
        &mut rx,
        Box::new(|_id| Box::pin(async { Ok(()) })),
    )
    .await
    .unwrap_or_else(|e| panic!("the approved write resolves: {e}"));
    let got = prompts.lock().unwrap().clone();
    assert_eq!(got.len(), 1, "exactly one EditFilePrompt fires: {got:?}");
    assert!(
        got[0].contains("hunter2-real"),
        "an ordinary file's prompt carries its real contents: {got:?}"
    );
    assert!(
        got[0].contains("hunter2-rotated"),
        "the new content is unmasked too: {got:?}"
    );
    assert!(
        !got[0].contains("<REDACTED>"),
        "nothing in the diff was masked: {got:?}"
    );
    assert!(
        got[0].contains("always_asked=false"),
        "the ordinary edit still follows the normal approval rules: {got:?}"
    );
    let written = std::fs::read_to_string(&file).unwrap();
    assert!(
        written.contains("hunter2-rotated"),
        "the write landed: {written:?}"
    );
    let ToolCallOutcome::Result(s) = outcome else {
        panic!("expected a Result outcome")
    };
    assert!(s.contains("Edited"), "the write reports success: {s:?}");
}

#[test]
fn edit_file_config_reports_the_reload_after_the_write() {
    assert_eq!(
        format!(
            "\n\nReload: {}",
            crate::cli::commands::format_reload(
                &["limits.total_tool_calls_per_turn".to_string()],
                &[],
            )
        ),
        "\n\nReload: applied (1 key):\n  limits.total_tool_calls_per_turn\n\
         waiting for a restart: (none)",
        "the applied keys render through the formatter daemoneye reload uses"
    );
    assert_eq!(
        format!(
            "\n\nReload: {}",
            crate::cli::commands::format_reload(&[], &[])
        ),
        "\n\nReload: nothing changed",
        "a no-change reload reads as `nothing changed`"
    );
    let e = anyhow::Error::msg("no config handle is installed — reload is a daemon-side op");
    let s = format!(
        "\n\nThe write succeeded, but the reload refused it: {e:#}. \
         Restart the daemon (or run `daemoneye reload`) to pick it up."
    );
    assert!(
        s.contains("The write succeeded"),
        "a refused reload never undoes the write: {s:?}"
    );
    assert!(
        s.contains("no config handle is installed"),
        "the reload's error is named: {s:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn the_seeded_template_round_trips_through_a_config_edit() {
    let (_home, dir) = crate::test_home();
    let cfg = dir.path().join(".daemoneye/etc/config.toml");
    std::fs::copy("assets/etc/config.toml", &cfg).unwrap();

    let read =
        super::super::read::run_read_file(cfg.to_str().unwrap(), Some(200), Some(60), None, None)
            .await
            .unwrap_or_else(|e| panic!("the seeded template's limits page is readable: {e}"));
    let ToolCallOutcome::Result(read_text) = read else {
        panic!("expected a Result outcome")
    };
    let limits_line = read_text
        .lines()
        .find(|l| l.contains("total_tool_calls_per_turn"))
        .unwrap_or("(absent)");
    assert!(
        limits_line.contains("= 0") && limits_line.contains("max total"),
        "the seeded template's [limits] block came back: {limits_line:?} in {read_text:?}"
    );

    let (mut sink_tx, mut sink_rx, _prompts) = auto_approve_channel().await.unwrap();

    let outcome = super::super::run_edit_file(
        super::super::EditArgs {
            id: "t20a-template",
            path: cfg.to_str().unwrap(),
            operation: "edit",
            old_string: Some("# [limits]\n# per_tool_batch            = 100    # max consecutive"),
            new_string: Some(
                "[limits]\ntotal_tool_calls_per_turn = 42\n\n\
                 # [limits]\n# per_tool_batch            = 100    # max consecutive",
            ),
            content: None,
            dest_path: None,
            host: None,
            sudo: false,
        },
        Some("sess-20a"),
        super::super::super::GhostCtx {
            policy: None,
            is_ghost: false,
        },
        &mut sink_tx,
        &mut sink_rx,
    )
    .await
    .unwrap_or_else(|e| panic!("the template edit resolves: {e}"));
    let ToolCallOutcome::Result(s) = outcome else {
        panic!("expected a Result outcome")
    };
    assert!(
        s.contains("Edited"),
        "the template edit is a normal approved edit: {s:?}"
    );
    let after = std::fs::read_to_string(&cfg).unwrap();
    assert!(
        after.contains("total_tool_calls_per_turn = 42\n"),
        "the [limits] key landed uncommented: {}",
        after
            .lines()
            .find(|l| l.starts_with("total_tool_"))
            .unwrap_or("(absent)")
    );
    let parsed = crate::config::parse_config_text(&cfg, &after)
        .unwrap_or_else(|e| panic!("the edited template must parse: {e}"));
    assert_eq!(
        parsed.limits.total_tool_calls_per_turn, 42,
        "the running config would pick the value up"
    );
}

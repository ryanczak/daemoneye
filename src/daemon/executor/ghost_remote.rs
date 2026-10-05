//! A ghost's command on a resolved host, run in a PTY shell there.
//!
//! Phase 08c: when a ghost runbook's `ssh_target` resolved to a configured
//! `[hosts]` entry (08a), the command runs on a PTY shell **on that host** —
//! the same substrate chat uses — instead of being wrapped in
//! `ssh <target> <cmd>` in a tmux window. The shell is owned by the ghost
//! (`Owner::Ghost(<session id>)`), reused for its later commands on that
//! host, and opened with `-o BatchMode=yes` so ssh never prompts: a host
//! that wants a password or an unknown key fails in milliseconds (08b's
//! exit detection reports it, 08f's wording names the unanswerable prompt).
//! An `ssh_target` that did not resolve keeps the 1.x tmux wrap, and so does
//! `[execution] backend = "tmux"`.
//!
//! A **local** ghost (no `ssh_target`) runs the same way on the daemon host
//! when the sandbox is off ([`ghost_local_pty`]), so its shell is one
//! `/agents` can attach to.

use crate::daemon::policy::GhostPolicy;
use crate::host::HostId;
use crate::shell::{Owner, ShellRegistry};

/// How long a ghost's remote command may run: the ghost turn budget
/// (`GHOST_TURN_TIMEOUT_SECS` in `daemon/ghost.rs`). PE 2026-09-27.
pub(super) const GHOST_REMOTE_TIMEOUT_SECS: u64 = 300;

/// The host a ghost's command runs on through this module, or `None` to keep
/// the 1.x tmux wrap.
///
/// Pure. The sandbox is deliberately **not** an input: the PTY branch runs
/// before the sandboxed local dispatch, and a remote ghost command never
/// executes on the daemon host — the runbook's `hosts:` list is the fence.
pub(super) fn ghost_remote_host(
    is_ghost: bool,
    uses_pty: bool,
    policy: Option<&GhostPolicy>,
) -> Option<HostId> {
    if !is_ghost || !uses_pty {
        return None;
    }
    policy.and_then(|p| p.remote_host.clone())
}

/// Whether a ghost's command runs in a PTY shell on the **daemon host**.
///
/// Pure. True for a ghost with the PTY backend, the sandbox off, and no
/// `ssh_target` (a resolved one took [`ghost_remote_host`]'s branch already;
/// an unresolved one keeps the 1.x `ssh <target>` wrap in a tmux window).
/// The sandboxed local command keeps its container in a tmux window.
pub(super) fn ghost_local_pty(
    is_ghost: bool,
    uses_pty: bool,
    sandbox_enabled: bool,
    policy: Option<&GhostPolicy>,
) -> bool {
    is_ghost && uses_pty && !sandbox_enabled && policy.is_some_and(|p| p.ssh_target.is_none())
}

/// Run one approved ghost command in a PTY shell on `host`, synchronously.
///
/// The shell is looked up (or spawned) under `owner` and **reused** for the
/// ghost's later commands on that host, exactly as chat's arm does. The
/// transport is this module's own `tokio::io::sink()` /
/// `tokio::io::BufReader::new(tokio::io::empty())` — a ghost never answers a
/// prompt, whatever the caller's `tx`/`rx` are, so a password prompt is
/// cancelled at once with 08f's honest message and the shell stays usable.
///
/// `spawn` is the spawn step, bound by the caller to the host the ghost
/// reaches (`spawn_ghost_ssh_host` in production).
pub(super) async fn run_ghost_remote<F, Fut>(
    reg: &ShellRegistry,
    owner: Owner,
    host: HostId,
    cmd: &str,
    call_id: &str,
    spawn: F,
) -> (String, i32)
where
    F: FnOnce(crate::shell::ShellId) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    let max_per_owner = crate::config::Config::load()
        .unwrap_or_default()
        .shells
        .max_per_owner;
    let mut sink = tokio::io::sink();
    let mut eof = tokio::io::BufReader::new(tokio::io::empty());
    let outcome = super::shell_exec::run_in_shell_with_prompts(
        reg,
        owner.clone(),
        cmd,
        max_per_owner,
        None,
        Some(GHOST_REMOTE_TIMEOUT_SECS),
        call_id,
        &mut sink,
        &mut eof,
        host,
        spawn,
    )
    .await;
    match outcome {
        Ok((_, result)) => (result.text, result.exit_code),
        Err(e) => {
            let shell_id = crate::shell::registry_lookup(&owner, |id| id.to_string())
                .unwrap_or_else(|| "s?".to_string());
            (super::shell_run_error_text(&shell_id, &e), 1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::{Owner, ShellRegistry, ShellResponse, ShellState, socket_path, status};
    use crate::test_home_guard;
    use anyhow::Context;
    use std::sync::Arc;

    /// The PTY geometry every shell host is spawned with, matching the
    /// detached `spawn_detached_host` production spawn (24×80).
    const ROWS: u16 = 24;
    /// The PTY geometry every shell host is spawned with.
    const COLS: u16 = 80;

    /// Point `HOME` at a fresh temp dir for the duration of the test and
    /// serialise against other HOME-mutating tests (bug-05c-1).
    fn test_home() -> (crate::TestHomeGuard, tempfile::TempDir) {
        let guard = test_home_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe {
            std::env::set_var("HOME", dir.path());
        }
        crate::config::Config::ensure_dirs().ok();
        (guard, dir)
    }

    /// A real in-process PTY host serving the configured socket for `id`: a
    /// bash session whose `.cast` the returned path points at. The
    /// detached `spawn_ghost_ssh_host` cannot run under a test binary
    /// (07b F4: `launch::spawn_detached` re-invokes `current_exe()`).
    async fn spawn_with_cast(id: crate::shell::ShellId) -> anyhow::Result<std::path::PathBuf> {
        let dir = tempfile::tempdir().context("tempdir for cast")?;
        let cast = dir.path().join(format!("{}.cast", id.as_str()));
        let backend = crate::shell::PtyBackend::spawn("bash", &[], ROWS, COLS, 100, &cast, 0)
            .context("spawning bash")?;
        let lock =
            crate::daemon::instance::InstanceLock::acquire(&crate::shell::lock_path(id.as_str()))
                .context("holding the per-id instance lock")?;
        let socket = socket_path(id.as_str());
        let listener = crate::shell::bind(&socket)
            .await
            .context("binding the in-process host's socket")?;
        tokio::spawn(crate::shell::serve(listener, Arc::clone(&backend)));
        std::mem::forget(lock);
        std::mem::forget(dir);
        Ok(cast)
    }

    /// The spawn step `run_in_shell_with_prompts` takes, in-process.
    async fn spawn_in_process_host(id: crate::shell::ShellId) -> anyhow::Result<()> {
        let cast = spawn_with_cast(id).await?;
        std::mem::forget(cast);
        Ok(())
    }

    /// A policy whose `ssh_target` resolved to `db01` (08a's stamp).
    fn resolved_policy() -> GhostPolicy {
        GhostPolicy {
            remote_host: Some(HostId::parse("db01").unwrap()),
            ..GhostPolicy::from_config(&crate::ipc::GhostConfig::default())
        }
    }

    #[test]
    fn a_ghost_reaches_the_pty_path_only_on_a_resolved_host_with_the_pty_backend() {
        let policy = resolved_policy();
        let unresolved = GhostPolicy {
            remote_host: None,
            ..GhostPolicy::from_config(&crate::ipc::GhostConfig::default())
        };
        let got = ghost_remote_host(true, true, Some(&policy));
        assert_eq!(
            got,
            Some(HostId::parse("db01").unwrap()),
            "a ghost with the PTY backend and a resolved host takes the branch"
        );
        let got = ghost_remote_host(true, false, Some(&policy));
        assert_eq!(
            got, None,
            "backend = \"tmux\" keeps the 1.x wrap: no remote PTY"
        );
        let got = ghost_remote_host(true, true, Some(&unresolved));
        assert_eq!(got, None, "an unresolved ssh_target keeps the 1.x wrap");
        let got = ghost_remote_host(false, true, Some(&policy));
        assert_eq!(got, None, "chat never routes through this module");
        let got = ghost_remote_host(true, true, None);
        assert_eq!(got, None, "no policy, no remote host");
    }

    #[tokio::test]
    async fn a_ghost_remote_command_runs_in_a_ghost_owned_shell_on_that_host() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Ghost("g1".into());
        let (text, exit_code) = run_ghost_remote(
            &reg,
            owner.clone(),
            HostId::Named("e2e-host".into()),
            "echo REMOTE_OK",
            "c1",
            spawn_in_process_host,
        )
        .await;
        assert_eq!(exit_code, 0, "the command ran cleanly: {text:?}");
        assert!(
            text.contains("REMOTE_OK"),
            "the command's output must come back: {text:?}"
        );
        let (count, owner_key, host) = crate::shell::with_shells(&reg, |store| {
            (
                store.len(),
                store.values().next().map(|r| r.owner.key()),
                store.values().next().map(|r| r.host.clone()),
            )
        });
        assert_eq!(count, 1, "exactly one shell record: {count}");
        assert_eq!(
            owner_key.as_deref(),
            Some("ghost:g1"),
            "the shell is the ghost's, not chat's: {owner_key:?}"
        );
        assert_eq!(
            host,
            Some(HostId::Named("e2e-host".into())),
            "the record names the host the command runs on: {host:?}"
        );
    }

    #[test]
    fn a_local_ghost_reaches_the_pty_path_only_unsandboxed_with_the_pty_backend() {
        let local = GhostPolicy::from_config(&crate::ipc::GhostConfig::default());
        assert!(
            ghost_local_pty(true, true, false, Some(&local)),
            "a local ghost with the PTY backend and no sandbox takes the branch"
        );
        assert!(
            !ghost_local_pty(true, false, false, Some(&local)),
            "backend = \"tmux\" keeps the tmux window"
        );
        assert!(
            !ghost_local_pty(true, true, true, Some(&local)),
            "a sandboxed command keeps its container in a tmux window"
        );
        assert!(
            !ghost_local_pty(false, true, false, Some(&local)),
            "chat never routes through this module"
        );
        assert!(
            !ghost_local_pty(true, true, false, None),
            "no policy, no shell"
        );
        let unresolved = GhostPolicy {
            ssh_target: Some("matt@db01.lan".to_string()),
            ..GhostPolicy::from_config(&crate::ipc::GhostConfig::default())
        };
        assert!(
            !ghost_local_pty(true, true, false, Some(&unresolved)),
            "an unresolved ssh_target keeps the ssh wrap"
        );
    }

    #[tokio::test]
    async fn a_local_ghost_command_runs_in_a_ghost_owned_shell_on_the_daemon_host() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Ghost("g-local".into());
        let (text, exit_code) = run_ghost_remote(
            &reg,
            owner.clone(),
            HostId::Local,
            "echo LOCAL_OK",
            "c1",
            spawn_in_process_host,
        )
        .await;
        let (owner_key, host) = crate::shell::with_shells(&reg, |store| {
            (
                store.values().next().map(|r| r.owner.key()),
                store.values().next().map(|r| r.host.clone()),
            )
        });
        crate::daemon::executor::knowledge::close_owner_shells(
            &reg,
            &owner,
            std::time::Duration::from_secs(5),
        )
        .await;
        assert_eq!(exit_code, 0, "the command ran cleanly: {text:?}");
        assert!(
            text.contains("LOCAL_OK"),
            "the command's output must come back: {text:?}"
        );
        assert_eq!(
            owner_key.as_deref(),
            Some("ghost:g-local"),
            "the shell is the ghost's: {owner_key:?}"
        );
        assert_eq!(host, Some(HostId::Local), "the shell is on the daemon host");
    }

    #[tokio::test]
    async fn a_ghost_reuses_its_shell_for_later_commands_on_that_host() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Ghost("g1".into());
        let (first, first_exit) = run_ghost_remote(
            &reg,
            owner.clone(),
            HostId::Named("e2e-host".into()),
            "export DE_REUSE=kept",
            "c1",
            spawn_in_process_host,
        )
        .await;
        assert_eq!(first_exit, 0, "the export ran cleanly: {first:?}");
        let (second, second_exit) = run_ghost_remote(
            &reg,
            owner.clone(),
            HostId::Named("e2e-host".into()),
            "echo \"v=$DE_REUSE\"",
            "c2",
            spawn_in_process_host,
        )
        .await;
        assert_eq!(second_exit, 0, "the echo ran cleanly: {second:?}");
        assert!(
            second.contains("v=kept"),
            "the same shell served the second command, so the export held: {second:?}"
        );
        let count = crate::shell::with_shells(&reg, |store| store.len());
        assert_eq!(count, 1, "the second command reused, not spawned: {count}");
    }

    #[tokio::test]
    async fn a_ghost_remote_password_prompt_fails_fast_and_says_nobody_can_answer() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Ghost("g1".into());
        // The fake, byte-for-byte as `shell_exec.rs` defines it: every `\$`
        // matters, so the typed line carries the classifier's sentinel.
        const PROMPTING: &str = r#"f=su; f+=do; eval "$f() { local pw=; while [ -z \"\$pw\" ]; do read -rs -p \"\$SUDO_PROMPT\" pw || return 1; echo; done; echo GOT_\${#pw}; }""#;
        let (seed, seed_exit) = run_ghost_remote(
            &reg,
            owner.clone(),
            HostId::Named("e2e-host".into()),
            PROMPTING,
            "c0",
            spawn_in_process_host,
        )
        .await;
        assert_eq!(seed_exit, 0, "the fake program was defined: {seed:?}");
        let started = std::time::Instant::now();
        let (text, exit_code) = run_ghost_remote(
            &reg,
            owner.clone(),
            HostId::Named("e2e-host".into()),
            "sudo true",
            "c1",
            spawn_in_process_host,
        )
        .await;
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "the give-up must be fast, not a timeout: {elapsed:?}"
        );
        assert_eq!(
            exit_code, 1,
            "the prompt was cancelled, the command ran not: {text:?}"
        );
        assert!(
            text.contains("nobody is attached"),
            "the reason names the real problem: {text:?}"
        );
        assert!(
            text.contains("cancelled"),
            "the wording says the prompt was cancelled: {text:?}"
        );
        let (after, after_exit) = run_ghost_remote(
            &reg,
            owner.clone(),
            HostId::Named("e2e-host".into()),
            "echo AFTER",
            "c2",
            spawn_in_process_host,
        )
        .await;
        assert_eq!(after_exit, 0, "the shell is still usable: {after:?}");
        assert!(
            after.contains("AFTER"),
            "the next command ran in the same shell: {after:?}"
        );
    }

    /// Live: a ghost's command runs on the configured host `pinky` through a
    /// real `ssh` PTY shell opened with `-o BatchMode=yes`, and the shell is
    /// closed when the ghost ends. Needs `pinky` reachable over ssh.
    #[tokio::test]
    #[ignore = "requires the configured host `pinky` reachable over ssh"]
    async fn a_ghost_command_runs_on_pinky_over_a_batchmode_shell() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Ghost("g1".into());
        let spawn_pinky = |id: crate::shell::ShellId| async move {
            let control = crate::config::ssh_dir();
            std::fs::create_dir_all(&control)
                .with_context(|| format!("creating {}", control.display()))?;
            let entry = crate::config::HostEntry {
                ssh: "pinky".into(),
                ..Default::default()
            };
            let argv = crate::host::ghost_ssh_argv(&entry, &control);
            let program = crate::shell::which_ssh()?;
            let dir = tempfile::tempdir().context("tempdir for cast")?;
            let cast = dir.path().join(format!("{}.cast", id.as_str()));
            let backend =
                crate::shell::PtyBackend::spawn(&program, &argv, ROWS, COLS, 100, &cast, 0)
                    .context("spawning ssh to pinky")?;
            let lock = crate::daemon::instance::InstanceLock::acquire(&crate::shell::lock_path(
                id.as_str(),
            ))
            .context("holding the per-id instance lock")?;
            let socket = socket_path(id.as_str());
            let listener = crate::shell::bind(&socket)
                .await
                .context("binding the in-process host's socket")?;
            tokio::spawn(crate::shell::serve(
                listener,
                std::sync::Arc::clone(&backend),
            ));
            std::mem::forget(lock);
            std::mem::forget(dir);
            Ok(())
        };
        let started = std::time::Instant::now();
        let (text, exit_code) = run_ghost_remote(
            &reg,
            owner.clone(),
            HostId::Named("pinky".into()),
            "hostname",
            "c1",
            spawn_pinky,
        )
        .await;
        println!(
            "pinky `hostname`: {:?} (exit {exit_code})",
            started.elapsed()
        );
        assert_eq!(exit_code, 0, "hostname ran on pinky: {text:?}");
        assert!(text.contains("pinky"), "pinky answered: {text:?}");
        let started = std::time::Instant::now();
        let (_, sudo_exit) = run_ghost_remote(
            &reg,
            owner.clone(),
            HostId::Named("pinky".into()),
            "sudo -k",
            "c2",
            spawn_pinky,
        )
        .await;
        println!(
            "pinky `sudo -k`: {:?} (exit {sudo_exit})",
            started.elapsed()
        );
        assert_eq!(sudo_exit, 0, "sudo -k ran: exit {sudo_exit}");
        let started = std::time::Instant::now();
        let closed = crate::daemon::executor::knowledge::close_owner_shells(
            &reg,
            &owner,
            std::time::Duration::from_secs(5),
        )
        .await;
        println!("pinky close: {:?} ({closed:?})", started.elapsed());
        assert_eq!(closed.len(), 1, "the ghost's one shell closed: {closed:?}");
        let mut exited = false;
        for _ in 0..40 {
            let state = status(
                &socket_path(closed[0].0.as_str()),
                std::time::Duration::from_millis(100),
            )
            .await;
            exited = matches!(state, Ok(ShellResponse::Exited { .. }))
                || matches!(&state,
                    Ok(ShellResponse::Status { state, .. }) if *state == ShellState::Exited);
            if exited {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(exited, "the closed shell reports Exited within 2 s");
    }
}

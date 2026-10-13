//! Getting a ready shell socket for an owner: reuse the owner's live shell or
//! create one, wait for it to answer readiness, and keep the registry honest
//! when a host never comes up.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Context;

use crate::config::Config;
use crate::host::ssh_argv;
use crate::shell::launch;
use crate::shell::registry::{
    Owner, ShellId, ShellRecord, ShellRegistry, host_is_alive, sandbox_sidecar_path, spawn_for,
    spawn_for_on_host, with_shells, write_sandbox_sidecar,
};

/// A ready shell socket for `owner` on `host`: reuse the owner's live shell
/// on that host, or create one. Returns the shell id and the path to its
/// bound, ready socket.
pub async fn shell_for_owner<F, Fut>(
    reg: &ShellRegistry,
    owner: Owner,
    host: crate::host::HostId,
    max_per_owner: u32,
    ready_timeout: Duration,
    spawn: F,
) -> anyhow::Result<(ShellId, PathBuf)>
where
    F: FnOnce(ShellId) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    // (1) Lowest-indexed existing record whose host is still alive, so the
    // answer is deterministic when an owner holds several shells; a match
    // whose host is dead is removed here so it is not handed to the next
    // caller as a live shell, then we fall through to create.
    let owner_key = owner.key();
    let reuse: Option<ShellId> = with_shells(reg, |store| {
        let mut matches: Vec<ShellId> = store
            .values()
            .filter(|record| {
                // A shell whose terminal an interactive command took passes
                // the liveness probe but eats every byte written to it
                // (08b F2); the owner gets a fresh one instead. The record
                // stays and the host stays alive: later work addresses the
                // session by id. A dedicated shell is excluded for the same
                // reason in the other direction: it belongs to exactly one
                // command, so it is never offered to anything else. Reuse is
                // host-keyed: a shell runs its commands on the host its
                // record names, so it is handed only to a caller asking for
                // that same host — anything else would silently relocate
                // the command. A sandbox shell runs its commands in a VM, so
                // it is handed only to a caller that asks for that sandbox,
                // never to `shell_for_owner`.
                !record.interactive
                    && !record.dedicated
                    && record.host == host
                    && record.sandbox.is_none()
                    && record.owner.key() == owner_key
            })
            .map(|record| record.id.clone())
            .collect();
        matches.sort_by_key(|id| id.index());
        let lowest = matches.into_iter().next()?;
        if host_is_alive(&launch::lock_path(lowest.as_str())) {
            Some(lowest)
        } else {
            store.remove(lowest.as_str());
            None
        }
    });

    // (2) Reuse a live shell without spawning and without waiting on
    // readiness: the lock's flock is the liveness contract (06b), and a
    // second check here would add nothing the caller's next command does not
    // already prove.
    if let Some(id) = reuse {
        let socket = launch::socket_path(id.as_str());
        return Ok((id, socket));
    }

    // (3) Otherwise create one on the requested host, then wait for it to
    // become ready.
    let id = spawn_for_on_host(reg, owner, max_per_owner, host, false, spawn).await?;
    let socket = launch::socket_path(id.as_str());
    if let Err(e) = launch::await_ready(&socket, Instant::now() + ready_timeout).await {
        // (4) A record for a host that never came up would be handed to the
        // next caller as a live shell; `spawn_for` rolled back only a spawn
        // failure, so readiness is this function's own responsibility to
        // undo.
        with_shells(reg, |store| {
            store.remove(id.as_str());
        });
        return Err(e).with_context(|| {
            format!(
                "shell {} never became ready at {} within {ready_timeout:?}",
                id.as_str(),
                socket.display()
            )
        });
    }
    Ok((id, socket))
}

/// `shell_for_owner` for shells inside the sbx sandbox `sandbox`: reuse the
/// owner's lowest-numbered live shell in **that** sandbox, or create one with
/// `spawn_for_in_sandbox`. A host shell, and a shell in any other sandbox, is
/// never returned.
pub async fn shell_for_owner_in_sandbox<F, Fut>(
    reg: &ShellRegistry,
    owner: Owner,
    sandbox: &str,
    max_per_owner: u32,
    ready_timeout: Duration,
    spawn: F,
) -> anyhow::Result<(ShellId, PathBuf)>
where
    F: FnOnce(ShellId) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    // The same reuse scan `shell_for_owner` runs, keyed on the sandbox
    // instead of the host: a sandbox shell's own host is always `Local`, so
    // the host is not what distinguishes two of the owner's shells here — the
    // sandbox is. Interactive and dedicated shells are excluded for the same
    // reasons `shell_for_owner` excludes them, and a match whose host is dead
    // is removed so it is not handed to the next caller as a live shell.
    let owner_key = owner.key();
    let reuse: Option<ShellId> = with_shells(reg, |store| {
        let mut matches: Vec<ShellId> = store
            .values()
            .filter(|record| {
                !record.interactive
                    && !record.dedicated
                    && record.owner.key() == owner_key
                    && record.sandbox.as_deref() == Some(sandbox)
            })
            .map(|record| record.id.clone())
            .collect();
        matches.sort_by_key(|id| id.index());
        let lowest = matches.into_iter().next()?;
        if host_is_alive(&launch::lock_path(lowest.as_str())) {
            Some(lowest)
        } else {
            store.remove(lowest.as_str());
            None
        }
    });

    // The same reuse shortcut: a live shell needs no readiness wait.
    if let Some(id) = reuse {
        let socket = launch::socket_path(id.as_str());
        return Ok((id, socket));
    }

    // Create in the named sandbox, then wait for readiness, rolling the
    // record back if the host never comes up — `spawn_for_in_sandbox` rolls
    // back only a spawn failure.
    let id = crate::shell::registry::spawn_for_in_sandbox(
        reg,
        owner,
        max_per_owner,
        sandbox.to_string(),
        false,
        spawn,
    )
    .await?;
    let socket = launch::socket_path(id.as_str());
    if let Err(e) = launch::await_ready(&socket, Instant::now() + ready_timeout).await {
        with_shells(reg, |store| {
            store.remove(id.as_str());
        });
        return Err(e).with_context(|| {
            format!(
                "shell {} never became ready at {} within {ready_timeout:?}",
                id.as_str(),
                socket.display()
            )
        });
    }
    Ok((id, socket))
}

/// The shell a host is spawned with, and the same name `client::run_command`
/// must be told so it reads the right exit variable (`$?` vs fish's
/// `$status`). One function so the two can never disagree.
pub fn login_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "bash".to_string())
}

/// The shell name whose exit-variable syntax the marker protocol must use.
///
/// For a remote shell this is **not** the daemon's `$SHELL`: the *remote*
/// shell interprets the marker. A fish-using operator on a bash host would
/// otherwise emit `$status`, which bash expands to empty, and the exit code
/// would never parse. Remote shells are pinned to POSIX `$?` (the name `"sh"`,
/// which [`exit_var`](crate::shell::pty::exit_var) maps to `$?`); a remote fish
/// login shell is unsupported and documented as such.
pub fn marker_shell_name(host: &crate::host::HostId) -> String {
    match host {
        crate::host::HostId::Local => login_shell(),
        crate::host::HostId::Named(_) => "sh".to_string(),
    }
}

/// The marker-protocol shell name for an existing shell: `"sh"` for a shell
/// inside a sandbox (its child is always `bash`), otherwise
/// [`marker_shell_name`] of its host.
pub fn shell_marker_name(record: &ShellRecord) -> String {
    if record.sandbox.is_some() {
        return "sh".to_string();
    }
    marker_shell_name(&record.host)
}

/// The `spawn` argument `shell_for_owner` takes in production: a detached
/// `daemoneye shell-host` for `id`. Separate from `shell_for_owner` because
/// `spawn_detached` resolves `current_exe()` and so cannot run under a test
/// (F4).
pub async fn spawn_detached_host(
    id: ShellId,
    args: &[String],
    rows: u16,
    cols: u16,
) -> anyhow::Result<()> {
    clear_sandbox_sidecar(&crate::config::shells_dir(), id.as_str());
    let shell = login_shell();
    launch::spawn_detached(id.as_str(), &shell, args, rows, cols).await
}

/// Spawn a detached `daemoneye shell-host` for `id` that opens a shell on the
/// named host `name`, over SSH, in the daemon user's own PTY — their
/// `~/.ssh/config`, keys and `SSH_AUTH_SOCK`. The entry is resolved with
/// [`host_entry_or_bare`]: a configured host uses its entry, an unconfigured
/// one becomes its own destination. `name` is the `[hosts.<name>]` key shape
/// `HostId::parse` accepts.
///
/// `host` is a **named** host; the implicit local host is refused here
/// because `spawn_detached_host` already covers it.
pub async fn spawn_ssh_host(id: ShellId, name: &str, rows: u16, cols: u16) -> anyhow::Result<()> {
    clear_sandbox_sidecar(&crate::config::shells_dir(), id.as_str());
    let entry = host_entry_or_bare(name)?;
    let control = crate::config::ssh_dir();
    std::fs::create_dir_all(&control).with_context(|| format!("creating {}", control.display()))?;
    let argv = ssh_argv(&entry, &control);
    let program = which_ssh()?;
    launch::spawn_detached(id.as_str(), &program, &argv, rows, cols).await
}

/// The `sbx` binary to spawn, resolved on `PATH`. A missing `sbx` is an error
/// naming the remedy, never a silent fallback and never a spawn failure deep
/// in the PTY.
pub fn which_sbx() -> anyhow::Result<String> {
    let path = std::env::var("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join("sbx");
        if candidate.is_file() {
            return Ok(candidate.to_string_lossy().into_owned());
        }
    }
    Err(anyhow::anyhow!(
        "sbx was not found on PATH for the daemon user — a sandbox shell needs \
         Docker Sandboxes (sbx) installed"
    ))
}

/// Remove any `<id>.sandbox` left by an earlier sandbox shell with this id,
/// so a plain shell that reuses the id is never adopted as a sandbox shell.
pub fn clear_sandbox_sidecar(dir: &std::path::Path, id: &str) {
    let _ = std::fs::remove_file(sandbox_sidecar_path(dir, id));
}

/// Spawn a detached `daemoneye shell-host` for `id` whose child is
/// `sbx exec -it <sandbox> bash`: the shell process itself is local, and every
/// command it runs executes inside the sandbox's VM. Like `spawn_ssh_host`,
/// this cannot run under a test — `spawn_detached` resolves `current_exe()`.
pub async fn spawn_sbx_host(
    id: ShellId,
    sandbox: &str,
    rows: u16,
    cols: u16,
) -> anyhow::Result<()> {
    let program = which_sbx()?;
    let dir = crate::config::shells_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    write_sandbox_sidecar(&dir, id.as_str(), sandbox).with_context(|| {
        format!(
            "writing {}",
            sandbox_sidecar_path(&dir, id.as_str()).display()
        )
    })?;
    let argv = crate::daemon::executor::sbx::tagged_shell_exec_args(sandbox, id.as_str());
    if let Err(e) = launch::spawn_detached(id.as_str(), &program, &argv, rows, cols).await {
        let _ = std::fs::remove_file(sandbox_sidecar_path(&dir, id.as_str()));
        return Err(e);
    }
    Ok(())
}

/// True when a ControlMaster is already running for `entry` under
/// `control_path`: `ssh -O check` exits 0 (measured 2–3 ms). Any error —
/// `ssh` missing, the check timing out, a spawn failure — reads as "no
/// master", the safe answer: the caller then tries to establish one.
pub fn master_running(entry: &crate::config::HostEntry, control_path: &std::path::Path) -> bool {
    let Ok(program) = which_ssh() else {
        return false;
    };
    std::process::Command::new(program)
        .args([
            "-O".to_string(),
            "check".to_string(),
            "-o".to_string(),
            format!("ControlPath={}/%C", control_path.display()),
            entry.ssh.clone(),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Spawn a detached `daemoneye shell-host` for `id` that opens a ghost's shell
/// on the named host `name`, over SSH, with [`ghost_ssh_argv`]: the same argv
/// `spawn_ssh_host` uses plus `-o BatchMode=yes` before the destination, so a
/// host that wants a password or an unknown key fails at once instead of
/// waiting for an answer nobody can give (08f's probe measured 17 ms).
///
/// The entry is resolved with the **strict** [`host_entry`], not the
/// permissive `host_entry_or_bare`: 08a stamps `remote_host` only for a
/// **configured** host, so a ghost's spawn must refuse a name `[hosts]` does
/// not configure rather than reach for it as its own destination.
pub async fn spawn_ghost_ssh_host(
    id: ShellId,
    name: &str,
    rows: u16,
    cols: u16,
) -> anyhow::Result<()> {
    clear_sandbox_sidecar(&crate::config::shells_dir(), id.as_str());
    let entry = host_entry(name)?;
    let control = crate::config::ssh_dir();
    std::fs::create_dir_all(&control).with_context(|| format!("creating {}", control.display()))?;
    let argv = crate::host::ghost_ssh_argv(&entry, &control);
    let program = which_ssh()?;
    launch::spawn_detached(id.as_str(), &program, &argv, rows, cols).await
}

/// Turn a named host into the `HostEntry` whose `ssh` destination it carries.
/// Refuses `HostId::Local` and any name `[hosts]` does not configure; a host
/// with an empty destination is its own refusal.
pub fn host_entry(name: &str) -> anyhow::Result<crate::config::HostEntry> {
    let cfg = Config::load().unwrap_or_default();
    if name.eq_ignore_ascii_case("local") {
        return Err(anyhow::anyhow!(
            "the local host is implicit — it has no [hosts] entry and needs no ssh \
             spawn; a plain command (no `shell` argument) already runs there"
        ));
    }
    match cfg.hosts.get(name) {
        Some(entry) if !entry.ssh.trim().is_empty() => Ok(entry.clone()),
        Some(_) => Err(anyhow::anyhow!(
            "host {name} is configured but its ssh destination is empty — \
             set ssh = \"user@hostname\" in [hosts.{name}]"
        )),
        None => {
            let mut known: Vec<&str> = cfg.hosts.keys().map(|s| s.as_str()).collect();
            known.sort();
            let listed = if known.is_empty() {
                "no hosts are configured — add a [hosts.<name>] section \
                 with an ssh = \"user@hostname\" entry to config.toml"
                    .to_string()
            } else {
                format!("configured hosts: {}", known.join(", "))
            };
            Err(anyhow::anyhow!("unknown host {name:?}; {listed}"))
        }
    }
}

/// The permissive lookup the run path uses: a configured host returns its
/// configured entry; a well-formed name `[hosts]` does not configure returns a
/// synthesised entry whose `ssh` destination is the name itself — `ssh(1)`
/// destinations honour `~/.ssh/config`, so a bare alias is a valid
/// destination, and `[hosts.pinky] ssh = "pinky"` is exactly that shape.
///
/// Callers: `run_terminal_command`'s pre-spawn check (`shell_exec.rs`,
/// `executor/mod.rs`) and `spawn_ssh_host`, all behind an approval the user
/// has already given for this destination. Not for `open_shell { host }`,
/// which keeps the strict [`host_entry`]: it is an explicit "give me a shell
/// there" and a name the operator has not configured is refused there rather
/// than reached for.
///
/// Still refused here: `"local"` in any case (the implicit host is never a
/// remote destination), and a configured host whose `ssh` is empty — a
/// half-written entry is a mistake, not an invitation to guess.
pub fn host_entry_or_bare(name: &str) -> anyhow::Result<crate::config::HostEntry> {
    let cfg = Config::load().unwrap_or_default();
    if name.eq_ignore_ascii_case("local") {
        return Err(anyhow::anyhow!(
            "the local host is implicit — it has no [hosts] entry and needs no ssh \
             spawn; a plain command (no `shell` argument) already runs there"
        ));
    }
    match cfg.hosts.get(name) {
        Some(entry) if !entry.ssh.trim().is_empty() => Ok(entry.clone()),
        Some(_) => Err(anyhow::anyhow!(
            "host {name} is configured but its ssh destination is empty — \
             set ssh = \"user@hostname\" in [hosts.{name}]"
        )),
        None => Ok(crate::config::HostEntry {
            ssh: name.to_string(),
            ..Default::default()
        }),
    }
}

/// The `ssh` binary to spawn, resolved on `PATH` (`/usr/bin/ssh` on this
/// daemon host). A missing `ssh` is an error naming the remedy, never a
/// silent fallback and never a spawn failure deep in the PTY.
pub fn which_ssh() -> anyhow::Result<String> {
    let path = std::env::var("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join("ssh");
        if candidate.is_file() {
            return Ok(candidate.to_string_lossy().into_owned());
        }
    }
    Err(anyhow::anyhow!(
        "ssh was not found on PATH for the daemon user — an SSH shell needs the \
         ssh(1) binary; install openssh-clients or add it to PATH"
    ))
}

/// A ready shell created for one command's exclusive use. Unlike
/// `shell_for_owner` this **never** reuses: a background command gets its own
/// shell or none.
///
/// The owner cap still applies and its error still propagates, so a background
/// command that would exceed `[shells] max_per_owner` fails loudly here rather
/// than silently borrowing a foreground shell. As in `shell_for_owner`, a
/// record for a host that never came up is worse than no record at all, so a
/// readiness failure removes the record before the error is returned.
pub async fn spawn_dedicated_for<F, Fut>(
    reg: &ShellRegistry,
    owner: Owner,
    max_per_owner: u32,
    ready_timeout: Duration,
    spawn: F,
) -> anyhow::Result<(ShellId, PathBuf)>
where
    F: FnOnce(ShellId) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    let id = spawn_for(reg, owner, max_per_owner, true, spawn).await?;
    let socket = launch::socket_path(id.as_str());
    if let Err(e) = launch::await_ready(&socket, Instant::now() + ready_timeout).await {
        // A record for a host that never came up would be handed to the next
        // caller as a live shell; `spawn_for` rolled back only a spawn
        // failure, so readiness is this function's own responsibility to
        // undo.
        with_shells(reg, |store| {
            store.remove(id.as_str());
        });
        return Err(e).with_context(|| {
            format!(
                "shell {} never became ready at {} within {ready_timeout:?}",
                id.as_str(),
                socket.display()
            )
        });
    }
    Ok((id, socket))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::shell::PtyBackend;
    use crate::shell::client;
    use crate::shell::host;
    use crate::shell::registry::ShellRecord;
    use crate::shell::registry::mark_interactive;
    use crate::shell::registry::spawn_for_in_sandbox;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Point `HOME` at a fresh temp dir for the duration of the test and
    /// serialise against other HOME-mutating tests (bug-05c-1). Callers hold
    /// the returned guard for the whole test so every `launch::socket_path` /
    /// `launch::lock_path` resolution — including inside the spawned host —
    /// lands under this same temp `HOME`.
    fn test_home() -> (crate::TestHomeGuard, tempfile::TempDir) {
        let guard = crate::test_home_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe {
            std::env::set_var("HOME", dir.path());
        }
        Config::ensure_dirs().ok();
        (guard, dir)
    }

    /// The `spawn` substitute tests pass instead of `spawn_detached_host`
    /// (F4): an in-process host that serves the *configured* socket for `id`
    /// and holds (leaked) the per-id instance lock, so it reads as alive to
    /// later `host_is_alive` probes exactly as a real host would. The caller's
    /// `test_home` guard must still be held when this runs.
    async fn spawn_in_process_host(id: ShellId) -> anyhow::Result<()> {
        let dir = tempfile::tempdir().context("tempdir for cast")?;
        let cast = dir.path().join(format!("{}.cast", id.as_str()));
        let backend =
            PtyBackend::spawn("bash", &[], 24, 80, 100, &cast, 0).context("spawning bash")?;
        let lock = crate::daemon::instance::InstanceLock::acquire(&launch::lock_path(id.as_str()))
            .context("holding the per-id instance lock")?;
        let socket = launch::socket_path(id.as_str());
        let listener = host::bind(&socket)
            .await
            .context("binding the in-process host's socket")?;
        tokio::spawn(host::serve(listener, Arc::clone(&backend)));
        std::mem::forget(lock);
        std::mem::forget(dir);
        Ok(())
    }

    #[tokio::test]
    async fn session_creates_a_shell_when_the_owner_has_none() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("a".into());

        let (id, socket) = shell_for_owner(
            &reg,
            owner.clone(),
            crate::host::HostId::Local,
            5,
            Duration::from_secs(15),
            spawn_in_process_host,
        )
        .await
        .expect("a fresh owner gets a shell");

        assert_eq!(socket, launch::socket_path(id.as_str()));

        let outcome = client::run_command(
            &socket,
            "printf 'SESSION_OK\\n'; sh -c 'exit 7'",
            "bash",
            Duration::from_secs(10),
        )
        .await
        .expect("the command completes inside the timeout");
        assert_eq!(outcome.exit_code, 7);
        let text = String::from_utf8_lossy(&outcome.output);
        assert!(
            text.contains("SESSION_OK"),
            "output must contain the command's bytes: {text:?}"
        );

        let guard = reg.try_lock().unwrap();
        let record = guard.get(id.as_str()).expect("the record is registered");
        assert!(matches!(record.owner, Owner::Chat(ref o) if o == "a"));
    }

    #[tokio::test]
    async fn session_reuses_the_owners_live_shell() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("b".into());
        let spawn_calls = Arc::new(AtomicUsize::new(0));

        let host_spawn = {
            let counter = Arc::clone(&spawn_calls);
            move |id: ShellId| {
                counter.fetch_add(1, Ordering::SeqCst);
                spawn_in_process_host(id)
            }
        };

        let (first_id, _) = shell_for_owner(
            &reg,
            owner.clone(),
            crate::host::HostId::Local,
            5,
            Duration::from_secs(15),
            host_spawn,
        )
        .await
        .expect("the first request creates a shell");

        let (second_id, _) = shell_for_owner(
            &reg,
            owner.clone(),
            crate::host::HostId::Local,
            5,
            Duration::from_secs(15),
            |_id| async { Ok(()) },
        )
        .await
        .expect("the second request reuses the shell");

        assert_eq!(
            first_id.as_str(),
            second_id.as_str(),
            "the same owner keeps its shell id"
        );
        assert_eq!(
            spawn_calls.load(Ordering::SeqCst),
            1,
            "the live shell must be reused, not respawned"
        );
    }

    #[tokio::test]
    async fn session_gives_different_owners_different_shells() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();

        let (ida, _) = shell_for_owner(
            &reg,
            Owner::Chat("a".into()),
            crate::host::HostId::Local,
            5,
            Duration::from_secs(15),
            spawn_in_process_host,
        )
        .await
        .expect("owner a gets a shell");
        let (idb, _) = shell_for_owner(
            &reg,
            Owner::Chat("b".into()),
            crate::host::HostId::Local,
            5,
            Duration::from_secs(15),
            spawn_in_process_host,
        )
        .await
        .expect("owner b gets a shell");

        assert_ne!(
            ida.as_str(),
            idb.as_str(),
            "distinct owners must not share a shell"
        );
        let guard = reg.try_lock().unwrap();
        assert_eq!(guard.len(), 2, "the registry holds both records");
    }

    /// A shell on a named host runs commands *there*; handing it to a caller
    /// that asked for no host would silently relocate the command, so the
    /// reuse filter excludes it even for its own owner.
    #[tokio::test]
    async fn a_remote_shell_is_never_reused_for_a_local_command() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("a".into());

        with_shells(&reg, |store| {
            store.insert(
                "s2".to_string(),
                ShellRecord {
                    id: ShellId::from_index(2),
                    owner: owner.clone(),
                    host: crate::host::HostId::Named("pinky".to_string()),
                    sandbox: None,
                    started_unix: 0,
                    interactive: false,
                    dedicated: false,
                    exited_unix: None,
                    last_foreground: None,
                    in_flight: None,
                },
            );
        });
        let (id, _) = shell_for_owner(
            &reg,
            owner.clone(),
            crate::host::HostId::Local,
            5,
            Duration::from_secs(15),
            spawn_in_process_host,
        )
        .await
        .expect("the local request must create a fresh shell");

        assert_ne!(
            id.as_str(),
            "s2",
            "the remote shell must not be handed to a caller that named no host"
        );
        let still_there = with_shells(&reg, |store| {
            store
                .get("s2")
                .map(|r| matches!(r.host, crate::host::HostId::Named(ref n) if n == "pinky"))
        });
        assert_eq!(
            still_there,
            Some(true),
            "the remote shell must stay registered and untouched"
        );
    }

    #[tokio::test]
    async fn session_replaces_a_shell_whose_host_is_dead() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("a".into());

        // Seed a stale record at a high index whose lock file has no live
        // holder. The reuse scan finds it dead, drops it, and the fresh
        // allocation fills the hole at 0 — a different id from the stale one.
        with_shells(&reg, |store| {
            store.insert(
                "s5".to_string(),
                ShellRecord {
                    id: ShellId::from_index(5),
                    owner: owner.clone(),
                    host: crate::host::HostId::Local,
                    sandbox: None,
                    started_unix: 0,
                    interactive: false,
                    dedicated: false,
                    exited_unix: None,
                    last_foreground: None,
                    in_flight: None,
                },
            );
        });

        let (id, socket) = shell_for_owner(
            &reg,
            owner.clone(),
            crate::host::HostId::Local,
            5,
            Duration::from_secs(15),
            spawn_in_process_host,
        )
        .await
        .expect("a dead host is replaced by a fresh shell");

        assert_ne!(id.as_str(), "s5", "the stale id must not be returned");
        assert_eq!(socket, launch::socket_path(id.as_str()));

        let guard = reg.try_lock().unwrap();
        assert!(
            guard.get("s5").is_none(),
            "the stale record must be dropped"
        );
        assert!(
            guard.get(id.as_str()).is_some(),
            "the fresh record must be registered"
        );
        assert_eq!(guard.len(), 1, "no extra records left behind");
    }

    #[tokio::test]
    async fn session_honours_the_per_owner_cap() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("a".into());

        // The owner holds a live shell under a different id (s1) than the
        // dead lower-indexed one the reuse scan hits first (s0). The scan
        // drops the dead s0 and falls through to create; the cap then counts
        // the live s1 and refuses.
        with_shells(&reg, |store| {
            store.insert(
                "s0".to_string(),
                ShellRecord {
                    id: ShellId::from_index(0),
                    owner: owner.clone(),
                    host: crate::host::HostId::Local,
                    sandbox: None,
                    started_unix: 0,
                    interactive: false,
                    dedicated: false,
                    exited_unix: None,
                    last_foreground: None,
                    in_flight: None,
                },
            );
            store.insert(
                "s1".to_string(),
                ShellRecord {
                    id: ShellId::from_index(1),
                    owner: owner.clone(),
                    host: crate::host::HostId::Local,
                    sandbox: None,
                    started_unix: 0,
                    interactive: false,
                    dedicated: false,
                    exited_unix: None,
                    last_foreground: None,
                    in_flight: None,
                },
            );
        });
        let lock = launch::lock_path("s1");
        let _held = crate::daemon::instance::InstanceLock::acquire(&lock).expect("hold s1");

        let result = shell_for_owner(
            &reg,
            owner,
            crate::host::HostId::Local,
            1,
            Duration::from_secs(15),
            |_id| async { Ok(()) },
        )
        .await;

        let err = result.expect_err("a request over the cap must Err, not panic");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("cap") && msg.contains("1"),
            "the error must name the cap: {msg}"
        );
    }

    #[tokio::test]
    async fn session_rolls_back_the_record_when_readiness_times_out() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("a".into());

        // The spawn succeeds without binding a socket, so `await_ready` cannot
        // succeed and must time out.
        let result = shell_for_owner(
            &reg,
            owner,
            crate::host::HostId::Local,
            5,
            Duration::from_millis(50),
            |_id| async { Ok(()) },
        )
        .await;

        assert!(result.is_err(), "a host that never becomes ready must Err");
        let guard = reg.try_lock().unwrap();
        assert!(
            guard.is_empty(),
            "the failed shell's record must be rolled back"
        );
    }

    /// 08b: an interactive command captures the owner's shell, so that record
    /// must not be offered for reuse — but the record itself stays and the
    /// host stays alive (M23 attaches to it later by id).
    #[tokio::test]
    async fn session_reuse_skips_a_shell_marked_interactive() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("captured".into());

        let (marked_id, _) = shell_for_owner(
            &reg,
            owner.clone(),
            crate::host::HostId::Local,
            5,
            Duration::from_secs(15),
            spawn_in_process_host,
        )
        .await
        .expect("the owner's first shell comes up");

        mark_interactive(&reg, &marked_id);

        // The marked shell is still in the registry, untouched.
        {
            let guard = reg.try_lock().unwrap();
            let record = guard
                .get(marked_id.as_str())
                .expect("the marked record must remain in the registry");
            assert!(
                record.interactive,
                "mark_interactive must have set the flag"
            );
        }

        let (second_id, _) = shell_for_owner(
            &reg,
            owner.clone(),
            crate::host::HostId::Local,
            5,
            Duration::from_secs(15),
            spawn_in_process_host,
        )
        .await
        .expect("the next command must get a fresh shell, not the captured one");

        assert_ne!(
            marked_id.as_str(),
            second_id.as_str(),
            "a shell marked interactive must never be reused"
        );

        let outcome = client::run_command(
            &launch::socket_path(second_id.as_str()),
            "echo NEW_SHELL_READY",
            "bash",
            Duration::from_secs(10),
        )
        .await
        .expect("the fresh shell answers");
        let text = String::from_utf8_lossy(&outcome.output);
        assert!(
            text.contains("NEW_SHELL_READY"),
            "the replacement shell must be usable: {text:?}"
        );

        // And the marked record survived the whole exchange.
        {
            let guard = reg.try_lock().unwrap();
            assert!(
                guard.contains_key(marked_id.as_str()),
                "the captured shell's record must not be removed or killed"
            );
        }
    }

    #[tokio::test]
    async fn session_dedicated_shell_is_never_reused_by_shell_for_owner() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("bg".into());

        // Seed the owner a live-looking shell that had been created for
        // exactly one command's use.
        with_shells(&reg, |store| {
            store.insert(
                "s0".to_string(),
                ShellRecord {
                    id: ShellId::from_index(0),
                    owner: owner.clone(),
                    host: crate::host::HostId::Local,
                    sandbox: None,
                    started_unix: 0,
                    interactive: false,
                    dedicated: true,
                    exited_unix: None,
                    last_foreground: None,
                    in_flight: None,
                },
            );
        });
        let lock = launch::lock_path("s0");
        let _held = crate::daemon::instance::InstanceLock::acquire(&lock).expect("hold s0");

        let (next_id, _) = shell_for_owner(
            &reg,
            owner.clone(),
            crate::host::HostId::Local,
            5,
            Duration::from_secs(15),
            spawn_in_process_host,
        )
        .await
        .expect("the owner's foreground command must get a shell");

        assert_ne!(
            next_id.as_str(),
            "s0",
            "a dedicated shell is one command's exclusive property; the owner's next foreground command must not land in it"
        );

        let outcome = client::run_command(
            &launch::socket_path(next_id.as_str()),
            "printf 'FRESH_FOREGROUND\\n'",
            "bash",
            Duration::from_secs(10),
        )
        .await
        .expect("the fresh shell answers");
        let text = String::from_utf8_lossy(&outcome.output);
        assert!(
            text.contains("FRESH_FOREGROUND"),
            "the replacement shell must be usable: {text:?}"
        );
    }

    #[tokio::test]
    async fn session_spawn_dedicated_for_creates_a_new_shell_even_when_one_is_reusable() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("bg".into());

        let (reusable_id, _) = shell_for_owner(
            &reg,
            owner.clone(),
            crate::host::HostId::Local,
            5,
            Duration::from_secs(15),
            spawn_in_process_host,
        )
        .await
        .expect("the owner's first shell comes up and looks reusable");

        let (dedicated_id, _) = spawn_dedicated_for(
            &reg,
            owner.clone(),
            5,
            Duration::from_secs(15),
            spawn_in_process_host,
        )
        .await
        .expect("a background command gets its own shell");

        assert_ne!(
            reusable_id.as_str(),
            dedicated_id.as_str(),
            "spawn_dedicated_for must never reuse, even a perfectly reusable shell"
        );

        let guard = reg.try_lock().unwrap();
        let record = guard
            .get(dedicated_id.as_str())
            .expect("the dedicated shell's record is registered");
        assert!(
            record.dedicated,
            "the shell it allocated must carry the dedicated flag"
        );
        assert!(
            !guard
                .get(reusable_id.as_str())
                .expect("the foreground record must be untouched")
                .dedicated,
            "the pre-existing foreground shell keeps its status"
        );
    }

    #[tokio::test]
    async fn session_spawn_dedicated_for_removes_the_record_when_readiness_never_arrives() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("bg".into());

        // The spawn succeeds without binding a socket, so only the readiness
        // wait can fail — exactly the case the rollback exists for.
        let result = spawn_dedicated_for(&reg, owner, 5, Duration::from_millis(50), |_id| async {
            Ok(())
        })
        .await;

        assert!(result.is_err(), "a host that never becomes ready must Err");
        let guard = reg.try_lock().unwrap();
        assert!(
            guard.is_empty(),
            "a record for a host that never came up must be rolled back, not handed to the next caller"
        );
    }

    /// Set `$SHELL` to fish for the assertion inside, then restore it. The
    /// fish value is what mutation B must turn red on; the test itself owns
    /// both the set and the cleanup because the caller cannot observe it.
    fn with_fish_shell(f: impl FnOnce()) {
        unsafe {
            std::env::set_var("SHELL", "/usr/bin/fish");
        }
        f();
        unsafe {
            std::env::remove_var("SHELL");
        }
    }

    /// The bug this phase exists for: a **remote** shell's marker must use
    /// POSIX `$?` even when the daemon host's `$SHELL` is fish, because the
    /// remote shell — not the daemon's — interprets the marker. fish's
    /// `$status`, expanded by bash on the far side, is empty and no exit code
    /// would ever parse.
    #[test]
    fn a_remote_shell_uses_the_posix_exit_variable() {
        let _guard = crate::test_home_guard();
        let named = crate::host::HostId::Named("pinky".to_string());
        with_fish_shell(|| {
            assert_eq!(
                crate::shell::pty::exit_var(&marker_shell_name(&named)),
                "$?",
                "a remote shell must ride POSIX $?, not the daemon's $SHELL"
            );
        });
        // Without the env var set the answer is the same, but the test above
        // is the load-bearing one: it is exactly the state mutation B turns
        // red in.
        assert_eq!(
            crate::shell::pty::exit_var(&marker_shell_name(&named)),
            "$?"
        );
    }

    /// A local shell's marker name is the daemon's login shell, unchanged from
    /// before hosts existed: `exit_var` keeps mapping fish/csh/tcsh to
    /// `$status`. Pinned because the remote branch must not leak into it.
    #[test]
    fn a_local_shell_still_uses_the_login_shell() {
        let _guard = crate::test_home_guard();
        unsafe {
            std::env::set_var("SHELL", "/usr/bin/fish");
        }
        let name = marker_shell_name(&crate::host::HostId::Local);
        assert_eq!(name, "/usr/bin/fish", "a local shell names login_shell()");
        assert_eq!(
            crate::shell::pty::exit_var(&name),
            "$status",
            "local fish keeps $status"
        );
        unsafe {
            std::env::remove_var("SHELL");
        }
        assert_eq!(
            marker_shell_name(&crate::host::HostId::Local),
            "bash",
            "no $SHELL falls back to bash, as login_shell always did"
        );
    }

    /// A sandbox shell's child is always `bash`, so its marker name is POSIX
    /// `"sh"` (`$?`) regardless of the daemon's `$SHELL` — the same rule a
    /// remote shell gets. A record with no sandbox keeps the host's name.
    #[test]
    fn shell_marker_name_is_posix_inside_a_sandbox() {
        let _guard = crate::test_home_guard();
        let boxed = ShellRecord {
            id: ShellId::from_index(9),
            owner: Owner::Chat("a".into()),
            host: crate::host::HostId::Local,
            sandbox: Some("de-ghost-r1".to_string()),
            started_unix: 0,
            interactive: false,
            dedicated: false,
            exited_unix: None,
            last_foreground: None,
            in_flight: None,
        };
        let plain = ShellRecord {
            id: ShellId::from_index(10),
            owner: Owner::Chat("a".into()),
            host: crate::host::HostId::Local,
            sandbox: None,
            started_unix: 0,
            interactive: false,
            dedicated: false,
            exited_unix: None,
            last_foreground: None,
            in_flight: None,
        };
        with_fish_shell(|| {
            assert_eq!(
                shell_marker_name(&boxed),
                "sh",
                "a sandbox shell's child is bash; its marker must ride POSIX $?"
            );
            assert_eq!(
                shell_marker_name(&plain),
                login_shell(),
                "a shell with no sandbox keeps its host's marker name"
            );
        });
    }

    /// A host the config does not carry is refused by name, and the refusal
    /// lists the ones that *are* configured so the model can correct itself
    /// in one turn.
    #[test]
    fn an_unknown_host_is_refused_by_name() {
        let (_home, dir) = test_home();
        std::fs::write(
            dir.path().join(".daemoneye/etc/config.toml"),
            "[models.default]\nprovider = \"ollama\"\n\n[hosts.web01]\n\
             ssh = \"matt@web01.lan\"\n\n[hosts.db01]\nssh = \"db01\"\n",
        )
        .expect("write config");
        let err = host_entry("pinkly").expect_err("an unknown host is refused");
        let msg = err.to_string();
        assert!(
            msg.contains("pinkly"),
            "the refusal must name the unknown host: {msg}"
        );
        assert!(
            msg.contains("web01") && msg.contains("db01"),
            "the refusal must list the configured hosts: {msg}"
        );
    }

    /// The implicit local host is not a `[hosts]` entry and this spawn path is
    /// for named hosts only; the message points at the local path that already
    /// exists instead of trying to ssh loopback.
    #[test]
    fn the_local_host_is_refused_here() {
        let _guard = crate::test_home_guard();
        for spelling in ["local", "Local", "LOCAL", "lOcAl"] {
            let err = host_entry(spelling).expect_err("the local host is refused");
            let msg = err.to_string();
            assert!(
                msg.contains("local"),
                "the refusal must name the local host: {msg}"
            );
            assert!(
                msg.contains("implicit") || msg.contains("already runs"),
                "the refusal must point at the existing local path: {msg}"
            );
        }
    }

    // ── host_entry_or_bare (M21 phase-06c) ──────────────────────────────────

    #[test]
    fn a_configured_host_returns_its_configured_entry() {
        let (_home, dir) = test_home();
        std::fs::write(
            dir.path().join(".daemoneye/etc/config.toml"),
            "[models.default]\nprovider = \"ollama\"\n\n[hosts.pinky]\n\
             ssh = \"pinky-real\"\n",
        )
        .expect("write config");
        let entry = host_entry_or_bare("pinky").expect("a configured host resolves permissively");
        assert_eq!(
            entry.ssh, "pinky-real",
            "configuration still wins: the entry's own destination, not the bare name"
        );
    }

    #[test]
    fn an_unconfigured_host_becomes_its_own_ssh_destination() {
        let (_home, dir) = test_home();
        std::fs::write(
            dir.path().join(".daemoneye/etc/config.toml"),
            "[models.default]\nprovider = \"ollama\"\n",
        )
        .expect("write config");
        let entry = host_entry_or_bare("nope").expect("an unconfigured host is allowed");
        assert_eq!(
            entry.ssh, "nope",
            "the bare name is its own ssh(1) destination, honouring ~/.ssh/config"
        );
        assert!(
            entry.tags.is_empty() && entry.sudo_scripts.is_empty(),
            "a synthesised entry carries nothing else"
        );
    }

    #[test]
    fn a_configured_host_with_an_empty_ssh_is_still_an_error() {
        let (_home, dir) = test_home();
        std::fs::write(
            dir.path().join(".daemoneye/etc/config.toml"),
            "[models.default]\nprovider = \"ollama\"\n\n[hosts.half]\n",
        )
        .expect("write config");
        let err =
            host_entry_or_bare("half").expect_err("a half-written entry is an error, not a guess");
        let msg = err.to_string();
        assert!(
            msg.contains("half") && msg.contains("ssh destination is empty"),
            "the refusal names the entry and its empty destination: {msg}"
        );
        assert!(
            !msg.contains("half\""),
            "the empty destination must never be replaced by the bare name: {msg}"
        );
    }

    #[test]
    fn the_local_host_is_still_refused_by_the_permissive_lookup() {
        let _guard = crate::test_home_guard();
        for spelling in ["local", "LOCAL", "Local"] {
            let err = host_entry_or_bare(spelling)
                .expect_err("the local host is never a remote destination");
            let msg = err.to_string();
            assert!(
                msg.contains("implicit"),
                "{spelling}'s refusal must point at the implicit host: {msg}"
            );
        }
    }

    #[test]
    fn the_strict_lookup_still_refuses_an_unconfigured_host() {
        let (_home, dir) = test_home();
        std::fs::write(
            dir.path().join(".daemoneye/etc/config.toml"),
            "[models.default]\nprovider = \"ollama\"\n",
        )
        .expect("write config");
        let err = host_entry("nope").expect_err("the strict lookup refuses an unknown host");
        let msg = err.to_string();
        assert!(
            msg.contains("unknown host"),
            "host_entry keeps its own refusal wording: {msg}"
        );
        assert!(
            msg.contains("no hosts are configured"),
            "the strict refusal still tells the model what to do: {msg}"
        );
    }

    /// Task 3's positive case: a caller asking for the shell's own host gets
    /// that shell back, reused, without a spawn.
    #[tokio::test]
    async fn shell_for_owner_reuses_a_shell_on_the_requested_host() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("hosted".into());

        let (opened, _) = shell_for_owner(
            &reg,
            owner.clone(),
            crate::host::HostId::Named("pinky".into()),
            5,
            Duration::from_secs(15),
            spawn_in_process_host,
        )
        .await
        .expect("the owner's pinky shell comes up");

        let (asked, _) = shell_for_owner(
            &reg,
            owner,
            crate::host::HostId::Named("pinky".into()),
            5,
            Duration::from_secs(15),
            spawn_in_process_host,
        )
        .await
        .expect("the second request on the same host is answered");

        assert_eq!(
            asked.as_str(),
            opened.as_str(),
            "a caller asking for the host a live shell runs on reuses that shell"
        );
    }

    /// Task 3's negative case: a host the shell is not on must not be handed
    /// out — the caller gets a new shell, and the remote shell is *skipped*,
    /// left in the registry for later work that names its host.
    #[tokio::test]
    async fn shell_for_owner_does_not_reuse_a_shell_from_another_host() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("mixed".into());

        let (remote_id, _) = shell_for_owner(
            &reg,
            owner.clone(),
            crate::host::HostId::Named("pinky".into()),
            5,
            Duration::from_secs(15),
            spawn_in_process_host,
        )
        .await
        .expect("the owner's pinky shell comes up");

        let (local_id, _) = shell_for_owner(
            &reg,
            owner.clone(),
            crate::host::HostId::Local,
            5,
            Duration::from_secs(15),
            spawn_in_process_host,
        )
        .await
        .expect("the local request is answered with a shell");

        assert_ne!(
            local_id.as_str(),
            remote_id.as_str(),
            "the local caller must not land in the pinky shell"
        );

        let (remote_rows, local_rows) = with_shells(&reg, |store| {
            (
                store
                    .values()
                    .filter(|r| r.id.as_str() == remote_id.as_str())
                    .count(),
                store
                    .values()
                    .filter(|r| r.id.as_str() == local_id.as_str())
                    .count(),
            )
        });
        assert_eq!(
            remote_rows, 1,
            "the pinky shell must be skipped, not reaped: it is still in the registry"
        );
        assert_eq!(
            local_rows, 1,
            "the fresh local shell is the one the local caller got"
        );
    }

    /// Task 4's negative case: an ordinary `shell_for_owner` caller is never
    /// handed a shell that lives in a sandbox — its commands would run in a
    /// VM. The sandbox shell is skipped, left in the registry for later work
    /// that asks for that sandbox by name, and the caller gets a fresh host
    /// shell.
    #[tokio::test]
    async fn shell_for_owner_never_reuses_a_sandbox_shell() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("boxed".into());

        let boxed_id = spawn_for_in_sandbox(
            &reg,
            owner.clone(),
            5,
            "de-chat-t".into(),
            false,
            spawn_in_process_host,
        )
        .await
        .expect("the sandbox shell's host comes up");
        launch::await_ready(
            &launch::socket_path(boxed_id.as_str()),
            std::time::Instant::now() + Duration::from_secs(15),
        )
        .await
        .expect("the sandbox shell's host must be alive, or the filter is not under test");

        let (local_id, _) = shell_for_owner(
            &reg,
            owner,
            crate::host::HostId::Local,
            5,
            Duration::from_secs(15),
            spawn_in_process_host,
        )
        .await
        .expect("the local request is answered with a shell");

        assert_ne!(
            local_id.as_str(),
            boxed_id.as_str(),
            "an ordinary caller must not land in a sandbox shell"
        );

        let boxed_rows = with_shells(&reg, |store| {
            store
                .values()
                .filter(|r| r.id.as_str() == boxed_id.as_str())
                .count()
        });
        assert_eq!(
            boxed_rows, 1,
            "the sandbox shell must be skipped, not reaped: it is still in the registry"
        );
    }

    // ── shell_for_owner_in_sandbox (M26 phase-06b) ──────────────────────────

    #[tokio::test]
    async fn shell_for_owner_in_sandbox_creates_then_reuses() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("boxed".into());
        let spawn_calls = Arc::new(AtomicUsize::new(0));

        let host_spawn = {
            let counter = Arc::clone(&spawn_calls);
            move |id: ShellId| {
                counter.fetch_add(1, Ordering::SeqCst);
                spawn_in_process_host(id)
            }
        };

        let (first, _) = shell_for_owner_in_sandbox(
            &reg,
            owner.clone(),
            "de-ghost-a",
            5,
            Duration::from_secs(15),
            host_spawn,
        )
        .await
        .expect("the first request creates the owner's sandbox shell");

        let (second, socket) = shell_for_owner_in_sandbox(
            &reg,
            owner,
            "de-ghost-a",
            5,
            Duration::from_secs(15),
            spawn_in_process_host,
        )
        .await
        .expect("the second request is answered");

        assert_eq!(
            second.as_str(),
            first.as_str(),
            "the same owner asking twice for the same sandbox must get the same shell"
        );
        assert_eq!(socket, launch::socket_path(second.as_str()));

        assert_eq!(
            spawn_calls.load(Ordering::SeqCst),
            1,
            "the live sandbox shell must be reused, not respawned"
        );

        let sandbox = with_shells(&reg, |store| {
            store.get(second.as_str()).and_then(|r| r.sandbox.clone())
        });
        assert_eq!(
            sandbox.as_deref(),
            Some("de-ghost-a"),
            "the record must carry the sandbox its child runs in"
        );
    }

    #[tokio::test]
    async fn shell_for_owner_in_sandbox_keeps_sandboxes_apart() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("boxed".into());

        let (in_a, _) = shell_for_owner_in_sandbox(
            &reg,
            owner.clone(),
            "de-ghost-a",
            5,
            Duration::from_secs(15),
            spawn_in_process_host,
        )
        .await
        .expect("owner's shell in de-ghost-a comes up");

        let (in_b, _) = shell_for_owner_in_sandbox(
            &reg,
            owner,
            "de-ghost-b",
            5,
            Duration::from_secs(15),
            spawn_in_process_host,
        )
        .await
        .expect("owner's shell in de-ghost-b comes up");

        assert_ne!(
            in_a.as_str(),
            in_b.as_str(),
            "two sandboxes must never share the owner's shell — each runs a \
             different VM"
        );

        let (sandbox_a, sandbox_b) = with_shells(&reg, |store| {
            (
                store.get(in_a.as_str()).and_then(|r| r.sandbox.clone()),
                store.get(in_b.as_str()).and_then(|r| r.sandbox.clone()),
            )
        });
        assert_eq!(sandbox_a.as_deref(), Some("de-ghost-a"));
        assert_eq!(sandbox_b.as_deref(), Some("de-ghost-b"));
    }

    #[tokio::test]
    async fn shell_for_owner_in_sandbox_never_returns_a_host_shell() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("boxed".into());

        let (plain, _) = shell_for_owner(
            &reg,
            owner.clone(),
            crate::host::HostId::Local,
            5,
            Duration::from_secs(15),
            spawn_in_process_host,
        )
        .await
        .expect("the owner's host shell comes up");

        let (boxed, _) = shell_for_owner_in_sandbox(
            &reg,
            owner,
            "de-ghost-a",
            5,
            Duration::from_secs(15),
            spawn_in_process_host,
        )
        .await
        .expect("the sandbox request must create its own shell, not reuse the host one");

        assert_ne!(
            boxed.as_str(),
            plain.as_str(),
            "a sandbox command must never land in a host shell — its commands \
             would run on the daemon host, outside the VM"
        );

        let (plain_sandbox, plain_host) = with_shells(&reg, |store| {
            store
                .get(plain.as_str())
                .map(|r| (r.sandbox.clone(), r.host.clone()))
        })
        .expect("the host shell's record stays registered");
        assert_eq!(
            plain_sandbox, None,
            "the host shell keeps its plain (no-sandbox) record"
        );
        assert!(
            matches!(plain_host, crate::host::HostId::Local),
            "the host shell is untouched by the sandbox request"
        );
    }

    #[tokio::test]
    async fn shell_for_owner_in_sandbox_ignores_another_owners_shell() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner_a = Owner::Chat("a".into());
        let owner_b = Owner::Chat("b".into());

        let (a_shell, _) = shell_for_owner_in_sandbox(
            &reg,
            owner_a,
            "de-ghost-a",
            5,
            Duration::from_secs(15),
            spawn_in_process_host,
        )
        .await
        .expect("owner A's sandbox shell comes up");

        let (b_shell, _) = shell_for_owner_in_sandbox(
            &reg,
            owner_b,
            "de-ghost-a",
            5,
            Duration::from_secs(15),
            spawn_in_process_host,
        )
        .await
        .expect("owner B's request in the same sandbox is answered with its own shell");

        assert_ne!(
            b_shell.as_str(),
            a_shell.as_str(),
            "owner B must never be handed owner A's shell, even in the same sandbox"
        );

        let a_owner_key = with_shells(&reg, |store| {
            store
                .get(a_shell.as_str())
                .map(|r| r.owner.key().to_string())
        });
        assert_ne!(
            a_owner_key.as_deref(),
            Some(Owner::Chat("b".into()).key().as_str()),
            "owner A's record keeps owner A"
        );
    }

    #[test]
    fn clear_sandbox_sidecar_removes_only_that_ids_sidecar() {
        let _home = crate::test_home_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        write_sandbox_sidecar(dir.path(), "s1", "de-ghost-r1").expect("sidecar s1 writable");
        write_sandbox_sidecar(dir.path(), "s2", "de-ghost-r2").expect("sidecar s2 writable");

        clear_sandbox_sidecar(dir.path(), "s1");

        assert!(
            !sandbox_sidecar_path(dir.path(), "s1").exists(),
            "s1's sidecar must be gone"
        );
        assert!(
            sandbox_sidecar_path(dir.path(), "s2").exists(),
            "s2's sidecar must survive"
        );

        clear_sandbox_sidecar(dir.path(), "s1");

        assert!(
            sandbox_sidecar_path(dir.path(), "s2").exists(),
            "clearing a missing sidecar must not touch the others"
        );
    }
}

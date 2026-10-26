//! Getting a ready shell socket for an owner: reuse the owner's live shell or
//! create one, wait for it to answer readiness, and keep the registry honest
//! when a host never comes up.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Context;

use crate::config::{Config, ShellsConfig};
use crate::host::ssh_argv;
use crate::shell::foreground::Foreground;
use crate::shell::launch;
use crate::shell::registry::{
    Owner, ShellId, ShellRecord, ShellRegistry, host_is_alive, sandbox_sidecar_path, spawn_for,
    spawn_for_on_host, with_shells, write_sandbox_sidecar,
};

/// How long `probe_foreground` waits for one `Status`/`Screen` round-trip.
/// Reuse probes run while a chat turn waits on the shell it asked for, so
/// this is short: a live local host answers in microseconds, and a probe
/// that outlives 2 s has classified a dead or wedged host, which reads
/// `Unknown` — the busy answer a fresh shell is the remedy for. The
/// refusal probe in `run_terminal_command` reuses it, so every probe in the
/// daemon waits the same amount.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Probe a shell's live foreground state and record it.
///
/// One `Status` round-trip over the shell's socket, and — only for a
/// **transport** record, a named host or a sandbox — one `Screen` read whose
/// last non-blank row is what the transport rule classifies from. Then
/// [`classify_record`] decides, and the result lands on the record's
/// `last_foreground` with the current unix second.
///
/// Any socket error ⇒ the status (and row) is `None`, which classifies
/// `Unknown`: the shell could not be asked, so nothing is asserted about it.
///
/// The registry lock is never held across an `.await`: the record's routing
/// facts are copied out first, the socket I/O runs with the lock released,
/// then the lock is taken again to classify and store.
pub async fn probe_foreground(reg: &ShellRegistry, id: &ShellId, timeout: Duration) -> Foreground {
    let (transport, in_registry) = with_shells(reg, |store| {
        store
            .get(id.as_str())
            .map(|record| {
                (
                    record.sandbox.is_some()
                        || matches!(record.host, crate::host::HostId::Named(_)),
                    true,
                )
            })
            .unwrap_or((false, false))
    });
    if !in_registry {
        return Foreground::Unknown;
    }
    let sock = launch::socket_path(id.as_str());
    let status = crate::shell::status(&sock, timeout).await.ok();
    let last_row: Option<String> = if transport {
        // A freshly spawned transport shell has a blank screen until its
        // prompt draws; the transport rule reads blank as busy, so re-read
        // briefly before classifying — the prompt is on its way.
        let mut row: Option<String> = None;
        for _ in 0..10 {
            match crate::shell::screen(&sock, false, 0, timeout).await {
                Ok(crate::shell::ShellResponse::ScreenText { text, .. }) => {
                    if let Some(last) = text.lines().rfind(|l| !l.trim().is_empty()) {
                        row = Some(last.to_string());
                        break;
                    }
                }
                _ => break,
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        row
    } else {
        None
    };
    let foreground = with_shells(reg, |store| {
        let observed_at = crate::shell::registry::now_unix();
        store.get_mut(id.as_str()).map(|record| {
            let fg = crate::shell::classify_record(record, status.as_ref(), last_row.as_deref());
            record.last_foreground = Some((fg.clone(), observed_at));
            fg
        })
    });
    foreground.unwrap_or(Foreground::Unknown)
}

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
    // (1) The owner's live shells on `host`, lowest index first — the reuse
    // order. A match whose host is dead is removed here so it is not handed
    // to the next caller as a live shell; the rest are candidates the probe
    // below walks in order. A busy candidate is **skipped, never removed**:
    // the program holding it is mid-run and the shell stays addressable.
    let owner_key = owner.key();
    let candidates: Vec<ShellId> = with_shells(reg, |store| {
        let mut matches: Vec<ShellId> = store
            .values()
            .filter(|record| {
                // A dedicated shell is excluded: it belongs to exactly one
                // command, so it is never offered to anything else. Reuse is
                // host-keyed: a shell runs its commands on the host its
                // record names, so it is handed only to a caller asking for
                // that same host — anything else would silently relocate
                // the command. A sandbox shell runs its commands in a VM, so
                // it is handed only to a caller that asks for that sandbox,
                // never to `shell_for_owner`.
                !record.dedicated
                    && record.host == host
                    && record.sandbox.is_none()
                    && record.owner.key() == owner_key
            })
            .map(|record| record.id.clone())
            .collect();
        matches.sort_by_key(|id| id.index());
        matches.retain(|id| {
            if host_is_alive(&launch::lock_path(id.as_str())) {
                true
            } else {
                store.remove(id.as_str());
                false
            }
        });
        matches
    });

    // (2) The first candidate whose live foreground is a prompt — a busy
    // shell is skipped, never removed, so the program holding it is
    // untouched. D4: the decision probes live, never the cached
    // `last_foreground`.
    let mut idle: Option<ShellId> = None;
    for id in &candidates {
        if probe_foreground(reg, id, PROBE_TIMEOUT).await == crate::shell::Foreground::Prompt {
            idle = Some(id.clone());
            break;
        }
    }
    if let Some(id) = idle {
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
    // The same candidate scan `shell_for_owner` runs, keyed on the sandbox
    // instead of the host: a sandbox shell's own host is always `Local`, so
    // the host is not what distinguishes two of the owner's shells here — the
    // sandbox is. Dedicated shells are excluded for the same reason
    // `shell_for_owner` excludes them, and a match whose host is dead
    // is removed so it is not handed to the next caller as a live shell.
    let owner_key = owner.key();
    let candidates: Vec<ShellId> = with_shells(reg, |store| {
        let mut matches: Vec<ShellId> = store
            .values()
            .filter(|record| {
                !record.dedicated
                    && record.owner.key() == owner_key
                    && record.sandbox.as_deref() == Some(sandbox)
            })
            .map(|record| record.id.clone())
            .collect();
        matches.sort_by_key(|id| id.index());
        matches.retain(|id| {
            if host_is_alive(&launch::lock_path(id.as_str())) {
                true
            } else {
                store.remove(id.as_str());
                false
            }
        });
        matches
    });

    // The same probe rule: the first candidate at a prompt is reused, a busy
    // one is skipped and never removed.
    let mut idle: Option<ShellId> = None;
    for id in &candidates {
        if probe_foreground(reg, id, PROBE_TIMEOUT).await == crate::shell::Foreground::Prompt {
            idle = Some(id.clone());
            break;
        }
    }
    if let Some(id) = idle {
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

/// The `(rows, cols)` every production spawn passes: `[shells] rows`/`cols`
/// (D10), or `ShellsConfig::default().size()` when the config cannot be
/// loaded. One helper so every spawn site reads the same geometry and the
/// sites can never drift apart.
pub fn spawn_size() -> (u16, u16) {
    Config::load()
        .map(|cfg| cfg.shells.size())
        .unwrap_or_else(|_| ShellsConfig::default().size())
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
#[path = "session_tests.rs"]
mod tests;

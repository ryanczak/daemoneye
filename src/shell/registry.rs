//! The in-memory shell registry: id grammar, ownership, per-owner caps, and
//! slot allocation with spawn-outside-the-lock, plus startup adoption of the
//! shells surviving a daemon restart, the sweep of dead hosts' leftovers, and
//! the record reaper for exited shells.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::shell::foreground::Foreground;
use crate::util::UnpoisonExt;

/// A canonical shell id: `s` followed by 1–9 ASCII digits. The grammar is
/// owned by `launch::validate_shell_id`; `parse` delegates to it rather than
/// duplicating a second security boundary that would drift.
#[derive(Clone, Debug)]
pub struct ShellId(String);

impl ShellId {
    pub fn parse(s: &str) -> anyhow::Result<ShellId> {
        super::launch::validate_shell_id(s)?;
        Ok(ShellId(s.to_string()))
    }

    pub fn from_index(n: u32) -> ShellId {
        ShellId(format!("s{n}"))
    }

    /// The numeric part of the id.
    pub fn index(&self) -> u32 {
        let digits = &self.0[1..];
        // Both constructors guarantee a numeric suffix via the shared grammar,
        // so the split is always parseable.
        digits.parse().unwrap_or(0)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ShellId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Who owns a shell, and the unit per-owner caps are enforced over.
#[derive(Clone, Debug)]
pub enum Owner {
    Chat(String),
    Ghost(String),
    /// A scheduled job's shells (`ActionOn::Script`), keyed by the job id.
    Scheduler(String),
    /// A shell adopted at daemon startup from a live host left behind by a
    /// previous daemon. Per the measured facts in phase-06b, nothing on disk
    /// records the original owner, and the owning session is gone by
    /// definition — so an adopted shell is owned by nobody until a later phase
    /// re-claims it.
    Adopted,
    /// A shell the user opened with `/shell` (M23), from any chat session.
    User,
}

impl Owner {
    /// The cap-grouping key. The kind prefix makes a chat session and a ghost
    /// job that share an id string distinct owners.
    pub fn key(&self) -> String {
        match self {
            Owner::Chat(id) => format!("chat:{id}"),
            Owner::Ghost(id) => format!("ghost:{id}"),
            Owner::Scheduler(id) => format!("scheduler:{id}"),
            Owner::Adopted => "adopted".to_string(),
            Owner::User => "user".to_string(),
        }
    }

    /// Whether this caller may run commands in, or type into, a shell whose
    /// owner key is `shell_owner`: its own shells, and — for a chat session —
    /// the user's (`/shell`), which every such action reaches only through an
    /// approval that is always asked. A ghost never drives the user's shells.
    pub fn may_drive(&self, shell_owner: &str) -> bool {
        shell_owner == self.key()
            || (shell_owner == Owner::User.key() && matches!(self, Owner::Chat(_)))
    }
}

/// Whether `id` names a shell the user owns (opened with `/shell`).
pub fn is_user_shell(reg: &ShellRegistry, id: &str) -> bool {
    with_shells(reg, |store| {
        store
            .get(id)
            .is_some_and(|record| matches!(record.owner, Owner::User))
    })
}

/// One entry in the registry, keyed in the underlying map by the id string.
pub struct ShellRecord {
    pub id: ShellId,
    pub owner: Owner,
    /// Where this shell runs. `HostId::Local` for every shell the daemon
    /// spawns today; a named host arrives with the SSH spawn.
    pub host: crate::host::HostId,
    /// The sbx sandbox this shell's child runs in (`sbx exec -it <sandbox> bash`),
    /// or `None` for a shell on the host itself. A sandbox shell is only ever
    /// handed to a caller that asks for that sandbox.
    pub sandbox: Option<String>,
    pub started_unix: u64,
    /// Set when this shell was created to serve exactly one command for its
    /// whole life — a background run. Unlike `interactive` this is decided at
    /// creation, not discovered later: the shell is never offered to another
    /// command, so a background job and the owner's foreground commands can
    /// never land in the same shell (08b's poisoning rule).
    pub dedicated: bool,
    /// Unix seconds when this shell's host was **first observed** dead, set by
    /// the reaper. `None` while the host is alive. The record is kept for
    /// `[shells] exited_retention_secs` after this, so an operator can still
    /// see that the shell existed.
    pub exited_unix: Option<u64>,
    /// The most recent `Foreground` the daemon observed on this shell, with
    /// the unix second it was observed at. Written by every `Status`-probing
    /// read (`read_shell_screen` today), never asserted — the classification
    /// is an observation the prompt block and phase 02's decisions read, and
    /// decisions always probe live rather than trust it.
    pub last_foreground: Option<(Foreground, u64)>,
    /// The marker-wrapped command the daemon currently has outstanding on
    /// this shell, and the unix second it started. Set immediately before a
    /// wrapped command is written and cleared on every way the run ends, so
    /// it is the daemon's own "this shell is busy with *this*" fact; a
    /// transport shell's classification reads it (D3) because the local
    /// terminal cannot say anything there.
    pub in_flight: Option<InFlight>,
}

/// A wrapped command the daemon has outstanding on a shell: what it is and
/// when it was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InFlight {
    pub cmd: String,
    pub started_unix: u64,
}

/// Shared registry of live shells.
///
/// A newtype rather than an alias so the inner mutex cannot be locked from
/// outside this module: `with_shells` is the only way in, which is what keeps
/// blocking work out of the critical section.
#[derive(Clone, Default)]
pub struct ShellRegistry(Arc<Mutex<HashMap<String, ShellRecord>>>);

impl ShellRegistry {
    /// Test-only escape hatch for asserting the guard was released.
    ///
    /// Deliberately `#[cfg(test)]`: production code must go through
    /// `with_shells`, and a non-blocking peek is only ever needed to prove a
    /// lock is *not* held.
    #[cfg(test)]
    pub fn try_lock(
        &self,
    ) -> std::sync::TryLockResult<std::sync::MutexGuard<'_, HashMap<String, ShellRecord>>> {
        self.0.try_lock()
    }
}

/// Run `f` with exclusive access to the shell map. The guard's lifetime is the
/// closure body, so it cannot escape or be held across an `.await`. Never call
/// anything inside `f` that takes this lock again: `std::sync::Mutex` is not
/// reentrant and a nested acquisition would deadlock every IPC handler that
/// touches the registry. Collect what you need, return it, and act after the
/// closure returns.
pub fn with_shells<T>(
    reg: &ShellRegistry,
    f: impl FnOnce(&mut HashMap<String, ShellRecord>) -> T,
) -> T {
    let mut store = reg.0.lock().unwrap_or_log();
    f(&mut store)
}

/// Record that `cmd` is now outstanding on `id`'s shell: called immediately
/// before the wrapped command is written. A no-op when the id is not in the
/// registry.
pub fn set_in_flight(reg: &ShellRegistry, id: &ShellId, cmd: &str, started_unix: u64) {
    with_shells(reg, |store| {
        if let Some(record) = store.get_mut(id.as_str()) {
            record.in_flight = Some(InFlight {
                cmd: cmd.to_string(),
                started_unix,
            });
        }
    });
}

/// Clear the outstanding-command mark on `id`'s shell. A no-op when the id is
/// not in the registry.
pub fn clear_in_flight(reg: &ShellRegistry, id: &ShellId) {
    with_shells(reg, |store| {
        if let Some(record) = store.get_mut(id.as_str()) {
            record.in_flight = None;
        }
    });
}

/// A guard holding `in_flight` on one shell: set on construction, cleared on
/// **every** exit — including an early `?` — by its `Drop`, so no run can end
/// with the mark still set.
pub struct InFlightGuard<'a> {
    reg: &'a ShellRegistry,
    id: ShellId,
}

impl<'a> InFlightGuard<'a> {
    pub fn new(reg: &'a ShellRegistry, id: ShellId, cmd: &str, started_unix: u64) -> Self {
        set_in_flight(reg, &id, cmd, started_unix);
        Self { reg, id }
    }

    /// The shell this guard holds the mark on.
    pub fn id(&self) -> &ShellId {
        &self.id
    }
}

impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        clear_in_flight(self.reg, &self.id);
    }
}

/// The current unix second, `0` if the clock is somehow before the epoch.
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The lowest unused index, starting at 0. Fills holes: `{0, 1, 3}` yields `2`.
pub fn allocate_index(occupied: &HashSet<u32>) -> u32 {
    let mut candidate = 0;
    while occupied.contains(&candidate) {
        candidate += 1;
    }
    candidate
}

/// `Ok(())` when `owner` may take one more shell. `max_per_owner == 0` means
/// unlimited.
pub fn check_owner_cap(
    existing: &HashMap<String, ShellRecord>,
    owner: &Owner,
    max_per_owner: u32,
) -> anyhow::Result<()> {
    if max_per_owner == 0 {
        return Ok(());
    }
    let owner_key = owner.key();
    let held = existing
        .values()
        .filter(|record| record.owner.key() == owner_key)
        .count() as u32;
    if held >= max_per_owner {
        anyhow::bail!("owner '{owner_key}' already holds {held} shells (cap {max_per_owner})");
    }
    Ok(())
}

/// Reserve a slot for `owner` under the lock, then run the caller-provided
/// spawn against that id with the lock released. On a failed spawn the
/// reservation is undone before returning the error, so a slow or failing
/// spawn never stalls the registry and never leaks its slot.
///
/// The spawn is a parameter precisely so a caller decides what "spawn" means:
/// production passes a closure wrapping the detached host launch, tests pass
/// stubs over the pure registry state.
pub async fn spawn_for<F, Fut>(
    reg: &ShellRegistry,
    owner: Owner,
    max_per_owner: u32,
    dedicated: bool,
    spawn: F,
) -> anyhow::Result<ShellId>
where
    F: FnOnce(ShellId) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    spawn_for_on_host(
        reg,
        owner,
        max_per_owner,
        crate::host::HostId::Local,
        dedicated,
        spawn,
    )
    .await
}

/// `spawn_for` on a named host: the record it stamps carries `host`, so every
/// reader of `record.host` (`marker_shell_name`, the `[SHELLS]` block) treats
/// the shell as remote. Additive over `spawn_for` on purpose — the local form
/// stays a one-line delegation, keeping its twelve call sites compiling
/// untouched.
pub async fn spawn_for_on_host<F, Fut>(
    reg: &ShellRegistry,
    owner: Owner,
    max_per_owner: u32,
    host: crate::host::HostId,
    dedicated: bool,
    spawn: F,
) -> anyhow::Result<ShellId>
where
    F: FnOnce(ShellId) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    spawn_for_placed(reg, owner, max_per_owner, host, None, dedicated, spawn).await
}

/// `spawn_for` for a shell whose child runs inside the sbx sandbox `sandbox`.
/// The shell process itself is local; its commands run in the VM.
pub async fn spawn_for_in_sandbox<F, Fut>(
    reg: &ShellRegistry,
    owner: Owner,
    max_per_owner: u32,
    sandbox: String,
    dedicated: bool,
    spawn: F,
) -> anyhow::Result<ShellId>
where
    F: FnOnce(ShellId) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    spawn_for_placed(
        reg,
        owner,
        max_per_owner,
        crate::host::HostId::Local,
        Some(sandbox),
        dedicated,
        spawn,
    )
    .await
}

async fn spawn_for_placed<F, Fut>(
    reg: &ShellRegistry,
    owner: Owner,
    max_per_owner: u32,
    host: crate::host::HostId,
    sandbox: Option<String>,
    dedicated: bool,
    spawn: F,
) -> anyhow::Result<ShellId>
where
    F: FnOnce(ShellId) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    let id = with_shells(reg, |store| -> anyhow::Result<ShellId> {
        check_owner_cap(store, &owner, max_per_owner)?;
        let occupied = store.values().map(|record| record.id.index()).collect();
        let id = ShellId::from_index(allocate_index(&occupied));
        let started_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        store.insert(
            id.as_str().to_string(),
            ShellRecord {
                id: id.clone(),
                owner: owner.clone(),
                host,
                sandbox,
                started_unix,
                dedicated,
                exited_unix: None,
                last_foreground: None,
                in_flight: None,
            },
        );
        Ok(id)
    })?;

    if let Err(e) = spawn(id.clone()).await {
        with_shells(reg, |store| {
            store.remove(id.as_str());
        });
        return Err(e);
    }
    Ok(id)
}

/// Whether the host identified by `lock` is still alive. The flock is the
/// liveness oracle, not a socket connect: a bound-but-unserved socket is
/// indistinguishable from a healthy one mid-startup. A `Held` outcome means a
/// live host holds the lock; `Ok` means nobody does. An unreadable lock fails
/// **closed** — we could not prove the host dead, and the caller deletes files
/// based on this answer, so a probe that cannot verify must not report death.
pub fn host_is_alive(lock: &Path) -> bool {
    match crate::daemon::instance::InstanceLock::acquire(lock) {
        Ok(guard) => {
            drop(guard);
            false
        }
        Err(crate::daemon::instance::AcquireError::Held { .. }) => true,
        Err(crate::daemon::instance::AcquireError::Io(_)) => true,
    }
}

/// Shell ids with a socket in `dir`, sorted by index. Ignores anything whose
/// stem is not a valid shell id. The name grammar is the identity gate: a
/// production shell socket is a real AF_UNIX socket, for which
/// `FileType::is_file()` is false, so an entry that is neither a regular file
/// nor a socket (a directory, most obviously) is skipped.
pub fn scan_shell_ids(dir: &Path) -> Vec<ShellId> {
    use std::os::unix::fs::FileTypeExt;

    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut ids = Vec::new();
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() && !file_type.is_socket() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(stem) = name.strip_suffix(".sock") else {
            continue;
        };
        if let Ok(id) = ShellId::parse(stem) {
            ids.push(id);
        }
    }
    ids.sort_by_key(|id| id.index());
    ids
}

/// `(id, started_unix)` parsed from a cast filename like
/// `s3-1788699323.cast`, or `None` for anything else. The timestamp part is
/// parsed as bare digits so `s3-17-9.cast` and `s3-abc.cast` both fail.
pub fn parse_cast_name(name: &str) -> Option<(ShellId, u64)> {
    let body = name.strip_suffix(".cast")?;
    let (id_part, ts_part) = body.rsplit_once('-')?;
    let id = ShellId::parse(id_part).ok()?;
    let started_unix = ts_part.parse::<u64>().ok()?;
    Some((id, started_unix))
}

/// The most recent `started_unix` recorded for each shell id in `logs` (a cast
/// named `<id>-<started_unix>.cast`), so adoption does not invent a timestamp.
/// Empty when the directory is unreadable or has no matching casts.
fn recover_started_unix(logs: &Path) -> HashMap<String, u64> {
    let mut by_id = HashMap::new();
    if let Ok(entries) = std::fs::read_dir(logs) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some((id, ts)) = parse_cast_name(&name.to_string_lossy()) else {
                continue;
            };
            by_id
                .entry(id.as_str().to_string())
                .and_modify(|cur| {
                    if ts > *cur {
                        *cur = ts;
                    }
                })
                .or_insert(ts);
        }
    }
    by_id
}

/// Adopt every live shell in `dir` into `reg`; return the ids adopted.
///
/// The flock probe is the death decision, and it runs before the registry lock
/// is taken — the collected ids are inserted in a single `with_shells` call so
/// the probe is never serialized against the registry. Adoption bypasses
/// `check_owner_cap`: these shells already exist, and refusing to adopt one
/// would orphan a live process.
pub fn adopt_at_startup(reg: &ShellRegistry, dir: &Path, logs: &Path) -> Vec<ShellId> {
    let started = recover_started_unix(logs);
    let adopted: Vec<ShellId> = scan_shell_ids(dir)
        .into_iter()
        .filter(|id| host_is_alive(&dir.join(format!("{}.lock", id.as_str()))))
        .collect();
    // File I/O stays outside `with_shells`: collect `(id, sidecar)` pairs
    // first, then insert under the lock.
    let sidecars: Vec<(ShellId, SandboxSidecar)> = adopted
        .iter()
        .map(|id| (id.clone(), read_sandbox_sidecar(dir, id.as_str())))
        .collect();
    for (id, sidecar) in &sidecars {
        if matches!(sidecar, SandboxSidecar::Invalid) {
            log::warn!(
                "shell {id}: sandbox sidecar was unreadable or invalid, adopting as \
                 dedicated so it is never reused",
            );
        }
    }
    with_shells(reg, |store| {
        for (id, sidecar) in &sidecars {
            let started_unix = started.get(id.as_str()).copied().unwrap_or(0);
            let (sandbox, dedicated) = match sidecar {
                // No sidecar: a shell on the host itself.
                SandboxSidecar::Absent => (None, false),
                SandboxSidecar::Sandbox(name) => (Some(name.clone()), false),
                // A bad sidecar fails closed: the child is probably in a VM
                // but which one is unknown, so the shell must never be
                // handed to another command.
                SandboxSidecar::Invalid => (None, true),
            };
            store.insert(
                id.as_str().to_string(),
                ShellRecord {
                    id: id.clone(),
                    owner: Owner::Adopted,
                    host: crate::host::HostId::Local,
                    sandbox,
                    started_unix,
                    // An adopted shell's `dedicated` carries its sidecar's
                    // verdict: a bad sidecar fails closed (see above), so a
                    // probably-in-a-VM child is never reused for another
                    // command; a good one keeps the shell reusable, which is
                    // today's behaviour.
                    dedicated,
                    exited_unix: None,
                    last_foreground: None,
                    in_flight: None,
                },
            );
        }
    });
    adopted
}

/// What `<dir>/<id>.sandbox` says about a shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxSidecar {
    /// No sidecar: a shell on the host itself.
    Absent,
    /// The sandbox this shell's child runs in.
    Sandbox(String),
    /// A sidecar exists but is unreadable or does not name a daemon sandbox.
    Invalid,
}

/// The sidecar path recording which sandbox `<id>`'s shell child runs in.
pub fn sandbox_sidecar_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.sandbox"))
}

/// Write the sandbox sidecar for `id`: the sandbox name followed by a
/// newline. Does not create `dir`; the caller does.
pub fn write_sandbox_sidecar(dir: &Path, id: &str, sandbox: &str) -> std::io::Result<()> {
    std::fs::write(
        sandbox_sidecar_path(dir, id),
        format!("{sandbox}\n").into_bytes(),
    )
}

/// Read the sandbox sidecar for `id`. `NotFound` means no sidecar was ever
/// written (a plain shell host) and reads as `Absent`; any other read error
/// fails closed as `Invalid`, as does content that does not name a daemon
/// sandbox — the same gate the sandbox sweep deletes by, so a file an
/// operator wrote can never be adopted as a sandbox shell.
pub fn read_sandbox_sidecar(dir: &Path, id: &str) -> SandboxSidecar {
    match std::fs::read_to_string(sandbox_sidecar_path(dir, id)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => SandboxSidecar::Absent,
        Err(_) => SandboxSidecar::Invalid,
        Ok(content) => {
            let trimmed = content.trim();
            if crate::daemon::executor::sbx::is_daemon_sandbox(trimmed) {
                SandboxSidecar::Sandbox(trimmed.to_string())
            } else {
                SandboxSidecar::Invalid
            }
        }
    }
}

/// Remove the socket and lock of every DEAD host in `dir`; return the ids
/// swept. A live host's files are never removed — the same probe that
/// authorizes adoption *denies* the deletion here.
pub fn sweep_stale(dir: &Path) -> Vec<ShellId> {
    let mut swept = Vec::new();
    for id in scan_shell_ids(dir) {
        let lock = dir.join(format!("{}.lock", id.as_str()));
        if host_is_alive(&lock) {
            continue;
        }
        let _ = std::fs::remove_file(dir.join(format!("{}.sock", id.as_str())));
        let _ = std::fs::remove_file(&lock);
        // Most shells have no sidecar; ignore the error there.
        let _ = std::fs::remove_file(sandbox_sidecar_path(dir, id.as_str()));
        swept.push(id);
    }
    swept
}

/// Reap exited shells. For each record: probe its host; a live host clears any
/// exit stamp, a newly dead host is stamped with `now_unix`, and a host
/// stamped `retention_secs` or more ago has its record removed (so
/// `retention_secs == 0` removes a dead shell on the pass after it was
/// stamped). Returns the ids removed. The host's socket, lock and `.cast` log
/// have their own lifecycles (`sweep_stale`, the daemon's log sweep) and are
/// left alone here.
pub fn reap_exited(reg: &ShellRegistry, retention_secs: u64, now_unix: u64) -> Vec<ShellId> {
    let mut reaped = with_shells(reg, |store| {
        let mut reaped: Vec<ShellId> = Vec::new();
        for (key, record) in store.iter_mut() {
            let lock = super::launch::lock_path(key);
            if host_is_alive(&lock) {
                // The probe fails closed. A transient read failure must not
                // leave a stale stamp that reaps a live shell later.
                record.exited_unix = None;
                continue;
            }
            match record.exited_unix {
                None => record.exited_unix = Some(now_unix),
                Some(stamp) if now_unix.saturating_sub(stamp) >= retention_secs => {
                    reaped.push(record.id.clone());
                }
                Some(_) => {}
            }
        }
        for id in &reaped {
            store.remove(id.as_str());
        }
        reaped
    });
    reaped.sort_by_key(|id| id.index());
    if !reaped.is_empty() {
        let ids: Vec<&str> = reaped.iter().map(|id| id.as_str()).collect();
        log::info!(
            "shell registry: reaped {} exited shell(s): {ids:?}",
            reaped.len()
        );
    }
    reaped
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;

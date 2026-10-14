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
mod tests {
    #[test]
    fn may_drive_reaches_the_users_shells_from_chat_only() {
        let chat = Owner::Chat("a".into());
        assert!(chat.may_drive("chat:a"), "its own shell");
        assert!(chat.may_drive("user"), "the user's shell, through approval");
        assert!(!chat.may_drive("chat:b"), "another session's shell");
        assert!(
            !chat.may_drive("ghost:a"),
            "a ghost's shell, even with the same id"
        );
        let ghost = Owner::Ghost("g".into());
        assert!(ghost.may_drive("ghost:g"));
        assert!(
            !ghost.may_drive("user"),
            "a ghost never drives the user's shell"
        );
        assert!(!chat.may_drive("adopted"));
    }

    use super::*;
    use crate::config::Config;
    use crate::test_home_guard;

    fn record(index: u32, owner: Owner) -> ShellRecord {
        ShellRecord {
            id: ShellId::from_index(index),
            owner,
            host: crate::host::HostId::Local,
            sandbox: None,
            started_unix: 0,
            dedicated: false,
            exited_unix: None,
            last_foreground: None,
            in_flight: None,
        }
    }

    /// Point the probe / adoption / sweep helpers at a throwaway `dir` and
    /// `logs` pair (they are plain temp dirs, not the configured ones).
    fn fixture_dirs() -> (tempfile::TempDir, tempfile::TempDir) {
        (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap())
    }

    fn write(dir: &Path, name: &str) {
        std::fs::write(dir.join(name), "fixture").unwrap();
    }

    fn make_dir(dir: &Path, name: &str) {
        std::fs::create_dir(dir.join(name)).unwrap();
    }

    /// Bind a **real** AF_UNIX socket at `<dir>/<name>`. The returned listener
    /// must stay alive for the rest of the test: dropping it unlinks the
    /// socket. A real socket is the production entry type the `fs::write`
    /// fixtures never reproduced — the gap that hid bug-10-1.
    fn bind_socket(dir: &Path, name: &str) -> std::os::unix::net::UnixListener {
        std::os::unix::net::UnixListener::bind(dir.join(name)).unwrap()
    }

    #[test]
    fn registry_host_is_alive_reports_a_held_lock_as_alive() {
        let dir = tempfile::tempdir().unwrap();
        let lock = dir.path().join("s0.lock");
        let held = crate::daemon::instance::InstanceLock::acquire(&lock).unwrap();
        assert!(
            host_is_alive(&lock),
            "a held lock must read as alive (the flock is the liveness oracle)"
        );
        drop(held);
        assert!(
            !host_is_alive(&lock),
            "after the holder drops, the same path must read as dead"
        );
    }

    #[test]
    fn registry_host_is_alive_fails_closed_on_an_unreadable_lock() {
        let dir = tempfile::tempdir().unwrap();
        let lock = dir.path().join("s0.lock");
        make_dir(dir.path(), "s0.lock");
        assert!(
            host_is_alive(&lock),
            "a probe that cannot prove death must not report death"
        );
    }

    #[test]
    fn registry_owner_adopted_key_does_not_collide() {
        let adopted = Owner::Adopted.key();
        assert_ne!(
            adopted,
            Owner::Chat("adopted".into()).key(),
            "adopted must not collide with a Chat whose id is literally 'adopted'"
        );
        assert_ne!(
            adopted,
            Owner::Ghost("adopted".into()).key(),
            "adopted must not collide with a Ghost whose id is literally 'adopted'"
        );
        assert_eq!(adopted, "adopted");
    }

    #[test]
    fn registry_scan_shell_ids_ignores_non_shell_entries() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "s0.sock",
            "s2.sock",
            "evil.sock",
            "s1.sock.bak",
            "s1.lock",
            "S1.sock",
            "s1234567890.sock",
        ] {
            write(dir.path(), name);
        }
        make_dir(dir.path(), "s3.sock");

        let scanned = scan_shell_ids(dir.path());
        let ids: Vec<&str> = scanned.iter().map(|id| id.as_str()).collect();
        assert_eq!(ids, vec!["s0", "s2"]);
    }

    #[test]
    fn scan_shell_ids_finds_a_real_socket() {
        let dir = tempfile::tempdir().unwrap();
        let _listener = bind_socket(dir.path(), "s0.sock");

        let scanned = scan_shell_ids(dir.path());
        let ids: Vec<&str> = scanned.iter().map(|id| id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["s0"],
            "a real AF_UNIX socket named s0.sock must be found — bug-10-1"
        );
    }

    #[test]
    fn scan_shell_ids_still_skips_a_directory_named_like_a_socket() {
        let dir = tempfile::tempdir().unwrap();
        make_dir(dir.path(), "s0.sock");

        let scanned = scan_shell_ids(dir.path());
        assert!(
            scanned.is_empty(),
            "a directory named s0.sock is not a shell socket and must be skipped"
        );
    }

    #[test]
    fn registry_parse_cast_name_accepts_and_rejects() {
        let parsed = parse_cast_name("s3-1788699323.cast");
        let (id, ts) = parsed.expect("s3-1788699323.cast must parse");
        assert_eq!(id.as_str(), "s3");
        assert_eq!(ts, 1788699323);

        for bad in [
            "s3.cast",
            "s3-abc.cast",
            "s3-.cast",
            "-17.cast",
            "notashell-17.cast",
            "s3-17.txt",
            "s3-17-9.cast",
        ] {
            assert!(parse_cast_name(bad).is_none(), "must reject {bad:?}");
        }
    }

    #[test]
    fn registry_adopt_inserts_live_hosts_with_owner_adopted() {
        let (dir, logs) = fixture_dirs();
        write(dir.path(), "s0.sock");
        write(logs.path(), "s0-1788699323.cast");
        let _held =
            crate::daemon::instance::InstanceLock::acquire(&dir.path().join("s0.lock")).unwrap();

        let reg = ShellRegistry::default();
        let adopted = adopt_at_startup(&reg, dir.path(), logs.path());
        assert_eq!(adopted.len(), 1);
        assert_eq!(adopted[0].as_str(), "s0");

        let guard = reg.try_lock().unwrap();
        let record = guard.get("s0").expect("s0 must be adopted");
        assert!(matches!(record.owner, Owner::Adopted));
        assert_eq!(record.started_unix, 1788699323);
    }

    /// The production file type, end to end at the registry layer: a real
    /// bound socket plus a held lock must adopt through the same path the
    /// daemon runs at startup. The existing adoption tests use `fs::write`
    /// stand-ins for the socket, which is why 24 tests were green while
    /// production adopted nothing (bug-10-1).
    #[test]
    fn adopt_at_startup_adopts_a_real_socket_host() {
        let (dir, logs) = fixture_dirs();
        let _listener = bind_socket(dir.path(), "s0.sock");
        write(logs.path(), "s0-1788699323.cast");
        let _held =
            crate::daemon::instance::InstanceLock::acquire(&dir.path().join("s0.lock")).unwrap();

        let reg = ShellRegistry::default();
        let adopted = adopt_at_startup(&reg, dir.path(), logs.path());
        let ids: Vec<&str> = adopted.iter().map(|id| id.as_str()).collect();
        assert_eq!(ids, vec!["s0"]);

        let guard = reg.try_lock().unwrap();
        let record = guard.get("s0").expect("s0 must be adopted");
        assert!(matches!(record.owner, Owner::Adopted));
        assert_eq!(record.started_unix, 1788699323);
    }

    #[test]
    fn registry_adopt_uses_zero_when_no_cast_file_matches() {
        let (dir, logs) = fixture_dirs();
        write(dir.path(), "s0.sock");
        let _held =
            crate::daemon::instance::InstanceLock::acquire(&dir.path().join("s0.lock")).unwrap();

        let reg = ShellRegistry::default();
        adopt_at_startup(&reg, dir.path(), logs.path());

        let guard = reg.try_lock().unwrap();
        let record = guard.get("s0").expect("s0 must be adopted");
        assert_eq!(record.started_unix, 0, "no matching cast must pin to 0");
    }

    #[test]
    fn registry_adopt_skips_dead_hosts() {
        let (dir, logs) = fixture_dirs();
        write(dir.path(), "s0.sock");

        let reg = ShellRegistry::default();
        let adopted = adopt_at_startup(&reg, dir.path(), logs.path());
        assert!(adopted.is_empty(), "a dead host must not be adopted");

        let guard = reg.try_lock().unwrap();
        assert!(
            guard.is_empty(),
            "no record may be inserted for a dead host"
        );
    }

    #[test]
    fn registry_sweep_never_touches_a_live_host() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "s0.sock");
        write(dir.path(), "s1.sock");
        let _held_live =
            crate::daemon::instance::InstanceLock::acquire(&dir.path().join("s0.lock")).unwrap();

        let swept = sweep_stale(dir.path());
        assert_eq!(swept.len(), 1);
        assert_eq!(swept[0].as_str(), "s1");

        assert!(
            !dir.path().join("s1.sock").exists() && !dir.path().join("s1.lock").exists(),
            "the dead host's socket and lock must both be removed"
        );
        assert!(
            dir.path().join("s0.sock").exists() && dir.path().join("s0.lock").exists(),
            "the live host's files must both survive"
        );
    }

    #[test]
    fn sandbox_sidecar_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        write_sandbox_sidecar(dir.path(), "s3", "de-ghost-r1").unwrap();

        assert_eq!(
            std::fs::read_to_string(dir.path().join("s3.sandbox")).unwrap(),
            "de-ghost-r1\n",
            "the file content must be exactly the name plus a newline"
        );
        assert_eq!(
            read_sandbox_sidecar(dir.path(), "s3"),
            SandboxSidecar::Sandbox("de-ghost-r1".into()),
            "the written name must read back"
        );
    }

    #[test]
    fn sandbox_sidecar_absent_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            read_sandbox_sidecar(dir.path(), "s0"),
            SandboxSidecar::Absent,
            "no sidecar file must read as Absent"
        );
    }

    #[test]
    fn sandbox_sidecar_rejects_foreign_or_empty_names() {
        for content in ["", "   \n", "de-probe-a\n", "claude-x\n"] {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("s0.sandbox"), content).unwrap();
            assert_eq!(
                read_sandbox_sidecar(dir.path(), "s0"),
                SandboxSidecar::Invalid,
                "`{content:?}` must not read as a daemon sandbox"
            );
        }
    }

    #[test]
    fn adopt_at_startup_recovers_the_sandbox_from_its_sidecar() {
        let (dir, logs) = fixture_dirs();
        write(dir.path(), "s0.sock");
        write_sandbox_sidecar(dir.path(), "s0", "de-ghost-r1").unwrap();
        let _held =
            crate::daemon::instance::InstanceLock::acquire(&dir.path().join("s0.lock")).unwrap();

        let reg = ShellRegistry::default();
        adopt_at_startup(&reg, dir.path(), logs.path());

        let guard = reg.try_lock().unwrap();
        let record = guard.get("s0").expect("s0 must be adopted");
        assert_eq!(
            record.sandbox,
            Some("de-ghost-r1".to_string()),
            "the sidecar's sandbox must be recovered"
        );
        assert!(!record.dedicated, "a known sandbox shell stays reusable");
    }

    #[test]
    fn adopt_at_startup_without_a_sidecar_has_no_sandbox() {
        let (dir, logs) = fixture_dirs();
        write(dir.path(), "s0.sock");
        let _held =
            crate::daemon::instance::InstanceLock::acquire(&dir.path().join("s0.lock")).unwrap();

        let reg = ShellRegistry::default();
        adopt_at_startup(&reg, dir.path(), logs.path());

        let guard = reg.try_lock().unwrap();
        let record = guard.get("s0").expect("s0 must be adopted");
        assert_eq!(record.sandbox, None, "no sidecar means no sandbox");
        assert!(!record.dedicated, "today's behaviour: still reusable");
    }

    #[test]
    fn adopt_at_startup_marks_a_bad_sidecar_dedicated() {
        let (dir, logs) = fixture_dirs();
        write(dir.path(), "s0.sock");
        std::fs::write(dir.path().join("s0.sandbox"), "claude-x\n").unwrap();
        let _held =
            crate::daemon::instance::InstanceLock::acquire(&dir.path().join("s0.lock")).unwrap();

        let reg = ShellRegistry::default();
        adopt_at_startup(&reg, dir.path(), logs.path());

        let guard = reg.try_lock().unwrap();
        let record = guard.get("s0").expect("s0 must be adopted");
        assert_eq!(
            record.sandbox, None,
            "an invalid sidecar must not name a sandbox"
        );
        assert!(
            record.dedicated,
            "an invalid sidecar fails closed: the shell must never be reused"
        );
    }

    #[test]
    fn sweep_stale_removes_a_dead_hosts_sandbox_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "s0.sock");
        write(dir.path(), "s1.sock");
        write_sandbox_sidecar(dir.path(), "s0", "de-ghost-r1").unwrap();
        write_sandbox_sidecar(dir.path(), "s1", "de-ghost-r2").unwrap();
        let _held_live =
            crate::daemon::instance::InstanceLock::acquire(&dir.path().join("s0.lock")).unwrap();

        let swept = sweep_stale(dir.path());
        assert_eq!(swept.len(), 1);
        assert_eq!(swept[0].as_str(), "s1");

        assert!(
            !dir.path().join("s1.sandbox").exists(),
            "the dead host's sidecar must be removed"
        );
        assert!(
            dir.path().join("s0.sandbox").exists(),
            "the live host's sidecar must survive"
        );
    }

    #[test]
    fn registry_allocate_index_fills_holes() {
        assert_eq!(allocate_index(&HashSet::new()), 0);
        assert_eq!(allocate_index(&[0, 1, 3].into_iter().collect()), 2);
        assert_eq!(allocate_index(&[0, 1, 2].into_iter().collect()), 3);
    }

    #[test]
    fn registry_shell_id_parse_delegates_to_the_shared_validator() {
        for id in ["s0", "s7", "s123456789"] {
            assert!(ShellId::parse(id).is_ok(), "must accept {id}");
        }
        for id in [
            "",
            ".",
            "..",
            "../etc/passwd",
            "s1/x",
            "s",
            "s-1",
            "S1",
            "shell",
            "s1.sock",
            "s1234567890",
        ] {
            assert!(ShellId::parse(id).is_err(), "must reject {id:?}");
        }
    }

    #[test]
    fn registry_shell_id_round_trips_index_and_string() {
        let id = ShellId::from_index(42);
        assert_eq!(id.as_str(), "s42");
        assert_eq!(id.index(), 42);
        assert_eq!(id.to_string(), "s42");
    }

    #[test]
    fn registry_owner_keys_do_not_collide_across_kinds() {
        let chat = Owner::Chat("x".into()).key();
        let ghost = Owner::Ghost("x".into()).key();
        assert_ne!(
            chat, ghost,
            "a chat session and a ghost sharing an id string are different owners"
        );
    }

    #[test]
    fn registry_check_owner_cap_counts_only_that_owner() {
        let mut existing = HashMap::new();
        existing.insert("s0".into(), record(0, Owner::Chat("a".into())));
        existing.insert("s1".into(), record(1, Owner::Chat("a".into())));
        existing.insert("s2".into(), record(2, Owner::Chat("b".into())));

        assert!(
            check_owner_cap(&existing, &Owner::Chat("a".into()), 2).is_err(),
            "owner a holds two shells and the cap is two; a third must be refused"
        );
        assert!(
            check_owner_cap(&existing, &Owner::Chat("b".into()), 2).is_ok(),
            "owner b holds one shell; one more must be allowed"
        );
    }

    #[test]
    fn registry_check_owner_cap_zero_means_unlimited() {
        let mut existing = HashMap::new();
        for i in 0..5 {
            existing.insert(format!("s{i}"), record(i, Owner::Chat("a".into())));
        }
        assert!(
            check_owner_cap(&existing, &Owner::Chat("a".into()), 0).is_ok(),
            "cap 0 (unlimited) must allow a further shell"
        );
    }

    /// Point `HOME` at a fresh temp dir so `reap_exited`'s probes resolve
    /// `<id>.lock` under that dir, serialised against every other
    /// HOME-touching test. The `TempDir` must stay bound for the whole test —
    /// it deletes the tree when dropped — so return both.
    fn reaper_home() -> (crate::TestHomeGuard, tempfile::TempDir) {
        let guard = test_home_guard();
        let dir = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("HOME", dir.path());
        }
        Config::ensure_dirs().expect("reaper test HOME setup must create the dirs");
        (guard, dir)
    }

    #[test]
    fn registry_reap_removes_a_dead_shell_past_its_retention() {
        let (_home, _home_dir) = reaper_home();
        // No lock file at all → the probe reads the host dead.
        let reg = ShellRegistry::default();
        let dead = record(0, Owner::Chat("a".into()));
        let dead_id = dead.id.clone();
        with_shells(&reg, |store| {
            store.insert(dead_id.as_str().to_string(), dead);
        });

        // First pass: the death is *observed* — stamped, not removed.
        let reaped = reap_exited(&reg, 300, 1_000);
        assert!(
            reaped.is_empty(),
            "the first sighting must not reap; retention starts here"
        );
        let exited = with_shells(&reg, |store| {
            store.get(dead_id.as_str()).map(|r| r.exited_unix)
        });
        assert_eq!(
            exited,
            Some(Some(1_000)),
            "the first sight of a dead host must stamp the record with now_unix"
        );

        // Second pass inside the retention: still there.
        let reaped = reap_exited(&reg, 300, 1_000 + 299);
        assert!(
            reaped.is_empty(),
            "299 s into a 300 s retention is too early"
        );
        let still_listed = with_shells(&reg, |store| store.contains_key(dead_id.as_str()));
        assert!(still_listed, "the record must outlive its retention");

        // Third pass at the retention boundary: reaped.
        let reaped = reap_exited(&reg, 300, 1_000 + 300);
        assert_eq!(reaped.len(), 1, "exactly the retention boundary reaps");
        assert_eq!(reaped[0].as_str(), "s0");
        let gone = with_shells(&reg, |store| store.contains_key(dead_id.as_str()));
        assert!(!gone, "the reaped record must be removed from the registry");
    }

    #[test]
    fn registry_reap_keeps_a_live_shell_forever() {
        let (_home, _home_dir) = reaper_home();
        let lock_path = crate::config::shells_dir().join("s2.lock");
        let _held = crate::daemon::instance::InstanceLock::acquire(&lock_path).unwrap();
        let reg = ShellRegistry::default();
        let live = record(2, Owner::Chat("a".into()));
        let live_id = live.id.clone();
        with_shells(&reg, |store| {
            store.insert(live_id.as_str().to_string(), live);
        });

        for now in [1_000u64, 1_000 + 300, 1_000 + 100_000] {
            let reaped = reap_exited(&reg, 300, now);
            assert!(reaped.is_empty(), "a live shell must never be reaped");
        }

        let (shell_listed, stamped) = with_shells(&reg, |store| {
            let stamped = store.get(live_id.as_str()).map(|r| r.exited_unix);
            (store.contains_key(live_id.as_str()), stamped)
        });
        assert!(shell_listed, "the live shell's record must survive");
        assert_eq!(
            stamped,
            Some(None),
            "a live shell must never be stamped exited"
        );
    }

    #[test]
    fn registry_reap_clears_the_stamp_when_a_host_reads_alive_again() {
        let (_home, _home_dir) = reaper_home();
        let lock_path = crate::config::shells_dir().join("s1.lock");
        let reg = ShellRegistry::default();
        let flappy = record(1, Owner::Ghost("g".into()));
        let flappy_id = flappy.id.clone();
        with_shells(&reg, |store| {
            store.insert(flappy_id.as_str().to_string(), flappy);
        });

        // Pass 1: dead (nothing holds the lock), so the record is stamped.
        let reaped = reap_exited(&reg, 300, 1_000);
        assert!(reaped.is_empty());
        let stamped = with_shells(&reg, |store| {
            store.get(flappy_id.as_str()).map(|r| r.exited_unix)
        });
        assert_eq!(stamped, Some(Some(1_000)));

        // Pass 2: the lock is now held — a probe failure reads *alive*. The
        // stale stamp must be cleared, not trusted, or a later pass would
        // reap a live shell.
        let recovered = crate::daemon::instance::InstanceLock::acquire(&lock_path).unwrap();
        let reaped = reap_exited(&reg, 300, 1_000 + 400);
        assert!(reaped.is_empty());
        let unstamped = with_shells(&reg, |store| {
            store.get(flappy_id.as_str()).map(|r| r.exited_unix)
        });
        assert_eq!(
            unstamped,
            Some(None),
            "a host that reads alive again must have its exit stamp cleared"
        );

        // Pass 3: far past where the stale stamp would have reaped — and the
        // shell survives, because the fresh probe keeps saying alive.
        let reaped = reap_exited(&reg, 300, 1_000 + 400 + 100_000);
        assert!(
            reaped.is_empty(),
            "the cleared stamp must protect the shell"
        );
        let still_listed = with_shells(&reg, |store| store.contains_key(flappy_id.as_str()));
        assert!(still_listed);
        drop(recovered);
    }

    #[test]
    fn registry_reap_with_zero_retention_removes_on_the_pass_after_stamping() {
        let (_home, _home_dir) = reaper_home();
        let reg = ShellRegistry::default();
        let dead = record(0, Owner::Chat("a".into()));
        let dead_id = dead.id.clone();
        with_shells(&reg, |store| {
            store.insert(dead_id.as_str().to_string(), dead);
        });

        // Pass 1: stamped, retained — even with retention 0 the death has to
        // be *seen* before anything is removed, so a caller that acquires a
        // lock between two passes cannot lose a shell it never saw die.
        let reaped = reap_exited(&reg, 0, 5_000);
        assert!(reaped.is_empty(), "the stamping pass must not also reap");

        // Pass 2: the boundary the doc comment names.
        let reaped = reap_exited(&reg, 0, 5_000);
        assert_eq!(
            reaped.len(),
            1,
            "retention 0 removes a dead shell on the pass right after the stamp"
        );
        assert_eq!(reaped[0].as_str(), "s0");
        let gone = with_shells(&reg, |store| store.contains_key(dead_id.as_str()));
        assert!(!gone);
    }

    #[test]
    fn registry_reap_reports_removed_ids_in_id_order() {
        let (_home, _home_dir) = reaper_home();
        let reg = ShellRegistry::default();
        for index in [3u32, 1u32, 2u32] {
            let r = record(index, Owner::Chat("a".into()));
            let id = r.id.clone();
            with_shells(&reg, |store| {
                store.insert(id.as_str().to_string(), r);
            });
        }
        let _stamp_pass = reap_exited(&reg, 0, 100);
        let reaped = reap_exited(&reg, 0, 100);
        let ids: Vec<&str> = reaped.iter().map(|id| id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["s1", "s2", "s3"],
            "the removed ids must come back in id order regardless of map order"
        );
    }

    #[tokio::test]
    async fn registry_spawn_for_does_not_hold_the_lock_across_the_spawn() {
        let reg = ShellRegistry::default();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let reg_in_task = reg.clone();

        let task = tokio::spawn(async move {
            spawn_for(
                &reg_in_task,
                Owner::Chat("a".into()),
                5,
                false,
                move |_id| async move {
                    let _ = started_tx.send(());
                    let _ = release_rx.await;
                    Ok(())
                },
            )
            .await
        });

        started_rx.await.expect("the stub spawn must be running");

        {
            let guard = reg
                .try_lock()
                .expect("the registry lock must be free while the spawn is in flight");
            assert!(
                guard.contains_key("s0"),
                "the slot must be reserved before the spawn runs"
            );
        }

        let _ = release_tx.send(());
        let id = task
            .await
            .expect("spawn_for task must complete")
            .expect("the stub spawn must succeed");
        assert_eq!(id.as_str(), "s0");
    }

    /// The named-host form stamps the record with the host it was given, so
    /// every reader of `record.host` (`marker_shell_name`, the `[SHELLS]`
    /// block) can tell a remote shell from a local one.
    #[tokio::test]
    async fn spawn_for_on_host_records_the_named_host() {
        let reg = ShellRegistry::default();
        let id = spawn_for_on_host(
            &reg,
            Owner::Chat("a".into()),
            5,
            crate::host::HostId::Named("pinky".to_string()),
            false,
            |_id| async { Ok(()) },
        )
        .await
        .expect("the stub spawn must succeed");

        let guard = reg.try_lock().unwrap();
        let saved = guard.get(id.as_str()).expect("the record is registered");
        assert_eq!(
            saved.host,
            crate::host::HostId::Named("pinky".to_string()),
            "the record must carry the host the spawn named"
        );
    }

    /// Task 2: the sandbox form stamps the record with the sandbox its child
    /// runs in, while the shell process itself stays on the local host.
    #[tokio::test]
    async fn spawn_for_in_sandbox_records_the_sandbox() {
        let reg = ShellRegistry::default();
        let id = spawn_for_in_sandbox(
            &reg,
            Owner::Chat("a".into()),
            5,
            "de-ghost-r1".into(),
            false,
            |_id| async { Ok(()) },
        )
        .await
        .expect("the stub spawn must succeed");

        let guard = reg.try_lock().unwrap();
        let saved = guard.get(id.as_str()).expect("the record is registered");
        assert_eq!(
            saved.sandbox,
            Some("de-ghost-r1".to_string()),
            "the record must carry the sandbox the spawn named"
        );
        assert_eq!(
            saved.host,
            crate::host::HostId::Local,
            "the shell process itself is local; only its commands run in the VM"
        );
    }

    /// The named-host form must keep leaving `sandbox` empty: a sandbox name
    /// arrives only from the sandbox spawn.
    #[tokio::test]
    async fn spawn_for_on_host_records_no_sandbox() {
        let reg = ShellRegistry::default();
        let id = spawn_for_on_host(
            &reg,
            Owner::Chat("a".into()),
            5,
            crate::host::HostId::Named("pinky".to_string()),
            false,
            |_id| async { Ok(()) },
        )
        .await
        .expect("the stub spawn must succeed");

        let guard = reg.try_lock().unwrap();
        let saved = guard.get(id.as_str()).expect("the record is registered");
        assert_eq!(saved.sandbox, None, "a host spawn records no sandbox");
    }

    /// The one-line delegation is what every existing caller still goes
    /// through, so it must keep stamping `Local` exactly as before.
    #[tokio::test]
    async fn spawn_for_still_records_the_local_host() {
        let reg = ShellRegistry::default();
        let id = spawn_for(&reg, Owner::Chat("a".into()), 5, false, |_id| async {
            Ok(())
        })
        .await
        .expect("the stub spawn must succeed");

        let guard = reg.try_lock().unwrap();
        let saved = guard.get(id.as_str()).expect("the record is registered");
        assert_eq!(
            saved.host,
            crate::host::HostId::Local,
            "the delegating form must leave the local stamp unchanged"
        );
    }

    #[tokio::test]
    async fn registry_spawn_for_releases_the_slot_when_the_spawn_fails() {
        let reg = ShellRegistry::default();
        let result = spawn_for(&reg, Owner::Chat("a".into()), 5, false, |_id| async {
            Err(anyhow::anyhow!("boom"))
        })
        .await;
        assert!(result.is_err(), "a failed spawn must propagate its error");

        let guard = reg.try_lock().unwrap();
        assert!(
            guard.is_empty(),
            "no record may remain after a failed spawn"
        );
        let occupied: HashSet<u32> = guard.values().map(|record| record.id.index()).collect();
        assert_eq!(
            allocate_index(&occupied),
            0,
            "the released slot must be reallocatable"
        );
    }

    #[tokio::test]
    async fn registry_spawn_for_sets_dedicated_from_its_argument() {
        let foreground = ShellRegistry::default();
        assert!(
            spawn_for(
                &foreground,
                Owner::Chat("a".into()),
                5,
                false,
                |_id| async { Ok(()) }
            )
            .await
            .is_ok(),
            "the foreground spawn must succeed"
        );
        let background = ShellRegistry::default();
        assert!(
            spawn_for(&background, Owner::Chat("a".into()), 5, true, |_id| async {
                Ok(())
            })
            .await
            .is_ok(),
            "the dedicated spawn must succeed"
        );
        let fg_flag = with_shells(&foreground, |store| {
            store.get("s0").map(|record| record.dedicated)
        });
        let bg_flag = with_shells(&background, |store| {
            store.get("s0").map(|record| record.dedicated)
        });
        assert_eq!(
            fg_flag,
            Some(false),
            "spawn_for's caller decides; a foreground shell must not carry the flag"
        );
        assert_eq!(
            bg_flag,
            Some(true),
            "the pair is the behaviour: the same one constructor site must yield both"
        );
    }

    #[tokio::test]
    async fn a_spawned_shell_records_the_local_host() {
        let reg = ShellRegistry::default();
        let id = spawn_for(&reg, Owner::Chat("a".into()), 5, false, |_id| async {
            Ok(())
        })
        .await
        .expect("the stub spawn must succeed");
        let host = with_shells(&reg, |store| {
            store.get(id.as_str()).map(|record| record.host.clone())
        });
        assert_eq!(
            host,
            Some(crate::host::HostId::Local),
            "the record the normal path creates must carry the implicit local host"
        );
    }
}

//! The in-memory shell registry: id grammar, ownership, per-owner caps, and
//! slot allocation with spawn-outside-the-lock, plus startup adoption of the
//! shells surviving a daemon restart, the sweep of dead hosts' leftovers, and
//! the record reaper for exited shells.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex};

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
    /// A shell adopted at daemon startup from a live host left behind by a
    /// previous daemon. Per the measured facts in phase-06b, nothing on disk
    /// records the original owner, and the owning session is gone by
    /// definition — so an adopted shell is owned by nobody until a later phase
    /// re-claims it.
    Adopted,
}

impl Owner {
    /// The cap-grouping key. The kind prefix makes a chat session and a ghost
    /// job that share an id string distinct owners.
    pub fn key(&self) -> String {
        match self {
            Owner::Chat(id) => format!("chat:{id}"),
            Owner::Ghost(id) => format!("ghost:{id}"),
            Owner::Adopted => "adopted".to_string(),
        }
    }
}

/// One entry in the registry, keyed in the underlying map by the id string.
pub struct ShellRecord {
    pub id: ShellId,
    pub owner: Owner,
    pub started_unix: u64,
    /// Set when an interactive command (ssh, mosh, …) took this shell's
    /// terminal. Such a shell still passes the liveness probe — its `bash` is
    /// alive — but every byte written to it is consumed by the interactive
    /// process, so it must never be handed to another command (08b F2).
    pub interactive: bool,
    /// Unix seconds when this shell's host was **first observed** dead, set by
    /// the reaper. `None` while the host is alive. The record is kept for
    /// `[shells] exited_retention_secs` after this, so an operator can still
    /// see that the shell existed.
    pub exited_unix: Option<u64>,
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

/// Mark `id`'s shell captured by an interactive session. Idempotent; a no-op
/// when the id is not in the registry.
pub fn mark_interactive(reg: &ShellRegistry, id: &ShellId) {
    with_shells(reg, |store| {
        if let Some(record) = store.get_mut(id.as_str()) {
            record.interactive = true;
        }
    });
}

/// Lowest unused index, starting at 0. Fills holes: `{0, 1, 3}` yields `2`.
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
                started_unix,
                interactive: false,
                exited_unix: None,
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
/// stem is not a valid shell id, and anything that is not a `.sock` file.
pub fn scan_shell_ids(dir: &Path) -> Vec<ShellId> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut ids = Vec::new();
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
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
    with_shells(reg, |store| {
        for id in &adopted {
            let started_unix = started.get(id.as_str()).copied().unwrap_or(0);
            store.insert(
                id.as_str().to_string(),
                ShellRecord {
                    id: id.clone(),
                    owner: Owner::Adopted,
                    started_unix,
                    interactive: false,
                    exited_unix: None,
                },
            );
        }
    });
    adopted
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
    use super::*;
    use crate::config::Config;
    use crate::test_home_guard;

    fn record(index: u32, owner: Owner) -> ShellRecord {
        ShellRecord {
            id: ShellId::from_index(index),
            owner,
            started_unix: 0,
            interactive: false,
            exited_unix: None,
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

    #[tokio::test]
    async fn registry_spawn_for_releases_the_slot_when_the_spawn_fails() {
        let reg = ShellRegistry::default();
        let result = spawn_for(&reg, Owner::Chat("a".into()), 5, |_id| async {
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

    /// 08b Task 1's boundary cases: the setter is idempotent, and an id that
    /// is not in the registry is a no-op rather than an insert.
    #[test]
    fn registry_mark_interactive_is_idempotent_and_ignores_unknown_ids() {
        let reg = ShellRegistry::default();
        let record = record(0, Owner::Chat("a".into()));
        let id = record.id.clone();
        with_shells(&reg, |store| {
            store.insert(id.as_str().to_string(), record);
        });

        mark_interactive(&reg, &id);
        mark_interactive(&reg, &id);

        let guard = reg.try_lock().unwrap();
        assert!(
            guard[id.as_str()].interactive,
            "marking twice must leave the flag set"
        );
        assert_eq!(
            guard.len(),
            1,
            "marking must never insert; the second call must not have added a record"
        );
    }

    #[test]
    fn registry_mark_interactive_leaves_an_unknown_id_absent() {
        let reg = ShellRegistry::default();
        let stranger = ShellId::from_index(9);
        mark_interactive(&reg, &stranger);
        let guard = reg.try_lock().unwrap();
        assert!(
            guard.is_empty(),
            "marking an id that is not in the registry must be a no-op"
        );
        assert!(
            !guard.contains_key(stranger.as_str()),
            "a no-op must not materialise the stranger"
        );
    }
}

//! The in-memory shell registry: id grammar, ownership, per-owner caps, and
//! slot allocation with spawn-outside-the-lock, plus startup adoption of the
//! shells surviving a daemon restart and the sweep of dead hosts' leftovers.
//! Exited-shell GC is phase-08.

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

#[cfg(test)]
mod tests {
    use super::*;

    fn record(index: u32, owner: Owner) -> ShellRecord {
        ShellRecord {
            id: ShellId::from_index(index),
            owner,
            started_unix: 0,
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
}

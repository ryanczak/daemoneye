//! The in-memory shell registry: id grammar, ownership, per-owner caps, and
//! slot allocation with spawn-outside-the-lock. Startup adoption, the stale
//! socket/lock sweep and exited-shell GC are phase-06b; this module is the
//! registry's shape only.

use std::collections::{HashMap, HashSet};
use std::fmt;
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
}

impl Owner {
    /// The cap-grouping key. The kind prefix makes a chat session and a ghost
    /// job that share an id string distinct owners.
    pub fn key(&self) -> String {
        match self {
            Owner::Chat(id) => format!("chat:{id}"),
            Owner::Ghost(id) => format!("ghost:{id}"),
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

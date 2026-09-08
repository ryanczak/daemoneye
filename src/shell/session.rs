//! Getting a ready shell socket for an owner: reuse the owner's live shell or
//! create one, wait for it to answer readiness, and keep the registry honest
//! when a host never comes up.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Context;

use crate::shell::launch;
use crate::shell::registry::{
    Owner, ShellId, ShellRegistry, host_is_alive, spawn_for, with_shells,
};

/// A ready shell socket for `owner`: reuse the owner's live shell, or create
/// one. Returns the shell id and the path to its bound, ready socket.
pub async fn shell_for_owner<F, Fut>(
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
    // (1) Lowest-indexed existing record whose host is still alive, so the
    // answer is deterministic when an owner holds several shells; a match
    // whose host is dead is removed here so it is not handed to the next
    // caller as a live shell, then we fall through to create.
    let owner_key = owner.key();
    let reuse: Option<ShellId> = with_shells(reg, |store| {
        let mut matches: Vec<ShellId> = store
            .values()
            .filter(|record| record.owner.key() == owner_key)
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

    // (3) Otherwise create one, then wait for it to become ready.
    let id = spawn_for(reg, owner, max_per_owner, spawn).await?;
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

/// The shell a host is spawned with, and the same name `client::run_command`
/// must be told so it reads the right exit variable (`$?` vs fish's
/// `$status`). One function so the two can never disagree.
pub fn login_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "bash".to_string())
}

/// The `spawn` argument `shell_for_owner` takes in production: a detached
/// `daemoneye shell-host` for `id`. Separate from `shell_for_owner` because
/// `spawn_detached` resolves `current_exe()` and so cannot run under a test
/// (F4).
pub async fn spawn_detached_host(id: ShellId, rows: u16, cols: u16) -> anyhow::Result<()> {
    let shell = login_shell();
    launch::spawn_detached(id.as_str(), &shell, rows, cols).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::shell::PtyBackend;
    use crate::shell::client;
    use crate::shell::host;
    use crate::shell::registry::ShellRecord;
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
        let backend = PtyBackend::spawn("bash", 24, 80, 100, &cast, 0).context("spawning bash")?;
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

        let (first_id, _) =
            shell_for_owner(&reg, owner.clone(), 5, Duration::from_secs(15), host_spawn)
                .await
                .expect("the first request creates a shell");

        let (second_id, _) = shell_for_owner(
            &reg,
            owner.clone(),
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
            5,
            Duration::from_secs(15),
            spawn_in_process_host,
        )
        .await
        .expect("owner a gets a shell");
        let (idb, _) = shell_for_owner(
            &reg,
            Owner::Chat("b".into()),
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
                    started_unix: 0,
                },
            );
        });

        let (id, socket) = shell_for_owner(
            &reg,
            owner.clone(),
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
                    started_unix: 0,
                },
            );
            store.insert(
                "s1".to_string(),
                ShellRecord {
                    id: ShellId::from_index(1),
                    owner: owner.clone(),
                    started_unix: 0,
                },
            );
        });
        let lock = launch::lock_path("s1");
        let _held = crate::daemon::instance::InstanceLock::acquire(&lock).expect("hold s1");

        let result = shell_for_owner(&reg, owner, 1, Duration::from_secs(15), |_id| async {
            Ok(())
        })
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
        let result = shell_for_owner(&reg, owner, 5, Duration::from_millis(50), |_id| async {
            Ok(())
        })
        .await;

        assert!(result.is_err(), "a host that never becomes ready must Err");
        let guard = reg.try_lock().unwrap();
        assert!(
            guard.is_empty(),
            "the failed shell's record must be rolled back"
        );
    }
}

//! The dead-host sweeps beside the registry: `sweep_stale` runs once at
//! startup; `sweep_orphaned` runs on the maintenance tick, and
//! `reap_and_sweep` is the tick's reaper + sweeper in one call.

use super::registry::{
    ShellId, ShellRegistry, host_is_alive, owner_sidecar_path, reap_exited, sandbox_sidecar_path,
    scan_shell_ids, with_shells,
};
use std::path::Path;

/// Remove the socket and lock of every DEAD host in `dir`; return the ids
/// swept. A live host's files are never removed — the same probe that
/// authorizes adoption *denies* the deletion here.
pub fn sweep_stale(dir: &Path) -> Vec<ShellId> {
    let mut swept = Vec::new();
    for id in scan_shell_ids(dir) {
        if host_is_alive(&dir.join(format!("{}.lock", id.as_str()))) {
            continue;
        }
        sweep_files(dir, &id);
        swept.push(id);
    }
    swept
}

/// The file half of both sweeps: remove `<id>.sock`, `<id>.lock` and the
/// two sidecars. The caller has already decided the host is dead.
fn sweep_files(dir: &Path, id: &ShellId) {
    let _ = std::fs::remove_file(dir.join(format!("{}.sock", id.as_str())));
    let _ = std::fs::remove_file(dir.join(format!("{}.lock", id.as_str())));
    // Most shells have no sidecar; ignore the error there.
    let _ = std::fs::remove_file(sandbox_sidecar_path(dir, id.as_str()));
    let _ = std::fs::remove_file(owner_sidecar_path(dir, id.as_str()));
}

/// Sweep the files of dead hosts the registry no longer holds: for every id
/// `scan_shell_ids` finds in `dir` that has **no record** in `reg` and whose
/// host is dead, remove the same files `sweep_stale` removes, and return the
/// ids swept. Where `sweep_stale` runs once at startup, this runs on the
/// maintenance tick — a reaped record leaves its files behind, and without
/// this they would accumulate while the daemon runs.
///
/// The fence is "no record", not "exited": a probe re-creates a missing
/// `<id>.lock`, so touching the files of an id the registry still holds would
/// cycle — swept one tick, back the next. An id with a record — live, exited
/// or in flight — is never touched, nor is a live host's. Ids are collected
/// under one `with_shells` acquisition; every probe and `remove_file` runs
/// with the lock released — the collect-then-act shape.
pub fn sweep_orphaned(reg: &ShellRegistry, dir: &Path) -> Vec<ShellId> {
    let scanned = scan_shell_ids(dir);
    let orphaned: Vec<ShellId> = with_shells(reg, |store| {
        scanned
            .into_iter()
            .filter(|id| !store.contains_key(id.as_str()))
            .collect()
    });
    let mut swept = Vec::new();
    for id in orphaned {
        if host_is_alive(&dir.join(format!("{}.lock", id.as_str()))) {
            continue;
        }
        sweep_files(dir, &id);
        swept.push(id);
    }
    swept
}

/// The maintenance tick's reaper + sweeper in one call: reap the records past
/// their exit retention, then sweep the files of any dead host the registry
/// no longer holds — including the records just reaped, so a shell reaped on
/// this tick has its files swept on the same tick rather than the next.
pub fn reap_and_sweep(
    reg: &ShellRegistry,
    dir: &Path,
    retention_secs: u64,
    now_unix: u64,
) -> Vec<ShellId> {
    let _reaped = reap_exited(reg, retention_secs, now_unix);
    sweep_orphaned(reg, dir)
}

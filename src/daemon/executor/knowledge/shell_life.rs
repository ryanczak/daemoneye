//! The shell lifecycle: `open_shell`, `close_shell`, `close_owner_shells`, and their test seams.

use super::*;

// open_shell + close_shell (M22 phase-04a)
// ---------------------------------------------------------------------------

/// The default readiness timeout for a freshly spawned shell host.
pub(super) const SHELL_READY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// How long `send_signal` may wait for the host to acknowledge the frame.
/// Short on purpose: the acknowledgement is a socket round-trip, not the job
/// reacting to the signal.
pub(super) const SIGNAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// The PTY geometry this module's test host spawns with — the same 24×80
/// `shell_exec` uses, so both fixtures produce interchangeable hosts. Test-only:
/// production spawns read `[shells] rows`/`cols` through `spawn_size()`.
#[cfg(test)]
pub(super) const TEST_ROWS: u16 = 24;
/// The PTY geometry this module's test host spawns with.
#[cfg(test)]
pub(super) const TEST_COLS: u16 = 80;

/// Renders an anyhow chain as `top-level (source) (source)` — the shape
/// `open_shell`'s readiness and cap errors use — and is plain for a chainless
/// error.
pub(crate) fn format_err_chain(e: &anyhow::Error) -> String {
    let mut msg = e.to_string();
    for cause in e.chain().skip(1) {
        msg.push_str(&format!(" ({cause})"));
    }
    msg
}

/// The size a shell is opened at when `open_shell` was given `rows`/`cols`.
/// A missing value comes from `default`, and the resolved pair is clamped by
/// the same bounds `ShellsConfig::size` applies — routed through that method
/// rather than a second copy of the numbers, so the two can never drift.
pub fn resolve_size(rows: Option<u64>, cols: Option<u64>, default: (u16, u16)) -> (u16, u16) {
    // An out-of-`u16` request saturates rather than falling back to the
    // default: the model asked for a size, and the clamp must see *its*
    // number (99999 → the 500 ceiling), not the default it did not ask for.
    let narrow =
        |v: Option<u64>| -> Option<u16> { v.map(|v| u16::try_from(v).unwrap_or(u16::MAX)) };
    let cfg = crate::config::ShellsConfig {
        rows: narrow(rows).unwrap_or(default.0),
        cols: narrow(cols).unwrap_or(default.1),
        ..crate::config::ShellsConfig::default()
    };
    cfg.size()
}

#[cfg(test)]
mod resolve_size_tests {
    use super::resolve_size;

    /// A missing `rows`/`cols` falls back to `default` whole; a present one
    /// replaces only its own half; an absurd pair clamps to the `[shells]`
    /// bounds the config method applies.
    #[test]
    fn resolve_size_fills_and_clamps() {
        assert_eq!(resolve_size(None, None, (40, 120)), (40, 120));
        assert_eq!(resolve_size(Some(50), None, (40, 120)), (50, 120));
        assert_eq!(resolve_size(None, Some(160), (40, 120)), (40, 160));
        assert_eq!(resolve_size(Some(0), Some(99999), (40, 120)), (10, 500));
    }
}
/// through `open_shell_error`, which would wrap a host problem in per-owner
/// shell-limit advice the model cannot act on.
pub async fn open_shell(
    caller: &Owner,
    host: Option<&str>,
    rows: Option<u64>,
    cols: Option<u64>,
) -> String {
    match open_shell_id(caller, host, rows, cols).await {
        Ok((id, host, size)) => opened_shell_sentence(&id, &host, size),
        Err(msg) => msg,
    }
}

/// `open_shell` for a caller that needs the shell's id rather than the
/// model's sentence: the user's `/shell` (M23). `Ok` carries the id, the host
/// it landed on and the size it was opened at; `Err` carries the same
/// sentence `open_shell` returns.
pub async fn open_shell_id(
    caller: &Owner,
    host: Option<&str>,
    rows: Option<u64>,
    cols: Option<u64>,
) -> Result<(ShellId, crate::host::HostId, (u16, u16)), String> {
    let config = crate::config::Config::load().unwrap_or_default();
    let max_per_owner = config.shells.max_per_owner;
    let size = resolve_size(rows, cols, crate::shell::spawn_size());
    let host = match host.map(crate::host::HostId::parse) {
        None => crate::host::HostId::Local,
        Some(Ok(parsed)) => parsed,
        Some(Err(msg)) => return Err(format!("Error: {msg}")),
    };
    // The two arms spawn different programs, so their closure types differ and
    // each calls `open_shell_with_id` itself.
    let id = match &host {
        crate::host::HostId::Local => {
            open_shell_with_id(
                &open_shell_test_registry(),
                caller,
                crate::host::HostId::Local,
                max_per_owner,
                SHELL_READY_TIMEOUT,
                move |id| spawn_in_process_host_sized(id, size),
            )
            .await
        }
        crate::host::HostId::Named(name) => {
            // `host_entry` resolves before the spawn on purpose: its refusals
            // (unknown host, empty destination) must reach the model as their
            // own sentences. Routing them through `open_shell_error` would
            // dress a host problem in per-owner shell-limit advice the model
            // cannot act on (§ Current state 5).
            if let Err(e) = crate::shell::host_entry(name) {
                return Err(format!("Error: {e}"));
            }
            let name = name.clone();
            open_shell_with_id(
                &open_shell_test_registry(),
                caller,
                crate::host::HostId::Named(name.clone()),
                max_per_owner,
                SHELL_READY_TIMEOUT,
                move |id| {
                    let name = name.clone();
                    async move { spawn_on_named_host(id, &name, size).await }
                },
            )
            .await
        }
    }?;
    Ok((id, host, size))
}

/// `open_shell` with every effect injected: the registry, the host the
/// record is stamped with, the per-owner cap, the readiness timeout and the
/// spawn step. Tests replace `spawn` with an in-process host because
/// `spawn_detached_host` re-invokes `current_exe()`, which under a test
/// binary is not the daemon (07b F4).
#[cfg(test)]
pub(super) async fn open_shell_with<F, Fut>(
    reg: &ShellRegistry,
    caller: &Owner,
    host: crate::host::HostId,
    max_per_owner: u32,
    ready_timeout: Duration,
    spawn: F,
) -> String
where
    F: FnOnce(ShellId) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    match open_shell_with_id(
        reg,
        caller,
        host.clone(),
        max_per_owner,
        ready_timeout,
        spawn,
    )
    .await
    {
        Ok(id) => opened_shell_sentence(
            &id,
            &host,
            resolve_size(None, None, crate::shell::spawn_size()),
        ),
        Err(msg) => msg,
    }
}

/// `open_shell_with`'s work: spawn, wait for readiness, and return the id, or
/// the sentence the model reads when the shell could not be opened.
async fn open_shell_with_id<F, Fut>(
    reg: &ShellRegistry,
    caller: &Owner,
    host: crate::host::HostId,
    max_per_owner: u32,
    ready_timeout: Duration,
    spawn: F,
) -> Result<ShellId, String>
where
    F: FnOnce(ShellId) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    // Neither reuse helper is used: the reuse scan would hand the same id
    // back twice, and the dedicated path sets `dedicated: true`, which
    // excludes a shell from that scan forever. The `false` below is what
    // keeps a freshly opened shell reachable by the reuse rule.
    let id = match spawn_for_on_host(
        reg,
        caller.clone(),
        max_per_owner,
        host.clone(),
        false,
        spawn,
    )
    .await
    {
        Ok(id) => id,
        // The per-owner cap lives in `spawn_for`'s `check_owner_cap` breach,
        // returned as text rather than an `Err`: every tool result is a value
        // the model reads, so the refusal must name the configured limit.
        Err(e) => return Err(open_shell_error(max_per_owner, &e)),
    };
    // `spawn_for` reserved the record and rolled back only a *spawn* failure,
    // so a host that never came up leaves a record behind that would be
    // handed to the next caller as a live shell. Undo the reservation here.
    let socket = socket_path(id.as_str());
    if let Err(e) = await_ready(&socket, Instant::now() + ready_timeout).await {
        with_shells(reg, |store| {
            store.remove(id.as_str());
        });
        return Err(format!(
            "Error: shell {} did not become ready, and its empty registry record \
             was removed, so nothing can reach a dead shell: {}.",
            id.as_str(),
            format_err_chain(&e)
        ));
    }
    Ok(id)
}

/// The sentence the model reads after `open_shell` opened shell `id` on
/// `host`. `size` is the `(rows, cols)` the shell was actually opened at —
/// the resolved request, after the clamp — so the model knows what it got
/// when the request was out of range or absent.
fn opened_shell_sentence(id: &ShellId, host: &crate::host::HostId, size: (u16, u16)) -> String {
    // The local sentence is byte-identical to the pre-host one (`HostId` has
    // no printable form); a named host is appended so the model can see where
    // the shell landed and address follow-up work there. The size clause
    // names the real geometry after the host clause, before the `Note:`.
    let opened_on = match host {
        crate::host::HostId::Local => String::new(),
        crate::host::HostId::Named(name) => format!(" It is on host {name}."),
    };
    format!(
        "Opened shell {}. It is now registered and readable — list_shells lists \
         it, read_shell and find_in_shells read its recorded output, close_shell \
         ends it.{opened_on} Its terminal is {r} rows × {c} columns. Note: a \
         command still runs in this session's lowest-numbered open shell by \
         default, so this shell sees work only when it is that one — pass \
         shell: {:?} to run_terminal_command to target it directly, or \
         shell: \"new\" for one command's exclusive shell.",
        id.as_str(),
        id.as_str(),
        r = size.0,
        c = size.1,
    )
}

/// The registry `open_shell`'s tests drive. The daemon-wide singleton is a
/// `OnceLock` that survives across tests, so a test that goes through the
/// public form needs a registry it can assert on afterwards — swap in a fresh
/// one and restore the old when the returned guard drops.
#[cfg(test)]
pub(crate) mod open_shell_test_seam {
    use std::sync::Mutex;

    static REPLACE: Mutex<Option<crate::shell::ShellRegistry>> = Mutex::new(None);

    /// Install `fresh` as the registry `open_shell` resolves, for as long as
    /// the returned guard is alive: dropping it puts back whatever `REPLACE`
    /// held before the swap, so nested swaps unwind in order and an outer
    /// test's registry survives an inner test's.
    pub(crate) fn swap_in(fresh: &crate::shell::ShellRegistry) -> SwapGuard {
        let previous = REPLACE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .replace(fresh.clone());
        SwapGuard { previous }
    }

    /// The swapped-in registry, if any test currently holds one.
    pub(crate) fn swapped() -> Option<crate::shell::ShellRegistry> {
        REPLACE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Restores the pre-swap contents of `REPLACE` when dropped. Holds the
    /// *previous* value rather than clearing, so nesting restores in order.
    pub(crate) struct SwapGuard {
        previous: Option<crate::shell::ShellRegistry>,
    }

    impl Drop for SwapGuard {
        fn drop(&mut self) {
            *REPLACE
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = self.previous.take();
        }
    }
}

#[cfg(test)]
pub(super) fn open_shell_test_registry() -> crate::shell::ShellRegistry {
    match open_shell_test_seam::swapped() {
        Some(swapped) => swapped,
        None => crate::shell::registry().clone(),
    }
}

#[cfg(test)]
pub(super) async fn spawn_in_process_host(id: ShellId) -> anyhow::Result<()> {
    tests::spawn_in_process_host(id).await
}

/// The test build's local host: the fixed-size in-process host, ignoring the
/// resolved `size` the request named. The production form below spawns a
/// detached host at `size`; under `cargo test` the detached host would
/// re-execute the test binary (07b F4), so the in-process fixture is used.
#[cfg(test)]
async fn spawn_in_process_host_sized(id: ShellId, _size: (u16, u16)) -> anyhow::Result<()> {
    spawn_in_process_host(id).await
}

/// The ignored ssh variant of the in-process host: the real `ssh` argv for
/// `pinky`, spawned by `PtyBackend` right here (not detached), so the process
/// bound to the shell's socket is an `ssh` talking to the real remote host
/// while the registry sees a normal in-process shell host.
#[cfg(test)]
pub(crate) async fn spawn_pinky_ssh(id: ShellId) -> anyhow::Result<()> {
    let control = crate::config::ssh_dir();
    use anyhow::Context as _;
    std::fs::create_dir_all(&control).with_context(|| format!("creating {}", control.display()))?;
    let entry = crate::shell::host_entry("pinky")?;
    let argv = crate::host::ssh_argv(&entry, &control);
    let program = crate::shell::which_ssh()?;
    let dir = tempfile::tempdir().context("tempdir for cast")?;
    let cast = dir.path().join(format!("{}.cast", id.as_str()));
    let backend =
        crate::shell::PtyBackend::spawn(&program, &argv, TEST_ROWS, TEST_COLS, 100, &cast, 0)
            .context("spawning ssh to pinky")?;
    let lock =
        crate::daemon::instance::InstanceLock::acquire(&crate::shell::lock_path(id.as_str()))
            .context("holding the per-id instance lock")?;
    let socket = crate::shell::socket_path(id.as_str());
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
}

#[cfg(not(test))]
pub(super) fn open_shell_test_registry() -> crate::shell::ShellRegistry {
    crate::shell::registry().clone()
}

async fn spawn_on_named_host(id: ShellId, name: &str, size: (u16, u16)) -> anyhow::Result<()> {
    crate::shell::spawn_ssh_host(id, name, size.0, size.1).await
}

/// The local host at the resolved size. The production form is a detached
/// `daemoneye shell-host` whose PTY is `size`; under `cargo test` that would
/// re-execute the test binary (07b F4), so the fixed-size in-process host is
/// used and the request's size is ignored.
#[cfg(not(test))]
async fn spawn_in_process_host_sized(id: ShellId, size: (u16, u16)) -> anyhow::Result<()> {
    crate::shell::spawn_detached_host(id, &[], size.0, size.1).await
}

/// The cap-breach sentence for `open_shell`. `max_per_owner == 0` means
/// unlimited, so the limit the text names must be the message's own wording,
/// not a number that reads as a finite cap.
fn open_shell_error(max_per_owner: u32, e: &anyhow::Error) -> String {
    let limit = if max_per_owner == 0 {
        "unlimited".to_string()
    } else {
        format!("{max_per_owner}")
    };
    format!(
        "Error: could not open a shell: {} — the per-owner shell limit is \
         {limit}. Close one of your shells with close_shell, or stop asking for \
         new shells this turn.",
        format_err_chain(e)
    )
}

/// Close one of `caller`'s PTY shells and return the sentence the model reads.
///
/// The public form uses the daemon's registry and the standard signal
/// timeout; the `_in` form takes both, mirroring the `list_shells` /
/// `list_shells_in` split.
pub async fn close_shell(shell: &str, caller: &Owner) -> String {
    close_shell_with(crate::shell::registry(), shell, caller, SIGNAL_TIMEOUT).await
}

/// Close every shell `owner` holds and return `(id, sentence)` per shell.
///
/// 08c: a ghost's shells are closed when the ghost ends, so a scheduled
/// remote ghost does not leave one idle ssh shell host per run. Only
/// records whose `owner.key()` matches are touched — a chat session's
/// shells are never closed by a ghost's teardown.
pub(crate) async fn close_owner_shells(
    reg: &ShellRegistry,
    owner: &Owner,
    signal_timeout: Duration,
) -> Vec<(String, String)> {
    let key = owner.key();
    let ids = with_shells(reg, |store| {
        let mut ids: Vec<String> = store
            .values()
            .filter(|record| record.owner.key() == key)
            .map(|record| record.id.as_str().to_string())
            .collect();
        ids.sort_by_key(|id| ShellId::parse(id).map(|s| s.index()).unwrap_or(u32::MAX));
        ids
    });
    let mut closed = Vec::new();
    for id in ids {
        let sentence = close_shell_with(reg, &id, owner, signal_timeout).await;
        closed.push((id, sentence));
    }
    closed
}

/// `close_shell` with the registry and the signal timeout injected.
pub(crate) async fn close_shell_with(
    reg: &ShellRegistry,
    shell: &str,
    caller: &Owner,
    signal_timeout: Duration,
) -> String {
    // Ownership first, and on `Owner::key()` rather than the inner string: a
    // chat session and a ghost job can share an id string, and the kinds are
    // what the cap groups, so they are what the owner check must compare.
    let owner_in_registry: Option<String> = with_shells(reg, |store| {
        store.get(shell).map(|record| record.owner.key())
    });
    let Some(owner_key) = owner_in_registry else {
        return unknown_shell_error(reg, shell);
    };
    if owner_key != caller.key() {
        return format!(
            "Error: shell {shell} is not yours (it belongs to {owner_key}), so this \
             session cannot close it. If it needs closing, the session that owns \
             it does that; list_shells shows which shells are whose."
        );
    }
    // `Close` hangs the shell up (SIGHUP to its pid), which also ends a job
    // running in it; a host too old to know `Close` gets TERM instead. Either
    // is sent exactly once. Whether the shell actually leaves is observed
    // afterwards, not assumed; a host that lingers still holds the lock, so
    // the tool says so instead of claiming success.
    let socket = socket_path(shell);
    if let Err(e) = crate::shell::close_or_term(&socket, signal_timeout).await {
        return format!(
            "Error: shell {shell} could not be signalled, so it is still open: \
             {}. The shell is unchanged; list_shells shows its state, and a \
             later close_shell attempt is safe. If the socket is gone but the \
             shell still lists, its host may have died without the registry \
             noticing.",
            format_err_chain(&e)
        );
    }
    // The signal is one-way; what closes the shell is the host's PTY ending.
    // Give it a bounded moment and observe, rather than asserting either way.
    let lock = crate::shell::lock_path(shell);
    closed_by_observed_end(shell, &lock).await
}

/// The observable outcome of one close: poll the lock up to 1.5s from now
/// and report what is actually seen. A host releases its lock when its shell
/// exits (`serve_until_exit`), so a hung-up shell normally reads as gone well
/// inside the window; one that has not, is reported as still finishing, and
/// the tool's sentence never claims a closure it did not see.
async fn closed_by_observed_end(shell: &str, lock: &std::path::Path) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1500);
    loop {
        if !host_is_alive(lock) {
            return format!(
                "Closed shell {shell}: its host is gone and the shell is no longer live."
            );
        }
        if std::time::Instant::now() >= deadline {
            return format!(
                "Asked shell {shell} to close, but its host is still holding its \
                 lock — the shell may still be finishing. list_shells still shows \
                 it; check again before assuming it is closed."
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

#[cfg(test)]
pub(crate) mod lifecycle_test_support {
    //! Shared by `shell_exec.rs` and `bg_shell.rs`'s own copies — not by this
    //! module's tests, which use the module-local `spawn_in_process_host`. A
    //! fourth caller is the point where the three copies are expected to be
    //! lifted here.

    use anyhow::Context;
    use std::path::PathBuf;

    /// The PTY geometry the in-process test host uses — the same 24×80 the
    /// daemon's real spawn uses.
    pub const ROWS: u16 = 24;
    /// The PTY geometry the in-process test host uses.
    pub const COLS: u16 = 80;

    /// A real in-process PTY host serving the configured socket for `id`: a
    /// bash session whose `.cast` the returned path points at, with the
    /// per-id instance lock held (leaked) so `host_is_alive` reads it as live
    /// exactly as a real host would. `spawn_detached_host` cannot run under a
    /// test (07b F4).
    pub async fn spawn_with_cast(id: crate::shell::ShellId) -> anyhow::Result<PathBuf> {
        let dir = tempfile::tempdir().context("tempdir for cast")?;
        let cast = dir.path().join(format!("{}.cast", id.as_str()));
        let backend = crate::shell::PtyBackend::spawn("bash", &[], ROWS, COLS, 100, &cast, 0)
            .context("spawning bash")?;
        let lock =
            crate::daemon::instance::InstanceLock::acquire(&crate::shell::lock_path(id.as_str()))
                .context("holding the per-id instance lock")?;
        let socket = crate::shell::socket_path(id.as_str());
        let listener = crate::shell::bind(&socket)
            .await
            .context("binding the in-process host's socket")?;
        // Like a real host: serve until the shell exits, then release the lock.
        tokio::spawn(async move {
            let _ = crate::shell::serve_until_exit(
                listener,
                backend,
                std::time::Duration::from_millis(100),
            )
            .await;
            drop(lock);
        });
        std::mem::forget(dir);
        Ok(cast)
    }
}

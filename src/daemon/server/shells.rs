//! The shell relay: the three IPC requests that reach a PTY shell the daemon
//! runs. The caller of every one is the **user** (M23), not the model, so
//! `handle_shell_list` lists every shell regardless of owner — there is no
//! caller key to scope by.

use anyhow::Result;

use crate::daemon::utils::send_response_split;
use crate::ipc::{Response, ShellInfo};
use crate::shell::{ShellId, ShellRegistry, meta_path_for, with_shells};

/// The socket round-trip budget for one relay call. The ack is the write, not
/// the program reacting — the same reasoning `INPUT_TIMEOUT` uses in the
/// executor's `send_input`.
const RELAY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

pub(super) async fn handle_shell_list<W>(tx: &mut W, reg: &ShellRegistry) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    // Collected under the lock — no I/O in the closure — then the per-shell
    // log reads happen outside it.
    let collected: Vec<(String, String, String, u64)> = with_shells(reg, |store| {
        store
            .values()
            .map(|record| {
                (
                    record.id.as_str().to_string(),
                    record.host.as_str().to_string(),
                    record.owner.key(),
                    record.started_unix,
                )
            })
            .collect()
    });
    let mut infos: Vec<ShellInfo> = collected
        .into_iter()
        .map(|(id, host, owner, started_unix)| ShellInfo {
            id,
            host,
            owner,
            started_unix,
            interactive: false,
            dedicated: false,
            exited_unix: None,
            last_command: None,
        })
        .collect();
    infos.sort_by_key(|info| shell_index(&info.id));
    let shells = infos
        .into_iter()
        .map(|info| ShellInfo {
            last_command: last_command(&info.id, info.started_unix),
            ..info
        })
        .collect();
    send_response_split(tx, Response::ShellList { shells }).await?;
    Ok(())
}

pub(super) async fn handle_shell_input<W>(
    tx: &mut W,
    reg: &ShellRegistry,
    id: &str,
    bytes: &[u8],
) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    if let Err(msg) = live_shell(reg, id) {
        send_response_split(tx, Response::Error(msg)).await?;
        return Ok(());
    }
    let result =
        crate::shell::send_input(&crate::shell::socket_path(id), bytes, RELAY_TIMEOUT).await;
    answer_relay(tx, id, result).await
}

pub(super) async fn handle_shell_resize<W>(
    tx: &mut W,
    reg: &ShellRegistry,
    id: &str,
    rows: u16,
    cols: u16,
) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    if let Err(msg) = live_shell(reg, id) {
        send_response_split(tx, Response::Error(msg)).await?;
        return Ok(());
    }
    let result =
        crate::shell::send_resize(&crate::shell::socket_path(id), rows, cols, RELAY_TIMEOUT).await;
    answer_relay(tx, id, result).await
}

/// The registry half of an input/resize relay: `Ok` only when `id` names a
/// shell whose host has not been observed dead. `Err` carries the exact
/// sentence the client reads.
fn live_shell(reg: &ShellRegistry, id: &str) -> Result<(), String> {
    let exited_unix = with_shells(reg, |store| store.get(id).map(|record| record.exited_unix));
    match exited_unix {
        None | Some(Some(_)) => Err(format!("no live shell {id}")),
        Some(None) => Ok(()),
    }
}

async fn answer_relay<W>(tx: &mut W, id: &str, result: anyhow::Result<()>) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    match result {
        Ok(()) => send_response_split(tx, Response::Ok).await?,
        Err(e) => send_response_split(tx, Response::Error(format!("shell {id}: {e:#}"))).await?,
    }
    Ok(())
}

/// The numeric part of a shell id. The ordering key: `s2` must list before
/// `s10`, which comparing the id strings gets backwards.
fn shell_index(id: &str) -> u32 {
    ShellId::parse(id).map(|s| s.index()).unwrap_or(u32::MAX)
}

/// The last command recorded in `id`'s index, or `None` when the record is
/// absent, unreadable or has no commands yet. Best-effort by design: the
/// listing still answers when one shell's log is unreadable.
fn last_command(id: &str, started_unix: u64) -> Option<String> {
    let cast = crate::shell::cast_path(id, started_unix);
    let path = meta_path_for(&cast);
    crate::shell::MetaIndex::load(&path)
        .ok()
        .and_then(|meta| meta.commands.last().map(|c| c.command.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::shell::Owner;
    use crate::test_home_guard;
    use std::sync::Arc;

    // ── Shared fixtures ───────────────────────────────────────────────────

    /// Point `HOME` at a fresh temp dir **and** create the dirs the shell
    /// substrate resolves through it (`shells_dir()` for sockets and locks,
    /// `shell_logs_dir()` for casts and their meta indexes), so a test never
    /// touches the developer's real `~/.daemoneye`. Must be held for the
    /// test's whole body — the guard restores `HOME` on drop.
    fn shell_test_home() -> (crate::TestHomeGuard, tempfile::TempDir) {
        let guard = test_home_guard();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) }
        Config::ensure_dirs().ok();
        (guard, dir)
    }

    /// A real in-process PTY host serving the configured socket for `id`:
    /// a bash session whose `.cast` the returned path points at, with the
    /// per-id instance lock held (leaked) so `host_is_alive` reads it as live
    /// exactly as a real host would. Fourth copy of the fixture — the
    /// registry's `spawn` is a parameter so tests can pass this instead of a
    /// detached launch, which cannot run under a test binary.
    async fn spawn_in_process_host(id: crate::shell::ShellId) -> anyhow::Result<()> {
        use anyhow::Context as _;

        let dir = tempfile::tempdir().context("tempdir for cast")?;
        let cast = dir.path().join(format!("{}.cast", id.as_str()));
        let backend = crate::shell::PtyBackend::spawn("bash", &[], 24, 80, 100, &cast, 0)
            .context("spawning bash")?;
        let lock =
            crate::daemon::instance::InstanceLock::acquire(&crate::shell::lock_path(id.as_str()))
                .context("holding the per-id instance lock")?;
        let socket = crate::shell::socket_path(id.as_str());
        let listener = crate::shell::bind(&socket)
            .await
            .context("binding the in-process host's socket")?;
        tokio::spawn(crate::shell::serve(listener, Arc::clone(&backend)));
        std::mem::forget(lock);
        std::mem::forget(dir);
        Ok(())
    }

    /// The one response line `tx` produced, parsed.
    fn parse_response(tx: &[u8]) -> Response {
        let text = String::from_utf8_lossy(tx);
        serde_json::from_str::<Response>(
            text.lines()
                .next()
                .expect("the handler wrote one response line"),
        )
        .expect("the response line parses")
    }

    // ── Tests ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn shell_list_reports_every_shell_with_its_last_command() {
        let (_guard, _dir) = shell_test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("t".into());
        let id_a = crate::shell::spawn_for(&reg, owner.clone(), 5, false, |_id| async { Ok(()) })
            .await
            .unwrap();
        let id_b = crate::shell::spawn_for(&reg, owner.clone(), 5, false, |_id| async { Ok(()) })
            .await
            .unwrap();
        let started_unix: u64 = with_shells(&reg, |store| {
            store.get(id_a.as_str()).map(|record| record.started_unix)
        })
        .unwrap();

        let cast = crate::shell::cast_path(id_a.as_str(), started_unix);
        std::fs::create_dir_all(cast.parent().unwrap()).unwrap();
        let meta = serde_json::json!({
            "shell_id": id_a.as_str(),
            "cast": cast.file_name().unwrap().to_string_lossy(),
            "started_unix": started_unix,
            "commands": [{
                "index": 0,
                "command": "echo hi",
                "started": 1.0,
                "ended": 1.5,
                "exit_code": 0,
                "first_byte": 355,
                "end_byte": 355,
            }],
        });
        std::fs::write(
            crate::shell::meta_path_for(&cast),
            serde_json::to_string_pretty(&meta).unwrap(),
        )
        .unwrap();

        let mut tx: Vec<u8> = Vec::new();
        handle_shell_list(&mut tx, &reg).await.unwrap();
        let Response::ShellList { shells } = parse_response(&tx) else {
            panic!("expected a ShellList response, got {tx:?}");
        };
        assert_eq!(shells.len(), 2, "both shells the registry holds are listed");
        assert_eq!(shells[0].id, id_a.as_str(), "index order, not string order");
        assert_eq!(shells[1].id, id_b.as_str());
        for (shell, id) in shells.iter().zip([id_a.as_str(), id_b.as_str()]) {
            assert_eq!(shell.host, "local", "a local shell names its host");
            assert_eq!(shell.owner, owner.key(), "the owner key rides the wire");
            assert_eq!(shell.id, id, "the row is the shell the registry recorded");
        }
        assert_eq!(
            shells[0].last_command.as_deref(),
            Some("echo hi"),
            "the first shell's recorded last command is on the wire"
        );
        assert_eq!(
            shells[1].last_command, None,
            "a shell with no readable index reports no last command"
        );
    }

    #[tokio::test]
    async fn shell_input_reaches_the_shell() {
        let (_guard, dir) = shell_test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("t".into());
        let id = crate::shell::spawn_for(&reg, owner.clone(), 5, false, |id| {
            Box::pin(spawn_in_process_host(id))
        })
        .await
        .unwrap();
        let probe = dir.path().join("in-ok");
        let probe_str = probe.display().to_string();
        let cmd = format!("printf in-ok > {probe_str}\n");
        let mut tx: Vec<u8> = Vec::new();
        handle_shell_input(&mut tx, &reg, id.as_str(), cmd.as_bytes())
            .await
            .unwrap();
        assert!(
            matches!(parse_response(&tx), Response::Ok),
            "an acked write answers Ok, got {tx:?}"
        );

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !probe.exists() && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(
            probe.exists(),
            "the shell must have run what was typed: {probe:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&probe).unwrap_or_default(),
            "in-ok",
            "the typed command's output landed in the file"
        );

        let _ = crate::daemon::executor::knowledge::close_shell_with(
            &reg,
            id.as_str(),
            &owner,
            std::time::Duration::from_millis(3000),
        )
        .await;
    }

    #[tokio::test]
    async fn shell_resize_changes_the_shells_size() {
        let (_guard, _dir) = shell_test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("t".into());
        let id = crate::shell::spawn_for(&reg, owner.clone(), 5, false, |id| {
            Box::pin(spawn_in_process_host(id))
        })
        .await
        .unwrap();
        let mut tx: Vec<u8> = Vec::new();
        handle_shell_resize(&mut tx, &reg, id.as_str(), 30, 100)
            .await
            .unwrap();
        assert!(
            matches!(parse_response(&tx), Response::Ok),
            "an acked resize answers Ok, got {tx:?}"
        );
        match crate::shell::status(&crate::shell::socket_path(id.as_str()), RELAY_TIMEOUT)
            .await
            .expect("the resized shell answers a status probe")
        {
            crate::shell::ShellResponse::Status { rows, cols, .. } => {
                assert_eq!(rows, 30, "the resize must have reached the PTY");
                assert_eq!(cols, 100, "the resize must have reached the PTY");
            }
            other => panic!("expected a Status frame, got {other:?}"),
        }

        let _ = crate::daemon::executor::knowledge::close_shell_with(
            &reg,
            id.as_str(),
            &owner,
            std::time::Duration::from_millis(3000),
        )
        .await;
    }

    #[tokio::test]
    async fn shell_ipc_refuses_an_unknown_shell() {
        let (_guard, _dir) = shell_test_home();
        let reg = ShellRegistry::default();
        let mut tx: Vec<u8> = Vec::new();
        handle_shell_input(&mut tx, &reg, "s99", b"x")
            .await
            .unwrap();
        assert!(
            matches!(parse_response(&tx), Response::Error(ref msg) if msg.contains("s99")),
            "an unknown id is refused with the id named: {tx:?}"
        );
        let mut tx2: Vec<u8> = Vec::new();
        handle_shell_resize(&mut tx2, &reg, "not-a-shell", 1, 1)
            .await
            .unwrap();
        assert!(
            matches!(parse_response(&tx2), Response::Error(_)),
            "an id outside the grammar is refused too: {tx2:?}"
        );
        assert!(reg.try_lock().is_ok(), "the registry lock was released");
    }
}

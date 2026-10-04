//! The shell relay: the three IPC requests that reach a PTY shell the daemon
//! runs. The caller of every one is the **user** (M23), not the model, so
//! `handle_shell_list` lists every shell regardless of owner — there is no
//! caller key to scope by.

use anyhow::Result;

use crate::daemon::utils::send_response_split;
use crate::ipc::{LogCommand, LogEvent, Response, ShellInfo, ShellLogInfo};
use crate::shell::{ShellId, ShellRegistry, meta_path_for, with_shells};

/// The socket round-trip budget for one relay call. The ack is the write, not
/// the program reacting — the same reasoning `INPUT_TIMEOUT` uses in the
/// executor's `send_input`.
const RELAY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

pub(super) async fn handle_shell_list<W>(tx: &mut W, reg: &ShellRegistry) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    // Collected under the lock — no I/O in the closure, only field copies —
    // then the per-shell log reads happen outside it.
    let mut infos: Vec<ShellInfo> = with_shells(reg, |store| {
        store
            .values()
            .map(|record| ShellInfo {
                id: record.id.as_str().to_string(),
                host: record.host.as_str().to_string(),
                owner: record.owner.key(),
                started_unix: record.started_unix,
                interactive: record.interactive,
                dedicated: record.dedicated,
                exited_unix: record.exited_unix,
                last_command: None,
            })
            .collect()
    });
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

/// How long a signal's effect is waited for. A stopped job reads `T` in
/// `/proc` only once the kernel has delivered `SIGSTOP`, so the state right
/// after the ack can still be the old one.
const SIGNAL_SETTLE: std::time::Duration = std::time::Duration::from_secs(1);

/// Relay a signal from attached mode to a shell's foreground job, then answer
/// with the state it settled in, so the client shows "paused" only when a job
/// really stopped: `STOP` with nothing running leaves the shell `Idle`.
pub(super) async fn handle_shell_signal<W>(
    tx: &mut W,
    reg: &ShellRegistry,
    id: &str,
    signal: &str,
) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    use crate::shell::ShellSignal;
    let sig = match signal {
        "STOP" => ShellSignal::Stop,
        "CONT" => ShellSignal::Cont,
        "INT" => ShellSignal::Int,
        other => {
            let msg = format!("{other:?} is not a signal attached mode sends (STOP, CONT, INT)");
            send_response_split(tx, Response::Error(msg)).await?;
            return Ok(());
        }
    };
    if let Err(msg) = live_shell(reg, id) {
        send_response_split(tx, Response::Error(msg)).await?;
        return Ok(());
    }
    let sock = crate::shell::socket_path(id);
    if let Err(e) = crate::shell::send_signal(&sock, sig, RELAY_TIMEOUT).await {
        send_response_split(tx, Response::Error(format!("shell {id}: {e:#}"))).await?;
        return Ok(());
    }
    let response = match settled_state(&sock, sig).await {
        Ok(state) => Response::ShellSignalled {
            state: state_name(state).to_string(),
        },
        Err(e) => Response::Error(format!("shell {id}: {e:#}")),
    };
    send_response_split(tx, response).await?;
    Ok(())
}

/// The shell's state once `sig` has had its effect: read until it is `Paused`
/// after `STOP`, or anything but `Paused` after `CONT` or `INT`, or until
/// `SIGNAL_SETTLE` runs out, when the last state read is the answer.
async fn settled_state(
    sock: &std::path::Path,
    sig: crate::shell::ShellSignal,
) -> anyhow::Result<crate::shell::ShellState> {
    use crate::shell::{ShellResponse, ShellSignal, ShellState};
    let deadline = tokio::time::Instant::now() + SIGNAL_SETTLE;
    loop {
        let state = match crate::shell::status(sock, RELAY_TIMEOUT).await? {
            ShellResponse::Status { state, .. } => state,
            other => anyhow::bail!("unexpected status reply {other:?}"),
        };
        let settled = match sig {
            ShellSignal::Stop => state == ShellState::Paused,
            _ => state != ShellState::Paused,
        };
        if settled || tokio::time::Instant::now() >= deadline {
            return Ok(state);
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// A shell state as `ShellSignalled` names it.
fn state_name(state: crate::shell::ShellState) -> &'static str {
    use crate::shell::ShellState;
    match state {
        ShellState::Idle => "Idle",
        ShellState::Running => "Running",
        ShellState::Paused => "Paused",
        ShellState::Exited => "Exited",
    }
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

/// How many recordings `/logs` shows when it names no limit.
const LOG_LIST_DEFAULT_LIMIT: usize = 50;

pub(super) async fn handle_shell_log_list<W>(
    tx: &mut W,
    since: Option<u64>,
    host: Option<&str>,
    owner: Option<&str>,
    limit: Option<usize>,
) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    let all = scan_shell_logs(&crate::config::shell_logs_dir());
    let matched: Vec<ShellLogInfo> = all
        .into_iter()
        .filter(|log| log_matches(log, since, host, owner))
        .collect();
    let total = matched.len();
    let logs = matched
        .into_iter()
        .take(limit.unwrap_or(LOG_LIST_DEFAULT_LIMIT))
        .collect();
    send_response_split(tx, Response::ShellLogList { logs, total }).await?;
    Ok(())
}

/// Every recording in `dir`, newest first. A `.cast` whose name is not
/// `<shell id>-<start>` is not a recording and is skipped; a missing or
/// unreadable index leaves `owner`, `host`, `commands` and `first_command`
/// `None`. The first command is masked before it leaves the daemon.
fn scan_shell_logs(dir: &std::path::Path) -> Vec<ShellLogInfo> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut logs: Vec<ShellLogInfo> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_string();
            let (_, started_unix) = crate::shell::parse_cast_name(&name)?;
            let meta = crate::shell::MetaIndex::load(&meta_path_for(&entry.path())).ok();
            Some(ShellLogInfo {
                id: name.strip_suffix(".cast").unwrap_or(&name).to_string(),
                started_unix,
                owner: meta.as_ref().and_then(|m| m.owner.clone()),
                host: meta.as_ref().and_then(|m| m.host.clone()),
                commands: meta.as_ref().map(|m| m.commands.len()),
                first_command: meta.as_ref().and_then(|m| {
                    m.commands
                        .first()
                        .map(|c| crate::ai::filter::mask_sensitive(&c.command))
                }),
            })
        })
        .collect();
    logs.sort_by_key(|log| std::cmp::Reverse(log.started_unix));
    logs
}

/// Whether `log` passes `/logs`' filters. `owner` is a kind: `"ghost"` keeps
/// `ghost:<job>`. A recording whose index does not say its owner or host
/// fails a filter on that field — it cannot be shown to match.
fn log_matches(
    log: &ShellLogInfo,
    since: Option<u64>,
    host: Option<&str>,
    owner: Option<&str>,
) -> bool {
    if since.is_some_and(|s| log.started_unix < s) {
        return false;
    }
    if let Some(h) = host
        && log.host.as_deref() != Some(h)
    {
        return false;
    }
    if let Some(kind) = owner {
        let Some(key) = log.owner.as_deref() else {
            return false;
        };
        if key != kind && !key.starts_with(&format!("{kind}:")) {
            return false;
        }
    }
    true
}

/// The most output text one `ShellLogRead` sends. A larger recording loses
/// its oldest events, and the reply says so.
const LOG_READ_CAP: usize = 4 << 20;

pub(super) async fn handle_shell_log_read<W>(tx: &mut W, id: &str) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    let response = match read_shell_log(&crate::config::shell_logs_dir(), id, LOG_READ_CAP) {
        Ok(log) => Response::ShellLog {
            id: id.to_string(),
            rows: log.rows,
            cols: log.cols,
            events: log.events,
            commands: log.commands,
            truncated: log.truncated,
        },
        Err(e) => Response::Error(e),
    };
    send_response_split(tx, response).await?;
    Ok(())
}

/// A recording as `ShellLogRead` returns it.
struct ShellLogData {
    rows: u16,
    cols: u16,
    events: Vec<LogEvent>,
    commands: Vec<LogCommand>,
    truncated: bool,
}

/// The cast header fields the viewer needs.
#[derive(serde::Deserialize)]
struct CastHeader {
    width: u16,
    height: u16,
}

/// Read recording `id` from `dir`: its `"o"` events in order and, from its
/// index, where each command begins. A command begins at the first output
/// event after the last `"i"` event before its `first_byte` (the typed
/// command line), or at its `first_byte` when nothing was typed. A line that
/// does not parse — the unfinished last line of a live recording — is
/// skipped. Past `cap` bytes of output the oldest events are dropped, and
/// commands that began in them with them.
fn read_shell_log(dir: &std::path::Path, id: &str, cap: usize) -> Result<ShellLogData, String> {
    if crate::shell::parse_cast_name(&format!("{id}.cast")).is_none() {
        return Err(format!(
            "{id:?} is not a log id: a log id is a shell id plus its start time, \
             like s3-1788470000 (see /logs)"
        ));
    }
    let cast = dir.join(format!("{id}.cast"));
    let text = std::fs::read_to_string(&cast)
        .map_err(|e| format!("no recording named {id} ({e}); /logs lists them"))?;
    let mut lines = text.split_inclusive('\n');
    let header: CastHeader = lines
        .next()
        .and_then(|l| serde_json::from_str(l).ok())
        .ok_or_else(|| format!("recording {id} has no readable header"))?;
    let records = crate::shell::MetaIndex::load(&meta_path_for(&cast))
        .map(|m| m.commands)
        .unwrap_or_default();

    let mut events: Vec<LogEvent> = Vec::new();
    // For each record, the output-event count at the last "i" line in its
    // window, if one was typed there.
    let mut typed_at: Vec<Option<usize>> = vec![None; records.len()];
    let mut starts: Vec<Option<usize>> = vec![None; records.len()];
    let mut offset = text.find('\n').map_or(text.len(), |n| n + 1) as u64;
    for line in lines {
        let line_start = offset;
        offset += line.len() as u64;
        let Ok((t, code, data)) = serde_json::from_str::<(f64, String, String)>(line) else {
            continue;
        };
        for (k, record) in records.iter().enumerate() {
            let window_start = if k == 0 { 0 } else { records[k - 1].end_byte };
            if starts[k].is_none() && line_start >= record.first_byte {
                starts[k] = Some(typed_at[k].unwrap_or(events.len()));
            }
            if code == "i" && line_start >= window_start && line_start < record.first_byte {
                typed_at[k] = Some(events.len());
            }
        }
        if code == "o" {
            events.push(LogEvent { t, data });
        }
    }

    let mut commands: Vec<LogCommand> = records
        .iter()
        .zip(&starts)
        .filter_map(|(record, start)| {
            start.map(|event| LogCommand {
                event,
                command: record.command.clone(),
                exit_code: record.exit_code,
            })
        })
        .collect();

    let total: usize = events.iter().map(|e| e.data.len()).sum();
    let mut dropped = 0;
    let mut kept = total;
    while kept > cap && dropped < events.len() {
        kept -= events[dropped].data.len();
        dropped += 1;
    }
    if dropped > 0 {
        events.drain(..dropped);
        commands.retain(|c| c.event >= dropped);
        for c in &mut commands {
            c.event -= dropped;
        }
    }
    Ok(ShellLogData {
        rows: header.height,
        cols: header.width,
        events,
        commands,
        truncated: dropped > 0,
    })
}

/// The shell host's answers, one JSON frame per line.
type HostLines = tokio::io::Lines<tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>>;

/// How often a followed tail asks the host whether its shell has exited. A
/// current host closes the connection when its shell ends, but a host from
/// an older binary never does, so the tail also asks.
const TAIL_STATUS_POLL: std::time::Duration = std::time::Duration::from_millis(500);

/// Relay one shell to the client: its current screen as `ShellScreen`, then,
/// with `follow`, each output chunk as `ShellChunk` until the shell exits
/// (`ShellExited`) or the client goes away. `rx` is the client's half of the
/// connection; the tail ends when it reaches EOF.
pub(super) async fn handle_shell_tail<R, W>(
    rx: &mut R,
    tx: &mut W,
    reg: &ShellRegistry,
    id: &str,
    follow: bool,
) -> Result<()>
where
    R: tokio::io::AsyncBufRead + Unpin,
    W: tokio::io::AsyncWriteExt + Unpin,
{
    use tokio::io::AsyncBufReadExt;

    if let Err(msg) = live_shell(reg, id) {
        send_response_split(tx, Response::Error(msg)).await?;
        return Ok(());
    }
    let sock = crate::shell::socket_path(id);
    // The screen bytes assume the shell's own terminal size; the client needs
    // it to draw them. A host that cannot answer reads as the spawn default.
    let (rows, cols) = match crate::shell::status(&sock, RELAY_TIMEOUT).await {
        Ok(crate::shell::ShellResponse::Status { rows, cols, .. }) => (rows, cols),
        _ => (24, 80),
    };
    let stream = match tokio::net::UnixStream::connect(&sock).await {
        Ok(s) => s,
        Err(e) => {
            send_response_split(tx, Response::Error(format!("shell {id}: {e}"))).await?;
            return Ok(());
        }
    };
    // `host_w` must stay alive for the whole tail: the host ends a connection
    // whose write side reaches EOF, which would end the subscription.
    let (host_r, mut host_w) = stream.into_split();
    let mut host = tokio::io::BufReader::new(host_r).lines();

    let screen = match subscribe(&mut host_w, &mut host).await {
        Ok(bytes) => bytes,
        Err(e) => {
            send_response_split(tx, Response::Error(format!("shell {id}: {e:#}"))).await?;
            return Ok(());
        }
    };
    // From here on a failed write means the client hung up: that ends the
    // tail normally, it is not an error.
    if send_response_split(
        tx,
        Response::ShellScreen {
            bytes: screen,
            rows,
            cols,
        },
    )
    .await
    .is_err()
    {
        return Ok(());
    }
    if !follow {
        return Ok(());
    }

    let mut client = rx.lines();
    let mut tick = tokio::time::interval(TAIL_STATUS_POLL);
    loop {
        tokio::select! {
            line = host.next_line() => match line {
                Ok(Some(line)) => {
                    if let Ok(crate::shell::ShellResponse::Chunk { bytes }) =
                        serde_json::from_str(&line)
                        && send_response_split(tx, Response::ShellChunk { bytes })
                            .await
                            .is_err()
                    {
                        return Ok(());
                    }
                }
                // The host closed the connection: a current host does that
                // when its shell exits.
                Ok(None) | Err(_) => {
                    let _ = send_response_split(tx, Response::ShellExited).await;
                    return Ok(());
                }
            },
            line = client.next_line() => match line {
                Ok(Some(_)) => {}
                // The client hung up; stop relaying.
                Ok(None) | Err(_) => return Ok(()),
            },
            _ = tick.tick() => {
                if shell_has_exited(&sock).await {
                    let _ = send_response_split(tx, Response::ShellExited).await;
                    return Ok(());
                }
            }
        }
    }
}

/// Ask the host for its screen (`SubscribeScreen`). A host from before that
/// request answers `Error`; fall back to `Subscribe` and an empty screen.
async fn subscribe(
    host_w: &mut tokio::net::unix::OwnedWriteHalf,
    host: &mut HostLines,
) -> anyhow::Result<Vec<u8>> {
    use crate::shell::{ShellRequest, ShellResponse, encode};
    use anyhow::Context as _;
    use tokio::io::AsyncWriteExt;

    host_w
        .write_all(encode(ShellRequest::SubscribeScreen)?.as_bytes())
        .await
        .context("sending SubscribeScreen")?;
    match next_frame(host).await? {
        ShellResponse::Screen { bytes } => Ok(bytes),
        ShellResponse::Error { .. } => {
            host_w
                .write_all(encode(ShellRequest::Subscribe)?.as_bytes())
                .await
                .context("sending Subscribe")?;
            match next_frame(host).await? {
                ShellResponse::Ok => Ok(Vec::new()),
                other => anyhow::bail!("unexpected answer to Subscribe: {other:?}"),
            }
        }
        other => anyhow::bail!("unexpected answer to SubscribeScreen: {other:?}"),
    }
}

/// The host's next frame, within the relay timeout.
async fn next_frame(host: &mut HostLines) -> anyhow::Result<crate::shell::ShellResponse> {
    use anyhow::Context as _;
    let line = tokio::time::timeout(RELAY_TIMEOUT, host.next_line())
        .await
        .context("timed out waiting for the shell host")?
        .context("reading from the shell host")?
        .context("the shell host closed the connection")?;
    serde_json::from_str(&line).context("parsing the shell host's frame")
}

/// Whether the host says its shell has exited, or no longer answers at all.
async fn shell_has_exited(sock: &std::path::Path) -> bool {
    matches!(
        crate::shell::status(sock, RELAY_TIMEOUT).await,
        Ok(crate::shell::ShellResponse::Status {
            state: crate::shell::ShellState::Exited,
            ..
        }) | Err(_)
    )
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

    /// Write a cast (header only) named `<id>`.cast into the logs dir, and its
    /// index when `meta` is given.
    fn write_recording(id: &str, meta: Option<serde_json::Value>) {
        let dir = crate::config::shell_logs_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let cast = dir.join(format!("{id}.cast"));
        std::fs::write(&cast, "{\"version\":2,\"width\":80,\"height\":24}\n").unwrap();
        if let Some(meta) = meta {
            std::fs::write(meta_path_for(&cast), meta.to_string()).unwrap();
        }
    }

    async fn log_list(
        since: Option<u64>,
        host: Option<&str>,
        owner: Option<&str>,
        limit: Option<usize>,
    ) -> (Vec<String>, usize, Vec<ShellLogInfo>) {
        let mut tx: Vec<u8> = Vec::new();
        handle_shell_log_list(&mut tx, since, host, owner, limit)
            .await
            .unwrap();
        let Response::ShellLogList { logs, total } = parse_response(&tx) else {
            panic!("expected a ShellLogList response, got {tx:?}");
        };
        (logs.iter().map(|l| l.id.clone()).collect(), total, logs)
    }

    /// A recording written through the real `CastWriter`: a prompt, two
    /// typed commands with their echo and output, and (with `indexed`) the
    /// index naming both commands at their output lines. Returns the dir.
    fn two_command_recording(id: &str, indexed: bool) -> std::path::PathBuf {
        use crate::shell::{CastWriter, CommandRecord, MetaIndex};
        use std::time::Duration;
        let dir = crate::config::shell_logs_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let cast = dir.join(format!("{id}.cast"));
        let at = Duration::from_secs_f64;
        let mut w = CastWriter::create(&cast, 100, 30, 17).unwrap();
        w.write_output(at(0.1), b"$ ").unwrap(); // event 0
        w.write_input(at(1.0), b"echo one\n").unwrap();
        w.write_output(at(1.1), b"echo one\r\n").unwrap(); // event 1
        let first0 = w.byte_len();
        w.write_output(at(1.2), b"one\r\n").unwrap(); // event 2
        let end0 = w.byte_len();
        w.write_output(at(1.3), b"$ ").unwrap(); // event 3
        w.write_input(at(2.0), b"echo two\n").unwrap();
        w.write_output(at(2.1), b"echo two\r\n").unwrap(); // event 4
        let first1 = w.byte_len();
        w.write_output(at(2.2), b"two\r\n").unwrap(); // event 5
        let end1 = w.byte_len();
        drop(w);
        if indexed {
            let record = |command: &str, exit_code, first_byte, end_byte| CommandRecord {
                index: 0,
                command: command.to_string(),
                started: 0.0,
                ended: 0.0,
                exit_code,
                first_byte,
                end_byte,
            };
            let meta = MetaIndex {
                shell_id: "s4".into(),
                cast: format!("{id}.cast"),
                started_unix: 17,
                commands: vec![
                    record("echo one", 0, first0, end0),
                    record("echo two", 3, first1, end1),
                ],
                owner: None,
                host: None,
            };
            meta.save(&meta_path_for(&cast)).unwrap();
        }
        dir
    }

    #[tokio::test]
    async fn shell_log_read_returns_events_and_where_each_command_begins() {
        let (_guard, _dir) = shell_test_home();
        two_command_recording("s4-17", true);
        let mut tx: Vec<u8> = Vec::new();
        handle_shell_log_read(&mut tx, "s4-17").await.unwrap();
        let Response::ShellLog {
            id,
            rows,
            cols,
            events,
            commands,
            truncated,
        } = parse_response(&tx)
        else {
            panic!("expected a ShellLog response, got {tx:?}");
        };
        assert_eq!(
            (id.as_str(), rows, cols),
            ("s4-17", 30, 100),
            "the header's size"
        );
        let data: Vec<&str> = events.iter().map(|e| e.data.as_str()).collect();
        assert_eq!(
            data,
            [
                "$ ",
                "echo one\r\n",
                "one\r\n",
                "$ ",
                "echo two\r\n",
                "two\r\n"
            ],
            "every output event in order; input events are not output"
        );
        assert_eq!(events[4].t, 2.1, "each event keeps its time");
        let marks: Vec<(usize, &str, i32)> = commands
            .iter()
            .map(|c| (c.event, c.command.as_str(), c.exit_code))
            .collect();
        assert_eq!(
            marks,
            [(1, "echo one", 0), (4, "echo two", 3)],
            "a command begins at the output after it was typed"
        );
        assert!(!truncated);
    }

    #[test]
    fn shell_log_read_marks_at_the_output_when_nothing_was_typed() {
        let (_guard, _dir) = shell_test_home();
        let dir = two_command_recording("s4-17", true);
        let cast = dir.join("s4-17.cast");
        let text = std::fs::read_to_string(&cast).unwrap();
        let no_input: String = text
            .split_inclusive('\n')
            .map(|l| if l.contains(", \"i\", ") { " \n" } else { l })
            .collect();
        // Blanking the "i" lines moves no other line, but re-derive the
        // offsets from the new text anyway rather than trust that.
        std::fs::write(&cast, &no_input).unwrap();
        let mut meta = crate::shell::MetaIndex::load(&meta_path_for(&cast)).unwrap();
        let line_start = |needle: &str| no_input.find(needle).unwrap() as u64;
        meta.commands[0].first_byte = line_start("[1.2,");
        meta.commands[0].end_byte = line_start("[1.3,");
        meta.commands[1].first_byte = line_start("[2.2,");
        meta.commands[1].end_byte = no_input.len() as u64;
        meta.save(&meta_path_for(&cast)).unwrap();
        let log = read_shell_log(&dir, "s4-17", LOG_READ_CAP).unwrap();
        let marks: Vec<usize> = log.commands.iter().map(|c| c.event).collect();
        assert_eq!(
            marks,
            [2, 5],
            "with no typed line, the command's own output"
        );
    }

    #[test]
    fn shell_log_read_refuses_what_it_cannot_read() {
        let (_guard, _dir) = shell_test_home();
        let dir = two_command_recording("s4-17", false);
        let bare = read_shell_log(&dir, "s4", LOG_READ_CAP).err().unwrap();
        assert!(bare.contains("not a log id"), "{bare}");
        let missing = read_shell_log(&dir, "s9-99", LOG_READ_CAP).err().unwrap();
        assert!(missing.contains("no recording named s9-99"), "{missing}");
        let unindexed = read_shell_log(&dir, "s4-17", LOG_READ_CAP).unwrap();
        assert_eq!(
            unindexed.events.len(),
            6,
            "an unindexed recording still reads"
        );
        assert!(unindexed.commands.is_empty(), "but has no command marks");
        let cast = dir.join("s4-17.cast");
        let mut text = std::fs::read_to_string(&cast).unwrap();
        text.push_str("[3.0, \"o\", \"unfinish");
        std::fs::write(&cast, text).unwrap();
        let live = read_shell_log(&dir, "s4-17", LOG_READ_CAP).unwrap();
        assert_eq!(live.events.len(), 6, "a partial last line is skipped");
    }

    #[test]
    fn shell_log_read_drops_the_oldest_events_past_the_cap() {
        let (_guard, _dir) = shell_test_home();
        let dir = two_command_recording("s4-17", true);
        // The last three events ("$ ", "echo two\r\n", "two\r\n") are 17 bytes.
        let log = read_shell_log(&dir, "s4-17", 17).unwrap();
        assert!(log.truncated);
        let data: Vec<&str> = log.events.iter().map(|e| e.data.as_str()).collect();
        assert_eq!(data, ["$ ", "echo two\r\n", "two\r\n"]);
        let marks: Vec<(usize, &str)> = log
            .commands
            .iter()
            .map(|c| (c.event, c.command.as_str()))
            .collect();
        assert_eq!(
            marks,
            [(1, "echo two")],
            "a dropped command goes; the rest shift"
        );
    }

    #[tokio::test]
    async fn shell_log_list_is_newest_first_and_filters() {
        let (_guard, _dir) = shell_test_home();
        let cmd = |c: &str| {
            serde_json::json!({"index": 0, "command": c, "started": 0.0,
            "ended": 0.0, "exit_code": 0, "first_byte": 0, "end_byte": 1})
        };
        write_recording(
            "s1-100",
            Some(
                serde_json::json!({"shell_id": "s1", "cast": "s1-100.cast", "started_unix": 100,
                "commands": [cmd("echo one")], "owner": "chat:t", "host": "local"}),
            ),
        );
        write_recording(
            "s2-200",
            Some(
                serde_json::json!({"shell_id": "s2", "cast": "s2-200.cast", "started_unix": 200,
                "commands": [cmd("uptime"), cmd("df -h")], "owner": "ghost:job-1",
                "host": "pinky"}),
            ),
        );
        write_recording("s3-300", None);
        write_recording("notes", None);

        let (ids, total, logs) = log_list(None, None, None, None).await;
        assert_eq!(
            ids,
            ["s3-300", "s2-200", "s1-100"],
            "newest first, junk skipped"
        );
        assert_eq!(total, 3);
        assert_eq!(logs[0].commands, None, "no index is unknown, not 0");
        assert_eq!(logs[1].commands, Some(2));
        assert_eq!(logs[1].first_command.as_deref(), Some("uptime"));
        assert_eq!(logs[1].host.as_deref(), Some("pinky"));

        assert_eq!(
            log_list(None, None, Some("ghost"), None).await.0,
            ["s2-200"]
        );
        assert_eq!(log_list(None, None, Some("chat"), None).await.0, ["s1-100"]);
        assert!(
            log_list(None, None, Some("gho"), None).await.0.is_empty(),
            "an owner filter matches a whole kind, not a prefix of one"
        );
        assert_eq!(
            log_list(None, Some("local"), None, None).await.0,
            ["s1-100"]
        );
        assert_eq!(
            log_list(Some(150), None, None, None).await.0,
            ["s3-300", "s2-200"]
        );
        let (ids, total, _) = log_list(None, None, None, Some(1)).await;
        assert_eq!(
            (ids.len(), total),
            (1, 3),
            "the limit caps rows, total counts all"
        );
    }

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

    /// What `handle_shell_signal` answered: the settled state, or the error.
    async fn signal_state(reg: &ShellRegistry, id: &str, signal: &str) -> Result<String, String> {
        let mut tx: Vec<u8> = Vec::new();
        handle_shell_signal(&mut tx, reg, id, signal).await.unwrap();
        match parse_response(&tx) {
            Response::ShellSignalled { state } => Ok(state),
            Response::Error(msg) => Err(msg),
            other => panic!("unexpected reply {other:?}"),
        }
    }

    /// Poll the shell's own status until it reads `want` (5 s cap).
    async fn status_reaches(id: &str, want: crate::shell::ShellState) -> bool {
        let sock = crate::shell::socket_path(id);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if let Ok(crate::shell::ShellResponse::Status { state, .. }) =
                crate::shell::status(&sock, RELAY_TIMEOUT).await
                && state == want
            {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        false
    }

    #[tokio::test]
    async fn shell_signal_pauses_resumes_and_interrupts_the_foreground_job() {
        use crate::shell::ShellState;
        let (_guard, _dir) = shell_test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("t".into());
        let id = crate::shell::spawn_for(&reg, owner.clone(), 5, false, |id| {
            Box::pin(spawn_in_process_host(id))
        })
        .await
        .unwrap();
        let id = id.as_str().to_string();
        let state = |name: &str| Ok(name.to_string());

        assert_eq!(
            signal_state(&reg, &id, "STOP").await,
            state("Idle"),
            "STOP with nothing running pauses nothing"
        );

        let mut tx: Vec<u8> = Vec::new();
        handle_shell_input(&mut tx, &reg, &id, b"sleep 30\n")
            .await
            .unwrap();
        assert!(
            status_reaches(&id, ShellState::Running).await,
            "sleep 30 must be running before it is signalled"
        );
        assert_eq!(
            signal_state(&reg, &id, "STOP").await,
            state("Paused"),
            "STOP answers with the state it settled in"
        );
        assert_eq!(signal_state(&reg, &id, "CONT").await, state("Running"));
        assert_ne!(
            signal_state(&reg, &id, "INT").await,
            state("Paused"),
            "INT leaves nothing paused"
        );
        assert!(
            status_reaches(&id, ShellState::Idle).await,
            "INT ended the foreground job"
        );

        let _ = crate::daemon::executor::knowledge::close_shell_with(
            &reg,
            &id,
            &owner,
            std::time::Duration::from_millis(3000),
        )
        .await;
    }

    #[tokio::test]
    async fn shell_signal_refuses_unknown_signals_and_shells() {
        let (_guard, _dir) = shell_test_home();
        let reg = ShellRegistry::default();
        let bad = signal_state(&reg, "s1", "TERM").await.unwrap_err();
        assert!(bad.contains("STOP, CONT, INT"), "{bad}");
        let missing = signal_state(&reg, "s99", "STOP").await.unwrap_err();
        assert!(missing.contains("no live shell s99"), "{missing}");
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

    #[tokio::test]
    async fn shell_list_carries_each_shells_flags() {
        let (_guard, _dir) = shell_test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("t".into());
        let id_a = crate::shell::spawn_for(&reg, owner.clone(), 5, false, |_id| async { Ok(()) })
            .await
            .unwrap();
        let id_b = crate::shell::spawn_for(&reg, owner.clone(), 5, true, |_id| async { Ok(()) })
            .await
            .unwrap();
        crate::shell::mark_interactive(&reg, &id_a);
        with_shells(&reg, |store| {
            store.get_mut(id_b.as_str()).unwrap().exited_unix = Some(42);
        });

        let mut tx: Vec<u8> = Vec::new();
        handle_shell_list(&mut tx, &reg).await.unwrap();
        let Response::ShellList { shells } = parse_response(&tx) else {
            panic!("expected a ShellList response, got {tx:?}");
        };
        let (first, second) = if shells[0].id == id_a.as_str() {
            (&shells[0], &shells[1])
        } else {
            (&shells[1], &shells[0])
        };
        assert_eq!(first.id, id_a.as_str());
        assert!(
            first.interactive,
            "the record's interactive flag rides the wire, not a hardcoded false"
        );
        assert!(!first.dedicated);
        assert_eq!(first.exited_unix, None);
        assert_eq!(second.id, id_b.as_str());
        assert!(!second.interactive);
        assert!(second.dedicated, "the spawn's dedicated flag rides");
        assert_eq!(
            second.exited_unix,
            Some(42),
            "an exited shell's stamp rides the wire"
        );
    }

    // ── ShellTail ─────────────────────────────────────────────────────────

    /// The client's end of a tail: the response lines it reads.
    type ClientLines =
        tokio::io::Lines<tokio::io::BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>>;

    /// Every response line `buf` holds, parsed.
    fn parse_all(buf: &[u8]) -> Vec<Response> {
        String::from_utf8_lossy(buf)
            .lines()
            .map(|l| serde_json::from_str::<Response>(l).expect("a response line parses"))
            .collect()
    }

    /// The next response the tail wrote to the client, within 10 s.
    async fn next_response(lines: &mut ClientLines) -> Response {
        let line = tokio::time::timeout(std::time::Duration::from_secs(10), lines.next_line())
            .await
            .expect("a response within 10 s")
            .expect("reading the client side")
            .expect("the tail is still writing");
        serde_json::from_str(&line).expect("a response line parses")
    }

    /// Render escape-code bytes on a fresh 24×80 terminal and return the text.
    fn rendered(bytes: &[u8]) -> String {
        let mut p = vt100::Parser::new(24, 80, 0);
        p.process(bytes);
        p.screen().contents()
    }

    /// Run `handle_shell_tail` against `reg` in a task with a duplex standing
    /// in for the client connection. Returns the client's line reader, its
    /// write half (drop it to hang up), and the task.
    fn spawn_tail(
        reg: ShellRegistry,
        id: String,
    ) -> (
        ClientLines,
        tokio::io::WriteHalf<tokio::io::DuplexStream>,
        tokio::task::JoinHandle<anyhow::Result<()>>,
    ) {
        use tokio::io::AsyncBufReadExt;
        let (client, server) = tokio::io::duplex(1 << 20);
        let (server_r, mut server_w) = tokio::io::split(server);
        let task = tokio::spawn(async move {
            let mut rx = tokio::io::BufReader::new(server_r);
            handle_shell_tail(&mut rx, &mut server_w, &reg, &id, true).await
        });
        let (client_r, client_w) = tokio::io::split(client);
        (tokio::io::BufReader::new(client_r).lines(), client_w, task)
    }

    /// A host that predates `SubscribeScreen`: it refuses that frame the way
    /// an older binary does, accepts `Subscribe` and sends one chunk, and
    /// answers `Status` as idle.
    async fn spawn_old_host(id: crate::shell::ShellId) -> anyhow::Result<()> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        let listener = crate::shell::bind(&crate::shell::socket_path(id.as_str())).await?;
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let (r, mut w) = stream.into_split();
                    let mut lines = tokio::io::BufReader::new(r).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        let reply: &[u8] = if line.contains("SubscribeScreen") {
                            b"{\"type\":\"Error\",\"message\":\"malformed frame: unknown variant `SubscribeScreen`\"}\n"
                        } else if line.contains("Subscribe") {
                            b"{\"type\":\"Ok\"}\n{\"type\":\"Chunk\",\"bytes\":[111,108,100]}\n"
                        } else {
                            b"{\"type\":\"Status\",\"state\":\"Idle\",\"rows\":24,\"cols\":80,\"pid\":0}\n"
                        };
                        if w.write_all(reply).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });
        Ok(())
    }

    /// Close an in-process shell through the executor's close path, waiting
    /// up to 3 s for it to end. The last statement of every test that leaves
    /// a shell behind, so the suite never strands one.
    async fn close_tail_shell(reg: &ShellRegistry, id: &str, owner: &Owner) {
        let _ = crate::daemon::executor::knowledge::close_shell_with(
            reg,
            id,
            owner,
            std::time::Duration::from_millis(3000),
        )
        .await;
    }

    #[tokio::test]
    async fn shell_tail_without_follow_sends_only_the_screen() {
        let (_guard, _dir) = shell_test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("t".into());
        let id = crate::shell::spawn_for(&reg, owner.clone(), 5, false, |id| {
            Box::pin(spawn_in_process_host(id))
        })
        .await
        .unwrap();
        crate::shell::send_input(
            &crate::shell::socket_path(id.as_str()),
            b"printf 'marker-%s\\n' tail-a\n",
            RELAY_TIMEOUT,
        )
        .await
        .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut tx: Vec<u8> = Vec::new();
        loop {
            let mut rx = tokio::io::BufReader::new(tokio::io::empty());
            handle_shell_tail(&mut rx, &mut tx, &reg, id.as_str(), false)
                .await
                .unwrap();
            if let [Response::ShellScreen { bytes, .. }] = &parse_all(&tx)[..]
                && rendered(bytes).contains("marker-tail-a")
            {
                break;
            }
            tx.clear();
            assert!(
                std::time::Instant::now() < deadline,
                "no screen with marker-tail-a within 10 s: {:?}",
                parse_all(&tx)
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let frames = parse_all(&tx);
        assert_eq!(
            frames.len(),
            1,
            "without follow exactly one frame is sent, got {frames:?}"
        );
        close_tail_shell(&reg, id.as_str(), &owner).await;
    }

    #[tokio::test]
    async fn shell_tail_reports_the_shells_size() {
        let (_guard, _dir) = shell_test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("t".into());
        let id = crate::shell::spawn_for(&reg, owner.clone(), 5, false, |id| {
            Box::pin(spawn_in_process_host(id))
        })
        .await
        .unwrap();
        crate::shell::send_resize(
            &crate::shell::socket_path(id.as_str()),
            30,
            100,
            RELAY_TIMEOUT,
        )
        .await
        .unwrap();
        let mut rx = tokio::io::BufReader::new(tokio::io::empty());
        let mut tx: Vec<u8> = Vec::new();
        handle_shell_tail(&mut rx, &mut tx, &reg, id.as_str(), false)
            .await
            .unwrap();
        match parse_all(&tx).as_slice() {
            [Response::ShellScreen { rows, cols, .. }] => {
                assert_eq!(
                    (*rows, *cols),
                    (30, 100),
                    "the shell's own size, not a default"
                )
            }
            other => panic!("expected one ShellScreen, got {other:?}"),
        }
        close_tail_shell(&reg, id.as_str(), &owner).await;
    }

    #[tokio::test]
    async fn shell_tail_follow_streams_live_chunks_until_the_client_disconnects() {
        let (_guard, _dir) = shell_test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("t".into());
        let id = crate::shell::spawn_for(&reg, owner.clone(), 5, false, |id| {
            Box::pin(spawn_in_process_host(id))
        })
        .await
        .unwrap();
        let (mut lines, client_w, task) = spawn_tail(reg.clone(), id.as_str().to_string());

        match next_response(&mut lines).await {
            Response::ShellScreen { .. } => {}
            other => panic!("the tail's first frame is the screen, got {other:?}"),
        }

        crate::shell::send_input(
            &crate::shell::socket_path(id.as_str()),
            b"printf 'marker-%s\\n' tail-b\n",
            RELAY_TIMEOUT,
        )
        .await
        .unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut seen = Vec::new();
        loop {
            let bytes = match next_response(&mut lines).await {
                Response::ShellChunk { bytes } => bytes,
                other => panic!("only chunks after the screen, got {other:?}"),
            };
            seen.extend_from_slice(&bytes);
            if String::from_utf8_lossy(&seen).contains("marker-tail-b") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "marker-tail-b never arrived: {:?}",
                String::from_utf8_lossy(&seen)
            );
        }

        drop(client_w);
        drop(lines);
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .expect("the tail ends within 5 s of the client hanging up");
        assert!(
            matches!(&outcome, Ok(Ok(()))),
            "the tail returns Ok after a hang-up, got {outcome:?}"
        );
        close_tail_shell(&reg, id.as_str(), &owner).await;
    }

    /// A client connection that takes the first frame and then hangs up:
    /// every later write fails with `BrokenPipe`.
    struct HangsUpAfterFirstFrame {
        done: bool,
    }

    impl tokio::io::AsyncWrite for HangsUpAfterFirstFrame {
        fn poll_write(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buf: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            if self.done {
                return std::task::Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
            }
            if buf.contains(&b'\n') {
                self.done = true;
            }
            std::task::Poll::Ready(Ok(buf.len()))
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn shell_tail_ends_quietly_when_the_client_hangs_up_mid_stream() {
        let (_guard, _dir) = shell_test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("t".into());
        let id = crate::shell::spawn_for(&reg, owner.clone(), 5, false, |id| {
            Box::pin(spawn_in_process_host(id))
        })
        .await
        .unwrap();
        // The client's reading side never reaches EOF (its other end is kept
        // open), so the tail can only learn of the hang-up from a failed write.
        let (_keep_open, client_side) = tokio::io::duplex(64);
        let task_reg = reg.clone();
        let task_id = id.as_str().to_string();
        let task = tokio::spawn(async move {
            let mut rx = tokio::io::BufReader::new(client_side);
            let mut tx = HangsUpAfterFirstFrame { done: false };
            handle_shell_tail(&mut rx, &mut tx, &task_reg, &task_id, true).await
        });
        // Give the tail time to send its screen, then make the shell print.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        crate::shell::send_input(
            &crate::shell::socket_path(id.as_str()),
            b"printf 'marker-%s\\n' tail-c\n",
            RELAY_TIMEOUT,
        )
        .await
        .unwrap();
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .expect("the tail ends within 5 s of the failed write");
        assert!(
            matches!(&outcome, Ok(Ok(()))),
            "a client that hung up ends the tail without an error, got {outcome:?}"
        );
        close_tail_shell(&reg, id.as_str(), &owner).await;
    }

    #[tokio::test]
    async fn shell_tail_reports_exited_when_the_shell_ends() {
        let (_guard, _dir) = shell_test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("t".into());
        let id = crate::shell::spawn_for(&reg, owner.clone(), 5, false, |id| {
            Box::pin(spawn_in_process_host(id))
        })
        .await
        .unwrap();
        let (mut lines, _client_w, task) = spawn_tail(reg.clone(), id.as_str().to_string());

        match next_response(&mut lines).await {
            Response::ShellScreen { .. } => {}
            other => panic!("the tail's first frame is the screen, got {other:?}"),
        }

        crate::shell::send_input(
            &crate::shell::socket_path(id.as_str()),
            b"exit\n",
            RELAY_TIMEOUT,
        )
        .await
        .unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match next_response(&mut lines).await {
                Response::ShellChunk { .. } => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "ShellExited never arrived within 10 s"
                    );
                }
                Response::ShellExited => break,
                other => panic!("the tail relays chunks then ShellExited, got {other:?}"),
            }
        }

        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .expect("the tail ends within 5 s of the shell exiting");
        assert!(
            matches!(&outcome, Ok(Ok(()))),
            "the tail returns Ok after the shell ends, got {outcome:?}"
        );
    }

    #[tokio::test]
    async fn shell_tail_falls_back_to_subscribe_on_an_old_host() {
        let (_guard, _dir) = shell_test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("t".into());
        let id = crate::shell::spawn_for(&reg, owner.clone(), 5, false, |id| {
            Box::pin(spawn_old_host(id))
        })
        .await
        .unwrap();
        let (mut lines, client_w, task) = spawn_tail(reg.clone(), id.as_str().to_string());

        match next_response(&mut lines).await {
            Response::ShellScreen { bytes, .. } => assert!(
                bytes.is_empty(),
                "an old host's screen is empty, got {} bytes",
                bytes.len()
            ),
            other => panic!("the tail's first frame is the screen, got {other:?}"),
        }
        match next_response(&mut lines).await {
            Response::ShellChunk { bytes } => {
                assert_eq!(bytes, b"old", "the old host's one chunk rides through")
            }
            other => panic!("the chunk after the empty screen, got {other:?}"),
        }

        drop(client_w);
        drop(lines);
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .expect("the tail ends within 5 s of the client hanging up");
        assert!(
            matches!(&outcome, Ok(Ok(()))),
            "the tail returns Ok after a hang-up, got {outcome:?}"
        );
        close_tail_shell(&reg, id.as_str(), &owner).await;
    }

    #[tokio::test]
    async fn shell_tail_refuses_an_unknown_shell() {
        let (_guard, _dir) = shell_test_home();
        let reg = ShellRegistry::default();
        let mut rx = tokio::io::BufReader::new(tokio::io::empty());
        let mut tx: Vec<u8> = Vec::new();
        handle_shell_tail(&mut rx, &mut tx, &reg, "s99", true)
            .await
            .unwrap();
        assert!(
            matches!(parse_response(&tx), Response::Error(ref msg) if msg.contains("s99")),
            "an unknown id is refused with the id named: {tx:?}"
        );
    }
}

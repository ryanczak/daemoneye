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
                dedicated: record.dedicated,
                exited_unix: record.exited_unix,
                last_command: None,
                foreground: shell_foreground(
                    record.last_foreground.as_ref(),
                    record.in_flight.as_ref(),
                ),
                last_line: None,
            })
            .collect()
    });
    infos.sort_by_key(|info| shell_index(&info.id));
    let shells = infos
        .into_iter()
        .map(|info| ShellInfo {
            last_command: last_command(&info.id, info.started_unix),
            last_line: last_line(&info.id, info.started_unix),
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
    if result.is_ok() {
        record_user_input(reg, id, bytes);
    }
    answer_relay(tx, id, result).await
}

/// Account for input the user typed into shell `id` while attached: a
/// `shell_input` event in `events.jsonl`, and a note for the run that owns
/// the shell, which it is told at its next turn boundary. Only the byte
/// count is recorded, never the bytes: a password typed at a prompt must not
/// reach either record (the cast log's own scrub covers the `"i"` events).
fn record_user_input(reg: &ShellRegistry, id: &str, bytes: &[u8]) {
    let owner_key = with_shells(reg, |store| store.get(id).map(|record| record.owner.key()));
    crate::daemon::utils::log_event(
        "shell_input",
        serde_json::json!({
            "shell": id,
            "owner": owner_key,
            "bytes": bytes.len(),
            "source": "user_attached",
        }),
    );
    if let Some(key) = owner_key.as_deref() {
        crate::daemon::run::note_owner_typed(key, id, bytes.len());
    }
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

/// Open a shell owned by the user (`/shell [host]`): the same path the
/// model's `open_shell` takes, with `Owner::User` as the owner. Answers with
/// the new shell's id, or the refusal (an unknown host, the per-owner cap).
pub(super) async fn handle_shell_open<W>(tx: &mut W, host: Option<&str>) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    let response = match crate::daemon::executor::knowledge::open_shell_id(
        &crate::shell::Owner::User,
        host,
        None,
        None,
    )
    .await
    {
        Ok((id, _, _)) => Response::ShellOpened {
            id: id.as_str().to_string(),
        },
        Err(msg) => Response::Error(msg.trim_start_matches("Error: ").to_string()),
    };
    send_response_split(tx, response).await?;
    Ok(())
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
    let owner_key = with_shells(reg, |store| store.get(id).map(|record| record.owner.key()));
    // `Ctrl-c` while attached also cancels the run that owns the shell. The
    // cancel is cooperative (the run stops at its next turn boundary), so it
    // is fired first; the signal then ends what the shell is running now.
    let cancelled_run = if sig == ShellSignal::Int {
        owner_key
            .as_deref()
            .and_then(|key| {
                crate::daemon::run::cancel_owner_run(
                    key,
                    crate::daemon::run::CancelReason::UserStop,
                )
            })
            .map(|run| run.as_str().to_string())
    } else {
        None
    };
    let sock = crate::shell::socket_path(id);
    if let Err(e) = crate::shell::send_signal(&sock, sig, RELAY_TIMEOUT).await {
        send_response_split(tx, Response::Error(format!("shell {id}: {e:#}"))).await?;
        return Ok(());
    }
    let response = match settled_state(&sock, sig).await {
        Ok(state) => {
            if let Some(key) = owner_key.as_deref() {
                record_pause(key, id, sig, state);
            }
            Response::ShellSignalled {
                state: state_name(state).to_string(),
                cancelled_run,
            }
        }
        Err(e) => Response::Error(format!("shell {id}: {e:#}")),
    };
    send_response_split(tx, response).await?;
    Ok(())
}

/// Tell the run that owns shell `id` (owner `owner_key`) about a pause from
/// attached mode: a `STOP` that really stopped the job parks the run at its
/// next turn boundary, and a `CONT` wakes it.
fn record_pause(
    owner_key: &str,
    id: &str,
    sig: crate::shell::ShellSignal,
    state: crate::shell::ShellState,
) {
    use crate::shell::{ShellSignal, ShellState};
    match sig {
        ShellSignal::Stop if state == ShellState::Paused => {
            crate::daemon::run::pause_owner_run(owner_key, id);
        }
        ShellSignal::Cont => {
            crate::daemon::run::resume_owner_run(owner_key);
        }
        _ => {}
    }
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

/// What holds `id`'s shell's foreground, as the daemon last observed it: the
/// `Display` of the record's `last_foreground`, or `running <cmd>` when an
/// in-flight command started later than that observation — the in-flight
/// command is newer news. `None` when the record holds neither. Pure.
fn shell_foreground(
    last_foreground: Option<&(crate::shell::Foreground, u64)>,
    in_flight: Option<&crate::shell::InFlight>,
) -> Option<String> {
    match (last_foreground, in_flight) {
        (Some((fg, at)), Some(in_flight)) if in_flight.started_unix > *at => {
            Some(format!("running {}", in_flight.cmd))
        }
        (Some((fg, _)), _) => Some(fg.to_string()),
        (None, Some(in_flight)) => Some(format!("running {}", in_flight.cmd)),
        (None, None) => None,
    }
}

/// The last readable line of `id`'s latest recorded command output, or `None`
/// when the record is absent, unreadable or has no commands yet. Best-effort
/// by design: the listing still answers when one shell's log is unreadable.
fn last_line(id: &str, started_unix: u64) -> Option<String> {
    let cast = crate::shell::cast_path(id, started_unix);
    let path = meta_path_for(&cast);
    crate::shell::MetaIndex::load(&path).ok().and_then(|meta| {
        let record = meta.commands.last()?;
        crate::shell::read_command_output(&cast, &meta, record.index)
            .ok()
            .and_then(|bytes| last_visible_line(&bytes))
    })
}

/// The last non-empty line of raw PTY output, made readable: escape sequences
/// gone, carriage-return overwrites resolved, secrets masked, cut to 120
/// characters. Empty or all-blank input gives `None`. Pure.
fn last_visible_line(raw: &[u8]) -> Option<String> {
    let text = crate::daemon::executor::knowledge::visible_text(&String::from_utf8_lossy(raw));
    let line = text
        .lines()
        .rfind(|l| !l.trim().is_empty())
        .map(str::trim)?;
    if line.is_empty() {
        return None;
    }
    let masked = crate::ai::filter::mask_sensitive(line);
    Some(masked.chars().take(LAST_LINE_CAP).collect())
}

/// The most characters a `last_line` row shows.
const LAST_LINE_CAP: usize = 120;

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
        _ => crate::shell::spawn_size(),
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
#[path = "shells_tests.rs"]
mod tests;

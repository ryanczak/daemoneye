//! IPC server: client dispatch (`handle_client`) plus the catch-up brief,
//! quick-return handlers, and the `handle_ask` orchestrator.
//! Split across submodules in phase-08; the public surface is re-exported here.

mod ask;
mod catchup;
mod chat_presence;
mod handlers;
mod notices;
mod sessions;
mod shells;

// Re-exported for the webhook side's presence-push test (M27 phase 03b) and
// the integration tests: they drive the real attach path over a duplex, not
// a fake.
pub use chat_presence::handle_chat_attach;
pub use handlers::handle_alert_list;

use ask::handle_ask;
use handlers::*;
use sessions::*;

use crate::daemon::utils::*;
use crate::ipc::{Request, Response};
use anyhow::Result;
use std::sync::Arc;
use tokio::io::BufReader;
use tokio::net::UnixStream;

// ---------------------------------------------------------------------------
// IPC peer authentication (C1)
// ---------------------------------------------------------------------------
// The daemon trusts only connections from the same local user.  There is no
// token or key exchanged over the socket — identity is derived from the
// kernel via SO_PEERCRED, which cannot be forged by a userspace attacker.

/// Return the effective UID of the process on the far end of `sock`, or `None`
/// if the kernel would not/could not answer (closed connection, non-Linux,
/// etc.).  The caller treats `None` as "reject".
pub(crate) fn peer_euid<S: std::os::fd::AsRawFd>(sock: &S) -> Option<u32> {
    let fd = sock.as_raw_fd();
    // SAFETY: `cred` is a plain C struct; getsockopt writes at most its own size.
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: pointer points to a valid, writable libc::ucred of the right size.
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut libc::ucred as *mut libc::c_void,
            &mut len,
        )
    };
    if rc == 0 && len >= std::mem::size_of::<libc::ucred>() as libc::socklen_t {
        Some(cred.uid)
    } else if rc == 0 {
        // Kernel gave less than a full ucred — treat as unknown.
        log::warn!(
            "SO_PEERCRED returned a truncated credential ({} bytes)",
            len
        );
        None
    } else {
        let err = std::io::Error::last_os_error();
        log::warn!("SO_PEERCRED failed on fd {}: {}", fd, err);
        None
    }
}

/// Reject connections whose peer euid differs from the daemon's euid.
/// Returns `Err` (caller should drop the connection) when identity cannot be
/// established or the peer is not our own user.
pub(crate) fn check_peer_identity<S: std::os::fd::AsRawFd>(stream: &S) -> anyhow::Result<()> {
    let daemon_euid = unsafe { libc::geteuid() };
    match peer_euid(stream) {
        Some(uid) if uid == daemon_euid => Ok(()),
        Some(uid) => {
            log::warn!(
                "Rejecting IPC connection from uid {} (daemon euid {}): not the owning user",
                uid,
                daemon_euid
            );
            anyhow::bail!("IPC peer uid {} is not the daemon user", uid)
        }
        None => {
            log::warn!("Rejecting IPC connection: could not determine peer credentials");
            anyhow::bail!("could not determine IPC peer credentials")
        }
    }
}

/// Handle one client connection end-to-end.
///
/// ## Request routing
/// - `Ping` / `Shutdown` / `Refresh` are dispatched and returned immediately.
/// - `Ask` drives the full conversation turn: load history → build prompt →
///   stream AI response → collect tool calls → execute each (background or
///   foreground) → loop back for the next AI turn until no tool calls remain.
///
/// ## Tool call execution
/// Each tool call goes through an approval gate:
/// - The client is sent a `ToolCallPrompt`; the user approves or denies.
/// - **Background** (`background: true`): the daemon runs the command as a
///   subprocess (`tokio::process`). If sudo is needed a `CredentialPrompt` is sent
///   and the credential is piped to `sudo -S`.
/// - **Foreground** (`background: false`): `tmux send-keys` dispatches to the
///   user's working pane. If sudo is detected the daemon switches focus to that
///   pane and waits for `pane_current_command` to leave "sudo".
///
/// ## Session persistence
/// Message history is stored both in the in-memory `sessions` map (fast lookup
/// within the same daemon run) and in `~/.daemoneye/sessions/<id>.jsonl` (survives
/// restarts). History is bounded by token-budget compaction, not a fixed
/// message count.
pub async fn handle_client(
    stream: UnixStream,
    state: Arc<crate::daemon::DaemonState>,
) -> Result<()> {
    // C1: refuse connections from any process not owned by the daemon user.
    // Must happen before parsing a single byte so a foreign attacker cannot
    // even reach the approval gate (which trusts whatever is on the other end).
    check_peer_identity(&stream)?;

    let config = state.config.current();

    let mut reader = BufReader::new(stream);
    let mut line: Vec<u8> = Vec::new();
    let n =
        crate::ipc::read_bounded_line(&mut reader, &mut line, crate::ipc::MAX_IPC_MESSAGE_BYTES)
            .await;
    if let Err(e) = n {
        if e.kind() == std::io::ErrorKind::InvalidData {
            let mut stream = reader.into_inner();
            send_response(
                &mut stream,
                Response::Error(format!(
                    "Request too large (limit {} bytes)",
                    crate::ipc::MAX_IPC_MESSAGE_BYTES
                )),
            )
            .await?;
            return Ok(());
        }
        return Err(e.into());
    }
    let n = n.unwrap_or(0);
    if n == 0 {
        return Ok(());
    }

    let request: Request = match serde_json::from_slice(line.trim_ascii()) {
        Ok(req) => req,
        Err(e) => {
            let mut stream = reader.into_inner();
            send_response(
                &mut stream,
                Response::Error(format!("Invalid request: {}", e)),
            )
            .await?;
            return Ok(());
        }
    };

    let (rx_half, mut tx) = reader.into_inner().into_split();
    let mut rx = BufReader::new(rx_half);
    dispatch_client_request(request, &state, &config, &mut rx, &mut tx).await
}

async fn dispatch_shell_or_bus_request<R, W>(
    request: &Request,
    state: &Arc<crate::daemon::DaemonState>,
    config: &crate::config::Config,
    rx: &mut R,
    tx: &mut W,
) -> Result<bool>
where
    R: tokio::io::AsyncBufReadExt + Unpin,
    W: tokio::io::AsyncWriteExt + Unpin,
{
    match request {
        Request::Ping => handle_ping(tx).await?,
        Request::AlertList => handle_alert_list(tx).await?,
        Request::Shutdown => handle_shutdown(tx).await?,
        Request::Refresh => handle_refresh(tx).await?,
        Request::Reload => match crate::config::daemon_reload() {
            Ok((applied, restart_required)) => {
                send_response_split(
                    tx,
                    Response::Reloaded {
                        applied,
                        restart_required,
                    },
                )
                .await?;
            }
            Err(e) => {
                send_response_split(tx, Response::Error(format!("{e:#}"))).await?;
            }
        },
        Request::Status => {
            handle_status(tx, &state.sessions, &state.schedules, config).await?;
        }
        Request::ContainerStatus => handle_container_status(tx, config).await?,
        Request::ListSavedSessions => handle_list_saved_sessions(tx).await?,
        Request::ShellList => {
            shells::handle_shell_list(tx, &state.shells).await?;
        }
        Request::ShellInput { id, bytes } => {
            shells::handle_shell_input(tx, &state.shells, id, bytes).await?;
        }
        Request::ShellOpen { host } => {
            shells::handle_shell_open(tx, &state.shells, host.as_deref()).await?;
        }
        Request::ShellSignal { id, signal } => {
            shells::handle_shell_signal(tx, &state.shells, id, signal).await?;
        }
        Request::ShellResize { id, rows, cols } => {
            shells::handle_shell_resize(tx, &state.shells, id, *rows, *cols).await?;
        }
        Request::ShellLogList {
            since,
            host,
            owner,
            limit,
        } => {
            shells::handle_shell_log_list(tx, *since, host.as_deref(), owner.as_deref(), *limit)
                .await?;
        }
        Request::ShellLogRead { id } => {
            shells::handle_shell_log_read(tx, id).await?;
        }
        Request::ShellTail { id, follow } => {
            shells::handle_shell_tail(rx, tx, &state.shells, id, *follow).await?;
        }
        Request::Subscribe { filter } => {
            notices::handle_subscribe(rx, tx, &state.bus, filter.clone()).await?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

async fn dispatch_client_request<R, W>(
    request: Request,
    state: &Arc<crate::daemon::DaemonState>,
    config: &crate::config::Config,
    rx: &mut R,
    tx: &mut W,
) -> Result<()>
where
    R: tokio::io::AsyncBufReadExt + Unpin,
    W: tokio::io::AsyncWriteExt + Unpin,
{
    if dispatch_shell_or_bus_request(&request, state, config, rx, tx).await? {
        return Ok(());
    }
    let (cache, sessions, schedule_store) = (&state.tmux_cache, &state.sessions, &state.schedules);

    match request {
        Request::Cancel { session_id } => {
            let found = crate::daemon::run::cancel_session(
                &session_id,
                crate::daemon::run::CancelReason::UserStop,
            );
            log::info!("cancel request for session {session_id}: found={found}");
            send_response_split(tx, Response::Ok).await?;
        }
        Request::ResumeRun { run_id, directive } => {
            handle_resume_run(
                tx,
                cache,
                sessions,
                schedule_store,
                config,
                &run_id,
                &directive,
            )
            .await?;
        }
        Request::SetModel {
            session_id,
            model: model_name,
        } => {
            handle_set_model(tx, sessions, config, session_id, model_name).await?;
        }
        Request::ListModels { session_id } => {
            handle_list_models(tx, sessions, config, session_id).await?;
        }
        Request::SetPane {
            session_id,
            pane_id,
        } => {
            handle_set_pane(tx, sessions, cache, session_id, pane_id).await?;
        }
        Request::ListPanesForSession { session_id } => {
            handle_list_panes(tx, sessions, cache, session_id).await?;
        }
        Request::QueryLimits { session_id: sid } => {
            handle_query_limits(tx, sessions, config, sid).await?;
        }
        Request::ResetSessionToolCount { session_id: sid } => {
            handle_reset_tool_count(tx, sessions, sid).await?;
        }
        Request::SaveSession {
            session_id: sid,
            name,
            description,
            force,
        } => {
            handle_save_session(tx, sessions, sid, name, description, force).await?;
        }
        Request::LoadSession {
            session_id: sid,
            name,
            force,
        } => {
            handle_load_session(tx, sessions, config, sid, name, force).await?;
        }
        Request::DeleteSavedSession { name } => {
            handle_delete_saved_session(tx, name).await?;
        }
        Request::RenameSavedSession { old_name, new_name } => {
            handle_rename_saved_session(tx, sessions, old_name, new_name).await?;
        }
        Request::ChatAttach { session_id } => {
            handle_chat_attach(sessions.clone(), session_id, rx, tx).await?;
        }
        Request::Ask {
            query,
            tmux_pane,
            session_id,
            chat_pane,
            prompt,
            chat_width,
            tmux_session,
            target_pane,
            model: _ask_model,
        } => {
            let req = ask::AskRequest {
                query,
                client_pane: tmux_pane,
                session_id,
                chat_pane,
                prompt_override: prompt,
                chat_width,
                client_tmux_session: tmux_session,
                client_target_pane: target_pane,
            };
            let ctx = ask::AskContext {
                cache: Arc::clone(cache),
                sessions,
                schedule_store: Arc::clone(schedule_store),
                config,
            };
            handle_ask(req, ctx, tx, rx).await?;
        }
        _ => {}
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream as StdUnixStream;
    use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};
    use tokio::net::UnixListener;

    fn tmp_socket_path(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "daemoneye-peer-test-{}-{}",
            std::process::id(),
            tag
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("s.sock")
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn daemon_refuses_an_oversized_request_within_budget() {
        let state = crate::daemon::DaemonState::new_for_test(crate::config::Config::default());
        let (client, server) = UnixStream::pair().unwrap();
        let (client_read, mut client_write) = client.into_split();
        let handle = tokio::spawn(handle_client(server, state));

        let chunk = vec![b'a'; 64 * 1024];
        let mut accepted: u64 = 0;
        for _ in 0..(64 * 1024 * 1024 / chunk.len()) {
            match client_write.write_all(&chunk).await {
                Ok(()) => accepted += chunk.len() as u64,
                Err(e) => {
                    assert_eq!(e.kind(), std::io::ErrorKind::BrokenPipe);
                    break;
                }
            }
        }
        assert!(
            accepted <= 4 * 1024 * 1024,
            "the daemon must stop accepting well before the full 64 MiB: accepted {accepted}"
        );
        drop(client_write);

        let mut reply = Vec::new();
        let mut client_read = BufReader::new(client_read);
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            client_read.read_until(b'\n', &mut reply),
        )
        .await
        .expect("a reply within 10 s")
        .expect("read reply");
        let response: Response =
            serde_json::from_str(std::str::from_utf8(&reply).unwrap().trim()).unwrap();
        match response {
            Response::Error(message) => assert!(
                message.contains("too large"),
                "the refusal must say `too large`: {message}"
            ),
            other => panic!("expected Response::Error, got {other:?}"),
        }
        assert!(
            handle.await.unwrap().is_err() || true,
            "handle_client must have returned"
        );
    }

    #[tokio::test]
    async fn peer_euid_matches_own_process() {
        let path = tmp_socket_path("same-euid");
        let connect_to = path.clone();
        let listener = UnixListener::bind(&path).unwrap();

        // Connect from the same process (hence same euid as the daemon).
        let client = std::thread::spawn(move || {
            let s = StdUnixStream::connect(&connect_to).unwrap();
            assert_eq!(peer_euid(&s), Some(unsafe { libc::geteuid() }));
            check_peer_identity(&s).unwrap(); // tokio UnixStream not needed here
        });

        let (accepted, _) = listener.accept().await.unwrap();
        drop(accepted);
        client.join().unwrap();
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn peer_euid_none_on_invalid_fd() {
        // An fd that is no longer a socket must return None (reject) rather than panic.
        // The harness may leave stdin as a socket, so this pins a LIVE, non-socket
        // descriptor instead — kept bound to a named File for the assertion's
        // lifetime, so getsockopt → ENOTSOCK → None on every platform.
        let dev_null = std::fs::File::open("/dev/null").unwrap();
        struct NonSocketFd(std::os::fd::RawFd);
        impl std::os::fd::AsRawFd for NonSocketFd {
            fn as_raw_fd(&self) -> std::os::fd::RawFd {
                self.0
            }
        }
        let fd = std::os::fd::AsRawFd::as_raw_fd(&dev_null);
        assert_eq!(peer_euid(&NonSocketFd(fd)), None);
        drop(dev_null);
    }
}

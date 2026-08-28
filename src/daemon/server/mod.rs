//! IPC server: client dispatch (`handle_client`) plus the catch-up brief,
//! quick-return handlers, and the `handle_ask` orchestrator.
//! Split across submodules in phase-08; the public surface is re-exported here.

mod ask;
mod catchup;
mod handlers;

pub(crate) use catchup::is_valid_pane_id;

use ask::handle_ask;
use handlers::*;

use crate::config::Config;
use crate::daemon::session::*;
use crate::daemon::utils::*;
use crate::ipc::{Request, Response};
use crate::scheduler::ScheduleStore;
use crate::tmux::cache::SessionCache;
use anyhow::Result;
use std::sync::Arc;
use tokio::io::AsyncBufReadExt;
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
fn peer_euid<S: std::os::fd::AsRawFd>(sock: &S) -> Option<u32> {
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
fn check_peer_identity<S: std::os::fd::AsRawFd>(stream: &S) -> anyhow::Result<()> {
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
    cache: Arc<SessionCache>,
    sessions: SessionStore,
    schedule_store: Arc<ScheduleStore>,
    bg_session: Arc<std::sync::Mutex<String>>,
    managed_session: Arc<Option<String>>,
) -> Result<()> {
    // C1: refuse connections from any process not owned by the daemon user.
    // Must happen before parsing a single byte so a foreign attacker cannot
    // even reach the approval gate (which trusts whatever is on the other end).
    check_peer_identity(&stream)?;

    let config = Config::load().unwrap_or_else(|_| {
        log::warn!("Failed to load config, using defaults");
        Config::default()
    });

    /// Maximum size of a single incoming IPC message (1 MiB).
    /// Prevents a malicious or buggy client from exhausting daemon memory by
    /// sending an arbitrarily large JSON payload without a newline.
    const MAX_IPC_MESSAGE_BYTES: usize = 1 << 20;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let n = reader.read_line(&mut line).await?;
    if n == 0 {
        return Ok(());
    }
    if line.len() > MAX_IPC_MESSAGE_BYTES {
        let mut stream = reader.into_inner();
        send_response(
            &mut stream,
            Response::Error(format!(
                "Request too large ({} bytes; limit {} bytes)",
                line.len(),
                MAX_IPC_MESSAGE_BYTES
            )),
        )
        .await?;
        return Ok(());
    }

    let request: Request = match serde_json::from_str(line.trim()) {
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

    match request {
        Request::Ping => {
            handle_ping(&mut tx).await?;
        }
        Request::Cancel { session_id } => {
            let found = crate::daemon::cancel::cancel_turn(&session_id);
            log::info!("cancel request for session {session_id}: found={found}");
            send_response_split(&mut tx, Response::Ok).await?;
        }
        Request::Shutdown => {
            handle_shutdown(&mut tx).await?;
            return Ok(());
        }
        Request::Refresh => {
            handle_refresh(&mut tx).await?;
        }
        Request::SetModel {
            session_id,
            model: model_name,
        } => {
            handle_set_model(&mut tx, &sessions, &config, session_id, model_name).await?;
        }
        Request::ListModels { session_id } => {
            handle_list_models(&mut tx, &sessions, &config, session_id).await?;
        }
        Request::SetPane {
            session_id,
            pane_id,
        } => {
            handle_set_pane(&mut tx, &sessions, &cache, session_id, pane_id).await?;
        }
        Request::ListPanesForSession { session_id } => {
            handle_list_panes(&mut tx, &sessions, &cache, session_id).await?;
        }
        Request::Status => {
            handle_status(&mut tx, &sessions, &schedule_store, &config).await?;
        }
        Request::QueryLimits { session_id: sid } => {
            handle_query_limits(&mut tx, &sessions, &config, sid).await?;
        }
        Request::ResetSessionToolCount { session_id: sid } => {
            handle_reset_tool_count(&mut tx, &sessions, sid).await?;
        }
        Request::SaveSession {
            session_id: sid,
            name,
            description,
            force,
        } => {
            handle_save_session(&mut tx, &sessions, sid, name, description, force).await?;
        }
        Request::LoadSession {
            session_id: sid,
            name,
            force,
        } => {
            handle_load_session(&mut tx, &sessions, &config, sid, name, force).await?;
        }
        Request::ListSavedSessions => {
            handle_list_saved_sessions(&mut tx).await?;
        }
        Request::DeleteSavedSession { name } => {
            handle_delete_saved_session(&mut tx, name).await?;
        }
        Request::RenameSavedSession { old_name, new_name } => {
            handle_rename_saved_session(&mut tx, &sessions, old_name, new_name).await?;
        }

        Request::NotifyActivity { pane_id, .. } => {
            crate::daemon::hook::handle_notify_activity(&mut tx, &pane_id).await?;
        }
        Request::NotifyComplete {
            pane_id, exit_code, ..
        } => {
            crate::daemon::hook::handle_notify_complete(&mut tx, &pane_id, exit_code).await?;
        }
        Request::NotifyFocus { pane_id, .. } => {
            crate::daemon::hook::handle_notify_focus(&cache, &mut tx, &pane_id).await?;
        }
        Request::NotifyWindowChanged { .. } => {
            crate::daemon::hook::handle_notify_window_changed(&cache, &mut tx).await?;
        }
        Request::NotifySessionClosed { session_name } => {
            crate::daemon::hook::handle_notify_session_closed(
                sessions.clone(),
                Arc::clone(&cache),
                Arc::clone(&managed_session),
                Arc::clone(&bg_session),
                &mut tx,
                session_name,
            )
            .await?;
        }
        Request::NotifySessionCreated { session_name } => {
            crate::daemon::hook::handle_notify_session_created(&mut tx, session_name).await?;
        }
        Request::NotifyClientDetached { session_name } => {
            crate::daemon::hook::handle_notify_client_detached(
                sessions.clone(),
                &mut tx,
                session_name,
            )
            .await?;
        }
        Request::NotifyClientAttached { session_name } => {
            crate::daemon::hook::handle_notify_client_attached(
                sessions.clone(),
                &mut tx,
                session_name,
            )
            .await?;
        }
        Request::NotifyResize { width, height, .. } => {
            crate::daemon::hook::handle_notify_resize(&cache, &mut tx, width, height).await?;
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
                cache,
                sessions: &sessions,
                schedule_store,
                bg_session,
                config: &config,
            };
            handle_ask(req, ctx, &mut tx, &mut rx).await?;
            return Ok(());
        }
        _ => {}
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream as StdUnixStream;
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
        // The harness may leave stdin as a socket (or reuse its number during
        // the full suite), so this pins a deterministically closed fd number
        // instead: getsockopt → EBADF → None on every platform.
        let fd = std::os::fd::AsRawFd::as_raw_fd(&std::fs::File::open("/dev/null").unwrap()); // closes on drop
        struct ClosedFd(std::os::fd::RawFd);
        impl std::os::fd::AsRawFd for ClosedFd {
            fn as_raw_fd(&self) -> std::os::fd::RawFd {
                self.0
            }
        }
        assert_eq!(peer_euid(&ClosedFd(fd)), None);
    }
}

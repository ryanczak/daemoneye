//! Chat presence (M27 phase 03): `ChatAttach` holds one connection per chat
//! client for the client's lifetime. Detach state is stamped when the last
//! connection for a session closes; a later attach ≥ 30 s later receives the
//! catch-up brief in the attach reply.

use super::catchup::build_catchup_brief;
use crate::daemon::session::{
    PresenceConnection, SessionEntry, SessionStore, presence_register, presence_unregister,
    record_chat_detach, with_sessions,
};
use crate::daemon::utils::send_response_split;
use crate::ipc::Response;
use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

/// Compute the catch-up brief for a session id at attach time, then clear the
/// session's detach state.
///
/// Mirrors the `Ask`-time brief it replaces (M27 phase 03): when the entry
/// has `last_detach`, the away time is measured from it and the brief is
/// built over the messages added since `messages_at_detach`. The detach
/// state is cleared whether or not a brief was produced. A session with no
/// entry yields no brief — presence alone carries the attach. Since M29
/// phase 05 an attach inserts the entry from disk first, so a session that
/// detached before a daemon restart re-attaches resident and reads the same
/// branch.
pub(crate) fn attach_brief_for(sessions: &SessionStore, session_id: &str) -> Option<String> {
    with_sessions(sessions, |store| {
        let entry = store.get_mut(session_id)?;
        let brief = entry.last_detach.and_then(|detach| {
            let away_secs = detach.elapsed().as_secs();
            let new_msgs = &entry.messages[entry.messages_at_detach.min(entry.messages.len())..];
            build_catchup_brief(new_msgs, away_secs, entry.detach_time_utc)
        });
        entry.last_detach = None;
        entry.detach_time_utc = None;
        entry.messages_at_detach = 0;
        brief
    })
}

/// Handle `ChatAttach { session_id }`.
///
/// Registers the connection as present for the session, replies with the
/// catch-up brief (zero or one `SystemMsg`) and then `Ok`, then holds the
/// connection: reads until EOF or error, ignoring whatever the client sends.
/// Every way the hold ends — clean EOF, a read error, or a failed reply
/// write — unregisters the connection exactly once, and a last-close stamps
/// detach state on the entry (if it exists) so a later re-attach can be
/// briefed. The step's result is returned last.
pub(crate) async fn handle_chat_attach<R, W>(
    sessions: SessionStore,
    session_id: String,
    rx: &mut R,
    tx: &mut W,
) -> Result<()>
where
    R: AsyncBufReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    let (push_tx, mut push_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let conn = PresenceConnection { push: push_tx };
    presence_register(&session_id, conn.clone());
    // The attach makes the session resident (M29 phase 05): load its disk
    // history and meta into the store now, so injection, the catch-up brief and
    // `handle_ask` all serve one entry. `from_disk` reads files, so it runs
    // before the lock: probe with `contains_key` (no I/O), build outside
    // `with_sessions`, then insert — keeping an entry another task created in
    // between.
    let absent = !with_sessions(&sessions, |store| store.contains_key(&session_id));
    let built = if absent {
        Some(SessionEntry::from_disk(&session_id))
    } else {
        None
    };
    with_sessions(&sessions, |store| {
        if let Some(built) = built {
            store.entry(session_id.clone()).or_insert(built);
        }
    });
    log::info!("chat presence attached for session {session_id}");

    // One fallible step: reply, then hold until EOF or error. Nothing after
    // `presence_register` may skip the unregister, so this runs to completion
    // (either way) before the bookkeeping below.
    let held = async {
        if let Some(brief) = attach_brief_for(&sessions, &session_id) {
            send_response_split(tx, Response::SystemMsg(brief)).await?;
        }
        send_response_split(tx, Response::Ok).await?;

        // Hold the connection for the client's lifetime. Any bytes the client
        // sends are read and discarded — the channel carries daemon→client
        // traffic; the client talks over its other connections. Pushes queued
        // by `presence_broadcast` are written ahead of the reads, with the
        // read parked until one arrives so a quiet connection costs nothing.
        // EOF, a read error, or a failed write (reply or push) all end the
        // hold.
        let mut discarded = String::new();
        loop {
            let line = tokio::select! {
                n = rx.read_line(&mut discarded) => {
                    // EOF or a read error both end the hold; `?` propagates
                    // the error, EOF returns Ok.
                    if n? == 0 {
                        return Ok(());
                    }
                    None
                }
                Some(msg) = push_rx.recv() => Some(msg),
            };
            let Some(msg) = line else { continue };
            send_response_split(tx, Response::SystemMsg(msg)).await?;
            discarded.clear();
        }
    };
    let result = held.await;

    let was_last = presence_unregister(&session_id, &conn);
    log::info!("chat presence detached for session {session_id} (last={was_last})");
    if was_last {
        record_chat_detach(&session_id, &sessions);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::Message;
    use crate::daemon::session::{
        SessionEntry, presence_broadcast, presence_broadcast_in, presence_count,
    };
    use std::collections::HashMap;
    use std::collections::HashSet;
    use std::sync::Mutex;
    use std::time::Instant;
    use tokio::io::AsyncWriteExt;

    fn entry(messages: Vec<Message>) -> SessionEntry {
        SessionEntry {
            messages,
            last_accessed: Instant::now(),
            chat_pane: None,
            default_target_pane: None,
            bg_windows: Vec::new(),
            last_prompt_tokens: 0,
            tmux_session: "test".to_string(),
            last_detach: None,
            detach_time_utc: None,
            messages_at_detach: 0,
            is_ghost: false,
            ghost_config: None,
            ghost_bg_prefix: "",
            ghost_runbook_name: None,
            started_at: chrono::Utc::now(),
            turn_count: 0,
            tool_calls_this_session: 0,
            active_model: None,
            last_snapshot_activity: 0,
            saved_name: None,
            dirty: false,
            artifacts_created: Vec::new(),
            auto_name_suggested: false,
            ghost_task_message: None,
            loaded_tools: HashSet::new(),
            cost_usd: 0.0,
            cost_by_agent: std::collections::HashMap::new(),
            has_untracked_cost: false,
            token_scale: 1.5,
            compaction_in_flight: false,
            pending_compaction_notice: None,
        }
    }

    fn msg(content: &str) -> Message {
        Message {
            role: "system".to_string(),
            content: content.to_string(),
            tool_calls: None,
            tool_results: None,
            turn: None,
        }
    }

    type ClientRx = tokio::io::BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>;
    type ClientTx = tokio::io::WriteHalf<tokio::io::DuplexStream>;

    /// Run `handle_chat_attach` over a duplex, with the daemon side in a task.
    /// Returns the client's reader, writer half and the task handle.
    async fn drive(
        sessions: SessionStore,
        session_id: &str,
    ) -> (ClientRx, ClientTx, tokio::task::JoinHandle<Result<()>>) {
        let (client, server) = tokio::io::duplex(1 << 16);
        let (server_r, server_w) = tokio::io::split(server);
        let sid = session_id.to_string();
        let task = tokio::spawn(async move {
            let mut rx = tokio::io::BufReader::new(server_r);
            let mut tx = server_w;
            handle_chat_attach(sessions, sid, &mut rx, &mut tx).await
        });
        let (client_r, client_w) = tokio::io::split(client);
        (tokio::io::BufReader::new(client_r), client_w, task)
    }

    /// The dead-client shape: drop the vanished client's reader and writer
    /// halves together. The daemon's next read sees EOF and the hold ends
    /// through the same exit path as an I/O error.
    async fn drop_client_entirely(rx: ClientRx, tx: ClientTx) {
        drop(rx);
        drop(tx);
    }

    /// Read exactly `n` response lines from the client side, each within 5 s.
    async fn read_responses(rx: &mut ClientRx, n: usize) -> Vec<Response> {
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let mut line = String::new();
            let read = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                AsyncBufReadExt::read_line(rx, &mut line),
            )
            .await
            .expect("a response line within 5 s")
            .expect("the attach handler is still writing");
            assert!(read > 0, "the attach handler wrote a non-empty line");
            out.push(serde_json::from_str(line.trim()).expect("a response line parses"));
        }
        out
    }

    /// Read response lines until one carries `text` — a concurrent test's
    /// global broadcast can interleave its own SystemMsg on this connection's
    /// stream, so delivery is observed by content, not by count.
    async fn read_until_text(rx: &mut ClientRx, text: &str) -> Vec<Response> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut out = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            assert!(
                !left.is_zero(),
                "the pushed line '{text}' never arrived within 5 s: {out:?}"
            );
            let mut line = String::new();
            let read = tokio::time::timeout(left, AsyncBufReadExt::read_line(rx, &mut line))
                .await
                .expect("a response line within the deadline")
                .expect("the attach handler is still writing");
            assert!(read > 0, "the attach handler wrote a non-empty line");
            let resp: Response = serde_json::from_str(line.trim()).expect("a response line parses");
            let hit = matches!(&resp, Response::SystemMsg(m) if m == text);
            out.push(resp);
            if hit {
                return out;
            }
        }
    }

    /// Drop the client side, then wait for the handler to finish.
    async fn close(client_w: &mut ClientTx, task: tokio::task::JoinHandle<Result<()>>) {
        client_w.shutdown().await.unwrap();
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn pushed_alert_reaches_every_attached_client() {
        let sessions = SessionStore::new();
        let (mut rx1, mut client1, task1) = drive(sessions.clone(), "push-1").await;
        let _ = read_responses(&mut rx1, 1).await;
        let (mut rx2, mut client2, task2) = drive(sessions.clone(), "push-2").await;
        let _ = read_responses(&mut rx2, 1).await;

        // The alert reaches both clients, regardless of any other test's
        // live connections in the process-global registry.
        presence_broadcast("ALERT: disk usage at 95%");
        assert_eq!(presence_count("push-1"), 1);
        assert_eq!(presence_count("push-2"), 1);

        let got1 = read_until_text(&mut rx1, "ALERT: disk usage at 95%").await;
        let got2 = read_until_text(&mut rx2, "ALERT: disk usage at 95%").await;
        // Other tests' global broadcasts can interleave on this stream; the
        // pinned assertion is that this test's own alert line arrived last.
        assert!(
            matches!(got1.last(), Some(Response::SystemMsg(m)) if m == "ALERT: disk usage at 95%"),
            "client 1 receives the alert as a SystemMsg: {got1:?}"
        );
        assert!(
            matches!(got2.last(), Some(Response::SystemMsg(m)) if m == "ALERT: disk usage at 95%"),
            "client 2 receives the alert as a SystemMsg: {got2:?}"
        );

        close(&mut client1, task1).await;
        close(&mut client2, task2).await;
        assert_eq!(presence_count("push-1"), 0);
        assert_eq!(presence_count("push-2"), 0);
    }

    #[tokio::test]
    async fn last_presence_disconnect_records_detach_state() {
        let sessions = SessionStore::new();
        with_sessions(&sessions, |store| {
            store.insert("s1".to_string(), entry(vec![msg("hello")]));
        });
        let (mut rx, mut client_w, task) = drive(sessions.clone(), "s1").await;
        let replies = read_responses(&mut rx, 1).await;
        assert!(
            matches!(replies.as_slice(), [Response::Ok]),
            "no detach state → Ok alone, got {replies:?}"
        );
        close(&mut client_w, task).await;

        with_sessions(&sessions, |store| {
            let e = store.get("s1").unwrap();
            assert!(e.last_detach.is_some(), "the close stamps last_detach");
            assert!(e.detach_time_utc.is_some());
            assert_eq!(
                e.messages_at_detach, 1,
                "messages_at_detach is the message count at close"
            );
        });
        assert_eq!(presence_count("s1"), 0, "presence unregistered on close");
    }

    #[tokio::test]
    async fn push_to_a_dead_client_unregisters_it() {
        let sessions = SessionStore::new();
        let (rx1, _client1, task1) = drive(sessions.clone(), "push-dead").await;
        let (mut rx2, mut client2, task2) = drive(sessions.clone(), "push-dead").await;
        let _ = read_responses(&mut rx2, 1).await;
        assert_eq!(presence_count("push-dead"), 2);

        // The dead client: drop the whole client side (its stream holder) —
        // the daemon's next read sees EOF and the hold ends through the same
        // exit path as an I/O error.
        drop_client_entirely(rx1, _client1).await;
        while presence_count("push-dead") == 2 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert_eq!(
            presence_count("push-dead"),
            1,
            "the dead connection's handler unregistered it"
        );
        let hold_end = task1
            .await
            .expect("the dead client's handler join must not panic");
        let io_only = match &hold_end {
            Ok(()) => true,
            Err(e) => e.downcast_ref::<std::io::Error>().is_some(),
        };
        assert!(
            io_only,
            "the dead client's hold ends Ok (EOF) or with an I/O error; got {hold_end:?}"
        );
        // The dead client's earlier `read_responses` call left the BufReader
        // holding no buffered data, but the duplex may still carry a stray
        // push from a concurrent test; read rx2 only after the broadcast of
        // this test's own text.
        presence_broadcast("ALERT: after one client died");
        // The dead connection may still hold one queued push from before it
        // died; drain until this test's own alert line arrives.
        let got = read_until_text(&mut rx2, "ALERT: after one client died").await;
        assert!(
            matches!(got.last(), Some(Response::SystemMsg(m)) if m == "ALERT: after one client died"),
            "the live client still gets the alert: {got:?}"
        );

        close(&mut client2, task2).await;
        assert_eq!(presence_count("push-dead"), 0);
    }

    #[tokio::test]
    async fn broadcast_with_no_clients_is_a_noop() {
        let map: Mutex<HashMap<String, Vec<PresenceConnection>>> = Mutex::new(HashMap::new());
        let start = std::time::Instant::now();
        let pushed = presence_broadcast_in(&map, "ALERT: nobody listening");
        assert!(
            start.elapsed() < std::time::Duration::from_secs(1),
            "a broadcast with no clients returns promptly: {:?}",
            start.elapsed()
        );
        assert_eq!(pushed, 0, "delivered to nobody");
    }

    #[tokio::test]
    async fn presence_with_two_clients_detaches_only_on_last() {
        let sessions = SessionStore::new();
        with_sessions(&sessions, |store| {
            store.insert("s2".to_string(), entry(vec![msg("hello")]));
        });

        let (mut rx1, mut client1, task1) = drive(sessions.clone(), "s2").await;
        let _ = read_responses(&mut rx1, 1).await;
        let (mut rx2, mut client2, task2) = drive(sessions.clone(), "s2").await;
        let _ = read_responses(&mut rx2, 1).await;
        assert_eq!(presence_count("s2"), 2, "two clients attached");
        close(&mut client1, task1).await;
        with_sessions(&sessions, |store| {
            let e = store.get("s2").unwrap();
            assert!(
                e.last_detach.is_none(),
                "one of two still attached → no detach stamp"
            );
        });

        close(&mut client2, task2).await;
        with_sessions(&sessions, |store| {
            let e = store.get("s2").unwrap();
            assert!(e.last_detach.is_some(), "the last close stamps the detach");
            assert_eq!(e.messages_at_detach, 1);
        });
        assert_eq!(presence_count("s2"), 0);
    }

    #[tokio::test]
    async fn reattach_after_30s_returns_catchup_brief() {
        let sessions = SessionStore::new();
        with_sessions(&sessions, |store| {
            let mut e = entry(vec![
                msg("user question"),
                msg("[Background Task Completed] apt upgrade finished (exit 0)"),
            ]);
            e.last_detach = Some(Instant::now() - std::time::Duration::from_secs(60));
            e.detach_time_utc = Some(chrono::Utc::now() - chrono::Duration::seconds(60));
            e.messages_at_detach = 1;
            store.insert("s3".to_string(), e);
        });

        let (mut rx, mut client_w, task) = drive(sessions.clone(), "s3").await;
        let replies = read_responses(&mut rx, 2).await;
        match replies.as_slice() {
            [Response::SystemMsg(brief), Response::Ok] => {
                assert!(
                    brief.contains("[Catch-up]"),
                    "brief must be the catch-up brief: {brief}"
                );
                assert!(
                    brief.contains("[Background Task Completed] apt upgrade finished"),
                    "brief must cover the message added after the detach: {brief}"
                );
            }
            other => panic!("expected brief + Ok, got {other:?}"),
        }
        // Cleared by the attach itself — before the close below re-stamps it.
        with_sessions(&sessions, |store| {
            let e = store.get("s3").unwrap();
            assert!(
                e.last_detach.is_none(),
                "detach state cleared after the brief"
            );
            assert!(e.detach_time_utc.is_none());
            assert_eq!(e.messages_at_detach, 0);
        });
        close(&mut client_w, task).await;
    }

    #[tokio::test]
    async fn reattach_within_30s_returns_no_brief() {
        let sessions = SessionStore::new();
        with_sessions(&sessions, |store| {
            let mut e = entry(vec![
                msg("user question"),
                msg("[Background Task Completed] apt upgrade finished (exit 0)"),
            ]);
            e.last_detach = Some(Instant::now());
            e.detach_time_utc = Some(chrono::Utc::now());
            e.messages_at_detach = 1;
            store.insert("s4".to_string(), e);
        });

        let (mut rx, mut client_w, task) = drive(sessions.clone(), "s4").await;
        let replies = read_responses(&mut rx, 1).await;
        assert!(
            matches!(replies.as_slice(), [Response::Ok]),
            "absence under 30 s → Ok with no SystemMsg, got {replies:?}"
        );
        // Cleared by the attach itself — before the close below re-stamps it.
        with_sessions(&sessions, |store| {
            let e = store.get("s4").unwrap();
            assert!(
                e.last_detach.is_none(),
                "state cleared even when no brief was produced"
            );
            assert!(e.detach_time_utc.is_none());
            assert_eq!(e.messages_at_detach, 0);
        });
        close(&mut client_w, task).await;
    }

    #[tokio::test]
    async fn presence_unregisters_when_the_hold_ends_in_error() {
        let sessions = SessionStore::new();
        with_sessions(&sessions, |store| {
            store.insert("s-io".to_string(), entry(vec![msg("hello")]));
        });

        let (client, server) = tokio::io::duplex(1 << 16);
        let (server_r, server_w) = tokio::io::split(server);
        let store = sessions.clone();
        let sid = "s-io".to_string();
        let task = tokio::spawn(async move {
            let mut rx = tokio::io::BufReader::new(server_r);
            let mut tx = server_w;
            handle_chat_attach(store, sid, &mut rx, &mut tx).await
        });
        let (client_r, mut client_w) = tokio::io::split(client);
        let mut rx = tokio::io::BufReader::new(client_r);
        let replies = read_responses(&mut rx, 1).await;
        assert!(
            matches!(replies.as_slice(), [Response::Ok]),
            "no detach state → Ok alone, got {replies:?}"
        );

        // Non-UTF-8 bytes make the hold's `read_line` return `Err(InvalidData)`.
        client_w.write_all(&[0xff, 0xfe, b'\n']).await.unwrap();
        client_w.shutdown().await.unwrap();

        let result = task.await.unwrap();
        assert!(result.is_err(), "the hold ends in an I/O error: {result:?}");
        assert_eq!(
            presence_count("s-io"),
            0,
            "an I/O error during the hold must unregister the connection"
        );
        with_sessions(&sessions, |store| {
            let e = store.get("s-io").unwrap();
            assert!(
                e.last_detach.is_some(),
                "an I/O error on the last connection stamps last_detach"
            );
            assert!(e.detach_time_utc.is_some());
            assert_eq!(
                e.messages_at_detach, 1,
                "messages_at_detach is the message count at close"
            );
        });
    }

    #[test]
    fn attach_brief_includes_an_alert_injected_while_away() {
        let _home = crate::test_home_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("HOME", dir.path()) };
        crate::config::Config::ensure_dirs().ok();

        let sessions = SessionStore::new();
        with_sessions(&sessions, |store| {
            store.insert(
                "away-1".to_string(),
                entry(vec![msg("hello"), msg("hi again")]),
            );
        });

        record_chat_detach("away-1", &sessions);

        let alert = msg("[Webhook Alert] disk usage at 95%");
        crate::webhook::inject_into_sessions(&sessions, &alert);

        with_sessions(&sessions, |store| {
            let entry = store.get_mut("away-1").unwrap();
            entry.last_detach = Some(Instant::now() - std::time::Duration::from_secs(60));
        });

        let brief = attach_brief_for(&sessions, "away-1");
        assert!(
            brief.is_some(),
            "an alert injected while away produces a brief"
        );
        assert!(
            brief
                .as_deref()
                .unwrap_or_default()
                .contains("[Webhook Alert]"),
            "the brief names the alert: {brief:?}"
        );

        with_sessions(&sessions, |store| {
            let entry = store.get("away-1").unwrap();
            assert!(
                entry.last_detach.is_none(),
                "the attach clears the detach state"
            );
        });
    }

    /// The reattach sequence, step by step (M29 phase 05): attach — which
    /// since this phase makes the session resident with its disk history —
    /// quit, then an alert arrives while the chat is away. The backdated
    /// detach stamp drives the catch-up brief.
    #[test]
    fn catchup_brief_reaches_a_session_that_is_not_resident() {
        let _home = crate::test_home_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("HOME", dir.path()) };
        crate::config::Config::ensure_dirs().ok();
        std::fs::create_dir_all(crate::config::sessions_dir()).unwrap();

        let sessions = SessionStore::new();
        let sid = "nonresident-catchup";

        // The chat re-attached after a daemon restart. The attach made it
        // resident with its disk history; then it quit, stamping the detach
        // fields on the entry.
        let (push_tx, _push_rx) = tokio::sync::mpsc::unbounded_channel();
        let conn = crate::daemon::session::PresenceConnection { push: push_tx };
        assert_eq!(
            crate::daemon::session::presence_register(sid, conn.clone()),
            1,
            "the reconnected chat is attached over presence"
        );
        let completed = msg("[Background Task Completed] deploy finished (exit 0)");
        with_sessions(&sessions, |store| {
            let entry = store
                .entry(sid.to_string())
                .or_insert_with(|| SessionEntry::from_disk(sid));
            // A resident turn appends to disk and pushes to the entry's
            // history; a test simulating one must do both or `record_chat_detach`
            // counts 0.
            entry.messages.push(completed);
        });
        crate::daemon::session::append_session_message(
            sid,
            &msg("[Background Task Completed] deploy finished (exit 0)"),
        );
        assert!(
            with_sessions(&sessions, |store| store.contains_key(sid)),
            "the attach made the session resident"
        );
        assert!(
            crate::daemon::session::presence_unregister(sid, &conn),
            "the chat quit: its last presence connection closed"
        );
        assert!(
            !crate::daemon::session::presence_session_ids().contains(&sid.to_string()),
            "this test's presence connection is released"
        );
        record_chat_detach(sid, &sessions);

        let alert = msg("[Webhook Alert] disk usage at 95%");
        crate::webhook::inject_into_sessions(&sessions, &alert);

        // Backdate the detach 60 s — the detach must look ≥ 30 s away.
        with_sessions(&sessions, |store| {
            let e = store.get_mut(sid).expect("the resident entry");
            assert_eq!(
                e.messages_at_detach, 1,
                "one message was in the history at detach"
            );
            e.last_detach = Some(Instant::now() - std::time::Duration::from_secs(60));
            e.detach_time_utc = Some(chrono::Utc::now() - chrono::Duration::seconds(60));
        });

        let brief = attach_brief_for(&sessions, sid);
        assert!(
            brief.is_some(),
            "a session away for 60 s gets a catch-up brief"
        );
        assert!(
            brief
                .as_deref()
                .unwrap_or_default()
                .contains("[Webhook Alert]"),
            "the brief names the alert: {brief:?}"
        );
    }

    /// The attach sequence (M29 phase 05): attach, then the entry exists and
    /// its messages equal the disk file's history. The renamed former
    /// `attach_brief_creates_no_store_entry_for_a_non_resident_session`.
    #[test]
    fn attach_makes_the_session_resident_with_its_disk_history() {
        let _home = crate::test_home_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("HOME", dir.path()) };
        crate::config::Config::ensure_dirs().ok();
        std::fs::create_dir_all(crate::config::sessions_dir()).unwrap();

        let sessions = SessionStore::new();
        let sid = "attach-resident";
        crate::daemon::session::append_session_message(
            sid,
            &msg("[Background Task Completed] deploy finished (exit 0)"),
        );

        let (push_tx, _push_rx) = tokio::sync::mpsc::unbounded_channel();
        let conn = crate::daemon::session::PresenceConnection { push: push_tx };
        crate::daemon::session::presence_register(sid, conn.clone());
        with_sessions(&sessions, |store| {
            store
                .entry(sid.to_string())
                .or_insert_with(|| SessionEntry::from_disk(sid));
        });

        assert!(
            with_sessions(&sessions, |store| store.contains_key(sid)),
            "the attach made the session resident"
        );
        let messages = crate::daemon::session::read_session_file(sid, None);
        with_sessions(&sessions, |store| {
            let e = store.get(sid).expect("the attach inserted the entry");
            assert_eq!(
                e.messages.len(),
                messages.len(),
                "the entry holds the disk history's message count"
            );
            for (entry_msg, file_msg) in e.messages.iter().zip(messages.iter()) {
                assert_eq!(
                    entry_msg.content, file_msg.content,
                    "the entry's message matches the disk history's"
                );
            }
        });

        assert!(
            crate::daemon::session::presence_unregister(sid, &conn),
            "the presence connection is released"
        );
    }

    /// The detach stamp still lands only on an entry the store already holds:
    /// after a close, `record_chat_detach` backdated to 60 s away produces a
    /// catch-up brief naming the injected alert (M29 phase 05).
    #[test]
    fn catchup_brief_names_an_alert_posted_after_the_chat_left() {
        let _home = crate::test_home_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("HOME", dir.path()) };
        crate::config::Config::ensure_dirs().ok();
        std::fs::create_dir_all(crate::config::sessions_dir()).unwrap();

        let sessions = SessionStore::new();
        let sid = "post-quit-alert";

        // The chat re-attached after a restart, which made it resident with
        // its disk history; then it quit, stamping the detach fields.
        let (push_tx, _push_rx) = tokio::sync::mpsc::unbounded_channel();
        let conn = crate::daemon::session::PresenceConnection { push: push_tx };
        assert_eq!(
            crate::daemon::session::presence_register(sid, conn.clone()),
            1,
            "the chat is attached over presence"
        );
        let completed = msg("[Background Task Completed] deploy finished (exit 0)");
        with_sessions(&sessions, |store| {
            let entry = store
                .entry(sid.to_string())
                .or_insert_with(|| SessionEntry::from_disk(sid));
            entry.messages.push(completed);
        });
        crate::daemon::session::append_session_message(
            sid,
            &msg("[Background Task Completed] deploy finished (exit 0)"),
        );
        assert!(
            with_sessions(&sessions, |store| store.contains_key(sid)),
            "the attach made the session resident"
        );
        assert!(
            crate::daemon::session::presence_unregister(sid, &conn),
            "the chat quit: its last presence connection closed"
        );
        assert!(
            !crate::daemon::session::presence_session_ids().contains(&sid.to_string()),
            "precondition: no presence connection for the id any more"
        );
        record_chat_detach(sid, &sessions);
        with_sessions(&sessions, |store| {
            let e = store.get(sid).expect("the entry survives the detach stamp");
            assert_eq!(
                e.messages_at_detach, 1,
                "one message was in the history at detach"
            );
        });

        // Backdate the detach 60 s — the detach must look ≥ 30 s away.
        with_sessions(&sessions, |store| {
            let e = store.get_mut(sid).expect("the resident entry");
            e.last_detach = Some(Instant::now() - std::time::Duration::from_secs(60));
            e.detach_time_utc = Some(chrono::Utc::now() - chrono::Duration::seconds(60));
        });

        // The alert arrives after the chat left, while it is still resident.
        let alert = msg("[Webhook Alert] LiveTicks: ticks resumed");
        crate::webhook::inject_into_sessions(&sessions, &alert);

        let brief = attach_brief_for(&sessions, sid);
        assert!(
            brief
                .as_deref()
                .unwrap_or_default()
                .contains("[Webhook Alert]"),
            "the catch-up brief names the alert posted after the chat left: {brief:?}"
        );
    }

    /// Restart sequence 1 (M29 phase 05): a fresh store (the restart), a
    /// seeded disk history, attach, then an alert injected. The resident
    /// entry's messages carry the alert — which is what the next ask reads —
    /// and so does the disk file.
    #[test]
    fn restart_reconnect_alert_ask_sees_the_alert() {
        let _home = crate::test_home_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("HOME", dir.path()) };
        crate::config::Config::ensure_dirs().ok();
        std::fs::create_dir_all(crate::config::sessions_dir()).unwrap();

        let sessions = SessionStore::new();
        let sid = "restart-seq-alert-ask";
        crate::daemon::session::append_session_message(sid, &msg("user question"));

        let (push_tx, _push_rx) = tokio::sync::mpsc::unbounded_channel();
        let conn = crate::daemon::session::PresenceConnection { push: push_tx };
        assert_eq!(
            crate::daemon::session::presence_register(sid, conn.clone()),
            1,
            "the chat re-attaches after the restart"
        );
        with_sessions(&sessions, |store| {
            store
                .entry(sid.to_string())
                .or_insert_with(|| SessionEntry::from_disk(sid));
        });

        let alert = msg("[Webhook Alert] disk usage at 95%");
        crate::webhook::inject_into_sessions(&sessions, &alert);

        with_sessions(&sessions, |store| {
            let e = store
                .get(sid)
                .expect("the attach made the session resident");
            assert!(
                e.messages
                    .iter()
                    .any(|m| m.content.contains("disk usage at 95%")),
                "the resident entry carries the alert — what the next ask reads"
            );
        });
        let file = crate::config::sessions_dir().join(format!("{sid}.jsonl"));
        let text = std::fs::read_to_string(&file)
            .unwrap_or_else(|e| panic!("the session file {file:?} exists: {e}"));
        assert!(
            text.contains("[Webhook Alert] disk usage at 95%"),
            "the alert also landed in the session file: {text}"
        );

        assert!(
            crate::daemon::session::presence_unregister(sid, &conn),
            "the presence connection is released"
        );
    }

    /// Restart sequence 2 (M29 phase 05): fresh store, seeded disk history,
    /// attach, the chat quits (the last connection closes and the detach is
    /// stamped), an alert arrives while it is away, then it re-attaches. The
    /// brief names the alert.
    #[test]
    fn restart_reconnect_quit_alert_resume_briefs_the_alert() {
        let _home = crate::test_home_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("HOME", dir.path()) };
        crate::config::Config::ensure_dirs().ok();
        std::fs::create_dir_all(crate::config::sessions_dir()).unwrap();

        let sessions = SessionStore::new();
        let sid = "restart-seq-quit-alert";
        crate::daemon::session::append_session_message(sid, &msg("user question"));

        // Attach (resident now), quit.
        let (push_tx, _push_rx) = tokio::sync::mpsc::unbounded_channel();
        let conn = crate::daemon::session::PresenceConnection { push: push_tx };
        assert_eq!(
            crate::daemon::session::presence_register(sid, conn.clone()),
            1,
            "the chat attaches"
        );
        with_sessions(&sessions, |store| {
            store
                .entry(sid.to_string())
                .or_insert_with(|| SessionEntry::from_disk(sid));
        });
        assert!(
            crate::daemon::session::presence_unregister(sid, &conn),
            "the chat quits: its last presence connection closes"
        );
        record_chat_detach(sid, &sessions);

        // The alert arrives while the chat is away.
        let alert = msg("[Webhook Alert] LiveTicks: ticks resumed");
        crate::webhook::inject_into_sessions(&sessions, &alert);

        // Backdate the detach 60 s — the detach must look ≥ 30 s away.
        with_sessions(&sessions, |store| {
            let e = store.get_mut(sid).expect("the resident entry");
            e.last_detach = Some(Instant::now() - std::time::Duration::from_secs(60));
            e.detach_time_utc = Some(chrono::Utc::now() - chrono::Duration::seconds(60));
        });

        // Re-attach ≥ 60 s later: the catch-up brief names the alert.
        let (tx2, _rx2) = tokio::sync::mpsc::unbounded_channel();
        let conn2 = crate::daemon::session::PresenceConnection { push: tx2 };
        assert_eq!(
            crate::daemon::session::presence_register(sid, conn2.clone()),
            1,
            "the chat re-attaches"
        );
        let brief = attach_brief_for(&sessions, sid);
        assert!(
            brief
                .as_deref()
                .unwrap_or_default()
                .contains("[Webhook Alert] LiveTicks: ticks resumed"),
            "the resume brief names the alert posted while the chat was away: {brief:?}"
        );
        assert!(
            crate::daemon::session::presence_unregister(sid, &conn2),
            "the presence connection is released"
        );
    }

    /// Restart sequence 3 (M29 phase 05): fresh store, a never-loaded
    /// session's disk history and meta, attach. The entry carries both —
    /// history and continuity meta.
    #[test]
    fn fresh_resume_of_a_never_loaded_session_carries_history_and_meta() {
        let _home = crate::test_home_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("HOME", dir.path()) };
        crate::config::Config::ensure_dirs().ok();
        std::fs::create_dir_all(crate::config::sessions_dir()).unwrap();

        let sessions = SessionStore::new();
        let sid = "restart-seq-fresh-resume";
        crate::daemon::session::append_session_message(sid, &msg("first question"));
        crate::daemon::session::append_session_message(sid, &msg("second question"));
        crate::daemon::session::write_session_meta(
            sid,
            &crate::daemon::session::SessionMeta {
                started_at: chrono::Utc::now(),
                turn_count: 5,
                last_prompt_tokens: 1234,
                token_scale: 1.25,
                tool_calls_this_session: 3,
                saved_name: Some("deploy hunt".to_string()),
            },
        );

        let (push_tx, _push_rx) = tokio::sync::mpsc::unbounded_channel();
        let conn = crate::daemon::session::PresenceConnection { push: push_tx };
        assert_eq!(
            crate::daemon::session::presence_register(sid, conn.clone()),
            1,
            "the chat attaches"
        );
        with_sessions(&sessions, |store| {
            store
                .entry(sid.to_string())
                .or_insert_with(|| SessionEntry::from_disk(sid));
        });

        with_sessions(&sessions, |store| {
            let e = store
                .get(sid)
                .expect("the attach made the session resident");
            assert_eq!(
                e.messages.len(),
                2,
                "the entry carries the disk history's two messages"
            );
            assert_eq!(e.turn_count, 5, "the meta's turn_count is restored");
            assert_eq!(
                e.last_prompt_tokens, 1234,
                "the meta's last_prompt_tokens is restored"
            );
            assert_eq!(e.token_scale, 1.25, "the meta's token_scale is restored");
            assert_eq!(
                e.tool_calls_this_session, 3,
                "the meta's tool_calls_this_session is restored"
            );
            assert_eq!(
                e.saved_name.as_deref(),
                Some("deploy hunt"),
                "the meta's saved_name is restored"
            );
        });

        assert!(
            crate::daemon::session::presence_unregister(sid, &conn),
            "the presence connection is released"
        );
    }
}

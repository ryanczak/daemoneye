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
/// has `last_detach`, the away time is measured from it and the brief lists
/// the notices the journal holds for `[detach, now)`. The detach state is
/// cleared whether or not a brief was produced. A session with no entry
/// yields no brief — presence alone carries the attach. Since M29 phase 05
/// an attach inserts the entry from disk first, so a session that detached
/// before a daemon restart re-attaches resident and reads the same branch.
pub(crate) fn attach_brief_for(sessions: &SessionStore, session_id: &str) -> Option<String> {
    with_sessions(sessions, |store| {
        let i = store.get_mut(session_id)?.as_interactive_mut()?;
        let brief = i.last_detach.and_then(|detach| {
            let away_secs = detach.elapsed().as_secs();
            build_catchup_brief(session_id, away_secs, i.detach_time_utc)
        });
        i.last_detach = None;
        i.detach_time_utc = None;
        i.messages_at_detach = 0;
        brief
    })
}

/// Join the catch-up brief and the away digest into the one `SystemMsg` the
/// attach parser accepts, separated by a blank line. Whichever exists is
/// alone; when neither exists nothing is sent.
fn join_brief_and_digest(brief: Option<String>, digest: Option<String>) -> Option<String> {
    match (brief, digest) {
        (Some(b), Some(d)) => Some(format!("{b}\n\n{d}")),
        (Some(b), None) => Some(b),
        (None, Some(d)) => Some(d),
        (None, None) => None,
    }
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
pub async fn handle_chat_attach<R, W>(
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
    // The away digest reads files and must not run under the session lock —
    // the same rule `from_disk` above follows. The attaching connection is
    // already registered, so the gate inside sees every live connection.
    let digest = crate::daemon::away_digest::take_away_digest_on_attach(
        crate::daemon::away_digest::now_unix(),
    );
    log::info!("chat presence attached for session {session_id}");

    // One fallible step: reply, then hold until EOF or error. Nothing after
    // `presence_register` may skip the unregister, so this runs to completion
    // (either way) before the bookkeeping below.
    let held = async {
        if let Some(msg) = join_brief_and_digest(attach_brief_for(&sessions, &session_id), digest) {
            send_response_split(tx, Response::SystemMsg(msg)).await?;
        }
        send_response_split(tx, Response::Ok).await?;
        // M37 phase 01: a chat attaching while alerts are already firing
        // learns about them once, after the attach reply's `Ok` — the client's
        // attach parser accepts exactly brief-then-`Ok`, and its parked task
        // forwards every later frame. An empty registry sends nothing: the
        // client's bar has no alerts to show.
        let snapshot = crate::webhook::active_alerts_snapshot();
        if !snapshot.is_empty() {
            send_response_split(tx, Response::AlertState { alerts: snapshot }).await?;
        }
        // M37 phase 02b: the session's counters once at attach, after the
        // `Ok` (and after the `AlertState` when one is sent). This value
        // seeds the 1 s task's last-pushed record, so a session whose
        // counters are already stable gets no duplicate push a second later.
        let reg = crate::shell::registry().clone();
        let counters = crate::daemon::counters::counters_for_session(
            &reg,
            &session_id,
            crate::daemon::stats::get_ghosts_active() as u32,
        );
        crate::daemon::counters::record_session_counters(&session_id, counters, |frame| {
            crate::daemon::counters::presence_push_to_session(
                crate::daemon::session::global_presence(),
                &session_id,
                frame,
            );
        });

        // Hold the connection for the client's lifetime. Any bytes the client
        // sends are read and discarded — the channel carries daemon→client
        // traffic; the client talks over its other connections. Pushes queued
        // by `presence_broadcast_frame` are written ahead of the reads, with
        // the read parked until one arrives so a quiet connection costs
        // nothing. EOF, a read error, or a failed write (reply or push) all
        // end the hold.
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
                Some(frame) = push_rx.recv() => Some(frame),
            };
            let Some(frame) = line else { continue };
            // Written as it arrived: `presence_broadcast_frame` queued a
            // serialised `Response`, and the hold loop does not re-wrap it.
            tx.write_all(frame.as_bytes()).await?;
            tx.write_all(b"\n").await?;
            discarded.clear();
        }
    };
    let result = held.await;

    let was_last = presence_unregister(&session_id, &conn);
    log::info!("chat presence detached for session {session_id} (last={was_last})");
    // Not only when `was_last`: the stamp is daemon-wide, so any close that
    // leaves no presence connection anywhere bounds the digest window.
    crate::daemon::away_digest::stamp_on_last_detach();
    if was_last {
        crate::daemon::counters::forget_session_counters(&session_id);
        record_chat_detach(&session_id, &sessions);
        // B11 (M36 phase-17a): the session's shells get a grace. The task
        // owns the id; no lock — presence or registry — crosses the sleep.
        // `0` keeps today's behaviour: never close on detach.
        let grace_secs = crate::config::current().shells.detach_close_secs;
        if grace_secs > 0 {
            let sid = session_id.clone();
            tokio::spawn(async move {
                let reg = crate::shell::registry().clone();
                let closed = crate::daemon::executor::knowledge::close_after_detach(
                    &reg,
                    &sid,
                    std::time::Duration::from_secs(grace_secs),
                )
                .await;
                if closed.is_empty() {
                    log::info!(
                        "detach-close for session {sid}: nothing to close (re-attached in time, or no shells)"
                    );
                }
            });
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::Message;
    use crate::daemon::session::{
        SessionEntry, presence_broadcast, presence_broadcast_frame_in, presence_broadcast_in,
        presence_count,
    };
    use std::collections::HashMap;
    use std::io::Write;
    use std::sync::Mutex;
    use std::time::Instant;
    use tokio::io::AsyncWriteExt;

    fn entry(messages: Vec<Message>) -> SessionEntry {
        let mut e = SessionEntry::new_interactive("test");
        e.history.messages = messages;
        e
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

    /// The serialized-notice journal record `Bus::new` writes, with a pinned
    /// `ts`, `event: "notice"` and `pid` — the shape
    /// `for_each_notice_between` parses. The record's `ts` is the notice's
    /// own, so the notice is pinned with `ts_refire` before this runs.
    fn journal_record(notice: &crate::bus::Notice) -> serde_json::Value {
        let mut record = serde_json::to_value(notice).expect("serialize notice");
        let obj = record.as_object_mut().expect("notice is an object");
        let mut stamped = serde_json::Map::new();
        stamped.insert(
            "ts".to_string(),
            serde_json::Value::String(notice.ts.to_rfc3339()),
        );
        stamped.insert(
            "event".to_string(),
            serde_json::Value::String("notice".to_string()),
        );
        stamped.insert("pid".to_string(), serde_json::Value::from(4242));
        for (k, v) in obj.clone() {
            stamped.insert(k, v);
        }
        serde_json::Value::Object(stamped)
    }

    /// Journal one notice at `ts`: append its record into the dated segment
    /// the instant names, the way `Bus::new`'s journal records land.
    fn seed_notice_record(notice: &crate::bus::Notice, ts: chrono::DateTime<chrono::Utc>) {
        let notice = notice.clone().ts_refire(ts.timestamp());
        let path =
            crate::config::events_dir().join(format!("events-{}.jsonl", ts.format("%Y%m%d")));
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).expect("the events dir");
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .expect("opening the segment");
        writeln!(
            f,
            "{}",
            serde_json::to_string(&journal_record(&notice)).expect("serializing record")
        )
        .expect("appending");
    }

    /// A webhook alert notice shaped as `process_alert` writes it: `Warning`
    /// firing, `kind` `alert`, the raw severity label carried on top.
    fn alert_notice(name: &str, title: &str) -> crate::bus::Notice {
        crate::bus::Notice::new(
            crate::bus::Severity::Warning,
            crate::bus::Source::Webhook,
            name,
            title,
        )
        .source_id("alertmanager")
        .status(crate::bus::NoticeStatus::Firing)
        .fingerprint(format!("fp-{name}"))
        .label("severity", "warning")
        .label("kind", "alert")
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
        // A throwaway HOME: the attach path now stamps `chat_last_seen`, and
        // the guard serializes every attach-driving test so no other chat is
        // live in the process-global presence map while this one runs.
        let (_home, _dir) = crate::test_home();
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
        let (_home, _dir) = crate::test_home();
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
            let e = store.get("s1").unwrap().as_interactive().unwrap();
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
        let (_home, _dir) = crate::test_home();
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

    /// `presence_broadcast_frame` reaches every connection in the map with the
    /// serialised `Response` — one frame, delivered to each `push` channel.
    #[tokio::test]
    async fn presence_broadcast_frame_reaches_every_connection() {
        let map: Mutex<HashMap<String, Vec<PresenceConnection>>> = Mutex::new(HashMap::new());
        let (tx1, mut rx1) = tokio::sync::mpsc::unbounded_channel();
        let (tx2, mut rx2) = tokio::sync::mpsc::unbounded_channel();
        map.lock().unwrap().insert(
            "frame-sess".to_string(),
            vec![
                PresenceConnection { push: tx1 },
                PresenceConnection { push: tx2 },
            ],
        );

        let frame = Response::AlertState {
            alerts: vec![crate::ipc::AlertBadge {
                fingerprint: "fp-frame".to_string(),
                name: "DiskFull".to_string(),
                severity: "warning".to_string(),
                first_seen_unix: 1_000,
                last_seen_unix: 2_000,
            }],
        };
        let pushed = presence_broadcast_frame_in(&map, &frame);
        assert_eq!(pushed, 2, "both connections got the frame");

        let got1 = rx1.recv().await.expect("connection 1's push");
        let got2 = rx2.recv().await.expect("connection 2's push");
        for (which, got) in [("conn 1", got1), ("conn 2", got2)] {
            let parsed: Response = serde_json::from_str(&got)
                .unwrap_or_else(|e| panic!("{which} received a parseable frame: {e}"));
            match parsed {
                Response::AlertState { alerts } => {
                    assert_eq!(alerts.len(), 1, "{which} carries one badge");
                    assert_eq!(alerts[0].name, "DiskFull");
                    assert_eq!(alerts[0].last_seen_unix, 2_000);
                }
                other => panic!("{which} received the AlertState frame, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn presence_with_two_clients_detaches_only_on_last() {
        let (_home, _dir) = crate::test_home();
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
            let e = store.get("s2").unwrap().as_interactive().unwrap();
            assert!(
                e.last_detach.is_none(),
                "one of two still attached → no detach stamp"
            );
        });

        close(&mut client2, task2).await;
        with_sessions(&sessions, |store| {
            let e = store.get("s2").unwrap().as_interactive().unwrap();
            assert!(e.last_detach.is_some(), "the last close stamps the detach");
            assert_eq!(e.messages_at_detach, 1);
        });
        assert_eq!(presence_count("s2"), 0);
    }

    #[tokio::test]
    async fn reattach_after_30s_returns_catchup_brief() {
        let (_home, _dir) = crate::test_home();
        let sessions = SessionStore::new();
        with_sessions(&sessions, |store| {
            let mut e = entry(vec![
                msg("user question"),
                msg("[Background Task Completed] apt upgrade finished (exit 0)"),
            ]);
            let i = e.as_interactive_mut().unwrap();
            i.last_detach = Some(Instant::now() - std::time::Duration::from_secs(60));
            i.detach_time_utc = Some(chrono::Utc::now() - chrono::Duration::seconds(60));
            i.messages_at_detach = 1;
            store.insert("s3".to_string(), e);
        });

        // The brief reads the journal, not the history: journal a notice
        // inside the detach window so the brief has something to list.
        let detach = with_sessions(&sessions, |store| {
            store
                .get("s3")
                .expect("the entry")
                .as_interactive()
                .unwrap()
                .detach_time_utc
        })
        .expect("the detach stamp");
        let bg = crate::bus::Notice::new(
            crate::bus::Severity::Info,
            crate::bus::Source::Background,
            "run-apt",
            "[Background Task Completed] apt upgrade finished (exit 0)",
        )
        .source_id("run-apt")
        .label("kind", "completed");
        seed_notice_record(&bg, detach + chrono::Duration::seconds(5));

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
                    "brief must cover the notice journaled after the detach: {brief}"
                );
            }
            other => panic!("expected brief + Ok, got {other:?}"),
        }
        // Cleared by the attach itself — before the close below re-stamps it.
        with_sessions(&sessions, |store| {
            let e = store.get("s3").unwrap().as_interactive().unwrap();
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
        let (_home, _dir) = crate::test_home();
        let sessions = SessionStore::new();
        with_sessions(&sessions, |store| {
            let mut e = entry(vec![
                msg("user question"),
                msg("[Background Task Completed] apt upgrade finished (exit 0)"),
            ]);
            let i = e.as_interactive_mut().unwrap();
            i.last_detach = Some(Instant::now());
            i.detach_time_utc = Some(chrono::Utc::now());
            i.messages_at_detach = 1;
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
            let e = store.get("s4").unwrap().as_interactive().unwrap();
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
        let (_home, _dir) = crate::test_home();
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
            let e = store.get("s-io").unwrap().as_interactive().unwrap();
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

        // Backdate the detach 60 s — the detach must look ≥ 30 s away — and
        // journal the alert inside the window it names.
        with_sessions(&sessions, |store| {
            let entry = store
                .get_mut("away-1")
                .unwrap()
                .as_interactive_mut()
                .unwrap();
            entry.last_detach = Some(Instant::now() - std::time::Duration::from_secs(60));
            entry.detach_time_utc = Some(chrono::Utc::now() - chrono::Duration::seconds(60));
        });
        let detach = with_sessions(&sessions, |store| {
            store
                .get("away-1")
                .unwrap()
                .as_interactive()
                .unwrap()
                .detach_time_utc
        })
        .expect("the detach stamp");
        let alert = alert_notice("disk-high", "[Webhook Alert] disk usage at 95%");
        seed_notice_record(&alert, detach + chrono::Duration::seconds(5));

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
            let entry = store.get("away-1").unwrap().as_interactive().unwrap();
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
            entry.history.messages.push(completed);
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

        // Backdate the detach 60 s — the detach must look ≥ 30 s away — and
        // journal the alert inside the window it names.
        with_sessions(&sessions, |store| {
            let e = store
                .get_mut(sid)
                .expect("the resident entry")
                .as_interactive_mut()
                .unwrap();
            assert_eq!(
                e.messages_at_detach, 1,
                "one message was in the history at detach"
            );
            e.last_detach = Some(Instant::now() - std::time::Duration::from_secs(60));
            e.detach_time_utc = Some(chrono::Utc::now() - chrono::Duration::seconds(60));
        });
        let detach = with_sessions(&sessions, |store| {
            store
                .get(sid)
                .expect("the resident entry")
                .as_interactive()
                .unwrap()
                .detach_time_utc
        })
        .expect("the detach stamp");
        let alert = alert_notice("disk-high", "[Webhook Alert] disk usage at 95%");
        seed_notice_record(&alert, detach + chrono::Duration::seconds(5));

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
                e.history.messages.len(),
                messages.len(),
                "the entry holds the disk history's message count"
            );
            for (entry_msg, file_msg) in e.history.messages.iter().zip(messages.iter()) {
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
            entry.history.messages.push(completed);
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
            let e = store
                .get(sid)
                .expect("the entry survives the detach stamp")
                .as_interactive()
                .unwrap();
            assert_eq!(
                e.messages_at_detach, 1,
                "one message was in the history at detach"
            );
        });

        // Backdate the detach 60 s — the detach must look ≥ 30 s away.
        with_sessions(&sessions, |store| {
            let e = store
                .get_mut(sid)
                .expect("the resident entry")
                .as_interactive_mut()
                .unwrap();
            e.last_detach = Some(Instant::now() - std::time::Duration::from_secs(60));
            e.detach_time_utc = Some(chrono::Utc::now() - chrono::Duration::seconds(60));
        });

        // The alert arrives after the chat left, while it is still resident:
        // it is journaled inside the detach window the brief reads.
        let detach = with_sessions(&sessions, |store| {
            store
                .get(sid)
                .expect("the resident entry")
                .as_interactive()
                .unwrap()
                .detach_time_utc
        })
        .expect("the detach stamp");
        let alert = alert_notice("LiveTicks", "[Webhook Alert] LiveTicks: ticks resumed");
        seed_notice_record(&alert, detach + chrono::Duration::seconds(5));

        let brief = attach_brief_for(&sessions, sid);
        assert!(
            brief
                .as_deref()
                .unwrap_or_default()
                .contains("[Webhook Alert] LiveTicks: ticks resumed"),
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
                e.history
                    .messages
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

        // Backdate the detach 60 s — the detach must look ≥ 30 s away.
        let detach = chrono::Utc::now() - chrono::Duration::seconds(60);
        with_sessions(&sessions, |store| {
            let e = store
                .get_mut(sid)
                .expect("the resident entry")
                .as_interactive_mut()
                .unwrap();
            e.last_detach = Some(Instant::now() - std::time::Duration::from_secs(60));
            e.detach_time_utc = Some(detach);
        });

        // The alert arrives while the chat is away: journaled inside the
        // `[detach, now)` window the brief reads — never in the future.
        let alert = alert_notice("LiveTicks", "[Webhook Alert] LiveTicks: ticks resumed");
        seed_notice_record(&alert, detach + chrono::Duration::seconds(5));

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
                e.history.messages.len(),
                2,
                "the entry carries the disk history's two messages"
            );
            assert_eq!(e.history.turn_count, 5, "the meta's turn_count is restored");
            assert_eq!(
                e.ledger.last_prompt_tokens, 1234,
                "the meta's last_prompt_tokens is restored"
            );
            assert_eq!(
                e.ledger.token_scale, 1.25,
                "the meta's token_scale is restored"
            );
            assert_eq!(
                e.ledger.tool_calls_this_session, 3,
                "the meta's tool_calls_this_session is restored"
            );
            assert_eq!(
                e.persistence.saved_name.as_deref(),
                Some("deploy hunt"),
                "the meta's saved_name is restored"
            );
        });

        assert!(
            crate::daemon::session::presence_unregister(sid, &conn),
            "the presence connection is released"
        );
    }

    // ── M37 phase 08: the away digest rides the attach message ──────────────

    #[tokio::test]
    async fn attach_joins_the_digest_to_the_brief_in_one_system_msg() {
        let _home = crate::test_home_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("HOME", dir.path()) };
        crate::config::Config::ensure_dirs().ok();

        let stamp = 1_700_000_000u64; // 2023-11-14 22:13:20 UTC
        let from = chrono::DateTime::from_timestamp(stamp as i64, 0).unwrap();
        crate::daemon::away_digest::stamp_last_seen(stamp);
        seed_notice_record(
            &crate::bus::Notice::new(
                crate::bus::Severity::Info,
                crate::bus::Source::Scheduler,
                "tmp-cast-purge",
                "`tmp-cast-purge.sh` finished (exit 0)",
            )
            .source_id("job-tmp-cast-purge")
            .label("kind", "script")
            .label("exit_code", "0"),
            from + chrono::Duration::minutes(5),
        );

        // A catch-up brief is pending too: the digest must join it in the
        // ONE SystemMsg the attach parser accepts.
        let sessions = SessionStore::new();
        with_sessions(&sessions, |store| {
            let mut e = entry(vec![
                msg("user question"),
                msg("[Background Task Completed] apt upgrade finished (exit 0)"),
            ]);
            let i = e.as_interactive_mut().unwrap();
            i.last_detach = Some(Instant::now() - std::time::Duration::from_secs(60));
            i.detach_time_utc = Some(from + chrono::Duration::minutes(30));
            i.messages_at_detach = 1;
            store.insert("away-1".to_string(), e);
        });
        // The brief lists what the journal holds for the detach window:
        // a background notice inside it, after the digest's stamp.
        let bg = crate::bus::Notice::new(
            crate::bus::Severity::Info,
            crate::bus::Source::Background,
            "run-apt",
            "[Background Task Completed] apt upgrade finished (exit 0)",
        )
        .source_id("run-apt")
        .label("kind", "completed");
        seed_notice_record(&bg, from + chrono::Duration::minutes(40));

        let (mut rx, mut client_w, task) = drive(sessions.clone(), "away-1").await;
        let replies = read_responses(&mut rx, 2).await;
        match replies.as_slice() {
            [Response::SystemMsg(msg), Response::Ok] => {
                assert!(
                    msg.contains("[Catch-up]"),
                    "the brief is first in the joined message: {msg}"
                );
                assert!(
                    msg.contains("[Away digest]"),
                    "the digest rides the same message: {msg}"
                );
                assert!(
                    msg.contains("tmp-cast-purge"),
                    "the digest carries the job's counts: {msg}"
                );
                let brief_end = msg.find("[Catch-up]").unwrap();
                assert!(
                    msg[brief_end..].contains("\n\n"),
                    "brief and digest are separated by a blank line: {msg}"
                );
            }
            other => panic!("expected one joined SystemMsg + Ok, got {other:?}"),
        }
        close(&mut client_w, task).await;
        let restamped = crate::daemon::away_digest::read_last_seen()
            .expect("delivery writes the stamp even when the digest was produced");
        assert!(
            restamped > stamp,
            "the stamp moved past the window, so the same events are never reported twice: \
             {restamped} <= {stamp}"
        );
        // A second attach over the same events reports nothing new: the
        // window's lower bound moved past them.
        let (mut rx2, mut client_w2, task2) = drive(sessions.clone(), "away-1").await;
        let replies2 = read_responses(&mut rx2, 1).await;
        assert!(
            matches!(replies2.as_slice(), [Response::Ok]),
            "the window is never reported twice, got {replies2:?}"
        );
        close(&mut client_w2, task2).await;
    }

    #[tokio::test]
    async fn the_last_detach_stamps_last_seen() {
        let _home = crate::test_home_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("HOME", dir.path()) };
        crate::config::Config::ensure_dirs().ok();

        let before = crate::daemon::away_digest::now_unix();
        crate::daemon::away_digest::stamp_last_seen(before.saturating_sub(3600));

        let sessions = SessionStore::new();
        let (mut rx, mut client_w, task) = drive(sessions.clone(), "away-detach-1").await;
        let _ = read_responses(&mut rx, 1).await;
        close(&mut client_w, task).await;

        let after =
            crate::daemon::away_digest::read_last_seen().expect("the last close stamps last seen");
        assert!(
            after >= before,
            "the stamp moved forward to the close time: {after} < {before}"
        );
    }
}

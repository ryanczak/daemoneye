//! Asynchronous background compaction.
//!
//! After a turn completes, if the session crossed the compaction threshold
//! (but not the emergency threshold), a background `tokio::spawn` task builds
//! the epoch (with narrative) and swaps the compacted working set into the
//! session entry with a staleness check.

use crate::ai::Message;
use crate::config::Config;
use crate::daemon::context::epochs;
use crate::daemon::session::SessionStore;
use crate::daemon::utils::log_event;
use crate::util::UnpoisonExt;
use std::sync::Arc;

/// Whether an epoch build may spend a model call on a narrative summary.
///
/// The emergency (synchronous) compaction path NEVER builds a narrative,
/// regardless of the operator's `narrative_enabled` flag — it is the
/// extreme-pressure backstop and must not block the interactive turn on a
/// model call (phase 08 acceptance criterion). The background path respects
/// the flag.
pub(crate) fn epoch_narrative_allowed(is_emergency: bool, narrative_enabled: bool) -> bool {
    !is_emergency && narrative_enabled
}

/// Snapshot of the data needed for background compaction.
/// Cloned out of the lock before the async work begins.
struct CompactionSnapshot {
    session_id: String,
    messages: Vec<Message>,
    turn_count: usize,
    msg_len: usize,
    token_scale: f64,
}

/// Spawn the deferred compaction for `session_id` if none is in flight.
/// Called from the end-of-turn write-back in stream.rs.
pub fn spawn_compaction(session_id: String, sessions: SessionStore, config_snapshot: Arc<Config>) {
    // Check if already in flight — don't even spawn if so.
    let snapshot = match try_snapshot(&session_id, &sessions) {
        Some(s) => s,
        None => return, // already in flight or ghost
    };

    let sessions_clone = sessions.clone();
    let config_clone = Arc::clone(&config_snapshot);

    tokio::spawn(async move {
        let result = run_compaction(&snapshot, &sessions_clone, &config_clone).await;
        match result {
            Ok(_) => {}
            Err(e) => {
                log::warn!(
                    "Background compaction for session {}: {}",
                    snapshot.session_id,
                    e
                );
            }
        }
    });
}

/// Try to take a snapshot and mark the session as in-flight.
/// Returns `None` if already in flight or the entry is a ghost.
fn try_snapshot(session_id: &str, sessions: &SessionStore) -> Option<CompactionSnapshot> {
    let mut store = sessions.lock().unwrap_or_log();
    let entry = store.get_mut(session_id)?;
    if entry.compaction_in_flight || entry.is_ghost {
        return None;
    }
    entry.compaction_in_flight = true;

    let snapshot = CompactionSnapshot {
        session_id: session_id.to_string(),
        messages: entry.messages.clone(),
        turn_count: entry.turn_count,
        msg_len: entry.messages.len(),
        token_scale: entry.token_scale,
    };
    drop(store);
    Some(snapshot)
}

/// Run the background compaction logic on a snapshot.
///
/// Structure:
/// 1. Async work (no lock): plan cut, build epoch, compact.
/// 2. Swap (lock once): staleness check → swap or discard.
/// 3. Persist (no lock): write session file.
async fn run_compaction(
    snapshot: &CompactionSnapshot,
    sessions: &SessionStore,
    config: &Config,
) -> Result<(), String> {
    // Step 1: Async work — build the epoch with narrative.
    let context_window = config.resolve_model(None).context_window();
    let budget = (context_window as u64 * config.compaction.target_pct as u64) / 100;
    let prior = epochs::read_epochs(&snapshot.session_id);

    // Plan the cut FIRST — the idempotency guard below compares against the
    // last turn of the span this build would actually drop, not the whole
    // snapshot (the dropped span never reaches the snapshot's final turn).
    let tail_start = crate::daemon::digest::planned_tail_start_by_budget(
        &snapshot.messages,
        budget,
        snapshot.token_scale,
    )
    .or_else(|| {
        crate::daemon::digest::synthesized_tail_start(
            &snapshot.messages,
            budget,
            snapshot.token_scale,
        )
    });

    let Some(tail_start) = tail_start else {
        // No viable cut — discard.
        if let Some(entry) = sessions
            .lock()
            .unwrap_or_log()
            .get_mut(&snapshot.session_id)
        {
            entry.compaction_in_flight = false;
        }
        return Ok(());
    };

    // Idempotency guard: if the most recent epoch already covers the last turn
    // of the span we would drop, a prior (possibly discarded) build already
    // recorded this epoch — skip to avoid a duplicate. Guarded on a non-zero
    // turn so histories without turn numbers don't spuriously skip everything.
    let dropped_last_turn = epochs::last_turn_of(&snapshot.messages[..tail_start]);
    if let Some(last_prior) = prior.last()
        && dropped_last_turn > 0
        && last_prior.turn_end >= dropped_last_turn
    {
        if let Some(entry) = sessions
            .lock()
            .unwrap_or_log()
            .get_mut(&snapshot.session_id)
        {
            entry.compaction_in_flight = false;
        }
        return Ok(());
    }

    // Build the epoch with narrative (this is the whole point of async).
    let span_start = prior
        .last()
        .map(|e| e.ts_end)
        .unwrap_or_else(chrono::Utc::now);
    let span_end = chrono::Utc::now();

    // Narrative allowed here — this is the whole point of doing it in the
    // background — but still gated on the config flag so operators can disable
    // the model call entirely. The emergency (synchronous) path never sets it
    // (is_emergency = false here; see epoch_narrative_allowed).
    let narrative = if epoch_narrative_allowed(false, config.digest.narrative_enabled) {
        crate::daemon::digest::build_narrative_summary(
            &snapshot.messages[..tail_start],
            config.resolve_model(Some("digest")),
        )
        .await
    } else {
        None
    };

    let record = epochs::EpochRecord {
        seq: prior.last().map(|e| e.seq + 1).unwrap_or(1),
        kind: "epoch".into(),
        turn_start: epochs::first_turn_of(&snapshot.messages[..tail_start]),
        turn_end: epochs::last_turn_of(&snapshot.messages[..tail_start]),
        ts_start: span_start,
        ts_end: span_end,
        msg_count: tail_start as u32,
        narrative,
        tally: epochs::tally_span(&snapshot.session_id, span_start, span_end),
        artifacts: epochs::scan_artifacts_span(span_start, span_end),
        covers: None,
    };
    epochs::append_epoch(&snapshot.session_id, &record);
    log_event(
        "epoch_created",
        serde_json::json!({
            "session": &snapshot.session_id,
            "seq": record.seq,
            "turns": [record.turn_start, record.turn_end],
            "msgs": record.msg_count,
        }),
    );

    // Opt-in memory extraction from the dropped span.
    let _ = epochs::extract_memories_from_epoch(
        &snapshot.session_id,
        &record,
        &snapshot.messages[..tail_start],
        config,
    )
    .await;

    // Attempt chapter rollup.
    let _ = epochs::maybe_rollup(&snapshot.session_id, config).await;

    // Render context block and compact.
    let chain = epochs::read_epochs(&snapshot.session_id);
    let env = config.context.environment.clone();
    let host = crate::daemon::utils::daemon_hostname();
    let rendered = epochs::render_context_block(&chain);

    let compacted = epochs::compact_with_epochs(
        snapshot.messages.clone(),
        &rendered,
        &env,
        &host,
        record.turn_end as usize,
        0, // tail_first_turn — not needed for background compaction
        tail_start,
    );

    // Repair the tail head for any orphan tool_results.
    let mut compacted = compacted;
    if 2 < compacted.len() {
        let tail = &mut compacted[2..];
        crate::daemon::digest::repair_tail_head(tail);
    }

    // Step 2: Swap (lock once, synchronous). No `.await` may occur while the
    // guard is alive — all async work is already done above.
    let (before_len, after_len) = {
        let mut store = sessions.lock().unwrap_or_log();
        // Staleness check: if the entry is gone, or turn_count/msg_len changed,
        // discard the compacted vec.
        let Some(entry) = store.get_mut(&snapshot.session_id) else {
            // Entry evicted — discard. The epoch record already appended is
            // harmless — it describes real history; the next load simply has
            // one epoch whose messages are still in the working file.
            return Ok(());
        };

        if entry.turn_count != snapshot.turn_count || entry.messages.len() != snapshot.msg_len {
            // A turn ran while we worked — discard. Clear the flag so the next
            // turn's end can re-spawn with fresh data.
            entry.compaction_in_flight = false;
            return Ok(());
        }

        // Match — swap.
        let before_len = entry.messages.len();
        let after_len = compacted.len();
        entry.messages = compacted.clone();
        entry.compaction_in_flight = false;
        entry.pending_compaction_notice = Some(format!(
            "↩ Session history compacted in the background ({} → {} messages) — epoch {} recorded",
            before_len, after_len, record.seq,
        ));
        entry.dirty = true;
        (before_len, after_len)
    };

    // Step 3: Persist (no lock held).
    // File write outside the lock is safe: the per-turn append path only runs
    // during a turn, and a turn arriving now would have failed the staleness
    // check above — so no concurrent write can interleave.
    crate::daemon::session::write_session_file(&snapshot.session_id, &compacted);
    log_event(
        "compaction",
        serde_json::json!({
            "session": &snapshot.session_id,
            "msgs_before": before_len,
            "msgs_after": after_len,
            "mode": "background",
        }),
    );
    crate::daemon::stats::record_compaction(before_len, after_len);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::Message;
    use crate::config::Config;
    use crate::daemon::session::SessionEntry;
    use std::collections::HashMap;

    /// RAII test-home guard: holds `TEST_HOME_LOCK`, points `HOME` at a fresh
    /// tempdir, and restores it on drop — required for any test that drives
    /// `run_compaction` to completion (it writes epoch/session files under
    /// `~/.daemoneye`). Held across `.await` — fine on the current-thread test
    /// runtime (`#[tokio::test]` is single-threaded, so the !Send guard is ok).
    struct TestHome {
        _tmp: tempfile::TempDir,
        _lock: std::sync::MutexGuard<'static, ()>,
        saved: Option<String>,
    }

    impl TestHome {
        fn new() -> Self {
            let lock = crate::TEST_HOME_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let saved = std::env::var("HOME").ok();
            let tmp = tempfile::tempdir().unwrap();
            unsafe {
                std::env::set_var("HOME", tmp.path());
            }
            Self {
                _tmp: tmp,
                _lock: lock,
                saved,
            }
        }
    }

    impl Drop for TestHome {
        fn drop(&mut self) {
            unsafe {
                match &self.saved {
                    Some(h) => std::env::set_var("HOME", h),
                    None => std::env::remove_var("HOME"),
                }
            }
        }
    }

    /// Build `n` alternating user/assistant messages with sequential turn
    /// numbers, so `first_turn_of`/`last_turn_of` and `next_clean_turn_start`
    /// find real boundaries.
    fn make_turn_msgs(n: usize) -> Vec<Message> {
        (0..n)
            .map(|i| Message {
                role: if i % 2 == 0 { "user" } else { "assistant" }.to_string(),
                content: format!("msg-{}", i),
                tool_calls: None,
                tool_results: None,
                turn: Some(i / 2),
            })
            .collect()
    }

    /// Config whose background compaction makes no model call (hermetic).
    fn hermetic_config() -> Config {
        let mut config = Config::default();
        config.digest.narrative_enabled = false;
        config
    }

    fn make_test_entry() -> SessionEntry {
        SessionEntry {
            messages: vec![],
            last_accessed: std::time::Instant::now(),
            chat_pane: None,
            default_target_pane: None,
            bg_windows: vec![],
            last_prompt_tokens: 0,
            token_scale: 1.5,
            compaction_in_flight: false,
            pending_compaction_notice: None,
            tmux_session: "test".to_string(),
            last_detach: None,
            detach_time_utc: None,
            messages_at_detach: 0,
            pipe_source_pane: None,
            is_ghost: false,
            ghost_config: None,
            ghost_bg_prefix: "",
            started_at: chrono::Utc::now(),
            turn_count: 0,
            tool_calls_this_session: 0,
            active_model: None,
            last_snapshot_activity: 0,
            saved_name: None,
            dirty: false,
            artifacts_created: vec![],
            auto_name_suggested: false,
            ghost_task_message: None,
            loaded_tools: std::collections::HashSet::new(),
            cost_usd: 0.0,
            cost_by_agent: std::collections::HashMap::new(),
            has_untracked_cost: false,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn background_swap_discards_on_new_turn() {
        let sessions: SessionStore = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let session_id = "test-discard".to_string();
        let entry = make_test_entry();
        sessions
            .lock()
            .unwrap_or_log()
            .insert(session_id.clone(), entry);

        // Simulate a snapshot with turn_count = 1, then bump to 2.
        {
            let mut store = sessions.lock().unwrap_or_log();
            let entry = store.get_mut(&session_id).unwrap();
            entry.turn_count = 1;
            entry.messages.push(Message {
                role: "user".to_string(),
                content: "hello".to_string(),
                tool_calls: None,
                tool_results: None,
                turn: None,
            });
        }

        // Take a snapshot (marks in-flight).
        let snapshot = try_snapshot(&session_id, &sessions).unwrap();
        assert_eq!(snapshot.turn_count, 1);

        // Simulate a new turn arriving — bump turn_count and msg_len.
        {
            let mut store = sessions.lock().unwrap_or_log();
            let entry = store.get_mut(&session_id).unwrap();
            entry.turn_count = 2;
            entry.messages.push(Message {
                role: "assistant".to_string(),
                content: "world".to_string(),
                tool_calls: None,
                tool_results: None,
                turn: None,
            });
        }

        // Run compaction — should discard because turn_count changed.
        let config = Arc::new(Config::default());
        let result = run_compaction(&snapshot, &sessions, &config).await;
        assert!(result.is_ok());

        // Verify: compaction_in_flight is cleared, messages untouched.
        let store = sessions.lock().unwrap_or_log();
        let entry = store.get(&session_id).unwrap();
        assert!(!entry.compaction_in_flight);
        assert_eq!(entry.messages.len(), 2); // original 2 messages
        assert!(entry.pending_compaction_notice.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn spawn_is_noop_when_in_flight() {
        let sessions: SessionStore = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let session_id = "test-in-flight".to_string();
        let entry = make_test_entry();
        sessions
            .lock()
            .unwrap_or_log()
            .insert(session_id.clone(), entry);

        // Mark as in-flight.
        {
            let mut store = sessions.lock().unwrap_or_log();
            let entry = store.get_mut(&session_id).unwrap();
            entry.compaction_in_flight = true;
        }

        // spawn_compaction should be a no-op (doesn't spawn a task).
        // We verify by checking that the in-flight flag is unchanged.
        let config = Arc::new(Config::default());
        spawn_compaction(session_id.clone(), Arc::clone(&sessions), config);

        // Give the (non-existent) task a moment.
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        // Flag should still be true — no task touched it.
        let store = sessions.lock().unwrap_or_log();
        let entry = store.get(&session_id).unwrap();
        assert!(entry.compaction_in_flight);
    }

    #[test]
    fn notice_delivered_next_turn() {
        // Pure state test: entry with a queued notice; the drain site returns it
        // and clears the field.
        let sessions: SessionStore = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let session_id = "test-notice".to_string();
        let mut entry = make_test_entry();
        entry.pending_compaction_notice = Some("test notice".to_string());
        sessions
            .lock()
            .unwrap_or_log()
            .insert(session_id.clone(), entry);

        // Drain the notice.
        let notice = {
            let mut store = sessions.lock().unwrap_or_log();
            let entry = store.get_mut(&session_id).unwrap();
            entry.pending_compaction_notice.take()
        };

        assert_eq!(notice.as_deref(), Some("test notice"));

        // Verify the field is cleared.
        let store = sessions.lock().unwrap_or_log();
        let entry = store.get(&session_id).unwrap();
        assert!(entry.pending_compaction_notice.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn background_swap_applies_when_unchanged() {
        let _home = TestHome::new();
        let sessions: SessionStore = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let session_id = "swap-applies".to_string();

        let msgs = make_turn_msgs(32);
        let mut entry = make_test_entry();
        entry.messages = msgs.clone();
        entry.turn_count = 5;
        sessions
            .lock()
            .unwrap_or_log()
            .insert(session_id.clone(), entry);

        let snapshot = CompactionSnapshot {
            session_id: session_id.clone(),
            messages: msgs.clone(),
            turn_count: 5,
            msg_len: msgs.len(),
            // Huge scale forces a budget cut regardless of the default context
            // window, so the MIN_TAIL floor lands the cut at len-4.
            token_scale: 1e9,
        };

        let result = run_compaction(&snapshot, &sessions, &hermetic_config()).await;
        assert!(result.is_ok());

        let store = sessions.lock().unwrap_or_log();
        let entry = store.get(&session_id).unwrap();
        assert!(!entry.compaction_in_flight, "in-flight flag cleared");
        assert!(
            entry.messages.len() < 32,
            "history compacted, got {}",
            entry.messages.len()
        );
        assert!(
            entry.pending_compaction_notice.is_some(),
            "notice queued for next turn"
        );
        let recorded = epochs::read_epochs(&session_id);
        assert_eq!(recorded.len(), 1, "exactly one epoch recorded");
        assert!(
            recorded[0].narrative.is_none(),
            "narrative_enabled = false → no narrative"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn epoch_build_idempotent_after_discard() {
        let _home = TestHome::new();
        let sessions: SessionStore = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let session_id = "idempotent".to_string();

        let msgs = make_turn_msgs(32);
        let mut entry = make_test_entry();
        entry.messages = msgs.clone();
        entry.turn_count = 5;
        sessions
            .lock()
            .unwrap_or_log()
            .insert(session_id.clone(), entry);

        let snapshot = CompactionSnapshot {
            session_id: session_id.clone(),
            messages: msgs.clone(),
            turn_count: 5,
            msg_len: msgs.len(),
            token_scale: 1e9,
        };
        let config = hermetic_config();

        // First build: appends epoch #1 and swaps.
        run_compaction(&snapshot, &sessions, &config).await.unwrap();
        assert_eq!(epochs::read_epochs(&session_id).len(), 1);

        // Restore the entry so the staleness check would PASS — isolating the
        // idempotency guard as the sole reason a second epoch is not created.
        {
            let mut store = sessions.lock().unwrap_or_log();
            let e = store.get_mut(&session_id).unwrap();
            e.messages = msgs.clone();
            e.turn_count = 5;
            e.compaction_in_flight = false;
        }

        // Second build over the same snapshot: the guard must skip it.
        run_compaction(&snapshot, &sessions, &config).await.unwrap();
        assert_eq!(
            epochs::read_epochs(&session_id).len(),
            1,
            "no duplicate epoch after a re-run over the same snapshot"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn swap_discards_on_evicted_entry() {
        let _home = TestHome::new();
        let sessions: SessionStore = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let session_id = "evicted".to_string();

        let msgs = make_turn_msgs(32);
        let mut entry = make_test_entry();
        entry.messages = msgs.clone();
        entry.turn_count = 5;
        sessions
            .lock()
            .unwrap_or_log()
            .insert(session_id.clone(), entry);

        let snapshot = CompactionSnapshot {
            session_id: session_id.clone(),
            messages: msgs.clone(),
            turn_count: 5,
            msg_len: msgs.len(),
            token_scale: 1e9,
        };

        // Evict the entry before the swap. run_compaction builds the epoch on
        // the owned snapshot, then finds the entry gone at swap time.
        sessions.lock().unwrap_or_log().remove(&session_id);

        let result = run_compaction(&snapshot, &sessions, &hermetic_config()).await;
        assert!(result.is_ok(), "clean discard, no panic");
        assert!(
            sessions.lock().unwrap_or_log().get(&session_id).is_none(),
            "evicted entry stays gone"
        );
    }

    #[test]
    fn emergency_path_skips_narrative_with_flag_on() {
        // The extreme-pressure (synchronous) path never builds a narrative,
        // even when the operator enabled it — the phase-08 negative case.
        assert!(!epoch_narrative_allowed(true, true));
        assert!(!epoch_narrative_allowed(true, false));
        // The background path respects the flag.
        assert!(epoch_narrative_allowed(false, true));
        assert!(!epoch_narrative_allowed(false, false));
    }
}

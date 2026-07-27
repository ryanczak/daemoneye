//! Synchronous, model-call-free working-set control for autonomous (ghost)
//! sessions.
//!
//! Ghost sessions do not benefit from the interactive compaction ladder
//! (which is async and makes model calls). This module provides a pure
//! synchronous guard that elides oversized tool results and, above the
//! compact threshold, builds a structured-only epoch (no narrative call)
//! and cuts the working set. No `.await`, no network, no model call.

use crate::ai::Message;
use crate::config::Config;

/// Synchronous, model-call-free working-set control for autonomous (ghost)
/// sessions.
///
/// Ladder (estimate-based pressure only; ghosts are not token-calibrated):
///   token_pct >= compact_at_pct  → aggressive elision + structured-only epoch
///                                  (narrative = None) + compact. NO rollup
///                                  (maybe_rollup can make a model call).
///   token_pct >= elide_at_pct    → soft elision only.
///   below elide_at_pct, or below DIGEST_THRESHOLD messages → strict no-op.
///
/// Returns the possibly-compacted vec and a `compacted` flag (true means
/// the caller must rewrite the working session file).
pub fn enforce_ghost_working_set(
    session_id: &str,
    messages: Vec<Message>,
    token_scale: f64,
    started_at: chrono::DateTime<chrono::Utc>,
    context_window: u32,
    config: &Config,
) -> (Vec<Message>, bool) {
    let token_pct = if context_window > 0 {
        ((crate::daemon::context::estimate::estimate_history_tokens(&messages) as f64
            * token_scale)
            / context_window as f64
            * 100.0) as u32
    } else {
        0
    };
    let above_floor = messages.len() >= crate::daemon::digest::DIGEST_THRESHOLD;
    if !above_floor || token_pct < config.compaction.elide_at_pct {
        return (messages, false);
    }

    let mut messages = messages;
    if token_pct >= config.compaction.compact_at_pct {
        crate::daemon::digest::elide_old_tool_results(&mut messages, true);
        let budget = (context_window as u64 * config.compaction.target_pct as u64) / 100;
        let tail_start =
            crate::daemon::digest::planned_tail_start_by_budget(&messages, budget, token_scale)
                .or_else(|| {
                    crate::daemon::digest::synthesized_tail_start(&messages, budget, token_scale)
                });
        if let Some(ts) = tail_start {
            let prior = crate::daemon::context::epochs::read_epochs(session_id);
            let span_start = prior.last().map(|e| e.ts_end).unwrap_or(started_at);
            let span_end = chrono::Utc::now();
            let dropped = &messages[..ts];
            let record = crate::daemon::context::epochs::EpochRecord {
                seq: prior.last().map(|e| e.seq + 1).unwrap_or(1),
                kind: "epoch".into(),
                turn_start: crate::daemon::context::epochs::first_turn_of(dropped),
                turn_end: crate::daemon::context::epochs::last_turn_of(dropped),
                ts_start: span_start,
                ts_end: span_end,
                msg_count: dropped.len() as u32,
                narrative: None,
                tally: crate::daemon::context::epochs::tally_span(session_id, span_start, span_end),
                artifacts: crate::daemon::context::epochs::scan_artifacts_span(
                    span_start, span_end,
                ),
                covers: None,
            };
            crate::daemon::context::epochs::append_epoch(session_id, &record);
            let chain = crate::daemon::context::epochs::read_epochs(session_id);
            let rendered = crate::daemon::context::epochs::render_context_block(&chain);
            let env = config.context.environment.clone();
            let host = crate::daemon::utils::daemon_hostname();
            messages = crate::daemon::context::epochs::compact_with_epochs(
                messages,
                &rendered,
                &env,
                &host,
                record.turn_end as usize,
                0,
                ts,
            );
            if 2 < messages.len() {
                crate::daemon::digest::repair_tail_head(&mut messages[2..]);
            }
            return (messages, true);
        }
        // No viable cut — the aggressive elision above still changed content
        // in place.
        return (messages, true);
    }

    // Elide-only tier.
    let elided = crate::daemon::digest::elide_old_tool_results(&mut messages, false);
    (messages, elided > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::ToolResult;
    use crate::config::ContextConfig;

    /// RAII test-home guard: holds the `TEST_HOME_LOCK`, points `HOME` at a
    /// fresh tempdir, and restores the original `HOME` on drop.
    struct TestHome {
        _tmp: tempfile::TempDir,
        _lock: std::sync::MutexGuard<'static, ()>,
        saved_home: Option<String>,
    }

    impl Drop for TestHome {
        fn drop(&mut self) {
            unsafe {
                match &self.saved_home {
                    Some(h) => std::env::set_var("HOME", h),
                    None => std::env::remove_var("HOME"),
                }
            }
        }
    }

    fn setup_test_env() -> TestHome {
        let lock = crate::test_home_guard();
        let saved_home = std::env::var("HOME").ok();
        let tmp = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("HOME", tmp.path());
        }
        TestHome {
            _tmp: tmp,
            _lock: lock,
            saved_home,
        }
    }

    fn make_config() -> Config {
        Config {
            context: ContextConfig::default(),
            ..Config::default()
        }
    }

    fn make_msg(content: &str) -> Message {
        Message {
            role: "user".to_string(),
            content: content.to_string(),
            tool_calls: None,
            tool_results: None,
            turn: None,
        }
    }

    fn make_msg_with_tool_results(role: &str, content: &str, results: Vec<ToolResult>) -> Message {
        Message {
            role: role.to_string(),
            content: content.to_string(),
            tool_calls: None,
            tool_results: Some(results),
            turn: None,
        }
    }

    fn make_tool_result(name: &str, content: &str) -> ToolResult {
        ToolResult {
            tool_call_id: "test-call-id".to_string(),
            tool_name: name.to_string(),
            content: content.to_string(),
        }
    }

    fn make_history(n: usize) -> Vec<Message> {
        (0..n)
            .map(|i| {
                make_msg_with_tool_results(
                    "user",
                    &format!("msg {i}"),
                    vec![make_tool_result("read_file", &"x".repeat(5000))],
                )
            })
            .collect()
    }

    fn assert_no_orphan_tool_results(messages: &[Message]) {
        let mut saw_tool_calls = false;
        for msg in messages {
            if msg.tool_calls.as_ref().is_some_and(|c| !c.is_empty()) {
                saw_tool_calls = true;
            }
            if msg.tool_results.as_ref().is_some_and(|r| !r.is_empty()) && !saw_tool_calls {
                panic!(
                    "Orphan tool_results found: message has results but no \
                     preceding tool_calls"
                );
            }
        }
    }

    #[test]
    fn ghost_guard_noop_below_threshold() {
        let _home = setup_test_env();
        let started_at = chrono::Utc::now();
        let messages = vec![make_msg("hello")];
        let config = make_config();

        let (result, compacted) = enforce_ghost_working_set(
            "test-session",
            messages.clone(),
            1.0,
            started_at,
            32768,
            &config,
        );

        assert!(!compacted, "Below threshold should be a strict no-op");
        assert_eq!(
            result.len(),
            messages.len(),
            "Message count should be unchanged"
        );
        assert_eq!(
            result[0].content, messages[0].content,
            "Content should be unchanged"
        );
        assert!(
            crate::daemon::context::epochs::read_epochs("test-session").is_empty(),
            "No epoch should be appended"
        );
    }

    #[test]
    fn ghost_guard_compacts_structured_only() {
        let _home = setup_test_env();
        let started_at = chrono::Utc::now();
        let messages = make_history(30);
        let input_len = messages.len();

        let mut config = make_config();
        config.compaction.compact_at_pct = 10;
        config.compaction.target_pct = 5;

        let (result, compacted) =
            enforce_ghost_working_set("compact-session", messages, 1.0, started_at, 1000, &config);

        assert!(compacted, "Should have compacted");
        assert!(
            result.len() < input_len,
            "Result ({}) should be shorter than input ({})",
            result.len(),
            input_len
        );

        let epochs = crate::daemon::context::epochs::read_epochs("compact-session");
        assert!(!epochs.is_empty(), "At least one epoch should be appended");
        let last = epochs.last().unwrap();
        assert!(
            last.narrative.is_none(),
            "Ghost epoch narrative should be None (structured-only)"
        );
        assert_eq!(last.kind, "epoch");

        assert_no_orphan_tool_results(&result);
    }

    #[test]
    fn ghost_guard_output_orphan_free() {
        let _home = setup_test_env();
        let started_at = chrono::Utc::now();
        let messages = make_history(25);

        let mut config = make_config();
        config.compaction.compact_at_pct = 10;
        config.compaction.target_pct = 5;

        let (result, compacted) =
            enforce_ghost_working_set("orphan-session", messages, 1.0, started_at, 1000, &config);

        assert!(compacted, "Should have compacted");
        assert_no_orphan_tool_results(&result);
    }

    #[test]
    fn ghost_guard_elide_only_tier() {
        let _home = setup_test_env();
        let started_at = chrono::Utc::now();
        let messages = make_history(25);

        let config = make_config();
        // Set a very large context_window so token_pct stays between
        // elide_at_pct (50) and compact_at_pct (60) — the default thresholds.
        // 25 msgs * ~1250 est tokens * 1.0 = ~31250 tokens.
        // 31250 / 50000 * 100 = 62.5% → above compact_at_pct (60).
        // 31250 / 60000 * 100 = 52% → above elide (50) but below compact (60).
        // So context_window = 60000 lands us in the elide-only band.
        let (_result, compacted) =
            enforce_ghost_working_set("elide-session", messages, 1.0, started_at, 60000, &config);

        // Pressure is above elide_at_pct (50) but below compact_at_pct (60),
        // so we get elide-only (aggressive elision, no epoch).
        assert!(compacted, "Elision should mark compacted=true");
        assert!(
            crate::daemon::context::epochs::read_epochs("elide-session").is_empty(),
            "No epoch should be appended in elide-only tier"
        );
    }
}

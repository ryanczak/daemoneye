/// Build the N15 catch-up brief from messages injected while the client was away.
///
/// `new_msgs` is the slice of messages added after detach.
/// `away_secs` is how long the client was gone.
/// `detach_time_utc` is the UTC wall-clock time of the detach, used to query
/// cost events from `events.jsonl` (Phase 7).
/// Returns `None` when the absence was too short or no relevant events occurred.
/// Validate that a pane_id received from an external hook matches the tmux
/// format `%<digits>` (e.g. `%0`, `%23`).  Rejects anything else so that
/// crafted hook payloads cannot inject escape sequences or unexpected strings
/// into the cache or broadcast channels.
pub(crate) fn is_valid_pane_id(id: &str) -> bool {
    id.starts_with('%') && id.len() > 1 && id[1..].bytes().all(|b| b.is_ascii_digit())
}

/// Build the N15 catch-up brief from messages injected while the client was away.
///
/// `new_msgs` is the slice of messages added after detach.
/// `away_secs` is how long the client was gone.
/// `detach_time_utc` is the UTC wall-clock time of the detach, used to query
/// cost events from `events.jsonl` (Phase 7).
/// Returns `None` when the absence was too short or no relevant events occurred.
pub(crate) fn build_catchup_brief(
    new_msgs: &[crate::ai::Message],
    away_secs: u64,
    detach_time_utc: Option<chrono::DateTime<chrono::Utc>>,
) -> Option<String> {
    // Skip if the user was away less than 30 s — too brief to be useful.
    if away_secs < 30 {
        return None;
    }

    let away_str = if away_secs < 60 {
        format!("{}s", away_secs)
    } else if away_secs < 3600 {
        format!("{}m", away_secs / 60)
    } else {
        format!("{}h{}m", away_secs / 3600, (away_secs % 3600) / 60)
    };

    // Scan for injected event messages the AI adds to session history.
    let events: Vec<String> = new_msgs
        .iter()
        .filter_map(|m| {
            let c = &m.content;
            if c.contains("[Background Task Completed")
                || c.contains("[Webhook Alert]")
                || c.contains("[Watchdog]")
                || c.contains("[Watch Pane")
                || c.contains("[Ghost Shell Started]")
                || c.contains("[Ghost Shell Completed]")
                || c.contains("[Ghost Shell Failed]")
            {
                // Extract just the first line as a terse summary.
                let first_line = c.lines().next().unwrap_or(c.as_str()).trim();
                Some(first_line.to_string())
            } else {
                None
            }
        })
        .collect();

    // Compute cost during the detach window (Phase 7).
    let cost_line = detach_time_utc.and_then(|detach_time| {
        let now = chrono::Utc::now();
        let summary = crate::daemon::utils::sum_cost_between(detach_time, now);
        if summary.call_count == 0 {
            // No AI calls during detach — omit cost line entirely.
            return None;
        }
        let total = summary.total_cost_usd;
        let marker = if summary.has_untracked { "+" } else { "" };
        let agent_detail = if total < 0.001 {
            // All costs are zero (local providers only).
            "local providers only".to_string()
        } else {
            summary
                .by_agent
                .iter()
                .map(|(name, cost)| format!("{} ${:.2}", name, cost))
                .collect::<Vec<_>>()
                .join(" · ")
        };
        Some(format!(
            "Cost during detach: ${:.2}{} ({})",
            total, marker, agent_detail
        ))
    });

    let has_events = !events.is_empty();
    let has_cost = cost_line.is_some();

    if !has_events && !has_cost {
        return None;
    }

    let count = events.len();
    let mut parts = Vec::new();

    if has_events {
        let lines = events
            .iter()
            .map(|e| format!("  • {}", e))
            .collect::<Vec<_>>()
            .join("\n");
        parts.push(format!(
            "[Catch-up] {} event{} while you were away ({}):\n{}",
            count,
            if count == 1 { "" } else { "s" },
            away_str,
            lines,
        ));
        if let Some(cost) = cost_line {
            parts.push(format!("  • {}", cost));
        }
    } else if let Some(cost) = cost_line {
        parts.push(format!(
            "[Catch-up] AI activity while you were away ({}):\n  • {}",
            away_str, cost
        ));
    }

    Some(parts.join("\n"))
}

#[cfg(test)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::Message;

    fn msg(content: &str) -> Message {
        Message {
            role: "user".to_string(),
            content: content.to_string(),
            tool_calls: None,
            tool_results: None,
            turn: None,
        }
    }

    // ── build_catchup_brief ───────────────────────────────────────────────────

    #[test]
    fn catchup_brief_none_when_away_less_than_30s() {
        let msgs = vec![msg("[Background Task Completed] deploy finished")];
        assert!(build_catchup_brief(&msgs, 29, None).is_none());
    }

    #[test]
    fn catchup_brief_none_when_no_new_messages() {
        assert!(build_catchup_brief(&[], 120, None).is_none());
    }

    #[test]
    fn catchup_brief_none_when_no_matching_events() {
        let msgs = vec![
            msg("User: what is load avg?"),
            msg("The load average is 0.5"),
        ];
        assert!(build_catchup_brief(&msgs, 120, None).is_none());
    }

    #[test]
    fn catchup_brief_detects_background_task() {
        let msgs = vec![msg(
            "[Background Task Completed] apt upgrade finished (exit 0)",
        )];
        let brief = build_catchup_brief(&msgs, 60, None).expect("should produce a brief");
        assert!(brief.contains("[Catch-up]"), "missing header: {brief}");
        assert!(
            brief.contains("[Background Task Completed]"),
            "missing event: {brief}"
        );
        assert!(brief.contains("1m"), "wrong away time: {brief}");
    }

    #[test]
    fn catchup_brief_detects_webhook_alert() {
        let msgs = vec![msg("[Webhook Alert] Disk usage at 92% on web01")];
        let brief = build_catchup_brief(&msgs, 3600, None).expect("should produce a brief");
        assert!(brief.contains("[Webhook Alert]"), "missing event: {brief}");
        assert!(brief.contains("1h0m"), "wrong away time: {brief}");
    }

    #[test]
    fn catchup_brief_detects_watchdog() {
        let msgs = vec![msg("[Watchdog] nginx: 5xx rate above threshold")];
        let brief = build_catchup_brief(&msgs, 90, None).expect("should produce a brief");
        assert!(brief.contains("[Watchdog]"), "missing event: {brief}");
        assert!(brief.contains("1m"), "wrong away time: {brief}");
    }

    #[test]
    fn catchup_brief_detects_watch_pane() {
        let msgs = vec![msg("[Watch Pane %3] pattern 'ready' matched after 45s")];
        let brief = build_catchup_brief(&msgs, 120, None).expect("should produce a brief");
        assert!(brief.contains("[Watch Pane"), "missing event: {brief}");
    }

    #[test]
    fn catchup_brief_counts_events_correctly() {
        let msgs = vec![
            msg("[Background Task Completed] job1 (exit 0)"),
            msg("User: check this"),
            msg("[Webhook Alert] CPU spike on prod"),
            msg("[Background Task Completed] job2 (exit 1)"),
        ];
        let brief = build_catchup_brief(&msgs, 200, None).expect("should produce a brief");
        assert!(brief.contains("3 events"), "expected count 3: {brief}");
    }

    #[test]
    fn catchup_brief_singular_event_label() {
        let msgs = vec![msg("[Webhook Alert] single alert")];
        let brief = build_catchup_brief(&msgs, 60, None).expect("should produce a brief");
        assert!(brief.contains("1 event "), "expected singular: {brief}");
        assert!(!brief.contains("1 events"), "should be singular: {brief}");
    }

    #[test]
    fn catchup_brief_extracts_first_line_only() {
        let msgs = vec![msg(
            "[Background Task Completed] job done\nFull output:\nline 1\nline 2",
        )];
        let brief = build_catchup_brief(&msgs, 60, None).expect("should produce a brief");
        // Only the first line should appear as the bullet
        assert!(
            brief.contains("[Background Task Completed] job done"),
            "missing first line: {brief}"
        );
        assert!(
            !brief.contains("Full output:"),
            "should not include subsequent lines: {brief}"
        );
    }

    #[test]
    fn catchup_brief_away_time_hours_minutes() {
        let msgs = vec![msg("[Watchdog] alert")];
        let brief = build_catchup_brief(&msgs, 7260, None).expect("should produce a brief");
        // 7260 s = 2h1m
        assert!(brief.contains("2h1m"), "expected 2h1m: {brief}");
    }

    // ── Phase 7: catch-up brief cost integration ──────────────────────────────

    #[test]
    fn catchup_brief_includes_cost_when_ghosts_ran() {
        let _lock = crate::test_home_guard();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };

        let events_path = crate::config::events_path();
        std::fs::create_dir_all(events_path.parent().unwrap()).unwrap();

        let one_hour_ago = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        let thirty_min_ago = (chrono::Utc::now() - chrono::Duration::minutes(30)).to_rfc3339();
        let line1 = format!(
            r#"{{"event":"ai_cost","ts":"{one_hour_ago}","session_id":"gs-1","agent_name":"architect","cost":{{"total_cost_usd":0.20}}}}"#
        );
        let line2 = format!(
            r#"{{"event":"ai_cost","ts":"{thirty_min_ago}","session_id":"gs-2","agent_name":"ghost-anonymous","cost":{{"total_cost_usd":0.14}}}}"#
        );
        std::fs::write(&events_path, format!("{}\n{}\n", line1, line2)).unwrap();

        let detach_time = chrono::Utc::now() - chrono::Duration::hours(2);
        let msgs = vec![msg("[Ghost Shell Completed] architect finished")];
        let brief =
            build_catchup_brief(&msgs, 7200, Some(detach_time)).expect("should produce a brief");
        assert!(
            brief.contains("Cost during detach:"),
            "missing cost line: {brief}"
        );
        assert!(brief.contains("$0.34"), "wrong total: {brief}");
        assert!(brief.contains("architect"), "missing agent: {brief}");
        assert!(
            brief.contains("ghost-anonymous"),
            "missing ghost agent: {brief}"
        );
    }

    #[test]
    fn catchup_brief_omits_cost_line_when_no_ai_calls() {
        let _lock = crate::test_home_guard();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };

        let events_path = crate::config::events_path();
        std::fs::create_dir_all(events_path.parent().unwrap()).unwrap();
        // Write a non-ai_cost event — should not trigger cost line.
        let ts = chrono::Utc::now().to_rfc3339();
        let line = format!(r#"{{"event":"command","ts":"{ts}","session":"s1","cmd":"ls"}}"#);
        std::fs::write(&events_path, format!("{}\n", line)).unwrap();

        let detach_time = chrono::Utc::now() - chrono::Duration::hours(1);
        let msgs = vec![msg("[Background Task Completed] job done")];
        let brief =
            build_catchup_brief(&msgs, 3600, Some(detach_time)).expect("should produce a brief");
        assert!(
            !brief.contains("Cost during detach:"),
            "should omit cost line when no ai_cost events: {brief}"
        );
    }

    #[test]
    fn catchup_brief_local_only_shows_zero_explicitly() {
        let _lock = crate::test_home_guard();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };

        let events_path = crate::config::events_path();
        std::fs::create_dir_all(events_path.parent().unwrap()).unwrap();

        // Local provider call with zero cost.
        let ts = (chrono::Utc::now() - chrono::Duration::minutes(30)).to_rfc3339();
        let line = format!(
            r#"{{"event":"ai_cost","ts":"{ts}","session_id":"gs-local","agent_name":"chat","cost":{{"total_cost_usd":0.0}}}}"#
        );
        std::fs::write(&events_path, format!("{}\n", line)).unwrap();

        let detach_time = chrono::Utc::now() - chrono::Duration::hours(1);
        let msgs = vec![msg("[Ghost Shell Completed] local job done")];
        let brief =
            build_catchup_brief(&msgs, 3600, Some(detach_time)).expect("should produce a brief");
        assert!(
            brief.contains("Cost during detach:"),
            "should show cost line: {brief}"
        );
        assert!(brief.contains("$0.00"), "should show zero cost: {brief}");
        assert!(
            brief.contains("local providers only"),
            "should indicate local providers: {brief}"
        );
    }

    #[test]
    fn catchup_brief_marks_untracked_spend() {
        let _lock = crate::test_home_guard();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };

        let events_path = crate::config::events_path();
        std::fs::create_dir_all(events_path.parent().unwrap()).unwrap();

        // Unknown pricing source — cost is zero but should be flagged.
        let ts = (chrono::Utc::now() - chrono::Duration::minutes(30)).to_rfc3339();
        let line = format!(
            r#"{{"event":"ai_cost","ts":"{ts}","session_id":"s1","agent_name":"chat","cost":{{"total_cost_usd":0.0}},"pricing_source":"Unknown"}}"#
        );
        std::fs::write(&events_path, format!("{}\n", line)).unwrap();

        let detach_time = chrono::Utc::now() - chrono::Duration::hours(1);
        let msgs = vec![msg("[Background Task Completed] job done")];
        let brief =
            build_catchup_brief(&msgs, 3600, Some(detach_time)).expect("should produce a brief");
        assert!(
            brief.contains("$0.00+"),
            "should have + marker for untracked: {brief}"
        );
    }

    #[test]
    fn catchup_brief_cost_only_has_header_when_no_events() {
        let _lock = crate::test_home_guard();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };

        let events_path = crate::config::events_path();
        std::fs::create_dir_all(events_path.parent().unwrap()).unwrap();

        // AI cost event during the detach window.
        let ts = (chrono::Utc::now() - chrono::Duration::minutes(30)).to_rfc3339();
        let line = format!(
            r#"{{"event":"ai_cost","ts":"{ts}","session_id":"gs-1","agent_name":"architect","cost":{{"total_cost_usd":0.34}}}}"#
        );
        std::fs::write(&events_path, format!("{}\n", line)).unwrap();

        // No injected event messages — only cost.
        let detach_time = chrono::Utc::now() - chrono::Duration::hours(1);
        let msgs: Vec<crate::ai::Message> = vec![];
        let brief =
            build_catchup_brief(&msgs, 3600, Some(detach_time)).expect("should produce a brief");
        assert!(
            brief.contains("[Catch-up] AI activity while you were away"),
            "should have header when cost-only: {brief}"
        );
        assert!(
            brief.contains("Cost during detach:"),
            "should have cost line: {brief}"
        );
        assert!(
            brief.contains("architect"),
            "should show agent name: {brief}"
        );
    }

    #[test]
    fn sum_cost_between_excludes_events_outside_window() {
        let _lock = crate::test_home_guard();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };

        let events_path = crate::config::events_path();
        std::fs::create_dir_all(events_path.parent().unwrap()).unwrap();

        let now = chrono::Utc::now();
        let outside_before = (now - chrono::Duration::hours(3)).to_rfc3339();
        let inside = (now - chrono::Duration::hours(1)).to_rfc3339();
        let outside_after = (now + chrono::Duration::hours(1)).to_rfc3339();

        let line_before = format!(
            r#"{{"event":"ai_cost","ts":"{outside_before}","session_id":"s1","agent_name":"chat","cost":{{"total_cost_usd":9.99}}}}"#
        );
        let line_inside = format!(
            r#"{{"event":"ai_cost","ts":"{inside}","session_id":"s1","agent_name":"chat","cost":{{"total_cost_usd":0.50}}}}"#
        );
        let line_after = format!(
            r#"{{"event":"ai_cost","ts":"{outside_after}","session_id":"s1","agent_name":"chat","cost":{{"total_cost_usd":8.88}}}}"#
        );
        std::fs::write(
            &events_path,
            format!("{}\n{}\n{}\n", line_before, line_inside, line_after),
        )
        .unwrap();

        let from = now - chrono::Duration::hours(2);
        let to = now;
        let summary = crate::daemon::utils::sum_cost_between(from, to);

        assert!(
            (summary.total_cost_usd - 0.50).abs() < 1e-10,
            "should only include inside-window event, got {}",
            summary.total_cost_usd
        );
        assert_eq!(summary.call_count, 1, "should have exactly 1 call");
    }

    // ── is_valid_pane_id ──────────────────────────────────────────────────────

    #[test]
    fn valid_pane_ids_accepted() {
        assert!(is_valid_pane_id("%0"));
        assert!(is_valid_pane_id("%1"));
        assert!(is_valid_pane_id("%23"));
        assert!(is_valid_pane_id("%999"));
    }

    #[test]
    fn invalid_pane_ids_rejected() {
        assert!(!is_valid_pane_id(""));
        assert!(!is_valid_pane_id("%")); // no digits
        assert!(!is_valid_pane_id("0")); // no leading %
        assert!(!is_valid_pane_id("%0a")); // non-digit character
        assert!(!is_valid_pane_id("%23\x1b[31m")); // ANSI escape injection
        assert!(!is_valid_pane_id("%;rm -rf /")); // shell injection attempt
    }
}

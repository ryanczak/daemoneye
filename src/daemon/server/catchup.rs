/// Validate that a pane_id received from an external hook matches the tmux
/// format `%<digits>` (e.g. `%0`, `%23`).  Rejects anything else so that
/// crafted hook payloads cannot inject escape sequences or unexpected strings
/// into the cache or broadcast channels.
pub(crate) fn is_valid_pane_id(id: &str) -> bool {
    id.starts_with('%') && id.len() > 1 && id[1..].bytes().all(|b| b.is_ascii_digit())
}

/// The notice lines the catch-up brief lists, in journal order: a notice
/// survives when it is addressed to this session (or to every session) and
/// either the history sink or the chat-notice sink routes it under the
/// running config. The defaults are the ones the daemon's sinks subscribe
/// with at the fixed `warning` threshold. A notice both sinks route appears
/// once. Each line is the notice's one-line form; a `Scheduler` notice's
/// title carries no tag, so its line is prefixed `[Scheduled] <name>: `.
fn brief_lines<'a, I>(
    notices: I,
    session_id: &str,
    cfg: &crate::config::NotificationsConfig,
) -> Vec<String>
where
    I: IntoIterator<Item = &'a crate::bus::Notice>,
{
    let routes = crate::daemon::notice_sinks::default_routes(crate::bus::Severity::Warning);
    let mut lines = Vec::new();
    for notice in notices {
        if notice.session.as_ref().is_some_and(|s| s != session_id) {
            continue;
        }
        let routed_history = crate::daemon::notice_sinks::sink_routed(
            cfg,
            &routes.history,
            crate::config::RouteSink::History,
            notice,
        );
        let routed_chat = crate::daemon::notice_sinks::sink_routed(
            cfg,
            &routes.chat_notice,
            crate::config::RouteSink::ChatNotice,
            notice,
        );
        if !routed_history && !routed_chat {
            continue;
        }
        let line = if notice.source == crate::bus::Source::Scheduler {
            format!("[Scheduled] {}: {}", notice.name, notice.line())
        } else {
            notice.line().to_string()
        };
        lines.push(line);
    }
    lines
}

/// Build the N15 catch-up brief from the notices the journal holds for
/// `[detach, now)` that were routed to this chat's history or to the chat
/// notice while the client was away.
///
/// `session_id` is the attaching chat's session. `away_secs` is how long
/// the client was gone. `detach_time_utc` is the UTC wall-clock time of the
/// detach, bounding both the notice window and the cost events read from
/// `events.jsonl` (Phase 7). With no `detach_time_utc` no notices are
/// listed.
/// Returns `None` when the absence was too short or no relevant events occurred.
pub(crate) fn build_catchup_brief(
    session_id: &str,
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

    // Read the notices the journal holds for `[detach, now)`. With no
    // detach time there is no window, and nothing is listed: an open start
    // would read the whole journal.
    let now = chrono::Utc::now();
    let mut notices: Vec<crate::bus::Notice> = Vec::new();
    if let Some(detach) = detach_time_utc {
        crate::bus::for_each_notice_between(Some(detach), Some(now), &mut |n| {
            notices.push(n.clone());
        });
    }
    let cfg = crate::config::current().notifications.clone();
    let events = brief_lines(&notices, session_id, &cfg);

    // Compute cost during the detach window (Phase 7).
    let cost_line = detach_time_utc.and_then(|detach_time| {
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
mod tests {
    use super::*;
    use crate::bus::{Notice, NoticeStatus, Severity, Source};
    use std::io::Write;

    /// A webhook alert notice shaped as `process_alert` writes it.
    fn alert_notice(name: &str, severity: Severity, status: NoticeStatus) -> Notice {
        Notice::new(
            severity,
            Source::Webhook,
            name,
            format!("[Webhook Alert] {name}"),
        )
        .source_id("alertmanager")
        .status(status)
        .fingerprint(format!("fp-{name}"))
        .label("severity", severity.as_str())
        .label("kind", "alert")
    }

    /// A `[Webhook Analysis]` notice shaped as `analysis_notice` writes it:
    /// the raw label's severity, status `Event`, `kind` `analysis`.
    fn analysis_notice(name: &str, severity: Severity) -> Notice {
        Notice::new(
            severity,
            Source::Webhook,
            name,
            format!("[Webhook Analysis] {name}: analysis"),
        )
        .source_id("alertmanager")
        .status(NoticeStatus::Event)
        .fingerprint(format!("fp-{name}"))
        .label("severity", severity.as_str())
        .label("kind", "analysis")
    }

    /// A `Scheduler` job-result notice shaped as `notify_job_result` writes it.
    fn scheduler_notice(job: &str, title: &str) -> Notice {
        Notice::new(Severity::Info, Source::Scheduler, job, title)
            .source_id(format!("job-{job}"))
            .label("kind", "script")
    }

    /// A `Ghost` notice shaped as `inject_ghost_event` writes it.
    fn ghost_notice(name: &str, title: &str) -> Notice {
        Notice::new(Severity::Info, Source::Ghost, name, title)
            .source_id("ghost-1")
            .label("kind", "started")
    }

    /// A `Background` notice shaped as `background_notice` writes it, with
    /// the `[Background Task Completed]` tag in the title.
    fn background_notice(session: Option<&str>, title: &str) -> Notice {
        let mut n = Notice::new(Severity::Info, Source::Background, "run-1", title)
            .source_id("run-1")
            .label("kind", "completed");
        if let Some(s) = session {
            n = n.session(s);
        }
        n
    }

    /// A `Watchdog` notice shaped as the watchdog producer writes it.
    fn watchdog_notice(name: &str, title: &str) -> Notice {
        Notice::new(Severity::Warning, Source::Watchdog, name, title)
            .source_id("watchdog-1")
            .label("kind", "watchdog")
    }

    /// A `Shell` notice shaped as the terminal-state producer writes it.
    fn shell_notice(name: &str, title: &str) -> Notice {
        Notice::new(Severity::Info, Source::Shell, name, title)
            .source_id("shell-1")
            .label("kind", "state")
    }

    /// The serialized-notice journal record `Bus::new` writes, with a pinned
    /// `ts`, `event: "notice"` and `pid` — the shape
    /// `for_each_notice_between` parses. The record's `ts` is the notice's
    /// own, so a notice whose instant matters is pinned with `ts_refire`
    /// before this runs.
    fn journal_record(notice: &Notice) -> serde_json::Value {
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

    /// Append one journal record with a pinned `ts` into the segment its own
    /// date names — the seam `for_each_notice_between` reads, mirroring how
    /// `Bus::new`'s journal records land in the segment of their day.
    fn seed_notice(notice: &Notice, ts: chrono::DateTime<chrono::Utc>) {
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
            serde_json::to_string(&journal_record(notice)).expect("serializing record")
        )
        .expect("appending");
    }

    /// A `test_home` environment with the config dirs ensured, so journal
    /// seeding and `config::current()` read the test's sandbox.
    fn home() -> (crate::TestHomeGuard, tempfile::TempDir) {
        let (guard, dir) = crate::test_home();
        (guard, dir)
    }

    /// A fixed window: `from` two hours before `now`, pinned off the epoch so
    /// journal segments are date-stable across runs.
    fn window() -> (chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>) {
        let from = chrono::DateTime::from_timestamp(1_700_000_000, 0).expect("valid instant");
        let now = from + chrono::Duration::hours(2);
        (from, now)
    }

    // ── brief_lines: selection by route ───────────────────────────────────────

    #[test]
    fn catchup_lists_what_history_and_chat_notice_route() {
        let cfg = crate::config::NotificationsConfig::default();
        let base = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let ts = |mins: i64| base + chrono::Duration::minutes(mins);
        let alerts = vec![
            alert_notice("disk-high", Severity::Warning, NoticeStatus::Firing),
            ghost_notice("g-1", "[Ghost Shell Started] architect"),
            background_notice(Some("sess-1"), "[Background Task Completed] job1 (exit 0)"),
            scheduler_notice("backup", "`backup.sh` finished (exit 0)"),
            watchdog_notice("nginx", "[Watchdog] nginx: 5xx rate above threshold"),
        ];
        let lines = brief_lines(&alerts, "sess-1", &cfg);
        assert_eq!(
            lines,
            vec![
                "[Webhook Alert] disk-high".to_string(),
                "[Ghost Shell Started] architect".to_string(),
                "[Background Task Completed] job1 (exit 0)".to_string(),
                "[Scheduled] backup: `backup.sh` finished (exit 0)".to_string(),
                "[Watchdog] nginx: 5xx rate above threshold".to_string(),
            ],
            "the routed notices are listed in journal order, each once"
        );
        assert_eq!(lines.len(), 5, "one line per listed notice: {lines:?}");
        let _ = ts(0);
    }

    #[test]
    fn catchup_skips_what_no_route_reaches_and_other_sessions() {
        let cfg = crate::config::NotificationsConfig::default();
        let not_routed = vec![
            alert_notice("heartbeat", Severity::Info, NoticeStatus::Firing),
            analysis_notice("anomaly", Severity::Info),
            shell_notice("sh-1", "[Shell State] idle"),
            background_notice(Some("sess-9"), "[Background Task Completed] other (exit 0)"),
        ];
        let lines = brief_lines(&not_routed, "sess-1", &cfg);
        assert!(
            lines.is_empty(),
            "an info webhook alert, a shell notice and another session's \
             background notice are not listed: {lines:?}"
        );
    }

    #[test]
    fn catchup_honours_configured_routes() {
        let cfg = crate::config::NotificationsConfig {
            default_routes: false,
            route: vec![crate::config::RouteConfig {
                sink: crate::config::RouteSink::History,
                command: None,
                min_severity: None,
                sources: vec![Source::Shell],
                names: Vec::new(),
                statuses: Vec::new(),
            }],
        };
        let notices = vec![
            shell_notice("sh-1", "[Shell State] idle"),
            alert_notice("disk-high", Severity::Warning, NoticeStatus::Firing),
        ];
        let lines = brief_lines(&notices, "sess-1", &cfg);
        assert_eq!(
            lines,
            vec!["[Shell State] idle".to_string()],
            "only the configured shell route's notice is listed: {lines:?}"
        );
    }

    #[test]
    fn catchup_prefixes_scheduler_lines_with_the_job() {
        let cfg = crate::config::NotificationsConfig::default();
        let notices = vec![scheduler_notice(
            "tmp-cast-purge",
            "`tmp-cast-purge.sh` done",
        )];
        let lines = brief_lines(&notices, "sess-1", &cfg);
        assert_eq!(
            lines,
            vec!["[Scheduled] tmp-cast-purge: `tmp-cast-purge.sh` done".to_string()],
            "a scheduler line is prefixed with the job name: {lines:?}"
        );
    }

    // ── build_catchup_brief: the journal window and the delivery ─────────────

    #[test]
    fn catchup_brief_reads_the_journal_window() {
        let (_home, _home_dir) = home();
        let (from, now) = window();

        let before = alert_notice("before-detach", Severity::Warning, NoticeStatus::Firing)
            .ts_refire((from - chrono::Duration::minutes(5)).timestamp());
        let after = alert_notice("after-detach", Severity::Warning, NoticeStatus::Firing)
            .ts_refire((from + chrono::Duration::minutes(5)).timestamp());
        seed_notice(&before, before.ts);
        seed_notice(&after, after.ts);

        let brief = build_catchup_brief("sess-1", 7200, Some(from)).expect("a brief");
        assert!(
            brief.contains("[Catch-up] 1 event while you were away (2h0m):"),
            "the header carries the count and the away time: {brief}"
        );
        assert!(
            brief.contains("[Webhook Alert] after-detach"),
            "only the notice inside the window is listed: {brief}"
        );
        assert!(
            !brief.contains("before-detach"),
            "a notice journaled before the detach is not listed: {brief}"
        );
        let _ = now;
    }

    #[test]
    fn catchup_brief_none_when_away_less_than_30s() {
        assert!(build_catchup_brief("sess-1", 29, None).is_none());
    }

    #[test]
    fn catchup_brief_lists_nothing_without_a_detach_time() {
        let (_home, _home_dir) = home();
        // A routed alert sits in the journal: with no detach time there is no
        // window, so it must not be listed (an open start reads it all).
        let ts = chrono::Utc::now() - chrono::Duration::minutes(5);
        let alert = alert_notice("disk-high", Severity::Warning, NoticeStatus::Firing)
            .ts_refire(ts.timestamp());
        seed_notice(&alert, ts);
        assert!(build_catchup_brief("sess-1", 120, None).is_none());
    }

    #[test]
    fn catchup_brief_none_when_no_matching_events() {
        let (_home, _home_dir) = home();
        let (from, _) = window();
        let unrouted = alert_notice("heartbeat", Severity::Info, NoticeStatus::Firing)
            .ts_refire((from + chrono::Duration::minutes(1)).timestamp());
        seed_notice(&unrouted, unrouted.ts);
        assert!(
            build_catchup_brief("sess-1", 120, Some(from)).is_none(),
            "nothing routed in the window → no brief"
        );
    }

    #[test]
    fn catchup_brief_detects_background_task() {
        let (_home, _home_dir) = home();
        let (from, _) = window();
        let bg = background_notice(
            None,
            "[Background Task Completed] apt upgrade finished (exit 0)",
        )
        .ts_refire((from + chrono::Duration::minutes(1)).timestamp());
        seed_notice(&bg, bg.ts);

        let brief = build_catchup_brief("sess-1", 60, Some(from)).expect("a brief");
        assert!(brief.contains("[Catch-up]"), "missing header: {brief}");
        assert!(
            brief.contains("[Background Task Completed] apt upgrade finished (exit 0)"),
            "missing event: {brief}"
        );
        assert!(brief.contains("(1m)"), "wrong away time: {brief}");
    }

    #[test]
    fn catchup_brief_detects_webhook_alert() {
        let (_home, _home_dir) = home();
        let (from, _) = window();
        let alert = alert_notice("disk-high", Severity::Warning, NoticeStatus::Firing)
            .ts_refire((from + chrono::Duration::minutes(1)).timestamp());
        seed_notice(&alert, alert.ts);

        let brief = build_catchup_brief("sess-1", 3600, Some(from)).expect("a brief");
        assert!(
            brief.contains("[Webhook Alert] disk-high"),
            "missing event: {brief}"
        );
        assert!(brief.contains("(1h0m)"), "wrong away time: {brief}");
    }

    #[test]
    fn catchup_brief_detects_watchdog() {
        let (_home, _home_dir) = home();
        let (from, _) = window();
        let wd = watchdog_notice("nginx", "[Watchdog] nginx: 5xx rate above threshold")
            .ts_refire((from + chrono::Duration::minutes(1)).timestamp());
        seed_notice(&wd, wd.ts);

        let brief = build_catchup_brief("sess-1", 90, Some(from)).expect("a brief");
        assert!(brief.contains("[Watchdog] nginx"), "missing event: {brief}");
        assert!(brief.contains("(1m)"), "wrong away time: {brief}");
    }

    #[test]
    fn catchup_lists_webhook_analysis() {
        let (_home, _home_dir) = home();
        let (from, _) = window();
        let analysis = analysis_notice("HighCPU", Severity::Warning)
            .ts_refire((from + chrono::Duration::minutes(1)).timestamp());
        seed_notice(&analysis, analysis.ts);

        let brief = build_catchup_brief("sess-1", 90, Some(from)).expect("a brief");
        assert!(
            brief.contains("[Webhook Analysis] HighCPU: analysis"),
            "the analysis' first line is the summary: {brief}"
        );
    }

    #[test]
    fn catchup_brief_counts_events_correctly() {
        let (_home, _home_dir) = home();
        let (from, _) = window();
        let bg1 = background_notice(None, "[Background Task Completed] job1 (exit 0)")
            .ts_refire((from + chrono::Duration::minutes(1)).timestamp());
        let alert = alert_notice("cpu-hot", Severity::Warning, NoticeStatus::Firing)
            .ts_refire((from + chrono::Duration::minutes(2)).timestamp());
        let bg2 = background_notice(None, "[Background Task Completed] job2 (exit 1)")
            .ts_refire((from + chrono::Duration::minutes(3)).timestamp());
        for n in [&bg1, &alert, &bg2] {
            seed_notice(n, n.ts);
        }

        let brief = build_catchup_brief("sess-1", 200, Some(from)).expect("a brief");
        assert!(brief.contains("3 events"), "expected count 3: {brief}");
    }

    #[test]
    fn catchup_brief_singular_event_label() {
        let (_home, _home_dir) = home();
        let (from, _) = window();
        let alert = alert_notice("single", Severity::Warning, NoticeStatus::Firing)
            .ts_refire((from + chrono::Duration::minutes(1)).timestamp());
        seed_notice(&alert, alert.ts);

        let brief = build_catchup_brief("sess-1", 60, Some(from)).expect("a brief");
        assert!(brief.contains("1 event "), "expected singular: {brief}");
        assert!(!brief.contains("1 events"), "should be singular: {brief}");
    }

    #[test]
    fn catchup_brief_shows_only_the_first_line() {
        let (_home, _home_dir) = home();
        let (from, _) = window();
        let mut bg = background_notice(
            None,
            "[Background Task Completed] job done\nFull output:\nline 1\nline 2",
        );
        bg.body = "Full output:\nline 1\nline 2".to_string();
        let bg = bg.ts_refire((from + chrono::Duration::minutes(1)).timestamp());
        seed_notice(&bg, bg.ts);

        let brief = build_catchup_brief("sess-1", 60, Some(from)).expect("a brief");
        assert!(
            brief.contains("[Background Task Completed] job done"),
            "the title's first line is the bullet: {brief}"
        );
        assert!(
            !brief.contains("Full output:"),
            "the body's lines are not in the brief: {brief}"
        );
    }

    #[test]
    fn catchup_brief_away_time_hours_minutes() {
        let (_home, _home_dir) = home();
        let (from, _) = window();
        let wd = watchdog_notice("nginx", "[Watchdog] alert")
            .ts_refire((from + chrono::Duration::minutes(1)).timestamp());
        seed_notice(&wd, wd.ts);

        let brief = build_catchup_brief("sess-1", 7260, Some(from)).expect("a brief");
        assert!(brief.contains("(2h1m)"), "expected 2h1m: {brief}");
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
        let ghost = ghost_notice("architect", "[Ghost Shell Completed] architect finished");
        seed_notice(&ghost, ghost.ts);
        let brief =
            build_catchup_brief("sess-1", 7200, Some(detach_time)).expect("should produce a brief");
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
        let bg = background_notice(None, "[Background Task Completed] job done")
            .ts_refire((detach_time + chrono::Duration::minutes(1)).timestamp());
        seed_notice(&bg, bg.ts);
        let brief =
            build_catchup_brief("sess-1", 3600, Some(detach_time)).expect("should produce a brief");
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
        let ghost = ghost_notice("local", "[Ghost Shell Completed] local job done");
        seed_notice(&ghost, ghost.ts);
        let brief =
            build_catchup_brief("sess-1", 3600, Some(detach_time)).expect("should produce a brief");
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
        let bg = background_notice(None, "[Background Task Completed] job done")
            .ts_refire((detach_time + chrono::Duration::minutes(1)).timestamp());
        seed_notice(&bg, bg.ts);
        let brief =
            build_catchup_brief("sess-1", 3600, Some(detach_time)).expect("should produce a brief");
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

        // No notices in the window — only cost.
        let detach_time = chrono::Utc::now() - chrono::Duration::hours(1);
        let brief =
            build_catchup_brief("sess-1", 3600, Some(detach_time)).expect("should produce a brief");
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

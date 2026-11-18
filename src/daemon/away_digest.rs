//! The away digest (M37 phase 08, GitHub issue #5): one summary of what
//! happened while no chat was attached, delivered once to the first chat to
//! attach after a gap.
//!
//! Two pieces of state bound the window. The **last-seen stamp** — one file,
//! `var/run/chat_last_seen`, holding unix seconds — is written when the last
//! live presence connection in the daemon closes (a chat leaving) and again
//! at every attach that considers delivery, so a window is never reported
//! twice. The **notice journal** carries the counts: `Bus::new` journals
//! every notice producers publish as an `event: "notice"` record, and the
//! digest streams them back with `for_each_notice_between` (M38 phase 08b),
//! counting script runs (source `Scheduler`, kinds `script` and `failure`)
//! and alerts (source `Scheduler` kind `alert`, and webhook firing alerts
//! whose severity reaches `warning`).

use crate::bus::{Notice, NoticeStatus, Source, for_each_notice_between};
use crate::config::var_run_dir;
use std::collections::BTreeMap;
use std::io::Write;

/// The stamp file: unix seconds, one value, replaced.
fn last_seen_path() -> std::path::PathBuf {
    var_run_dir().join("chat_last_seen")
}

/// Read the last-seen stamp. A missing, empty or unparseable file reads as
/// `None` — the first run after install, or a deleted stamp, has no window
/// to report.
pub(crate) fn read_last_seen() -> Option<u64> {
    let text = std::fs::read_to_string(last_seen_path()).ok()?;
    text.trim().parse::<u64>().ok()
}

/// Write the last-seen stamp. Atomic (temp file, then rename) so a crash
/// mid-write leaves the previous stamp intact. Failures are logged and
/// non-fatal: a stamp that could not be written at worst widens the next
/// digest's window.
pub(crate) fn stamp_last_seen(now: u64) {
    let path = last_seen_path();
    let tmp = path.with_extension("tmp");
    let result: std::io::Result<()> = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        writeln!(f, "{now}")?;
        f.sync_all()?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    })();
    if let Err(e) = result {
        log::warn!(
            "Failed to write the away-digest stamp {}: {e}",
            path.display()
        );
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Unix seconds for `now` at the daemon's call sites.
pub(crate) fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Counting
// ---------------------------------------------------------------------------

/// Per-name run counts: `(succeeded, failed)` for scripts, plain count for
/// alerts.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub(crate) struct AwayCounts {
    /// Script job runs per `job_name`: `(succeeded, failed)`.
    pub scripts: BTreeMap<String, (u64, u64)>,
    /// Alerts fired per name (`alert_name` / `job_name`).
    pub alerts: BTreeMap<String, u64>,
}

impl AwayCounts {
    fn count_script(&mut self, job: &str, ok: bool) {
        let e = self.scripts.entry(job.to_string()).or_insert((0, 0));
        if ok {
            e.0 += 1;
        } else {
            e.1 += 1;
        }
    }

    fn count_alert(&mut self, name: &str) {
        *self.alerts.entry(name.to_string()).or_insert(0) += 1;
    }

    fn script_runs(&self) -> u64 {
        self.scripts.values().map(|(ok, failed)| ok + failed).sum()
    }

    fn alerts_fired(&self) -> u64 {
        self.alerts.values().sum()
    }
}

/// Whether a webhook notice reached the operator: the same rule as the gate
/// in `process_alert` — firing, and severity at or above `warning` (an
/// unrankable severity degrades to `Debug`, below the gate — fail-closed).
fn alert_reaches_operator(notice: &Notice) -> bool {
    notice.status == NoticeStatus::Firing && notice.severity >= crate::bus::Severity::Warning
}

/// Tally one notice into `counts`. Pure: the same function the streaming
/// caller drives, so the window filtering it relies on is
/// `for_each_notice_between`'s `[from, to)` rule and nothing else.
fn count_notice(notice: &Notice, counts: &mut AwayCounts) {
    let action = notice.labels.get("action").map(String::as_str);
    let kind = notice.labels.get("kind").map(String::as_str);
    match (notice.source, kind) {
        (Source::Scheduler, Some("script")) => {
            let ok = notice.labels.get("exit_code").map(String::as_str) == Some("0");
            counts.count_script(&notice.name, ok);
        }
        (Source::Scheduler, Some("failure")) if action == Some("script") => {
            counts.count_script(&notice.name, false);
        }
        (Source::Scheduler, Some("alert")) => counts.count_alert(&notice.name),
        (Source::Webhook, Some("alert")) if alert_reaches_operator(notice) => {
            counts.count_alert(&notice.name);
        }
        _ => {}
    }
}

/// Count every script run and operator-visible alert in one window of
/// notices. Pure over an iterator; the file streaming lives in
/// [`build_away_digest`].
fn count_notices<'a, I>(notices: I) -> AwayCounts
where
    I: IntoIterator<Item = &'a Notice>,
{
    let mut counts = AwayCounts::default();
    for notice in notices {
        count_notice(notice, &mut counts);
    }
    counts
}

// ---------------------------------------------------------------------------
// Formatting
// ---------------------------------------------------------------------------

/// At most five names per bullet, most frequent first, then `+N more`.
const MAX_NAMES: usize = 5;

/// Sort a count map into `(name, count)` pairs, most frequent first, name
/// breaking ties deterministically.
fn sorted_pairs(map: &BTreeMap<String, u64>) -> Vec<(String, u64)> {
    let mut pairs: Vec<(String, u64)> = map.iter().map(|(k, v)| (k.clone(), *v)).collect();
    pairs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    pairs
}

/// Format the digest text from the counts, the window and nothing else.
/// One block: a header, then at most two bullets. A bullet whose count is
/// zero is omitted; when both are zero the function returns `None` —
/// nothing happened, nothing is shown.
fn format_away_digest(
    counts: &AwayCounts,
    from: chrono::DateTime<chrono::Utc>,
    to: chrono::DateTime<chrono::Utc>,
) -> Option<String> {
    let script_runs = counts.script_runs();
    let alerts_fired = counts.alerts_fired();
    if script_runs == 0 && alerts_fired == 0 {
        return None;
    }

    let away_secs = to.signed_duration_since(from).num_seconds().max(0) as u64;
    let away_str = if away_secs < 3600 {
        format!("{}m", away_secs / 60)
    } else {
        format!("{}h {}m", away_secs / 3600, (away_secs % 3600) / 60)
    };

    let mut parts = vec![format!(
        "[Away digest] While no chat was attached ({}, {away_str}):",
        from.format("%Y-%m-%d %H:%M UTC")
    )];

    if script_runs > 0 {
        // Most frequent job first, counting both outcomes together; a job's
        // own `(ok, failed)` follows it.
        let mut jobs: Vec<(String, u64)> = counts
            .scripts
            .iter()
            .map(|(name, (ok, failed))| (name.clone(), ok + failed))
            .collect();
        jobs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let names: Vec<String> = jobs
            .iter()
            .map(|(name, _total)| {
                let (ok, failed) = counts.scripts[name];
                format!("{name} ({ok} ok, {failed} failed)")
            })
            .collect();
        parts.push(format!(
            "  • Scripts: {} run{} — {}",
            script_runs,
            if script_runs == 1 { "" } else { "s" },
            name_list_with(&jobs, &names),
        ));
    }

    if alerts_fired > 0 {
        let pairs = sorted_pairs(&counts.alerts);
        let names: Vec<String> = pairs
            .iter()
            .map(|(name, n)| format!("{name} ×{n}"))
            .collect();
        parts.push(format!(
            "  • Alerts: {} fired — {}",
            alerts_fired,
            name_list_with(&pairs, &names),
        ));
    }

    Some(parts.join("\n"))
}

/// The per-name text for a bullet whose cap applies to `pairs`' length, not
/// to the rendered strings — the top [`MAX_NAMES`] names and a `+N more`.
fn name_list_with(pairs: &[(String, u64)], rendered: &[String]) -> String {
    let mut out = rendered
        .iter()
        .take(MAX_NAMES)
        .map(String::clone)
        .collect::<Vec<_>>()
        .join(", ");
    if pairs.len() > MAX_NAMES {
        out.push_str(&format!(" +{} more", pairs.len() - MAX_NAMES));
    }
    out
}

// ---------------------------------------------------------------------------
// The window and the delivery
// ---------------------------------------------------------------------------

/// Build the digest over `[from, to)`, streaming the notice journal once.
/// Returns `None` when nothing in the window would have reached an
/// operator.
pub(crate) fn build_away_digest(
    from: chrono::DateTime<chrono::Utc>,
    to: chrono::DateTime<chrono::Utc>,
) -> Option<String> {
    let mut notices: Vec<Notice> = Vec::new();
    for_each_notice_between(Some(from), Some(to), &mut |notice| {
        notices.push(notice.clone());
    });
    let counts = count_notices(&notices);
    format_away_digest(&counts, from, to)
}

/// The daemon's total live presence-connection count, across all sessions.
/// The delivery gate counts connections, not sessions: a second tab on the
/// same session is also receiving live messages.
fn live_presence_connections() -> usize {
    crate::daemon::session::global_presence().total_connections()
}

/// The attach-time delivery: one digest for the window since the stamp, or
/// `None`.
///
/// Acts only when this connection is the daemon's only live presence
/// connection — any other chat attached (in any session) is already
/// receiving live messages. With no stamp on disk (first run, or a deleted
/// file) the stamp is written and nothing is delivered: there is no window
/// to report. With a stamp, the digest covers `[stamp, now)` and the stamp
/// is written **whether or not** a digest was produced, so the same window
/// is never reported twice.
///
/// File I/O runs outside any session lock — the caller invokes this before
/// the attach reply, the way `SessionEntry::from_disk` is invoked.
pub(crate) fn take_away_digest_on_attach(now: u64) -> Option<String> {
    if live_presence_connections() > 1 {
        return None;
    }
    let Some(from_secs) = read_last_seen() else {
        stamp_last_seen(now);
        return None;
    };
    stamp_last_seen(now);
    if now <= from_secs {
        return None;
    }
    let from = chrono::DateTime::from_timestamp(from_secs as i64, 0)?;
    let to = chrono::DateTime::from_timestamp(now as i64, 0)?;
    build_away_digest(from, to)
}

/// Stamp when the last chat leaves: called after `presence_unregister` when
/// no presence connection remains in the daemon. Events from while a chat
/// *was* attached were seen live and are never counted.
pub(crate) fn stamp_on_last_detach() {
    if crate::daemon::session::presence_session_ids().is_empty() {
        stamp_last_seen(now_unix());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::Severity;

    /// A `Scheduler` script-run notice shaped as `script_job_notice` writes
    /// it, with a pinned instant so a test can place it in a window.
    fn script_notice(job: &str, exit_code: i32, ts: chrono::DateTime<chrono::Utc>) -> Notice {
        Notice::new(
            if exit_code == 0 {
                Severity::Info
            } else {
                Severity::Error
            },
            Source::Scheduler,
            job,
            format!("`{job}.sh` finished (exit {exit_code})"),
        )
        .source_id(format!("job-{job}"))
        .label("kind", "script")
        .label("exit_code", exit_code.to_string())
        .label("script", format!("{job}.sh"))
        .ts_refire(ts.timestamp())
    }

    /// A `Scheduler` failure notice shaped as `failure_notice` writes it.
    fn failure_notice(action: &str, job: &str, ts: chrono::DateTime<chrono::Utc>) -> Notice {
        Notice::new(
            Severity::Error,
            Source::Scheduler,
            job,
            "refused: cannot start",
        )
        .source_id(format!("job-{job}"))
        .label("kind", "failure")
        .label("action", action)
        .ts_refire(ts.timestamp())
    }

    /// A `Scheduler` alert-job notice shaped as `alert_job_notice` writes it.
    fn alert_job_notice(job: &str, ts: chrono::DateTime<chrono::Utc>) -> Notice {
        Notice::new(
            Severity::Warning,
            Source::Scheduler,
            job,
            "threshold crossed",
        )
        .source_id(format!("job-{job}"))
        .label("kind", "alert")
        .ts_refire(ts.timestamp())
    }

    /// A `Webhook` alert notice shaped as `process_alert` writes it.
    /// `severity` here is the raw label the webhook carried; the notice's
    /// own severity is what an unrankable string degrades to `Debug`.
    fn webhook_notice(
        name: &str,
        status: NoticeStatus,
        severity: &str,
        ts: chrono::DateTime<chrono::Utc>,
    ) -> Notice {
        Notice::new(
            Severity::from_external(severity),
            Source::Webhook,
            name,
            format!("[Webhook Alert] {name}"),
        )
        .source_id("alertmanager")
        .status(status)
        .fingerprint(format!("fp-{name}"))
        .label("severity", severity)
        .label("kind", "alert")
        .ts_refire(ts.timestamp())
    }

    /// A notice that is neither a script run nor an alert, of the shape the
    /// named producer writes.
    fn other_notice(
        source: Source,
        name: &str,
        kind: &str,
        ts: chrono::DateTime<chrono::Utc>,
    ) -> Notice {
        Notice::new(Severity::Info, source, name, format!("[{kind}] {name}"))
            .label("kind", kind)
            .ts_refire(ts.timestamp())
    }

    /// The serialized-notice journal record `Bus::new` writes, with a pinned
    /// `ts`, `event: "notice"` and `pid` — the shape `for_each_notice_between`
    /// parses, used by the delivery tests that read real journal files.
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
    fn seed_journal_record(record: &serde_json::Value, ts: chrono::DateTime<chrono::Utc>) {
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
        writeln!(f, "{}", serde_json::to_string(record).unwrap()).expect("appending");
    }

    /// Seed one notice into the journal at `ts`.
    fn seed_notice(notice: &Notice, ts: chrono::DateTime<chrono::Utc>) {
        seed_journal_record(&journal_record(notice), ts);
    }

    #[test]
    fn last_seen_round_trips_and_a_bad_file_reads_none() {
        let _home = crate::test_home_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("HOME", dir.path()) };
        crate::config::Config::ensure_dirs().ok();

        assert_eq!(read_last_seen(), None, "a missing stamp file reads as None");

        stamp_last_seen(1_700_000_123);
        assert_eq!(
            read_last_seen(),
            Some(1_700_000_123),
            "stamp then read gives the value back"
        );

        std::fs::write(last_seen_path(), "").expect("emptying the stamp file");
        assert_eq!(read_last_seen(), None, "an empty stamp file reads as None");

        std::fs::write(last_seen_path(), "garbage").expect("writing garbage");
        assert_eq!(
            read_last_seen(),
            None,
            "an unparseable stamp file reads as None"
        );
    }

    #[test]
    fn counts_script_runs_per_job_and_outcome() {
        let from = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let ts = |mins: i64| from + chrono::Duration::minutes(mins);
        let counts = count_notices([
            &script_notice("tmp-cast-purge", 0, ts(5)),
            &script_notice("tmp-cast-purge", 0, ts(10)),
            &script_notice("tmp-cast-purge", 1, ts(15)),
            &failure_notice("script", "backup", ts(20)),
        ]);
        assert_eq!(
            counts.scripts.get("tmp-cast-purge"),
            Some(&(2, 1)),
            "per-job (ok, failed) counts both outcomes"
        );
        assert_eq!(
            counts.scripts.get("backup"),
            Some(&(0, 1)),
            "a failure notice with action script counts as a failed run"
        );
        assert!(counts.alerts.is_empty(), "no alerts in these notices");
    }

    #[test]
    fn counts_only_alerts_that_reach_the_operator() {
        let from = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let ts = |mins: i64| from + chrono::Duration::minutes(mins);
        let counts = count_notices([
            &webhook_notice("disk-high", NoticeStatus::Firing, "warning", ts(1)),
            &webhook_notice("heartbeat", NoticeStatus::Firing, "info", ts(2)),
            &webhook_notice("disk-high", NoticeStatus::Resolved, "warning", ts(3)),
            &webhook_notice("weird", NoticeStatus::Firing, "banana", ts(4)),
            &webhook_notice("anomaly", NoticeStatus::Event, "warning", ts(5)),
            &alert_job_notice("deadman", ts(6)),
        ]);
        assert_eq!(
            counts.alerts.get("disk-high"),
            Some(&1),
            "a firing warning counts"
        );
        assert_eq!(
            counts.alerts.get("deadman"),
            Some(&1),
            "a scheduler kind alert notice is a fired alert"
        );
        assert!(
            !counts.alerts.contains_key("heartbeat"),
            "a firing info sits below the fixed warning gate"
        );
        assert_eq!(
            counts.alerts.len(),
            2,
            "a resolved alert, an unrankable raw severity and a webhook \
             analysis notice do not count: {:?}",
            counts.alerts
        );
    }

    #[test]
    fn ignores_notices_that_are_neither_runs_nor_alerts() {
        let from = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let ts = |mins: i64| from + chrono::Duration::minutes(mins);
        let counts = count_notices([
            &other_notice(Source::Ghost, "g", "started", ts(1)),
            &other_notice(Source::Watchdog, "w", "watchdog", ts(2)),
            &other_notice(Source::Background, "bg", "completed", ts(3)),
            &other_notice(Source::Shell, "sh", "state", ts(4)),
            &failure_notice("command", "c", ts(5)),
            &webhook_notice("n", NoticeStatus::Firing, "critical", ts(6)),
        ]);
        assert!(
            counts.scripts.is_empty(),
            "a ghost, watchdog, background or shell notice, and a refused \
             command failure, are not script runs: {:?}",
            counts.scripts
        );
        // The sixth notice is a real firing alert; the assertion is that the
        // other five contributed nothing.
        assert_eq!(
            counts.alerts.len(),
            1,
            "only the webhook alert counted: {:?}",
            counts.alerts
        );
    }

    #[test]
    fn a_legacy_scheduled_job_finished_record_is_not_read() {
        let _home = crate::test_home_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("HOME", dir.path()) };
        crate::config::Config::ensure_dirs().ok();

        let from = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let to = from + chrono::Duration::hours(1);
        seed_journal_record(
            &serde_json::json!({
                "ts": (from + chrono::Duration::minutes(5)).to_rfc3339(),
                "event": "scheduled_job_finished",
                "job_name": "tmp-cast-purge",
                "action": "script",
                "outcome": "succeeded",
            }),
            from + chrono::Duration::minutes(5),
        );
        seed_journal_record(
            &serde_json::json!({
                "ts": (from + chrono::Duration::minutes(6)).to_rfc3339(),
                "event": "webhook_alert",
                "alert_name": "disk-high",
                "status": "firing",
                "severity": "critical",
            }),
            from + chrono::Duration::minutes(6),
        );

        let mut notices: Vec<Notice> = Vec::new();
        for_each_notice_between(Some(from), Some(to), &mut |n| notices.push(n.clone()));
        let counts = count_notices(&notices);
        assert!(
            counts.scripts.is_empty() && counts.alerts.is_empty(),
            "the old event records are no longer read: {:?}",
            counts
        );
    }

    #[test]
    fn format_omits_empty_bullets_and_returns_none_when_all_zero() {
        let empty = AwayCounts::default();
        let from = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let to = chrono::DateTime::from_timestamp(1_700_028_000, 0).unwrap();
        assert_eq!(
            format_away_digest(&empty, from, to),
            None,
            "both counts zero → no digest at all"
        );

        let alerts_only = AwayCounts {
            scripts: BTreeMap::new(),
            alerts: BTreeMap::from([("disk-high".to_string(), 2)]),
        };
        let text = format_away_digest(&alerts_only, from, to).expect("alerts produce a digest");
        assert!(
            text.contains("• Alerts: 2 fired"),
            "the alerts bullet is present: {text}"
        );
        assert!(
            !text.contains("Scripts:"),
            "a zero-count bullet is omitted: {text}"
        );
    }

    #[test]
    fn format_caps_each_bullet_at_five_names() {
        let mut scripts = BTreeMap::new();
        for name in ["s1", "s2", "s3", "s4", "s5", "s6", "s7"] {
            scripts.insert(name.to_string(), (1, 0));
        }
        let counts = AwayCounts {
            scripts,
            alerts: BTreeMap::new(),
        };
        let from = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let to = chrono::DateTime::from_timestamp(1_700_000_000 + 7 * 3600 + 40 * 60, 0).unwrap();
        let text = format_away_digest(&counts, from, to).expect("scripts produce a digest");
        for name in ["s1", "s5"] {
            assert!(text.contains(name), "a top-five name is shown: {text}");
        }
        assert!(
            !text.contains("s6") && !text.contains("s7"),
            "names past the cap are not shown individually: {text}"
        );
        assert!(text.contains("+2 more"), "the overflow is counted: {text}");
    }

    #[test]
    fn first_attach_with_no_stamp_writes_one_and_delivers_nothing() {
        let _home = crate::test_home_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("HOME", dir.path()) };
        crate::config::Config::ensure_dirs().ok();
        // The presence map is process-global; this test runs with no live
        // connections, and registers none.
        let now = 1_700_000_500u64;

        assert_eq!(
            take_away_digest_on_attach(now),
            None,
            "no stamp → nothing delivered"
        );
        assert_eq!(
            read_last_seen(),
            Some(now),
            "the first attach writes the stamp"
        );
    }

    #[test]
    fn a_later_attach_delivers_the_digest_once() {
        let _home = crate::test_home_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("HOME", dir.path()) };
        crate::config::Config::ensure_dirs().ok();

        let stamp = 1_700_000_000u64;
        let now = stamp + 7 * 3600 + 40 * 60;
        let from = chrono::DateTime::from_timestamp(stamp as i64, 0).unwrap();

        // Seed the window the production streaming path reads: journal
        // records with pinned `ts` values, in the shape `Bus::new` writes.
        seed_notice(&script_notice("tmp-cast-purge", 0, from), from);
        seed_notice(&script_notice("tmp-cast-purge", 0, from), from);
        seed_notice(&script_notice("tmp-cast-purge", 1, from), from);
        seed_notice(
            &webhook_notice("disk-high", NoticeStatus::Firing, "warning", from),
            from,
        );

        stamp_last_seen(stamp);
        let digest = take_away_digest_on_attach(now).expect("events since the stamp → a digest");
        assert!(
            digest.contains("tmp-cast-purge (2 ok, 1 failed)"),
            "the digest carries the per-job counts: {digest}"
        );
        assert!(
            digest.contains("disk-high ×1"),
            "the digest carries the alert counts: {digest}"
        );
        assert_eq!(
            read_last_seen(),
            Some(now),
            "the stamp is now, whether or not a digest was produced"
        );

        // The same events, a new attach: the window moved past them, so
        // nothing is delivered again.
        let now2 = now + 60;
        assert_eq!(
            take_away_digest_on_attach(now2),
            None,
            "the window is never reported twice"
        );
    }

    #[test]
    fn an_attach_with_another_chat_present_delivers_nothing_and_stamps_nothing() {
        let _home = crate::test_home_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("HOME", dir.path()) };
        crate::config::Config::ensure_dirs().ok();

        // Production order: the attaching connection is registered before
        // `take_away_digest_on_attach` runs, so the gate sees it. This test
        // registers both the attaching connection and another chat's — a
        // total of 2 — and must deliver nothing either way.
        let attaching = crate::daemon::session::PresenceConnection::test_conn();
        assert_eq!(
            crate::daemon::session::presence_register("away-sess", attaching.clone()),
            1,
            "the attaching chat is registered first, as handle_chat_attach does"
        );
        let other = crate::daemon::session::PresenceConnection::test_conn();
        assert_eq!(
            crate::daemon::session::presence_register("away-other-sess", other.clone()),
            1,
            "another chat is attached"
        );

        stamp_last_seen(1_700_000_000);
        let now = 1_700_000_999u64;
        assert_eq!(
            take_away_digest_on_attach(now),
            None,
            "another chat present → nothing delivered"
        );
        assert_eq!(
            read_last_seen(),
            Some(1_700_000_000),
            "and the stamp is untouched"
        );

        assert!(
            crate::daemon::session::presence_unregister("away-sess", &attaching),
            "the map is left clean for the next test"
        );
        assert!(
            crate::daemon::session::presence_unregister("away-other-sess", &other),
            "the map is left clean for the next test"
        );
    }

    #[test]
    fn events_older_than_the_stamp_are_not_reported() {
        let _home = crate::test_home_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("HOME", dir.path()) };
        crate::config::Config::ensure_dirs().ok();

        // A notice at T, a stamp at T + 3600: the notice is before the
        // window's lower bound and must not appear in a digest over
        // [T+3600, now).
        let notice_ts = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        seed_notice(&script_notice("old-job", 1, notice_ts), notice_ts);

        stamp_last_seen(1_700_003_600);
        let now = 1_700_007_200u64;
        assert_eq!(
            take_away_digest_on_attach(now),
            None,
            "an event older than the stamp is outside the window"
        );
    }

    #[test]
    fn stamp_on_last_detach_only_when_no_connection_remains() {
        let _home = crate::test_home_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("HOME", dir.path()) };
        crate::config::Config::ensure_dirs().ok();

        let conn = crate::daemon::session::PresenceConnection::test_conn();
        assert_eq!(
            crate::daemon::session::presence_register("away-detach-sess", conn.clone()),
            1
        );
        stamp_on_last_detach();
        assert_eq!(
            read_last_seen(),
            None,
            "a live connection elsewhere means no stamp is written"
        );

        assert!(
            crate::daemon::session::presence_unregister("away-detach-sess", &conn),
            "the last connection closed"
        );
        let before = 1_700_000_000u64;
        stamp_last_seen(before);
        stamp_on_last_detach();
        let after = read_last_seen().expect("the empty registry stamps last seen");
        assert!(
            after >= before,
            "the stamp moved forward (or equals it within the same second)"
        );
    }
}

//! `daemoneye notices` CLI — prints past notices from the event journal,
//! filtered by severity, source and name.
//!
//! No daemon round-trip — reads the event log directly (consistent with
//! `daemoneye costs`). Works even when the daemon is down.
use std::collections::VecDeque;

use anyhow::{Context, Result};
use chrono::Local;
use tokio::io::BufReader;

use crate::bus::{Filter, Notice, Severity, Source};
use crate::cli::commands::ipc_client::{connect, recv, send_request};
use crate::ipc::{Request, Response};

/// The severities `--min-severity` accepts, for the error message.
const SEVERITIES: &str = "debug, info, warning, error, critical";
/// The sources `--source` accepts, for the error message.
const SOURCES: &str = "webhook, scheduler, watchdog, ghost, background, shell, daemon";

/// Build the [`Filter`] the flags describe. An unknown severity or source
/// is an error naming the valid values.
fn filter_from_flags(
    min_severity: Option<&str>,
    sources: &[String],
    names: &[String],
) -> Result<Filter> {
    let min_severity = match min_severity {
        Some(s) => Some(
            Severity::parse(s)
                .with_context(|| format!("Invalid --min-severity '{s}' (valid: {SEVERITIES})"))?,
        ),
        None => None,
    };
    let mut parsed_sources = Vec::with_capacity(sources.len());
    for src in sources {
        parsed_sources.push(
            parse_source(src)
                .with_context(|| format!("Invalid --source '{src}' (valid: {SOURCES})"))?,
        );
    }
    Ok(Filter {
        min_severity,
        sources: parsed_sources,
        names: names.to_vec(),
        statuses: Vec::new(),
    })
}

/// Offer one notice to the selection: a match joins the back of `ring`,
/// and the oldest is dropped once `ring` holds `limit`.
fn keep_last(ring: &mut VecDeque<Notice>, filter: &Filter, limit: usize, notice: &Notice) {
    if !filter.matches(notice) {
        return;
    }
    if ring.len() == limit {
        ring.pop_front();
    }
    ring.push_back(notice.clone());
}

/// Stream the whole journal, keep the notices the filter matches, and
/// return the last `limit` of them, in journal order (oldest first). Only
/// `limit` notices are held in memory at a time.
fn select_notices(filter: &Filter, limit: usize) -> Vec<Notice> {
    let mut ring = VecDeque::with_capacity(limit);
    crate::bus::for_each_notice_between(None, None, &mut |notice| {
        keep_last(&mut ring, filter, limit, notice);
    });
    ring.into_iter().collect()
}

/// One line per notice: local time, severity, source, status, name and the
/// title's first line, with the severity and source columns padded so the
/// columns line up. Control characters are escaped — a webhook payload can
/// carry escape bytes.
fn notice_line(notice: &Notice) -> String {
    let time = notice.ts.with_timezone(&Local).format("%Y-%m-%d %H:%M:%S");
    let severity = notice.severity.as_str();
    let source = notice.source.as_str();
    let sev_w = SEVERITY_NAMES.iter().map(|s| s.len()).max().unwrap_or(1);
    let src_w = SOURCE_NAMES.iter().map(|s| s.len()).max().unwrap_or(1);
    let title = notice.line();
    format!(
        "{time} {severity:<sev_w$} {source:<src_w$} {status} {name} {title}",
        status = status_str(notice.status),
        name = notice.name,
        title = crate::cli::render::escape_controls(title),
        sev_w = sev_w,
        src_w = src_w,
    )
}

/// The lowercase severity names, longest last, for column padding.
const SEVERITY_NAMES: [&str; 5] = ["debug", "info", "warning", "error", "critical"];
/// The lowercase source names, for column padding and `--source` parsing.
const SOURCE_NAMES: [&str; 7] = [
    "webhook",
    "scheduler",
    "watchdog",
    "ghost",
    "background",
    "shell",
    "daemon",
];

/// The lowercase name a [`NoticeStatus`] serialises as.
fn status_str(status: crate::bus::NoticeStatus) -> &'static str {
    match status {
        crate::bus::NoticeStatus::Firing => "firing",
        crate::bus::NoticeStatus::Resolved => "resolved",
        crate::bus::NoticeStatus::Event => "event",
    }
}

/// The full output text for a selection: one line per notice, or
/// `no notices` when the selection is empty. Pure, so tests pin the exact
/// text the command prints.
fn render_notices(selected: &[Notice]) -> String {
    if selected.is_empty() {
        return "no notices\n".to_string();
    }
    let mut out = String::new();
    for notice in selected {
        out.push_str(&notice_line(notice));
        out.push('\n');
    }
    out
}

/// CLI entry point — reads the journal, filters, and prints the last
/// `limit` matches, oldest first.
pub fn run_notices(
    min_severity: Option<&str>,
    sources: &[String],
    names: &[String],
    limit: usize,
) -> Result<()> {
    if limit == 0 {
        anyhow::bail!("--limit must be at least 1");
    }
    let filter = filter_from_flags(min_severity, sources, names)?;
    let selected = select_notices(&filter, limit);
    print!("{}", render_notices(&selected));
    Ok(())
}

/// The live-stream line for `notice`, or `None` when the backlog already
/// printed it: a notice published between the ack and the backlog read is
/// both in the journal and on the live stream, and must print once. Pure.
fn live_line(printed: &std::collections::HashSet<String>, n: &Notice) -> Option<String> {
    if printed.contains(&n.id) {
        return None;
    }
    Some(notice_line(n))
}

/// `--follow`: print the backlog, then stream live notices until the
/// connection closes or the user interrupts.
///
/// The backlog prints even when the daemon is down (the journal is read
/// locally); the live stream needs the daemon, so without one the command
/// exits 1 after the backlog.
pub async fn run_notices_follow(
    min_severity: Option<&str>,
    sources: &[String],
    names: &[String],
    limit: usize,
) -> Result<()> {
    if limit == 0 {
        anyhow::bail!("--limit must be at least 1");
    }
    let filter = filter_from_flags(min_severity, sources, names)?;

    // Subscribe first and wait for the ack: from then on every publish
    // reaches this connection. Only then read the backlog, so a notice
    // published in between is in the journal, on the live stream, or both
    // (and `live_line` prints it once) — never in neither.
    let subscribed = match connect().await {
        Ok(stream) => {
            let (rx, mut tx) = stream.into_split();
            let mut rx = BufReader::new(rx);
            send_request(
                &mut tx,
                Request::Subscribe {
                    filter: filter.clone(),
                },
            )
            .await?;
            match recv(&mut rx).await {
                Ok(Response::Ok) => {}
                Ok(other) => anyhow::bail!("unexpected reply to Subscribe: {}", other.kind()),
                Err(e) => anyhow::bail!("reading the Subscribe ack: {e}"),
            }
            Some((rx, tx))
        }
        Err(_) => None,
    };

    let selected = select_notices(&filter, limit);
    let printed: std::collections::HashSet<String> =
        selected.iter().map(|n| n.id.clone()).collect();
    print!("{}", render_notices(&selected));
    use std::io::Write as _;
    std::io::stdout().flush()?;

    // The backlog prints even without a daemon; following needs one.
    let Some((mut rx, _tx)) = subscribed else {
        eprintln!("daemon not running — cannot follow");
        std::process::exit(1);
    };

    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    loop {
        tokio::select! {
            msg = recv(&mut rx) => match msg {
                Ok(Response::Notice { notice }) => {
                    if let Some(line) = live_line(&printed, &notice) {
                        println!("{line}");
                        std::io::stdout().flush()?;
                    }
                }
                Ok(_) => {}
                // The daemon closed the connection (or errored): done.
                Err(_) => return Ok(()),
            },
            _ = interrupt.recv() => {
                return Ok(());
            }
        }
    }
}

/// Parse a `--source` value into a [`Source`], `None` when unrecognised.
fn parse_source(s: &str) -> Option<Source> {
    SOURCE_NAMES
        .iter()
        .find(|name| *name == &s)
        .map(|name| match *name {
            "webhook" => Source::Webhook,
            "scheduler" => Source::Scheduler,
            "watchdog" => Source::Watchdog,
            "ghost" => Source::Ghost,
            "background" => Source::Background,
            "shell" => Source::Shell,
            _ => Source::Daemon,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::NoticeStatus;

    fn notice(severity: Severity, source: Source, name: &str, title: &str) -> Notice {
        Notice::new(severity, source, name, title)
    }

    #[test]
    fn notices_keeps_the_last_limit_oldest_first() {
        let filter = filter_from_flags(None, &["webhook".to_string()], &[]).expect("valid flags");
        let mut ring = VecDeque::new();
        for i in 0..5 {
            let kept = notice(Severity::Info, Source::Webhook, "HighCPU", &format!("n{i}"));
            keep_last(&mut ring, &filter, 3, &kept);
            // A non-matching notice between each must not take a slot.
            let other = notice(Severity::Info, Source::Ghost, "run", &format!("g{i}"));
            keep_last(&mut ring, &filter, 3, &other);
        }
        let titles: Vec<&str> = ring.iter().map(|n| n.title.as_str()).collect();
        assert_eq!(titles, ["n2", "n3", "n4"], "the last three, oldest first");
    }

    #[test]
    fn notices_filter_by_severity_source_and_name() {
        let warning_webhook = notice(Severity::Warning, Source::Webhook, "HighCPU", "cpu");
        let info_webhook = notice(Severity::Info, Source::Webhook, "HighCPU", "cpu");
        let warning_scheduler = notice(Severity::Warning, Source::Scheduler, "HighCPU", "cpu");
        let other_name = notice(Severity::Warning, Source::Webhook, "HighMemory", "mem");

        let filter = filter_from_flags(
            Some("warning"),
            &["webhook".to_string()],
            &["HighCPU".to_string()],
        )
        .expect("valid flags");
        assert!(
            filter.matches(&warning_webhook),
            "a warning webhook HighCPU is kept"
        );
        assert!(
            !filter.matches(&info_webhook),
            "--min-severity warning drops info"
        );
        assert!(
            !filter.matches(&warning_scheduler),
            "--source webhook drops a scheduler notice"
        );
        assert!(
            !filter.matches(&other_name),
            "--name HighCPU drops another name"
        );

        let bad_sev = filter_from_flags(Some("bogus"), &[], &[]).expect_err("unknown severity");
        assert!(format!("{bad_sev:#}").contains("debug, info, warning, error, critical"));
        let bad_src =
            filter_from_flags(None, &["bogus".to_string()], &[]).expect_err("unknown source");
        assert!(format!("{bad_src:#}").contains("webhook, scheduler, watchdog"));
    }

    #[test]
    fn notice_line_escapes_control_bytes() {
        let n = notice(
            Severity::Warning,
            Source::Webhook,
            "HighCPU",
            "\x1b[31mCPU high\nsecond line",
        );
        let line = notice_line(&n);
        assert!(!line.contains('\x1b'), "a raw ESC byte must never print");
        assert!(line.contains("\\x1b[31m"), "ESC is escaped: {line}");
        assert!(!line.contains("second line"), "only the first line prints");
    }

    #[test]
    fn notices_reads_the_journal() {
        let _home = crate::test_home_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("HOME", dir.path()) };
        crate::config::Config::ensure_dirs().ok();

        let past = |mins: i64| chrono::Utc::now() - chrono::Duration::minutes(mins);
        let seed = |n: &Notice, ts: chrono::DateTime<chrono::Utc>| {
            let record = {
                let mut record = serde_json::to_value(n).expect("serialize notice");
                let obj = record.as_object_mut().expect("notice is an object");
                let mut stamped = serde_json::Map::new();
                stamped.insert(
                    "ts".to_string(),
                    serde_json::Value::String(n.ts.to_rfc3339()),
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
            };
            let path =
                crate::config::events_dir().join(format!("events-{}.jsonl", ts.format("%Y%m%d")));
            std::fs::create_dir_all(path.parent().expect("events dir parent")).expect("dir");
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .expect("opening the segment");
            use std::io::Write;
            writeln!(f, "{}", serde_json::to_string(&record).unwrap()).expect("appending");
        };

        seed(
            &notice(Severity::Info, Source::Webhook, "HighCPU", "cpu notice")
                .status(NoticeStatus::Firing)
                .ts_refire(past(60).timestamp()),
            past(60),
        );
        seed(
            &notice(
                Severity::Warning,
                Source::Scheduler,
                "Backup",
                "backup notice",
            )
            .status(NoticeStatus::Firing)
            .ts_refire(past(50).timestamp()),
            past(50),
        );
        seed(
            &notice(Severity::Error, Source::Webhook, "DiskFull", "disk notice")
                .status(NoticeStatus::Firing)
                .ts_refire(past(40).timestamp()),
            past(40),
        );

        let filter = Filter {
            min_severity: Some(Severity::Warning),
            sources: vec![Source::Webhook],
            names: vec!["DiskFull".to_string()],
            statuses: Vec::new(),
        };
        let out = render_notices(&select_notices(&filter, 50));
        assert_eq!(
            out.lines().count(),
            1,
            "only the DiskFull webhook survives: {out}"
        );
        assert!(
            out.contains(" error ")
                && out.contains(" webhook ")
                && out.ends_with("firing DiskFull disk notice\n"),
            "the line carries the fields: {out}"
        );
    }

    #[test]
    fn notices_with_none_prints_no_notices() {
        let _home = crate::test_home_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("HOME", dir.path()) };
        crate::config::Config::ensure_dirs().ok();
        let filter = Filter::default();
        let empty = select_notices(&filter, 50);
        assert!(empty.is_empty(), "an empty journal");
        assert_eq!(render_notices(&empty), "no notices\n");
    }

    /// A live notice whose id the backlog printed must not print again; any
    /// other notice prints as `notice_line` (phase 10b).
    #[test]
    fn live_line_skips_a_backlog_notice() {
        let backlog = notice(Severity::Info, Source::Webhook, "HighCPU", "cpu 95%");
        let printed: std::collections::HashSet<String> = [backlog.id.clone()].into_iter().collect();

        let dup = live_line(&printed, &backlog);
        assert!(
            dup.is_none(),
            "a notice the backlog printed must not print again, got {dup:?}"
        );

        let fresh = notice(Severity::Info, Source::Webhook, "LowDisk", "disk 90%");
        let line = live_line(&printed, &fresh);
        assert_eq!(
            line.as_deref(),
            Some(notice_line(&fresh).as_str()),
            "a fresh live notice prints its line"
        );
    }
}

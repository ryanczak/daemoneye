/// `daemoneye notices` CLI — prints past notices from the event journal,
/// filtered by severity, source and name.
///
/// No daemon round-trip — reads the event log directly (consistent with
/// `daemoneye costs`). Works even when the daemon is down.
use anyhow::{Context, Result};
use chrono::Local;

use crate::bus::{Filter, Notice, Severity, Source};

/// The severities `--min-severity` accepts, for the error message.
const SEVERITIES: &str = "debug, info, warning, error, critical";
/// The sources `--source` accepts, for the error message.
const SOURCES: &str = "webhook, scheduler, watchdog, ghost, background, shell, daemon";

/// The flags of one `daemoneye notices` invocation, before they build a
/// [`Filter`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoticesArgs {
    pub min_severity: Option<Severity>,
    pub sources: Vec<Source>,
    pub names: Vec<String>,
    pub limit: usize,
}

/// Build a [`Filter`] from the CLI flags.
fn build_filter(args: &NoticesArgs) -> Filter {
    Filter {
        min_severity: args.min_severity,
        sources: args.sources.clone(),
        names: args.names.clone(),
        statuses: Vec::new(),
    }
}

/// Stream the whole journal, keep the notices the filter matches, and
/// return the last `limit` of them, in journal order (oldest first). Only
/// `limit` notices are held in memory at a time.
fn select_notices(filter: &Filter, limit: usize) -> Vec<Notice> {
    let mut ring: std::collections::VecDeque<Notice> = std::collections::VecDeque::new();
    crate::bus::for_each_notice_between(None, None, &mut |notice| {
        if !filter.matches(notice) {
            return;
        }
        if ring.len() == limit {
            ring.pop_front();
        }
        ring.push_back(notice.clone());
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
    let args = NoticesArgs {
        min_severity,
        sources: parsed_sources,
        names: names.to_vec(),
        limit,
    };
    let filter = build_filter(&args);
    let selected = select_notices(&filter, limit);
    print!("{}", render_notices(&selected));
    Ok(())
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
        let five: Vec<Notice> = (0..5)
            .map(|i| notice(Severity::Info, Source::Webhook, "HighCPU", &format!("n{i}")))
            .collect();
        // Stream through the same selection the command runs, holding only
        // the ring of `limit` notices.
        let mut kept: Vec<Notice> = Vec::new();
        for n in &five {
            if kept.len() == 3 {
                kept.remove(0);
            }
            kept.push(n.clone());
        }
        assert_eq!(kept.len(), 3);
        assert_eq!(kept[0].title, "n2", "the oldest kept is the third");
        assert_eq!(kept[2].title, "n4", "the newest kept is the last");
    }

    #[test]
    fn notices_filter_by_severity_source_and_name() {
        let warning_webhook = notice(Severity::Warning, Source::Webhook, "HighCPU", "cpu");
        let info_webhook = notice(Severity::Info, Source::Webhook, "HighCPU", "cpu");
        let warning_scheduler = notice(Severity::Warning, Source::Scheduler, "HighCPU", "cpu");
        let other_name = notice(Severity::Warning, Source::Webhook, "HighMemory", "mem");

        let min_warning = Filter {
            min_severity: Some(Severity::Warning),
            ..Filter::default()
        };
        assert!(min_warning.matches(&warning_webhook), "warning is kept");
        assert!(!min_warning.matches(&info_webhook), "info is dropped");

        let webhook_only = Filter {
            sources: vec![Source::Webhook],
            ..Filter::default()
        };
        assert!(webhook_only.matches(&warning_webhook));
        assert!(
            !webhook_only.matches(&warning_scheduler),
            "a scheduler notice is dropped by --source webhook"
        );

        let named = Filter {
            names: vec!["HighCPU".to_string()],
            ..Filter::default()
        };
        assert!(named.matches(&warning_webhook));
        assert!(
            !named.matches(&other_name),
            "another name is dropped by --name"
        );
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
        let selected = select_notices(&filter, 50);
        assert_eq!(selected.len(), 1, "only the DiskFull webhook survives");
        let line = notice_line(&selected[0]);
        assert!(
            line.contains("error") && line.contains("webhook") && line.contains("disk notice"),
            "the line carries the fields: {line}"
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
}

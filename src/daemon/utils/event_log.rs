/// Write a structured JSONL event record to a dated segment file under
/// `~/.daemoneye/var/log/events/events-YYYYMMDD.jsonl`.
///
/// Each call appends one JSON object per line.  The top-level fields
/// `ts` (ISO-8601 UTC), `event` (event type name), and `pid` (the emitting
/// process) are always present.  Additional fields are provided by the caller
/// as a `serde_json::Value` object and merged in.
///
/// Errors are silently discarded — logging must never crash the daemon.
pub fn log_event(event: &str, mut fields: serde_json::Value) {
    use std::io::Write;

    crate::ai::mask_json_value(&mut fields);

    let path = crate::config::current_event_segment_path();
    let ts = chrono::Utc::now().to_rfc3339();

    if let Some(obj) = fields.as_object_mut() {
        // Prepend ts + event + pid so they appear first in the line.
        let mut record = serde_json::Map::new();
        record.insert("ts".to_string(), serde_json::Value::String(ts));
        record.insert(
            "event".to_string(),
            serde_json::Value::String(event.to_string()),
        );
        record.insert(
            "pid".to_string(),
            serde_json::Value::from(std::process::id()),
        );

        // Take ownership of the fields from the caller's object
        let drained = std::mem::take(obj);
        for (k, v) in drained {
            record.insert(k, v);
        }

        let mut line = serde_json::to_string(&record).unwrap_or_default();
        line.push('\n');

        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }

        // Determine segment label: "legacy" for the old single-file path,
        // otherwise the file stem (e.g. "events-20260803").
        let segment_label = if path == crate::config::events_path() {
            "legacy".to_string()
        } else {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string()
        };

        // Capture offset before the append.
        let offset = std::fs::metadata(&path).ok().map(|m| m.len()).unwrap_or(0);

        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            && f.write_all(line.as_bytes()).is_ok()
        {
            // Best-effort index the event.
            let body = crate::search::json_to_readable(line.trim_end());
            if let Err(e) = crate::memory::index::index_event(&segment_label, offset, event, &body)
            {
                log::warn!("event index update failed: {e:#}");
            }
        }
    }
}

/// Aggregated cost summary for a time window.
///
/// Produced by `sum_cost_between` and consumed by the catch-up brief
/// renderer to produce a one-line cost summary.
pub struct CostSummary {
    /// Total cost across all agents in the window.
    pub total_cost_usd: f64,
    /// Per-agent cost breakdown: `(agent_name, cost_usd)`.
    pub by_agent: Vec<(String, f64)>,
    /// True when at least one AI call had Unknown pricing.
    pub has_untracked: bool,
    /// Number of AI calls in the window.
    pub call_count: u32,
}

/// Parse the date from a segment filename like `events-20240115.jsonl`.
/// Returns `None` if the filename doesn't match the pattern.
fn segment_date_from_path(path: &std::path::Path) -> Option<chrono::NaiveDate> {
    let stem = path.file_stem()?.to_str()?;
    if !stem.starts_with("events-") {
        return None;
    }
    let date_str = &stem[7..];
    if date_str.len() != 8 {
        return None;
    }
    chrono::NaiveDate::parse_from_str(date_str, "%Y%m%d").ok()
}

/// All event files overlapping `[from, to]`, oldest first.
///
/// The legacy `var/log/events.jsonl` (if present) is always first — it is
/// treated as the oldest segment and is never date-filtered (we cannot
/// know its content range from the filename).
/// `None` bounds mean unbounded on that side.
pub fn event_segment_paths_between(
    from: Option<chrono::DateTime<chrono::Utc>>,
    to: Option<chrono::DateTime<chrono::Utc>>,
) -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();

    // Legacy file is always first (oldest, no date filter).
    let legacy = crate::config::events_path();
    if legacy.exists() {
        paths.push(legacy);
    }

    let events_dir = crate::config::events_dir();
    let Ok(entries) = std::fs::read_dir(&events_dir) else {
        return paths;
    };

    let mut dated: Vec<(chrono::NaiveDate, std::path::PathBuf)> = Vec::new();
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if let Some(date) = segment_date_from_path(&path) {
            dated.push((date, path));
        }
    }
    dated.sort_by_key(|a| a.0);

    for (date, path) in dated {
        let seg_start = date.and_hms_opt(0, 0, 0).unwrap().and_utc();
        let seg_end = seg_start + chrono::Duration::days(1);

        let in_range = match (from, to) {
            (Some(f), Some(t)) => seg_start < t && seg_end > f,
            (Some(f), None) => seg_end > f,
            (None, Some(t)) => seg_start < t,
            (None, None) => true,
        };

        if in_range {
            paths.push(path);
        }
    }

    paths
}

/// Stream every event line in `[from, to]` (parsed JSON) through `f`,
/// oldest segment first. Lines that fail to parse are skipped.
/// Per-line `ts` filtering still applies (segment granularity is a day;
/// the window is finer).
pub fn for_each_event_between(
    from: Option<chrono::DateTime<chrono::Utc>>,
    to: Option<chrono::DateTime<chrono::Utc>>,
    f: &mut dyn FnMut(&serde_json::Value),
) {
    let paths = event_segment_paths_between(from, to);
    for path in &paths {
        let Ok(file) = std::fs::File::open(path) else {
            continue;
        };
        let reader = std::io::BufReader::new(file);
        use std::io::BufRead;
        for line_result in reader.lines() {
            let Ok(line) = line_result else { continue };
            let Ok(value): Result<serde_json::Value, _> = serde_json::from_str(&line) else {
                continue;
            };

            // Per-line ts filtering
            if from.is_some() || to.is_some() {
                let Some(ts_str) = value.get("ts").and_then(|v| v.as_str()) else {
                    continue;
                };
                let Ok(ts) = chrono::DateTime::parse_from_rfc3339(ts_str) else {
                    continue;
                };
                let ts_utc = ts.with_timezone(&chrono::Utc);
                if let Some(f) = from
                    && ts_utc < f
                {
                    continue;
                }
                if let Some(t) = to
                    && ts_utc >= t
                {
                    continue;
                }
            }

            f(&value);
        }
    }
}

/// Sum `ai_cost` events from dated segments between two UTC timestamps.
///
/// Streams segments line-by-line (never loads the whole file). Only events
/// with `event == "ai_cost"` and a `ts` field within `[from, to)` are included.
/// Returns a `CostSummary` with per-agent breakdown and an untracked flag.
pub fn sum_cost_between(
    from: chrono::DateTime<chrono::Utc>,
    to: chrono::DateTime<chrono::Utc>,
) -> CostSummary {
    let mut total: f64 = 0.0;
    let mut by_agent: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    let mut has_untracked = false;
    let mut call_count: u32 = 0;

    for_each_event_between(Some(from), Some(to), &mut |value| {
        if value.get("event").and_then(|v| v.as_str()) != Some("ai_cost") {
            return;
        }
        let cost = value
            .get("cost")
            .and_then(|c| c.get("total_cost_usd"))
            .and_then(|c| c.as_f64())
            .unwrap_or(0.0);
        total += cost;
        call_count += 1;
        if let Some(agent) = value.get("agent_name").and_then(|v| v.as_str()) {
            *by_agent.entry(agent.to_string()).or_insert(0.0) += cost;
        }
        if let Some(src) = value.get("pricing_source").and_then(|v| v.as_str())
            && src == "Unknown"
        {
            has_untracked = true;
        }
    });

    let mut by_agent_vec: Vec<(String, f64)> = by_agent.into_iter().collect();
    by_agent_vec.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    CostSummary {
        total_cost_usd: total,
        by_agent: by_agent_vec,
        has_untracked,
        call_count,
    }
}

/// Delete dated event segments whose filename date is older than
/// `retention_days` days before today (UTC). The legacy `var/events.jsonl`
/// is never deleted. When `retention_days == 0`, this is a no-op.
pub fn sweep_event_segments(retention_days: u32) {
    if retention_days == 0 {
        return;
    }

    let events_dir = crate::config::events_dir();
    let Ok(entries) = std::fs::read_dir(&events_dir) else {
        return;
    };

    let today = chrono::Utc::now().date_naive();
    let cutoff = today - chrono::Duration::days(retention_days as i64);

    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if let Some(date) = segment_date_from_path(&path)
            && date < cutoff
        {
            log::info!("events: deleting expired segment {}", path.display());
            if let Err(e) = std::fs::remove_file(&path) {
                log::warn!("events: failed to delete {}: {}", path.display(), e);
            } else {
                let segment = path.file_stem().map(|s| s.to_string_lossy().to_string());
                if let Some(seg) = segment
                    && let Err(e) = crate::memory::index::remove_event_segment(&seg)
                {
                    log::warn!(
                        "events: failed to remove index rows for segment {}: {}",
                        seg,
                        e
                    );
                }
            }
        }
    }
}

/// Back-compat shim — existing call sites in server.rs still compile while
/// the migration to `log_event` is in progress.  New code should call
/// `log_event` directly.
pub fn log_command(
    session_id: Option<&str>,
    mode: &str,
    pane: &str,
    command: &str,
    status: &str,
    output_excerpt: &str,
) {
    let cmd: String = command
        .chars()
        .map(|c| if c == '\n' { ' ' } else { c })
        .collect();
    let out: String = output_excerpt
        .chars()
        .take(200)
        .map(|c| if c == '\n' { ' ' } else { c })
        .collect();
    log_event(
        "command",
        serde_json::json!({
            "session": session_id.unwrap_or("-"),
            "mode":    mode,
            "pane":    pane,
            "cmd":     cmd,
            "status":  status,
            "out":     out,
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn with_test_home<F: FnOnce()>(f: F) {
        let _lock = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        let saved_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", tmp.path()) };
        f();
        if let Some(h) = saved_home {
            unsafe { std::env::set_var("HOME", h) };
        } else {
            unsafe { std::env::remove_var("HOME") };
        }
    }

    fn write_event(path: &std::path::Path, event: &str, ts: &str) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        let record = serde_json::json!({"ts": ts, "event": event});
        writeln!(file, "{}", record).unwrap();
    }

    fn write_cost_event(path: &std::path::Path, ts: &str, agent: &str, cost: f64) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        let record = serde_json::json!({
            "ts": ts,
            "event": "ai_cost",
            "agent_name": agent,
            "cost": {"total_cost_usd": cost},
            "pricing_source": "Known"
        });
        writeln!(file, "{}", record).unwrap();
    }

    #[test]
    fn segments_enumerate_legacy_first() {
        with_test_home(|| {
            let legacy = crate::config::events_path();
            let events_dir = crate::config::events_dir();
            let _ = std::fs::create_dir_all(&events_dir);

            // Write legacy file
            write_event(&legacy, "test", "2024-01-01T00:00:00+00:00");
            // Write two dated segments
            let seg1 = events_dir.join("events-20240115.jsonl");
            let seg2 = events_dir.join("events-20240116.jsonl");
            write_event(&seg1, "test", "2024-01-15T00:00:00+00:00");
            write_event(&seg2, "test", "2024-01-16T00:00:00+00:00");

            let paths = event_segment_paths_between(None, None);
            assert_eq!(paths.len(), 3);
            assert_eq!(paths[0].file_name().unwrap(), "events.jsonl");
            assert_eq!(paths[1].file_name().unwrap(), "events-20240115.jsonl");
            assert_eq!(paths[2].file_name().unwrap(), "events-20240116.jsonl");
        });
    }

    #[test]
    fn segments_window_excludes_out_of_range() {
        with_test_home(|| {
            let events_dir = crate::config::events_dir();
            let _ = std::fs::create_dir_all(&events_dir);

            // Three dated segments
            let seg1 = events_dir.join("events-20240114.jsonl");
            let seg2 = events_dir.join("events-20240115.jsonl");
            let seg3 = events_dir.join("events-20240116.jsonl");

            // seg2 has a line whose ts is inside the window but seg1/seg3 are outside
            write_cost_event(&seg1, "2024-01-14T12:00:00+00:00", "a", 1.0);
            write_cost_event(&seg2, "2024-01-15T12:00:00+00:00", "b", 2.0);
            write_cost_event(&seg3, "2024-01-16T12:00:00+00:00", "c", 3.0);

            // Window covering only Jan 15
            let from = chrono::NaiveDate::from_ymd_opt(2024, 1, 15)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc();
            let to = from + chrono::Duration::days(1);

            let paths = event_segment_paths_between(Some(from), Some(to));
            assert_eq!(paths.len(), 1);
            assert_eq!(paths[0].file_name().unwrap(), "events-20240115.jsonl");
        });
    }

    #[test]
    fn for_each_streams_across_segments_in_order() {
        with_test_home(|| {
            let events_dir = crate::config::events_dir();
            let _ = std::fs::create_dir_all(&events_dir);

            let seg1 = events_dir.join("events-20240115.jsonl");
            let seg2 = events_dir.join("events-20240116.jsonl");
            write_event(&seg1, "first", "2024-01-15T00:00:00+00:00");
            write_event(&seg2, "second", "2024-01-16T00:00:00+00:00");

            let mut events = Vec::new();
            for_each_event_between(None, None, &mut |v| {
                events.push(v.get("event").unwrap().as_str().unwrap().to_string());
            });

            assert_eq!(events, vec!["first", "second"]);
        });
    }

    #[test]
    fn for_each_skips_unparseable_lines() {
        with_test_home(|| {
            let events_dir = crate::config::events_dir();
            let _ = std::fs::create_dir_all(&events_dir);

            let seg = events_dir.join("events-20240115.jsonl");
            // Write a valid line, a garbage line, then another valid line
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&seg)
                .unwrap();
            writeln!(
                file,
                "{{\"ts\": \"2024-01-15T00:00:00+00:00\", \"event\": \"good1\"}}"
            )
            .unwrap();
            writeln!(file, "THIS IS NOT JSON").unwrap();
            writeln!(
                file,
                "{{\"ts\": \"2024-01-15T01:00:00+00:00\", \"event\": \"good2\"}}"
            )
            .unwrap();

            let mut events = Vec::new();
            for_each_event_between(None, None, &mut |v| {
                events.push(v.get("event").unwrap().as_str().unwrap().to_string());
            });

            assert_eq!(events, vec!["good1", "good2"]);
        });
    }

    #[test]
    fn sum_cost_between_spans_segments() {
        with_test_home(|| {
            let legacy = crate::config::events_path();
            let events_dir = crate::config::events_dir();
            let _ = std::fs::create_dir_all(&events_dir);

            // Legacy file has a cost event
            write_cost_event(&legacy, "2024-01-14T12:00:00+00:00", "agent-a", 1.5);
            // Dated segment has a cost event
            let seg = events_dir.join("events-20240115.jsonl");
            write_cost_event(&seg, "2024-01-15T12:00:00+00:00", "agent-b", 2.5);

            let from = chrono::NaiveDate::from_ymd_opt(2024, 1, 14)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc();
            let to = chrono::NaiveDate::from_ymd_opt(2024, 1, 16)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc();

            let summary = sum_cost_between(from, to);
            assert!((summary.total_cost_usd - 4.0).abs() < f64::EPSILON);
            assert_eq!(summary.call_count, 2);
            assert_eq!(summary.by_agent.len(), 2);
        });
    }

    #[test]
    fn sweep_deletes_only_expired_segments() {
        with_test_home(|| {
            let events_dir = crate::config::events_dir();
            let _ = std::fs::create_dir_all(&events_dir);

            let today = chrono::Utc::now().date_naive();
            let old_date = today - chrono::Duration::days(100);
            let recent_date = today - chrono::Duration::days(5);

            let old_seg = events_dir.join(format!("events-{}.jsonl", old_date.format("%Y%m%d")));
            let recent_seg =
                events_dir.join(format!("events-{}.jsonl", recent_date.format("%Y%m%d")));
            let legacy = crate::config::events_path();

            write_event(&old_seg, "old", "2020-01-01T00:00:00+00:00");
            write_event(&recent_seg, "recent", "2024-06-01T00:00:00+00:00");
            write_event(&legacy, "legacy", "2019-01-01T00:00:00+00:00");

            // retention_days = 90: old_seg should be deleted, recent_seg and legacy kept
            sweep_event_segments(90);
            assert!(!old_seg.exists());
            assert!(recent_seg.exists());
            assert!(legacy.exists());

            // retention_days = 0: no-op
            let old_seg2 = events_dir.join(format!("events-{}.jsonl", old_date.format("%Y%m%d")));
            write_event(&old_seg2, "old2", "2020-01-01T00:00:00+00:00");
            sweep_event_segments(0);
            assert!(old_seg2.exists());
        });
    }

    #[test]
    fn log_event_writes_today_segment() {
        with_test_home(|| {
            log_event("test_event", serde_json::json!({"key": "value"}));

            let seg = crate::config::current_event_segment_path();
            assert!(seg.exists());

            // Legacy file should NOT have been created
            let legacy = crate::config::events_path();
            assert!(!legacy.exists());

            // Verify content
            let content = std::fs::read_to_string(&seg).unwrap();
            let line = content.lines().next().unwrap();
            let value: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(value["event"], "test_event");
            assert_eq!(value["key"], "value");
            assert!(value.get("ts").is_some());
        });
    }

    #[test]
    fn log_event_stamps_emitting_pid() {
        with_test_home(|| {
            log_event("pid_test", serde_json::json!({}));
            let seg = crate::config::current_event_segment_path();
            let content = std::fs::read_to_string(&seg).unwrap();
            let line = content.lines().next().unwrap();
            let record: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(record["pid"], serde_json::Value::from(std::process::id()));
        });
    }

    #[test]
    fn log_event_always_stamps_ts_event_and_pid() {
        with_test_home(|| {
            log_event("presence_test", serde_json::json!({"custom": "val"}));
            let seg = crate::config::current_event_segment_path();
            let content = std::fs::read_to_string(&seg).unwrap();
            let line = content.lines().next().unwrap();
            let record: serde_json::Value = serde_json::from_str(line).unwrap();
            // Presence, not position: serde_json::Map is a BTreeMap here
            // (no `preserve_order`), so serialized key order is alphabetical
            // and insertion order is not observable in the output.
            assert!(record.get("ts").is_some(), "ts missing");
            assert!(record.get("event").is_some(), "event missing");
            assert!(record.get("pid").is_some(), "pid missing");
            assert_eq!(record["event"], "presence_test");
            assert_eq!(record["custom"], "val");
        });
    }

    #[test]
    fn log_event_caller_pid_overrides_stamp() {
        with_test_home(|| {
            log_event("override_test", serde_json::json!({"pid": 999_999}));
            let seg = crate::config::current_event_segment_path();
            let content = std::fs::read_to_string(&seg).unwrap();
            let line = content.lines().next().unwrap();
            let record: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(record["pid"], 999_999);
        });
    }

    #[test]
    fn test_log_event_masks_caller_fields() {
        with_test_home(|| {
            let canary = "AKIAIOSFODNN7EXAMPLE";
            log_event(
                "mask_test",
                serde_json::json!({
                    "top_level": canary,
                    "nested": { "inner": canary },
                    "arr": [canary]
                }),
            );
            let seg = crate::config::current_event_segment_path();
            let content = std::fs::read_to_string(&seg).unwrap();
            let line = content.lines().next().unwrap();
            // All canary instances replaced
            assert!(
                line.contains("<AWS_KEY>"),
                "expected masked value in: {line}"
            );
            assert!(!line.contains(canary), "canary still present in: {line}");
            // Line still parses as valid JSON with daemon fields
            let record: serde_json::Value = serde_json::from_str(line).unwrap();
            assert!(record.get("ts").is_some(), "ts missing");
            assert!(record.get("event").is_some(), "event missing");
            assert!(record.get("pid").is_some(), "pid missing");
        });
    }

    #[test]
    fn test_log_event_leaves_daemon_fields_and_numbers() {
        with_test_home(|| {
            log_event(
                "numbers_test",
                serde_json::json!({
                    "prompt_tokens": 123,
                    "label": "safe"
                }),
            );
            let seg = crate::config::current_event_segment_path();
            let content = std::fs::read_to_string(&seg).unwrap();
            let line = content.lines().next().unwrap();
            let record: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(record["event"], "numbers_test");
            assert_eq!(record["prompt_tokens"], 123);
            assert_eq!(record["label"], "safe");
        });
    }

    #[test]
    fn sweeping_a_segment_removes_its_events_rows() {
        let _lock = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        let old_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let events_dir = crate::config::events_dir();
        std::fs::create_dir_all(&events_dir).unwrap();

        let seg_name = "events-20260101";
        let seg_path = events_dir.join(format!("{}.jsonl", seg_name));
        write_event(
            &seg_path,
            "unique sweep event target",
            "2026-01-01T00:00:00Z",
        );

        crate::memory::index::index_event_segment(seg_name).unwrap();

        let conn = crate::memory::index::open_index().unwrap();
        let events_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM events WHERE events MATCH 'unique sweep event'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(events_count, 1, "events should be indexed before sweep");

        let map_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM events_map WHERE segment = ?1",
                (seg_name,),
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(map_count, 1, "events_map should have a row before sweep");

        sweep_event_segments(14);

        assert!(!seg_path.exists(), "expired segment should be deleted");

        let conn = crate::memory::index::open_index().unwrap();
        let events_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM events WHERE events MATCH 'unique sweep event'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            events_count, 0,
            "events FTS rows should be removed after sweep"
        );

        let map_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM events_map WHERE segment = ?1",
                (seg_name,),
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            map_count, 0,
            "events_map rows should be removed after sweep"
        );

        match old_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }

    #[test]
    fn sweeping_a_segment_leaves_other_segments_indexed() {
        let _lock = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        let old_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let events_dir = crate::config::events_dir();
        std::fs::create_dir_all(&events_dir).unwrap();

        let seg_path_old = events_dir.join("events-20260101.jsonl");
        write_event(
            &seg_path_old,
            "old segment alpha event",
            "2026-01-01T00:00:00Z",
        );
        crate::memory::index::index_event_segment("events-20260101").unwrap();

        let seg_path_new = events_dir.join("events-20260803.jsonl");
        write_event(
            &seg_path_new,
            "new segment beta event",
            "2026-08-03T00:00:00Z",
        );
        crate::memory::index::index_event_segment("events-20260803").unwrap();

        sweep_event_segments(14);

        assert!(!seg_path_old.exists(), "old segment should be deleted");
        let conn = crate::memory::index::open_index().unwrap();
        let old_events: i64 = conn
            .query_row(
                "SELECT count(*) FROM events WHERE events MATCH 'old segment alpha'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(old_events, 0, "old events should be removed");

        assert!(seg_path_new.exists(), "new segment should survive");
        let new_events: i64 = conn
            .query_row(
                "SELECT count(*) FROM events WHERE events MATCH 'new segment beta'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(new_events, 1, "new events should survive");

        let new_map: i64 = conn
            .query_row(
                "SELECT count(*) FROM events_map WHERE segment = 'events-20260803'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(new_map, 1, "new map rows should survive");

        match old_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }

    #[test]
    fn sweep_event_segments_zero_retention_removes_nothing() {
        let _lock = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        let old_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let events_dir = crate::config::events_dir();
        std::fs::create_dir_all(&events_dir).unwrap();

        let seg_path = events_dir.join("events-20260101.jsonl");
        write_event(&seg_path, "zero retention event", "2026-01-01T00:00:00Z");
        crate::memory::index::index_event_segment("events-20260101").unwrap();

        sweep_event_segments(0);

        assert!(
            seg_path.exists(),
            "segment should survive with retention_days=0"
        );

        let conn = crate::memory::index::open_index().unwrap();
        let events_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM events WHERE events MATCH 'zero retention'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            events_count, 1,
            "events should survive with retention_days=0"
        );

        match old_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }

    #[test]
    fn sweep_event_segments_survives_unwritable_index() {
        let _lock = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        let old_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let events_dir = crate::config::events_dir();
        std::fs::create_dir_all(&events_dir).unwrap();

        let seg_path = events_dir.join("events-20260101.jsonl");
        write_event(&seg_path, "unwritable event sweep", "2026-01-01T00:00:00Z");
        crate::memory::index::index_event_segment("events-20260101").unwrap();

        let index_path = crate::config::memory_index_path();
        let index_dir = index_path.parent().unwrap();
        let original_perms = std::fs::metadata(index_dir).unwrap().permissions();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(index_dir, std::fs::Permissions::from_mode(0o000)).unwrap();

        sweep_event_segments(14);

        assert!(
            !seg_path.exists(),
            "segment should be deleted even when index is unwritable"
        );

        std::fs::set_permissions(index_dir, original_perms).unwrap();

        match old_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }
}

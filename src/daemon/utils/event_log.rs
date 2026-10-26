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

/// A `command` event for a shell-substrate run. Same shape as `log_command`
/// plus the real exit code, which the tmux path cannot supply, and the 2.0
/// `wait`/`shell` keys: `wait` is false for a `wait: false` run, `shell` is
/// the PTY shell id.
pub fn log_shell_command(
    session_id: Option<&str>,
    shell_id: &str,
    wait: bool,
    command: &str,
    status: &str,
    exit_code: i32,
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
            "mode":    "shell",
            "wait":    wait,
            "shell":   shell_id,
            "pane":    shell_id,
            "cmd":     cmd,
            "status":  status,
            "exit":    exit_code,
            "out":     out,
        }),
    );
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
    let wait = match mode {
        "foreground" => Some(true),
        "background" => Some(false),
        _ => None,
    };
    log_event(
        "command",
        serde_json::json!({
            "session": session_id.unwrap_or("-"),
            "mode":    mode,
            "wait":    wait,
            "shell":   serde_json::Value::Null,
            "pane":    pane,
            "cmd":     cmd,
            "status":  status,
            "out":     out,
        }),
    );
}

#[cfg(test)]
#[path = "event_log_tests.rs"]
mod tests;

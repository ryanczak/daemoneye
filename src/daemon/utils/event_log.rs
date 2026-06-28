/// Write a structured JSONL event record to `~/.daemoneye/var/events.jsonl`.
///
/// Each call appends one JSON object per line.  The top-level fields
/// `ts` (ISO-8601 UTC) and `event` (event type name) are always present.
/// Additional fields are provided by the caller as a `serde_json::Value`
/// object and merged in.
///
/// Errors are silently discarded — logging must never crash the daemon.
pub fn log_event(event: &str, mut fields: serde_json::Value) {
    use std::io::Write;

    let path = crate::config::events_path();
    let ts = chrono::Utc::now().to_rfc3339();

    if let Some(obj) = fields.as_object_mut() {
        // Prepend ts + event so they appear first in the line.
        let mut record = serde_json::Map::new();
        record.insert("ts".to_string(), serde_json::Value::String(ts));
        record.insert(
            "event".to_string(),
            serde_json::Value::String(event.to_string()),
        );

        // Take ownership of the fields from the caller's object
        let drained = std::mem::take(obj);
        for (k, v) in drained {
            record.insert(k, v);
        }

        let mut line = serde_json::to_string(&record).unwrap_or_default();
        line.push('\n');

        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = f.write_all(line.as_bytes());
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

/// Sum `ai_cost` events from `events.jsonl` between two UTC timestamps.
///
/// Streams the file line-by-line (never loads the whole file). Only events
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

    let events_path = crate::config::events_path();
    if let Ok(file) = std::fs::File::open(&events_path) {
        use std::io::BufRead;
        let reader = std::io::BufReader::new(file);
        for line in reader.lines() {
            let Ok(line) = line else { continue };
            let Ok(value): Result<serde_json::Value, _> = serde_json::from_str(&line) else {
                continue;
            };
            if value.get("event").and_then(|v| v.as_str()) != Some("ai_cost") {
                continue;
            }
            let Some(ts_str) = value.get("ts").and_then(|v| v.as_str()) else {
                continue;
            };
            let Ok(ts) = chrono::DateTime::parse_from_rfc3339(ts_str) else {
                continue;
            };
            let ts_utc = ts.with_timezone(&chrono::Utc);
            if ts_utc < from || ts_utc >= to {
                continue;
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
        }
    }

    let mut by_agent_vec: Vec<(String, f64)> = by_agent.into_iter().collect();
    by_agent_vec.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    CostSummary {
        total_cost_usd: total,
        by_agent: by_agent_vec,
        has_untracked,
        call_count,
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

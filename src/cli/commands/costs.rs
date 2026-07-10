/// `daemoneye costs` CLI — slices `events.jsonl` cost data by day, agent,
/// provider, model, or session.
///
/// No daemon round-trip — reads `events.jsonl` directly (consistent with
/// `daemoneye logs`). Works even when the daemon is down.
use anyhow::{Context, Result};
use chrono::{NaiveDate, Utc};
use serde::Serialize;
use std::collections::HashMap;
use std::io::BufRead;

/// How to group cost aggregation results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum GroupBy {
    Day,
    Agent,
    Provider,
    Model,
    Session,
}

/// Aggregated cost data for one group key (day, agent, provider, etc.).
#[derive(Debug, Clone, Default, Serialize, serde::Deserialize)]
pub struct CostGroup {
    pub key: String,
    pub calls: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_tokens: u64,
    pub total_cost_usd: f64,
    pub untracked_calls: u32,
}

/// Full summary returned by the cost aggregation engine.
#[derive(Debug, Clone, Default, Serialize, serde::Deserialize)]
pub struct CostSummary {
    pub groups: Vec<CostGroup>,
    pub total_calls: u32,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_tokens: u64,
    pub total_cost_usd: f64,
    pub untracked_calls: u32,
    pub untracked_tokens: u64,
}

/// Inner aggregation engine, separated from I/O for testability.
///
/// Streams the reader line-by-line (never loads the whole file). Stops
/// reading early once timestamps exceed `until_dt` (events are append-only
/// in time order).
///
/// * `since_dt` — inclusive start of the date range (start of that day UTC).
/// * `until_dt` — exclusive end of the date range (start of the next day UTC).
///   Events with `ts >= until_dt` are excluded.
/// * `group_by` — how to bucket the results.
/// * `agent_filter` — if `Some`, only include events for this agent.
pub fn aggregate_costs(
    reader: impl BufRead,
    since_dt: chrono::DateTime<Utc>,
    until_dt: chrono::DateTime<Utc>,
    group_by: GroupBy,
    agent_filter: Option<&str>,
) -> CostSummary {
    let mut groups: HashMap<String, CostGroup> = HashMap::new();
    let mut total_calls: u32 = 0;
    let mut total_input: u64 = 0;
    let mut total_output: u64 = 0;
    let mut total_cache: u64 = 0;
    let mut total_cost: f64 = 0.0;
    let mut untracked_calls: u32 = 0;
    let mut untracked_tokens: u64 = 0;

    for line in reader.lines() {
        let Ok(line) = line else { continue };
        if line.trim().is_empty() {
            continue;
        }

        let value: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if value.get("event").and_then(|v| v.as_str()) != Some("ai_cost") {
            continue;
        }

        let ts_str = match value.get("ts").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => continue,
        };
        let ts = match chrono::DateTime::parse_from_rfc3339(ts_str) {
            Ok(dt) => dt.with_timezone(&Utc),
            Err(_) => continue,
        };

        if ts >= until_dt {
            break;
        }

        if ts < since_dt {
            continue;
        }

        let agent_name = value
            .get("agent_name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        if let Some(filter) = agent_filter
            && agent_name != filter
        {
            continue;
        }

        let key = match group_by {
            GroupBy::Day => {
                let date = ts.date_naive();
                date.format("%Y-%m-%d").to_string()
            }
            GroupBy::Agent => agent_name.to_string(),
            GroupBy::Provider => value
                .get("provider")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string(),
            GroupBy::Model => value
                .get("model")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string(),
            GroupBy::Session => value
                .get("session_id")
                .and_then(|v| v.as_str())
                .unwrap_or("-")
                .to_string(),
        };

        let tokens = value.get("tokens");
        let input_tokens: u64 = tokens
            .and_then(|t| t.get("input_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let output_tokens: u64 = tokens
            .and_then(|t| t.get("output_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let cache_read: u64 = tokens
            .and_then(|t| t.get("cache_read_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let cache_write: u64 = tokens
            .and_then(|t| t.get("cache_write_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let cache_tokens = cache_read + cache_write;

        let cost = value
            .get("cost")
            .and_then(|c| c.get("total_cost_usd"))
            .and_then(|c| c.as_f64())
            .unwrap_or(0.0);

        let pricing_source = value
            .get("pricing_source")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let is_untracked = pricing_source.eq_ignore_ascii_case("unknown");

        let group = groups.entry(key.clone()).or_default();
        group.key = key;
        group.calls += 1;
        group.input_tokens += input_tokens;
        group.output_tokens += output_tokens;
        group.cache_tokens += cache_tokens;
        group.total_cost_usd += cost;
        if is_untracked {
            group.untracked_calls += 1;
        }

        total_calls += 1;
        total_input += input_tokens;
        total_output += output_tokens;
        total_cache += cache_tokens;
        total_cost += cost;
        if is_untracked {
            untracked_calls += 1;
            untracked_tokens += input_tokens + output_tokens + cache_tokens;
        }
    }

    let mut sorted_groups: Vec<CostGroup> = groups.into_values().collect();
    match group_by {
        GroupBy::Day => {
            sorted_groups.sort_by(|a, b| a.key.cmp(&b.key));
        }
        _ => {
            sorted_groups.sort_by(|a, b| {
                b.total_cost_usd
                    .partial_cmp(&a.total_cost_usd)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
    }

    CostSummary {
        groups: sorted_groups,
        total_calls,
        total_input_tokens: total_input,
        total_output_tokens: total_output,
        total_cache_tokens: total_cache,
        total_cost_usd: total_cost,
        untracked_calls,
        untracked_tokens,
    }
}

/// Aggregate `ai_cost` events across every segment overlapping
/// `[since_dt, until_dt)`, merging per-segment group summaries by key and
/// re-sorting the merged groups into the documented order (ascending key for
/// `Day`, descending cost otherwise). Factored out of `run_costs` so the
/// merge+sort behavior is unit-testable against real on-disk segments.
fn aggregate_over_range(
    since_dt: chrono::DateTime<Utc>,
    until_dt: chrono::DateTime<Utc>,
    group_by: GroupBy,
    agent_filter: Option<&str>,
) -> CostSummary {
    let segments =
        crate::daemon::utils::event_segment_paths_between(Some(since_dt), Some(until_dt));

    let mut summary = CostSummary::default();
    for path in &segments {
        let Ok(file) = std::fs::File::open(path) else {
            continue;
        };
        let reader = std::io::BufReader::new(file);
        let seg_summary = aggregate_costs(reader, since_dt, until_dt, group_by, agent_filter);
        summary.total_calls += seg_summary.total_calls;
        summary.total_input_tokens += seg_summary.total_input_tokens;
        summary.total_output_tokens += seg_summary.total_output_tokens;
        summary.total_cache_tokens += seg_summary.total_cache_tokens;
        summary.total_cost_usd += seg_summary.total_cost_usd;
        summary.untracked_calls += seg_summary.untracked_calls;
        summary.untracked_tokens += seg_summary.untracked_tokens;

        // Merge groups by key.
        for seg_group in seg_summary.groups {
            if let Some(existing) = summary.groups.iter_mut().find(|g| g.key == seg_group.key) {
                existing.calls += seg_group.calls;
                existing.input_tokens += seg_group.input_tokens;
                existing.output_tokens += seg_group.output_tokens;
                existing.cache_tokens += seg_group.cache_tokens;
                existing.total_cost_usd += seg_group.total_cost_usd;
                existing.untracked_calls += seg_group.untracked_calls;
            } else {
                summary.groups.push(seg_group);
            }
        }
    }

    // Re-sort merged groups to match the documented order (descending cost
    // for non-Day groupings, ascending key for Day). The per-segment sort
    // from `aggregate_costs` only guarantees ordering within each segment;
    // merging can disrupt that.
    match group_by {
        GroupBy::Day => summary.groups.sort_by(|a, b| a.key.cmp(&b.key)),
        _ => summary.groups.sort_by(|a, b| {
            b.total_cost_usd
                .partial_cmp(&a.total_cost_usd)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
    }

    summary
}

/// CLI entry point — reads event segments, aggregates, and renders.
pub fn run_costs(
    since: Option<String>,
    until: Option<String>,
    group_by: GroupBy,
    agent_filter: Option<String>,
    json: bool,
) -> Result<()> {
    let now = Utc::now();
    let since_dt = match &since {
        Some(s) => parse_date_bound(s, true)
            .with_context(|| format!("Invalid --since date: {s} (expected YYYY-MM-DD)"))?,
        None => {
            let naive = now.date_naive() - chrono::Duration::days(6);
            // INVARIANT: midnight (0, 0, 0) is always a valid NaiveTime
            naive.and_hms_opt(0, 0, 0).unwrap().and_utc()
        }
    };
    let until_dt = match &until {
        Some(s) => parse_date_bound(s, false)
            .with_context(|| format!("Invalid --until date: {s} (expected YYYY-MM-DD)"))?,
        None => now,
    };

    let summary = aggregate_over_range(since_dt, until_dt, group_by, agent_filter.as_deref());

    if json {
        let json_str = serde_json::to_string_pretty(&summary)?;
        println!("{json_str}");
    } else {
        print_human(&summary, &since_dt, &until_dt, group_by)?;
    }

    Ok(())
}

/// Parse a YYYY-MM-DD string into a DateTime<Utc> boundary.
///
/// * `start` — if `true`, returns 00:00:00 of the given date (inclusive start).
///   if `false`, returns 00:00:00 of the *next* day (exclusive end bound),
///   matching the `today_end` pattern in `stats.rs:compute_cost_today`.
fn parse_date_bound(s: &str, start: bool) -> Result<chrono::DateTime<Utc>> {
    let date = NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .with_context(|| format!("Invalid date format: {s}"))?;
    if start {
        // INVARIANT: midnight (0, 0, 0) is always a valid NaiveTime
        Ok(date.and_hms_opt(0, 0, 0).unwrap().and_utc())
    } else {
        let next_day = date
            .succ_opt()
            .with_context(|| format!("Invalid date: {s}"))?;
        // INVARIANT: midnight (0, 0, 0) is always a valid NaiveTime
        Ok(next_day.and_hms_opt(0, 0, 0).unwrap().and_utc())
    }
}

/// Format a token count as a compact human-readable string.
///
/// Values >= 1_000_000 are shown as `{N}m`, values >= 1_000 as `{N}k`,
/// otherwise raw number. Integer division truncates (not rounds).
fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{}m", n / 1_000_000)
    } else if n >= 1_000 {
        format!("{}k", n / 1_000)
    } else {
        n.to_string()
    }
}

/// Print the human-readable cost summary table.
fn print_human(
    summary: &CostSummary,
    since_dt: &chrono::DateTime<Utc>,
    until_dt: &chrono::DateTime<Utc>,
    _group_by: GroupBy,
) -> Result<()> {
    let since_date = since_dt.date_naive().format("%Y-%m-%d");
    let until_date = until_dt.date_naive().format("%Y-%m-%d");
    println!("Cost summary — {since_date} to {until_date}");
    println!();

    let key_w = if summary.groups.is_empty() {
        5
    } else {
        summary
            .groups
            .iter()
            .map(|g| g.key.len())
            .max()
            .unwrap_or(5)
            .max(5)
    };
    let calls_w = 6usize;
    let tokens_w = 22usize;
    let cost_w = 10usize;

    println!(
        "  {:<key_w$}  {:>calls_w$}  {:>tokens_w$}  {:>cost_w$}",
        "",
        "Calls",
        "Tokens (in/out/cache)",
        "Cost (USD)",
        key_w = key_w,
        calls_w = calls_w,
        tokens_w = tokens_w,
        cost_w = cost_w,
    );

    println!(
        "  {}  {}  {}  {}",
        "─".repeat(key_w),
        "─".repeat(calls_w),
        "─".repeat(tokens_w),
        "─".repeat(cost_w),
    );

    for g in &summary.groups {
        let token_str = format!(
            "{} / {} / {}",
            fmt_tokens(g.input_tokens),
            fmt_tokens(g.output_tokens),
            fmt_tokens(g.cache_tokens)
        );
        println!(
            "  {:<key_w$}  {:>calls_w$}  {:>tokens_w$}  ${:>cost_w$.2}",
            g.key,
            g.calls,
            token_str,
            g.total_cost_usd,
            key_w = key_w,
            calls_w = calls_w,
            tokens_w = tokens_w,
            cost_w = cost_w,
        );
    }

    let total_token_str = format!(
        "{} / {} / {}",
        fmt_tokens(summary.total_input_tokens),
        fmt_tokens(summary.total_output_tokens),
        fmt_tokens(summary.total_cache_tokens)
    );
    println!(
        "  {}  {}  {}  {}",
        "─".repeat(key_w),
        "─".repeat(calls_w),
        "─".repeat(tokens_w),
        "─".repeat(cost_w),
    );
    println!(
        "  {:<key_w$}  {:>calls_w$}  {:>tokens_w$}  ${:>cost_w$.2}",
        "Total",
        summary.total_calls,
        total_token_str,
        summary.total_cost_usd,
        key_w = key_w,
        calls_w = calls_w,
        tokens_w = tokens_w,
        cost_w = cost_w,
    );

    if summary.untracked_calls > 0 {
        println!(
            "  Untracked (unknown pricing): {} calls, ~{} tokens",
            summary.untracked_calls,
            fmt_tokens(summary.untracked_tokens)
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;
    use std::io::{BufReader, Cursor};

    fn aggregate_from_lines(
        lines: &[&str],
        since_dt: chrono::DateTime<Utc>,
        until_dt: chrono::DateTime<Utc>,
        group_by: GroupBy,
        agent_filter: Option<&str>,
    ) -> CostSummary {
        let fixture: String = lines.iter().map(|l| format!("{l}\n")).collect();
        let reader = BufReader::new(Cursor::new(fixture));
        aggregate_costs(reader, since_dt, until_dt, group_by, agent_filter)
    }

    fn wide_range() -> (chrono::DateTime<Utc>, chrono::DateTime<Utc>) {
        let since = NaiveDate::from_ymd_opt(2020, 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc();
        let until = NaiveDate::from_ymd_opt(2030, 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc();
        (since, until)
    }

    #[test]
    fn aggregate_costs_basic_input_output() {
        let (since, until) = wide_range();
        let lines = vec![
            r#"{"event":"ai_cost","ts":"2026-05-16T10:00:00Z","agent_name":"chat","provider":"anthropic","model":"claude-sonnet-4-6","tokens":{"input_tokens":1000,"output_tokens":500,"cache_read_tokens":0,"cache_write_tokens":0},"cost":{"total_cost_usd":0.01},"pricing_source":"BuiltinDefault"}"#,
            r#"{"event":"ai_cost","ts":"2026-05-16T11:00:00Z","agent_name":"chat","provider":"anthropic","model":"claude-sonnet-4-6","tokens":{"input_tokens":2000,"output_tokens":1000,"cache_read_tokens":0,"cache_write_tokens":0},"cost":{"total_cost_usd":0.02},"pricing_source":"BuiltinDefault"}"#,
        ];
        let summary = aggregate_from_lines(&lines, since, until, GroupBy::Day, None);
        assert_eq!(summary.total_calls, 2);
        assert!((summary.total_cost_usd - 0.03).abs() < 1e-10);
        assert_eq!(summary.total_input_tokens, 3000);
        assert_eq!(summary.total_output_tokens, 1500);
        assert_eq!(summary.groups.len(), 1);
        assert_eq!(summary.groups[0].key, "2026-05-16");
        assert_eq!(summary.groups[0].calls, 2);
    }

    #[test]
    fn aggregate_costs_by_agent_splits_correctly() {
        let (since, until) = wide_range();
        let lines = vec![
            r#"{"event":"ai_cost","ts":"2026-05-16T10:00:00Z","agent_name":"chat","provider":"anthropic","model":"claude-sonnet-4-6","tokens":{"input_tokens":1000,"output_tokens":500,"cache_read_tokens":0,"cache_write_tokens":0},"cost":{"total_cost_usd":0.01},"pricing_source":"BuiltinDefault"}"#,
            r#"{"event":"ai_cost","ts":"2026-05-16T11:00:00Z","agent_name":"architect","provider":"anthropic","model":"claude-sonnet-4-6","tokens":{"input_tokens":2000,"output_tokens":1000,"cache_read_tokens":0,"cache_write_tokens":0},"cost":{"total_cost_usd":0.02},"pricing_source":"BuiltinDefault"}"#,
        ];
        let summary = aggregate_from_lines(&lines, since, until, GroupBy::Agent, None);
        assert_eq!(summary.total_calls, 2);
        assert_eq!(summary.groups.len(), 2);
        assert_eq!(summary.groups[0].key, "architect");
        assert!((summary.groups[0].total_cost_usd - 0.02).abs() < 1e-10);
        assert_eq!(summary.groups[1].key, "chat");
        assert!((summary.groups[1].total_cost_usd - 0.01).abs() < 1e-10);
    }

    #[test]
    fn aggregate_costs_filter_by_agent_excludes_others() {
        let (since, until) = wide_range();
        let lines = vec![
            r#"{"event":"ai_cost","ts":"2026-05-16T10:00:00Z","agent_name":"chat","provider":"anthropic","model":"claude-sonnet-4-6","tokens":{"input_tokens":1000,"output_tokens":500,"cache_read_tokens":0,"cache_write_tokens":0},"cost":{"total_cost_usd":0.01},"pricing_source":"BuiltinDefault"}"#,
            r#"{"event":"ai_cost","ts":"2026-05-16T11:00:00Z","agent_name":"architect","provider":"anthropic","model":"claude-sonnet-4-6","tokens":{"input_tokens":2000,"output_tokens":1000,"cache_read_tokens":0,"cache_write_tokens":0},"cost":{"total_cost_usd":0.02},"pricing_source":"BuiltinDefault"}"#,
        ];
        let summary = aggregate_from_lines(&lines, since, until, GroupBy::Day, Some("chat"));
        assert_eq!(summary.total_calls, 1);
        assert!((summary.total_cost_usd - 0.01).abs() < 1e-10);
    }

    #[test]
    fn aggregate_costs_date_range_excludes_outside_events() {
        let lines = vec![
            r#"{"event":"ai_cost","ts":"2026-05-14T10:00:00Z","agent_name":"chat","provider":"anthropic","model":"claude-sonnet-4-6","tokens":{"input_tokens":1000,"output_tokens":500,"cache_read_tokens":0,"cache_write_tokens":0},"cost":{"total_cost_usd":0.01},"pricing_source":"BuiltinDefault"}"#,
            r#"{"event":"ai_cost","ts":"2026-05-15T10:00:00Z","agent_name":"chat","provider":"anthropic","model":"claude-sonnet-4-6","tokens":{"input_tokens":2000,"output_tokens":1000,"cache_read_tokens":0,"cache_write_tokens":0},"cost":{"total_cost_usd":0.02},"pricing_source":"BuiltinDefault"}"#,
            r#"{"event":"ai_cost","ts":"2026-05-16T10:00:00Z","agent_name":"chat","provider":"anthropic","model":"claude-sonnet-4-6","tokens":{"input_tokens":500,"output_tokens":200,"cache_read_tokens":0,"cache_write_tokens":0},"cost":{"total_cost_usd":0.005},"pricing_source":"BuiltinDefault"}"#,
        ];
        let since_dt = NaiveDate::from_ymd_opt(2026, 5, 15)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc();
        let until_dt = NaiveDate::from_ymd_opt(2026, 5, 17)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc();
        let summary = aggregate_from_lines(&lines, since_dt, until_dt, GroupBy::Day, None);
        assert_eq!(summary.total_calls, 2);
        assert!((summary.total_cost_usd - 0.025).abs() < 1e-10);
    }

    #[test]
    fn aggregate_costs_untracked_calls_surface() {
        let (since, until) = wide_range();
        let lines = vec![
            r#"{"event":"ai_cost","ts":"2026-05-16T10:00:00Z","agent_name":"chat","provider":"anthropic","model":"claude-sonnet-4-6","tokens":{"input_tokens":1000,"output_tokens":500,"cache_read_tokens":0,"cache_write_tokens":0},"cost":{"total_cost_usd":0.01},"pricing_source":"BuiltinDefault"}"#,
            r#"{"event":"ai_cost","ts":"2026-05-16T11:00:00Z","agent_name":"chat","provider":"unknown","model":"unknown","tokens":{"input_tokens":2000,"output_tokens":1000,"cache_read_tokens":0,"cache_write_tokens":0},"cost":{"total_cost_usd":0.0},"pricing_source":"Unknown"}"#,
        ];
        let summary = aggregate_from_lines(&lines, since, until, GroupBy::Day, None);
        assert_eq!(summary.total_calls, 2);
        assert_eq!(summary.untracked_calls, 1);
        assert_eq!(summary.untracked_tokens, 3000);
    }

    #[test]
    fn aggregate_costs_empty_input_returns_zero_summary() {
        let (since, until) = wide_range();
        let summary = aggregate_from_lines(&[], since, until, GroupBy::Day, None);
        assert_eq!(summary.total_calls, 0);
        assert!(summary.groups.is_empty());
    }

    #[test]
    fn aggregate_costs_snapshot_deterministic_fixture() {
        let (since, until) = wide_range();
        let mut lines = Vec::new();
        for i in 0..20 {
            let cost = 0.01 * (i as f64 + 1.0);
            let ts = format!("2026-05-16T{:02}:00:00Z", i % 24);
            let agent = if i % 3 == 0 {
                "architect"
            } else if i % 3 == 1 {
                "chat"
            } else {
                "ghost-anonymous"
            };
            let input = 1000 * (i as u64 + 1);
            let output = 500 * (i as u64 + 1);
            lines.push(format!(
                r#"{{"event":"ai_cost","ts":"{ts}","agent_name":"{agent}","provider":"anthropic","model":"claude-sonnet-4-6","tokens":{{"input_tokens":{input},"output_tokens":{output},"cache_read_tokens":0,"cache_write_tokens":0}},"cost":{{"total_cost_usd":{cost}}},"pricing_source":"BuiltinDefault"}}"#
            ));
        }
        let line_refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        let summary = aggregate_from_lines(&line_refs, since, until, GroupBy::Day, None);

        // total_cost_usd = 0.01 * (1 + 2 + ... + 20) = 0.01 * 210 = 2.10
        assert!((summary.total_cost_usd - 2.10).abs() < 1e-10);
        assert_eq!(summary.total_calls, 20);
        // total_input = 1000 * (1 + 2 + ... + 20) = 1000 * 210 = 210_000
        assert_eq!(summary.total_input_tokens, 210_000);
        // total_output = 500 * 210 = 105_000
        assert_eq!(summary.total_output_tokens, 105_000);
        assert_eq!(summary.groups.len(), 1);
        assert_eq!(summary.groups[0].key, "2026-05-16");
        assert_eq!(summary.groups[0].calls, 20);
    }

    #[test]
    fn aggregate_costs_sub_second_boundary_included() {
        let (since, _until) = wide_range();
        let lines = vec![
            r#"{"event":"ai_cost","ts":"2026-05-16T23:59:59.001Z","agent_name":"chat","provider":"anthropic","model":"claude-sonnet-4-6","tokens":{"input_tokens":1000,"output_tokens":500,"cache_read_tokens":0,"cache_write_tokens":0},"cost":{"total_cost_usd":0.05},"pricing_source":"BuiltinDefault"}"#,
            r#"{"event":"ai_cost","ts":"2026-05-17T00:00:00Z","agent_name":"chat","provider":"anthropic","model":"claude-sonnet-4-6","tokens":{"input_tokens":100,"output_tokens":50,"cache_read_tokens":0,"cache_write_tokens":0},"cost":{"total_cost_usd":999.0},"pricing_source":"BuiltinDefault"}"#,
        ];
        let until_dt = NaiveDate::from_ymd_opt(2026, 5, 17)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc();
        let summary = aggregate_from_lines(&lines, since, until_dt, GroupBy::Day, None);
        assert_eq!(summary.total_calls, 1);
        assert!((summary.total_cost_usd - 0.05).abs() < 1e-10);
    }

    #[test]
    fn fmt_tokens_formats_correctly() {
        assert_eq!(fmt_tokens(0), "0");
        assert_eq!(fmt_tokens(500), "500");
        assert_eq!(fmt_tokens(1_000), "1k");
        assert_eq!(fmt_tokens(12_000), "12k");
        assert_eq!(fmt_tokens(180_000), "180k");
        assert_eq!(fmt_tokens(1_000_000), "1m");
        assert_eq!(fmt_tokens(1_500_000), "1m");
    }

    #[test]
    fn parse_date_bound_start_of_day() {
        let dt = parse_date_bound("2026-05-01", true).unwrap();
        assert_eq!(dt.date_naive().to_string(), "2026-05-01");
        assert_eq!(dt.hour(), 0);
        assert_eq!(dt.minute(), 0);
        assert_eq!(dt.second(), 0);
    }

    #[test]
    fn parse_date_bound_next_day_start() {
        let dt = parse_date_bound("2026-05-16", false).unwrap();
        assert_eq!(dt.date_naive().to_string(), "2026-05-17");
        assert_eq!(dt.hour(), 0);
        assert_eq!(dt.minute(), 0);
        assert_eq!(dt.second(), 0);
    }

    #[test]
    fn parse_date_bound_invalid_format() {
        let result = parse_date_bound("not-a-date", true);
        assert!(result.is_err());
    }

    #[test]
    fn cost_summary_serializes_to_json() {
        let summary = CostSummary {
            groups: vec![CostGroup {
                key: "2026-05-16".to_string(),
                calls: 42,
                input_tokens: 58_000,
                output_tokens: 12_000,
                cache_tokens: 4_000,
                total_cost_usd: 0.41,
                untracked_calls: 3,
            }],
            total_calls: 42,
            total_input_tokens: 58_000,
            total_output_tokens: 12_000,
            total_cache_tokens: 4_000,
            total_cost_usd: 0.41,
            untracked_calls: 3,
            untracked_tokens: 45_000,
        };
        let json = serde_json::to_string(&summary).unwrap();
        let back: CostSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(back.total_calls, 42);
        assert_eq!(back.groups[0].key, "2026-05-16");
    }

    #[test]
    fn cli_costs_default_groups_by_day() {
        let _lock = crate::TEST_HOME_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };
        crate::config::Config::ensure_dirs().unwrap();

        let today = Utc::now().date_naive().format("%Y-%m-%d").to_string();
        let yesterday = (Utc::now().date_naive() - chrono::Duration::days(1))
            .format("%Y-%m-%d")
            .to_string();
        let events_path = crate::config::events_path();

        let lines = vec![
            format!(
                r#"{{"event":"ai_cost","ts":"{today}T10:00:00Z","agent_name":"chat","provider":"anthropic","model":"claude-sonnet-4-6","tokens":{{"input_tokens":1000,"output_tokens":500,"cache_read_tokens":0,"cache_write_tokens":0}},"cost":{{"total_cost_usd":0.01}},"pricing_source":"BuiltinDefault"}}"#
            ),
            format!(
                r#"{{"event":"ai_cost","ts":"{today}T11:00:00Z","agent_name":"chat","provider":"anthropic","model":"claude-sonnet-4-6","tokens":{{"input_tokens":2000,"output_tokens":1000,"cache_read_tokens":0,"cache_write_tokens":0}},"cost":{{"total_cost_usd":0.02}},"pricing_source":"BuiltinDefault"}}"#
            ),
            format!(
                r#"{{"event":"ai_cost","ts":"{yesterday}T10:00:00Z","agent_name":"architect","provider":"gemini","model":"gemini-2.5-pro","tokens":{{"input_tokens":500,"output_tokens":200,"cache_read_tokens":0,"cache_write_tokens":0}},"cost":{{"total_cost_usd":0.005}},"pricing_source":"BuiltinDefault"}}"#
            ),
        ];
        std::fs::create_dir_all(events_path.parent().unwrap()).unwrap();
        let mut f = std::fs::File::create(&events_path).unwrap();
        for line in &lines {
            use std::io::Write;
            writeln!(f, "{line}").unwrap();
        }

        let result = run_costs(None, None, GroupBy::Day, None, false);
        assert!(result.is_ok());
    }

    #[test]
    fn cli_costs_json_output_matches_schema() {
        let _lock = crate::TEST_HOME_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };
        crate::config::Config::ensure_dirs().unwrap();

        let today = Utc::now().date_naive().format("%Y-%m-%d").to_string();
        let events_path = crate::config::events_path();

        let lines = vec![format!(
            r#"{{"event":"ai_cost","ts":"{today}T10:00:00Z","agent_name":"chat","provider":"anthropic","model":"claude-sonnet-4-6","tokens":{{"input_tokens":1000,"output_tokens":500,"cache_read_tokens":0,"cache_write_tokens":0}},"cost":{{"total_cost_usd":0.01}},"pricing_source":"BuiltinDefault"}}"#
        )];
        std::fs::create_dir_all(events_path.parent().unwrap()).unwrap();
        let mut f = std::fs::File::create(&events_path).unwrap();
        for line in &lines {
            use std::io::Write;
            writeln!(f, "{line}").unwrap();
        }

        let result = run_costs(None, None, GroupBy::Agent, None, true);
        assert!(result.is_ok());
    }

    #[test]
    fn cli_costs_empty_events_file_shows_zero_total() {
        let _lock = crate::TEST_HOME_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };
        crate::config::Config::ensure_dirs().unwrap();

        let events_path = crate::config::events_path();
        std::fs::create_dir_all(events_path.parent().unwrap()).unwrap();
        std::fs::File::create(&events_path).unwrap();

        let result = run_costs(None, None, GroupBy::Day, None, true);
        assert!(result.is_ok());
    }

    #[test]
    fn cli_costs_missing_events_file_shows_zero_total() {
        let _lock = crate::TEST_HOME_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };

        let result = run_costs(None, None, GroupBy::Day, None, true);
        assert!(result.is_ok());
    }

    #[test]
    fn cli_costs_multi_segment_groups_re_sorted_by_cost() {
        let _lock = crate::TEST_HOME_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };
        crate::config::Config::ensure_dirs().unwrap();

        // Two dated segments: seg1 has "high"=$100 and "low"=$1;
        // seg2 has "low"=$200. Merged: "low"=$201 must sort before "high"=$100,
        // even though "high" appeared first (in the earlier segment). This pins
        // the bug-01-1 fix: without the post-merge re-sort, "high" would keep
        // its earlier-segment lead position and break descending-cost order.
        let events_dir = crate::config::events_dir();
        std::fs::create_dir_all(&events_dir).unwrap();

        let seg1 = events_dir.join("events-20260101.jsonl");
        let seg2 = events_dir.join("events-20260102.jsonl");

        let lines1 = concat!(
            r#"{"event":"ai_cost","ts":"2026-01-01T10:00:00Z","agent_name":"high","provider":"anthropic","model":"claude-sonnet-4-6","tokens":{"input_tokens":1000,"output_tokens":500,"cache_read_tokens":0,"cache_write_tokens":0},"cost":{"total_cost_usd":100.0},"pricing_source":"BuiltinDefault"}"#,
            "\n",
            r#"{"event":"ai_cost","ts":"2026-01-01T11:00:00Z","agent_name":"low","provider":"anthropic","model":"claude-sonnet-4-6","tokens":{"input_tokens":1000,"output_tokens":500,"cache_read_tokens":0,"cache_write_tokens":0},"cost":{"total_cost_usd":1.0},"pricing_source":"BuiltinDefault"}"#,
            "\n",
        );
        let lines2 = concat!(
            r#"{"event":"ai_cost","ts":"2026-01-02T10:00:00Z","agent_name":"low","provider":"anthropic","model":"claude-sonnet-4-6","tokens":{"input_tokens":1000,"output_tokens":500,"cache_read_tokens":0,"cache_write_tokens":0},"cost":{"total_cost_usd":200.0},"pricing_source":"BuiltinDefault"}"#,
            "\n",
        );

        std::fs::write(&seg1, lines1).unwrap();
        std::fs::write(&seg2, lines2).unwrap();

        let since_dt = NaiveDate::from_ymd_opt(2026, 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc();
        let until_dt = NaiveDate::from_ymd_opt(2026, 1, 3)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc();

        // Exercise the real code path run_costs uses (reads segments from disk,
        // merges by key, re-sorts) — not a re-implementation of it.
        let summary = aggregate_over_range(since_dt, until_dt, GroupBy::Agent, None);

        // "low" ($201) must come before "high" ($100).
        assert_eq!(summary.groups.len(), 2);
        assert_eq!(summary.groups[0].key, "low");
        assert!((summary.groups[0].total_cost_usd - 201.0).abs() < 1e-10);
        assert_eq!(summary.groups[1].key, "high");
        assert!((summary.groups[1].total_cost_usd - 100.0).abs() < 1e-10);
    }
}

use chrono::Local;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecentCommand {
    pub id: usize,
    pub cmd: String,
    pub timestamp: String,
    pub mode: String,
    pub approval: String,
    pub status: String,
}

static COMMANDS_FG_SUCCEEDED: AtomicUsize = AtomicUsize::new(0);
static COMMANDS_FG_FAILED: AtomicUsize = AtomicUsize::new(0);
static COMMANDS_FG_APPROVED: AtomicUsize = AtomicUsize::new(0);
static COMMANDS_FG_DENIED: AtomicUsize = AtomicUsize::new(0);
static COMMANDS_BG_SUCCEEDED: AtomicUsize = AtomicUsize::new(0);
static COMMANDS_BG_FAILED: AtomicUsize = AtomicUsize::new(0);
static COMMANDS_BG_APPROVED: AtomicUsize = AtomicUsize::new(0);
static COMMANDS_BG_DENIED: AtomicUsize = AtomicUsize::new(0);
static COMMANDS_SCHED_SUCCEEDED: AtomicUsize = AtomicUsize::new(0);
static COMMANDS_SCHED_FAILED: AtomicUsize = AtomicUsize::new(0);

static SCRIPTS_APPROVED: AtomicUsize = AtomicUsize::new(0);
static SCRIPTS_DENIED: AtomicUsize = AtomicUsize::new(0);
static RUNBOOKS_APPROVED: AtomicUsize = AtomicUsize::new(0);
static RUNBOOKS_DENIED: AtomicUsize = AtomicUsize::new(0);
static FILE_EDITS_APPROVED: AtomicUsize = AtomicUsize::new(0);
static FILE_EDITS_DENIED: AtomicUsize = AtomicUsize::new(0);

static WEBHOOKS_RECEIVED: AtomicUsize = AtomicUsize::new(0);
static WEBHOOKS_REJECTED: AtomicUsize = AtomicUsize::new(0);

static RUNBOOKS_CREATED: AtomicUsize = AtomicUsize::new(0);
static RUNBOOKS_EXECUTED: AtomicUsize = AtomicUsize::new(0);
static RUNBOOKS_DELETED: AtomicUsize = AtomicUsize::new(0);

static SCRIPTS_CREATED: AtomicUsize = AtomicUsize::new(0);
static SCRIPTS_EXECUTED: AtomicUsize = AtomicUsize::new(0);
static SCRIPTS_DELETED: AtomicUsize = AtomicUsize::new(0);

static MEMORIES_CREATED: AtomicUsize = AtomicUsize::new(0);
static MEMORIES_RECALLED: AtomicUsize = AtomicUsize::new(0);
static MEMORIES_DELETED: AtomicUsize = AtomicUsize::new(0);

static SCHEDULES_CREATED: AtomicUsize = AtomicUsize::new(0);
static SCHEDULES_EXECUTED: AtomicUsize = AtomicUsize::new(0);
static SCHEDULES_DELETED: AtomicUsize = AtomicUsize::new(0);

static GHOSTS_LAUNCHED: AtomicUsize = AtomicUsize::new(0);
static GHOSTS_ACTIVE: AtomicUsize = AtomicUsize::new(0);
static GHOSTS_COMPLETED: AtomicUsize = AtomicUsize::new(0);
static GHOSTS_FAILED: AtomicUsize = AtomicUsize::new(0);

static COMPACTIONS: AtomicUsize = AtomicUsize::new(0);
static COMPACTION_MSGS_IN: AtomicUsize = AtomicUsize::new(0);
static COMPACTION_MSGS_OUT: AtomicUsize = AtomicUsize::new(0);

static NEXT_CMD_ID: AtomicUsize = AtomicUsize::new(1);
static RECENT_COMMANDS: Mutex<VecDeque<RecentCommand>> = Mutex::new(VecDeque::new());

pub fn start_command(cmd: &str, mode: &str) -> usize {
    let id = NEXT_CMD_ID.fetch_add(1, Ordering::Relaxed);

    let recent = RecentCommand {
        id,
        cmd: cmd.to_string(),
        timestamp: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        mode: mode.to_string(),
        approval: "approved".to_string(),
        status: "pending".to_string(),
    };

    if let Ok(mut cmds) = RECENT_COMMANDS.lock() {
        if cmds.len() >= 6 {
            cmds.pop_front();
        }
        cmds.push_back(recent);
    }

    id
}

pub fn finish_command(id: usize, exit_code: i32) {
    let mut is_fg = false;
    let mut is_bg = false;
    let mut is_sched = false;
    if let Ok(mut cmds) = RECENT_COMMANDS.lock()
        && let Some(cmd) = cmds.iter_mut().find(|c| c.id == id)
    {
        let success = exit_code == 0;
        cmd.status = if success {
            "succeeded".to_string()
        } else {
            format!("failed ({})", exit_code)
        };
        if cmd.mode == "foreground" {
            is_fg = true;
        } else if cmd.mode == "scheduled" {
            is_sched = true;
        } else {
            is_bg = true;
        }
    }

    if is_fg {
        if exit_code == 0 {
            COMMANDS_FG_SUCCEEDED.fetch_add(1, Ordering::Relaxed);
        } else {
            COMMANDS_FG_FAILED.fetch_add(1, Ordering::Relaxed);
        }
    } else if is_sched {
        if exit_code == 0 {
            COMMANDS_SCHED_SUCCEEDED.fetch_add(1, Ordering::Relaxed);
        } else {
            COMMANDS_SCHED_FAILED.fetch_add(1, Ordering::Relaxed);
        }
    } else if is_bg {
        if exit_code == 0 {
            COMMANDS_BG_SUCCEEDED.fetch_add(1, Ordering::Relaxed);
        } else {
            COMMANDS_BG_FAILED.fetch_add(1, Ordering::Relaxed);
        }
    }
}

pub fn record_webhook() {
    WEBHOOKS_RECEIVED.fetch_add(1, Ordering::Relaxed);
}
pub fn record_webhook_rejected() {
    WEBHOOKS_REJECTED.fetch_add(1, Ordering::Relaxed);
}
pub fn get_webhooks_received() -> usize {
    WEBHOOKS_RECEIVED.load(Ordering::Relaxed)
}
pub fn get_webhooks_rejected() -> usize {
    WEBHOOKS_REJECTED.load(Ordering::Relaxed)
}

pub fn get_commands_fg_succeeded() -> usize {
    COMMANDS_FG_SUCCEEDED.load(Ordering::Relaxed)
}
pub fn get_commands_fg_failed() -> usize {
    COMMANDS_FG_FAILED.load(Ordering::Relaxed)
}
pub fn get_commands_fg_approved() -> usize {
    COMMANDS_FG_APPROVED.load(Ordering::Relaxed)
}
pub fn get_commands_fg_denied() -> usize {
    COMMANDS_FG_DENIED.load(Ordering::Relaxed)
}
pub fn get_commands_bg_succeeded() -> usize {
    COMMANDS_BG_SUCCEEDED.load(Ordering::Relaxed)
}
pub fn get_commands_bg_failed() -> usize {
    COMMANDS_BG_FAILED.load(Ordering::Relaxed)
}
pub fn get_commands_bg_approved() -> usize {
    COMMANDS_BG_APPROVED.load(Ordering::Relaxed)
}
pub fn get_commands_bg_denied() -> usize {
    COMMANDS_BG_DENIED.load(Ordering::Relaxed)
}
pub fn inc_commands_fg_approved() {
    COMMANDS_FG_APPROVED.fetch_add(1, Ordering::Relaxed);
}
pub fn inc_commands_fg_denied() {
    COMMANDS_FG_DENIED.fetch_add(1, Ordering::Relaxed);
}
pub fn inc_commands_bg_approved() {
    COMMANDS_BG_APPROVED.fetch_add(1, Ordering::Relaxed);
}
pub fn inc_commands_bg_denied() {
    COMMANDS_BG_DENIED.fetch_add(1, Ordering::Relaxed);
}
pub fn inc_scripts_approved() {
    SCRIPTS_APPROVED.fetch_add(1, Ordering::Relaxed);
}
pub fn inc_scripts_denied() {
    SCRIPTS_DENIED.fetch_add(1, Ordering::Relaxed);
}
pub fn get_scripts_approved() -> usize {
    SCRIPTS_APPROVED.load(Ordering::Relaxed)
}
pub fn get_scripts_denied() -> usize {
    SCRIPTS_DENIED.load(Ordering::Relaxed)
}
pub fn inc_runbooks_approved() {
    RUNBOOKS_APPROVED.fetch_add(1, Ordering::Relaxed);
}
pub fn inc_runbooks_denied() {
    RUNBOOKS_DENIED.fetch_add(1, Ordering::Relaxed);
}
pub fn get_runbooks_approved() -> usize {
    RUNBOOKS_APPROVED.load(Ordering::Relaxed)
}
pub fn get_runbooks_denied() -> usize {
    RUNBOOKS_DENIED.load(Ordering::Relaxed)
}
pub fn inc_file_edits_approved() {
    FILE_EDITS_APPROVED.fetch_add(1, Ordering::Relaxed);
}
pub fn inc_file_edits_denied() {
    FILE_EDITS_DENIED.fetch_add(1, Ordering::Relaxed);
}
pub fn get_file_edits_approved() -> usize {
    FILE_EDITS_APPROVED.load(Ordering::Relaxed)
}
pub fn get_file_edits_denied() -> usize {
    FILE_EDITS_DENIED.load(Ordering::Relaxed)
}
pub fn get_commands_sched_succeeded() -> usize {
    COMMANDS_SCHED_SUCCEEDED.load(Ordering::Relaxed)
}
pub fn get_commands_sched_failed() -> usize {
    COMMANDS_SCHED_FAILED.load(Ordering::Relaxed)
}

pub fn get_recent_commands() -> Vec<crate::ipc::RecentCommand> {
    if let Ok(cmds) = RECENT_COMMANDS.lock() {
        cmds.iter()
            .map(|c| crate::ipc::RecentCommand {
                id: c.id,
                cmd: c.cmd.clone(),
                timestamp: c.timestamp.clone(),
                mode: c.mode.clone(),
                approval: c.approval.clone(),
                status: c.status.clone(),
            })
            .collect()
    } else {
        Vec::new()
    }
}

// Ecosystem Counters
pub fn inc_runbooks_created() {
    RUNBOOKS_CREATED.fetch_add(1, Ordering::Relaxed);
}
pub fn inc_runbooks_executed() {
    RUNBOOKS_EXECUTED.fetch_add(1, Ordering::Relaxed);
}
pub fn inc_runbooks_deleted() {
    RUNBOOKS_DELETED.fetch_add(1, Ordering::Relaxed);
}
pub fn get_runbooks_created() -> usize {
    RUNBOOKS_CREATED.load(Ordering::Relaxed)
}
pub fn get_runbooks_executed() -> usize {
    RUNBOOKS_EXECUTED.load(Ordering::Relaxed)
}
pub fn get_runbooks_deleted() -> usize {
    RUNBOOKS_DELETED.load(Ordering::Relaxed)
}

pub fn inc_scripts_created() {
    SCRIPTS_CREATED.fetch_add(1, Ordering::Relaxed);
}
pub fn inc_scripts_executed() {
    SCRIPTS_EXECUTED.fetch_add(1, Ordering::Relaxed);
}
pub fn inc_scripts_deleted() {
    SCRIPTS_DELETED.fetch_add(1, Ordering::Relaxed);
}
pub fn get_scripts_created() -> usize {
    SCRIPTS_CREATED.load(Ordering::Relaxed)
}
pub fn get_scripts_executed() -> usize {
    SCRIPTS_EXECUTED.load(Ordering::Relaxed)
}
pub fn get_scripts_deleted() -> usize {
    SCRIPTS_DELETED.load(Ordering::Relaxed)
}

pub fn inc_memories_created() {
    MEMORIES_CREATED.fetch_add(1, Ordering::Relaxed);
}
pub fn inc_memories_recalled() {
    MEMORIES_RECALLED.fetch_add(1, Ordering::Relaxed);
}
pub fn inc_memories_deleted() {
    MEMORIES_DELETED.fetch_add(1, Ordering::Relaxed);
}
pub fn get_memories_created() -> usize {
    MEMORIES_CREATED.load(Ordering::Relaxed)
}
pub fn get_memories_recalled() -> usize {
    MEMORIES_RECALLED.load(Ordering::Relaxed)
}
pub fn get_memories_deleted() -> usize {
    MEMORIES_DELETED.load(Ordering::Relaxed)
}

// G5: tiered memory prompt stats
static STABLE_BLOCK_REBUILDS: AtomicUsize = AtomicUsize::new(0);
static DYNAMIC_BLOCK_EMPTY_TURNS: AtomicUsize = AtomicUsize::new(0);
static MEMORIES_IN_STABLE_BLOCK: AtomicUsize = AtomicUsize::new(0);
static MEMORIES_IN_DYNAMIC_BLOCK_AVG: AtomicUsize = AtomicUsize::new(0);
static MEMORIES_PINNED: AtomicUsize = AtomicUsize::new(0);
static MEMORIES_UNPINNED: AtomicUsize = AtomicUsize::new(0);

pub fn inc_stable_block_rebuilds() {
    STABLE_BLOCK_REBUILDS.fetch_add(1, Ordering::Relaxed);
}
pub fn get_stable_block_rebuilds() -> usize {
    STABLE_BLOCK_REBUILDS.load(Ordering::Relaxed)
}
pub fn set_memories_in_stable_block(count: usize) {
    MEMORIES_IN_STABLE_BLOCK.store(count, Ordering::Relaxed);
}
pub fn get_memories_in_stable_block() -> usize {
    MEMORIES_IN_STABLE_BLOCK.load(Ordering::Relaxed)
}
pub fn inc_dynamic_block_empty_turns() {
    DYNAMIC_BLOCK_EMPTY_TURNS.fetch_add(1, Ordering::Relaxed);
}
pub fn get_dynamic_block_empty_turns() -> usize {
    DYNAMIC_BLOCK_EMPTY_TURNS.load(Ordering::Relaxed)
}
pub fn set_memories_in_dynamic_block_avg(count: usize) {
    MEMORIES_IN_DYNAMIC_BLOCK_AVG.store(count, Ordering::Relaxed);
}
pub fn get_memories_in_dynamic_block_avg() -> usize {
    MEMORIES_IN_DYNAMIC_BLOCK_AVG.load(Ordering::Relaxed)
}
pub fn inc_memories_pinned() {
    MEMORIES_PINNED.fetch_add(1, Ordering::Relaxed);
}
pub fn get_memories_pinned() -> usize {
    MEMORIES_PINNED.load(Ordering::Relaxed)
}
pub fn inc_memories_unpinned() {
    MEMORIES_UNPINNED.fetch_add(1, Ordering::Relaxed);
}
pub fn get_memories_unpinned() -> usize {
    MEMORIES_UNPINNED.load(Ordering::Relaxed)
}

pub fn inc_schedules_created() {
    SCHEDULES_CREATED.fetch_add(1, Ordering::Relaxed);
}
pub fn inc_schedules_executed() {
    SCHEDULES_EXECUTED.fetch_add(1, Ordering::Relaxed);
}
pub fn inc_schedules_deleted() {
    SCHEDULES_DELETED.fetch_add(1, Ordering::Relaxed);
}
pub fn get_schedules_created() -> usize {
    SCHEDULES_CREATED.load(Ordering::Relaxed)
}
pub fn get_schedules_executed() -> usize {
    SCHEDULES_EXECUTED.load(Ordering::Relaxed)
}
pub fn get_schedules_deleted() -> usize {
    SCHEDULES_DELETED.load(Ordering::Relaxed)
}

pub fn inc_ghosts_launched() {
    GHOSTS_LAUNCHED.fetch_add(1, Ordering::Relaxed);
    GHOSTS_ACTIVE.fetch_add(1, Ordering::Relaxed);
}
pub fn dec_ghosts_active() {
    // Saturating to guard against any double-decrement.
    GHOSTS_ACTIVE
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
            Some(v.saturating_sub(1))
        })
        .ok();
}
pub fn inc_ghosts_completed() {
    GHOSTS_COMPLETED.fetch_add(1, Ordering::Relaxed);
    dec_ghosts_active();
}
pub fn inc_ghosts_failed() {
    GHOSTS_FAILED.fetch_add(1, Ordering::Relaxed);
    dec_ghosts_active();
}
pub fn get_ghosts_launched() -> usize {
    GHOSTS_LAUNCHED.load(Ordering::Relaxed)
}
pub fn get_ghosts_active() -> usize {
    GHOSTS_ACTIVE.load(Ordering::Relaxed)
}
pub fn get_ghosts_completed() -> usize {
    GHOSTS_COMPLETED.load(Ordering::Relaxed)
}
pub fn get_ghosts_failed() -> usize {
    GHOSTS_FAILED.load(Ordering::Relaxed)
}

pub fn record_compaction(pre: usize, post: usize) {
    COMPACTIONS.fetch_add(1, Ordering::Relaxed);
    COMPACTION_MSGS_IN.fetch_add(pre, Ordering::Relaxed);
    COMPACTION_MSGS_OUT.fetch_add(post, Ordering::Relaxed);
}
pub fn get_compactions() -> usize {
    COMPACTIONS.load(Ordering::Relaxed)
}
/// Returns the average compression ratio (msgs_in / msgs_out) across all compaction events.
/// Returns 0.0 if no compactions have occurred.
pub fn get_compaction_ratio() -> f64 {
    let out = COMPACTION_MSGS_OUT.load(Ordering::Relaxed);
    if out == 0 {
        return 0.0;
    }
    COMPACTION_MSGS_IN.load(Ordering::Relaxed) as f64 / out as f64
}

/// Cached result of today's daemon-wide cost aggregation.
/// The tuple is `(cached_at, total_cost_usd, session_costs)`.
type CostTodayCacheEntry = (Instant, f64, Vec<(String, f64)>);
static COST_TODAY_CACHE: Mutex<Option<CostTodayCacheEntry>> = Mutex::new(None);

/// Cost aggregation result for today.
pub struct CostTodayResult {
    /// Total cost across all sessions today.
    pub total_cost_usd: f64,
    /// Per-session cost breakdown: `(session_id, cost_usd)`.
    pub session_costs: Vec<(String, f64)>,
}

/// Compute today's daemon-wide cost by scanning `events.jsonl` for `ai_cost` events.
///
/// Results are cached for 5 seconds to avoid re-scanning on every status call.
/// Only events with a `timestamp` field falling on today's date (UTC) are included.
pub fn compute_cost_today() -> CostTodayResult {
    const CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(5);

    // Check cache first.
    if let Ok(guard) = COST_TODAY_CACHE.lock()
        && let Some((cached_at, total, sessions)) = guard.as_ref()
        && cached_at.elapsed() < CACHE_TTL
    {
        return CostTodayResult {
            total_cost_usd: *total,
            session_costs: sessions.clone(),
        };
    }

    // Scan events.jsonl for today's ai_cost events.
    let today_start = chrono::Utc::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc();
    let today_end = today_start + chrono::Duration::days(1);

    let mut total: f64 = 0.0;
    let mut by_session: std::collections::HashMap<String, f64> = std::collections::HashMap::new();

    let events_path = crate::config::events_path();
    if let Ok(file) = std::fs::File::open(&events_path) {
        use std::io::BufRead;
        let reader = std::io::BufReader::new(file);
        for line in reader.lines() {
            let Ok(line) = line else { continue };
            let Ok(value): Result<serde_json::Value, _> = serde_json::from_str(&line) else {
                continue;
            };
            // Only process ai_cost events.
            if value.get("event").and_then(|v| v.as_str()) != Some("ai_cost") {
                continue;
            }
            // Parse timestamp and check if it falls within today.
            let Some(ts_str) = value.get("ts").and_then(|v| v.as_str()) else {
                continue;
            };
            let Ok(ts) = chrono::DateTime::parse_from_rfc3339(ts_str) else {
                continue;
            };
            let ts_utc = ts.with_timezone(&chrono::Utc);
            if ts_utc < today_start || ts_utc >= today_end {
                continue;
            }
            // Extract cost.
            let cost = value
                .get("cost")
                .and_then(|c| c.get("total_cost_usd"))
                .and_then(|c| c.as_f64())
                .unwrap_or(0.0);
            total += cost;
            let session_id = value
                .get("session_id")
                .and_then(|v| v.as_str())
                .unwrap_or("-")
                .to_string();
            *by_session.entry(session_id).or_insert(0.0) += cost;
        }
    }

    let mut session_costs: Vec<(String, f64)> = by_session.into_iter().collect();
    session_costs.sort_by(|a, b| a.0.cmp(&b.0));

    // Update cache.
    if let Ok(mut guard) = COST_TODAY_CACHE.lock() {
        *guard = Some((Instant::now(), total, session_costs.clone()));
    }

    CostTodayResult {
        total_cost_usd: total,
        session_costs,
    }
}

/// Reset the cost-today cache. Intended for test use only.
#[cfg(test)]
pub fn reset_cost_today_cache() {
    if let Ok(mut guard) = COST_TODAY_CACHE.lock() {
        *guard = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_cost_today_returns_zero_on_missing_file() {
        let _lock = crate::TEST_HOME_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };
        reset_cost_today_cache();

        let result = compute_cost_today();
        assert!((result.total_cost_usd - 0.0).abs() < 1e-10);
        assert!(result.session_costs.is_empty());
    }

    #[test]
    fn compute_cost_today_caches_result() {
        let _lock = crate::TEST_HOME_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };
        reset_cost_today_cache();

        let r1 = compute_cost_today();
        let r2 = compute_cost_today();
        assert!((r1.total_cost_usd - r2.total_cost_usd).abs() < 1e-10);
    }

    #[test]
    fn daemon_total_cost_today_aggregates_events_jsonl() {
        let _lock = crate::TEST_HOME_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };
        reset_cost_today_cache();

        let events_path = crate::config::events_path();
        std::fs::create_dir_all(events_path.parent().unwrap()).unwrap();
        let today = chrono::Utc::now().to_rfc3339();
        let line1 = format!(
            r#"{{"event":"ai_cost","ts":"{today}","session_id":"s1","cost":{{"total_cost_usd":0.10}}}}"#
        );
        let line2 = format!(
            r#"{{"event":"ai_cost","ts":"{today}","session_id":"s1","cost":{{"total_cost_usd":0.05}}}}"#
        );
        std::fs::write(&events_path, format!("{}\n{}\n", line1, line2)).unwrap();

        let result = compute_cost_today();

        assert!((result.total_cost_usd - 0.15).abs() < 1e-10);
        assert_eq!(result.session_costs.len(), 1);
        assert!((result.session_costs[0].1 - 0.15).abs() < 1e-10);
    }

    #[test]
    fn daemon_total_cost_today_excludes_yesterday() {
        let _lock = crate::TEST_HOME_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };
        reset_cost_today_cache();

        let events_path = crate::config::events_path();
        std::fs::create_dir_all(events_path.parent().unwrap()).unwrap();
        let today = chrono::Utc::now().to_rfc3339();
        let yesterday = (chrono::Utc::now() - chrono::Duration::days(1)).to_rfc3339();
        let old = format!(
            r#"{{"event":"ai_cost","ts":"{yesterday}","session_id":"s1","cost":{{"total_cost_usd":5.00}}}}"#
        );
        let new = format!(
            r#"{{"event":"ai_cost","ts":"{today}","session_id":"s1","cost":{{"total_cost_usd":0.10}}}}"#
        );
        std::fs::write(&events_path, format!("{}\n{}\n", old, new)).unwrap();

        let result = compute_cost_today();

        assert!((result.total_cost_usd - 0.10).abs() < 1e-10);
    }
}

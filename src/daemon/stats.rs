use chrono::Local;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use crate::util::UnpoisonExt;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecentCommand {
    pub id: usize,
    pub cmd: String,
    pub timestamp: String,
    pub mode: String,
    pub approval: String,
    pub status: String,
}

#[derive(Default)]
pub struct CommandCounters {
    pub fg_succeeded: AtomicUsize,
    pub fg_failed: AtomicUsize,
    pub bg_succeeded: AtomicUsize,
    pub bg_failed: AtomicUsize,
    pub sched_succeeded: AtomicUsize,
    pub sched_failed: AtomicUsize,
}

#[derive(Default)]
pub struct ApprovalCounters {
    pub commands_fg_approved: AtomicUsize,
    pub commands_fg_denied: AtomicUsize,
    pub commands_bg_approved: AtomicUsize,
    pub commands_bg_denied: AtomicUsize,
    pub scripts_approved: AtomicUsize,
    pub scripts_denied: AtomicUsize,
    pub runbooks_approved: AtomicUsize,
    pub runbooks_denied: AtomicUsize,
    pub file_edits_approved: AtomicUsize,
    pub file_edits_denied: AtomicUsize,
}

#[derive(Default)]
pub struct ArtifactCounters {
    pub runbooks_created: AtomicUsize,
    pub runbooks_executed: AtomicUsize,
    pub runbooks_deleted: AtomicUsize,
    pub scripts_created: AtomicUsize,
    pub scripts_executed: AtomicUsize,
    pub scripts_deleted: AtomicUsize,
    pub memories_created: AtomicUsize,
    pub memories_recalled: AtomicUsize,
    pub memories_deleted: AtomicUsize,
    pub schedules_created: AtomicUsize,
    pub schedules_executed: AtomicUsize,
    pub schedules_deleted: AtomicUsize,
}

#[derive(Default)]
pub struct MemoryPromptCounters {
    pub stable_block_rebuilds: AtomicUsize,
    pub dynamic_block_empty_turns: AtomicUsize,
    pub memories_in_stable_block: AtomicUsize,
    pub memories_in_dynamic_block_avg: AtomicUsize,
    pub memories_pinned: AtomicUsize,
    pub memories_unpinned: AtomicUsize,
}

#[derive(Default)]
pub struct GhostCounters {
    pub launched: AtomicUsize,
    pub active: AtomicUsize,
    pub completed: AtomicUsize,
    pub failed: AtomicUsize,
}

impl GhostCounters {
    pub fn inc_launched(&self) {
        self.launched.fetch_add(1, Ordering::Relaxed);
        self.active.fetch_add(1, Ordering::Relaxed);
    }

    pub fn dec_active(&self) {
        self.active
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                Some(v.saturating_sub(1))
            })
            .ok();
    }

    pub fn inc_completed(&self) {
        self.completed.fetch_add(1, Ordering::Relaxed);
        self.dec_active();
    }

    pub fn inc_failed(&self) {
        self.failed.fetch_add(1, Ordering::Relaxed);
        self.dec_active();
    }

    pub fn release(&self, end: GhostEnd) {
        match end {
            GhostEnd::Completed => self.inc_completed(),
            GhostEnd::Failed => self.inc_failed(),
        }
    }

    pub fn launched(&self) -> usize {
        self.launched.load(Ordering::Relaxed)
    }

    pub fn active(&self) -> usize {
        self.active.load(Ordering::Relaxed)
    }

    pub fn completed(&self) -> usize {
        self.completed.load(Ordering::Relaxed)
    }

    pub fn failed(&self) -> usize {
        self.failed.load(Ordering::Relaxed)
    }
}

#[derive(Default)]
pub struct CompactionCounters {
    pub count: AtomicUsize,
    pub msgs_in: AtomicUsize,
    pub msgs_out: AtomicUsize,
}

impl CompactionCounters {
    pub fn record(&self, pre: usize, post: usize) {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.msgs_in.fetch_add(pre, Ordering::Relaxed);
        self.msgs_out.fetch_add(post, Ordering::Relaxed);
    }

    pub fn count(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }

    pub fn ratio(&self) -> f64 {
        let out = self.msgs_out.load(Ordering::Relaxed);
        if out == 0 {
            return 0.0;
        }
        self.msgs_in.load(Ordering::Relaxed) as f64 / out as f64
    }
}

#[derive(Default)]
pub struct WebhookCounters {
    pub received: AtomicUsize,
    pub rejected: AtomicUsize,
}

/// Cached result of today's daemon-wide cost aggregation.
/// The tuple is `(cached_at, total_cost_usd, session_costs, cost_by_provider, cost_by_agent)`.
type CostTodayCacheEntry = (
    Instant,
    f64,
    Vec<(String, f64)>,
    Vec<(String, f64)>,
    Vec<(String, f64)>,
);

/// Consolidated daemon runtime counters, recent command history, and cost cache.
pub struct DaemonMetrics {
    pub commands: CommandCounters,
    pub approvals: ApprovalCounters,
    pub artifacts: ArtifactCounters,
    pub memory_prompt: MemoryPromptCounters,
    pub ghosts: GhostCounters,
    pub compactions: CompactionCounters,
    pub webhooks: WebhookCounters,
    next_cmd_id: AtomicUsize,
    recent_commands: Mutex<VecDeque<RecentCommand>>,
    cost_today_cache: Mutex<Option<CostTodayCacheEntry>>,
}

impl Default for DaemonMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl DaemonMetrics {
    pub fn new() -> Self {
        Self {
            commands: CommandCounters::default(),
            approvals: ApprovalCounters::default(),
            artifacts: ArtifactCounters::default(),
            memory_prompt: MemoryPromptCounters::default(),
            ghosts: GhostCounters::default(),
            compactions: CompactionCounters::default(),
            webhooks: WebhookCounters::default(),
            next_cmd_id: AtomicUsize::new(1),
            recent_commands: Mutex::new(VecDeque::new()),
            cost_today_cache: Mutex::new(None),
        }
    }

    pub fn start_command(&self, cmd: &str, mode: &str) -> usize {
        let id = self.next_cmd_id.fetch_add(1, Ordering::Relaxed);
        let recent = RecentCommand {
            id,
            cmd: cmd.to_string(),
            timestamp: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            mode: mode.to_string(),
            approval: "approved".to_string(),
            status: "pending".to_string(),
        };
        let mut cmds = self.recent_commands.lock().unwrap_or_log();
        if cmds.len() >= 6 {
            cmds.pop_front();
        }
        cmds.push_back(recent);
        id
    }

    pub fn finish_command(&self, id: usize, exit_code: i32) {
        let mut is_fg = false;
        let mut is_bg = false;
        let mut is_sched = false;
        {
            let mut cmds = self.recent_commands.lock().unwrap_or_log();
            if let Some(cmd) = cmds.iter_mut().find(|c| c.id == id) {
                cmd.status = if exit_code == 0 {
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
        }

        let target = match (is_fg, is_sched, is_bg, exit_code == 0) {
            (true, _, _, true) => Some(&self.commands.fg_succeeded),
            (true, _, _, false) => Some(&self.commands.fg_failed),
            (_, true, _, true) => Some(&self.commands.sched_succeeded),
            (_, true, _, false) => Some(&self.commands.sched_failed),
            (_, _, true, true) => Some(&self.commands.bg_succeeded),
            (_, _, true, false) => Some(&self.commands.bg_failed),
            _ => None,
        };
        if let Some(counter) = target {
            counter.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn recent_commands(&self) -> Vec<crate::ipc::RecentCommand> {
        let cmds = self.recent_commands.lock().unwrap_or_log();
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
    }
}

static GLOBAL_METRICS: OnceLock<Arc<DaemonMetrics>> = OnceLock::new();

/// Access the process-wide `DaemonMetrics` instance.
pub fn global() -> &'static Arc<DaemonMetrics> {
    GLOBAL_METRICS.get_or_init(|| Arc::new(DaemonMetrics::new()))
}

pub fn start_command(cmd: &str, mode: &str) -> usize {
    global().start_command(cmd, mode)
}

pub fn finish_command(id: usize, exit_code: i32) {
    global().finish_command(id, exit_code);
}

pub fn get_recent_commands() -> Vec<crate::ipc::RecentCommand> {
    global().recent_commands()
}

macro_rules! forward_counters {
    (
        inc: [ $($inc_fn:ident => $i_group:ident . $i_field:ident),* $(,)? ],
        get: [ $($get_fn:ident => $g_group:ident . $g_field:ident),* $(,)? ],
        set: [ $($set_fn:ident => $s_group:ident . $s_field:ident),* $(,)? ]
    ) => {
        $(
            pub fn $inc_fn() {
                global().$i_group.$i_field.fetch_add(1, Ordering::Relaxed);
            }
        )*
        $(
            pub fn $get_fn() -> usize {
                global().$g_group.$g_field.load(Ordering::Relaxed)
            }
        )*
        $(
            pub fn $set_fn(count: usize) {
                global().$s_group.$s_field.store(count, Ordering::Relaxed);
            }
        )*
    };
}

forward_counters! {
    inc: [
        record_webhook => webhooks.received,
        record_webhook_rejected => webhooks.rejected,
        inc_commands_fg_approved => approvals.commands_fg_approved,
        inc_commands_fg_denied => approvals.commands_fg_denied,
        inc_commands_bg_approved => approvals.commands_bg_approved,
        inc_commands_bg_denied => approvals.commands_bg_denied,
        inc_scripts_approved => approvals.scripts_approved,
        inc_scripts_denied => approvals.scripts_denied,
        inc_runbooks_approved => approvals.runbooks_approved,
        inc_runbooks_denied => approvals.runbooks_denied,
        inc_file_edits_approved => approvals.file_edits_approved,
        inc_file_edits_denied => approvals.file_edits_denied,
        inc_runbooks_created => artifacts.runbooks_created,
        inc_runbooks_executed => artifacts.runbooks_executed,
        inc_runbooks_deleted => artifacts.runbooks_deleted,
        inc_scripts_created => artifacts.scripts_created,
        inc_scripts_executed => artifacts.scripts_executed,
        inc_scripts_deleted => artifacts.scripts_deleted,
        inc_memories_created => artifacts.memories_created,
        inc_memories_recalled => artifacts.memories_recalled,
        inc_memories_deleted => artifacts.memories_deleted,
        inc_schedules_created => artifacts.schedules_created,
        inc_schedules_executed => artifacts.schedules_executed,
        inc_schedules_deleted => artifacts.schedules_deleted,
        inc_stable_block_rebuilds => memory_prompt.stable_block_rebuilds,
        inc_dynamic_block_empty_turns => memory_prompt.dynamic_block_empty_turns,
        inc_memories_pinned => memory_prompt.memories_pinned,
        inc_memories_unpinned => memory_prompt.memories_unpinned,
    ],
    get: [
        get_webhooks_received => webhooks.received,
        get_webhooks_rejected => webhooks.rejected,
        get_commands_fg_succeeded => commands.fg_succeeded,
        get_commands_fg_failed => commands.fg_failed,
        get_commands_fg_approved => approvals.commands_fg_approved,
        get_commands_fg_denied => approvals.commands_fg_denied,
        get_commands_bg_succeeded => commands.bg_succeeded,
        get_commands_bg_failed => commands.bg_failed,
        get_commands_bg_approved => approvals.commands_bg_approved,
        get_commands_bg_denied => approvals.commands_bg_denied,
        get_commands_sched_succeeded => commands.sched_succeeded,
        get_commands_sched_failed => commands.sched_failed,
        get_scripts_approved => approvals.scripts_approved,
        get_scripts_denied => approvals.scripts_denied,
        get_runbooks_approved => approvals.runbooks_approved,
        get_runbooks_denied => approvals.runbooks_denied,
        get_file_edits_approved => approvals.file_edits_approved,
        get_file_edits_denied => approvals.file_edits_denied,
        get_runbooks_created => artifacts.runbooks_created,
        get_runbooks_executed => artifacts.runbooks_executed,
        get_runbooks_deleted => artifacts.runbooks_deleted,
        get_scripts_created => artifacts.scripts_created,
        get_scripts_executed => artifacts.scripts_executed,
        get_scripts_deleted => artifacts.scripts_deleted,
        get_memories_created => artifacts.memories_created,
        get_memories_recalled => artifacts.memories_recalled,
        get_memories_deleted => artifacts.memories_deleted,
        get_schedules_created => artifacts.schedules_created,
        get_schedules_executed => artifacts.schedules_executed,
        get_schedules_deleted => artifacts.schedules_deleted,
        get_stable_block_rebuilds => memory_prompt.stable_block_rebuilds,
        get_memories_in_stable_block => memory_prompt.memories_in_stable_block,
        get_dynamic_block_empty_turns => memory_prompt.dynamic_block_empty_turns,
        get_memories_in_dynamic_block_avg => memory_prompt.memories_in_dynamic_block_avg,
        get_memories_pinned => memory_prompt.memories_pinned,
        get_memories_unpinned => memory_prompt.memories_unpinned,
    ],
    set: [
        set_memories_in_stable_block => memory_prompt.memories_in_stable_block,
        set_memories_in_dynamic_block_avg => memory_prompt.memories_in_dynamic_block_avg,
    ]
}

pub fn inc_ghosts_launched() {
    global().ghosts.inc_launched();
}
pub fn dec_ghosts_active() {
    global().ghosts.dec_active();
}
pub fn inc_ghosts_completed() {
    global().ghosts.inc_completed();
}
pub fn inc_ghosts_failed() {
    global().ghosts.inc_failed();
}
pub fn get_ghosts_launched() -> usize {
    global().ghosts.launched()
}
pub fn get_ghosts_active() -> usize {
    global().ghosts.active()
}
pub fn get_ghosts_completed() -> usize {
    global().ghosts.completed()
}
pub fn get_ghosts_failed() -> usize {
    global().ghosts.failed()
}

/// How a ghost run ended, as `GhostSlot::finish` records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GhostEnd {
    Completed,
    Failed,
}

/// One ghost run's concurrency slot, claimed by `inc_ghosts_launched` in
/// `start_session_with_config` (or directly via [`GhostSlot::claim`]) and
/// released exactly once, however the run ends — `Ok`, `Err`, or the run's
/// future being dropped.
pub struct GhostSlot {
    metrics: Option<Arc<DaemonMetrics>>,
}

impl GhostSlot {
    /// Claim a new slot on `metrics` (incrementing `launched` and `active`)
    /// and return an RAII guard that releases it on `finish` or `Drop`.
    pub fn claim(metrics: Arc<DaemonMetrics>) -> Self {
        metrics.ghosts.inc_launched();
        Self {
            metrics: Some(metrics),
        }
    }

    /// Take ownership of an already-launched slot on `metrics`.
    pub fn for_metrics(metrics: Arc<DaemonMetrics>) -> Self {
        Self {
            metrics: Some(metrics),
        }
    }

    /// Hold the slot already claimed on the process-wide `DaemonMetrics`.
    pub fn held() -> Self {
        Self::for_metrics(global().clone())
    }

    /// Release the slot, recording the run's outcome. Exactly once — a
    /// second call, or a later `Drop`, releases nothing.
    pub fn finish(mut self, end: GhostEnd) {
        if let Some(m) = self.metrics.take() {
            m.ghosts.release(end);
        }
    }
}

impl Drop for GhostSlot {
    fn drop(&mut self) {
        if let Some(m) = self.metrics.take() {
            m.ghosts.release(GhostEnd::Failed);
        }
    }
}

pub fn record_compaction(pre: usize, post: usize) {
    global().compactions.record(pre, post);
}
pub fn get_compactions() -> usize {
    global().compactions.count()
}
/// Returns the average compression ratio (msgs_in / msgs_out) across all compaction events.
/// Returns 0.0 if no compactions have occurred.
pub fn get_compaction_ratio() -> f64 {
    global().compactions.ratio()
}

/// Cost aggregation result for today.
pub struct CostTodayResult {
    /// Total cost across all sessions today.
    pub total_cost_usd: f64,
    /// Per-session cost breakdown: `(session_id, cost_usd)`.
    pub session_costs: Vec<(String, f64)>,
    /// Per-provider cost breakdown: `(provider, cost_usd)`.
    pub cost_by_provider: Vec<(String, f64)>,
    /// Per-agent cost breakdown: `(agent_name, cost_usd)`.
    pub cost_by_agent: Vec<(String, f64)>,
}

/// Compute today's daemon-wide cost by scanning `events.jsonl` for `ai_cost` events.
///
/// Results are cached for 5 seconds to avoid re-scanning on every status call.
/// Only events with a `timestamp` field falling on today's date (UTC) are included.
pub fn compute_cost_today() -> CostTodayResult {
    global().compute_cost_today()
}

impl DaemonMetrics {
    pub fn compute_cost_today(&self) -> CostTodayResult {
        const CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(5);

        // Check cache first.
        {
            let guard = self.cost_today_cache.lock().unwrap_or_log();
            if let Some((cached_at, total, sessions, by_provider, by_agent)) = guard.as_ref()
                && cached_at.elapsed() < CACHE_TTL
            {
                return CostTodayResult {
                    total_cost_usd: *total,
                    session_costs: sessions.clone(),
                    cost_by_provider: by_provider.clone(),
                    cost_by_agent: by_agent.clone(),
                };
            }
        }

        // Scan event segments for today's ai_cost events.
        let today_start = chrono::Utc::now()
            .date_naive()
            .and_hms_opt(0, 0, 0)
            // INVARIANT: midnight (0, 0, 0) is always a valid NaiveTime
            .unwrap()
            .and_utc();
        let today_end = today_start + chrono::Duration::days(1);

        let mut total: f64 = 0.0;
        let mut by_session: std::collections::HashMap<String, f64> =
            std::collections::HashMap::new();
        let mut by_provider: std::collections::HashMap<String, f64> =
            std::collections::HashMap::new();
        let mut by_agent: std::collections::HashMap<String, f64> = std::collections::HashMap::new();

        crate::daemon::utils::for_each_event_between(
            Some(today_start),
            Some(today_end),
            &mut |value| {
                // Only process ai_cost events.
                if value.get("event").and_then(|v| v.as_str()) != Some("ai_cost") {
                    return;
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
                if let Some(provider) = value.get("provider").and_then(|v| v.as_str()) {
                    *by_provider.entry(provider.to_string()).or_insert(0.0) += cost;
                }
                if let Some(agent) = value.get("agent_name").and_then(|v| v.as_str()) {
                    *by_agent.entry(agent.to_string()).or_insert(0.0) += cost;
                }
            },
        );

        let mut session_costs: Vec<(String, f64)> = by_session.into_iter().collect();
        session_costs.sort_by(|a, b| a.0.cmp(&b.0));

        let mut provider_costs: Vec<(String, f64)> = by_provider.into_iter().collect();
        provider_costs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let mut agent_costs: Vec<(String, f64)> = by_agent.into_iter().collect();
        agent_costs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Update cache.
        *self.cost_today_cache.lock().unwrap_or_log() = Some((
            Instant::now(),
            total,
            session_costs.clone(),
            provider_costs.clone(),
            agent_costs.clone(),
        ));

        CostTodayResult {
            total_cost_usd: total,
            session_costs,
            cost_by_provider: provider_costs,
            cost_by_agent: agent_costs,
        }
    }
}

/// Reset the cost-today cache. Intended for test use only.
#[cfg(test)]
pub fn reset_cost_today_cache() {
    *global().cost_today_cache.lock().unwrap_or_log() = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_cost_today_returns_zero_on_missing_file() {
        let _lock = crate::test_home_guard();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };
        reset_cost_today_cache();

        let result = compute_cost_today();
        assert!((result.total_cost_usd - 0.0).abs() < 1e-10);
        assert!(result.session_costs.is_empty());
    }

    #[test]
    fn compute_cost_today_caches_result() {
        let _lock = crate::test_home_guard();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };
        reset_cost_today_cache();

        let r1 = compute_cost_today();
        let r2 = compute_cost_today();
        assert!((r1.total_cost_usd - r2.total_cost_usd).abs() < 1e-10);
    }

    #[test]
    fn daemon_total_cost_today_aggregates_events_jsonl() {
        let _lock = crate::test_home_guard();
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
        let _lock = crate::test_home_guard();
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

    #[test]
    fn compute_cost_today_aggregates_by_provider() {
        let _lock = crate::test_home_guard();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };
        reset_cost_today_cache();

        let events_path = crate::config::events_path();
        std::fs::create_dir_all(events_path.parent().unwrap()).unwrap();
        let today = chrono::Utc::now().to_rfc3339();
        let line1 = format!(
            r#"{{"event":"ai_cost","ts":"{today}","session_id":"s1","provider":"anthropic","cost":{{"total_cost_usd":0.10}}}}"#
        );
        let line2 = format!(
            r#"{{"event":"ai_cost","ts":"{today}","session_id":"s1","provider":"gemini","cost":{{"total_cost_usd":0.05}}}}"#
        );
        let line3 = format!(
            r#"{{"event":"ai_cost","ts":"{today}","session_id":"s1","provider":"anthropic","cost":{{"total_cost_usd":0.20}}}}"#
        );
        std::fs::write(&events_path, format!("{}\n{}\n{}\n", line1, line2, line3)).unwrap();

        let result = compute_cost_today();

        assert!((result.total_cost_usd - 0.35).abs() < 1e-10);
        assert_eq!(result.cost_by_provider.len(), 2);
        // Sorted by cost descending: anthropic (0.30) first, gemini (0.05) second.
        assert_eq!(result.cost_by_provider[0].0, "anthropic");
        assert!((result.cost_by_provider[0].1 - 0.30).abs() < 1e-10);
        assert_eq!(result.cost_by_provider[1].0, "gemini");
        assert!((result.cost_by_provider[1].1 - 0.05).abs() < 1e-10);
    }

    #[test]
    fn compute_cost_today_aggregates_by_agent() {
        let _lock = crate::test_home_guard();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };
        reset_cost_today_cache();

        let events_path = crate::config::events_path();
        std::fs::create_dir_all(events_path.parent().unwrap()).unwrap();
        let today = chrono::Utc::now().to_rfc3339();
        let line1 = format!(
            r#"{{"event":"ai_cost","ts":"{today}","session_id":"s1","agent_name":"chat","cost":{{"total_cost_usd":0.15}}}}"#
        );
        let line2 = format!(
            r#"{{"event":"ai_cost","ts":"{today}","session_id":"s2","agent_name":"architect","cost":{{"total_cost_usd":0.25}}}}"#
        );
        std::fs::write(&events_path, format!("{}\n{}\n", line1, line2)).unwrap();

        let result = compute_cost_today();

        assert_eq!(result.cost_by_agent.len(), 2);
        // Sorted by cost descending: architect (0.25) first, chat (0.15) second.
        assert_eq!(result.cost_by_agent[0].0, "architect");
        assert!((result.cost_by_agent[0].1 - 0.25).abs() < 1e-10);
        assert_eq!(result.cost_by_agent[1].0, "chat");
        assert!((result.cost_by_agent[1].1 - 0.15).abs() < 1e-10);
    }

    #[test]
    fn compute_cost_today_empty_events_shows_zero_breakdowns() {
        let _lock = crate::test_home_guard();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };
        reset_cost_today_cache();

        let events_path = crate::config::events_path();
        std::fs::create_dir_all(events_path.parent().unwrap()).unwrap();
        std::fs::write(&events_path, "").unwrap();

        let result = compute_cost_today();

        assert!((result.total_cost_usd - 0.0).abs() < 1e-10);
        assert!(result.cost_by_provider.is_empty());
        assert!(result.cost_by_agent.is_empty());
    }

    // ── GhostSlot (M36 phase-13 B2 / Phase 03 DaemonMetrics injection) ────
    //
    // Each test constructs its own isolated `Arc<DaemonMetrics>`, so
    // assertions observe `m.ghosts.launched()`, `m.ghosts.active()`,
    // `m.ghosts.completed()`, and `m.ghosts.failed()` directly without
    // touching process-global state or thread-local recorders.

    #[test]
    fn ghost_slot_finish_completed_releases_once() {
        let m = Arc::new(DaemonMetrics::new());
        let slot = GhostSlot::claim(m.clone());
        assert_eq!(m.ghosts.launched(), 1);
        assert_eq!(m.ghosts.active(), 1);
        slot.finish(GhostEnd::Completed);
        assert_eq!(m.ghosts.active(), 0);
        assert_eq!(m.ghosts.completed(), 1);
        assert_eq!(m.ghosts.failed(), 0);
    }

    #[test]
    fn ghost_slot_finish_failed_releases_once() {
        let m = Arc::new(DaemonMetrics::new());
        let slot = GhostSlot::claim(m.clone());
        assert_eq!(m.ghosts.active(), 1);
        slot.finish(GhostEnd::Failed);
        assert_eq!(m.ghosts.active(), 0);
        assert_eq!(m.ghosts.completed(), 0);
        assert_eq!(m.ghosts.failed(), 1);
    }

    #[test]
    fn ghost_slot_drop_unfinished_releases_as_failed() {
        let m = Arc::new(DaemonMetrics::new());
        {
            let _slot = GhostSlot::claim(m.clone());
            assert_eq!(m.ghosts.active(), 1);
            // Drop fires here, with no `finish` call.
        }
        assert_eq!(m.ghosts.active(), 0);
        assert_eq!(m.ghosts.completed(), 0);
        assert_eq!(
            m.ghosts.failed(),
            1,
            "a slot dropped without finishing releases exactly once, as Failed"
        );
    }
}

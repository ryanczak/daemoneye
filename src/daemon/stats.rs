use chrono::Local;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

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
        if cmds.len() >= 5 {
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
    if let Ok(mut cmds) = RECENT_COMMANDS.lock() {
        if let Some(cmd) = cmds.iter_mut().find(|c| c.id == id) {
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

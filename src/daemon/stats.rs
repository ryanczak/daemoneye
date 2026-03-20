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
    pub exit_code: Option<i32>,
    pub runtime_ms: Option<u64>,
    #[serde(skip)]
    pub start_time: Option<std::time::Instant>,
}

static COMMANDS_EXECUTED: AtomicUsize = AtomicUsize::new(0);
static COMMANDS_SUCCEEDED: AtomicUsize = AtomicUsize::new(0);
static COMMANDS_FAILED: AtomicUsize = AtomicUsize::new(0);
static WEBHOOKS_RECEIVED: AtomicUsize = AtomicUsize::new(0);

static RUNBOOKS_CREATED: AtomicUsize = AtomicUsize::new(0);
static RUNBOOKS_EXECUTED: AtomicUsize = AtomicUsize::new(0);
static SCRIPTS_CREATED: AtomicUsize = AtomicUsize::new(0);
static SCRIPTS_EXECUTED: AtomicUsize = AtomicUsize::new(0);
static MEMORIES_CREATED: AtomicUsize = AtomicUsize::new(0);
static MEMORIES_RECALLED: AtomicUsize = AtomicUsize::new(0);
static SCHEDULES_CREATED: AtomicUsize = AtomicUsize::new(0);
static SCHEDULES_EXECUTED: AtomicUsize = AtomicUsize::new(0);

static NEXT_CMD_ID: AtomicUsize = AtomicUsize::new(1);
static RECENT_COMMANDS: Mutex<VecDeque<RecentCommand>> = Mutex::new(VecDeque::new());

pub fn start_command(cmd: &str) -> usize {
    COMMANDS_EXECUTED.fetch_add(1, Ordering::Relaxed);
    let id = NEXT_CMD_ID.fetch_add(1, Ordering::Relaxed);

    let recent = RecentCommand {
        id,
        cmd: cmd.to_string(),
        timestamp: Local::now().format("%H:%M:%S").to_string(),
        exit_code: None,
        runtime_ms: None,
        start_time: Some(std::time::Instant::now()),
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
    if exit_code == 0 {
        COMMANDS_SUCCEEDED.fetch_add(1, Ordering::Relaxed);
    } else {
        COMMANDS_FAILED.fetch_add(1, Ordering::Relaxed);
    }

    if let Ok(mut cmds) = RECENT_COMMANDS.lock() {
        if let Some(cmd) = cmds.iter_mut().find(|c| c.id == id) {
            cmd.exit_code = Some(exit_code);
            if let Some(start) = cmd.start_time {
                cmd.runtime_ms = Some(start.elapsed().as_millis() as u64);
            }
        }
    }
}

pub fn record_webhook() {
    WEBHOOKS_RECEIVED.fetch_add(1, Ordering::Relaxed);
}
pub fn get_webhooks_received() -> usize {
    WEBHOOKS_RECEIVED.load(Ordering::Relaxed)
}

pub fn get_commands_succeeded() -> usize {
    COMMANDS_SUCCEEDED.load(Ordering::Relaxed)
}
pub fn get_commands_failed() -> usize {
    COMMANDS_FAILED.load(Ordering::Relaxed)
}

pub fn get_recent_commands() -> Vec<crate::ipc::RecentCommand> {
    if let Ok(cmds) = RECENT_COMMANDS.lock() {
        cmds.iter()
            .map(|c| crate::ipc::RecentCommand {
                id: c.id,
                cmd: c.cmd.clone(),
                timestamp: c.timestamp.clone(),
                exit_code: c.exit_code,
                runtime_ms: c.runtime_ms,
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
pub fn get_runbooks_created() -> usize {
    RUNBOOKS_CREATED.load(Ordering::Relaxed)
}
pub fn get_runbooks_executed() -> usize {
    RUNBOOKS_EXECUTED.load(Ordering::Relaxed)
}

pub fn inc_scripts_created() {
    SCRIPTS_CREATED.fetch_add(1, Ordering::Relaxed);
}
pub fn inc_scripts_executed() {
    SCRIPTS_EXECUTED.fetch_add(1, Ordering::Relaxed);
}
pub fn get_scripts_created() -> usize {
    SCRIPTS_CREATED.load(Ordering::Relaxed)
}
pub fn get_scripts_executed() -> usize {
    SCRIPTS_EXECUTED.load(Ordering::Relaxed)
}

pub fn inc_memories_created() {
    MEMORIES_CREATED.fetch_add(1, Ordering::Relaxed);
}
pub fn inc_memories_recalled() {
    MEMORIES_RECALLED.fetch_add(1, Ordering::Relaxed);
}
pub fn get_memories_created() -> usize {
    MEMORIES_CREATED.load(Ordering::Relaxed)
}
pub fn get_memories_recalled() -> usize {
    MEMORIES_RECALLED.load(Ordering::Relaxed)
}

pub fn inc_schedules_created() {
    SCHEDULES_CREATED.fetch_add(1, Ordering::Relaxed);
}
pub fn inc_schedules_executed() {
    SCHEDULES_EXECUTED.fetch_add(1, Ordering::Relaxed);
}
pub fn get_schedules_created() -> usize {
    SCHEDULES_CREATED.load(Ordering::Relaxed)
}
pub fn get_schedules_executed() -> usize {
    SCHEDULES_EXECUTED.load(Ordering::Relaxed)
}

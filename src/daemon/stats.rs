use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

static COMMANDS_EXECUTED: AtomicUsize = AtomicUsize::new(0);
static WEBHOOKS_RECEIVED: AtomicUsize = AtomicUsize::new(0);
static RECENT_COMMANDS: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

/// Increment the count of executed commands and record the command string.
pub fn record_command(cmd: &str) {
    COMMANDS_EXECUTED.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut cmds) = RECENT_COMMANDS.lock() {
        if cmds.len() >= 5 {
            cmds.pop_front();
        }
        cmds.push_back(cmd.to_string());
    }
}

/// Increment the count of successfully parsed webhook alerts.
pub fn record_webhook() {
    WEBHOOKS_RECEIVED.fetch_add(1, Ordering::Relaxed);
}

/// Retrieve the total number of executed commands since daemon start.
pub fn get_commands_executed() -> usize {
    COMMANDS_EXECUTED.load(Ordering::Relaxed)
}

/// Retrieve the total number of parsed webhooks since daemon start.
pub fn get_webhooks_received() -> usize {
    WEBHOOKS_RECEIVED.load(Ordering::Relaxed)
}

/// Retrieve a copy of the last up to 5 executed commands.
pub fn get_recent_commands() -> Vec<String> {
    if let Ok(cmds) = RECENT_COMMANDS.lock() {
        cmds.iter().cloned().collect()
    } else {
        Vec::new()
    }
}

pub mod cache;

use anyhow::Result;
use std::process::Command;

/// Check if a tmux session exists.
pub fn has_session(session_name: &str) -> bool {
    Command::new("tmux")
        .args(["has-session", "-t", session_name])
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Create a new detached tmux session.
pub fn create_session(session_name: &str) -> Result<()> {
    let output = Command::new("tmux")
        .args(["new-session", "-d", "-s", session_name])
        .output()?;
        
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to create tmux session '{}': {}", session_name, err);
    }
    
    Ok(())
}

/// Get the active pane ID in the format 'session:window.pane'
pub fn get_active_pane(session_name: &str) -> Result<String> {
    let output = Command::new("tmux")
        .args(["display-message", "-t", session_name, "-p", "#S:#I.#P"])
        .output()?;
        
    if !output.status.success() {
        anyhow::bail!("Failed to get active pane for session '{}'", session_name);
    }
    
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// List all panes in the session with their IDs and current titles/commands
pub fn list_panes(session_name: &str) -> Result<Vec<String>> {
    let output = Command::new("tmux")
        .args(["list-panes", "-s", "-t", session_name, "-F", "#S:#I.#P"])
        .output()?;
        
    if !output.status.success() {
        anyhow::bail!("Failed to list panes for session '{}'", session_name);
    }
    
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.lines().map(|s| s.to_string()).collect())
}

/// Capture the content of a specific pane.
pub fn capture_pane(pane_id: &str, depth: usize) -> Result<String> {
    let output = Command::new("tmux")
        .args(["capture-pane", "-p", "-t", pane_id, "-S", &format!("-{}", depth)])
        .output()?;
        
    if !output.status.success() {
        anyhow::bail!("Failed to capture pane '{}'", pane_id);
    }
    
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}


/// Send keys (a command) to a specific pane.
pub fn send_keys(pane_id: &str, cmd: &str) -> Result<()> {
    let output = Command::new("tmux")
        .args(["send-keys", "-t", pane_id, cmd, "C-m"])
        .output()?;
        
    if !output.status.success() {
        anyhow::bail!("Failed to send keys to pane '{}'", pane_id);
    }
    
    Ok(())
}

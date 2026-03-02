pub mod cache;

use anyhow::Result;
use std::collections::HashMap;
use std::process::Command;

// ---------------------------------------------------------------------------
// Rich pane metadata (P1 + P2 + P3)
// ---------------------------------------------------------------------------

/// Metadata for a single tmux pane, fetched in one `list-panes` call.
pub struct RichPaneInfo {
    pub pane_id: String,
    pub current_cmd: String,
    /// Absolute path of the shell's working directory (`#{pane_current_path}`).
    pub current_path: String,
    /// Terminal title set by the running application via OSC sequences (`#{pane_title}`).
    pub title: String,
    /// True when the pane's foreground process has exited.
    pub dead: bool,
    /// Exit code of the foreground process if `dead` is true.
    pub dead_status: Option<i32>,
    /// Lines scrolled back from the visible bottom (0 = at bottom, R3).
    pub scroll_position: usize,
    /// Total scrollback history lines available (`#{history_size}`, R3).
    pub history_size: usize,
    /// True when the pane is in copy/scroll mode (`#{pane_in_mode}`, R4).
    pub in_copy_mode: bool,
    /// True when pane input is synchronized with other panes (`#{pane_synchronized}`, R6).
    pub synchronized: bool,
}

/// List all panes in the session with rich metadata using a single tmux call.
///
/// Fields are tab-separated in the format string.  Tab characters cannot appear
/// in pane paths or command names, making `\t` a safe delimiter.
pub fn list_panes_detailed(session: &str) -> Result<Vec<RichPaneInfo>> {
    let output = Command::new("tmux")
        .args([
            "list-panes", "-s", "-t", session, "-F",
            "#{pane_id}\t#{pane_current_command}\t#{pane_current_path}\t#{pane_title}\t#{pane_dead}\t#{pane_dead_status}\t#{scroll_position}\t#{history_size}\t#{pane_in_mode}\t#{pane_synchronized}",
        ])
        .output()?;

    if !output.status.success() {
        anyhow::bail!("Failed to list panes for session '{}'", session);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut panes = Vec::new();
    for line in stdout.lines() {
        let fields: Vec<&str> = line.splitn(10, '\t').collect();
        if fields.len() < 5 {
            continue;
        }
        let dead = fields[4] == "1";
        let dead_status = if dead {
            fields.get(5).and_then(|s| s.trim().parse::<i32>().ok())
        } else {
            None
        };
        panes.push(RichPaneInfo {
            pane_id:         fields[0].to_string(),
            current_cmd:     fields[1].to_string(),
            current_path:    fields[2].to_string(),
            title:           fields[3].to_string(),
            dead,
            dead_status,
            scroll_position: fields.get(6).and_then(|s| s.trim().parse::<usize>().ok()).unwrap_or(0),
            history_size:    fields.get(7).and_then(|s| s.trim().parse::<usize>().ok()).unwrap_or(0),
            in_copy_mode:    fields.get(8).map(|s| s.trim() == "1").unwrap_or(false),
            synchronized:    fields.get(9).map(|s| s.trim() == "1").unwrap_or(false),
        });
    }
    Ok(panes)
}

// ---------------------------------------------------------------------------
// Session environment (P5)
// ---------------------------------------------------------------------------

/// Fetch the tmux session environment and return high-signal variables.
///
/// Only variables on the allowlist are returned.  Values are passed back
/// as-is; callers should run them through `mask_sensitive` before sending to
/// the AI.  Lines prefixed with `-` (unset variables) are skipped.
pub fn session_environment(session: &str) -> Result<HashMap<String, String>> {
    const ALLOWLIST: &[&str] = &[
        // Cloud / infra
        "AWS_PROFILE", "AWS_DEFAULT_REGION", "AWS_REGION",
        "KUBECONFIG", "KUBE_CONTEXT", "KUBECTL_CONTEXT",
        "VAULT_ADDR",
        "DOCKER_HOST", "DOCKER_CONTEXT",
        // App environment tier
        "ENVIRONMENT", "APP_ENV", "NODE_ENV", "RAILS_ENV", "RACK_ENV",
        // Language runtimes
        "VIRTUAL_ENV", "CONDA_DEFAULT_ENV",
        "GOPATH", "GOENV",
        "JAVA_HOME",
        // Locale
        "LANG", "LC_ALL",
    ];

    let output = Command::new("tmux")
        .args(["show-environment", "-t", session])
        .output()?;

    // Not a hard error if unavailable (e.g. session not found).
    if !output.status.success() {
        return Ok(HashMap::new());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut env = HashMap::new();
    for line in stdout.lines() {
        if line.starts_with('-') {
            continue; // variable unset in this session
        }
        if let Some(eq) = line.find('=') {
            let key = &line[..eq];
            let val = &line[eq + 1..];
            if ALLOWLIST.contains(&key) {
                env.insert(key.to_string(), val.to_string());
            }
        }
    }
    Ok(env)
}

// ---------------------------------------------------------------------------
// Pane dead-status (P7)
// ---------------------------------------------------------------------------

/// Query the exit status of the foreground process in a pane.
///
/// Returns `Some(code)` if the pane's foreground process has exited, `None`
/// if the pane is still alive or the status cannot be determined.
pub fn pane_dead_status(pane_id: &str) -> Option<i32> {
    let output = Command::new("tmux")
        .args(["display-message", "-t", pane_id, "-p", "#{pane_dead}\t#{pane_dead_status}"])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&output.stdout);
    let mut parts = s.trim().splitn(2, '\t');
    let dead = parts.next()? == "1";
    if !dead {
        return None;
    }
    parts.next().and_then(|s| s.parse::<i32>().ok())
}

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

/// Get the active pane ID in `#{pane_id}` format (e.g. `%5`).
pub fn get_active_pane(session_name: &str) -> Result<String> {
    let output = Command::new("tmux")
        .args(["display-message", "-t", session_name, "-p", "#{pane_id}"])
        .output()?;
        
    if !output.status.success() {
        anyhow::bail!("Failed to get active pane for session '{}'", session_name);
    }
    
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// List all panes in the session using `#{pane_id}` format (e.g. `%3`, `%5`).
pub fn list_panes(session_name: &str) -> Result<Vec<String>> {
    let output = Command::new("tmux")
        .args(["list-panes", "-s", "-t", session_name, "-F", "#{pane_id}"])
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


/// Capture pane content anchored at a historical scroll position (R3).
///
/// `scroll_pos` is the value of `#{scroll_position}` — lines scrolled back
/// from the current bottom.  0 means the pane is at the bottom (use the
/// regular [`capture_pane`] instead).  `depth` is how many lines to capture.
pub fn capture_pane_at_scroll(pane_id: &str, scroll_pos: usize, depth: usize) -> Result<String> {
    // In tmux line numbering, 0 is the visible bottom; negative numbers go up
    // into scrollback.  When scrolled back N lines, the visible bottom is at
    // offset -(scroll_pos) and the visible top is depth lines above that.
    let end:   i64 = -(scroll_pos as i64);
    let start: i64 = end - depth as i64;
    let output = Command::new("tmux")
        .args([
            "capture-pane", "-p", "-t", pane_id,
            "-S", &start.to_string(),
            "-E", &end.to_string(),
        ])
        .output()?;
    if !output.status.success() {
        anyhow::bail!("Failed to capture pane '{}' at scroll position {}", pane_id, scroll_pos);
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Return the PID of the shell process that owns the given pane.
pub fn pane_pid(pane_id: &str) -> Result<u32> {
    let output = Command::new("tmux")
        .args(["display-message", "-t", pane_id, "-p", "#{pane_pid}"])
        .output()?;
    if !output.status.success() {
        anyhow::bail!("Failed to query pane_pid for '{}'", pane_id);
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u32>()
        .map_err(|e| anyhow::anyhow!("Could not parse pane pid: {}", e))
}

/// Return the name of the foreground process running in a pane.
pub fn pane_current_command(pane_id: &str) -> Result<String> {
    let output = Command::new("tmux")
        .args(["display-message", "-t", pane_id, "-p", "#{pane_current_command}"])
        .output()?;
    if !output.status.success() {
        anyhow::bail!("Failed to query pane_current_command for '{}'", pane_id);
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Query the current width of a pane in columns.
pub fn query_pane_width(pane_id: &str) -> Result<usize> {
    let output = Command::new("tmux")
        .args(["display-message", "-t", pane_id, "-p", "#{pane_width}"])
        .output()?;
    if !output.status.success() {
        anyhow::bail!("Failed to query pane width for '{}'", pane_id);
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<usize>()
        .map_err(|e| anyhow::anyhow!("Could not parse pane width: {}", e))
}

/// Query the height of a pane in rows.
pub fn query_pane_height(pane_id: &str) -> Result<usize> {
    let output = Command::new("tmux")
        .args(["display-message", "-t", pane_id, "-p", "#{pane_height}"])
        .output()?;
    if !output.status.success() {
        anyhow::bail!("Failed to query pane height for '{}'", pane_id);
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<usize>()
        .map_err(|e| anyhow::anyhow!("Could not parse pane height: {}", e))
}

/// Query the width of the window containing a pane in columns.
pub fn query_window_width(pane_id: &str) -> Result<usize> {
    let output = Command::new("tmux")
        .args(["display-message", "-t", pane_id, "-p", "#{window_width}"])
        .output()?;
    if !output.status.success() {
        anyhow::bail!("Failed to query window width for '{}'", pane_id);
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<usize>()
        .map_err(|e| anyhow::anyhow!("Could not parse window width: {}", e))
}

/// Resize a pane to the given number of columns.
pub fn resize_pane_width(pane_id: &str, width: usize) -> Result<()> {
    let output = Command::new("tmux")
        .args(["resize-pane", "-t", pane_id, "-x", &width.to_string()])
        .output()?;
    if !output.status.success() {
        anyhow::bail!("Failed to resize pane '{}'", pane_id);
    }
    Ok(())
}

/// Switch tmux focus to the specified pane.
pub fn select_pane(pane_id: &str) -> Result<()> {
    let output = Command::new("tmux")
        .args(["select-pane", "-t", pane_id])
        .output()?;
    if !output.status.success() {
        anyhow::bail!("Failed to select pane '{}'", pane_id);
    }
    Ok(())
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

/// Create a new detached background window in `session` with the given `name`.
///
/// If a window with that name already exists it is killed first.
/// Returns the pane ID of the new window (e.g. `%12`).
pub fn create_job_window(session: &str, name: &str) -> Result<String> {
    // Silently kill any pre-existing window with that name.
    let _ = Command::new("tmux")
        .args(["kill-window", "-t", &format!("{}:{}", session, name)])
        .output();

    // Use "session:" (trailing colon) so tmux picks the next available window
    // index rather than defaulting to 0 and colliding with existing windows.
    let target = format!("{}:", session);
    let output = Command::new("tmux")
        .args([
            "new-window", "-d",
            "-n", name,
            "-t", &target,
            "-P", "-F", "#{pane_id}",
        ])
        .output()?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to create job window '{}': {}", name, err.trim());
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Kill a background window by name.  Silently ignores missing windows.
pub fn kill_job_window(session: &str, name: &str) -> Result<()> {
    let _ = Command::new("tmux")
        .args(["kill-window", "-t", &format!("{}:{}", session, name)])
        .output();
    Ok(())
}

// ---------------------------------------------------------------------------
// Window inventory (P4)
// ---------------------------------------------------------------------------

pub struct WindowState {
    pub window_id: String,
    pub window_name: String,
    pub active: bool,
    pub pane_count: usize,
    pub zoomed: bool,
    pub last_active: bool,
}

/// Single tmux call; tab-separated format string:
/// "#{window_id}\t#{window_name}\t#{window_active}\t#{window_panes}\t#{window_zoomed_flag}\t#{window_last_flag}"
pub fn list_windows(session: &str) -> Result<Vec<WindowState>> {
    let output = Command::new("tmux")
        .args([
            "list-windows", "-t", session, "-F",
            "#{window_id}\t#{window_name}\t#{window_active}\t#{window_panes}\t#{window_zoomed_flag}\t#{window_last_flag}",
        ])
        .output()?;

    if !output.status.success() {
        anyhow::bail!("Failed to list windows for session '{}'", session);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut windows = Vec::new();
    for line in stdout.lines() {
        let fields: Vec<&str> = line.splitn(6, '\t').collect();
        if fields.len() < 6 {
            continue;
        }
        windows.push(WindowState {
            window_id:   fields[0].to_string(),
            window_name: fields[1].to_string(),
            active:      fields[2] == "1",
            pane_count:  fields[3].parse::<usize>().unwrap_or(1),
            zoomed:      fields[4] == "1",
            last_active: fields[5] == "1",
        });
    }
    Ok(windows)
}

// ---------------------------------------------------------------------------
// Foreground activity hooks (P6)
// ---------------------------------------------------------------------------

pub fn set_monitor_activity(pane_id: &str, enable: bool) -> Result<()> {
    let value = if enable { "on" } else { "off" };
    let output = Command::new("tmux")
        .args(["set-option", "-t", pane_id, "monitor-activity", value])
        .output()?;
    if !output.status.success() {
        anyhow::bail!("Failed to set monitor-activity for pane '{}'", pane_id);
    }
    Ok(())
}

pub fn unset_monitor_activity(pane_id: &str) -> Result<()> {
    let output = Command::new("tmux")
        .args(["set-option", "-u", "-t", pane_id, "monitor-activity"])
        .output()?;
    if !output.status.success() {
        anyhow::bail!("Failed to unset monitor-activity for pane '{}'", pane_id);
    }
    Ok(())
}

pub fn install_activity_hook(session: &str, hook_index: usize, daemon_pid: u32) -> Result<()> {
    let hook_name = format!("alert-activity[{}]", hook_index);
    let cmd = format!("run-shell 'kill -USR1 {}'", daemon_pid);
    let output = Command::new("tmux")
        .args(["set-hook", "-t", session, &hook_name, &cmd])
        .output()?;
    if !output.status.success() {
        anyhow::bail!("Failed to install activity hook for session '{}'", session);
    }
    Ok(())
}

pub fn remove_activity_hook(session: &str, hook_index: usize) -> Result<()> {
    let hook_name = format!("alert-activity[{}]", hook_index);
    let _ = Command::new("tmux")
        .args(["set-hook", "-u", "-t", session, &hook_name])
        .output();
    Ok(())
}

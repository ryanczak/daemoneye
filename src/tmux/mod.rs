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
}

/// List all panes in the session with rich metadata using a single tmux call.
///
/// Fields are tab-separated in the format string.  Tab characters cannot appear
/// in pane paths or command names, making `\t` a safe delimiter.
pub fn list_panes_detailed(session: &str) -> Result<Vec<RichPaneInfo>> {
    let output = Command::new("tmux")
        .args([
            "list-panes", "-s", "-t", session, "-F",
            "#{pane_id}\t#{pane_current_command}\t#{pane_current_path}\t#{pane_title}\t#{pane_dead}\t#{pane_dead_status}",
        ])
        .output()?;

    if !output.status.success() {
        anyhow::bail!("Failed to list panes for session '{}'", session);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut panes = Vec::new();
    for line in stdout.lines() {
        let fields: Vec<&str> = line.splitn(6, '\t').collect();
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
            pane_id:      fields[0].to_string(),
            current_cmd:  fields[1].to_string(),
            current_path: fields[2].to_string(),
            title:        fields[3].to_string(),
            dead,
            dead_status,
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

    let output = Command::new("tmux")
        .args([
            "new-window", "-d",
            "-n", name,
            "-t", session,
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

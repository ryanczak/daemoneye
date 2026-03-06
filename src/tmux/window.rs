use anyhow::Result;
use std::process::Command;

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
            window_id: fields[0].to_string(),
            window_name: fields[1].to_string(),
            active: fields[2] == "1",
            pane_count: fields[3].parse::<usize>().unwrap_or(1),
            zoomed: fields[4] == "1",
            last_active: fields[5] == "1",
        });
    }
    Ok(windows)
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
            "new-window",
            "-d",
            "-n",
            name,
            "-t",
            &target,
            "-P",
            "-F",
            "#{pane_id}",
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


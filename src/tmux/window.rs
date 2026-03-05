use anyhow::Result;
use std::process::Command;
use std::path::Path;

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

/// Ensure that the `de-info` window exists in the session.
/// If not, it creates it with four panes in a 2x2 grid:
/// - Top-Left: tail daemon.log
/// - Top-Right: tail activity.log
/// - Bottom-Left: tail commands.log
/// - Bottom-Right: interactive shell
pub fn ensure_info_window(
    session: &str,
    daemon_log: &std::path::Path,
    activity_log: &std::path::Path,
    commands_log: &std::path::Path,
) -> Result<()> {
    let check = Command::new("tmux")
        .args(["list-windows", "-t", session, "-F", "#{window_name}"])
        .output()?;

    let out = String::from_utf8_lossy(&check.stdout);
    if out.lines().any(|l| l.trim() == "de-info") {
        return Ok(());
    }

    // Touch the files so tail doesn't fail
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(daemon_log);
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(activity_log);
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(commands_log);

    let d_log_str = daemon_log.to_string_lossy();
    let a_log_str = activity_log.to_string_lossy();
    let c_log_str = commands_log.to_string_lossy();

    // 1. Create the window with pane 0 tailing the daemon log
    let cmd1 = format!("tail -f '{}'", d_log_str);
    let out1 = Command::new("tmux")
        .args([
            "new-window",
            "-d",
            "-t",
            &format!("{}:", session),
            "-n",
            "de-info",
            &cmd1,
        ])
        .output()?;
    if !out1.status.success() {
        anyhow::bail!("Failed to create de-info window");
    }

    // 2. Split it vertically (creates a full-width bottom pane) for commands log
    let cmd2 = format!("tail -f '{}'", c_log_str);
    let out2 = Command::new("tmux")
        .args([
            "split-window",
            "-d",
            "-t",
            &format!("{}:de-info", session),
            "-v",
            &cmd2,
        ])
        .output()?;
    if !out2.status.success() {
        anyhow::bail!("Failed to split de-info window (vertical)");
    }

    // 3. Split the top pane horizontally (creates top-right pane) for activity log
    let cmd3 = format!("tail -f '{}'", a_log_str);
    let out3 = Command::new("tmux")
        .args([
            "split-window",
            "-d",
            "-t",
            &format!("{}:de-info.0", session),
            "-h",
            &cmd3,
        ])
        .output()?;
    if !out3.status.success() {
        anyhow::bail!("Failed to split de-info top pane (horizontal)");
    }

    // Turn off remain-on-exit for these panes so if tail dies the pane cleans up
    // We get the pane IDs for tail commands to ensure they close cleanly.
    // Shell pane naturally closes on exit, but doesn't hurt to apply to all panes in the window.
    let _ = Command::new("tmux")
        .args([
            "set-option",
            "-t",
            &format!("{}:de-info", session),
            "-g",
            "remain-on-exit",
            "off",
        ])
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


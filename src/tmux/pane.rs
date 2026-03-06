use anyhow::Result;
use std::process::Command;

/// Metadata for a single tmux pane, fetched in one `list-panes` call.
pub struct RichPaneInfo {
    pub session_name: String,
    pub window_name: String,
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
pub fn list_panes_detailed() -> Result<Vec<RichPaneInfo>> {
    let output = Command::new("tmux")
        .args([
            "list-panes", "-a", "-F",
            "#{session_name}\t#{window_name}\t#{pane_id}\t#{pane_current_command}\t#{pane_current_path}\t#{pane_title}\t#{pane_dead}\t#{pane_dead_status}\t#{scroll_position}\t#{history_size}\t#{pane_in_mode}\t#{pane_synchronized}",
        ])
        .output()?;

    if !output.status.success() {
        anyhow::bail!("Failed to list all panes");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut panes = Vec::new();
    for line in stdout.lines() {
        let fields: Vec<&str> = line.splitn(12, '\t').collect();
        if fields.len() < 12 {
            continue;
        }
        let dead = fields[6] == "1";
        let dead_status = if dead {
            fields.get(7).and_then(|s| s.trim().parse::<i32>().ok())
        } else {
            None
        };
        panes.push(RichPaneInfo {
            session_name: fields[0].to_string(),
            window_name: fields[1].to_string(),
            pane_id: fields[2].to_string(),
            current_cmd: fields[3].to_string(),
            current_path: fields[4].to_string(),
            title: fields[5].to_string(),
            dead,
            dead_status,
            scroll_position: fields
                .get(8)
                .and_then(|s| s.trim().parse::<usize>().ok())
                .unwrap_or(0),
            history_size: fields
                .get(9)
                .and_then(|s| s.trim().parse::<usize>().ok())
                .unwrap_or(0),
            in_copy_mode: fields.get(10).map(|s| s.trim() == "1").unwrap_or(false),
            synchronized: fields.get(11).map(|s| s.trim() == "1").unwrap_or(false),
        });
    }
    Ok(panes)
}

/// Query the exit status of the foreground process in a pane.
///
/// Returns `Some(code)` if the pane's foreground process has exited, `None`
/// if the pane is still alive or the status cannot be determined.
pub fn pane_dead_status(pane_id: &str) -> Option<i32> {
    let output = Command::new("tmux")
        .args([
            "display-message",
            "-t",
            pane_id,
            "-p",
            "#{pane_dead}\t#{pane_dead_status}",
        ])
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

/// Capture the content of a specific pane.
pub fn capture_pane(pane_id: &str, depth: usize) -> Result<String> {
    let output = Command::new("tmux")
        .args([
            "capture-pane",
            "-p",
            "-t",
            pane_id,
            "-S",
            &format!("-{}", depth),
        ])
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
    let end: i64 = -(scroll_pos as i64);
    let start: i64 = end - depth as i64;
    let output = Command::new("tmux")
        .args([
            "capture-pane",
            "-p",
            "-t",
            pane_id,
            "-S",
            &start.to_string(),
            "-E",
            &end.to_string(),
        ])
        .output()?;
    if !output.status.success() {
        anyhow::bail!(
            "Failed to capture pane '{}' at scroll position {}",
            pane_id,
            scroll_pos
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Capture the entire scrollback history of a pane and save it directly to a file.
/// This prevents massive buffers from blowing up memory during daemon GC.
pub fn capture_pane_to_file(pane_id: &str, out_path: &std::path::Path) -> Result<()> {
    let out_path_str = out_path.to_string_lossy().into_owned();
    // Using `tmux capture-pane -S -` captures from the very beginning of the scrollback buffer
    let output = Command::new("tmux")
        .args(["capture-pane", "-S", "-", "-t", pane_id])
        .output()?;
    if !output.status.success() {
        anyhow::bail!("Failed to capture pane '{}' into buffer", pane_id);
    }

    // Save to the specified file path by piping the buffer out
    let output_save = Command::new("tmux")
        .args(["save-buffer", &out_path_str])
        .output()?;
    if !output_save.status.success() {
        anyhow::bail!(
            "Failed to save captured buffer from pane '{}' to file",
            pane_id
        );
    }

    // Clean up the tmux internal buffer
    let _ = Command::new("tmux").args(["delete-buffer"]).output();
    Ok(())
}

/// Return the name of the foreground process running in a pane.
pub fn pane_current_command(pane_id: &str) -> Result<String> {
    let output = Command::new("tmux")
        .args([
            "display-message",
            "-t",
            pane_id,
            "-p",
            "#{pane_current_command}",
        ])
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

/// Set the `remain-on-exit` option for a specific pane.
pub fn set_remain_on_exit(pane_id: &str, enable: bool) -> Result<()> {
    let value = if enable { "on" } else { "off" };
    let output = Command::new("tmux")
        .args(["set-option", "-t", pane_id, "remain-on-exit", value])
        .output()?;
    if !output.status.success() {
        anyhow::bail!("Failed to set remain-on-exit for pane '{}'", pane_id);
    }
    Ok(())
}


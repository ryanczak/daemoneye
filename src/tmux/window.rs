use anyhow::Result;
use std::process::Command;

pub struct WindowState {
    pub window_id: String,
    pub window_name: String,
    pub active: bool,
    pub pane_count: usize,
    pub zoomed: bool,
    pub last_active: bool,
    /// Raw tmux `#{window_flags}` string (e.g. `*!`, `#Z`).
    /// Use [`has_bell`](WindowState::has_bell) and
    /// [`has_activity`](WindowState::has_activity) rather than parsing directly.
    pub flags: String,
}

impl WindowState {
    /// True when tmux is holding an uncleared bell (`!`) for this window.
    ///
    /// The bell flag persists until the user visits the window, making it
    /// reliable for recovering bells that the `alert-bell` hook may have missed
    /// (e.g. during a daemon restart).
    pub fn has_bell(&self) -> bool {
        self.flags.contains('!')
    }

    /// True when tmux has seen new output in this window since it was last
    /// visited and `monitor-activity` is enabled (`#` flag).
    pub fn has_activity(&self) -> bool {
        self.flags.contains('#')
    }
}

/// Single tmux call; tab-separated format string (7 fields):
/// window_id, window_name, active, pane_count, zoomed_flag, last_flag, window_flags
pub fn list_windows(session: &str) -> Result<Vec<WindowState>> {
    let output = crate::process::bounded_output(
        Command::new("tmux").args([
            "list-windows", "-t", session, "-F",
            "#{window_id}\t#{window_name}\t#{window_active}\t#{window_panes}\t#{window_zoomed_flag}\t#{window_last_flag}\t#{window_flags}",
        ]),
    )?;

    if !output.status.success() {
        anyhow::bail!("Failed to list windows for session '{}'", session);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut windows = Vec::new();
    for line in stdout.lines() {
        let fields: Vec<&str> = line.splitn(7, '\t').collect();
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
            flags: fields.get(6).unwrap_or(&"").to_string(),
        });
    }
    Ok(windows)
}

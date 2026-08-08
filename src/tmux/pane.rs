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
    /// Unix timestamp of the last time the pane produced output (`#{pane_activity}`, N4).
    /// Zero if tmux did not return a value.
    pub last_activity: u64,
    /// The command the pane was originally created with (`#{pane_start_command}`, N5).
    /// Empty string when tmux did not record a start command.
    pub start_cmd: String,
    /// Window-relative pane index (0-based) as shown by `ctrl+a q` / `tmux display-panes`.
    /// This is the number the user sees in their tmux layout.
    pub pane_index: usize,
    /// PID of the foreground process running in the pane (`#{pane_pid}`).
    /// This is the shell PID when idle, or the child command PID when a command is running.
    /// Zero when tmux did not return a value.
    pub pane_pid: u32,
}

/// List all panes in the session with rich metadata using a single tmux call.
///
/// Fields are tab-separated in the format string.  Tab characters cannot appear
/// in pane paths or command names, making `\t` a safe delimiter.
pub fn list_panes_detailed() -> Result<Vec<RichPaneInfo>> {
    let output = crate::tmux::bounded_output(Command::new("tmux").args([
            "list-panes", "-a", "-F",
            "#{session_name}\t#{window_name}\t#{pane_id}\t#{pane_current_command}\t#{pane_current_path}\t#{pane_title}\t#{pane_dead}\t#{pane_dead_status}\t#{scroll_position}\t#{history_size}\t#{pane_in_mode}\t#{pane_synchronized}\t#{pane_activity}\t#{pane_start_command}\t#{pane_index}\t#{pane_pid}",
        ]))?;

    if !output.status.success() {
        anyhow::bail!("Failed to list all panes");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut panes = Vec::new();
    for line in stdout.lines() {
        let fields: Vec<&str> = line.splitn(16, '\t').collect();
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
            last_activity: fields
                .get(12)
                .and_then(|s| s.trim().parse::<u64>().ok())
                .unwrap_or(0),
            start_cmd: fields
                .get(13)
                .map(|s| s.trim().to_string())
                .unwrap_or_default(),
            pane_index: fields
                .get(14)
                .and_then(|s| s.trim().parse::<usize>().ok())
                .unwrap_or(0),
            pane_pid: fields
                .get(15)
                .and_then(|s| s.trim().parse::<u32>().ok())
                .unwrap_or(0),
        });
    }
    Ok(panes)
}

/// Query the exit status of the foreground process in a pane.
///
/// Returns `Some(code)` if the pane's foreground process has exited, `None`
/// if the pane is still alive or the status cannot be determined.
pub fn pane_dead_status(pane_id: &str) -> Option<i32> {
    let output = crate::tmux::bounded_output(Command::new("tmux").args([
        "display-message",
        "-t",
        pane_id,
        "-p",
        "#{pane_dead}\t#{pane_dead_status}",
    ]))
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
    let output = crate::tmux::bounded_output(Command::new("tmux").args([
        "capture-pane",
        "-p",
        "-t",
        pane_id,
        "-S",
        &format!("-{}", depth),
    ]))?;

    if !output.status.success() {
        anyhow::bail!("Failed to capture pane '{}'", pane_id);
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Capture the content of a specific pane, preserving ANSI escape sequences (R2).
///
/// Like [`capture_pane`] but passes `-e` to `tmux capture-pane`, which makes
/// tmux retain colour and attribute escape codes in the output.  Used for
/// semantic annotation when no pipe log is available.
pub fn capture_pane_with_escapes(pane_id: &str, depth: usize) -> Result<String> {
    let output = crate::tmux::bounded_output(Command::new("tmux").args([
        "capture-pane",
        "-p",
        "-e",
        "-t",
        pane_id,
        "-S",
        &format!("-{}", depth),
    ]))?;
    if !output.status.success() {
        anyhow::bail!("Failed to capture pane '{}' with escapes", pane_id);
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Capture pane content at a scroll position, preserving ANSI escapes (R2/R3).
pub fn capture_pane_at_scroll_with_escapes(
    pane_id: &str,
    scroll_pos: usize,
    depth: usize,
) -> Result<String> {
    let end: i64 = -(scroll_pos as i64);
    let start: i64 = end - depth as i64;
    let output = crate::tmux::bounded_output(Command::new("tmux").args([
        "capture-pane",
        "-p",
        "-e",
        "-t",
        pane_id,
        "-S",
        &start.to_string(),
        "-E",
        &end.to_string(),
    ]))?;
    if !output.status.success() {
        anyhow::bail!(
            "Failed to capture pane '{}' at scroll {} with escapes",
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
    let output = crate::tmux::bounded_output(Command::new("tmux").args([
        "capture-pane",
        "-S",
        "-",
        "-t",
        pane_id,
    ]))?;
    if !output.status.success() {
        anyhow::bail!("Failed to capture pane '{}' into buffer", pane_id);
    }

    // Save to the specified file path by piping the buffer out
    let output_save =
        crate::tmux::bounded_output(Command::new("tmux").args(["save-buffer", &out_path_str]))?;
    if !output_save.status.success() {
        anyhow::bail!(
            "Failed to save captured buffer from pane '{}' to file",
            pane_id
        );
    }

    // Clean up the tmux internal buffer
    let _ = crate::tmux::bounded_output(Command::new("tmux").args(["delete-buffer"]));
    Ok(())
}

// ---------------------------------------------------------------------------
// R1 — pipe-pane selective capture
// ---------------------------------------------------------------------------

/// Derive the pipe log path for a given pane ID.
///
/// The pane ID (e.g. `%3`) is sanitised to a plain number so the path is a
/// valid filename on every filesystem: `%3` → `~/.daemoneye/var/log/pipe/de-pipe-3.log`.
pub fn pipe_log_path(pane_id: &str) -> std::path::PathBuf {
    let safe = pane_id.trim_start_matches('%');
    crate::config::pipe_log_dir().join(format!("de-pipe-{}.log", safe))
}

/// Start piping all output from `pane_id` to its log file.
///
/// Uses `-O` to capture pane output only (not stdin keystrokes).  If a pipe
/// is already running for the pane, tmux replaces it — the log is append-only
/// so no content is lost.  Returns the log path on success.
pub fn start_pipe_pane(pane_id: &str) -> Result<std::path::PathBuf> {
    let path = pipe_log_path(pane_id);
    let cmd = format!("cat >> {}", path.to_string_lossy());
    let out = crate::tmux::bounded_output(std::process::Command::new("tmux").args([
        "pipe-pane",
        "-O",
        "-t",
        pane_id,
        &cmd,
    ]))?;
    if !out.status.success() {
        anyhow::bail!(
            "pipe-pane failed for {}: {}",
            pane_id,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(path)
}

/// Stop the pipe for `pane_id` and delete its log file.
///
/// An empty shell-command argument stops the pipe without error.
pub fn stop_pipe_pane(pane_id: &str) {
    let _ = crate::tmux::bounded_output(std::process::Command::new("tmux").args([
        "pipe-pane",
        "-t",
        pane_id,
    ]));
    let _ = std::fs::remove_file(pipe_log_path(pane_id));
}

/// Return the name of the foreground process running in a pane.
pub fn pane_current_command(pane_id: &str) -> Result<String> {
    let output = crate::tmux::bounded_output(Command::new("tmux").args([
        "display-message",
        "-t",
        pane_id,
        "-p",
        "#{pane_current_command}",
    ]))?;
    if !output.status.success() {
        anyhow::bail!("Failed to query pane_current_command for '{}'", pane_id);
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Return the PID of the foreground process running in a pane.
///
/// This is the shell PID when idle, or the child command PID when a command
/// is running.  Used for completion detection: when `pane_pid` returns to the
/// value captured before a command was sent, the command has finished.
pub fn pane_pid(pane_id: &str) -> Result<u32> {
    let output = crate::tmux::bounded_output(Command::new("tmux").args([
        "display-message",
        "-t",
        pane_id,
        "-p",
        "#{pane_pid}",
    ]))?;
    if !output.status.success() {
        anyhow::bail!("Failed to query pane_pid for '{}'", pane_id);
    }
    let pid = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u32>()
        .unwrap_or(0);
    Ok(pid)
}

/// Query the current width of a pane in columns.
pub fn query_pane_width(pane_id: &str) -> Result<usize> {
    let output = crate::tmux::bounded_output(Command::new("tmux").args([
        "display-message",
        "-t",
        pane_id,
        "-p",
        "#{pane_width}",
    ]))?;
    if !output.status.success() {
        anyhow::bail!("Failed to query pane width for '{}'", pane_id);
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<usize>()
        .map_err(|e| anyhow::anyhow!("Could not parse pane width: {}", e))
}

/// Resize a pane to the given number of columns.
pub fn resize_pane_width(pane_id: &str, width: usize) -> Result<()> {
    let output = crate::tmux::bounded_output(Command::new("tmux").args([
        "resize-pane",
        "-t",
        pane_id,
        "-x",
        &width.to_string(),
    ]))?;
    if !output.status.success() {
        anyhow::bail!("Failed to resize pane '{}'", pane_id);
    }
    Ok(())
}

/// Switch tmux focus to the specified pane.
pub fn select_pane(pane_id: &str) -> Result<()> {
    let output =
        crate::tmux::bounded_output(Command::new("tmux").args(["select-pane", "-t", pane_id]))?;
    if !output.status.success() {
        anyhow::bail!("Failed to select pane '{}'", pane_id);
    }
    Ok(())
}

/// Switch tmux focus to the window containing the specified pane.
///
/// tmux resolves a pane id to its window, so no lookup is needed.
pub fn select_window(pane_id: &str) -> Result<()> {
    let output =
        crate::tmux::bounded_output(Command::new("tmux").args(["select-window", "-t", pane_id]))?;
    if !output.status.success() {
        anyhow::bail!("Failed to select window for pane '{}'", pane_id);
    }
    Ok(())
}

/// Return `true` if the pane's window is currently zoomed.
///
/// Reads `#{window_zoomed_flag}` via `display-message -p`, which prints `0`
/// or `1`.  Verified against tmux 3.7b.
pub fn pane_window_zoomed(pane_id: &str) -> Result<bool> {
    let output = crate::tmux::bounded_output(Command::new("tmux").args([
        "display-message",
        "-p",
        "-t",
        pane_id,
        "#{window_zoomed_flag}",
    ]))?;
    if !output.status.success() {
        anyhow::bail!("Failed to read zoom state for pane '{}'", pane_id);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(text.trim() == "1")
}

/// Toggle the zoom state of the pane's window (`tmux resize-pane -Z`).
///
/// Named `toggle_` on purpose: `-Z` is a toggle, and naming it `zoom_pane` is
/// how the caller ends up unzooming a zoomed pane.
pub fn toggle_zoom(pane_id: &str) -> Result<()> {
    let output = crate::tmux::bounded_output(Command::new("tmux").args([
        "resize-pane",
        "-Z",
        "-t",
        pane_id,
    ]))?;
    if !output.status.success() {
        anyhow::bail!("Failed to toggle zoom for pane '{}'", pane_id);
    }
    Ok(())
}

/// Split the given pane, returning the new pane's id.
///
/// Uses `-P -F '#{pane_id}'` so tmux prints the new pane's id on stdout.
/// Without those flags tmux prints nothing and the caller cannot identify
/// the split result.
pub fn split_pane(pane_id: &str, horizontal: bool) -> Result<String> {
    let output = crate::tmux::bounded_output(Command::new("tmux").args([
        "split-window",
        if horizontal { "-h" } else { "-v" },
        "-t",
        pane_id,
        "-P",
        "-F",
        "#{pane_id}",
    ]))?;
    if !output.status.success() {
        anyhow::bail!("Failed to split pane '{}'", pane_id);
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Rename the window containing the given pane.
///
/// Named `_for_pane` to keep it distinct from `window::rename_window(session,
/// old, new)`, which has its own callers and a different signature.
pub fn rename_window_for_pane(pane_id: &str, new_name: &str) -> Result<()> {
    let output = crate::tmux::bounded_output(Command::new("tmux").args([
        "rename-window",
        "-t",
        pane_id,
        new_name,
    ]))?;
    if !output.status.success() {
        anyhow::bail!("Failed to rename window for pane '{}'", pane_id);
    }
    Ok(())
}

/// Kill the window containing the given pane.
///
/// Named `_for_pane` to match `rename_window_for_pane`.  tmux resolves a pane
/// id to its window, so no separate window lookup is needed.
pub fn kill_window_for_pane(pane_id: &str) -> Result<()> {
    let output =
        crate::tmux::bounded_output(Command::new("tmux").args(["kill-window", "-t", pane_id]))?;
    if !output.status.success() {
        anyhow::bail!("Failed to kill window for pane '{}'", pane_id);
    }
    Ok(())
}

/// Read the last exit status recorded by the shell hook in the given pane.
///
/// The shell hook (`PROMPT_COMMAND` / `precmd`) writes the exit code to the
/// tmux session environment under the key `DE_EXIT_<num>` (e.g. `DE_EXIT_3`
/// for pane `%3`).  Returns `None` when the key is absent (hook not set up)
/// or the value cannot be parsed.
pub fn read_pane_exit_status(pane_id: &str) -> Option<i32> {
    let key = format!("DE_EXIT_{}", pane_id.trim_start_matches('%'));
    let output =
        crate::tmux::bounded_output(Command::new("tmux").args(["show-environment", &key])).ok()?;
    if !output.status.success() {
        return None;
    }
    // Output format: "DE_EXIT_3=0\n"
    let text = String::from_utf8_lossy(&output.stdout);
    text.trim()
        .split_once('=')
        .and_then(|(_, val)| val.parse::<i32>().ok())
}

/// Clear the `DE_EXIT_<pane>` latch before sending a foreground command, so its
/// later reappearance — written by the shell hook when the prompt redraws — marks
/// *this* command's completion and carries its real exit code, not a stale one.
/// Best-effort: a missing hook simply means the key never reappears, and the
/// caller falls back to PID-return completion.
pub fn clear_pane_exit_status(pane_id: &str) {
    let key = format!("DE_EXIT_{}", pane_id.trim_start_matches('%'));
    let _ = crate::tmux::bounded_output(Command::new("tmux").args(["set-environment", "-u", &key]));
}

/// Send keys (a command) to a specific pane.
pub fn send_keys(pane_id: &str, cmd: &str) -> Result<()> {
    let output = crate::tmux::bounded_output(Command::new("tmux").args([
        "send-keys",
        "-t",
        pane_id,
        cmd,
        "C-m",
    ]))?;

    if !output.status.success() {
        anyhow::bail!("Failed to send keys to pane '{}'", pane_id);
    }

    Ok(())
}

/// Send a `C-c` (SIGINT) to a pane without a trailing Enter, to cancel a process
/// waiting at a prompt (e.g. a sudo password prompt the user let time out).
pub fn send_cancel(pane_id: &str) -> Result<()> {
    let output = crate::tmux::bounded_output(Command::new("tmux").args([
        "send-keys",
        "-t",
        pane_id,
        "C-c",
    ]))?;
    if !output.status.success() {
        anyhow::bail!("Failed to send C-c to pane '{}'", pane_id);
    }
    Ok(())
}

/// Set the `remain-on-exit` option for a specific pane.
/// Get the global window ID (`#{window_id}`, e.g. `@3`) of the window containing a pane.
pub fn pane_window_id(pane_id: &str) -> Result<String> {
    let output = crate::tmux::bounded_output(Command::new("tmux").args([
        "display-message",
        "-t",
        pane_id,
        "-p",
        "#{window_id}",
    ]))?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// List the pane IDs of all panes in a tmux window.
///
/// `window_id` is the global window ID (e.g. `@3`), not the session-relative index.
pub fn list_panes_in_window(window_id: &str) -> Result<Vec<String>> {
    let output = crate::tmux::bounded_output(Command::new("tmux").args([
        "list-panes",
        "-t",
        window_id,
        "-F",
        "#{pane_id}",
    ]))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

/// Returns true if the given pane ID still exists in any tmux session.
pub fn pane_exists(pane_id: &str) -> bool {
    crate::tmux::bounded_output(Command::new("tmux").args([
        "display-message",
        "-t",
        pane_id,
        "-p",
        "#{pane_id}",
    ]))
    .map(|o| o.status.success())
    .unwrap_or(false)
}

/// Apply a visual highlight to a pane so the user can identify it as the
/// agent's active target.  Uses a dark-blue background tint that is clearly
/// distinct from a typical terminal background without disrupting readability.
/// Call [`unhighlight_pane`] to restore the default style.
///
/// `restore_focus_to` — if provided, focus is immediately returned to that
/// pane after setting the style, so the user's active pane is not disturbed.
pub fn highlight_pane(pane_id: &str, restore_focus_to: Option<&str>) {
    let _ = crate::tmux::bounded_output(Command::new("tmux").args([
        "select-pane",
        "-t",
        pane_id,
        "-P",
        "bg=colour17",
    ]));
    if let Some(restore) = restore_focus_to {
        let _ =
            crate::tmux::bounded_output(Command::new("tmux").args(["select-pane", "-t", restore]));
    }
}

/// Remove the visual highlight previously set by [`highlight_pane`], restoring
/// the pane's style to the window default.
///
/// `restore_focus_to` — if provided, focus is immediately returned to that
/// pane after clearing the style.
pub fn unhighlight_pane(pane_id: &str, restore_focus_to: Option<&str>) {
    let _ = crate::tmux::bounded_output(Command::new("tmux").args([
        "select-pane",
        "-t",
        pane_id,
        "-P",
        "default",
    ]));
    if let Some(restore) = restore_focus_to {
        let _ =
            crate::tmux::bounded_output(Command::new("tmux").args(["select-pane", "-t", restore]));
    }
}

pub fn set_remain_on_exit(pane_id: &str, enable: bool) -> Result<()> {
    let value = if enable { "on" } else { "off" };
    let output = crate::tmux::bounded_output(Command::new("tmux").args([
        "set-option",
        "-t",
        pane_id,
        "remain-on-exit",
        value,
    ]))?;
    if !output.status.success() {
        anyhow::bail!("Failed to set remain-on-exit for pane '{}'", pane_id);
    }
    Ok(())
}

/// Read a named tmux buffer's contents to bytes (`tmux save-buffer -b <name> -`).
/// Returns the raw bytes; the buffer is NOT deleted (caller decides).
pub fn save_buffer(name: &str) -> Result<Vec<u8>> {
    let output =
        crate::tmux::bounded_output(Command::new("tmux").args(["save-buffer", "-b", name, "-"]))?;
    if !output.status.success() {
        anyhow::bail!("Failed to save tmux buffer '{}'", name);
    }
    Ok(output.stdout)
}

/// Best-effort delete of a named tmux buffer (`tmux delete-buffer -b <name>`).
/// Errors are swallowed — a missing buffer is not a failure.
pub fn delete_buffer(name: &str) {
    let _ = crate::tmux::bounded_output(Command::new("tmux").args(["delete-buffer", "-b", name]));
}

/// Block until `channel` is signalled with `tmux wait-for -S <channel>`, or until
/// `timeout` elapses. Returns `true` if the signal arrived, `false` on timeout.
/// On timeout the spawned waiter is killed and the channel is released so a later
/// reuse cannot inherit a stuck waiter. The signalling side must run on a shell
/// that shares THIS tmux server (i.e. a local/daemon-host pane) — a remote host's
/// shell cannot reach this server.
pub async fn wait_for(channel: &str, timeout: std::time::Duration) -> bool {
    let mut child = match tokio::process::Command::new("tmux")
        .args(["wait-for", channel])
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(_) => true,
        Err(_) => {
            let _ = child.start_kill();
            // Release the channel so a future waiter on the same name isn't stuck.
            let _ = tokio::process::Command::new("tmux")
                .args(["wait-for", "-S", channel])
                .output()
                .await;
            false
        }
    }
}

/// Capture `depth` lines of a pane's scrollback with ANSI escapes converted to
/// semantic markers (`[ERROR:]`/`[WARN:]`/`[OK:]`).
///
/// `read_pane` (M12 D3) uses this: `annotate_ansi` is private to `crate::tmux`,
/// so the conversion has to happen on this side of the module boundary.
pub fn capture_pane_annotated(pane_id: &str, depth: usize) -> Result<String> {
    let raw = capture_pane_with_escapes(pane_id, depth)?;
    Ok(super::ansi::annotate_ansi(&raw))
}

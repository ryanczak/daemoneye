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
    let output = crate::process::bounded_output(Command::new("tmux").args([
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

/// Capture the content of a specific pane.
pub fn capture_pane(pane_id: &str, depth: usize) -> Result<String> {
    let output = crate::process::bounded_output(Command::new("tmux").args([
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
    let output = crate::process::bounded_output(Command::new("tmux").args([
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
    let output = crate::process::bounded_output(Command::new("tmux").args([
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

/// Get the global window ID (`#{window_id}`, e.g. `@3`) of the window containing a pane.
pub fn pane_window_id(pane_id: &str) -> Result<String> {
    let output = crate::process::bounded_output(Command::new("tmux").args([
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
    let output = crate::process::bounded_output(Command::new("tmux").args([
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
    crate::process::bounded_output(Command::new("tmux").args([
        "display-message",
        "-t",
        pane_id,
        "-p",
        "#{pane_id}",
    ]))
    .map(|o| o.status.success())
    .unwrap_or(false)
}

/// Query the current width of a pane in columns.
pub fn query_pane_width(pane_id: &str) -> Result<usize> {
    let output = crate::process::bounded_output(Command::new("tmux").args([
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

/// Escape `msg` so tmux's FORMAT and strftime expansion of a
/// `display-message` argument reproduce it byte for byte (except control
/// characters, which cannot survive a terminal overlay). Without this, an
/// untrusted alert line's `#(cmd)` runs `cmd` through `/bin/sh` and `#{…}`
/// expands as a format variable.
///
/// Rules: `#` → `##`, `%` → `%%`, `\n` / `\r` / `\t` → one space, every other
/// `char::is_control()` character dropped, everything else (non-ASCII
/// included) unchanged.
pub fn format_literal(msg: &str) -> String {
    let mut out = String::with_capacity(msg.len() + 8);
    for ch in msg.chars() {
        match ch {
            '#' => out.push_str("##"),
            '%' => out.push_str("%%"),
            '\n' | '\r' | '\t' => out.push(' '),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// The argv for an inert overlay on `pane`: `--` terminates tmux's flag
/// parsing so a message starting with `-` is not read as a flag, and the
/// escaped message is format-inert. Without the `tmux` binary itself.
pub fn overlay_argv(pane: &str, msg: &str) -> Vec<String> {
    vec![
        "display-message".to_string(),
        "-d".to_string(),
        "8000".to_string(),
        "-t".to_string(),
        pane.to_string(),
        "--".to_string(),
        format_literal(msg),
    ]
}

/// Show a transient overlay on `pane`. Best-effort: a failing tmux call (no
/// server, dead pane) is not an alert-delivery failure.
pub fn show_overlay(pane: &str, msg: &str) {
    let _ = crate::process::bounded_output(Command::new("tmux").args(overlay_argv(pane, msg)));
}

#[cfg(test)]
mod tests {
    use super::{format_literal, overlay_argv};

    #[test]
    fn format_literal_escapes_hash_and_percent() {
        assert_eq!(
            format_literal("#(touch /tmp/x) %H #{pane_id} ##"),
            "##(touch /tmp/x) %%H ##{pane_id} ####"
        );
    }

    #[test]
    fn format_literal_strips_control_characters() {
        assert_eq!(format_literal("a\nb\rc\td"), "a b c d");
        assert_eq!(
            format_literal("x\u{1b}[31my\u{0}z\u{7f}"),
            "x[31myz",
            "ESC, NUL and DEL are dropped"
        );
    }

    #[test]
    fn format_literal_leaves_plain_text_alone() {
        assert_eq!(
            format_literal("WEBHOOK ALERT: disk full on db-1 — ünïcødé"),
            "WEBHOOK ALERT: disk full on db-1 — ünïcødé"
        );
    }

    #[test]
    fn format_literal_round_trips_through_tmux() {
        let mut cmd = std::process::Command::new("tmux");
        if cmd.output().is_err() {
            println!("skip: no tmux binary on PATH");
            return;
        }
        let dir = tempfile::tempdir().expect("temp dir under /tmp");
        let sock = dir.path().join("s.sock");
        let run = |args: &[&str]| {
            let out = std::process::Command::new("tmux")
                .arg("-S")
                .arg(&sock)
                .args(args)
                .output()
                .expect("run tmux on the private socket");
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        run(&["new-session", "-d", "-s", "t", "-x", "80", "-y", "24"]);
        let input = format!(
            "#(touch {}) %H #{{pane_id}} ## 100%",
            dir.path().join("pwned").display()
        );
        let printed = run(&["display-message", "-p", "--", &format_literal(&input)]);
        assert_eq!(printed, input);
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(
            !dir.path().join("pwned").exists(),
            "#(…) must not run — the escaped form is inert"
        );
    }

    #[test]
    fn overlay_argv_ends_with_double_dash_then_the_escaped_message() {
        assert_eq!(
            overlay_argv("%3", "-x #(id)"),
            vec![
                "display-message",
                "-d",
                "8000",
                "-t",
                "%3",
                "--",
                "-x ##(id)"
            ]
        );
    }
}

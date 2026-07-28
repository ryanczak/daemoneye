use crate::ai::Message;
use crate::ai::filter::mask_sensitive;
use crate::daemon::session::{SessionStore, append_session_message, with_sessions};
use crate::daemon::utils::normalize_output;
use crate::tmux;
use std::sync::Mutex;

// ---------------------------------------------------------------------------
// Shell helpers
// ---------------------------------------------------------------------------

/// Returns the exit-code variable for the detected shell.
/// Fish and csh/tcsh use `$status`; all POSIX-compatible shells use `$?`.
pub(super) fn shell_exit_var(shell_name: &str) -> &'static str {
    match shell_name.trim() {
        "fish" | "csh" | "tcsh" => "$status",
        _ => "$?",
    }
}

// ---------------------------------------------------------------------------
// Shared capture / archive / notify helpers
// ---------------------------------------------------------------------------

/// Maximum bytes of command output passed inline to the AI.
/// Outputs larger than this are trimmed to head + tail with an omission note.
const OUTPUT_INLINE_LIMIT: usize = 50_000;

/// Trim `raw` to at most `limit` bytes, preserving the first and last halves
/// and inserting an omission note with the archive path in between.
///
/// Splits are rounded to newline boundaries so lines are never cut mid-stream.
fn trim_large_output(raw: &str, limit: usize, win_name: &str) -> String {
    if raw.len() <= limit {
        return raw.to_string();
    }
    let half = limit / 2;

    // Head: up to `half` bytes, rounded down to the last newline.
    let head_end = raw[..half].rfind('\n').map(|i| i + 1).unwrap_or(half);
    let head = &raw[..head_end];

    // Tail: last `half` bytes, rounded up to the next newline.
    let tail_raw_start = raw.len() - half;
    let tail_start = raw[tail_raw_start..]
        .find('\n')
        .map(|i| tail_raw_start + i + 1)
        .unwrap_or(tail_raw_start);
    let tail = &raw[tail_start..];

    let omitted = raw
        .lines()
        .count()
        .saturating_sub(head.lines().count())
        .saturating_sub(tail.lines().count());

    // Use the absolute path so the agent can pass it directly to read_file.
    let archive = crate::config::pane_logs_dir()
        .join(format!("{}.log", win_name))
        .to_string_lossy()
        .to_string();
    // head already ends with '\n'; trim it so the format string doesn't insert a blank line.
    let head = head.trim_end_matches('\n');
    format!("{head}\n... ({omitted} lines omitted — full log: {archive}) ...\n{tail}")
}

/// Capture and mask pane output, archive the full output to `var/log/panes/`.
/// Returns the masked body string suitable for the AI.
///
/// `pipe_log` — path to the pipe-pane log file started before the command ran.
/// When present it is read directly (no scrollback cap) and then deleted.
/// Falls back to `capture_pane` if the file cannot be read.
///
/// The archive at `~/.daemoneye/var/log/panes/{win_name}.log` always uses the
/// best available content: the full pipe-log when present, otherwise the
/// scrollback-limited `capture_pane_to_file` fallback.  Ghost shell pane logs
/// are therefore never truncated due to scrollback limits.
pub(super) fn capture_and_archive(
    pane_id: &str,
    win_name: &str,
    pipe_log: Option<std::path::PathBuf>,
) -> String {
    // Fix B: prefer pipe log over scrollback-limited capture_pane.
    let have_pipe_log = pipe_log.is_some();
    let raw = match pipe_log {
        Some(ref log_path) => match std::fs::read_to_string(log_path) {
            Ok(content) => {
                let _ = std::fs::remove_file(log_path);
                content
            }
            Err(e) => {
                log::warn!(
                    "Failed to read pipe log {:?}: {} — falling back to capture_pane",
                    log_path,
                    e
                );
                let _ = std::fs::remove_file(log_path);
                tmux::capture_pane(pane_id, 5000).unwrap_or_default()
            }
        },
        None => tmux::capture_pane(pane_id, 5000).unwrap_or_default(),
    };
    let trimmed = trim_large_output(&raw, OUTPUT_INLINE_LIMIT, win_name);
    let normalized = normalize_output(&trimmed);
    let body = if normalized.is_empty() {
        "(no output)".to_string()
    } else {
        mask_sensitive(&normalized)
    };
    let logs_dir = crate::config::pane_logs_dir();
    if let Err(e) = std::fs::create_dir_all(&logs_dir) {
        log::warn!(
            "Failed to create pane_logs dir {}: {}",
            logs_dir.display(),
            e
        );
    } else {
        let archive_path = logs_dir.join(format!("{}.log", win_name));
        // When we have the full pipe-log content in `raw`, write it directly to the
        // archive so ghost shell pane logs are never truncated by scrollback limits.
        // Fall back to capture_pane_to_file only when no pipe log was available.
        if have_pipe_log && !raw.is_empty() {
            if let Err(e) = std::fs::write(&archive_path, raw.as_bytes()) {
                log::warn!("Failed to archive pane log for {}: {}", win_name, e);
            }
        } else if let Err(e) = tmux::pane::capture_pane_to_file(pane_id, &archive_path) {
            log::warn!("Failed to archive pane log for {}: {}", win_name, e);
        }
    }
    body
}

pub(super) struct BgJobInfo {
    pub(super) pane_id: String,
    pub(super) cmd: String,
    pub(super) win_name: String,
    pub(super) exit_code: i32,
    pub(super) body: String,
    pub(super) pane_persists: bool,
}

/// Inject a `[Background Task Completed]` message into the session history,
/// update `exit_code` in `bg_windows`, and flash a `tmux display-message`.
///
/// `pane_persists` — if true, the window is still open and the AI can reuse it.
pub(super) fn notify_session(sessions: &SessionStore, session_id: &str, job: BgJobInfo) {
    let BgJobInfo {
        pane_id,
        cmd,
        win_name,
        exit_code,
        body,
        pane_persists,
    } = job;
    // Phase 1 (locked): update the registry and take what the rest needs.
    // Returns None when the session entry is gone.
    let Some(chat_pane) = with_sessions(sessions, |store| {
        let entry = store.get_mut(session_id)?;

        // Update exit_code in the bg_windows registry.
        if let Some(w) = entry.bg_windows.iter_mut().find(|w| w.pane_id == pane_id) {
            w.exit_code = Some(exit_code);
        }

        Some(entry.chat_pane.clone())
    }) else {
        return;
    };

    // Phase 2 (unlocked): the filesystem scan, the formatting, and the file write.
    let persist_note = if pane_persists {
        format!(
            "The window is still open (pane {pane_id}). \
             Use target=\"{pane_id}\" to run follow-up commands in the same shell. \
             Call close_background_window(\"{pane_id}\") when you are done with this window."
        )
    } else {
        format!("The window was closed. Full log: ~/.daemoneye/var/log/panes/{win_name}.log")
    };

    let hints = crate::manifest::related_knowledge_hints(&body);
    let hints_section = if !hints.is_empty() {
        format!("\n{}", hints)
    } else {
        String::new()
    };
    let history_content = format!(
        "Background command `{cmd}` in window {win_name} finished with exit code {exit_code}.\n\
         {persist_note}\n<output>\n{body}\n</output>{hints_section}"
    );
    let completion_msg = Message {
        role: "user".to_string(),
        content: format!("[Background Task Completed]\n{}", history_content),
        tool_calls: None,
        tool_results: None,
        turn: None,
    };
    append_session_message(session_id, &completion_msg);

    // Phase 3 (locked): push the message into the in-memory history.
    with_sessions(sessions, |store| {
        if let Some(entry) = store.get_mut(session_id) {
            entry.messages.push(completion_msg);
        }
    });

    // Phase 4 (unlocked): the tmux notification.
    let status_word = if exit_code == 0 {
        "succeeded"
    } else {
        "failed"
    };
    let alert = format!("`{cmd}` {status_word} in pane {pane_id}");
    if let Some(ref cp) = chat_pane {
        let _ = std::process::Command::new("tmux")
            .args(["display-message", "-d", "5000", "-t", cp, &alert])
            .output();
    }
}

pub static BG_COMMAND_MAP: std::sync::OnceLock<Mutex<std::collections::HashMap<String, usize>>> =
    std::sync::OnceLock::new();

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_small_output_unchanged() {
        let s = "line1\nline2\nline3\n";
        assert_eq!(trim_large_output(s, 50_000, "win"), s);
    }

    #[test]
    fn trim_large_output_has_head_and_tail() {
        // Build a string well over the limit.
        let raw: String = (0..1000)
            .map(|i| format!("{:03}: {}\n", i, "x".repeat(94)))
            .collect();
        let limit = 10_000;
        let result = trim_large_output(&raw, limit, "myjob");

        // Head and tail preserved, omission marker present.
        assert!(result.contains("... ("), "expected omission marker");
        assert!(result.contains("myjob.log"), "expected archive path");

        // Result must be smaller than raw.
        assert!(result.len() < raw.len());
    }

    #[test]
    fn trim_output_respects_newline_boundaries() {
        // Each line is exactly 10 bytes including newline.
        let raw: String = (0..200).map(|i| format!("{:09}\n", i)).collect();
        let limit = 500; // 50 lines total budget
        let result = trim_large_output(&raw, limit, "w");

        // No line should be cut mid-stream.
        for line in result.lines() {
            if line.starts_with("...") {
                continue;
            }
            assert!(line.len() == 9, "line cut: {:?}", line);
        }
    }

    #[test]
    fn trim_output_omission_count_is_positive() {
        let raw: String = (0..1000).map(|i| format!("line {}\n", i)).collect();
        let result = trim_large_output(&raw, 2_000, "w");
        // Extract the omission count from the marker line.
        let marker = result.lines().find(|l| l.starts_with("...")).unwrap();
        let count: usize = marker
            .split('(')
            .nth(1)
            .unwrap()
            .split(' ')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(count > 0);
    }
}

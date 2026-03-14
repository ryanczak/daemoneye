use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

use crate::config::Config;
use crate::ipc::{Request, Response};

// ── Async stdin wrapper ───────────────────────────────────────────────────────

/// Non-owning handle to fd 0 used with `AsyncFd`.  Does not close the fd on
/// drop — closing stdin would break the process.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::commands::*;
    use crate::cli::render::*;
    use crate::cli::input::*;
    use crate::daemon::utils::command_has_sudo;

    // ── command_has_sudo ──────────────────────────────────────────────────────

    #[test]
    fn command_has_sudo_simple_cli() {
        assert!(command_has_sudo("sudo apt install vim"));
    }

    #[test]
    fn command_has_sudo_in_pipeline_cli() {
        assert!(command_has_sudo("echo hi | sudo tee /etc/hosts"));
    }

    #[test]
    fn command_has_sudo_after_semicolon_cli() {
        assert!(command_has_sudo("cd /tmp; sudo rm -rf foo"));
    }

    #[test]
    fn command_has_sudo_false_positive_guard_cli() {
        // "sudoers" is not "sudo" — word-boundary must hold.
        assert!(!command_has_sudo("cat /etc/sudoers"));
    }

    #[test]
    fn command_has_sudo_no_sudo_cli() {
        assert!(!command_has_sudo("ls -la /home"));
    }

    // ── visual_len ────────────────────────────────────────────────────────────

    #[test]
    fn visual_len_plain_ascii() {
        assert_eq!(visual_len("hello"), 5);
    }

    #[test]
    fn visual_len_empty_string() {
        assert_eq!(visual_len(""), 0);
    }

    #[test]
    fn visual_len_strips_ansi_reset() {
        // "\x1b[0m" is an ANSI reset — it contributes 0 visual columns.
        assert_eq!(visual_len("\x1b[0mhello"), 5);
    }

    #[test]
    fn visual_len_strips_ansi_colour() {
        assert_eq!(visual_len("\x1b[31mred\x1b[0m"), 3);
    }

    #[test]
    fn visual_len_strips_bold() {
        assert_eq!(visual_len("\x1b[1mbold text\x1b[0m"), 9);
    }

    #[test]
    fn visual_len_nested_escape_sequences() {
        // Two different ANSI sequences around some text.
        let s = "\x1b[1m\x1b[32mgreen bold\x1b[0m\x1b[0m";
        assert_eq!(visual_len(s), 10);
    }

    #[test]
    fn visual_len_no_escape_inside_word() {
        // "DaemonEye" has no escapes — all 9 chars count.
        assert_eq!(visual_len("DaemonEye"), 9);
    }

    // ── fmt_uptime ────────────────────────────────────────────────────────────

    #[test]
    fn fmt_uptime_seconds_only() {
        assert_eq!(fmt_uptime(std::time::Duration::from_secs(0)),  "0s");
        assert_eq!(fmt_uptime(std::time::Duration::from_secs(42)), "42s");
        assert_eq!(fmt_uptime(std::time::Duration::from_secs(59)), "59s");
    }

    #[test]
    fn fmt_uptime_minutes_and_seconds() {
        assert_eq!(fmt_uptime(std::time::Duration::from_secs(60)),  "1m 0s");
        assert_eq!(fmt_uptime(std::time::Duration::from_secs(90)),  "1m 30s");
        assert_eq!(fmt_uptime(std::time::Duration::from_secs(3599)), "59m 59s");
    }

    #[test]
    fn fmt_uptime_hours_and_minutes() {
        assert_eq!(fmt_uptime(std::time::Duration::from_secs(3600)),  "1h 0m");
        assert_eq!(fmt_uptime(std::time::Duration::from_secs(3660)),  "1h 1m");
        assert_eq!(fmt_uptime(std::time::Duration::from_secs(7322)),  "2h 2m");
    }

    #[test]
    fn fmt_uptime_exact_hour_boundary() {
        // 3600s == 1h 0m, not shown as minutes
        let out = fmt_uptime(std::time::Duration::from_secs(3600));
        assert!(out.contains('h'), "should show hours: {out}");
        assert!(!out.contains('s'), "should not show seconds: {out}");
    }
}

    #[test]
    fn wrap_line_hard_with_newlines() {
        use crate::cli::render::wrap_line_hard;
        let input = "line1\nline2";
        let wrapped = wrap_line_hard(input, 10);
        assert_eq!(wrapped, vec!["line1".to_string(), "line2".to_string()]);
    }

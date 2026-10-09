//! Pane status classification (M12 D2).

/// Output within this many seconds counts as "recent" for a shell pane.
const ACTIVE_WINDOW_SECS: u64 = 30;
/// A non-shell command with no output for at least this long is awaiting input.
const AWAITING_THRESHOLD_SECS: u64 = 60;

/// Live status of a tmux pane, derived from cached metadata only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneStatus {
    /// Foreground process has exited (remain-on-exit); carries the exit code.
    Dead(Option<i32>),
    /// The pane's window holds an uncleared tmux bell flag.
    Bell,
    /// Non-shell foreground command with no output for ≥ 60 s.
    AwaitingInput,
    /// Non-shell foreground command with recent (or unknown) output.
    Running,
    /// Shell prompt with output within the last 30 s.
    Active,
    /// Shell prompt with no recent output; carries the age in seconds
    /// (0 = age unknown).
    Idle(u64),
}

/// Return true when `cmd` is a shell name, meaning the pane is at a prompt.
pub fn is_shell_prompt(cmd: &str) -> bool {
    matches!(
        cmd.trim(),
        "bash"
            | "zsh"
            | "fish"
            | "sh"
            | "ksh"
            | "csh"
            | "tcsh"
            | "dash"
            | "nu"
            | "pwsh"
            | "elvish"
            | "xonsh"
            | "yash"
    )
}

/// Classify a pane from cached metadata. Pure — no tmux calls.
///
/// Priority: Dead > Bell > (shell? Active/Idle : Running/AwaitingInput).
/// `last_activity == 0` means "unknown": a shell classifies as `Idle(0)`,
/// a non-shell command as `Running` — never `AwaitingInput` without evidence.
pub fn classify(
    dead: bool,
    dead_status: Option<i32>,
    has_bell: bool,
    current_cmd: &str,
    last_activity: u64,
    now: u64,
) -> PaneStatus {
    if dead {
        return PaneStatus::Dead(dead_status);
    }
    if has_bell {
        return PaneStatus::Bell;
    }
    let age = (last_activity > 0).then(|| now.saturating_sub(last_activity));
    if is_shell_prompt(current_cmd) {
        match age {
            Some(a) if a < ACTIVE_WINDOW_SECS => PaneStatus::Active,
            Some(a) => PaneStatus::Idle(a),
            None => PaneStatus::Idle(0),
        }
    } else {
        match age {
            Some(a) if a >= AWAITING_THRESHOLD_SECS => PaneStatus::AwaitingInput,
            _ => PaneStatus::Running,
        }
    }
}

/// Format an age in seconds as `45s`, `3m`, or `2h5m`.
pub fn format_age(secs: u64) -> String {
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}

impl std::fmt::Display for PaneStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PaneStatus::Dead(Some(code)) => write!(f, "dead({})", code),
            PaneStatus::Dead(None) => write!(f, "dead(?)"),
            PaneStatus::Bell => write!(f, "bell"),
            PaneStatus::AwaitingInput => write!(f, "awaiting-input"),
            PaneStatus::Running => write!(f, "running"),
            PaneStatus::Active => write!(f, "active"),
            PaneStatus::Idle(0) => write!(f, "idle"),
            PaneStatus::Idle(age) => write!(f, "idle({})", format_age(*age)),
        }
    }
}

/// One-line pane summary: `<status> — <last meaningful line>` (line truncated
/// to 50 chars), or the status alone when the buffer has no non-empty line.
pub fn summarize(status: PaneStatus, buffer: &str) -> String {
    match buffer.lines().rfind(|l| !l.trim().is_empty()) {
        Some(line) => format!(
            "{} — {}",
            status,
            line.trim().chars().take(50).collect::<String>()
        ),
        None => status.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_dead_wins_over_bell_and_command() {
        let status = classify(true, Some(2), true, "vim", 1000, 1120);
        assert_eq!(status, PaneStatus::Dead(Some(2)));
    }

    #[test]
    fn classify_bell_beats_running_command() {
        let status = classify(false, None, true, "vim", 1000, 1005);
        assert_eq!(status, PaneStatus::Bell);
    }

    #[test]
    fn classify_running_for_nonshell_with_recent_output() {
        let status = classify(false, None, false, "vim", 1000, 1005);
        assert_eq!(status, PaneStatus::Running);
    }

    #[test]
    fn classify_awaiting_input_for_nonshell_stale_output() {
        let status = classify(false, None, false, "vim", 1000, 1120);
        assert_eq!(status, PaneStatus::AwaitingInput);
    }

    #[test]
    fn classify_idle_shell_never_awaiting_input() {
        let status = classify(false, None, false, "bash", 1000, 4600);
        assert_eq!(status, PaneStatus::Idle(3600));
        assert_ne!(status, PaneStatus::AwaitingInput);
    }

    #[test]
    fn classify_active_shell_with_recent_output() {
        let status = classify(false, None, false, "zsh", 1000, 1005);
        assert_eq!(status, PaneStatus::Active);
    }

    #[test]
    fn classify_unknown_activity_shell_is_idle_zero() {
        let status = classify(false, None, false, "bash", 0, 1000);
        assert_eq!(status, PaneStatus::Idle(0));
    }

    #[test]
    fn classify_unknown_activity_nonshell_is_running() {
        let status = classify(false, None, false, "vim", 0, 1000);
        assert_eq!(status, PaneStatus::Running);
        assert_ne!(status, PaneStatus::AwaitingInput);
    }

    #[test]
    fn classify_boundary_ages() {
        // age exactly 30 s on a shell → Idle(30), not Active
        let status = classify(false, None, false, "bash", 1000, 1030);
        assert_eq!(status, PaneStatus::Idle(30));

        // age exactly 60 s on a non-shell → AwaitingInput, not Running
        let status2 = classify(false, None, false, "vim", 1000, 1060);
        assert_eq!(status2, PaneStatus::AwaitingInput);
    }

    #[test]
    fn status_display_exact_forms() {
        assert_eq!(format!("{}", PaneStatus::Dead(Some(2))), "dead(2)");
        assert_eq!(format!("{}", PaneStatus::Dead(None)), "dead(?)");
        assert_eq!(format!("{}", PaneStatus::Bell), "bell");
        assert_eq!(format!("{}", PaneStatus::AwaitingInput), "awaiting-input");
        assert_eq!(format!("{}", PaneStatus::Running), "running");
        assert_eq!(format!("{}", PaneStatus::Active), "active");
        assert_eq!(format!("{}", PaneStatus::Idle(0)), "idle");
        assert_eq!(format!("{}", PaneStatus::Idle(45)), "idle(45s)");
        assert_eq!(format!("{}", PaneStatus::Idle(180)), "idle(3m)");
        assert_eq!(format!("{}", PaneStatus::Idle(3600)), "idle(1h0m)");
    }

    #[test]
    fn summarize_empty_buffer_is_status_alone() {
        assert_eq!(summarize(PaneStatus::Running, ""), "running");
        assert_eq!(summarize(PaneStatus::Running, "   \n  \n  "), "running");
    }

    #[test]
    fn summarize_appends_last_meaningful_line() {
        assert_eq!(summarize(PaneStatus::Active, "out\n$ "), "active — $");
    }

    #[test]
    fn summarize_truncates_line_to_50_chars() {
        let long_line = "x".repeat(100);
        let summary = summarize(PaneStatus::Running, &long_line);
        // "running — " is 10 chars, then 50 x's = 60 chars total
        assert_eq!(summary.chars().count(), 60);
        assert!(summary.ends_with(&"x".repeat(50)));
    }
}

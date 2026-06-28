/// Extract the output produced by a foreground command from a post-run pane snapshot.
///
/// `tmux capture-pane -S -N` returns up to N lines of scrollback oldest-first.
/// The relevant content (prompt + command + output) is at the *end* of that
/// string, not the beginning.  We find the command line by searching for the
/// last line in the capture whose text ends with the exact command string (the
/// shell echoes it as `<prompt> <cmd>`).  Everything from that line onward is
/// the command output.
///
/// Falls back to the last 50 lines of the capture when the command line cannot
/// be located (e.g. if the command string itself appears in output lines).
pub fn extract_command_output(after: &str, cmd: &str) -> String {
    let lines: Vec<&str> = after.lines().collect();
    if lines.is_empty() {
        return String::new();
    }
    if !cmd.is_empty() {
        // `rposition` gives the LAST (most recent) matching line so earlier
        // history entries with the same command don't confuse the search.
        if let Some(start) = lines.iter().rposition(|l| l.trim_end().ends_with(cmd)) {
            return lines[start..].join("\n");
        }
    }
    // Fallback: the last 50 lines cover the output of most commands.
    let tail = lines.len().saturating_sub(50);
    lines[tail..].join("\n")
}

/// Normalise command output for display and AI context:
/// - trims trailing whitespace from every line
/// - strips leading and trailing blank lines
/// - returns an empty string when all lines are blank
pub fn normalize_output(s: &str) -> String {
    let trimmed: Vec<&str> = s.lines().map(|l| l.trim_end()).collect();
    let start = trimmed.iter().position(|l| !l.is_empty()).unwrap_or(0);
    let end = trimmed
        .iter()
        .rposition(|l| !l.is_empty())
        .map(|i| i + 1)
        .unwrap_or(0);
    if start >= end {
        return String::new();
    }
    trimmed[start..end].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_basic() {
        assert_eq!(normalize_output("hello"), "hello");
    }

    #[test]
    fn normalize_trims_trailing_whitespace_per_line() {
        let input = "line one   \nline two  \nline three";
        let out = normalize_output(input);
        assert_eq!(out, "line one\nline two\nline three");
    }

    #[test]
    fn normalize_strips_leading_blank_lines() {
        let input = "\n\n\nhello\nworld";
        assert_eq!(normalize_output(input), "hello\nworld");
    }

    #[test]
    fn normalize_strips_trailing_blank_lines() {
        let input = "hello\nworld\n\n\n";
        assert_eq!(normalize_output(input), "hello\nworld");
    }

    #[test]
    fn normalize_all_blank_returns_empty() {
        assert_eq!(normalize_output("   \n  \n   "), "");
    }

    #[test]
    fn normalize_empty_input_returns_empty() {
        assert_eq!(normalize_output(""), "");
    }

    #[test]
    fn normalize_preserves_internal_blank_lines() {
        let input = "a\n\nb\n\nc";
        assert_eq!(normalize_output(input), "a\n\nb\n\nc");
    }

    fn pane_snap(lines: &[&str]) -> String {
        lines.join("\n")
    }

    #[test]
    fn extract_finds_command_line_by_suffix() {
        let snap = pane_snap(&[
            "matt@host:~$ ls",
            "file1  file2",
            "matt@host:~$ cat README.md",
            "# DaemonEye",
            "An AI-powered operator.",
            "matt@host:~$ ",
        ]);
        let result = extract_command_output(&snap, "cat README.md");
        assert!(
            result.starts_with("matt@host:~$ cat README.md"),
            "first line should be the prompt+command, got: {:?}",
            &result[..result.find('\n').unwrap_or(result.len())]
        );
        assert!(result.contains("# DaemonEye"));
    }

    #[test]
    fn extract_uses_rposition_to_pick_most_recent_invocation() {
        // The command appeared earlier in history — we want the most recent one.
        let snap = pane_snap(&[
            "matt@host:~$ ls -la",
            "old output line",
            "matt@host:~$ echo hi",
            "hi",
            "matt@host:~$ ls -la",
            "newer output",
            "matt@host:~$ ",
        ]);
        let result = extract_command_output(&snap, "ls -la");
        // Should start from the SECOND "ls -la" invocation, not the first.
        assert_eq!(result.lines().next().unwrap(), "matt@host:~$ ls -la");
        assert!(result.contains("newer output"));
        assert!(!result.contains("old output line"));
    }

    #[test]
    fn extract_fallback_when_cmd_not_found() {
        // Command string doesn't appear as a suffix anywhere — use last 50 lines.
        let mut lines: Vec<String> = (0..60).map(|i| format!("line {i}")).collect();
        lines.push("final line".to_string());
        let snap = lines.join("\n");
        let result = extract_command_output(&snap, "mystery_cmd_xyz");
        // Should contain the tail, not the beginning.
        assert!(result.contains("final line"));
        assert!(!result.contains("line 0"));
    }

    #[test]
    fn extract_empty_snap_returns_empty() {
        assert_eq!(extract_command_output("", "ls"), "");
    }
}

pub use crate::util::UnpoisonExt;

/// Return the hostname of the machine running the daemon.
pub fn daemon_hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Check whether a tmux pane's foreground process is SSH or mosh.
/// Returns a human-readable description if the pane is on a remote host.
pub fn get_pane_remote_host(pane_id: &str) -> Option<String> {
    let out = std::process::Command::new("tmux")
        .args([
            "display-message",
            "-t",
            pane_id,
            "-p",
            "#{pane_current_command}",
        ])
        .output()
        .ok()?;
    let cmd = String::from_utf8_lossy(&out.stdout).trim().to_string();
    match cmd.as_str() {
        "ssh" | "mosh-client" | "mosh" => Some(format!("remote (via {})", cmd)),
        _ => None,
    }
}

/// Escape `s` for safe embedding between `"…"` inside a tmux single-quoted
/// `run-shell` argument.
///
/// Two escaping layers are applied:
///
/// 1. **tmux-level** — a literal `'` would prematurely close the outer
///    single-quote context that tmux uses when parsing the hook command.
///    It is replaced with `'\''` (end-single-quote, backslash-escaped `'`,
///    begin-single-quote), which tmux's `cmd_string_parse` collapses to a
///    single `'` character.
///
/// 2. **shell-level** — the value appears inside `"…"` in the sh command
///    that `run-shell` executes, so `\`, `"`, `$`, and `` ` `` are
///    backslash-escaped.
pub fn shell_escape_arg(s: &str) -> String {
    s.replace('\\', "\\\\") // shell-level: double backslashes first
        .replace('\'', "'\\''") // tmux-level: ' → '\'' (must follow \ escaping)
        .replace('"', "\\\"") // shell-level
        .replace('$', "\\$")
        .replace('`', "\\`")
}

/// Return true when `cmd` will start an interactive session in the pane
/// rather than run a command and exit.  Such commands (ssh, mosh, telnet,
/// screen, rlogin) occupy the pane for the duration of the session and never
/// return the shell to an idle state.
///
/// Non-interactive sub-cases are excluded:
/// - `ssh host command` — two non-flag tokens (hostname + remote command); exits normally.
/// - `ssh -N …` or `ssh -f …` — tunnel-only / background; no shell allocated.
pub fn is_interactive_command(cmd: &str) -> bool {
    let mut tokens = cmd.split_whitespace();
    let base = match tokens.next() {
        Some(b) => b,
        None => return false,
    };
    // Strip any leading path prefix (e.g. /usr/bin/ssh → ssh).
    let base = base.rsplit('/').next().unwrap_or(base);

    match base {
        "mosh" | "telnet" | "rlogin" | "rsh" | "screen" => true,
        "ssh" => {
            // Flags that consume the next token as their argument.
            const TAKES_ARG: &[&str] = &[
                "-b", "-c", "-D", "-e", "-F", "-I", "-i", "-J", "-L", "-l", "-m", "-O", "-o", "-p",
                "-Q", "-R", "-S", "-W", "-w",
            ];
            let mut non_flag_count = 0usize;
            let mut skip_next = false;
            for tok in tokens {
                if skip_next {
                    skip_next = false;
                    continue;
                }
                // -N = no remote command (tunnel only); -f = go to background.
                if tok == "-N" || tok == "-f" {
                    return false;
                }
                if tok.starts_with('-') {
                    if TAKES_ARG.contains(&tok) {
                        skip_next = true;
                    }
                    continue;
                }
                non_flag_count += 1;
                // Two or more non-flag tokens means hostname + remote command.
                if non_flag_count >= 2 {
                    return false;
                }
            }
            // Exactly one non-flag token (the hostname) → interactive shell.
            non_flag_count == 1
        }
        _ => false,
    }
}

/// Extract the destination host/user from an interactive command string.
/// Returns `None` when the destination cannot be determined.
pub fn interactive_destination(cmd: &str) -> Option<String> {
    const SSH_TAKES_ARG: &[&str] = &[
        "-b", "-c", "-D", "-e", "-F", "-I", "-i", "-J", "-L", "-l", "-m", "-O", "-o", "-p", "-Q",
        "-R", "-S", "-W", "-w",
    ];
    let mut tokens = cmd.split_whitespace();
    let base = tokens.next()?;
    let base = base.rsplit('/').next().unwrap_or(base);
    match base {
        "ssh" => {
            let mut skip_next = false;
            for tok in tokens {
                if skip_next {
                    skip_next = false;
                    continue;
                }
                if tok.starts_with('-') {
                    if SSH_TAKES_ARG.contains(&tok) {
                        skip_next = true;
                    }
                    continue;
                }
                return Some(tok.to_string());
            }
            None
        }
        "mosh" | "telnet" | "rlogin" | "rsh" => {
            for tok in tokens {
                if !tok.starts_with('-') {
                    return Some(tok.to_string());
                }
            }
            None
        }
        _ => None,
    }
}

/// True if the command string contains `sudo` as a standalone word.
pub fn command_has_sudo(cmd: &str) -> bool {
    use regex::Regex;
    use std::sync::OnceLock;
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?:^|[;&|])\s*sudo\b").unwrap());
    re.is_match(cmd)
}

/// Write a structured JSONL event record to `~/.daemoneye/events.jsonl`.
///
/// Each call appends one JSON object per line.  The top-level fields
/// `ts` (ISO-8601 UTC) and `event` (event type name) are always present.
/// Additional fields are provided by the caller as a `serde_json::Value`
/// object and merged in.
///
/// Errors are silently discarded — logging must never crash the daemon.
pub fn log_event(event: &str, mut fields: serde_json::Value) {
    use std::io::Write;

    let path = crate::config::config_dir().join("events.jsonl");
    let ts = chrono::Utc::now().to_rfc3339();

    if let Some(obj) = fields.as_object_mut() {
        // Prepend ts + event so they appear first in the line.
        let mut record = serde_json::Map::new();
        record.insert("ts".to_string(), serde_json::Value::String(ts));
        record.insert(
            "event".to_string(),
            serde_json::Value::String(event.to_string()),
        );

        // Take ownership of the fields from the caller's object
        let drained = std::mem::take(obj);
        for (k, v) in drained {
            record.insert(k, v);
        }

        let mut line = serde_json::to_string(&record).unwrap_or_default();
        line.push('\n');

        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

/// Back-compat shim — existing call sites in server.rs still compile while
/// the migration to `log_event` is in progress.  New code should call
/// `log_event` directly.
pub fn log_command(
    session_id: Option<&str>,
    mode: &str,
    pane: &str,
    command: &str,
    status: &str,
    output_excerpt: &str,
) {
    let cmd: String = command
        .chars()
        .map(|c| if c == '\n' { ' ' } else { c })
        .collect();
    let out: String = output_excerpt
        .chars()
        .take(200)
        .map(|c| if c == '\n' { ' ' } else { c })
        .collect();
    log_event(
        "command",
        serde_json::json!({
            "session": session_id.unwrap_or("-"),
            "mode":    mode,
            "pane":    pane,
            "cmd":     cmd,
            "status":  status,
            "out":     out,
        }),
    );
}

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
    use crate::ai::Message;

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

    #[test]
    fn command_has_sudo_simple() {
        assert!(command_has_sudo("sudo apt install vim"));
    }

    #[test]
    fn command_has_sudo_in_pipeline() {
        assert!(command_has_sudo("echo hi | sudo tee /etc/hosts"));
    }

    #[test]
    fn command_has_sudo_after_semicolon() {
        assert!(command_has_sudo("cd /tmp; sudo rm -rf foo"));
    }

    #[test]
    fn command_has_sudo_false_positive_guard() {
        // "sudoer" is not "sudo" — word-boundary check must hold.
        assert!(!command_has_sudo("cat /etc/sudoers"));
    }

    #[test]
    fn command_has_sudo_no_sudo() {
        assert!(!command_has_sudo("ls -la /home"));
    }

    // ── shell_escape_arg ──────────────────────────────────────────────────────

    #[test]
    fn shell_escape_arg_plain_passthrough() {
        assert_eq!(shell_escape_arg("my-session"), "my-session");
    }

    #[test]
    fn shell_escape_arg_double_quote() {
        assert_eq!(shell_escape_arg(r#"a"b"#), r#"a\"b"#);
    }

    #[test]
    fn shell_escape_arg_dollar() {
        assert_eq!(shell_escape_arg("a$HOME"), r"a\$HOME");
    }

    #[test]
    fn shell_escape_arg_backtick() {
        assert_eq!(shell_escape_arg("a`cmd`"), r"a\`cmd\`");
    }

    #[test]
    fn shell_escape_arg_backslash() {
        assert_eq!(shell_escape_arg(r"a\b"), r"a\\b");
    }

    #[test]
    fn shell_escape_arg_spaces_unchanged() {
        // Spaces are safe inside "..." — no escaping needed.
        assert_eq!(shell_escape_arg("my session"), "my session");
    }

    #[test]
    fn shell_escape_arg_single_quote() {
        // A single-quote in the session name must be escaped as '\''
        // so it does not prematurely close the outer tmux single-quote context.
        assert_eq!(shell_escape_arg("my'session"), "my'\\''session");
    }

    #[test]
    fn shell_escape_arg_multiple_single_quotes() {
        assert_eq!(shell_escape_arg("a'b'c"), "a'\\''b'\\''c");
    }

    // ── is_interactive_command ────────────────────────────────────────────────

    #[test]
    fn interactive_plain_ssh() {
        assert!(is_interactive_command("ssh user@host"));
    }

    #[test]
    fn interactive_ssh_with_port_flag() {
        assert!(is_interactive_command("ssh -p 2222 user@host"));
    }

    #[test]
    fn interactive_ssh_with_identity_flag() {
        assert!(is_interactive_command("ssh -i ~/.ssh/id_rsa user@host"));
    }

    #[test]
    fn non_interactive_ssh_with_remote_command() {
        assert!(!is_interactive_command("ssh user@host ls /tmp"));
    }

    #[test]
    fn non_interactive_ssh_tunnel_N() {
        assert!(!is_interactive_command(
            "ssh -N -L 8080:localhost:80 user@host"
        ));
    }

    #[test]
    fn non_interactive_ssh_background_f() {
        assert!(!is_interactive_command(
            "ssh -f -N -R 2222:localhost:22 bastion"
        ));
    }

    #[test]
    fn interactive_mosh() {
        assert!(is_interactive_command("mosh user@host"));
    }

    #[test]
    fn interactive_telnet() {
        assert!(is_interactive_command("telnet 10.0.0.1 23"));
    }

    #[test]
    fn interactive_screen() {
        assert!(is_interactive_command("screen"));
    }

    #[test]
    fn non_interactive_ordinary_command() {
        assert!(!is_interactive_command("ls -la /home"));
    }

    #[test]
    fn non_interactive_empty() {
        assert!(!is_interactive_command(""));
    }

    // ── interactive_destination ───────────────────────────────────────────────

    #[test]
    fn destination_plain_ssh() {
        assert_eq!(
            interactive_destination("ssh user@host"),
            Some("user@host".to_string())
        );
    }

    #[test]
    fn destination_ssh_with_flags() {
        assert_eq!(
            interactive_destination("ssh -p 2222 -i ~/.ssh/id_rsa user@host"),
            Some("user@host".to_string())
        );
    }

    #[test]
    fn destination_mosh() {
        assert_eq!(
            interactive_destination("mosh admin@server"),
            Some("admin@server".to_string())
        );
    }

    #[test]
    fn destination_screen_returns_none() {
        assert_eq!(interactive_destination("screen"), None);
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

use crate::ipc::Response;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

pub async fn send_response(stream: &mut UnixStream, response: Response) -> anyhow::Result<()> {
    let mut data = serde_json::to_vec(&response)?;
    data.push(b'\n');
    stream.write_all(&data).await?;
    Ok(())
}

pub async fn send_response_split<W>(
    tx: &mut W,
    response: Response,
) -> anyhow::Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    let mut data = serde_json::to_vec(&response)?;
    data.push(b'\n');
    tx.write_all(&data).await?;
    Ok(())
}

pub fn fire_notification(job_name: &str, msg: &str, config: &crate::config::Config) {
    let cmd = &config.notifications.on_alert;
    if cmd.is_empty() {
        return;
    }
    let _ = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .env("DAEMONEYE_JOB", job_name)
        .env("DAEMONEYE_MSG", msg)
        .spawn();
}

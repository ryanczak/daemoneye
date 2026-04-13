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

/// Returns `true` when the system's PAM sudo configuration includes `pam_fprintd`,
/// indicating that fingerprint authentication may be requested for `sudo`.
///
/// Checks the standard PAM service files used by `sudo` on Linux.  Returns
/// `false` when the files cannot be read, which is the safe default — callers
/// fall back to the normal password-prompt path.
pub fn fingerprint_pam_configured() -> bool {
    for path in &["/etc/pam.d/sudo", "/etc/pam.d/sudo-i"] {
        if let Ok(content) = std::fs::read_to_string(path)
            && content.contains("pam_fprintd")
        {
            return true;
        }
    }
    false
}

/// True if the pane output contains a fingerprint-reader authentication prompt.
///
/// When PAM is configured to use a fingerprint reader, sudo replaces the normal
/// password prompt with a reader-specific message.  DaemonEye cannot satisfy
/// these prompts programmatically — callers must notify the user or abort.
pub fn is_fingerprint_prompt(output: &str) -> bool {
    output.contains("Place your finger on the fingerprint reader")
        || output.contains("Swipe your finger across the fingerprint reader")
        || output.contains("Failed to match fingerprint")
}

/// True if the command string contains `sudo` as a standalone word.
pub fn command_has_sudo(cmd: &str) -> bool {
    use regex::Regex;
    use std::sync::OnceLock;
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?:^|[;&|])\s*sudo\b").unwrap());
    re.is_match(cmd)
}

/// Returns `true` if the current user's sudo credentials are cached, i.e.
/// `sudo -n true` exits 0 without requiring a password.
///
/// Used as a pre-flight check before prompting the user or switching pane
/// focus.  A `false` return means a password will be required; `true` means
/// the command can proceed without interaction.
pub async fn sudo_credentials_cached() -> bool {
    tokio::process::Command::new("sudo")
        .args(["-n", "true"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Poll `pane_id` until a sudo password prompt appears in the scrollback, then
/// inject `credential` via `send-keys`.  Returns `true` if injection happened,
/// `false` if the prompt never appeared within the timeout.
///
/// Detects the locale-independent `[de-sudo-prompt]` sentinel (set by
/// `background.rs` for background windows) as well as the standard English
/// prompt strings for foreground panes.
pub async fn wait_for_sudo_prompt_and_inject(pane_id: &str, credential: &str) -> bool {
    const POLL: std::time::Duration = std::time::Duration::from_millis(200);
    const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
    let mut waited = std::time::Duration::ZERO;
    loop {
        tokio::time::sleep(POLL).await;
        waited += POLL;
        let snap = crate::tmux::capture_pane(pane_id, 20).unwrap_or_default();
        // Fingerprint prompts cannot be satisfied programmatically — fail fast
        // instead of waiting the full timeout.
        if is_fingerprint_prompt(&snap) {
            return false;
        }
        if snap.contains("[de-sudo-prompt]")
            || snap.contains("[sudo]")
            || snap.contains("password")
            || snap.contains("Password")
        {
            let _ = crate::tmux::send_keys(pane_id, credential);
            return true;
        }
        if waited >= TIMEOUT || crate::tmux::pane_dead_status(pane_id).is_some() {
            return false;
        }
    }
}

/// After injecting a sudo credential, poll the pane scrollback to see if sudo
/// rejected it ("Sorry, try again.").  Returns `true` if authentication failed
/// and a retry is needed, `false` if the credential was accepted.
pub async fn sudo_auth_failed(pane_id: &str) -> bool {
    const POLL: std::time::Duration = std::time::Duration::from_millis(150);
    const WINDOW: std::time::Duration = std::time::Duration::from_millis(2500);
    let mut waited = std::time::Duration::ZERO;
    loop {
        tokio::time::sleep(POLL).await;
        waited += POLL;
        let snap = crate::tmux::capture_pane(pane_id, 20).unwrap_or_default();
        if snap.contains("Sorry, try again") {
            return true;
        }
        if waited >= WINDOW {
            return false;
        }
    }
}

/// Write a structured JSONL event record to `~/.daemoneye/var/events.jsonl`.
///
/// Each call appends one JSON object per line.  The top-level fields
/// `ts` (ISO-8601 UTC) and `event` (event type name) are always present.
/// Additional fields are provided by the caller as a `serde_json::Value`
/// object and merged in.
///
/// Errors are silently discarded — logging must never crash the daemon.
pub fn log_event(event: &str, mut fields: serde_json::Value) {
    use std::io::Write;

    let path = crate::config::events_path();
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

/// Sanitize a shell command string into a short slug suitable for use as a
/// tmux window-name suffix.
///
/// Rules (applied in order):
/// 1. Tokenise on whitespace.
/// 2. Skip leading wrapper tokens: `sudo`, `env`, `nohup`, and any bare
///    `VAR=value` assignments.
/// 3. Take the basename of the first remaining token (strips path prefix).
/// 4. If that token is a common interpreter (`bash`, `sh`, `zsh`, `dash`,
///    `fish`, `ksh`, `python`, `python3`, `node`, `ruby`, `perl`), skip it
///    and use the basename of the *next* token instead (the script name).
/// 5. Replace any character outside `[a-zA-Z0-9._-]` with `-`.
/// 6. Collapse consecutive `-` characters into one.
/// 7. Truncate to `max_len` characters.
/// 8. Strip leading/trailing `-`.
/// 9. If the result is empty, return `"cmd"` as a fallback.
pub fn sanitize_cmd_for_window(cmd: &str, max_len: usize) -> String {
    let tokens: Vec<&str> = cmd.split_whitespace().collect();

    // Skip wrapper tokens at the front.
    let mut idx = 0;
    while idx < tokens.len() {
        let t = tokens[idx];
        if t == "sudo" || t == "env" || t == "nohup" {
            idx += 1;
        } else if t.contains('=') && !t.starts_with('-') {
            // bare VAR=value assignment
            idx += 1;
        } else {
            break;
        }
    }

    let Some(first) = tokens.get(idx) else {
        return "cmd".to_string();
    };

    // Take basename (strip path prefix).
    let first_base = std::path::Path::new(first)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(first);

    // If it's an interpreter, advance to the next token.
    const INTERPRETERS: &[&str] = &[
        "bash", "sh", "zsh", "dash", "fish", "ksh", "tcsh", "csh", "python", "python2", "python3",
        "node", "ruby", "perl",
    ];
    let raw = if INTERPRETERS.contains(&first_base) {
        if let Some(next) = tokens.get(idx + 1) {
            // Skip flags (e.g. `bash -c`)
            let next = if next.starts_with('-') {
                tokens.get(idx + 2).unwrap_or(next)
            } else {
                next
            };
            std::path::Path::new(next)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(next)
        } else {
            first_base
        }
    } else {
        first_base
    };

    // Sanitise: replace unsafe chars with '-', collapse runs, truncate.
    let sanitised: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();

    // Collapse consecutive dashes.
    let mut result = String::with_capacity(sanitised.len());
    let mut prev_dash = false;
    for c in sanitised.chars() {
        if c == '-' {
            if !prev_dash {
                result.push(c);
            }
            prev_dash = true;
        } else {
            result.push(c);
            prev_dash = false;
        }
    }

    // Truncate, then strip leading/trailing dashes.
    let truncated: String = result.chars().take(max_len).collect();
    let slug = truncated.trim_matches('-').to_string();

    if slug.is_empty() {
        "cmd".to_string()
    } else {
        slug
    }
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

    // ── sanitize_cmd_for_window ───────────────────────────────────────────────

    #[test]
    fn sanitize_simple_command() {
        assert_eq!(sanitize_cmd_for_window("ls -la /tmp", 30), "ls");
    }

    #[test]
    fn sanitize_strips_sudo() {
        assert_eq!(
            sanitize_cmd_for_window("sudo apt-get install foo", 30),
            "apt-get"
        );
    }

    #[test]
    fn sanitize_strips_env_prefix() {
        assert_eq!(
            sanitize_cmd_for_window("DEBIAN_FRONTEND=noninteractive apt update", 30),
            "apt"
        );
    }

    #[test]
    fn sanitize_strips_sudo_and_env() {
        assert_eq!(
            sanitize_cmd_for_window("sudo DEBIAN_FRONTEND=noninteractive apt-get upgrade", 30),
            "apt-get"
        );
    }

    #[test]
    fn sanitize_strips_path_prefix() {
        assert_eq!(
            sanitize_cmd_for_window("/usr/bin/curl -s http://example.com", 30),
            "curl"
        );
    }

    #[test]
    fn sanitize_interpreter_uses_script_name() {
        assert_eq!(
            sanitize_cmd_for_window("/usr/bin/python3 script.py", 30),
            "script.py"
        );
    }

    #[test]
    fn sanitize_bash_c_skips_flag() {
        // bash -c 'echo hi' — flag "-c" should be skipped, use next token
        assert_eq!(sanitize_cmd_for_window("bash -c 'echo hi'", 30), "echo");
    }

    #[test]
    fn sanitize_node_script() {
        assert_eq!(
            sanitize_cmd_for_window("node /home/user/app.js", 30),
            "app.js"
        );
    }

    #[test]
    fn sanitize_script_path_basename() {
        assert_eq!(
            sanitize_cmd_for_window("/home/user/.daemoneye/scripts/backup.sh", 30),
            "backup.sh"
        );
    }

    #[test]
    fn sanitize_special_chars_replaced() {
        assert_eq!(
            sanitize_cmd_for_window("./run@test#1.sh", 30),
            "run-test-1.sh"
        );
    }

    #[test]
    fn sanitize_truncates_to_max_len() {
        let long = "averylongcommandnamethatexceedslimit --flag";
        let result = sanitize_cmd_for_window(long, 10);
        assert!(result.len() <= 10);
    }

    #[test]
    fn sanitize_empty_returns_fallback() {
        assert_eq!(sanitize_cmd_for_window("", 30), "cmd");
    }

    #[test]
    fn sanitize_only_env_vars_returns_fallback() {
        assert_eq!(sanitize_cmd_for_window("FOO=bar BAZ=qux", 30), "cmd");
    }

    #[test]
    fn sanitize_only_special_chars_returns_fallback() {
        assert_eq!(sanitize_cmd_for_window("@@@", 30), "cmd");
    }

    #[test]
    fn sanitize_collapses_consecutive_dashes() {
        // Multiple adjacent non-alphanumeric chars become a single dash.
        assert_eq!(sanitize_cmd_for_window("a@@b", 30), "a-b");
    }

    #[test]
    fn sanitize_cargo_build() {
        assert_eq!(
            sanitize_cmd_for_window("cargo build --release", 30),
            "cargo"
        );
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

pub async fn send_response_split<W>(tx: &mut W, response: Response) -> anyhow::Result<()>
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

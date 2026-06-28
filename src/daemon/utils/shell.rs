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

/// Single-quote an arbitrary string so a POSIX shell parses it as one literal
/// token. Wraps in `'…'` and rewrites each embedded `'` as `'\''`. Use this
/// (NOT `shell_escape_arg`) whenever a value is placed inside single quotes —
/// e.g. building an `ssh <host> <cmd>` invocation.
pub fn sh_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
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

    // ── sh_single_quote ──────────────────────────────────────────────────────

    #[test]
    fn sh_single_quote_plain() {
        assert_eq!(sh_single_quote("echo hi"), "'echo hi'");
    }

    #[test]
    fn sh_single_quote_embedded_quote() {
        assert_eq!(sh_single_quote("echo 'pwned'"), r"'echo '\''pwned'\'''");
    }

    #[test]
    fn sh_single_quote_breakout_attempt() {
        assert_eq!(sh_single_quote("x'; rm -rf ~ #"), r"'x'\''; rm -rf ~ #'");
    }

    #[test]
    fn sh_single_quote_dollar_is_literal() {
        assert_eq!(sh_single_quote("$HOME"), "'$HOME'");
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
    fn non_interactive_ssh_tunnel_n() {
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

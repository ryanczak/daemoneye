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

/// Wrapper commands whose remaining words still name the command being
/// run, so a `sudo` after any of them is a command word. The set is
/// deliberately fail-closed (every word of the simple command counts) and
/// folds in `xargs`, formerly handled by its own pending flag.
const WRAPPER_COMMANDS: [&str; 22] = [
    "env",
    "nohup",
    "time",
    "exec",
    "command",
    "nice",
    "ionice",
    "timeout",
    "stdbuf",
    "setsid",
    "watch",
    "xargs",
    "strace",
    "ltrace",
    "flock",
    "chroot",
    "unshare",
    "nsenter",
    "setpriv",
    "chrt",
    "taskset",
    "systemd-run",
];

/// Shell keywords that, in command position, keep the *next* word there
/// too. In argument position they are ordinary words, so `echo then sudo`
/// does not match.
const COMMAND_KEYWORDS: [&str; 9] = [
    "if", "then", "else", "elif", "do", "while", "until", "{", "!",
];

/// Commands that elevate. In command position any of these counts as
/// elevation: by basename, so an absolute path (`/bin/su`) or a
/// leading-backslash spelling (`\doas`) counts too.
const ELEVATORS: [&str; 6] = ["sudo", "sudoedit", "su", "doas", "pkexec", "run0"];

/// One shell word, read the way the shell reads it: quote characters and
/// backslashes removed, the text they protect kept. So `s\udo`, `s'u'do`
/// and `"su""do"` all read as the word `sudo`.
struct ShellWord {
    /// The dequoted text.
    text: String,
    /// True when the word still carries an unquoted parameter expansion,
    /// glob character or brace expansion after dequoting — a word the
    /// detector cannot resolve.
    unresolvable: bool,
    /// True when the word carries an unquoted parameter expansion (`$`),
    /// as distinct from a glob. A wrapper runs its arguments through the
    /// shell, so an expansion behind one fails closed; a glob does not.
    expansion: bool,
    /// The contents of any quoted regions, scanned as a command in their own
    /// right: the string handed to `sh -c` is itself a command.
    quoted: String,
}

/// The characters that end a shell word. Quotes are deliberately absent:
/// they are read *through*, as part of the word.
fn is_word_delimiter(c: char) -> bool {
    matches!(
        c,
        ' ' | '\t' | ';' | '&' | '|' | '\n' | '\r' | '(' | ')' | '<' | '>' | '`'
    )
}

/// Read one shell word from `start`, dequoting as the shell does: a `\`
/// escapes the next character, single quotes protect everything literally,
/// double quotes protect everything but `$` (still an expansion) and the
/// escaped characters. A `$`, `*`, `?` or `[` that survives unquoted and
/// unescaped marks the word unresolvable. Returns the word and the index of
/// the first character after it.
fn read_shell_word(chars: &[char], start: usize) -> (ShellWord, usize) {
    let mut text = String::new();
    let mut quoted = String::new();
    let mut unresolvable = false;
    let mut expansion = false;
    let mut i = start;
    while i < chars.len() {
        match chars[i] {
            c if is_word_delimiter(c) => break,
            '\\' if i + 1 < chars.len() => {
                text.push(chars[i + 1]);
                i += 2;
            }
            '\\' => {
                text.push('\\');
                i += 1;
            }
            '\'' => {
                i += 1;
                while i < chars.len() && chars[i] != '\'' {
                    text.push(chars[i]);
                    quoted.push(chars[i]);
                    i += 1;
                }
                if i < chars.len() {
                    i += 1;
                }
            }
            '"' => {
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\'
                        && i + 1 < chars.len()
                        && matches!(chars[i + 1], '"' | '\\' | '$' | '`')
                    {
                        text.push(chars[i + 1]);
                        quoted.push(chars[i + 1]);
                        i += 2;
                    } else {
                        if chars[i] == '$' {
                            unresolvable = true;
                            expansion = true;
                        }
                        text.push(chars[i]);
                        quoted.push(chars[i]);
                        i += 1;
                    }
                }
                if i < chars.len() {
                    i += 1;
                }
            }
            c => {
                if matches!(c, '$' | '*' | '?' | '[') {
                    unresolvable = true;
                }
                if c == '$' {
                    expansion = true;
                }
                if c == '{' {
                    // bash brace expansion: `s{u,}do`, `{sudo,}`, `{a..b}`.
                    // A `{` whose word closes with `}` and whose contents
                    // hold `,` or `..` expands to words the detector cannot
                    // resolve. A bare `{` — the group keyword in `{ ls; }`
                    // — is left alone.
                    let mut j = i + 1;
                    while j < chars.len() && !is_word_delimiter(chars[j]) && chars[j] != '}' {
                        j += 1;
                    }
                    if j < chars.len() && chars[j] == '}' {
                        let inner = &chars[i + 1..j];
                        if inner.contains(&',') || inner.windows(2).any(|w| w == ['.', '.']) {
                            unresolvable = true;
                        }
                    }
                }
                text.push(c);
                i += 1;
            }
        }
    }
    (
        ShellWord {
            text,
            unresolvable,
            expansion,
            quoted,
        },
        i,
    )
}

/// Is `word` (already dequoted) an elevating command? Only the basename
/// after the last `/` is compared, so `/bin/su` counts; a leading `\` was
/// already removed by dequoting, so `\doas` counts.
fn is_elevator(word: &str) -> bool {
    let basename = std::path::Path::new(word)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(word);
    ELEVATORS.contains(&basename)
}

/// A leading `NAME=value` assignment in command position keeps the next
/// word there too: `FOO=1 sudo id`.
fn is_assignment(word: &str) -> bool {
    let name = word.split('=').next().unwrap_or("");
    word.split_once('=').is_some()
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_alphanumeric() || c == '_')
}

/// True when `cmd` elevates: a command word that is an elevator (`sudo`,
/// `sudoedit`, `su`, `doas`, `pkexec`, `run0`) or one the detector cannot
/// resolve, anywhere `sudo` could appear as a *command word* — at the
/// start, after `;`/`&`/`|` or a newline, inside `$(`, a backtick, `(` or a
/// quoted region (the string handed to `sh -c` is itself a command, so
/// quoted text is scanned, not skipped), after a wrapper command, after a
/// shell keyword, after a leading assignment, behind an absolute path, or
/// spelled across quotes and backslashes.
///
/// The detector **fails closed**: a command-position word that still
/// carries an unquoted parameter expansion (`$S`, `${X}udo`, `"$CMD"`) or
/// an unquoted glob character (`*`, `?`, `[`) after dequoting counts as
/// elevation. The same characters in argument position change nothing, so
/// `cat /etc/sudoers`, `echo pseudo`, `grep sudo /var/log/auth.log`,
/// `which sudo`, `man sudo`, `echo then sudo`, `ls *.txt` and
/// `FOO=$HOME ls` do not match. A quoted region may count more than an
/// unquoted argument does (`grep 'sudo' log` matches); that fails closed
/// and is accepted.
pub fn command_has_sudo(cmd: &str) -> bool {
    scan_shell_command(cmd, true)
}

/// The scanner behind [`command_has_sudo`]. `fail_closed` selects whether a
/// command-position word the detector cannot resolve counts as elevation.
/// Quoted regions that are plain arguments — a regex, a format string — are
/// rescanned with `fail_closed` off; a region that is code a shell will run
/// (the operand of a shell's `-c`, or of `eval`) gets it on (bug-05-1).
fn scan_shell_command(cmd: &str, fail_closed: bool) -> bool {
    let chars: Vec<char> = cmd.chars().collect();
    let mut i = 0;
    // The start of the string is a command position; every separator makes
    // the next word one again, and every plain word closes it off.
    let mut at_command = true;
    // A wrapper command is active: every later word of this simple command
    // is a command position, until the next separator.
    let mut wrapper = false;
    // Whether the word just read (or the word before it, for `su -c` and
    // `eval`) is a code operand: fail-closed applies inside its quoted
    // regions and to the word itself.
    let mut code_operand = false;

    while i < chars.len() {
        match chars[i] {
            ';' | '|' | '\n' | '\r' => {
                at_command = true;
                wrapper = false;
                code_operand = false;
                i += 1;
            }
            ' ' | '\t' => i += 1,
            // An unquoted `&` is a separator unless it opens a redirection
            // (`&>`, `&>>`), which never changes command position.
            '&' => {
                if chars.get(i + 1) == Some(&'>') {
                    i += 2;
                    if chars.get(i) == Some(&'>') {
                        i += 1;
                    }
                    // Skip the target word; it is never a command word.
                    let (_, end) = read_shell_word(&chars, i);
                    i = end;
                } else {
                    at_command = true;
                    wrapper = false;
                    i += 1;
                }
            }
            // A redirection and its target never change command position:
            // `</dev/null sudo id` and `2>/dev/null sudo id` still see
            // `sudo` as the command word. `<<<` folds into the same skip;
            // a leading fd digit (`2>`, `1<`) is read as a word and folded
            // in below.
            '<' | '>' => {
                i += 1;
                if chars.get(i) == Some(&'>') {
                    i += 1;
                }
                while i < chars.len() && chars[i] == '<' {
                    i += 1;
                }
                // Skip the target word; it is never a command word.
                let (_, end) = read_shell_word(&chars, i);
                i = end;
            }
            // Command substitution: what follows is a command. A quote
            // opens a command position exactly as `(` does: what is inside
            // is scanned, not skipped, because the string handed to
            // `sh -c` is itself a command (bug-13-1).
            '$' if chars.get(i + 1) == Some(&'(') => {
                at_command = true;
                wrapper = false;
                i += 2;
            }
            '`' | '(' => {
                at_command = true;
                wrapper = false;
                i += 1;
            }
            ')' => {
                at_command = false;
                wrapper = false;
                i += 1;
            }
            _ => {
                let (word, end) = read_shell_word(&chars, i);
                i = end;
                // An fd number written straight before a redirection
                // operator (`2>`, `1<`) belongs to the redirection, not
                // the command list; the operator arm consumes both.
                if i < chars.len()
                    && matches!(chars[i], '<' | '>')
                    && !word.text.is_empty()
                    && word.text.chars().all(|c| c.is_ascii_digit())
                {
                    continue;
                }
                if (at_command || wrapper) && is_elevator(&word.text) {
                    return true;
                }
                // The fail-closed rule applies inside a quoted region only
                // when that region is code a shell will run: the operand
                // after a shell's `-c`, an `eval` operand, or the operand
                // of `su -c`. A plain quoted argument — a regex, a format
                // string — is scanned for the elevator names but never
                // fails closed (bug-05-1).
                if !word.quoted.is_empty() && scan_shell_command(&word.quoted, code_operand) {
                    return true;
                }
                if code_operand && word.unresolvable {
                    // `sh -c "$CMD"`: the shell will expand and run it, so
                    // an unresolvable operand fails closed here too.
                    return true;
                }
                if wrapper {
                    if word.expansion {
                        return true;
                    }
                } else if at_command {
                    if word.unresolvable
                        && !is_assignment(&word.text)
                        && (fail_closed || code_operand)
                    {
                        // A command word the detector cannot resolve — a
                        // surviving parameter expansion, glob or brace
                        // expansion — fails closed to elevation. A leading
                        // assignment keeps its value's expansion out of
                        // command position.
                        return true;
                    }
                    if WRAPPER_COMMANDS.contains(&word.text.as_str()) {
                        wrapper = true;
                    } else if !COMMAND_KEYWORDS.contains(&word.text.as_str())
                        && !is_assignment(&word.text)
                    {
                        at_command = false;
                    }
                }
                code_operand = matches!(
                    word.text.as_str(),
                    "sh" | "bash" | "dash" | "zsh" | "ksh" | "su" | "eval"
                ) || (word.text == "-c" && code_operand);
            }
        }
    }
    false
}
/// The credential-prompt text sent to the client for a sudo password
/// request. Deliberately takes no command argument: the command is already
/// on screen directly above the dialog (the approval flow's scrollback
/// panel), so the prompt text must never embed a copy of it.
pub fn sudo_password_prompt(attempt: usize, max: usize) -> String {
    if attempt == 0 {
        "[sudo] password required".to_string()
    } else {
        format!(
            "sudo: Sorry, try again. Password for attempt {}/{}",
            attempt + 1,
            max
        )
    }
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

/// What a chunk of shell output is asking for, if anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    /// A sudo password prompt carrying our sentinel.
    SudoPassword,
    /// A fingerprint prompt — it cannot be satisfied by typing, so the
    /// caller must give up rather than wait.
    Fingerprint,
    /// ssh's unknown-host-key question. Answerable, but **not with a
    /// secret** — the answer is `yes` — so it is never routed through the
    /// credential round-trip; the caller gives up and the trust decision is
    /// the new-host approval flow's (M21 row 06).
    SshHostKey,
    /// ssh's login password prompt (`user@host's password:`) — answerable
    /// with a credential.
    SshPassword,
    /// ssh's key-passphrase prompt (`Enter passphrase for …`) — answerable
    /// with a credential.
    SshPassphrase,
}

/// Pure: what `output` is prompting for, given the sentinel this invocation
/// installed. `None` when it is not a prompt we can answer.
///
/// Matches the **sentinel**, never sudo's own `[sudo] password for …` text:
/// that text is locale- and `sudo.conf`-dependent, and stale output from an
/// earlier command in the same shell must not trigger a prompt. The sentinel
/// is per-invocation, so it cannot collide across commands.
///
/// The ssh branches below **are** matched by text, diverging from that rule —
/// deliberately, and safely for a narrower reason: ssh offers no sentinel
/// mechanism (no `SUDO_PROMPT` equivalent). That is acceptable here because
/// these prompts occur during **connection establishment on a shell the
/// daemon just spawned** — there is no earlier command in that shell whose
/// stale output could match. A `grep`ed fragment of a later command's own
/// echo cannot reach the classifier before ssh has finished connecting,
/// because nothing is written into the shell until its marker-wrapped
/// command runs, which is only after ssh is authenticated.
pub fn classify_prompt(output: &str, sentinel: &str) -> Option<PromptKind> {
    if is_fingerprint_prompt(output) {
        return Some(PromptKind::Fingerprint);
    }
    if output.contains(sentinel) {
        return Some(PromptKind::SudoPassword);
    }
    classify_ssh_prompt(output)
}

/// Pure: which ssh prompt `output` is raising, by text. Order with
/// `classify_prompt`'s: the fingerprint question (which contains the word
/// "fingerprint" twice, on the host-key form) and this invocation's sudo
/// sentinel both outrank every ssh text match. `None` when none matches.
pub fn classify_ssh_prompt(output: &str) -> Option<PromptKind> {
    if output.contains("Are you sure you want to continue connecting") {
        return Some(PromptKind::SshHostKey);
    }
    if output.contains("Enter passphrase for") {
        return Some(PromptKind::SshPassphrase);
    }
    // ssh's own prompt shape (`user@host's password: `). `su`'s bare
    // `Password:` is deliberately **not** matched: it is broader — any
    // program that prints `Password:` anywhere would arm it, stale text
    // included, and the stale-output negative the sentinel rule exists for
    // applies with full force in a shell that has already run commands. The
    // cost of missing it is bounded: `su` over a daemon ssh shell then waits
    // out the command's own timeout instead of being answered, which is the
    // status quo this phase found and does not worsen.
    if output.contains("'s password:") {
        return Some(PromptKind::SshPassword);
    }
    None
}

/// True when `output` carries ssh's terminal `Permission denied` line — not a
/// prompt: ssh has already failed and exits 255, so the caller must give up
/// rather than keep reading. Pure; named so the give-up decision can be
/// tested without a shell.
pub fn is_ssh_permission_denied(output: &str) -> bool {
    output.contains("Permission denied")
}

/// Per-invocation sudo prompt sentinel. The closing bracket makes the match
/// exact: `[de-sudo-prompt-3]` is not a substring of `[de-sudo-prompt-33]`.
pub fn sudo_sentinel(idx: usize) -> String {
    format!("[de-sudo-prompt-{idx}]")
}

/// Prefix `cmd` so sudo prints `sentinel` instead of its default password
/// prompt. Same shape as the background-window form in
/// `background/run.rs` (`SUDO_PROMPT='[de-sudo-prompt]' {cmd}`).
pub fn with_sudo_sentinel(cmd: &str, sentinel: &str) -> String {
    format!("SUDO_PROMPT='{sentinel}' {cmd}")
}

/// The PTY form of the prompt sentinel. `export` so every program on the
/// line sees it, not only the first; the sentinel is split across two
/// quoted strings so the shell's echo of the typed line never contains
/// it — only the program's real prompt does.
pub fn with_pty_prompt_sentinel(cmd: &str, sentinel: &str) -> String {
    format!(
        "export SUDO_PROMPT='{}'; {cmd}",
        sentinel.replacen("sudo-prompt", "sudo-''prompt", 1)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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

    // ── sudo_detect_ (M36 phase-13 S6) — sudo as a command word in every
    // position the whole-command rule must cover. ──────────────────────────

    #[test]
    fn sudo_detect_newline() {
        // A new line itself starts a new command: `sudo` there is a command
        // word, so it must be detected today.
        assert!(
            command_has_sudo("true\nsudo rm -rf /x"),
            "sudo on a continuation line is a command word and must be detected"
        );
    }

    #[test]
    fn sudo_detect_substitutions() {
        // `sudo` inside `$( )`, backticks or `( )` runs as a command — the
        // shell evaluates it before the outer command consumes the output.
        for (cmd, where_) in [
            ("echo $(sudo cat /etc/shadow)", "$("),
            ("echo `sudo id`", "backtick"),
            ("(sudo id)", "( )"),
        ] {
            assert!(
                command_has_sudo(cmd),
                "sudo inside a command substitution ({where_}) is a command word: {cmd:?}"
            );
        }
    }

    #[test]
    fn sudo_detect_xargs() {
        // `xargs` runs its remainder as a command — `sudo` there is the
        // command xargs will run, with or without `-0`.
        assert!(
            command_has_sudo("ls | xargs sudo rm"),
            "after xargs, sudo names the command xargs runs"
        );
        assert!(
            command_has_sudo("find . | xargs -0 sudo rm"),
            "after xargs -0, sudo names the command xargs runs"
        );
    }

    /// Preservation: the argument positions on the must-not-match list stay
    /// false, exactly as before this phase. Green before the phase.
    ///
    /// Extended with the keyword/assignment negatives the PE folded into
    /// bug-13-1: a keyword in **argument** position is an ordinary word, so
    /// `echo then sudo` stays sudo-free, and `sudo` as another command's
    /// argument (`which sudo`, `man sudo`) never becomes a command word.
    #[test]
    fn sudo_detect_argument_is_not_sudo() {
        for cmd in [
            "cat /etc/sudoers",
            "ls -la /home",
            "echo pseudo",
            "visudo -c",
            "grep sudo /var/log/auth.log",
            "echo then sudo",
            "which sudo",
            "man sudo",
        ] {
            assert!(
                !command_has_sudo(cmd),
                "preservation: sudo as an argument or substring must not match: {cmd:?}"
            );
        }
    }

    /// bug-13-1: a `sh -c` / `bash -c` string is itself a command, so quoted
    /// text must be **scanned**, not skipped. Test written red: the current
    /// detector skips the quoted region whole and misses both.
    #[test]
    fn sudo_detect_inside_quotes() {
        assert!(
            command_has_sudo("sh -c 'id; sudo rm -rf /x'"),
            "the string handed to sh -c is a command; sudo in it is a regression today"
        );
        assert!(
            command_has_sudo("bash -c \"sudo rm -rf /x\""),
            "the string handed to bash -c is a command; the old regex missed it, the \
             new detector must not"
        );
    }

    /// bug-13-1 (folded in): sudo behind a wrapper, a shell keyword, a
    /// leading assignment, an absolute path or a backslash escape — every
    /// position a shell puts `sudo` in command position. Test written red.
    #[test]
    fn sudo_detect_wrappers_keywords_paths() {
        for cmd in [
            "env sudo id",
            "env -i FOO=1 sudo id",
            "nohup sudo id",
            "time sudo id",
            "exec sudo id",
            "command sudo id",
            "nice -n 5 sudo id",
            "ionice -c 3 sudo id",
            "timeout 10 sudo id",
            "stdbuf -o0 sudo id",
            "setsid sudo id",
            "watch sudo id",
            "if true; then sudo id; fi",
            "{ sudo id; }",
            "! sudo id",
            "FOO=1 sudo id",
            "/usr/bin/sudo id",
            "\\sudo id",
        ] {
            assert!(
                command_has_sudo(cmd),
                "sudo in command position behind a wrapper, keyword, assignment, path \
                 or escape must be detected: {cmd:?}"
            );
        }
    }

    #[test]
    fn command_has_sudo_false_positive_guard() {
        // "sudoer" is not "sudo" — word-boundary check must hold.
        assert!(!command_has_sudo("cat /etc/sudoers"));
    }

    /// M39 phase-05: a shell word is read across quotes and backslash
    /// escapes, with the quote characters and backslashes removed before
    /// comparison. Quoted text is still scanned as a command as well.
    /// Test written red.
    #[test]
    fn sudo_detect_quote_and_backslash_spelling() {
        for cmd in [
            "s\\udo id",
            "s'u'do id",
            "\"su\"\"do\" id",
            "sh -c 's\\udo id'",
        ] {
            assert!(
                command_has_sudo(cmd),
                "a sudo spelled across quotes or backslashes is the command sudo: {cmd:?}"
            );
        }
    }

    /// M39 phase-05: a redirection and its target never change command
    /// position, so the word after a leading redirection is the command.
    /// Test written red.
    #[test]
    fn sudo_detect_after_leading_redirections() {
        for cmd in [
            "</dev/null sudo id",
            "2>/dev/null sudo id",
            ">out sudo id",
            ">>log sudo id",
            "&>/dev/null sudo id",
        ] {
            assert!(
                command_has_sudo(cmd),
                "a redirection and its target never change command position: {cmd:?}"
            );
        }
    }

    /// M39 phase-05: the other elevators — by basename, behind an absolute
    /// path or a leading-backslash spelling. Test written red.
    #[test]
    fn sudo_detect_other_elevators() {
        for cmd in [
            "su -c id",
            "doas id",
            "pkexec id",
            "run0 id",
            "sudoedit /etc/x",
            "/bin/su",
            "\\doas id",
        ] {
            assert!(
                command_has_sudo(cmd),
                "an elevator in command position is elevation whatever its spelling: {cmd:?}"
            );
        }
    }

    /// M39 phase-05: commands that run their remaining words as a command
    /// join the wrapper set; every later word of the simple command is a
    /// command position. Test written red.
    #[test]
    fn sudo_detect_more_wrappers() {
        for cmd in [
            "strace sudo id",
            "ltrace sudo id",
            "flock /tmp/l sudo id",
            "chroot /srv sudo id",
            "unshare -m sudo id",
            "nsenter -t 1 sudo id",
            "setpriv --reuid 0 sudo id",
            "chrt -f 1 sudo id",
            "taskset -c 0 sudo id",
            "systemd-run sudo id",
            "eval 'sudo id'",
        ] {
            assert!(
                command_has_sudo(cmd),
                "a wrapper keeps every later word in command position: {cmd:?}"
            );
        }
    }

    /// M39 phase-05: a command-position word the detector cannot resolve —
    /// one that still carries an unquoted parameter expansion or an
    /// unquoted glob character after dequoting — fails closed to
    /// elevation. Test written red.
    #[test]
    fn sudo_detect_unresolvable_command_word_fails_closed() {
        for cmd in [
            "/usr/bin/sud? id",
            "/usr/bin/s*do id",
            "$S id",
            "${X}udo id",
            "\"$CMD\" id",
        ] {
            assert!(
                command_has_sudo(cmd),
                "an unresolvable command word fails closed to elevation: {cmd:?}"
            );
        }
    }

    /// M39 phase-05 preservation: globs and expansions in **argument**
    /// position change nothing, and none of these is an elevating command
    /// word. Green before the phase.
    #[test]
    fn sudo_detect_globs_and_expansions_in_arguments_stay_clear() {
        for cmd in [
            "ls *.txt",
            "rm -f /tmp/*.log",
            "ls [ab].txt",
            "echo $HOME",
            "FOO=$HOME ls",
            "echo su",
            "echo doas",
            "cat /usr/bin/pkexec",
            "sudoku",
            "ls /usr/bin/su*",
            "git status",
        ] {
            assert!(
                !command_has_sudo(cmd),
                "preservation: globs and expansions in argument position stay clear: {cmd:?}"
            );
        }
    }

    /// M39 phase-05 bounce 1 (bug-05-1): quoted text is always scanned for
    /// the elevator names, but the fail-closed rule must NOT fire on a `$`
    /// or glob that is only a quoted *argument* — a regex or format string.
    /// Test written red.
    #[test]
    fn sudo_detect_quoted_arguments_stay_clear() {
        for cmd in [
            "find . -name '*.log'",
            "grep 'foo.*bar' f",
            "sed 's/a*/b/' f",
            "awk '$3 > 5' f",
            "echo \"$HOME\"",
            "cd \"$dir\"",
            "jq '.[0]' f",
            "ls \"$d\"/*.txt",
            "git commit -m 'fix: x'",
        ] {
            assert!(
                !command_has_sudo(cmd),
                "preservation: quoted arguments stay clear; the fail-closed rule must not \
                 fire inside a quoted region that is a plain argument: {cmd:?}"
            );
        }
    }

    /// M39 phase-05 bounce 1 (bug-05-1): the fail-closed rule DOES apply
    /// inside a quoted region that is code a shell will run — the operand
    /// after `-c` for the shells, an `eval` operand, or the operand of
    /// `su -c`. Green today; pins the behaviour the quoted-arguments fix
    /// must not lose.
    #[test]
    fn sudo_detect_shell_c_operand_fails_closed() {
        for cmd in [
            "sh -c \"$CMD\"",
            "bash -c 'sud? id'",
            "eval \"$X\"",
            "su -c \"$X\"",
        ] {
            assert!(
                command_has_sudo(cmd),
                "a shell -c / eval / su -c operand fails closed to elevation: {cmd:?}"
            );
        }
    }

    /// M39 phase-05 bounce 1 (bug-05-1): behind a wrapper, a word with an
    /// unquoted `$` expansion fails closed to elevation; a glob in that
    /// position does not. Test written red.
    #[test]
    fn sudo_detect_expansion_behind_a_wrapper_fails_closed() {
        for cmd in ["nohup $S id", "env $S id", "timeout 10 $S id"] {
            assert!(
                command_has_sudo(cmd),
                "an unquoted expansion behind a wrapper fails closed to elevation: {cmd:?}"
            );
        }
        assert!(
            !command_has_sudo("timeout 10 ls *.txt"),
            "preservation: a glob behind a wrapper stays clear: 'timeout 10 ls *.txt'"
        );
    }

    /// M39 phase-05 bounce 1 (bug-05-1): an unquoted brace expansion in
    /// command position is unresolvable and fails closed; a bare `{`
    /// keyword is unchanged. Test written red.
    #[test]
    fn sudo_detect_brace_expansion_fails_closed() {
        for cmd in ["s{u,}do id", "{sudo,} id", "/usr/bin/{su,x}do id"] {
            assert!(
                command_has_sudo(cmd),
                "an unquoted brace expansion in command position fails closed to \
                 elevation: {cmd:?}"
            );
        }
        assert!(
            !command_has_sudo("{ ls; }"),
            "preservation: a bare {{ keyword stays clear: '{{ ls; }}'"
        );
    }

    #[test]
    fn command_has_sudo_no_sudo() {
        assert!(!command_has_sudo("ls -la /home"));
    }

    #[test]
    fn sudo_password_prompt_first_attempt_omits_command() {
        assert_eq!(sudo_password_prompt(0, 3), "[sudo] password required");
        assert!(!sudo_password_prompt(0, 3).contains("for:"));
    }

    #[test]
    fn sudo_password_prompt_retry_names_attempt() {
        assert_eq!(
            sudo_password_prompt(1, 3),
            "sudo: Sorry, try again. Password for attempt 2/3"
        );
    }

    #[test]
    fn sudo_sentinel_bracket_disambiguates() {
        let snap = "[de-sudo-prompt-33]";
        assert!(
            !snap.contains(&sudo_sentinel(3)),
            "nonce 3 must not match a longer nonce 33"
        );
        assert!(snap.contains(&sudo_sentinel(33)));
    }

    #[test]
    fn with_sudo_sentinel_prefixes_sudo_command() {
        assert_eq!(
            with_sudo_sentinel("sudo pacman -Syu", "[de-sudo-prompt-4]"),
            "SUDO_PROMPT='[de-sudo-prompt-4]' sudo pacman -Syu"
        );
    }

    #[test]
    fn pty_prompt_sentinel_is_absent_from_the_typed_command() {
        let wrapped = with_pty_prompt_sentinel("x true", &sudo_sentinel(7));
        assert!(
            !wrapped.contains("[de-sudo-prompt-7]"),
            "the echo of the typed line must not carry the matchable sentinel: {wrapped}"
        );
        assert!(
            !wrapped.contains("de-sudo-prompt-"),
            "no matchable sentinel text may appear in the typed line at all: {wrapped}"
        );
        assert!(
            wrapped.starts_with("export SUDO_PROMPT="),
            "the PTY form exports so every command on the line sees it: {wrapped}"
        );
    }

    #[test]
    fn pty_prompt_sentinel_reaches_every_command_on_the_line() {
        let wrapped = with_pty_prompt_sentinel(
            "printf '%s|' \"$SUDO_PROMPT\"; true && printf '%s' \"$SUDO_PROMPT\"",
            &sudo_sentinel(7),
        );
        let out = std::process::Command::new("bash")
            .arg("-c")
            .arg(&wrapped)
            .output()
            .expect("bash is present in every test environment");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "[de-sudo-prompt-7]|[de-sudo-prompt-7]",
            "both commands on the line see the unsplit sentinel: {:?}",
            String::from_utf8_lossy(&out.stdout)
        );
    }

    #[test]
    fn stale_prompt_text_does_not_match_sentinel() {
        let snap = "$ sudo systemctl restart nginx\n[sudo] password for matt:\n$ sudo journalctl -u nginx\n";
        assert!(!snap.contains(&sudo_sentinel(7)));
    }

    #[test]
    fn command_echo_password_word_does_not_match_sentinel() {
        let snap = "$ sudo grep password /etc/shadow\n";
        assert!(!snap.contains(&sudo_sentinel(7)));
    }

    #[test]
    fn classify_prompt_matches_only_the_sentinel() {
        // Sudo's own text is locale- and sudo.conf-dependent: it must never
        // trigger, because stale output from an earlier command in the same
        // shell would otherwise re-arm the prompt.
        assert_eq!(
            classify_prompt("[sudo] password for matt: ", "[de-sudo-prompt-1]"),
            None,
            "sudo's own prompt text must not match"
        );
        assert_eq!(
            classify_prompt(
                "some preamble\r\n[de-sudo-prompt-1] \r\n",
                "[de-sudo-prompt-1]"
            ),
            Some(PromptKind::SudoPassword),
            "the sentinel this invocation installed must match"
        );
    }

    #[test]
    fn classify_prompt_rejects_a_longer_sentinel() {
        // The closing bracket is what makes the match exact: the shorter
        // sentinel must not match inside a longer one.
        assert_eq!(
            classify_prompt("[de-sudo-prompt-11]", "[de-sudo-prompt-1]"),
            None,
            "[de-sudo-prompt-1] is not a substring of [de-sudo-prompt-11]"
        );
    }

    #[test]
    fn classify_prompt_prefers_fingerprint() {
        let snap = "Place your finger on the fingerprint reader [de-sudo-prompt-1]";
        assert_eq!(
            classify_prompt(snap, "[de-sudo-prompt-1]"),
            Some(PromptKind::Fingerprint),
            "a fingerprint prompt cannot be typed at, even with the sentinel visible"
        );
    }

    #[test]
    fn classify_prompt_matches_the_ssh_host_key_question() {
        let snap = "The authenticity of host 'pinky.home.planetfoo.org (172.16.50.75)' \
                    can't be established.\r\nED25519 key fingerprint is: \
                    SHA256:8H2tFe0tLZodJlVr7s88lqZFVQTt6eo4XiGLGm1q4GI\r\n\
                    This key is not known by any other names.\r\nAre you sure you want to \
                    continue connecting (yes/no/[fingerprint])? ";
        assert_eq!(
            classify_prompt(snap, "[de-sudo-prompt-1]"),
            Some(PromptKind::SshHostKey)
        );
    }

    #[test]
    fn classify_prompt_matches_an_ssh_password_prompt() {
        let snap = "matt@pinky.home.planetfoo.org's password: ";
        assert_eq!(
            classify_prompt(snap, "[de-sudo-prompt-1]"),
            Some(PromptKind::SshPassword)
        );
    }

    #[test]
    fn classify_prompt_matches_an_ssh_passphrase_prompt() {
        let snap = "Enter passphrase for key '/home/matt/.ssh/id_ed25519': ";
        assert_eq!(
            classify_prompt(snap, "[de-sudo-prompt-1]"),
            Some(PromptKind::SshPassphrase)
        );
    }

    #[test]
    fn host_key_prompt_is_not_a_fingerprint_prompt() {
        let snap = "The authenticity of host 'pinky.home.planetfoo.org (172.16.50.75)' \
                    can't be established.\r\nED25519 key fingerprint is: \
                    SHA256:8H2tFe0tLZodJlVr7s88lqZFVQTt6eo4XiGLGm1q4GI\r\n\
                    This key is not known by any other names.\r\nAre you sure you want to \
                    continue connecting (yes/no/[fingerprint])? ";
        assert!(
            !is_fingerprint_prompt(snap),
            "the host-key question contains the word \"fingerprint\" twice but names no \
             reader; a biometric matcher that fires here would misclassify every unknown \
             host as an unanswerable fingerprint prompt"
        );
        assert_eq!(
            classify_prompt(snap, "[de-sudo-prompt-1]"),
            Some(PromptKind::SshHostKey),
            "the host-key question must classify as SshHostKey, not Fingerprint"
        );
    }

    #[test]
    fn a_real_biometric_prompt_still_classifies_as_fingerprint() {
        for snap in [
            "Place your finger on the fingerprint reader",
            "Swipe your finger across the fingerprint reader",
            "Failed to match fingerprint",
        ] {
            assert_eq!(
                classify_prompt(snap, "[de-sudo-prompt-1]"),
                Some(PromptKind::Fingerprint),
                "preservation: {snap:?} must remain unanswerable"
            );
        }
    }

    #[test]
    fn the_sentinel_still_wins_over_ssh_text() {
        let snap = "matt@pinky.home.planetfoo.org's password: \r\n[de-sudo-prompt-1] \r\n";
        assert_eq!(
            classify_prompt(snap, "[de-sudo-prompt-1]"),
            Some(PromptKind::SudoPassword),
            "this invocation's sentinel outranks ssh text in the same chunk"
        );
    }

    #[test]
    fn su_password_prompt_classification_is_pinned() {
        // `su` prints a bare `Password:`. It is deliberately NOT matched:
        // the bare form is broader than ssh's `'s password:` — any program
        // that prints `Password:` anywhere, stale output included, would arm
        // it — and the stale-output negative the sentinel rule exists for
        // applies with full force in a shell that has already run commands.
        // The cost of the miss is bounded: an unanswered `su` waits out the
        // command's own timeout, the status quo this phase measured and does
        // not worsen.
        assert_eq!(
            classify_prompt("Password: ", "[de-sudo-prompt-1]"),
            None,
            "a bare `Password:` is not classified as an ssh prompt"
        );
    }

    #[test]
    fn stale_ssh_prompt_text_from_an_earlier_command_does_not_classify() {
        // ssh's prompts appear only during connection establishment, so a
        // fragment of ssh text echoed by a *later* command in that shell can
        // only be a command's own echo, never a live prompt. The classifier
        // has no way to tell an echo from a prompt by text alone — the
        // limitation is pinned here rather than hidden: the matcher is
        // text-only and would match the echo too, so the guard is the
        // structural one in the phase doc's safety argument (ssh text
        // reaches the classifier only while the connection is being
        // established), not a property of `classify_prompt` itself.
        let echo = "$ grep \"Enter passphrase for\" notes.txt\nEnter passphrase for\n$ ";
        assert_eq!(
            classify_ssh_prompt(echo),
            Some(PromptKind::SshPassphrase),
            "pinned limitation: the text-only matcher cannot distinguish a command echo \
             from a live prompt"
        );
        assert_eq!(
            classify_prompt(echo, "[de-sudo-prompt-1]"),
            Some(PromptKind::SshPassphrase),
            "the same echo through classify_prompt"
        );
    }
}

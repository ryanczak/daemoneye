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
    let re = RE.get_or_init(|| {
        // INVARIANT: literal is a valid regex
        Regex::new(r"(?:^|[;&|])\s*sudo\b").unwrap()
    });
    re.is_match(cmd)
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

    #[test]
    fn command_has_sudo_false_positive_guard() {
        // "sudoer" is not "sudo" — word-boundary check must hold.
        assert!(!command_has_sudo("cat /etc/sudoers"));
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

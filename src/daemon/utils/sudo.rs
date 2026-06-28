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
}

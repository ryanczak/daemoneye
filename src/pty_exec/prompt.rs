use regex::Regex;
use std::sync::OnceLock;

/// The kind of interactive prompt detected in subprocess output.
pub enum PromptKind {
    /// A credential entry prompt (password, passphrase, PIN).
    Credential,
    /// A yes/no confirmation prompt (e.g. SSH host-key).
    Confirmation,
}

/// A detected interactive prompt event.
pub struct PromptEvent {
    pub kind: PromptKind,
    /// The last few lines of output that triggered the match (trimmed).
    pub text: String,
}

/// Detects interactive prompts in subprocess output.
///
/// Used by both the PTY background executor (to intercept and relay prompts)
/// and the foreground tmux-pane wait loop (to notify and switch focus).
pub struct PromptDetector {
    /// Number of credential injection attempts so far (PTY path only).
    pub attempt_count: u32,
}

impl PromptDetector {
    pub fn new() -> Self {
        Self { attempt_count: 0 }
    }

    /// Record a credential injection attempt (PTY path).
    pub fn record_attempt(&mut self) {
        self.attempt_count += 1;
    }

    /// True when the maximum number of credential attempts has been reached.
    pub fn exhausted(&self) -> bool {
        self.attempt_count >= 3
    }

    /// Check the tail of `buf` for an interactive prompt pattern.
    ///
    /// Only the last 5 lines are examined to avoid false positives from
    /// earlier output that happens to mention passwords or confirmations.
    /// Returns `Some(PromptEvent)` on the first match, `None` otherwise.
    pub fn check(&self, buf: &str) -> Option<PromptEvent> {
        let tail = tail_lines(buf, 5);
        for pat in credential_patterns() {
            if pat.is_match(tail) {
                return Some(PromptEvent {
                    kind: PromptKind::Credential,
                    text: tail.trim().to_string(),
                });
            }
        }
        for pat in confirmation_patterns() {
            if pat.is_match(tail) {
                return Some(PromptEvent {
                    kind: PromptKind::Confirmation,
                    text: tail.trim().to_string(),
                });
            }
        }
        None
    }
}

/// Return a sub-slice of `s` containing only the last `n` lines.
fn tail_lines(s: &str, n: usize) -> &str {
    let bytes = s.as_bytes();
    let mut count = 0usize;
    let mut start = 0usize;
    for i in (0..bytes.len()).rev() {
        if bytes[i] == b'\n' {
            count += 1;
            if count >= n {
                start = i + 1;
                break;
            }
        }
    }
    &s[start..]
}

fn credential_patterns() -> &'static [Regex] {
    static PATS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATS.get_or_init(|| {
        [
            r"(?i)\[sudo\] password for \S+:\s*$",
            r"(?mi)^\s*Password:\s*$",
            r"(?i)Enter passphrase for .*:\s*$",
            r"(?i)\S+@\S+'s password:\s*$",
            r"(?mi)^\s*Enter password:\s*$",
            r"(?i)Password \(again\):\s*$",
            r"(?mi)^\s*PIN:\s*$",
        ]
        .iter()
        .map(|p| Regex::new(p).unwrap())
        .collect()
    })
}

fn confirmation_patterns() -> &'static [Regex] {
    static PATS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATS.get_or_init(|| {
        [
            r"(?i)Are you sure you want to continue connecting \(yes/no",
            r"(?i)\(yes/no(/\[fingerprint\])?\)\s*[?:]?\s*$",
            r"(?i)\[y/N\]\s*[?:]\s*$",
            r"(?i)Proceed\? \(yes/no\)",
            r"(?i)Do you want to continue\? \[Y/n\]",
        ]
        .iter()
        .map(|p| Regex::new(p).unwrap())
        .collect()
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn det() -> PromptDetector { PromptDetector::new() }

    // ── Credential patterns ─────────────────────────────────────────────────

    #[test]
    fn detects_sudo_password() {
        let buf = "[sudo] password for alice:";
        assert!(matches!(det().check(buf), Some(PromptEvent { kind: PromptKind::Credential, .. })));
    }

    #[test]
    fn detects_bare_password_prompt() {
        let buf = "some output\nPassword:";
        assert!(matches!(det().check(buf), Some(PromptEvent { kind: PromptKind::Credential, .. })));
    }

    #[test]
    fn detects_ssh_passphrase() {
        let buf = "Enter passphrase for key '/home/alice/.ssh/id_ed25519':";
        assert!(matches!(det().check(buf), Some(PromptEvent { kind: PromptKind::Credential, .. })));
    }

    #[test]
    fn detects_ssh_password() {
        let buf = "alice@web01's password:";
        assert!(matches!(det().check(buf), Some(PromptEvent { kind: PromptKind::Credential, .. })));
    }

    // ── Confirmation patterns ───────────────────────────────────────────────

    #[test]
    fn detects_ssh_host_key() {
        let buf = "The authenticity of host 'example.com' can't be established.\n\
                   Are you sure you want to continue connecting (yes/no/[fingerprint])?";
        assert!(matches!(det().check(buf), Some(PromptEvent { kind: PromptKind::Confirmation, .. })));
    }

    #[test]
    fn detects_yes_no_prompt() {
        let buf = "Do you want to continue? [Y/n]";
        assert!(matches!(det().check(buf), Some(PromptEvent { kind: PromptKind::Confirmation, .. })));
    }

    // ── False-positive guards ───────────────────────────────────────────────

    #[test]
    fn no_false_positive_for_password_in_output() {
        // "Password:" mid-line (not the last stripped line) should not match.
        let buf = "Setting Password: done\nCommand completed successfully.";
        assert!(det().check(buf).is_none());
    }

    #[test]
    fn no_match_on_empty_buf() {
        assert!(det().check("").is_none());
    }

    #[test]
    fn no_match_on_normal_output() {
        let buf = "total 24\ndrwxr-xr-x 3 alice alice 4096 Jan 1 00:00 .\n";
        assert!(det().check(buf).is_none());
    }

    // ── Tail windowing ──────────────────────────────────────────────────────

    #[test]
    fn prompt_buried_beyond_tail_is_not_detected() {
        // Put a sudo prompt 10 lines back; the 5-line tail won't see it.
        let prefix = (0..10).map(|_| "output line\n").collect::<String>();
        let buf = format!("{}normal line\nnormal line\nnormal line\nnormal line\nnormal line",
                          prefix);
        // The sudo prompt is in the prefix (> 5 lines back), so not in tail.
        assert!(det().check(&buf).is_none());
    }

    // ── Attempt tracking ────────────────────────────────────────────────────

    #[test]
    fn exhausted_after_three_attempts() {
        let mut d = det();
        assert!(!d.exhausted());
        d.record_attempt();
        d.record_attempt();
        assert!(!d.exhausted());
        d.record_attempt();
        assert!(d.exhausted());
    }
}

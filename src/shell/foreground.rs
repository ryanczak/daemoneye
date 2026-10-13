//! The pure `Foreground` classifier: what holds a shell's terminal, decided
//! from the host's `Status` reply (local shells) or from the daemon's own
//! bookkeeping (transport shells).
//!
//! Design: `docs/design/interactive-shells.md` D3; the row matrix in § 5 is
//! what [`classify_matches_the_design_matrix`] and
//! [`classify_transport_matches_the_design_matrix`] pin.

use crate::shell::proto::{ShellResponse, ShellState};

/// What holds a shell's terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Foreground {
    /// `ShellState::Idle`: the shell owns its own terminal.
    Prompt,
    /// `ShellState::Running` with a named (or unknown) command in the
    /// foreground; `tui` is the raw-mode / alt-screen bit.
    Program { cmd: String, tui: bool },
    /// A stopped job holds the terminal.
    Paused { cmd: String },
    /// The shell child has exited.
    Exited,
    /// The status is unreadable, or the host predates the foreground facts
    /// and cannot answer. Treated as busy everywhere a decision is made.
    Unknown,
}

impl std::fmt::Display for Foreground {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Foreground::Prompt => write!(f, "shell prompt"),
            Foreground::Program { cmd, tui: true } => write!(f, "running {cmd} (TUI)"),
            Foreground::Program { cmd, tui: false } => write!(f, "running {cmd}"),
            Foreground::Paused { cmd } => write!(f, "paused {cmd}"),
            Foreground::Exited => write!(f, "exited"),
            Foreground::Unknown => write!(f, "unknown"),
        }
    }
}

/// Classify a shell from its host's `Status` reply.
///
/// `None` — the host did not answer — and a reply that is not a `Status`
/// frame classify `Unknown`. A `Status` without `raw_mode` (a host from
/// before this milestone) classifies `Unknown` in **every** state, never
/// `Prompt`: the cost of a false "busy" is a fresh shell, the cost of a
/// false "prompt" is a poisoned program. `alt_screen` and `foreground` ride
/// on the same build as `raw_mode`, so a host that answers one answers all;
/// a `None` foreground means the group is the shell's own, which is `"?"`
/// as a command name, not an absence.
pub fn classify(status: Option<&ShellResponse>) -> Foreground {
    let Some(ShellResponse::Status {
        state,
        raw_mode,
        alt_screen,
        foreground,
        ..
    }) = status
    else {
        return Foreground::Unknown;
    };
    if *state == ShellState::Exited {
        return Foreground::Exited;
    }
    let Some(raw_mode) = raw_mode else {
        return Foreground::Unknown;
    };
    let cmd = foreground.clone().unwrap_or_else(|| "?".to_string());
    match state {
        ShellState::Idle => Foreground::Prompt,
        ShellState::Paused => Foreground::Paused { cmd },
        ShellState::Running => Foreground::Program {
            cmd,
            tui: *raw_mode || *alt_screen == Some(true),
        },
        ShellState::Exited => unreachable!("handled above"),
    }
}

/// Classify a transport shell (a record with a named `host` or a `sandbox`)
/// from the daemon's own bookkeeping, never from the local terminal: over
/// ssh or sbx the local pty slave sits in raw mode for the life of the
/// session, so the terminal says nothing about the remote side.
///
/// - `state == Exited` → `Exited`;
/// - `in_flight` set — the daemon has a marker-wrapped command outstanding
///   on this shell — → `Program { cmd: <that command>, tui: false }`;
/// - otherwise, when the last non-blank rendered row ends in a prompt
///   character (`$`, `#`, `%`, `>` not preceded by a digit) → `Prompt`;
/// - otherwise → `Program { cmd: "<remote>", tui: true }`: something the
///   daemon did not start holds the remote terminal.
pub fn classify_transport(
    state: ShellState,
    in_flight: Option<&str>,
    last_line: Option<&str>,
) -> Foreground {
    match state {
        ShellState::Exited => Foreground::Exited,
        _ => match in_flight {
            Some(cmd) => Foreground::Program {
                cmd: cmd.to_string(),
                tui: false,
            },
            None => match last_line.filter(|l| is_prompt_line(l)) {
                Some(_) => Foreground::Prompt,
                None => Foreground::Program {
                    cmd: "<remote>".to_string(),
                    tui: true,
                },
            },
        },
    }
}

/// Whether a rendered row reads as a shell prompt: trimmed, it ends in `$`,
/// `#`, `%` or `>` whose preceding character is not an ASCII digit. A digit
/// before the character is a progress counter or a version, not a prompt
/// (`Progress 50%`), and an empty line is nothing at all.
fn is_prompt_line(line: &str) -> bool {
    let trimmed = line.trim_end();
    let mut chars = trimmed.chars();
    let Some(last) = chars.next_back() else {
        return false;
    };
    if !matches!(last, '$' | '#' | '%' | '>') {
        return false;
    }
    !matches!(chars.next_back(), Some(c) if c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::proto::ShellResponse;

    fn status(
        state: ShellState,
        raw: Option<bool>,
        alt: Option<bool>,
        fg: Option<&str>,
    ) -> ShellResponse {
        ShellResponse::Status {
            state,
            rows: 24,
            cols: 80,
            pid: 1,
            raw_mode: raw,
            alt_screen: alt,
            foreground: fg.map(str::to_string),
        }
    }

    #[test]
    fn classify_matches_the_design_matrix() {
        let cases: Vec<(ShellResponse, Foreground)> = vec![
            // Idle: the shell owns its own terminal.
            (
                status(ShellState::Idle, Some(false), Some(false), None),
                Foreground::Prompt,
            ),
            // Running, cooked, a plain command.
            (
                status(ShellState::Running, Some(false), Some(false), Some("sleep")),
                Foreground::Program {
                    cmd: "sleep".into(),
                    tui: false,
                },
            ),
            // Running, raw mode only.
            (
                status(ShellState::Running, Some(true), Some(false), Some("claude")),
                Foreground::Program {
                    cmd: "claude".into(),
                    tui: true,
                },
            ),
            // Running, raw mode and alt screen.
            (
                status(ShellState::Running, Some(true), Some(true), Some("vim")),
                Foreground::Program {
                    cmd: "vim".into(),
                    tui: true,
                },
            ),
            // Running, alt screen only.
            (
                status(ShellState::Running, Some(false), Some(true), Some("less")),
                Foreground::Program {
                    cmd: "less".into(),
                    tui: true,
                },
            ),
            // Paused, facts whatever they are.
            (
                status(ShellState::Paused, Some(false), Some(false), Some("sleep")),
                Foreground::Paused {
                    cmd: "sleep".into(),
                },
            ),
            // Exited, facts whatever they are.
            (
                status(ShellState::Exited, None, None, None),
                Foreground::Exited,
            ),
        ];
        for (i, (reply, want)) in cases.into_iter().enumerate() {
            assert_eq!(
                classify(Some(&reply)),
                want,
                "row {i} of the design matrix, reply {reply:?}"
            );
        }
        assert_eq!(classify(None), Foreground::Unknown, "no answer");
        assert_eq!(
            classify(Some(&ShellResponse::Ok)),
            Foreground::Unknown,
            "a non-Status reply"
        );
    }

    #[test]
    fn classify_never_calls_a_pre_phase_host_a_prompt() {
        // Every field `None`: a host from before this milestone. The facts
        // are absent, so the classification is `Unknown` in every state —
        // never `Prompt`, which would let a command be typed into whatever
        // really holds the terminal.
        for state in [ShellState::Idle, ShellState::Running, ShellState::Paused] {
            assert_eq!(
                classify(Some(&status(state, None, None, None))),
                Foreground::Unknown,
                "state {state:?} with no foreground facts"
            );
        }
    }

    #[test]
    fn classify_transport_matches_the_design_matrix() {
        // in_flight set: the daemon's own command is outstanding.
        assert_eq!(
            classify_transport(
                ShellState::Running,
                Some("cargo build"),
                Some("Compiling 3 crates")
            ),
            Foreground::Program {
                cmd: "cargo build".into(),
                tui: false
            },
            "in_flight wins over whatever the last row shows"
        );
        // No in_flight, last row a prompt: reusable.
        assert_eq!(
            classify_transport(ShellState::Running, None, Some("matt@zoidberg:~$ ")),
            Foreground::Prompt,
            "a remote prompt row"
        );
        // No in_flight, last row not a prompt: something daemon-unknown holds
        // the remote terminal.
        assert_eq!(
            classify_transport(ShellState::Running, None, Some("claude> thinking")),
            Foreground::Program {
                cmd: "<remote>".into(),
                tui: true
            },
            "a non-prompt last row"
        );
        // Exited wins over everything.
        assert_eq!(
            classify_transport(ShellState::Exited, Some("cargo build"), None),
            Foreground::Exited,
            "exited"
        );
    }

    #[test]
    fn classify_transport_prompt_rule() {
        for line in [
            "matt@zoidberg:~$ ",
            "root@box:/# ",
            "zoidberg% ",
            "PS C:\\> ",
        ] {
            assert!(is_prompt_line(line), "must match: {line:?}");
        }
        for line in ["Progress 50%", "Password:", "Continue? (y/n)", "", "   "] {
            assert!(!is_prompt_line(line), "must NOT match: {line:?}");
        }
    }
}

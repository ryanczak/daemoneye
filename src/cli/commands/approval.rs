//! Per-session auto-approval state for tool calls.

use std::collections::HashSet;

/// Per-session auto-approval state.
///
/// - `regular` / `sudo`: class-wide approval for terminal commands **on the
///   daemon host**; a session grant on the daemon host never covers a remote
///   one (and vice versa).
/// - `scripts_all` / `runbooks_all` / `file_edits_all`: class-wide approval seeded from
///   `config.toml [approvals]`; bypasses the per-name `HashSet` check entirely.
/// - `scripts` / `runbooks` / `file_edits`: name/path-scoped approval; each entry
///   auto-approves future writes to that specific artifact for the rest of the session.
/// - `hosts_regular` / `hosts_sudo`: per-host command approval, keyed by the
///   host name from the frame's `Classification`. Started empty; the daemon
///   host's flags live in `regular` / `sudo` above.
#[derive(Clone)]
pub(super) struct SessionApproval {
    /// Non-sudo commands auto-approve without prompting.
    pub(super) regular: bool,
    pub(super) sudo: bool,
    /// All script writes pre-approved (config-seeded class-wide flag).
    pub(super) scripts_all: bool,
    pub(super) scripts: HashSet<String>,
    /// All runbook writes pre-approved (config-seeded class-wide flag).
    pub(super) runbooks_all: bool,
    pub(super) runbooks: HashSet<String>,
    /// All file edits pre-approved (config-seeded class-wide flag).
    pub(super) file_edits_all: bool,
    /// Paths auto-approved for the rest of this session via `[A]pprove for session`.
    /// Keyed by the canonical path string.
    pub(super) file_edits: HashSet<String>,
    /// Non-sudo commands auto-approved for a named remote host via
    /// `[A]pprove for session`. Keyed by the `Classification` host name.
    pub(super) hosts_regular: HashSet<String>,
    /// Sudo commands auto-approved for a named remote host.
    pub(super) hosts_sudo: HashSet<String>,
}

impl Default for SessionApproval {
    fn default() -> Self {
        Self {
            regular: true,
            sudo: false,
            scripts_all: false,
            scripts: HashSet::new(),
            runbooks_all: false,
            runbooks: HashSet::new(),
            file_edits_all: false,
            file_edits: HashSet::new(),
            hosts_regular: HashSet::new(),
            hosts_sudo: HashSet::new(),
        }
    }
}

impl SessionApproval {
    /// Build initial approval state from `config.toml [approvals]` settings.
    /// Called once at session start (chat/ask entry). Never re-derive this
    /// mid-session: runtime state (`/approvals`, prompt answers) must win —
    /// a turn-end re-derive was the 2026-08-11 approval-persistence bug.
    pub(super) fn from_config(cfg: &crate::config::ApprovalsConfig) -> Self {
        Self {
            regular: cfg.commands,
            sudo: cfg.sudo,
            scripts_all: cfg.scripts,
            runbooks_all: cfg.runbooks,
            file_edits_all: cfg.file_edits,
            ..Self::default()
        }
    }

    /// Task 1's scope rule, in one place: does this session already hold the
    /// approval a tool call of `class` / `is_sudo` needs to skip the prompt?
    ///
    /// | Classification | non-sudo | sudo |
    /// |---|---|---|
    /// | `Command` | `regular` | `sudo` |
    /// | `RemoteShell { host }` | host in `hosts_regular` | host in `hosts_sudo` |
    /// | `NewHost { .. }` | never | never |
    /// | `EscapeHatch` | never | never |
    ///
    /// Per host, not global: a grant on the daemon host covers only the daemon
    /// host, a grant on `pinky` only `pinky`. `NewHost` and `EscapeHatch` read
    /// nothing — no session state, and no config default, can auto-approve
    /// them.
    pub(super) fn auto_approves(&self, class: &crate::ipc::Classification, is_sudo: bool) -> bool {
        match class {
            crate::ipc::Classification::Command => {
                if is_sudo {
                    self.sudo
                } else {
                    self.regular
                }
            }
            crate::ipc::Classification::RemoteShell { host } => {
                if is_sudo {
                    self.hosts_sudo.contains(host)
                } else {
                    self.hosts_regular.contains(host)
                }
            }
            // An unconfigured destination, and a ghost shell reaching outside
            // its sandbox, are always asked.
            crate::ipc::Classification::NewHost { .. }
            | crate::ipc::Classification::EscapeHatch => false,
        }
    }

    /// Record an `A` (approve for session) answer against the classification's
    /// key. `Command` sets the daemon-host flags; `RemoteShell` inserts into
    /// the per-host set; `NewHost` and `EscapeHatch` record nothing — they can
    /// never auto-approve, so offering to widen the session for them would be
    /// a lie. Returns the human-readable label of what was widened, or `None`
    /// when nothing was recorded.
    pub(super) fn record_session_approval(
        &mut self,
        class: &crate::ipc::Classification,
        is_sudo: bool,
    ) -> Option<String> {
        match class {
            crate::ipc::Classification::Command => {
                if is_sudo {
                    self.sudo = true;
                    Some("sudo".to_string())
                } else {
                    self.regular = true;
                    Some("regular".to_string())
                }
            }
            crate::ipc::Classification::RemoteShell { host } => {
                let set = if is_sudo {
                    &mut self.hosts_sudo
                } else {
                    &mut self.hosts_regular
                };
                set.insert(host.clone());
                Some(format!(
                    "{host} ({})",
                    if is_sudo { "sudo" } else { "commands" }
                ))
            }
            crate::ipc::Classification::NewHost { .. }
            | crate::ipc::Classification::EscapeHatch => None,
        }
    }

    /// The `/approvals` body lines for the per-host grants the session holds.
    /// `regular` / `sudo` render in their existing slots; these are the host
    /// rows below them, in a stable order.
    pub(super) fn host_approval_lines(&self) -> Vec<String> {
        let mut hosts: Vec<&String> = self
            .hosts_regular
            .iter()
            .chain(self.hosts_sudo.iter())
            .collect();
        hosts.sort();
        hosts.dedup();
        hosts
            .into_iter()
            .map(|h| {
                let scope = match (self.hosts_regular.contains(h), self.hosts_sudo.contains(h)) {
                    (true, true) => "commands + sudo",
                    (true, false) => "commands",
                    (false, true) => "sudo",
                    (false, false) => unreachable!("host came from one of the two sets"),
                };
                format!("  {h:<8} : {scope}")
            })
            .collect()
    }

    /// Build the status-bar hint string shown in the chat frame.
    pub(super) fn hint(&self) -> String {
        let mut active: Vec<String> = Vec::new();
        match (self.regular, self.sudo) {
            (true, true) => active.push("all".to_string()),
            (true, false) => {} // default state; shown as baseline label below
            (false, true) => active.push("sudo".to_string()),
            (false, false) => active.push("cmds: gated".to_string()),
        }
        if self.scripts_all {
            active.push("scripts: all".to_string());
        } else if !self.scripts.is_empty() {
            active.push(format!("scripts: {}", self.scripts.len()));
        }
        if self.runbooks_all {
            active.push("runbooks: all".to_string());
        } else if !self.runbooks.is_empty() {
            active.push(format!("runbooks: {}", self.runbooks.len()));
        }
        if self.file_edits_all {
            active.push("files: all".to_string());
        } else if !self.file_edits.is_empty() {
            active.push(format!("files: {}", self.file_edits.len()));
        }
        if active.is_empty() {
            "cmds: auto".to_string()
        } else {
            format!("⚡approvals: {} · Ctrl+C revokes", active.join(", "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ApprovalsConfig;

    fn approvals_all() -> ApprovalsConfig {
        ApprovalsConfig {
            commands: true,
            sudo: true,
            scripts: true,
            runbooks: true,
            file_edits: true,
            ghost_commands: true,
        }
    }

    fn approvals_none() -> ApprovalsConfig {
        ApprovalsConfig {
            commands: false,
            sudo: false,
            scripts: false,
            runbooks: false,
            file_edits: false,
            ghost_commands: false,
        }
    }

    // ── from_config ──────────────────────────────────────────────────────────

    #[test]
    fn from_config_default_matches_commands_true_others_false() {
        let cfg = ApprovalsConfig::default();
        let a = SessionApproval::from_config(&cfg);
        assert!(a.regular);
        assert!(!a.sudo);
        assert!(!a.scripts_all);
        assert!(!a.runbooks_all);
        assert!(!a.file_edits_all);
        assert!(a.scripts.is_empty());
        assert!(a.runbooks.is_empty());
        assert!(a.file_edits.is_empty());
    }

    #[test]
    fn from_config_all_true_sets_all_flags() {
        let a = SessionApproval::from_config(&approvals_all());
        assert!(a.regular);
        assert!(a.sudo);
        assert!(a.scripts_all);
        assert!(a.runbooks_all);
        assert!(a.file_edits_all);
    }

    #[test]
    fn from_config_all_false_clears_all_flags() {
        let a = SessionApproval::from_config(&approvals_none());
        assert!(!a.regular);
        assert!(!a.sudo);
        assert!(!a.scripts_all);
        assert!(!a.runbooks_all);
        assert!(!a.file_edits_all);
    }

    // ── *_all bypass ─────────────────────────────────────────────────────────

    #[test]
    fn scripts_all_bypasses_per_name_check() {
        let mut a = SessionApproval::from_config(&approvals_all());
        // scripts_all is true; the per-name set is empty — approval should still succeed.
        assert!(a.scripts_all || a.scripts.contains("any-script"));
        // Adding a name to the set doesn't change the *_all semantics.
        a.scripts.insert("other.sh".to_string());
        assert!(a.scripts_all || a.scripts.contains("new-script"));
    }

    #[test]
    fn runbooks_all_bypasses_per_name_check() {
        let a = SessionApproval::from_config(&approvals_all());
        assert!(a.runbooks_all || a.runbooks.contains("any-runbook"));
    }

    #[test]
    fn file_edits_all_bypasses_path_check() {
        let a = SessionApproval::from_config(&approvals_all());
        assert!(a.file_edits_all || a.file_edits.contains("/any/path"));
    }

    #[test]
    fn without_all_flag_per_name_is_required() {
        let a = SessionApproval::from_config(&approvals_none());
        assert!(!(a.scripts_all || a.scripts.contains("my-script")));
        assert!(!(a.runbooks_all || a.runbooks.contains("my-runbook")));
        assert!(!(a.file_edits_all || a.file_edits.contains("/tmp/f")));
    }

    #[test]
    fn per_name_set_works_when_all_flag_is_false() {
        let mut a = SessionApproval::from_config(&approvals_none());
        a.scripts.insert("specific.sh".to_string());
        assert!(a.scripts_all || a.scripts.contains("specific.sh"));
        assert!(!(a.scripts_all || a.scripts.contains("other.sh")));
    }

    // ── revoke ────────────────────────────────────────────────────────────────

    #[test]
    fn revoke_scripts_clears_flag_and_set() {
        let mut a = SessionApproval::from_config(&approvals_all());
        a.scripts.insert("foo.sh".to_string());
        // simulate revoke scripts
        a.scripts_all = false;
        a.scripts.clear();
        assert!(!a.scripts_all);
        assert!(a.scripts.is_empty());
        assert!(!(a.scripts_all || a.scripts.contains("foo.sh")));
    }

    #[test]
    fn revoke_all_clears_every_scope() {
        let mut a = SessionApproval::from_config(&approvals_all());
        a.scripts.insert("foo.sh".to_string());
        a.runbooks.insert("rb".to_string());
        a.file_edits.insert("/tmp/f".to_string());
        // simulate /approvals revoke (full struct replacement)
        a = SessionApproval {
            regular: false,
            sudo: false,
            scripts_all: false,
            scripts: HashSet::new(),
            runbooks_all: false,
            runbooks: HashSet::new(),
            file_edits_all: false,
            file_edits: HashSet::new(),
            hosts_regular: HashSet::new(),
            hosts_sudo: HashSet::new(),
        };
        assert!(!a.regular);
        assert!(!a.sudo);
        assert!(!a.scripts_all);
        assert!(a.scripts.is_empty());
        assert!(!a.runbooks_all);
        assert!(a.runbooks.is_empty());
        assert!(!a.file_edits_all);
        assert!(a.file_edits.is_empty());
    }

    // ── hint ─────────────────────────────────────────────────────────────────

    #[test]
    fn hint_default_is_auto() {
        let a = SessionApproval::default();
        assert_eq!(a.hint(), "cmds: auto");
    }

    #[test]
    fn hint_shows_all_when_both_command_flags_true() {
        let a = SessionApproval::from_config(&approvals_all());
        let h = a.hint();
        assert!(h.contains("all"), "hint='{}' should contain 'all'", h);
    }

    #[test]
    fn hint_shows_scripts_all_when_flag_set() {
        let a = SessionApproval {
            scripts_all: true,
            ..SessionApproval::default()
        };
        let h = a.hint();
        assert!(
            h.contains("scripts: all"),
            "hint='{}' should contain 'scripts: all'",
            h
        );
    }

    // ── per-host approval scope (M21 phase-06b) ─────────────────────────────

    fn remote(host: &str) -> crate::ipc::Classification {
        crate::ipc::Classification::RemoteShell {
            host: host.to_string(),
        }
    }

    #[test]
    fn local_command_auto_approves_when_regular_is_set() {
        let a = SessionApproval::from_config(&ApprovalsConfig::default());
        assert!(
            a.auto_approves(&crate::ipc::Classification::Command, false),
            "a Command + non-sudo must follow regular, which the shipped default sets"
        );
    }

    #[test]
    fn local_sudo_needs_the_sudo_flag() {
        let a = SessionApproval::from_config(&ApprovalsConfig::default());
        assert!(
            !a.auto_approves(&crate::ipc::Classification::Command, true),
            "a Command + sudo must follow sudo, not regular (shipped default: sudo=false)"
        );
        let a = SessionApproval::from_config(&approvals_all());
        assert!(
            a.auto_approves(&crate::ipc::Classification::Command, true),
            "with sudo=true a Command + sudo auto-approves"
        );
    }

    #[test]
    fn a_remote_host_is_not_covered_by_the_local_session_grant() {
        let a = SessionApproval::from_config(&ApprovalsConfig::default());
        assert!(
            !a.auto_approves(&remote("pinky"), false),
            "regular=true on the daemon host must not auto-approve a RemoteShell on pinky"
        );
    }

    #[test]
    fn approving_a_host_covers_that_host() {
        let mut a = SessionApproval::from_config(&ApprovalsConfig::default());
        a.record_session_approval(&remote("pinky"), false);
        assert!(
            a.auto_approves(&remote("pinky"), false),
            "recording a session grant for pinky must auto-approve a non-sudo command on pinky"
        );
    }

    #[test]
    fn approving_one_host_does_not_cover_another() {
        let mut a = SessionApproval::from_config(&ApprovalsConfig::default());
        a.record_session_approval(&remote("pinky"), false);
        assert!(
            !a.auto_approves(&remote("web01"), false),
            "a grant for pinky must not cover web01 - the grant is per host"
        );
    }

    #[test]
    fn a_remote_sudo_command_is_not_covered_by_a_remote_non_sudo_grant() {
        let mut a = SessionApproval::from_config(&ApprovalsConfig::default());
        a.record_session_approval(&remote("pinky"), false);
        assert!(
            !a.auto_approves(&remote("pinky"), true),
            "a pinky non-sudo grant must not cover a sudo command on pinky - the class dimension survives the host dimension"
        );
    }

    #[test]
    fn a_new_host_never_auto_approves() {
        let mut a = SessionApproval::from_config(&approvals_all());
        a.hosts_regular.insert("nope".to_string());
        a.hosts_sudo.insert("nope".to_string());
        for is_sudo in [false, true] {
            assert!(
                !a.auto_approves(
                    &crate::ipc::Classification::NewHost {
                        host: "nope".to_string()
                    },
                    is_sudo
                ),
                "a NewHost must be asked whatever the session holds (is_sudo={is_sudo})"
            );
        }
    }

    #[test]
    fn an_escape_hatch_never_auto_approves() {
        let mut a = SessionApproval::from_config(&approvals_all());
        a.hosts_regular.insert("anywhere".to_string());
        a.hosts_sudo.insert("anywhere".to_string());
        for is_sudo in [false, true] {
            assert!(
                !a.auto_approves(&crate::ipc::Classification::EscapeHatch, is_sudo),
                "an EscapeHatch must be asked whatever the session holds (is_sudo={is_sudo})"
            );
        }
    }

    #[test]
    fn approving_for_session_on_a_remote_host_records_only_that_host() {
        let mut a = SessionApproval::from_config(&approvals_none());
        let recorded = a.record_session_approval(&remote("pinky"), false);
        assert_eq!(
            recorded.as_deref(),
            Some("pinky (commands)"),
            "the recorded label names the host that was widened"
        );
        assert!(
            a.hosts_regular.contains("pinky"),
            "the A answer must land in hosts_regular for pinky"
        );
        assert!(
            !a.regular,
            "a remote grant must not widen the daemon-host flag"
        );
        assert!(a.hosts_sudo.is_empty());
        assert!(
            a.auto_approves(&remote("pinky"), false),
            "after recording, pinky non-sudo auto-approves"
        );
    }

    #[test]
    fn recording_a_sudo_grant_lands_in_hosts_sudo() {
        let mut a = SessionApproval::from_config(&ApprovalsConfig::default());
        let recorded = a.record_session_approval(&remote("pinky"), true);
        assert_eq!(
            recorded.as_deref(),
            Some("pinky (sudo)"),
            "the recorded label names the class that was widened"
        );
        assert!(a.hosts_sudo.contains("pinky"));
        assert!(!a.hosts_regular.contains("pinky"));
        assert!(
            !a.sudo,
            "a remote sudo grant must not widen the daemon-host sudo flag"
        );
        assert!(a.auto_approves(&remote("pinky"), true));
    }

    #[test]
    fn recording_on_a_new_host_and_escape_hatch_records_nothing() {
        let mut a = SessionApproval::from_config(&approvals_all());
        for class in [
            crate::ipc::Classification::NewHost {
                host: "nope".to_string(),
            },
            crate::ipc::Classification::EscapeHatch,
        ] {
            assert!(
                a.record_session_approval(&class, false).is_none(),
                "a class that can never auto-approve must record nothing"
            );
            assert!(a.hosts_regular.is_empty());
            assert!(a.hosts_sudo.is_empty());
        }
    }

    #[test]
    fn host_approval_lines_list_each_granted_host_with_its_scope() {
        let mut a = SessionApproval::default();
        assert!(
            a.host_approval_lines().is_empty(),
            "no grants, no host lines"
        );
        a.record_session_approval(&remote("web01"), false);
        a.record_session_approval(&remote("pinky"), true);
        a.record_session_approval(&remote("pinky"), false);
        let lines = a.host_approval_lines();
        assert_eq!(
            lines,
            vec![
                "  pinky    : commands + sudo".to_string(),
                "  web01    : commands".to_string()
            ],
            "hosts sort by name and carry the scopes they were granted"
        );
    }

    #[test]
    fn hint_shows_per_name_count_when_all_flag_false() {
        let mut a = SessionApproval::default();
        a.scripts.insert("foo.sh".to_string());
        a.scripts.insert("bar.sh".to_string());
        let h = a.hint();
        assert!(
            h.contains("scripts: 2"),
            "hint='{}' should contain 'scripts: 2'",
            h
        );
    }
}

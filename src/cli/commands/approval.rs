//! Per-session auto-approval state for tool calls.

use std::collections::HashSet;

/// Per-session auto-approval state.
///
/// - `regular` / `sudo`: class-wide approval for terminal commands.
/// - `scripts_all` / `runbooks_all` / `file_edits_all`: class-wide approval seeded from
///   `config.toml [approvals]`; bypasses the per-name `HashSet` check entirely.
/// - `scripts` / `runbooks` / `file_edits`: name/path-scoped approval; each entry
///   auto-approves future writes to that specific artifact for the rest of the session.
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
        }
    }
}

impl SessionApproval {
    /// Build initial approval state from `config.toml [approvals]` settings.
    /// Called at session start (chat, ask, and in-session resets via /clear etc.).
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
        let mut a = SessionApproval::default();
        a.scripts_all = true;
        let h = a.hint();
        assert!(
            h.contains("scripts: all"),
            "hint='{}' should contain 'scripts: all'",
            h
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

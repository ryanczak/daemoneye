//! Per-session auto-approval state for tool calls.

use std::collections::HashSet;

/// A write or delete prompt the client answers, and the name it would
/// record for the session when approved with `A`. Built from the wire
/// frames in the prompt arms; the auto-approve question lives on
/// [`SessionApproval::prompt_auto_approves`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PromptKind<'a> {
    /// `write_script`.
    ScriptWrite(&'a str),
    /// `write_runbook`.
    RunbookWrite(&'a str),
    /// `create_agent` — the agent config write.
    AgentWrite(&'a str),
    /// `schedule_command`.
    ScheduleWrite,
    /// `delete_script`.
    ScriptDelete(&'a str),
    /// `delete_runbook`.
    RunbookDelete(&'a str),
    /// `delete_agent`.
    AgentDelete(&'a str),
}

/// Per-session auto-approval state.
///
/// - `regular` / `sudo`: class-wide approval for terminal commands **on the
///   daemon host**; a session grant on the daemon host never covers a remote
///   one (and vice versa).
/// - `scripts_all` / `runbooks_all` / `file_edits_all`: class-wide approval seeded from
///   `config.toml [approvals]`; bypasses the per-name `HashSet` check entirely.
/// - `scripts` / `runbooks` / `file_edits` / `agents`: name/path-scoped approval; each entry
///   auto-approves future writes to that specific artifact for the rest of the session.
/// - `schedules` / `deletes`: whole-class approval, `off` by default and not
///   config-seeded — this session only.
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
    /// Every `schedule_command` write pre-approved for this session.
    pub(super) schedules: bool,
    /// Every `create_agent` write pre-approved for this session.
    pub(super) agents_all: bool,
    /// Agents whose config writes were approved for the session via `A`.
    pub(super) agents: HashSet<String>,
    /// Every delete (`delete_script`, `delete_runbook`, `delete_agent`)
    /// pre-approved for this session.
    pub(super) deletes: bool,
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
            schedules: false,
            agents_all: false,
            agents: HashSet::new(),
            deletes: false,
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
            // An unconfigured destination, a ghost shell reaching outside
            // its sandbox, and the user's own shell are always asked.
            crate::ipc::Classification::NewHost { .. }
            | crate::ipc::Classification::EscapeHatch
            | crate::ipc::Classification::UserShell { .. } => false,
        }
    }

    /// The kinds of write/delete prompt the client answers, and the one
    /// auto-approve question each has: does this session already hold the
    /// approval that kind needs to skip its prompt? Schedule writes have
    /// no per-name state; the whole category is the only grant.
    pub(super) fn prompt_auto_approves(&self, kind: &PromptKind<'_>) -> bool {
        match kind {
            PromptKind::ScriptWrite(name) => self.scripts_all || self.scripts.contains(*name),
            PromptKind::RunbookWrite(name) => self.runbooks_all || self.runbooks.contains(*name),
            PromptKind::AgentWrite(name) => self.agents_all || self.agents.contains(*name),
            PromptKind::ScheduleWrite => self.schedules,
            PromptKind::ScriptDelete(_)
            | PromptKind::RunbookDelete(_)
            | PromptKind::AgentDelete(_) => self.deletes,
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
            | crate::ipc::Classification::EscapeHatch
            | crate::ipc::Classification::UserShell { .. } => None,
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
        if self.schedules {
            active.push("schedules: all".to_string());
        }
        if self.agents_all {
            active.push("agents: all".to_string());
        } else if !self.agents.is_empty() {
            active.push(format!("agents: {}", self.agents.len()));
        }
        if self.deletes {
            active.push("deletes: all".to_string());
        }
        if active.is_empty() {
            "cmds: auto".to_string()
        } else {
            format!("⚡approvals: {} · Ctrl+C revokes", active.join(", "))
        }
    }
}

/// One line of `/approvals`'s list body: `"<label> : <state>"` for the eight
/// categories, a per-host row for each granted host, and the always-asked
/// block. `cmd_approvals` renders it as-is.
pub(super) fn approvals_list_body(a: &SessionApproval) -> Vec<String> {
    let row = |label: &str, state: String| format!("{label:<9} : {state}");
    let mut body = vec![
        row("commands", on_off(a.regular).to_string()),
        row("sudo", on_off(a.sudo).to_string()),
        row("scripts", scope(a.scripts_all, a.scripts.len())),
        row("runbooks", scope(a.runbooks_all, a.runbooks.len())),
        row("files", scope(a.file_edits_all, a.file_edits.len())),
        row("schedules", on_off(a.schedules).to_string()),
        row("agents", scope(a.agents_all, a.agents.len())),
        row("deletes", on_off(a.deletes).to_string()),
    ];
    let hosts = a.host_approval_lines();
    if !hosts.is_empty() {
        body.push(String::new());
        body.push("per-host commands:".to_string());
        body.extend(hosts);
    }
    body.push(String::new());
    body.push("always asked (nothing can auto-approve):".to_string());
    for kind in [
        "a host [hosts] does not configure",
        "a ghost reaching outside its sandbox",
        "your own /shell shells",
        "a sudo file edit",
    ] {
        body.push(format!("  · {kind}"));
    }
    body.push(String::new());
    body.push("change with: /approvals <category> on|off · all on|off · revoke".to_string());
    body
}

fn on_off(b: bool) -> &'static str {
    if b { "auto" } else { "gated" }
}

fn scope(all: bool, count: usize) -> String {
    if all {
        "auto".to_string()
    } else if count > 0 {
        format!("{count} approved")
    } else {
        "gated".to_string()
    }
}

/// What one `/approvals <args>` invocation did. The caller renders it;
/// the state change has already happened on `Note` and `Error` carries
/// the untouched-state guarantee.
#[derive(Debug, PartialEq)]
pub(super) enum ApprovalCmdOutcome {
    /// A panel body (the list).
    Panel(Vec<String>),
    /// One confirmation line (starts with ✓).
    Note(String),
    /// One refusal line (starts with ✗); the state is untouched.
    Error(String),
}

/// The eight categories, in the order `/approvals` lists them.
const CATEGORIES: [&str; 8] = [
    "commands",
    "sudo",
    "scripts",
    "runbooks",
    "files",
    "schedules",
    "agents",
    "deletes",
];

/// The `/approvals` grammar and its state changes, over the session state
/// alone — no renderer, so tests drive it directly and `cmd_approvals`
/// only renders the outcome. `args` is everything after `/approvals `.
pub(super) fn approvals_cmd(a: &mut SessionApproval, args: &str) -> ApprovalCmdOutcome {
    let args = args.trim();
    match args {
        "" | "list" | "show" => ApprovalCmdOutcome::Panel(approvals_list_body(a)),
        "on" | "auto" => {
            a.regular = true;
            ApprovalCmdOutcome::Note("✓ commands auto-approved for this session".to_string())
        }
        "off" | "manual" | "gated" => {
            a.regular = false;
            ApprovalCmdOutcome::Note("✓ commands now require approval".to_string())
        }
        "all on" => {
            a.regular = true;
            a.scripts_all = true;
            a.runbooks_all = true;
            a.file_edits_all = true;
            a.schedules = true;
            a.agents_all = true;
            a.deletes = true;
            ApprovalCmdOutcome::Note(
                "✓ every category auto-approved for this session — sudo stays as it was"
                    .to_string(),
            )
        }
        "all off" | "revoke" | "reset" => {
            *a = SessionApproval {
                regular: false,
                ..SessionApproval::default()
            };
            ApprovalCmdOutcome::Note("✓ all session approvals revoked".to_string())
        }
        other => {
            let words: Vec<&str> = other.split_whitespace().collect();
            let verb = words.first().copied().unwrap_or_default();
            if matches!(verb, "revoke" | "reset") {
                return match words.get(1) {
                    Some(category) if CATEGORIES.contains(category) => {
                        gate_category(a, category);
                        ApprovalCmdOutcome::Note(format!(
                            "✓ {category} now require{} approval",
                            if *category == "sudo" { "s" } else { "" }
                        ))
                    }
                    Some(unknown) => {
                        ApprovalCmdOutcome::Error(unknown_category(&format!("revoke {unknown}")))
                    }
                    None => ApprovalCmdOutcome::Error(format!(
                        "✗ unknown: /approvals {other} — valid forms: /approvals <category> on|off · all on|off · revoke [category] · host <name> [sudo] on|off ({CATEGORY_LIST})"
                    )),
                };
            }
            if verb == "host" {
                return match (words.get(1), words.get(2), words.get(3)) {
                    (Some(name), Some(&"sudo"), Some(state)) => host_switch(
                        &mut a.hosts_sudo,
                        name,
                        state,
                        &format!("sudo commands on {name}"),
                        other,
                    ),
                    (Some(name), Some(state), None) => host_switch(
                        &mut a.hosts_regular,
                        name,
                        state,
                        &format!("commands on {name}"),
                        other,
                    ),
                    _ => ApprovalCmdOutcome::Error(unknown_category(other)),
                };
            }
            if words.len() == 2 && matches!(words[1], "on" | "off") && CATEGORIES.contains(&verb) {
                let on = words[1] == "on";
                set_category(a, verb, on);
                return ApprovalCmdOutcome::Note(if on {
                    format!("✓ {verb} auto-approved for this session")
                } else if verb == "sudo" {
                    "✓ sudo now requires approval".to_string()
                } else {
                    format!("✓ {verb} now require approval")
                });
            }
            ApprovalCmdOutcome::Error(unknown_category(other))
        }
    }
}

fn set_category(a: &mut SessionApproval, category: &str, on: bool) {
    match category {
        "commands" => a.regular = on,
        "sudo" => a.sudo = on,
        "scripts" => {
            a.scripts_all = on;
            if !on {
                a.scripts.clear();
            }
        }
        "runbooks" => {
            a.runbooks_all = on;
            if !on {
                a.runbooks.clear();
            }
        }
        "files" => {
            a.file_edits_all = on;
            if !on {
                a.file_edits.clear();
            }
        }
        "schedules" => a.schedules = on,
        "agents" => {
            a.agents_all = on;
            if !on {
                a.agents.clear();
            }
        }
        "deletes" => a.deletes = on,
        _ => unreachable!("CATEGORIES is the only source of names"),
    }
}

fn gate_category(a: &mut SessionApproval, category: &str) {
    set_category(a, category, false);
}

fn host_switch(
    set: &mut HashSet<String>,
    name: &str,
    state: &str,
    what: &str,
    original: &str,
) -> ApprovalCmdOutcome {
    match state {
        "on" => {
            set.insert(name.to_string());
            ApprovalCmdOutcome::Note(format!("✓ {what} auto-approved for this session"))
        }
        "off" => {
            set.remove(name);
            ApprovalCmdOutcome::Note(format!("✓ {what} now require approval"))
        }
        _ => ApprovalCmdOutcome::Error(unknown_category(original)),
    }
}

/// The one error line every rejected `/approvals` argument gets: it names
/// the valid categories and the forms. An always-asked kind (e.g. a host
/// name) is not a category and gets the same refusal — nothing can enable it.
fn unknown_category(args: &str) -> String {
    format!(
        "✗ unknown: /approvals {args} — valid categories: {CATEGORY_LIST}; forms: /approvals <category> on|off · all on|off · revoke [category] · host <name> [sudo] on|off"
    )
}

const CATEGORY_LIST: &str = "commands, sudo, scripts, runbooks, files, schedules, agents, deletes";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ApprovalsConfig;
    use crate::ipc::Response;

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
    fn user_shell_is_always_asked() {
        let mut a = SessionApproval::from_config(&approvals_all());
        let class = crate::ipc::Classification::UserShell { shell: "s7".into() };
        assert!(
            !a.auto_approves(&class, false) && !a.auto_approves(&class, true),
            "every grant on: the user's shell is still asked"
        );
        assert_eq!(
            a.record_session_approval(&class, false),
            None,
            "[A] widens nothing for the user's shell"
        );
        assert!(!a.auto_approves(&class, false));
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
            schedules: false,
            agents_all: false,
            agents: HashSet::new(),
            deletes: false,
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

    // ── the /approvals command (M36 phase-03) ────────────────────────────────

    fn set_all(a: &mut SessionApproval) {
        a.regular = true;
        a.sudo = true;
        a.scripts_all = true;
        a.runbooks_all = true;
        a.file_edits_all = true;
        a.schedules = true;
        a.agents_all = true;
        a.deletes = true;
    }

    fn list_states(a: &SessionApproval) -> Vec<(String, String)> {
        let ApprovalCmdOutcome::Panel(lines) = approvals_cmd(&mut a.clone(), "list") else {
            panic!("list must return a panel");
        };
        lines
            .iter()
            .filter_map(|l| {
                let (name, state) = l.split_once(" : ")?;
                Some((name.trim().to_string(), state.to_string()))
            })
            .collect()
    }

    fn state_of(a: &SessionApproval, category: &str) -> String {
        list_states(a)
            .into_iter()
            .find(|(n, _)| n == category)
            .map(|(_, s)| s)
            .unwrap_or_else(|| format!("no '{category}' row in the list"))
    }

    fn assert_state(a: &SessionApproval, category: &str, want: &str) {
        assert_eq!(
            state_of(a, category),
            want,
            "'{category}' must read '{want}'"
        );
    }

    fn apply(a: &mut SessionApproval, args: &str) -> String {
        match approvals_cmd(a, args) {
            ApprovalCmdOutcome::Note(n) => n,
            other => panic!("'{args}' must be a note, got {other:?}"),
        }
    }

    fn apply_err(a: &mut SessionApproval, args: &str) -> String {
        match approvals_cmd(a, args) {
            ApprovalCmdOutcome::Error(e) => e,
            other => panic!("'{args}' must be an error, got {other:?}"),
        }
    }

    #[test]
    fn approvals_cmd_lists_every_category_and_the_always_asked_kinds() {
        let a = SessionApproval::default();
        let lines = match approvals_cmd(&mut a.clone(), "list") {
            ApprovalCmdOutcome::Panel(lines) => lines,
            other => panic!("list returns a panel, got {other:?}"),
        };
        let want_order = [
            "commands",
            "sudo",
            "scripts",
            "runbooks",
            "files",
            "schedules",
            "agents",
            "deletes",
        ];
        let mut found: Vec<&str> = Vec::new();
        for l in &lines {
            for name in want_order {
                if l.starts_with(&format!("{name} ")) && !found.contains(&name) {
                    found.push(name);
                }
            }
        }
        assert_eq!(
            found, want_order,
            "the list carries the eight categories in the table's order: {lines:?}"
        );
        assert!(
            !lines.iter().any(|l| l.contains("per-host")),
            "no host grants, no per-host block: {lines:?}"
        );
        let asked = lines
            .iter()
            .position(|l| l.contains("always asked"))
            .unwrap_or_else(|| panic!("the list names the always-asked kinds: {lines:?}"));
        let after: Vec<String> = lines[asked..].to_vec();
        for kind in [
            "host [hosts] does not configure",
            "ghost reaching outside its sandbox",
            "own /shell shells",
            "sudo file edit",
        ] {
            assert!(
                after.iter().any(|l| l.contains(kind)),
                "always-asked names {kind:?}: {after:?}"
            );
        }
        assert!(
            lines
                .last()
                .is_some_and(|l| l.contains("/approvals") && l.contains("on|off")),
            "the list ends with a usage reminder: {lines:?}"
        );
    }

    #[test]
    fn approvals_cmd_category_on_off_toggles_only_that_category() {
        for (category, seed) in [
            ("commands", None),
            ("sudo", None),
            ("scripts", Some("s1.sh")),
            ("runbooks", Some("r1")),
            ("files", Some("/tmp/f")),
            ("schedules", None),
            ("agents", Some("a1")),
            ("deletes", None),
        ] {
            let mut a = SessionApproval::default();
            if let Some(name) = seed {
                match category {
                    "scripts" => _ = a.scripts.insert(name.to_string()),
                    "runbooks" => _ = a.runbooks.insert(name.to_string()),
                    "files" => _ = a.file_edits.insert(name.to_string()),
                    "agents" => _ = a.agents.insert(name.to_string()),
                    _ => panic!("only name-holding categories seed a name"),
                }
            }
            let on = apply(&mut a, &format!("{category} on"));
            assert!(on.contains("✓"), "'{category} on' confirms: {on}");
            assert_state(&a, category, "auto");
            let off = apply(&mut a, &format!("{category} off"));
            assert!(off.contains("✓"), "'{category} off' confirms: {off}");
            assert_state(&a, category, "gated");
            if category == "sudo" {
                assert!(
                    off.contains("now requires approval"),
                    "sudo is the one singular category: {off}"
                );
            } else {
                assert!(
                    off.contains("now require approval") && !off.contains("requires"),
                    "every category but sudo is plural: {off}"
                );
            }
            if seed.is_some() {
                let names_empty = match category {
                    "scripts" => a.scripts.is_empty(),
                    "runbooks" => a.runbooks.is_empty(),
                    "files" => a.file_edits.is_empty(),
                    "agents" => a.agents.is_empty(),
                    _ => panic!("seeded categories hold names"),
                };
                assert!(
                    names_empty,
                    "'{category} off' forgets the names approved for the session"
                );
            }
        }
        // off on one category leaves the others alone.
        let mut a = SessionApproval::default();
        set_all(&mut a);
        apply(&mut a, "scripts off");
        assert_state(&a, "scripts", "gated");
        for untouched in [
            "commands",
            "sudo",
            "runbooks",
            "files",
            "schedules",
            "agents",
            "deletes",
        ] {
            assert_state(&a, untouched, "auto");
        }
    }

    #[test]
    fn approvals_cmd_all_on_spares_sudo_and_all_off_clears_everything() {
        let mut a = SessionApproval::default();
        let note = apply(&mut a, "all on");
        assert!(
            note.contains("sudo"),
            "'all on' says sudo stays as it was: {note}"
        );
        for category in [
            "commands",
            "scripts",
            "runbooks",
            "files",
            "schedules",
            "agents",
            "deletes",
        ] {
            assert_state(&a, category, "auto");
        }
        assert_state(&a, "sudo", "gated");
        assert!(!a.sudo, "'all on' never touches the sudo flag");

        a.sudo = true;
        a.record_session_approval(&remote("pinky"), false);
        a.record_session_approval(&remote("web01"), true);
        let note = apply(&mut a, "all off");
        assert!(note.contains("✓"), "'all off' confirms: {note}");
        for category in [
            "commands",
            "sudo",
            "scripts",
            "runbooks",
            "files",
            "schedules",
            "agents",
            "deletes",
        ] {
            assert_state(&a, category, "gated");
        }
        assert!(
            a.hosts_regular.is_empty() && a.hosts_sudo.is_empty(),
            "'all off' clears the per-host grants too"
        );
    }

    #[test]
    fn approvals_cmd_host_grants_on_off() {
        let mut a = SessionApproval::default();
        apply(&mut a, "host pinky on");
        assert!(
            a.hosts_regular.contains("pinky"),
            "'host pinky on' grants pinky's commands"
        );
        assert!(!a.hosts_sudo.contains("pinky"));
        assert!(
            a.auto_approves(&remote("pinky"), false),
            "the grant auto-approves a non-sudo command on pinky"
        );
        assert!(
            !a.auto_approves(&remote("pinky"), true),
            "the commands grant does not cover pinky's sudo commands"
        );
        let hosts_row = {
            let ApprovalCmdOutcome::Panel(lines) = approvals_cmd(&mut a.clone(), "list") else {
                panic!("list returns a panel");
            };
            lines
                .iter()
                .find(|l| l.contains("pinky"))
                .cloned()
                .unwrap_or_else(|| panic!("the list shows the pinky grant: {lines:?}"))
        };
        assert!(
            hosts_row.contains("commands") && !hosts_row.contains("sudo"),
            "the pinky row reads commands: {hosts_row}"
        );

        apply(&mut a, "host pinky sudo on");
        assert!(a.hosts_sudo.contains("pinky"));
        assert!(a.auto_approves(&remote("pinky"), true));
        let ApprovalCmdOutcome::Panel(lines) = approvals_cmd(&mut a.clone(), "list") else {
            panic!("list returns a panel");
        };
        let row = lines
            .iter()
            .find(|l| l.contains("pinky"))
            .expect("the pinky row still shows");
        assert!(
            row.contains("commands + sudo"),
            "both grants show on one row: {row}"
        );

        apply(&mut a, "host pinky sudo off");
        assert!(
            !a.hosts_sudo.contains("pinky"),
            "'host pinky sudo off' clears the sudo grant"
        );
        apply(&mut a, "host pinky off");
        assert!(
            !a.hosts_regular.contains("pinky"),
            "'host pinky off' clears the commands grant"
        );
        let ApprovalCmdOutcome::Panel(lines) = approvals_cmd(&mut a.clone(), "list") else {
            panic!("list returns a panel");
        };
        assert!(
            !lines.iter().any(|l| l.contains("per-host")),
            "no grants left, no per-host block: {lines:?}"
        );
    }

    #[test]
    fn approvals_cmd_revoke_category_and_legacy_on_off() {
        let mut a = SessionApproval::default();
        set_all(&mut a);
        a.scripts.insert("keep.sh".to_string());
        apply(&mut a, "revoke scripts");
        assert_state(&a, "scripts", "gated");
        assert!(
            a.scripts.is_empty(),
            "'revoke scripts' forgets the names too"
        );
        assert_state(&a, "runbooks", "auto");

        apply(&mut a, "reset");
        for category in [
            "commands",
            "sudo",
            "scripts",
            "runbooks",
            "files",
            "schedules",
            "agents",
            "deletes",
        ] {
            assert_state(&a, category, "gated");
        }

        apply(&mut a, "on");
        assert_state(&a, "commands", "auto");
        assert_state(&a, "sudo", "gated");
        apply(&mut a, "auto");
        assert_state(&a, "commands", "auto");
        apply(&mut a, "off");
        assert_state(&a, "commands", "gated");
        apply(&mut a, "manual");
        assert_state(&a, "commands", "gated");
    }

    #[test]
    fn approvals_cmd_rejects_unknown_and_always_asked_names() {
        for args in [
            "newhost on",
            "foo off",
            "scripts maybe",
            "host",
            "host pinky",
            "host pinky sudo",
        ] {
            let mut a = SessionApproval::default();
            set_all(&mut a);
            a.scripts.insert("keep.sh".to_string());
            a.agents.insert("keep-agent".to_string());
            let err = apply_err(&mut a, args);
            assert!(err.contains('✗'), "the error line starts ✗: {err}");
            assert!(
                err.contains("commands"),
                "the error names the valid categories: {err}"
            );
            let expected = format!("/approvals {args}");
            assert!(
                err.matches(&expected).count() == 1,
                "the refusal quotes the argument exactly once: {err}"
            );
            assert!(
                !err.contains("host host"),
                "the host form's refusal never doubles 'host': {err}"
            );
            let ApprovalCmdOutcome::Panel(lines) = approvals_cmd(&mut a.clone(), "list") else {
                panic!("list returns a panel");
            };
            assert!(
                lines.iter().any(|l| l.contains("auto")),
                "state unchanged after a rejected '{args}': {lines:?}"
            );
            assert!(
                a.scripts.contains("keep.sh") && a.agents.contains("keep-agent"),
                "a rejected '{args}' forgets nothing"
            );
        }
    }

    #[test]
    fn approvals_cmd_new_categories_auto_approve_their_prompts() {
        let mut a = SessionApproval::default();
        assert!(
            !a.prompt_auto_approves(&PromptKind::ScheduleWrite),
            "schedules default to gated"
        );
        assert!(
            !a.prompt_auto_approves(&PromptKind::AgentWrite("a1")),
            "agent writes default to gated"
        );
        assert!(
            !a.prompt_auto_approves(&PromptKind::ScriptDelete("s1")),
            "deletes default to gated"
        );
        a.schedules = true;
        assert!(
            a.prompt_auto_approves(&PromptKind::ScheduleWrite),
            "'schedules on' covers every schedule write"
        );
        a.deletes = true;
        for kind in [
            PromptKind::ScriptDelete("s1"),
            PromptKind::RunbookDelete("r1"),
            PromptKind::AgentDelete("a1"),
        ] {
            assert!(
                a.prompt_auto_approves(&kind),
                "'deletes on' covers {kind:?}"
            );
        }

        // Categories do not cross: a script write is never covered by agents,
        // an agent write never by scripts, and a delete is not a write.
        a.agents_all = true;
        assert!(
            !a.prompt_auto_approves(&PromptKind::ScriptWrite("s1")),
            "agents does not cover a script write"
        );
        a.deletes = false;
        a.agents_all = false;
        a.scripts_all = true;
        assert!(
            !a.prompt_auto_approves(&PromptKind::AgentWrite("a1")),
            "scripts does not cover an agent write"
        );
        assert!(
            !a.prompt_auto_approves(&PromptKind::RunbookWrite("r1")),
            "scripts does not cover a runbook write"
        );

        // A per-name agent grant works like scripts': A records the name.
        let mut a = SessionApproval::default();
        assert!(!a.prompt_auto_approves(&PromptKind::AgentWrite("a1")));
        a.record_session_approval(&remote("unused"), false);
        assert!(
            a.agents.is_empty(),
            "a command A records nothing under agents"
        );
        a.agents.insert("a1".to_string());
        assert!(
            a.prompt_auto_approves(&PromptKind::AgentWrite("a1")),
            "the agent's name approved for the session covers its next write"
        );
        assert!(
            !a.prompt_auto_approves(&PromptKind::AgentWrite("a2")),
            "the name grant covers only that agent"
        );
    }

    #[test]
    fn approvals_cmd_agent_flag_round_trips() {
        let resp = Response::ScriptWritePrompt {
            id: "aw_1".to_string(),
            script_name: "agents/deploy".to_string(),
            content: "name = 'deploy'".to_string(),
            existing_content: None,
            agent: true,
        };
        let json = serde_json::to_string(&resp).expect("serialize");
        assert!(
            json.contains("\"agent\":true"),
            "an agent write serialises agent: true: {json}"
        );
        let parsed: Response = serde_json::from_str(&json).expect("deserialize");
        match parsed {
            Response::ScriptWritePrompt { agent, .. } => {
                assert!(agent, "the agent flag survives the round trip");
            }
            _ => panic!("wrong variant"),
        }

        let del = Response::ScriptDeletePrompt {
            id: "ad_1".to_string(),
            script_name: "agent: deploy".to_string(),
            agent: true,
        };
        let json = serde_json::to_string(&del).expect("serialize");
        assert!(
            json.contains("\"agent\":true"),
            "an agent delete serialises agent: true: {json}"
        );

        // An older daemon's frame has no agent field; it deserialises as a
        // script prompt with agent false.
        let old = r##"{"ScriptWritePrompt":{"id":"s_1","script_name":"check.sh","content":"#!/bin/sh"}}"##;
        let parsed: Response = serde_json::from_str(old).expect("old frame deserialises");
        match parsed {
            Response::ScriptWritePrompt { agent, .. } => assert!(!agent),
            _ => panic!("wrong variant"),
        }
        let old_del = r#"{"ScriptDeletePrompt":{"id":"d_1","script_name":"check.sh"}}"#;
        let parsed: Response = serde_json::from_str(old_del).expect("old frame deserialises");
        match parsed {
            Response::ScriptDeletePrompt { agent, .. } => assert!(!agent),
            _ => panic!("wrong variant"),
        }
    }
}

use crate::daemon::utils::is_read_only_command;

/// Security policy for autonomous Ghost Sessions.
///
/// Determines whether a command is "safe" to auto-approve in a headless session
/// where no human user is present to provide manual approval.
#[derive(Debug, Clone)]
pub struct GhostPolicy {
    /// List of exact script names (e.g. `restart-nginx.sh`) pre-approved for execution.
    /// Scripts must exist in `~/.daemoneye/scripts/`.
    pub auto_approve_scripts: Vec<String>,
    /// Whether to auto-approve known read-only informational commands.
    pub auto_approve_read_only: bool,
}

impl GhostPolicy {
    /// Create a policy from the ghost configuration inherited from a runbook.
    pub fn from_config(config: &crate::ipc::GhostConfig) -> Self {
        Self {
            auto_approve_scripts: config.auto_approve_scripts.clone(),
            auto_approve_read_only: config.auto_approve_read_only,
        }
    }

    /// Returns true if the command is considered "safe" to run autonomously
    /// based on the policy rules.
    pub fn is_safe(&self, command: &str) -> bool {
        // 1. Check script whitelist (exact name matches)
        // We match against the script filename (e.g. "my-script.sh").
        if self.auto_approve_scripts.iter().any(|s| {
            command == s || command.ends_with(&format!("/{}", s))
        }) {
            return true;
        }

        // 2. Check read-only heuristic
        if self.auto_approve_read_only && is_read_only_command(command) {
            return true;
        }

        false
    }
}

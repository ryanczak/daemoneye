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
    /// Whether to prepend `sudo` when executing pre-approved scripts.
    pub run_with_sudo: bool,
}

impl GhostPolicy {
    /// Create a policy from the ghost configuration inherited from a runbook.
    pub fn from_config(config: &crate::ipc::GhostConfig) -> Self {
        Self {
            auto_approve_scripts: config.auto_approve_scripts.clone(),
            auto_approve_read_only: config.auto_approve_read_only,
            run_with_sudo: config.run_with_sudo,
        }
    }

    /// Returns true if the command is considered "safe" to run autonomously
    /// based on the policy rules.
    pub fn is_safe(&self, command: &str) -> bool {
        // 1. Check script whitelist — match by exact name or trailing path component.
        // Skip a leading `sudo` token so that commands rewritten by resolve_command()
        // with run_with_sudo=true ("sudo /path/to/script.sh") are still matched
        // against the whitelist by their script basename rather than "sudo".
        let mut tokens = command.split_whitespace();
        let first_token = tokens.next().unwrap_or("");
        let effective_token = if first_token == "sudo" {
            tokens.next().unwrap_or("")
        } else {
            first_token
        };
        let basename = std::path::Path::new(effective_token)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(effective_token);
        if self.auto_approve_scripts.iter().any(|s| s == basename) {
            return true;
        }

        // 2. Check read-only heuristic
        if self.auto_approve_read_only && is_read_only_command(command) {
            return true;
        }

        false
    }

    /// Rewrite a whitelisted script reference to its absolute path in
    /// `~/.daemoneye/scripts/` so it executes correctly regardless of the
    /// background pane's working directory.
    ///
    /// Handles bare names (`script.sh`), relative paths (`./script.sh`), and
    /// commands with arguments (`./script.sh arg1 arg2`).  Commands whose
    /// first token is already absolute are returned unchanged.
    pub fn resolve_command(&self, cmd: &str) -> String {
        let mut parts = cmd.splitn(2, |c: char| c.is_whitespace());
        let first = match parts.next() {
            Some(t) if !t.is_empty() => t,
            _ => return cmd.to_string(),
        };
        let rest = parts.next().map(|s| format!(" {}", s)).unwrap_or_default();

        // Already absolute — trust it as-is.
        if first.starts_with('/') {
            return cmd.to_string();
        }

        let basename = std::path::Path::new(first)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(first);

        if self.auto_approve_scripts.iter().any(|s| s == basename) {
            let full_path = crate::scripts::scripts_dir().join(basename);
            return if self.run_with_sudo {
                format!("sudo {}{}", full_path.display(), rest)
            } else {
                format!("{}{}", full_path.display(), rest)
            };
        }

        cmd.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(scripts: &[&str]) -> GhostPolicy {
        GhostPolicy {
            auto_approve_scripts: scripts.iter().map(|s| s.to_string()).collect(),
            auto_approve_read_only: false,
            run_with_sudo: false,
        }
    }

    fn sudo_policy(scripts: &[&str]) -> GhostPolicy {
        GhostPolicy {
            auto_approve_scripts: scripts.iter().map(|s| s.to_string()).collect(),
            auto_approve_read_only: false,
            run_with_sudo: true,
        }
    }

    #[test]
    fn is_safe_bare_name() {
        let p = policy(&["fix.sh"]);
        assert!(p.is_safe("fix.sh"));
    }

    #[test]
    fn is_safe_relative_path() {
        let p = policy(&["fix.sh"]);
        assert!(p.is_safe("./fix.sh"));
    }

    #[test]
    fn is_safe_relative_path_with_args() {
        let p = policy(&["fix.sh"]);
        assert!(p.is_safe("./fix.sh --dry-run"));
    }

    #[test]
    fn is_safe_absolute_path() {
        let p = policy(&["fix.sh"]);
        assert!(p.is_safe("/home/user/.daemoneye/scripts/fix.sh"));
    }

    #[test]
    fn is_safe_sudo_absolute_path() {
        let p = policy(&["fix.sh"]);
        assert!(p.is_safe("sudo /home/user/.daemoneye/scripts/fix.sh"));
    }

    #[test]
    fn is_safe_sudo_absolute_path_with_args() {
        let p = policy(&["fix.sh"]);
        assert!(p.is_safe("sudo /home/user/.daemoneye/scripts/fix.sh --flag"));
    }

    #[test]
    fn is_safe_sudo_not_on_whitelist() {
        let p = policy(&["fix.sh"]);
        assert!(!p.is_safe("sudo /home/user/.daemoneye/scripts/other.sh"));
    }

    #[test]
    fn is_safe_not_on_whitelist() {
        let p = policy(&["fix.sh"]);
        assert!(!p.is_safe("other.sh"));
        assert!(!p.is_safe("rm -rf /"));
    }

    #[test]
    fn resolve_command_bare_name() {
        let p = policy(&["fix.sh"]);
        let resolved = p.resolve_command("fix.sh");
        assert!(resolved.ends_with("/.daemoneye/scripts/fix.sh"), "got: {}", resolved);
    }

    #[test]
    fn resolve_command_relative_path() {
        let p = policy(&["fix.sh"]);
        let resolved = p.resolve_command("./fix.sh");
        assert!(resolved.ends_with("/.daemoneye/scripts/fix.sh"), "got: {}", resolved);
    }

    #[test]
    fn resolve_command_preserves_args() {
        let p = policy(&["fix.sh"]);
        let resolved = p.resolve_command("./fix.sh --dry-run --verbose");
        assert!(resolved.ends_with("/.daemoneye/scripts/fix.sh --dry-run --verbose"), "got: {}", resolved);
    }

    #[test]
    fn resolve_command_absolute_unchanged() {
        let p = policy(&["fix.sh"]);
        let cmd = "/home/user/.daemoneye/scripts/fix.sh";
        assert_eq!(p.resolve_command(cmd), cmd);
    }

    #[test]
    fn resolve_command_not_on_whitelist_unchanged() {
        let p = policy(&["fix.sh"]);
        assert_eq!(p.resolve_command("./other.sh"), "./other.sh");
    }

    #[test]
    fn resolve_command_sudo_bare_name() {
        let p = sudo_policy(&["fix.sh"]);
        let resolved = p.resolve_command("fix.sh");
        assert!(resolved.starts_with("sudo "), "got: {}", resolved);
        assert!(resolved.ends_with("/.daemoneye/scripts/fix.sh"), "got: {}", resolved);
    }

    #[test]
    fn resolve_command_sudo_relative_path() {
        let p = sudo_policy(&["fix.sh"]);
        let resolved = p.resolve_command("./fix.sh");
        assert!(resolved.starts_with("sudo "), "got: {}", resolved);
        assert!(resolved.ends_with("/.daemoneye/scripts/fix.sh"), "got: {}", resolved);
    }

    #[test]
    fn resolve_command_sudo_preserves_args() {
        let p = sudo_policy(&["fix.sh"]);
        let resolved = p.resolve_command("./fix.sh --dry-run");
        assert!(resolved.starts_with("sudo "), "got: {}", resolved);
        assert!(resolved.ends_with("/.daemoneye/scripts/fix.sh --dry-run"), "got: {}", resolved);
    }

    #[test]
    fn resolve_command_sudo_absolute_unchanged() {
        // Absolute paths bypass resolve_command entirely — no sudo prepended.
        let p = sudo_policy(&["fix.sh"]);
        let cmd = "/home/user/.daemoneye/scripts/fix.sh";
        assert_eq!(p.resolve_command(cmd), cmd);
    }

    #[test]
    fn resolve_command_no_sudo_without_flag() {
        let p = policy(&["fix.sh"]);
        let resolved = p.resolve_command("fix.sh");
        assert!(!resolved.starts_with("sudo "), "got: {}", resolved);
    }
}

use crate::daemon::utils::is_read_only_command;

/// Security policy for autonomous Ghost Shells.
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
    /// Optional SSH destination for remote execution (e.g. `user@host`).
    /// When set, `wrap_remote()` wraps every approved command in `ssh <target> <cmd>`.
    pub ssh_target: Option<String>,
}

impl GhostPolicy {
    /// Create a policy from the ghost configuration inherited from a runbook.
    pub fn from_config(config: &crate::ipc::GhostConfig) -> Self {
        Self {
            auto_approve_scripts: config.auto_approve_scripts.clone(),
            auto_approve_read_only: config.auto_approve_read_only,
            run_with_sudo: config.run_with_sudo,
            ssh_target: config.ssh_target.clone(),
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

    /// Rewrite a whitelisted script reference to its absolute (local) or
    /// tilde-prefixed (remote) path in `~/.daemoneye/scripts/` so it executes
    /// correctly regardless of the background pane's working directory.
    ///
    /// Handles bare names (`script.sh`), relative paths (`./script.sh`), commands
    /// with arguments (`./script.sh arg1 arg2`), and commands prefixed with `sudo`
    /// (`sudo script.sh`).  Commands whose first non-sudo token is already absolute
    /// are returned unchanged.
    ///
    /// When `ssh_target` is set the resolved path uses `~/.daemoneye/scripts/<name>`
    /// (tilde notation) so the remote shell expands it to the correct home directory.
    /// SSH wrapping itself is deferred to `wrap_remote()`.
    pub fn resolve_command(&self, cmd: &str) -> String {
        // Strip a leading `sudo` so `sudo script.sh` resolves identically to
        // bare `script.sh`.  The flag is re-applied at the end to avoid
        // double-sudo when run_with_sudo is also set.
        let (had_sudo, effective_cmd) = if let Some(rest) = cmd.strip_prefix("sudo ") {
            (true, rest.trim_start())
        } else {
            (false, cmd)
        };

        let mut parts = effective_cmd.splitn(2, |c: char| c.is_whitespace());
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
            // Use sudo if the original command had it OR if the policy requires it.
            let use_sudo = had_sudo || self.run_with_sudo;
            if self.ssh_target.is_some() {
                // Remote execution: use tilde path — the remote shell expands it.
                let remote_path = format!("~/.daemoneye/scripts/{}", basename);
                return if use_sudo {
                    format!("sudo {}{}", remote_path, rest)
                } else {
                    format!("{}{}", remote_path, rest)
                };
            } else {
                // Local execution: use the absolute path on this machine.
                let full_path = crate::scripts::scripts_dir().join(basename);
                return if use_sudo {
                    format!("sudo {}{}", full_path.display(), rest)
                } else {
                    format!("{}{}", full_path.display(), rest)
                };
            }
        }

        cmd.to_string()
    }

    /// Wrap an approved command for remote SSH execution when `ssh_target` is set.
    ///
    /// Called after policy approval, immediately before `run_background_in_window`.
    /// Commands that already begin with `ssh ` are returned unchanged to prevent
    /// double-wrapping if the AI emits an explicit SSH invocation despite instructions.
    pub fn wrap_remote(&self, cmd: &str) -> String {
        match &self.ssh_target {
            Some(target) if !cmd.trim_start().starts_with("ssh ") => {
                format!("ssh {} '{}'", target, cmd)
            }
            _ => cmd.to_string(),
        }
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
            ssh_target: None,
        }
    }

    fn sudo_policy(scripts: &[&str]) -> GhostPolicy {
        GhostPolicy {
            auto_approve_scripts: scripts.iter().map(|s| s.to_string()).collect(),
            auto_approve_read_only: false,
            run_with_sudo: true,
            ssh_target: None,
        }
    }

    fn remote_policy(scripts: &[&str], target: &str) -> GhostPolicy {
        GhostPolicy {
            auto_approve_scripts: scripts.iter().map(|s| s.to_string()).collect(),
            auto_approve_read_only: false,
            run_with_sudo: false,
            ssh_target: Some(target.to_string()),
        }
    }

    fn remote_sudo_policy(scripts: &[&str], target: &str) -> GhostPolicy {
        GhostPolicy {
            auto_approve_scripts: scripts.iter().map(|s| s.to_string()).collect(),
            auto_approve_read_only: false,
            run_with_sudo: true,
            ssh_target: Some(target.to_string()),
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

    // ── Remote (ssh_target) tests ────────────────────────────────────────────

    #[test]
    fn resolve_command_remote_bare_name_uses_tilde_path() {
        let p = remote_policy(&["fix.sh"], "user@zap");
        let resolved = p.resolve_command("fix.sh");
        assert_eq!(resolved, "~/.daemoneye/scripts/fix.sh", "got: {}", resolved);
    }

    #[test]
    fn resolve_command_remote_relative_path_uses_tilde_path() {
        let p = remote_policy(&["fix.sh"], "user@zap");
        let resolved = p.resolve_command("./fix.sh");
        assert_eq!(resolved, "~/.daemoneye/scripts/fix.sh", "got: {}", resolved);
    }

    #[test]
    fn resolve_command_remote_preserves_args() {
        let p = remote_policy(&["fix.sh"], "user@zap");
        let resolved = p.resolve_command("fix.sh --flag");
        assert_eq!(resolved, "~/.daemoneye/scripts/fix.sh --flag", "got: {}", resolved);
    }

    #[test]
    fn resolve_command_remote_sudo_prepends_sudo() {
        let p = remote_sudo_policy(&["fix.sh"], "user@zap");
        let resolved = p.resolve_command("fix.sh");
        assert_eq!(resolved, "sudo ~/.daemoneye/scripts/fix.sh", "got: {}", resolved);
    }

    #[test]
    fn resolve_command_remote_absolute_unchanged() {
        // Absolute paths bypass resolve_command entirely, even with ssh_target.
        let p = remote_policy(&["fix.sh"], "user@zap");
        let cmd = "/home/user/.daemoneye/scripts/fix.sh";
        assert_eq!(p.resolve_command(cmd), cmd);
    }

    #[test]
    fn wrap_remote_no_target_returns_unchanged() {
        let p = policy(&["fix.sh"]);
        assert_eq!(p.wrap_remote("fix.sh"), "fix.sh");
        assert_eq!(p.wrap_remote("ps aux"), "ps aux");
    }

    #[test]
    fn wrap_remote_wraps_script_in_ssh() {
        let p = remote_policy(&["fix.sh"], "user@zap");
        assert_eq!(
            p.wrap_remote("~/.daemoneye/scripts/fix.sh"),
            "ssh user@zap '~/.daemoneye/scripts/fix.sh'"
        );
    }

    #[test]
    fn wrap_remote_wraps_read_only_cmd_in_ssh() {
        let p = remote_policy(&[], "user@zap");
        assert_eq!(p.wrap_remote("ps aux"), "ssh user@zap 'ps aux'");
    }

    #[test]
    fn wrap_remote_no_double_wrap() {
        // If the AI (despite instructions) emits an SSH command, do not wrap again.
        let p = remote_policy(&["fix.sh"], "user@zap");
        let cmd = "ssh user@zap ~/.daemoneye/scripts/fix.sh";
        assert_eq!(p.wrap_remote(cmd), cmd);
    }

    #[test]
    fn wrap_remote_sudo_script() {
        let p = remote_sudo_policy(&["fix.sh"], "user@zap");
        // resolve_command produces "sudo ~/.daemoneye/scripts/fix.sh"
        // wrap_remote should wrap the whole thing, single-quoting to prevent local tilde expansion
        assert_eq!(
            p.wrap_remote("sudo ~/.daemoneye/scripts/fix.sh"),
            "ssh user@zap 'sudo ~/.daemoneye/scripts/fix.sh'"
        );
    }

    #[test]
    fn is_safe_tilde_path_on_whitelist() {
        // After resolve_command with ssh_target the command is a tilde path;
        // is_safe must still recognise it via basename matching.
        let p = remote_policy(&["fix.sh"], "user@zap");
        assert!(p.is_safe("~/.daemoneye/scripts/fix.sh"));
    }

    // ── sudo-prefix resolve_command tests ────────────────────────────────────

    #[test]
    fn resolve_command_sudo_prefix_bare_name_remote() {
        // AI emits `sudo script.sh` on a remote policy — must resolve to tilde path.
        let p = remote_policy(&["fix.sh"], "user@zap");
        let resolved = p.resolve_command("sudo fix.sh");
        assert_eq!(resolved, "sudo ~/.daemoneye/scripts/fix.sh", "got: {}", resolved);
    }

    #[test]
    fn resolve_command_sudo_prefix_no_double_sudo() {
        // Policy has run_with_sudo=true AND command starts with sudo — must not double-sudo.
        let p = remote_sudo_policy(&["fix.sh"], "user@zap");
        let resolved = p.resolve_command("sudo fix.sh");
        assert_eq!(resolved, "sudo ~/.daemoneye/scripts/fix.sh", "got: {}", resolved);
    }

    #[test]
    fn resolve_command_sudo_prefix_local() {
        // Local policy (no ssh_target), AI emits `sudo script.sh`.
        let p = sudo_policy(&["fix.sh"]);
        let resolved = p.resolve_command("sudo fix.sh");
        assert!(resolved.starts_with("sudo "), "got: {}", resolved);
        assert!(resolved.ends_with("/.daemoneye/scripts/fix.sh"), "got: {}", resolved);
    }

    #[test]
    fn resolve_command_sudo_prefix_with_args() {
        let p = remote_policy(&["fix.sh"], "user@zap");
        let resolved = p.resolve_command("sudo fix.sh --verbose");
        assert_eq!(resolved, "sudo ~/.daemoneye/scripts/fix.sh --verbose", "got: {}", resolved);
    }

    /// Full end-to-end: AI emits `sudo script.sh`, resolve then wrap_remote.
    #[test]
    fn resolve_then_wrap_remote_sudo_prefix() {
        let p = remote_sudo_policy(&["fix.sh"], "user@zap");
        let resolved = p.resolve_command("sudo fix.sh");
        let wrapped = p.wrap_remote(&resolved);
        assert_eq!(wrapped, "ssh user@zap 'sudo ~/.daemoneye/scripts/fix.sh'");
    }
}

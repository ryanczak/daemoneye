/// Security policy for autonomous Ghost Shells.
///
/// Trust model: non-sudo commands are always allowed — OS user permissions are the
/// boundary.  Sudo commands are only allowed when the script basename appears in
/// `auto_approve_scripts` (paired with a `/etc/sudoers.d/` NOPASSWD entry created
/// by `daemoneye install-sudoers`).  `run_with_sudo` controls whether the daemon
/// auto-prepends `sudo` when executing approved scripts — it does NOT grant broad
/// sudo access to arbitrary commands.
#[derive(Debug, Clone)]
pub struct GhostPolicy {
    /// List of exact script names (e.g. `restart-nginx.sh`) pre-approved for sudo
    /// execution.  Scripts must exist in `~/.daemoneye/scripts/`.
    pub auto_approve_scripts: Vec<String>,
    /// Whether to prepend `sudo` when executing pre-approved scripts.
    pub run_with_sudo: bool,
    /// Optional SSH destination for remote execution (e.g. `user@host`).
    /// When set, `wrap_remote()` wraps every approved command in `ssh <target> <cmd>`.
    pub ssh_target: Option<String>,
    /// When `true` the ghost shell system prompt explicitly states that non-sudo
    /// commands may be run freely.  Does not change the OS-permission boundary
    /// (non-sudo commands are always allowed regardless of this flag), but makes
    /// the permission explicit so the AI does not withhold investigation commands.
    /// Set per-runbook via `auto_approve_commands: true` in frontmatter, or
    /// daemon-wide via `[approvals] ghost_commands = true` in `config.toml`.
    /// Carried here for completeness; the system prompt reads from `GhostConfig` directly.
    #[allow(dead_code)]
    pub auto_approve_commands: bool,
}

impl GhostPolicy {
    /// Create a policy from the ghost configuration inherited from a runbook.
    pub fn from_config(config: &crate::ipc::GhostConfig) -> Self {
        Self {
            auto_approve_scripts: config.auto_approve_scripts.clone(),
            run_with_sudo: config.run_with_sudo,
            ssh_target: config.ssh_target.clone(),
            auto_approve_commands: config.auto_approve_commands,
        }
    }

    /// Returns true if the command is safe to run autonomously.
    ///
    /// - **Non-sudo commands** are always allowed; the OS user-permission model is the
    ///   boundary — the ghost runs as the same user as the daemon.
    /// - **Sudo commands** must have their script basename listed in
    ///   `auto_approve_scripts`, regardless of `run_with_sudo`.  The leading `sudo`
    ///   token and any absolute path prefix are stripped before the basename comparison
    ///   so that both `restart-nginx.sh` and
    ///   `sudo /home/user/.daemoneye/scripts/restart-nginx.sh` match the same entry.
    /// - **`run_with_sudo`** only controls whether `resolve_command()` auto-prepends
    ///   `sudo` when executing an approved script.  It does NOT grant permission to
    ///   run arbitrary sudo commands.
    pub fn is_safe(&self, command: &str) -> bool {
        if !crate::daemon::utils::command_has_sudo(command) {
            return true;
        }

        // Sudo command — check script whitelist.
        // run_with_sudo does not affect is_safe; it only controls auto-sudo in resolve_command.
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
        self.auto_approve_scripts.iter().any(|s| s == basename)
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
#[path = "policy_tests.rs"]
mod tests;

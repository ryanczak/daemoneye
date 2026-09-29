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
    /// Carried here for completeness; the system prompt reads from `GhostConfig`
    /// directly. Kept as a mirror so `GhostPolicy` is a complete snapshot.
    pub auto_approve_commands: bool,
    /// Hosts this ghost may open shells on, by `[hosts.<name>]` key.
    /// **Empty is unrestricted**: a runbook with no `hosts:` line constrains
    /// nothing, so every runbook written before this field existed keeps its
    /// behaviour. A non-empty list is a whitelist — the ghost may reach a
    /// named host only when it appears here (exact, case-sensitive match).
    pub hosts: Vec<String>,
    /// The configured host `ssh_target` resolved to, or `None` (no
    /// `ssh_target`, or one naming no configured host).
    pub remote_host: Option<crate::host::HostId>,
    /// `remote_host`'s `sudo_scripts`. Empty = unrestricted.
    pub remote_sudo_scripts: Vec<String>,
}

impl GhostPolicy {
    /// Create a policy from the ghost configuration inherited from a runbook.
    pub fn from_config(config: &crate::ipc::GhostConfig) -> Self {
        Self {
            auto_approve_scripts: config.auto_approve_scripts.clone(),
            run_with_sudo: config.run_with_sudo,
            ssh_target: config.ssh_target.clone(),
            auto_approve_commands: config.auto_approve_commands,
            hosts: config.hosts.clone(),
            remote_host: config
                .ssh_target_host
                .as_deref()
                .and_then(|h| crate::host::HostId::parse(h).ok()),
            remote_sudo_scripts: config.ssh_target_sudo_scripts.clone(),
        }
    }

    /// May this ghost reach its `ssh_target` at all? The runbook's `hosts:`
    /// list is the fence (06d's rule), but only when there is a resolved
    /// host to fence: with `ssh_target` unset or resolving to no configured
    /// host (`remote_host: None`), a ghost with no `ssh_target` runs on the
    /// daemon host and its list says nothing about that — it is permitted
    /// regardless of the list; an `ssh_target` that names no configured host
    /// keeps 08a's unresolved rule (empty list grants it, non-empty refuses
    /// it). A resolved host must be granted by the list: empty is
    /// unrestricted; non-empty must name it.
    pub fn may_use_ssh_target(&self) -> bool {
        match (&self.ssh_target, &self.remote_host) {
            (None, _) => true,
            (Some(_), None) => self.hosts.is_empty(),
            (Some(_), Some(host)) => self.may_use_host(host),
        }
    }

    /// May this whitelisted script run under elevated permissions here? Only
    /// a resolved host with a non-empty `sudo_scripts` restricts; everything
    /// else keeps today's behaviour.
    pub fn may_elevate_script(&self, name: &str) -> bool {
        match (&self.remote_host, self.remote_sudo_scripts.is_empty()) {
            (Some(_), true) => true,
            (Some(_), false) => self.remote_sudo_scripts.iter().any(|s| s == name),
            (None, _) => true,
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

    /// Whether this ghost may reach `host`.
    ///
    /// Rules, in this order:
    ///
    /// 1. `HostId::Local` is **always permitted**. `hosts:` restricts a
    ///    runbook's *remote* reach; a ghost running on the daemon host is not
    ///    a host question at all. This rule precedes the empty-list rule so
    ///    that an unrestricted list can never be misread as "local is
    ///    forbidden too" — empty must widen, never narrow.
    /// 2. An **empty `hosts` list is unrestricted** — the compatibility rule
    ///    that keeps every runbook written before this field existed working
    ///    (a `hosts:` line is absent from all of them).
    /// 3. A named host must appear in the list. Matching is **exact and
    ///    case-sensitive**, the same comparison `classify` and the
    ///    `[hosts]` lookup already use for `[hosts.<name>]` keys: a
    ///    `hosts:` entry and a host name are the same string or they are
    ///    nothing.
    pub fn may_use_host(&self, host: &crate::host::HostId) -> bool {
        match host {
            crate::host::HostId::Local => true,
            crate::host::HostId::Named(name) => {
                self.hosts.is_empty() || self.hosts.iter().any(|h| h == name)
            }
        }
    }

    /// This policy with its remote binding replaced by `host`, as if the
    /// runbook's `ssh_target` had named it. Every other field is kept.
    ///
    /// `Local` clears the remote binding (the ghost runs on the daemon
    /// host); a named host must be a configured `[hosts]` entry, whose
    /// `sudo_scripts` become the elevation whitelist. It does **not** check
    /// the `hosts:` list — the caller's fence does that next, with its own
    /// wording and event.
    pub fn for_host(
        &self,
        host: &crate::host::HostId,
        hosts: &std::collections::HashMap<String, crate::config::HostEntry>,
    ) -> Result<GhostPolicy, String> {
        let mut rebound = self.clone();
        match host {
            crate::host::HostId::Local => {
                rebound.ssh_target = None;
                rebound.remote_host = None;
                rebound.remote_sudo_scripts = Vec::new();
            }
            crate::host::HostId::Named(key) => {
                let Some(entry) = hosts.get(key) else {
                    let configured = if hosts.is_empty() {
                        "no hosts are configured".to_string()
                    } else {
                        let mut names: Vec<&String> = hosts.keys().collect();
                        names.sort();
                        let joined = names
                            .iter()
                            .map(|n| n.as_str())
                            .collect::<Vec<_>>()
                            .join(", ");
                        format!("configured hosts: {joined}")
                    };
                    return Err(format!(
                        "'{key}' is not a configured host ({joined}). \
                         Add a [hosts.{key}] entry, or use \"local\" for the \
                         daemon host.",
                        joined = configured
                    ));
                };
                rebound.ssh_target = Some(key.clone());
                rebound.remote_host = Some(host.clone());
                rebound.remote_sudo_scripts = entry.sudo_scripts.clone();
            }
        }
        Ok(rebound)
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
                format!(
                    "ssh {} {}",
                    target,
                    crate::daemon::utils::sh_single_quote(cmd)
                )
            }
            _ => cmd.to_string(),
        }
    }

    /// When `ssh_target` is set and `cmd` invokes a whitelisted script (bare or
    /// relative name, optionally `sudo`-prefixed, possibly with args), return
    /// `(basename, args_tail)` where `args_tail` is everything after the first token
    /// (verbatim, including its leading space; empty if none). `None` otherwise.
    /// Mirrors `resolve_command`'s whitelist detection exactly.
    pub fn remote_script_call(&self, cmd: &str) -> Option<(String, String)> {
        self.ssh_target.as_ref()?;

        // Strip a leading `sudo` so `sudo script.sh` resolves identically to
        // bare `script.sh`.
        let effective_cmd = if let Some(rest) = cmd.strip_prefix("sudo ") {
            rest.trim_start()
        } else {
            cmd
        };

        let mut parts = effective_cmd.splitn(2, |c: char| c.is_whitespace());
        let first = match parts.next() {
            Some(t) if !t.is_empty() => t,
            _ => return None,
        };

        // Already absolute — no script name to transfer.
        if first.starts_with('/') {
            return None;
        }

        let args_tail = parts.next().map(|s| format!(" {}", s)).unwrap_or_default();

        let basename = std::path::Path::new(first)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(first);

        if self.auto_approve_scripts.iter().any(|s| s == basename) {
            Some((basename.to_string(), args_tail))
        } else {
            None
        }
    }

    /// When `ssh_target` is set and `cmd` invokes a whitelisted script (bare or
    /// relative name, optionally `sudo`-prefixed, possibly with args), return the
    /// script basename that must be transferred to the remote host before execution.
    /// Returns `None` for local policies (no `ssh_target`), commands whose first
    /// token is already absolute, and commands that do not invoke a whitelisted
    /// script. Mirrors `resolve_command`'s whitelist detection exactly.
    pub fn remote_script_name(&self, cmd: &str) -> Option<String> {
        self.remote_script_call(cmd).map(|(name, _)| name)
    }
}

#[cfg(test)]
#[path = "policy_tests.rs"]
mod tests;

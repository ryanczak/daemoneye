use super::*;

fn policy(scripts: &[&str]) -> GhostPolicy {
    GhostPolicy {
        auto_approve_scripts: scripts.iter().map(|s| s.to_string()).collect(),
        run_with_sudo: false,
        ssh_target: None,
        auto_approve_commands: false,
    }
}

fn sudo_policy(scripts: &[&str]) -> GhostPolicy {
    GhostPolicy {
        auto_approve_scripts: scripts.iter().map(|s| s.to_string()).collect(),
        run_with_sudo: true,
        ssh_target: None,
        auto_approve_commands: false,
    }
}

fn remote_policy(scripts: &[&str], target: &str) -> GhostPolicy {
    GhostPolicy {
        auto_approve_scripts: scripts.iter().map(|s| s.to_string()).collect(),
        run_with_sudo: false,
        ssh_target: Some(target.to_string()),
        auto_approve_commands: false,
    }
}

fn remote_sudo_policy(scripts: &[&str], target: &str) -> GhostPolicy {
    GhostPolicy {
        auto_approve_scripts: scripts.iter().map(|s| s.to_string()).collect(),
        run_with_sudo: true,
        ssh_target: Some(target.to_string()),
        auto_approve_commands: false,
    }
}

// ── is_safe ───────────────────────────────────────────────────────────────

#[test]
fn is_safe_non_sudo_always_allowed() {
    // Any non-sudo command is allowed regardless of whitelist.
    let p = policy(&[]);
    assert!(p.is_safe("ps aux"));
    assert!(p.is_safe("dmesg | tail -n 1"));
    assert!(p.is_safe("rm -rf /tmp/foo"));
    assert!(p.is_safe("./fix.sh --dry-run"));
    assert!(p.is_safe("/home/user/.daemoneye/scripts/fix.sh"));
}

#[test]
fn is_safe_sudo_on_whitelist() {
    let p = policy(&["fix.sh"]);
    assert!(p.is_safe("sudo /home/user/.daemoneye/scripts/fix.sh"));
    assert!(p.is_safe("sudo /home/user/.daemoneye/scripts/fix.sh --flag"));
}

#[test]
fn is_safe_sudo_not_on_whitelist() {
    let p = policy(&["fix.sh"]);
    assert!(!p.is_safe("sudo /home/user/.daemoneye/scripts/other.sh"));
    assert!(!p.is_safe("sudo apt install vim"));
    assert!(!p.is_safe("sudo rm -rf /var/log"));
}

#[test]
fn is_safe_run_with_sudo_does_not_allow_arbitrary_sudo() {
    // run_with_sudo only auto-prepends sudo to approved scripts; it does NOT
    // permit arbitrary sudo commands.
    let p = sudo_policy(&["ghost-test-remediation.sh"]);
    assert!(!p.is_safe("sudo dmesg | tail -n 50"));
    assert!(!p.is_safe("sudo journalctl -u nginx --since '1 hour ago'"));
    assert!(!p.is_safe("sudo apt install -y vim"));
    assert!(!p.is_safe("sudo rm -rf /tmp/old-data"));
    // But the whitelisted script is still allowed with sudo.
    assert!(p.is_safe("sudo ghost-test-remediation.sh"));
}

#[test]
fn is_safe_run_with_sudo_still_allows_non_sudo() {
    let p = sudo_policy(&[]);
    assert!(p.is_safe("dmesg | tail -n 50"));
    assert!(p.is_safe("ps aux"));
}

#[test]
fn resolve_command_bare_name() {
    let p = policy(&["fix.sh"]);
    let resolved = p.resolve_command("fix.sh");
    assert!(
        resolved.ends_with("/.daemoneye/scripts/fix.sh"),
        "got: {}",
        resolved
    );
}

#[test]
fn resolve_command_relative_path() {
    let p = policy(&["fix.sh"]);
    let resolved = p.resolve_command("./fix.sh");
    assert!(
        resolved.ends_with("/.daemoneye/scripts/fix.sh"),
        "got: {}",
        resolved
    );
}

#[test]
fn resolve_command_preserves_args() {
    let p = policy(&["fix.sh"]);
    let resolved = p.resolve_command("./fix.sh --dry-run --verbose");
    assert!(
        resolved.ends_with("/.daemoneye/scripts/fix.sh --dry-run --verbose"),
        "got: {}",
        resolved
    );
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
    assert!(
        resolved.ends_with("/.daemoneye/scripts/fix.sh"),
        "got: {}",
        resolved
    );
}

#[test]
fn resolve_command_sudo_relative_path() {
    let p = sudo_policy(&["fix.sh"]);
    let resolved = p.resolve_command("./fix.sh");
    assert!(resolved.starts_with("sudo "), "got: {}", resolved);
    assert!(
        resolved.ends_with("/.daemoneye/scripts/fix.sh"),
        "got: {}",
        resolved
    );
}

#[test]
fn resolve_command_sudo_preserves_args() {
    let p = sudo_policy(&["fix.sh"]);
    let resolved = p.resolve_command("./fix.sh --dry-run");
    assert!(resolved.starts_with("sudo "), "got: {}", resolved);
    assert!(
        resolved.ends_with("/.daemoneye/scripts/fix.sh --dry-run"),
        "got: {}",
        resolved
    );
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
    assert_eq!(
        resolved, "~/.daemoneye/scripts/fix.sh --flag",
        "got: {}",
        resolved
    );
}

#[test]
fn resolve_command_remote_sudo_prepends_sudo() {
    let p = remote_sudo_policy(&["fix.sh"], "user@zap");
    let resolved = p.resolve_command("fix.sh");
    assert_eq!(
        resolved, "sudo ~/.daemoneye/scripts/fix.sh",
        "got: {}",
        resolved
    );
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
    assert_eq!(
        resolved, "sudo ~/.daemoneye/scripts/fix.sh",
        "got: {}",
        resolved
    );
}

#[test]
fn resolve_command_sudo_prefix_no_double_sudo() {
    // Policy has run_with_sudo=true AND command starts with sudo — must not double-sudo.
    let p = remote_sudo_policy(&["fix.sh"], "user@zap");
    let resolved = p.resolve_command("sudo fix.sh");
    assert_eq!(
        resolved, "sudo ~/.daemoneye/scripts/fix.sh",
        "got: {}",
        resolved
    );
}

#[test]
fn resolve_command_sudo_prefix_local() {
    // Local policy (no ssh_target), AI emits `sudo script.sh`.
    let p = sudo_policy(&["fix.sh"]);
    let resolved = p.resolve_command("sudo fix.sh");
    assert!(resolved.starts_with("sudo "), "got: {}", resolved);
    assert!(
        resolved.ends_with("/.daemoneye/scripts/fix.sh"),
        "got: {}",
        resolved
    );
}

#[test]
fn resolve_command_sudo_prefix_with_args() {
    let p = remote_policy(&["fix.sh"], "user@zap");
    let resolved = p.resolve_command("sudo fix.sh --verbose");
    assert_eq!(
        resolved, "sudo ~/.daemoneye/scripts/fix.sh --verbose",
        "got: {}",
        resolved
    );
}

/// Full end-to-end: AI emits `sudo script.sh`, resolve then wrap_remote.
#[test]
fn resolve_then_wrap_remote_sudo_prefix() {
    let p = remote_sudo_policy(&["fix.sh"], "user@zap");
    let resolved = p.resolve_command("sudo fix.sh");
    let wrapped = p.wrap_remote(&resolved);
    assert_eq!(wrapped, "ssh user@zap 'sudo ~/.daemoneye/scripts/fix.sh'");
}

// ── auto_approve_commands ────────────────────────────────────────────────

#[test]
fn auto_approve_commands_does_not_affect_non_sudo_already_allowed() {
    // Non-sudo commands are always allowed regardless of the flag.
    let p_off = policy(&[]);
    let p_on = GhostPolicy {
        auto_approve_commands: true,
        ..policy(&[])
    };
    assert!(p_off.is_safe("df -h"));
    assert!(p_on.is_safe("df -h"));
}

#[test]
fn auto_approve_commands_does_not_grant_sudo() {
    // The flag must never allow arbitrary sudo commands.
    let p = GhostPolicy {
        auto_approve_commands: true,
        ..policy(&[])
    };
    assert!(!p.is_safe("sudo apt install vim"));
    assert!(!p.is_safe("sudo rm -rf /tmp"));
}

#[test]
fn from_config_copies_auto_approve_commands() {
    let gc = crate::ipc::GhostConfig {
        auto_approve_commands: true,
        ..Default::default()
    };
    let policy = GhostPolicy::from_config(&gc);
    assert!(policy.auto_approve_commands);
}

use super::*;

fn policy(scripts: &[&str]) -> GhostPolicy {
    GhostPolicy {
        hosts: Vec::new(),
        auto_approve_scripts: scripts.iter().map(|s| s.to_string()).collect(),
        run_with_sudo: false,
        ssh_target: None,
        auto_approve_commands: false,
        remote_host: None,
        remote_sudo_scripts: Vec::new(),
    }
}

fn sudo_policy(scripts: &[&str]) -> GhostPolicy {
    GhostPolicy {
        hosts: Vec::new(),
        auto_approve_scripts: scripts.iter().map(|s| s.to_string()).collect(),
        run_with_sudo: true,
        ssh_target: None,
        auto_approve_commands: false,
        remote_host: None,
        remote_sudo_scripts: Vec::new(),
    }
}

fn remote_policy(scripts: &[&str], target: &str) -> GhostPolicy {
    GhostPolicy {
        hosts: Vec::new(),
        auto_approve_scripts: scripts.iter().map(|s| s.to_string()).collect(),
        run_with_sudo: false,
        ssh_target: Some(target.to_string()),
        auto_approve_commands: false,
        remote_host: None,
        remote_sudo_scripts: Vec::new(),
    }
}

fn remote_sudo_policy(scripts: &[&str], target: &str) -> GhostPolicy {
    GhostPolicy {
        hosts: Vec::new(),
        auto_approve_scripts: scripts.iter().map(|s| s.to_string()).collect(),
        run_with_sudo: true,
        ssh_target: Some(target.to_string()),
        auto_approve_commands: false,
        remote_host: None,
        remote_sudo_scripts: Vec::new(),
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
fn wrap_remote_escapes_single_quote() {
    let p = remote_policy(&[], "user@zap");
    assert_eq!(
        p.wrap_remote("echo 'pwned'"),
        r"ssh user@zap 'echo '\''pwned'\'''"
    );
}

#[test]
fn wrap_remote_escapes_breakout_attempt() {
    let p = remote_policy(&[], "user@zap");
    assert_eq!(
        p.wrap_remote("x'; rm -rf ~ #"),
        r"ssh user@zap 'x'\''; rm -rf ~ #'"
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

// ── remote_script_name ────────────────────────────────────────────────────

#[test]
fn remote_script_name_detects_whitelisted() {
    let p = remote_policy(&["foo.sh"], "host");
    assert_eq!(p.remote_script_name("foo.sh"), Some("foo.sh".into()));
    assert_eq!(p.remote_script_name("./foo.sh"), Some("foo.sh".into()));
    assert_eq!(p.remote_script_name("sudo foo.sh"), Some("foo.sh".into()));
    assert_eq!(
        p.remote_script_name("foo.sh --flag arg"),
        Some("foo.sh".into())
    );
}

#[test]
fn remote_script_name_none_when_local() {
    let p = policy(&["foo.sh"]);
    assert_eq!(p.remote_script_name("foo.sh"), None);
}

#[test]
fn remote_script_name_none_for_absolute_or_unlisted() {
    let p = remote_policy(&["foo.sh"], "host");
    assert_eq!(p.remote_script_name("/usr/bin/foo.sh"), None);
    assert_eq!(p.remote_script_name("bar.sh"), None);
    assert_eq!(p.remote_script_name(""), None);
}

#[test]
fn remote_script_name_agrees_with_resolve_command() {
    let p = remote_policy(&["foo.sh"], "h");
    assert_eq!(p.remote_script_name("foo.sh"), Some("foo.sh".into()));
    let resolved = p.resolve_command("foo.sh");
    assert!(
        resolved.contains("~/.daemoneye/scripts/foo.sh"),
        "resolve_command should rewrite to tilde path, got: {}",
        resolved
    );
}

// ── remote_script_call ────────────────────────────────────────────────────

#[test]
fn remote_script_call_returns_name_and_args() {
    let p = remote_policy(&["foo.sh"], "h");
    assert_eq!(
        p.remote_script_call("foo.sh"),
        Some(("foo.sh".into(), "".into()))
    );
    assert_eq!(
        p.remote_script_call("foo.sh --flag arg"),
        Some(("foo.sh".into(), " --flag arg".into()))
    );
    assert_eq!(
        p.remote_script_call("sudo foo.sh --flag arg"),
        Some(("foo.sh".into(), " --flag arg".into()))
    );
    assert_eq!(
        p.remote_script_call("./foo.sh"),
        Some(("foo.sh".into(), "".into()))
    );
}

#[test]
fn remote_script_call_none_cases() {
    // Local policy (no ssh_target)
    let p = policy(&["foo.sh"]);
    assert_eq!(p.remote_script_call("foo.sh"), None);

    // Absolute path
    let p = remote_policy(&["foo.sh"], "h");
    assert_eq!(p.remote_script_call("/usr/bin/foo.sh"), None);

    // Not on whitelist
    assert_eq!(p.remote_script_call("bar.sh"), None);

    // Empty command
    assert_eq!(p.remote_script_call(""), None);
}

#[test]
fn remote_script_name_delegates_to_call() {
    let p = remote_policy(&["foo.sh"], "h");
    // For every case where remote_script_call returns Some,
    // remote_script_name must return the same basename.
    for cmd in &[
        "foo.sh",
        "foo.sh --flag arg",
        "sudo foo.sh --flag arg",
        "./foo.sh",
    ] {
        let call_result = p.remote_script_call(cmd);
        let name_result = p.remote_script_name(cmd);
        assert_eq!(
            call_result.map(|(n, _)| n),
            name_result,
            "mismatch for cmd={:?}",
            cmd
        );
    }
}

// ── may_use_host (M21 phase-06d) ──────────────────────────────────────────

fn hosts_policy(hosts: &[&str]) -> GhostPolicy {
    GhostPolicy {
        hosts: hosts.iter().map(|h| h.to_string()).collect(),
        ..policy(&[])
    }
}

#[test]
fn an_empty_hosts_list_is_unrestricted() {
    // The compatibility guard: every runbook on disk omits `hosts:`, so an
    // empty list permitting everything named is what keeps them working.
    let p = hosts_policy(&[]);
    assert!(
        p.may_use_host(&crate::host::HostId::Named("anything".to_string())),
        "an empty hosts list must stay unrestricted, got a refusal for 'anything'"
    );
}

#[test]
fn a_listed_host_is_permitted() {
    let p = hosts_policy(&["pinky"]);
    assert!(
        p.may_use_host(&crate::host::HostId::Named("pinky".to_string())),
        "a host the runbook names must be permitted"
    );
}

#[test]
fn an_unlisted_host_is_refused() {
    let p = hosts_policy(&["pinky"]);
    assert!(
        !p.may_use_host(&crate::host::HostId::Named("web01".to_string())),
        "the headline negative: a host the runbook did not name must be refused"
    );
}

#[test]
fn the_local_host_is_always_permitted() {
    // Rule 1 precedes rule 2: local must be true for an empty list *and* for a
    // non-empty one that never mentions it — `hosts:` narrows remote reach,
    // never the daemon host the ghost is already running on.
    let empty = hosts_policy(&[]);
    let listed = hosts_policy(&["pinky"]);
    assert!(
        empty.may_use_host(&crate::host::HostId::Local),
        "local must stay permitted under an empty list"
    );
    assert!(
        listed.may_use_host(&crate::host::HostId::Local),
        "local must stay permitted even when the list names only 'pinky'"
    );
}

#[test]
fn host_matching_is_exact() {
    // Neither a prefix nor a case variant is a match — the same comparison
    // `[hosts]` config keys and `hosts:` entries use.
    let p = hosts_policy(&["pinky"]);
    assert!(
        !p.may_use_host(&crate::host::HostId::Named("pinky.example.com".to_string())),
        "a prefix must not match"
    );
    assert!(
        !p.may_use_host(&crate::host::HostId::Named("Pinky".to_string())),
        "a case variant must not match"
    );
}

#[test]
fn from_config_carries_the_hosts_list() {
    let gc = crate::ipc::GhostConfig {
        hosts: vec!["pinky".to_string(), "db01".to_string()],
        ..Default::default()
    };
    let policy = GhostPolicy::from_config(&gc);
    assert_eq!(
        policy.hosts,
        vec!["pinky".to_string(), "db01".to_string()],
        "the list must survive the GhostConfig → GhostPolicy copy, or the \
         executor gates on a policy that was never told"
    );
    assert!(
        !policy.may_use_host(&crate::host::HostId::Named("web01".to_string())),
        "the carried list must actually decide"
    );
}

// ── may_use_ssh_target / may_elevate_script (M21 phase-08a) ───────────────

fn remote_host_policy(
    ssh_target: Option<&str>,
    hosts: &[&str],
    remote_host: Option<&str>,
    remote_scripts: &[&str],
) -> GhostPolicy {
    GhostPolicy {
        ssh_target: ssh_target.map(|t| t.to_string()),
        hosts: hosts.iter().map(|h| h.to_string()).collect(),
        remote_host: remote_host.map(|h| crate::host::HostId::parse(h).expect("well-formed host")),
        remote_sudo_scripts: remote_scripts.iter().map(|s| s.to_string()).collect(),
        ..policy(&[])
    }
}

/// An `ssh_target` that names no configured host stays on the 1.x
/// `ssh <target>` path — and with the runbook's `hosts:` list empty, nothing
/// restricts it, exactly as before this row.
#[test]
fn an_unconfigured_ssh_target_stays_on_the_1x_path() {
    let p = remote_host_policy(Some("user@far"), &[], None, &[]);
    assert!(
        p.may_use_ssh_target(),
        "an unconfigured ssh_target with an empty hosts: list is the 1.x path: unrestricted"
    );
    assert_eq!(
        p.remote_host, None,
        "no configured host means the policy carries no remote host"
    );
}

/// The headline rule of the row: a non-empty `hosts:` list is a fence for
/// `ssh_target` too. When the value names no configured host, no host can be
/// granted, so every call is refused — a list that cannot name what it
/// allows is not a fence, and silently permitting the 1.x wrap under a
/// non-empty list would let a runbook sidestep its own whitelist.
#[test]
fn a_configured_ssh_target_must_be_granted_by_hosts() {
    let granted = remote_host_policy(Some("db01"), &["db01"], Some("db01"), &[]);
    assert!(
        granted.may_use_ssh_target(),
        "a runbook that lists the resolved host may reach it"
    );
    let unlisted = remote_host_policy(Some("db01"), &["pinky"], Some("db01"), &[]);
    assert!(
        !unlisted.may_use_ssh_target(),
        "the runbook's non-empty hosts: list does not grant 'db01', so the \
         ssh_target must be refused"
    );
}

/// `sudo_scripts` is a whitelist that only bites when the host declares one.
/// An empty list must widen, never narrow — every `[hosts]` entry on disk
/// omits it, and an enforced-by-default empty list would break each of them.
#[test]
fn an_empty_host_script_list_keeps_todays_behaviour() {
    let p = remote_host_policy(Some("db01"), &[], Some("db01"), &[]);
    assert!(
        p.may_elevate_script("anything.sh"),
        "an empty sudo_scripts list is unrestricted — the compatibility rule"
    );
    assert!(
        p.may_elevate_script("restart-nginx.sh"),
        "any whitelisted script name elevates on a host that declares no list"
    );
}

/// A non-empty `sudo_scripts` list is a whitelist: only the names it carries
/// may run under sudo on that host.
#[test]
fn a_nonempty_host_script_list_permits_only_its_names() {
    let p = remote_host_policy(Some("db01"), &[], Some("db01"), &["allowed.sh"]);
    assert!(
        p.may_elevate_script("allowed.sh"),
        "a listed script may run under sudo on its host"
    );
    assert!(
        !p.may_elevate_script("other.sh"),
        "the headline negative: an unlisted script must not elevate on that host"
    );
}

/// With no resolved host at all (no `ssh_target`, or the 1.x path), the
/// elevation gate keeps today's behaviour for every name.
#[test]
fn no_resolved_host_keeps_todays_elevation_behaviour() {
    let p = remote_host_policy(None, &[], None, &[]);
    assert!(
        p.may_elevate_script("restart-nginx.sh"),
        "no resolved host means the host-side whitelist cannot apply"
    );
}

/// A ghost that works on the daemon host (no `ssh_target`) is not fenced by
/// its `hosts:` list — the list narrows *remote* reach, and there is nothing
/// remote to reach. An `ssh_target` that names no configured host keeps 08a's
/// rule: a non-empty list refuses it, an empty one grants it.
#[test]
fn a_ghost_without_an_ssh_target_is_not_fenced_by_its_hosts_list() {
    let no_target = remote_host_policy(None, &["db01"], None, &[]);
    assert!(
        no_target.may_use_ssh_target(),
        "no ssh_target means the ghost runs on the daemon host; its hosts: \
         list must not refuse that"
    );

    let unconfigured = remote_host_policy(Some("user@far"), &["db01"], None, &[]);
    assert!(
        !unconfigured.may_use_ssh_target(),
        "an ssh_target that names no configured host stays refused by a \
         non-empty hosts: list (08a's rule, unchanged)"
    );

    let empty_list = remote_host_policy(Some("user@far"), &[], None, &[]);
    assert!(
        empty_list.may_use_ssh_target(),
        "an ssh_target that names no configured host with an empty hosts: \
         list stays permitted (08a's rule, unchanged)"
    );
}

/// `for_host` rebinds a policy's remote fields as if the runbook's
/// `ssh_target` had named the host; `hosts:` is untouched (the fence that
/// consumes the rebound policy decides with it). An unknown name is an
/// error naming it.
#[test]
fn for_host_rebinds_a_ghost_policy_to_the_named_host() {
    let mut hosts = std::collections::HashMap::new();
    hosts.insert(
        "db01".to_string(),
        crate::config::HostEntry {
            ssh: "db01".to_string(),
            sudo_scripts: vec!["a.sh".to_string()],
            ..Default::default()
        },
    );
    let policy = remote_host_policy(None, &["db01"], None, &[]);
    let policy = GhostPolicy {
        ssh_target: Some("old".to_string()),
        remote_sudo_scripts: vec!["stale.sh".to_string()],
        ..policy
    };

    let named = policy
        .for_host(
            &crate::host::HostId::parse("db01").expect("well-formed host"),
            &hosts,
        )
        .expect("a configured host rebinds");
    assert_eq!(named.ssh_target.as_deref(), Some("db01"));
    assert_eq!(
        named.remote_host,
        Some(crate::host::HostId::Named("db01".to_string()))
    );
    assert_eq!(named.remote_sudo_scripts, vec!["a.sh".to_string()]);
    assert_eq!(named.hosts, vec!["db01".to_string()], "hosts: is untouched");

    let local = policy
        .for_host(&crate::host::HostId::Local, &hosts)
        .expect("local always rebinds");
    assert_eq!(local.ssh_target, None, "local clears the target");
    assert_eq!(local.remote_host, None);
    assert!(local.remote_sudo_scripts.is_empty());

    let unknown = policy.for_host(
        &crate::host::HostId::parse("nope").expect("well-formed host"),
        &hosts,
    );
    let err = unknown.expect_err("an unconfigured host is refused");
    assert!(err.contains("nope"), "the error must name the host: {err}");
}

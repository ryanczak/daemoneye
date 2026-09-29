//! Host identity for the remote execution model (design § 2.2).
//!
//! The local host is implicit and always available; every configured
//! `[hosts.<name>]` entry is an execution target.  Nothing executes here —
//! this module is identities, validation, and the `ssh` argv a host shell
//! spawns with.

use crate::config::HostEntry;

/// The name of a configured host, or the implicit local host.
///
/// `Local` is implicit and is **not** a `[hosts]` entry (design § 2.2).  A
/// `Named` id is a `[hosts.<name>]` key, validated by [`HostId::parse`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HostId {
    Local,
    Named(String),
}

impl HostId {
    /// Parse a host name into an id.
    ///
    /// `"local"` (any case) is reserved — it names the implicit host and can
    /// never be a `[hosts]` entry, or it would shadow the host that needs no
    /// configuration.  A configured name is `[A-Za-z0-9._-]+`, 1–64 chars:
    /// the shape that survives both argv construction and path joins when
    /// later phases build on it.
    pub fn parse(name: &str) -> Result<HostId, String> {
        if name.eq_ignore_ascii_case("local") {
            return Ok(HostId::Local);
        }
        if name.is_empty() {
            return Err(format!(
                "host name {name:?} is empty; expected a name matching [A-Za-z0-9._-]+, 1-64 chars"
            ));
        }
        if name.len() > 64 {
            return Err(format!(
                "host name {name:?} is {} chars long; expected [A-Za-z0-9._-]+, 1-64 chars",
                name.len()
            ));
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        {
            return Err(format!(
                "host name {name:?} contains a character outside [A-Za-z0-9._-]; expected [A-Za-z0-9._-]+, 1-64 chars"
            ));
        }
        Ok(HostId::Named(name.to_string()))
    }

    /// The id's name: `"local"` or the configured name.
    pub fn as_str(&self) -> &str {
        match self {
            HostId::Local => "local",
            HostId::Named(name) => name,
        }
    }
}

impl std::fmt::Display for HostId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How a runbook's `ssh_target` maps onto `[hosts]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SshTargetResolution {
    /// It names this configured host (the `[hosts]` key).
    Configured(String),
    /// No configured host matches — the 1.x `ssh <target>` path, unchanged.
    Unconfigured,
    /// Two or more hosts share this `ssh` destination; none is chosen.
    /// Sorted, so the event and any message are deterministic.
    Ambiguous(Vec<String>),
}

/// Resolve a runbook's `ssh_target` against the configured `[hosts]` table.
///
/// A key match wins outright; otherwise a unique `ssh`-destination match
/// names the host, and two or more shared destinations name none — "the
/// first match" in a `HashMap` is iteration-order noise, never a rule.
pub fn resolve_ssh_target(
    target: &str,
    hosts: &std::collections::HashMap<String, HostEntry>,
) -> SshTargetResolution {
    if hosts.contains_key(target) {
        return SshTargetResolution::Configured(target.to_string());
    }
    let mut by_destination: Vec<String> = hosts
        .iter()
        .filter(|(_, entry)| entry.ssh == target)
        .map(|(key, _)| key.clone())
        .collect();
    by_destination.sort();
    match by_destination.len() {
        0 => SshTargetResolution::Unconfigured,
        1 => SshTargetResolution::Configured(by_destination.remove(0)),
        _ => SshTargetResolution::Ambiguous(by_destination),
    }
}

/// The `ssh` argv for a shell on `entry`, with the control-socket path the
/// daemon owns.
///
/// `-tt` forces a PTY even though ssh's stdin is already one — without it a
/// remote command sees no tty and line-buffers, and the marker protocol needs
/// an interactive shell. The ControlMaster trio is measured, not assumed:
/// cold connect 157 ms, warm 4–5 ms.
///
/// `entry.ssh` is passed as **one** argv element: `ssh` treats everything
/// after the destination as the remote command, so splitting it on whitespace
/// would turn the remainder into a command. The control path ends in `%C`
/// unquoted — `ssh` receives it as argv, so shell quoting would become part
/// of the path. No `-o BatchMod''e=yes`: prompts must reach the user, and
/// suppressing them here would defeat the credential mechanism (pinned by
/// `ssh_argv_does_not_set_batch_mode`).
pub fn ssh_argv(entry: &HostEntry, control_path: &std::path::Path) -> Vec<String> {
    vec![
        "-tt".to_string(),
        "-o".to_string(),
        "ControlMaster=auto".to_string(),
        "-o".to_string(),
        format!("ControlPath={}/%C", control_path.display()),
        "-o".to_string(),
        "ControlPersist=10m".to_string(),
        entry.ssh.clone(),
    ]
}

/// The argv that establishes (and backgrounds) this host's ControlMaster
/// without a remote command: `-M -N -f`, the same `ControlPath` and
/// `ControlPersist` as [`ssh_argv`] so every later shell multiplexes
/// through it, and `-o BatchMode=yes` when `batch` (a ghost: never prompt).
/// The destination is last — options after it become the remote command.
pub fn connect_argv(entry: &HostEntry, control_path: &std::path::Path, batch: bool) -> Vec<String> {
    let mut argv = vec![
        "-M".to_string(),
        "-N".to_string(),
        "-f".to_string(),
        "-o".to_string(),
        format!("ControlPath={}/%C", control_path.display()),
        "-o".to_string(),
        "ControlPersist=10m".to_string(),
    ];
    if batch {
        argv.extend(["-o".to_string(), "BatchMode=yes".to_string()]);
    }
    argv.push(entry.ssh.clone());
    argv
}

/// `ssh_argv` for a ghost's shell: the same argv with `-o BatchMode=yes`
/// inserted **before the destination**, so ssh never prompts — a host that
/// wants a password or an unknown key fails at once instead of waiting for
/// an answer nobody can give. Options after the destination would become
/// the remote command, so the insertion point is load-bearing.
pub fn ghost_ssh_argv(entry: &HostEntry, control_path: &std::path::Path) -> Vec<String> {
    let mut argv = ssh_argv(entry, control_path);
    let dest = argv.len() - 1;
    argv.splice(dest..dest, ["-o".to_string(), "BatchMode=yes".to_string()]);
    argv
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HostEntry;
    use std::collections::HashMap;

    fn hosts_from(pairs: &[(&str, &str)]) -> HashMap<String, HostEntry> {
        pairs
            .iter()
            .map(|(key, ssh)| {
                (
                    key.to_string(),
                    HostEntry {
                        ssh: ssh.to_string(),
                        ..Default::default()
                    },
                )
            })
            .collect()
    }

    /// The positive case for both halves of the rule: the target is a
    /// `[hosts]` **key**.
    #[test]
    fn an_ssh_target_naming_a_host_key_resolves_to_that_host() {
        let hosts = hosts_from(&[("db01", "matt@db01.lan")]);
        assert_eq!(
            resolve_ssh_target("db01", &hosts),
            SshTargetResolution::Configured("db01".to_string()),
            "a target that is a [hosts] key names that host"
        );
    }

    /// The other way a value resolves: it matches exactly one entry's `ssh`
    /// destination. This is the documented `user@host` form meeting the
    /// configured host.
    #[test]
    fn an_ssh_target_matching_a_hosts_destination_resolves_to_that_host() {
        let hosts = hosts_from(&[("db01", "matt@db01.lan"), ("web01", "matt@web01.lan")]);
        assert_eq!(
            resolve_ssh_target("matt@db01.lan", &hosts),
            SshTargetResolution::Configured("db01".to_string()),
            "a target matching one entry's ssh destination names that host"
        );
    }

    /// Two entries sharing a destination name none: picking either would be
    /// picking iteration order out of a `HashMap`, and the sudo whitelist
    /// that follows from the choice would be arbitrary.
    #[test]
    fn an_ssh_target_matching_two_destinations_resolves_to_neither() {
        let hosts = hosts_from(&[("db01", "shared.invalid"), ("web02", "shared.invalid")]);
        assert_eq!(
            resolve_ssh_target("shared.invalid", &hosts),
            SshTargetResolution::Ambiguous(vec!["db01".to_string(), "web02".to_string()]),
            "two hosts sharing one ssh destination must resolve to an ambiguous \
             set, sorted, never to a first pick"
        );
    }

    /// The 1.x compatibility case: a value no `[hosts]` entry names, and no
    /// entry's destination matches, is still an ordinary ssh destination —
    /// and with the runbook's `hosts:` list empty it is unrestricted, exactly
    /// as before this row.
    #[test]
    fn an_unconfigured_ssh_target_stays_on_the_1x_path() {
        let hosts = hosts_from(&[("db01", "matt@db01.lan")]);
        assert_eq!(
            resolve_ssh_target("user@far", &hosts),
            SshTargetResolution::Unconfigured,
            "a value no host claims is the 1.x path, not an error"
        );
        let gc = crate::ipc::GhostConfig {
            ssh_target: Some("user@far".to_string()),
            hosts: Vec::new(),
            ..Default::default()
        };
        let policy = crate::daemon::policy::GhostPolicy::from_config(&gc);
        assert!(
            policy.may_use_ssh_target(),
            "an unconfigured ssh_target under an empty hosts: list is unrestricted"
        );
    }

    /// The key match wins even when another entry's `ssh` destination equals
    /// the key: the operator wrote `[hosts.b]`, and `[hosts.a] ssh = "b"` is
    /// an ssh alias pointing *at* that host, not another host of the same
    /// name.
    #[test]
    fn a_key_match_wins_over_a_destination_match() {
        let hosts = hosts_from(&[("a", "b"), ("b", "x")]);
        assert_eq!(
            resolve_ssh_target("b", &hosts),
            SshTargetResolution::Configured("b".to_string()),
            "a key match must win outright over a destination match on 'b'"
        );
    }

    #[test]
    fn host_id_parses_a_named_host() {
        for name in ["web01", "db01.lan", "matt-host_2", "A.9-z_"] {
            let id = HostId::parse(name);
            assert!(id.is_ok(), "{name:?} should parse, got {id:?}");
            assert_eq!(id.as_ref().unwrap().to_string(), name);
        }
    }

    #[test]
    fn host_id_treats_local_as_reserved_and_case_insensitive() {
        for spelling in ["local", "Local", "LOCAL", "lOcAl"] {
            let id = HostId::parse(spelling);
            assert!(
                matches!(id, Ok(HostId::Local)),
                "{spelling:?} must parse as the implicit local host, not a name"
            );
        }
    }

    #[test]
    fn host_id_rejects_empty_whitespace_and_path_characters() {
        let cases = ["", "web01 prod", "../etc", "web/01"];
        for name in cases {
            let err = HostId::parse(name).err();
            let err = err.unwrap_or_else(|| panic!("{name:?} should have been rejected"));
            assert!(
                err.contains(&format!("{name:?}")),
                "error for {name:?} must name the offending value, got {err:?}"
            );
            assert!(
                err.contains("A-Za-z0-9._-"),
                "error for {name:?} must state the accepted shape, got {err:?}"
            );
        }
    }

    fn entry(ssh: &str) -> HostEntry {
        HostEntry {
            ssh: ssh.to_string(),
            tags: Vec::new(),
            sudo_scripts: Vec::new(),
            sandbox_profile: None,
        }
    }

    #[test]
    fn ssh_argv_puts_the_destination_last() {
        let argv = ssh_argv(&entry("matt@web01.lan"), std::path::Path::new("/tmp/cp"));
        assert_eq!(
            argv.last().map(String::as_str),
            Some("matt@web01.lan"),
            "the destination is the last argv element: {argv:?}"
        );
        assert_eq!(
            argv.iter()
                .filter(|a| a.as_str() == "matt@web01.lan")
                .count(),
            1,
            "the destination appears exactly once: {argv:?}"
        );
        // Statement-wise on the flag set, so the "last" claim cannot pass on
        // the prefix the builder happens to produce.
        assert_eq!(
            &argv[..argv.len() - 1],
            &[
                "-tt".to_string(),
                "-o".to_string(),
                "ControlMaster=auto".to_string(),
                "-o".to_string(),
                "ControlPath=/tmp/cp/%C".to_string(),
                "-o".to_string(),
                "ControlPersist=10m".to_string(),
            ],
            "every element before the destination is a flag or its value: {argv:?}"
        );
    }

    #[test]
    fn ssh_argv_keeps_the_destination_as_one_token() {
        let argv = ssh_argv(&entry("matt@web01.lan"), std::path::Path::new("/tmp/cp"));
        assert!(
            argv.contains(&"matt@web01.lan".to_string()),
            "entry.ssh is one argv element, never split on whitespace: {argv:?}"
        );
        assert_eq!(argv[argv.len() - 1], "matt@web01.lan");
    }

    #[test]
    fn ssh_argv_carries_the_control_trio() {
        let argv = ssh_argv(
            &entry("db01"),
            std::path::Path::new("/home/u/.daemoneye/var/run/ssh"),
        );
        assert_eq!(
            argv.iter().filter(|a| a.as_str() == "-o").count(),
            3,
            "exactly three -o flags: {argv:?}"
        );
        assert!(
            argv.iter().any(|a| a.as_str() == "ControlMaster=auto"),
            "ControlMaster=auto must be present: {argv:?}"
        );
        assert!(
            argv.iter()
                .any(|a| a.as_str() == "ControlPath=/home/u/.daemoneye/var/run/ssh/%C"),
            "the control path must end in %C and carry the daemon's dir: {argv:?}"
        );
        assert!(
            argv.iter().any(|a| a.as_str() == "ControlPersist=10m"),
            "ControlPersist=10m must be present: {argv:?}"
        );
        assert_eq!(
            argv.iter().filter(|a| a.as_str() == "-tt").count(),
            1,
            "-tt appears once: {argv:?}"
        );
    }

    #[test]
    fn ssh_argv_does_not_set_batch_mode() {
        let argv = ssh_argv(&entry("db01"), std::path::Path::new("/tmp/cp"));
        assert!(
            argv.iter().all(|a| !a.contains("BatchMode")),
            "prompts must reach the user; suppressing them would defeat the credential mechanism: {argv:?}"
        );
    }

    /// Task 1's acceptance: the connect argv establishes the master every
    /// later shell multiplexes through — the same ControlPath element
    /// `ssh_argv` emits, the destination last, no `-tt`, and `BatchMode=yes`
    /// only when the caller asks for it.
    #[test]
    fn connect_argv_establishes_the_master_the_shells_reuse() {
        let cp = std::path::Path::new("/tmp/cp");
        let warm = connect_argv(&entry("db01"), cp, false);
        assert_eq!(
            &warm[..3],
            ["-M", "-N", "-f"],
            "the master-establishing flags come first: {warm:?}"
        );
        assert!(
            warm.iter().any(|a| a.as_str() == "ControlPersist=10m"),
            "the master must persist for the shells to reuse: {warm:?}"
        );
        assert_eq!(
            warm.iter().find(|a| a.starts_with("ControlPath=")),
            ssh_argv(&entry("db01"), cp)
                .iter()
                .find(|a| a.starts_with("ControlPath=")),
            "the ControlPath element must equal ssh_argv's, so both reach the \
             same socket: {warm:?}"
        );
        assert_eq!(
            warm.last().map(String::as_str),
            Some("db01"),
            "the destination is last — options after it become the remote command: {warm:?}"
        );
        assert!(
            !warm.iter().any(|a| a.as_str() == "-tt"),
            "a master carries no remote command, so no PTY: {warm:?}"
        );
        assert!(
            !warm.iter().any(|a| a.contains("BatchMode")),
            "without batch the master may prompt, so a password is answerable: {warm:?}"
        );
        let batched = connect_argv(&entry("db01"), cp, true);
        assert!(
            batched.iter().any(|a| a.as_str() == "BatchMode=yes"),
            "batch adds BatchMode=yes before the destination: {batched:?}"
        );
        assert_eq!(
            batched.last().map(String::as_str),
            Some("db01"),
            "BatchMode sits before the destination in the batched argv: {batched:?}"
        );
    }

    #[test]
    fn ghost_ssh_argv_sets_batch_mode_before_the_destination() {
        let argv = ghost_ssh_argv(&entry("db01"), std::path::Path::new("/tmp/cp"));
        assert_eq!(
            argv.last().map(String::as_str),
            Some("db01"),
            "the destination is still the last element: {argv:?}"
        );
        assert_eq!(
            &argv[argv.len() - 3..argv.len() - 1],
            &["-o".to_string(), "BatchMode=yes".to_string()],
            "the BatchMode pair sits immediately before the destination: {argv:?}"
        );
        let base = ssh_argv(&entry("db01"), std::path::Path::new("/tmp/cp"));
        let without: Vec<String> = argv
            .iter()
            .enumerate()
            .filter(|(i, _)| !(argv.len() - 3..argv.len() - 1).contains(i))
            .map(|(_, a)| a.clone())
            .collect();
        assert_eq!(
            without, base,
            "removing the inserted pair must restore the plain argv: {argv:?} vs {base:?}"
        );
    }
}

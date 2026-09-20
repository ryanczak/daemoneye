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
/// of the path. No `-o BatchMode=yes`: prompts must reach the user, and
/// suppressing them here would defeat the credential mechanism.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HostEntry;

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
        assert!(
            argv.iter()
                .position(|a| a.as_str() == "matt@web01.lan")
                .expect("one")
                > argv
                    .iter()
                    .rposition(|a| a.as_str() == "ControlPersist=10m")
                    .expect("the last -o value"),
            "no flag may follow the destination — ssh reads a later flag as a remote command: {argv:?}"
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
            "prompts must reach the user; BatchMode would suppress them: {argv:?}"
        );
    }
}

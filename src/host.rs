//! Host identity for the remote execution model (design § 2.2).
//!
//! The local host is implicit and always available; every configured
//! `[hosts.<name>]` entry is an execution target.  Nothing executes here —
//! this module is identities and validation only.

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

#[cfg(test)]
mod tests {
    use super::HostId;

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
}

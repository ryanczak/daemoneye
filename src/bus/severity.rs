//! Five-level notice severity, ordered `Debug < Info < Warning < Error <
//! Critical`. External severity strings parse case-insensitively with a
//! small alias table; anything unrecognised — including an empty string —
//! degrades to [`Severity::Debug`], which keeps the M37 fail-closed rule:
//! a severity nobody can rank sits below every threshold.

use serde::{Deserialize, Serialize};

/// The five notice severities, ordered least to most urgent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Debug,
    Info,
    Warning,
    Error,
    Critical,
}

impl Severity {
    /// Parse a severity name case-insensitively, accepting the five names
    /// plus the aliases `warn`, `informational` and `err`. Anything else is
    /// `None`.
    pub fn parse(s: &str) -> Option<Severity> {
        match s.to_lowercase().as_str() {
            "debug" => Some(Severity::Debug),
            "info" | "informational" => Some(Severity::Info),
            "warning" | "warn" => Some(Severity::Warning),
            "error" | "err" => Some(Severity::Error),
            "critical" => Some(Severity::Critical),
            _ => None,
        }
    }

    /// Parse an external severity string, degrading anything unrecognised —
    /// including `""` — to [`Severity::Debug`].
    pub fn from_external(s: &str) -> Severity {
        Self::parse(s).unwrap_or(Severity::Debug)
    }

    /// The lowercase name this severity serialises as.
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Debug => "debug",
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Error => "error",
            Severity::Critical => "critical",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Severity;

    #[test]
    fn severity_orders_debug_to_critical() {
        assert!(Severity::Debug < Severity::Info);
        assert!(Severity::Info < Severity::Warning);
        assert!(Severity::Warning < Severity::Error);
        assert!(Severity::Error < Severity::Critical);
    }

    #[test]
    fn severity_parse_accepts_names_and_aliases_case_insensitive() {
        for (input, expected) in [
            ("WARN", Severity::Warning),
            ("Informational", Severity::Info),
            ("err", Severity::Error),
            ("Critical", Severity::Critical),
            ("debug", Severity::Debug),
            ("INFO", Severity::Info),
            ("Error", Severity::Error),
        ] {
            assert_eq!(
                Severity::parse(input),
                Some(expected),
                "{input:?} should parse as {expected:?}"
            );
        }
        for rejected in ["crit", "warning2", ""] {
            assert_eq!(
                Severity::parse(rejected),
                None,
                "{rejected:?} should not parse"
            );
        }
    }

    #[test]
    fn severity_from_external_unknown_is_debug() {
        assert_eq!(Severity::from_external("crit"), Severity::Debug);
        assert_eq!(Severity::from_external(""), Severity::Debug);
        assert_eq!(Severity::from_external("warning2"), Severity::Debug);
        assert_eq!(Severity::from_external("warn"), Severity::Warning);
    }

    #[test]
    fn severity_as_str_is_the_lowercase_name() {
        assert_eq!(Severity::Debug.as_str(), "debug");
        assert_eq!(Severity::Info.as_str(), "info");
        assert_eq!(Severity::Warning.as_str(), "warning");
        assert_eq!(Severity::Error.as_str(), "error");
        assert_eq!(Severity::Critical.as_str(), "critical");
    }
}

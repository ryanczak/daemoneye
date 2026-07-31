//! Startup warnings for retention settings that are off by default.
//!
//! A pure function inspects the config and returns a list of warnings for any
//! artifact class whose retention is `0` (keep forever). The daemon logs these
//! at startup; the function itself is testable without any side effects.

use crate::config::Config;

/// A warning about a retention setting that is off by default.
#[derive(Debug, Clone)]
pub struct RetentionWarning {
    /// Human-readable name of the artifact class.
    pub artifact_class: &'static str,
    /// The config key the operator can change.
    pub config_key: &'static str,
    /// Suggested action.
    pub suggestion: &'static str,
}

/// Return warnings for artifact classes whose retention is `0` (keep forever).
///
/// Returns an empty vec when nothing is disabled. This function is pure and
/// testable — it reads only the config values and produces structured output.
pub fn retention_warnings(cfg: &Config) -> Vec<RetentionWarning> {
    let mut warnings = Vec::new();

    if cfg.sessions.archive_retention_days == 0 {
        warnings.push(RetentionWarning {
            artifact_class: "session archives",
            config_key: "sessions.archive_retention_days",
            suggestion: "Set to a non-zero value (e.g. 7) to sweep expired archives",
        });
    }

    warnings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warns_when_archive_retention_is_zero() {
        let cfg = Config::default();
        // archive_retention_days defaults to 0
        let warnings = retention_warnings(&cfg);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].artifact_class, "session archives");
        assert_eq!(warnings[0].config_key, "sessions.archive_retention_days");
    }

    #[test]
    fn no_warning_when_archive_retention_is_nonzero() {
        let mut cfg = Config::default();
        cfg.sessions.archive_retention_days = 7;
        let warnings = retention_warnings(&cfg);
        assert!(warnings.is_empty());
    }

    #[test]
    fn empty_when_all_retentions_nonzero() {
        let mut cfg = Config::default();
        cfg.sessions.archive_retention_days = 30;
        let warnings = retention_warnings(&cfg);
        assert!(warnings.is_empty());
    }
}

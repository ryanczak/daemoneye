//! The [`Filter`] consumers subscribe with: a conjunction over the notice
//! fields, where every *set* criterion must hold and an empty list means
//! "any". Later phases load these from `[[notifications.route]]` TOML and
//! from `Request::Subscribe` IPC.

use serde::{Deserialize, Serialize};

use super::notice::{Notice, NoticeStatus, Source};
use super::severity::Severity;

/// A content filter over [`Notice`] fields. `Default` matches everything.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Filter {
    /// Minimum severity, inclusive. `None` = any severity.
    pub min_severity: Option<Severity>,
    /// Any-of list of sources. Empty = any source.
    pub sources: Vec<Source>,
    /// Any-of list of exact names. Empty = any name.
    pub names: Vec<String>,
    /// Any-of list of statuses. Empty = any status.
    pub statuses: Vec<NoticeStatus>,
}

impl Filter {
    /// Whether `n` satisfies every set criterion. `min_severity` is
    /// inclusive (`warning` passes a warning notice); an empty list means
    /// any; names compare exactly.
    pub fn matches(&self, n: &Notice) -> bool {
        if let Some(min) = self.min_severity
            && n.severity < min
        {
            return false;
        }
        if !self.sources.is_empty() && !self.sources.contains(&n.source) {
            return false;
        }
        if !self.names.is_empty() && !self.names.iter().any(|name| name == &n.name) {
            return false;
        }
        if !self.statuses.is_empty() && !self.statuses.contains(&n.status) {
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::Filter;
    use crate::bus::notice::{Notice, NoticeStatus, Severity, Source};

    fn firing_warning() -> Notice {
        Notice::new(Severity::Warning, Source::Webhook, "HighCPU", "CPU high")
            .status(NoticeStatus::Firing)
    }

    #[test]
    fn filter_default_matches_everything() {
        let f = Filter::default();
        assert!(f.matches(&firing_warning()));
        assert!(f.matches(&Notice::new(
            Severity::Debug,
            Source::Shell,
            "anything",
            "t"
        )));
    }

    #[test]
    fn filter_min_severity_is_inclusive() {
        let f = Filter {
            min_severity: Some(Severity::Warning),
            ..Filter::default()
        };
        assert!(f.matches(&firing_warning()), "warning passes warning");
        assert!(
            f.matches(&Notice::new(
                Severity::Critical,
                Source::Webhook,
                "HighCPU",
                "t"
            )),
            "critical passes warning threshold"
        );
        assert!(
            !f.matches(&Notice::new(
                Severity::Info,
                Source::Webhook,
                "HighCPU",
                "t"
            )),
            "info is below warning"
        );
        assert!(
            !f.matches(&Notice::new(
                Severity::Debug,
                Source::Webhook,
                "HighCPU",
                "t"
            )),
            "debug is below warning"
        );
    }

    #[test]
    fn filter_lists_are_any_of_and_empty_is_any() {
        let sources = Filter {
            sources: vec![Source::Scheduler, Source::Ghost],
            ..Filter::default()
        };
        assert!(sources.matches(&Notice::new(Severity::Info, Source::Scheduler, "j", "t")));
        assert!(sources.matches(&Notice::new(Severity::Info, Source::Ghost, "j", "t")));
        assert!(!sources.matches(&Notice::new(Severity::Info, Source::Webhook, "j", "t")));

        let names = Filter {
            names: vec!["HighCPU".to_string()],
            ..Filter::default()
        };
        assert!(names.matches(&firing_warning()));
        assert!(!names.matches(&Notice::new(
            Severity::Warning,
            Source::Webhook,
            "HighMemory",
            "t"
        )));

        let statuses = Filter {
            statuses: vec![NoticeStatus::Resolved],
            ..Filter::default()
        };
        assert!(statuses.matches(&firing_warning().status(NoticeStatus::Resolved)));
        assert!(!statuses.matches(&firing_warning()));
    }

    #[test]
    fn filter_deserializes_from_toml() {
        let f: Filter = toml::from_str(
            r#"
            min_severity = "warning"
            sources = ["webhook", "scheduler"]
            names = ["HighCPU"]
            statuses = ["firing"]
            "#,
        )
        .expect("valid filter TOML");
        assert_eq!(f.min_severity, Some(Severity::Warning));
        assert_eq!(f.sources, vec![Source::Webhook, Source::Scheduler]);
        assert_eq!(f.names, vec!["HighCPU".to_string()]);
        assert_eq!(f.statuses, vec![NoticeStatus::Firing]);
    }

    #[test]
    fn filter_rejects_unknown_field() {
        let result: Result<Filter, _> = toml::from_str("min_severty = \"info\"");
        assert!(result.is_err(), "a misspelled field must be an error");
    }

    #[test]
    fn filter_partial_toml_defaults_the_rest() {
        let f: Filter = toml::from_str("min_severity = \"error\"").expect("valid");
        assert_eq!(f.min_severity, Some(Severity::Error));
        assert!(f.sources.is_empty());
        assert!(f.names.is_empty());
        assert!(f.statuses.is_empty());
    }

    /// The `Request::Subscribe` filter crosses the IPC wire as JSON, with all
    /// four fields set (phase 10b).
    #[test]
    fn filter_round_trips_through_json() {
        let f = Filter {
            min_severity: Some(Severity::Warning),
            sources: vec![Source::Webhook, Source::Scheduler],
            names: vec!["HighCPU".to_string()],
            statuses: vec![NoticeStatus::Firing],
        };
        let json = serde_json::to_string(&f).expect("serialize filter");
        let back: Filter = serde_json::from_str(&json).expect("deserialize filter");
        assert_eq!(back, f, "every field survives the wire");
    }
}

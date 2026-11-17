//! The notice envelope and its field enums: [`Notice`], the record every
//! producer publishes and every consumer filters on, [`Source`] (which
//! subsystem emitted it) and [`NoticeStatus`] (firing / resolved / event).

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub use super::severity::Severity;

/// The subsystem a notice came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Webhook,
    Scheduler,
    Watchdog,
    Ghost,
    Background,
    Shell,
    Daemon,
}

impl Source {
    /// The lowercase name this source serialises as.
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Webhook => "webhook",
            Source::Scheduler => "scheduler",
            Source::Watchdog => "watchdog",
            Source::Ghost => "ghost",
            Source::Background => "background",
            Source::Shell => "shell",
            Source::Daemon => "daemon",
        }
    }
}

/// What a notice reports: an alert firing, an alert resolving, or a plain
/// one-off event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NoticeStatus {
    Firing,
    Resolved,
    Event,
}

/// One thing one subsystem wants someone to know. Producers publish it;
/// consumers subscribe with a [`Filter`](super::filter::Filter) over these
/// fields.
///
/// No field may be named `event` or `pid` — the journal record already uses
/// those keys, and `log_event` stamps them itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notice {
    /// Fresh uuid v4 per notice.
    pub id: String,
    /// When the notice was created (UTC).
    pub ts: DateTime<Utc>,
    pub severity: Severity,
    pub source: Source,
    /// The producer's own identifier for the thing that fired (an alert
    /// fingerprint, a job id, a shell id). Empty when there is none.
    pub source_id: String,
    /// The machine-readable name consumers route on (e.g. `HighCPU`).
    pub name: String,
    pub status: NoticeStatus,
    /// Correlates a firing with its resolve. Defaults to the notice's own
    /// id for one-off events.
    pub fingerprint: String,
    /// `None` = every chat session, `Some(id)` = one chat session.
    pub session: Option<String>,
    pub title: String,
    pub body: String,
    pub labels: BTreeMap<String, String>,
}

impl Notice {
    /// A new notice with a fresh id and timestamp, `status = Event`,
    /// `fingerprint = id`, and empty `source_id`, `body` and labels. Builder
    /// methods set the rest.
    pub fn new(
        severity: Severity,
        source: Source,
        name: impl Into<String>,
        title: impl Into<String>,
    ) -> Self {
        let id = uuid::Uuid::new_v4().to_string();
        Self {
            id: id.clone(),
            ts: Utc::now(),
            severity,
            source,
            source_id: String::new(),
            name: name.into(),
            status: NoticeStatus::Event,
            fingerprint: id,
            session: None,
            title: title.into(),
            body: String::new(),
            labels: BTreeMap::new(),
        }
    }

    /// Set `source_id`.
    pub fn source_id(mut self, source_id: impl Into<String>) -> Self {
        self.source_id = source_id.into();
        self
    }

    /// Set `status`.
    pub fn status(mut self, status: NoticeStatus) -> Self {
        self.status = status;
        self
    }

    /// Set `fingerprint`.
    pub fn fingerprint(mut self, fingerprint: impl Into<String>) -> Self {
        self.fingerprint = fingerprint.into();
        self
    }

    /// Set `session`.
    pub fn session(mut self, session: impl Into<String>) -> Self {
        self.session = Some(session.into());
        self
    }

    /// Set `body`.
    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = body.into();
        self
    }

    /// Add one label.
    pub fn label(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.labels.insert(key.into(), value.into());
        self
    }

    /// The text the history sink appends to a session: the title, or
    /// `title + "\n" + body` when the body is non-empty.
    pub fn history_text(&self) -> String {
        if self.body.is_empty() {
            self.title.clone()
        } else {
            format!("{}\n{}", self.title, self.body)
        }
    }

    /// The one-line form the chat-notice overlay shows: the title's first
    /// line.
    pub fn line(&self) -> &str {
        self.title.lines().next().unwrap_or(&self.title)
    }
}

#[cfg(test)]
mod tests {
    use super::{Notice, NoticeStatus, Severity, Source};

    #[test]
    fn notice_new_defaults() {
        let a = Notice::new(Severity::Info, Source::Webhook, "HighCPU", "CPU high");
        let b = Notice::new(Severity::Info, Source::Webhook, "HighCPU", "CPU high");
        assert_ne!(a.id, b.id, "two notices get distinct ids");
        assert_eq!(a.status, NoticeStatus::Event);
        assert_eq!(a.fingerprint, a.id, "fingerprint defaults to id");
        assert_eq!(b.fingerprint, b.id);
        assert_eq!(a.session, None);
        assert_eq!(a.source_id, "");
        assert_eq!(a.body, "");
        assert!(a.labels.is_empty());
    }

    #[test]
    fn notice_serde_uses_lowercase_enums() {
        let n = Notice::new(Severity::Warning, Source::Webhook, "HighCPU", "CPU high")
            .status(NoticeStatus::Firing);
        let json = serde_json::to_value(&n).expect("serialize");
        assert_eq!(json["severity"], "warning");
        assert_eq!(json["source"], "webhook");
        assert_eq!(json["status"], "firing");
        let round: Notice = serde_json::from_value(json).expect("deserialize");
        assert_eq!(round, n);
    }

    #[test]
    fn notice_builder_sets_the_rest() {
        let n = Notice::new(
            Severity::Error,
            Source::Scheduler,
            "Backup",
            "Backup failed",
        )
        .source_id("job-7")
        .status(NoticeStatus::Resolved)
        .fingerprint("fp-1")
        .session("sess-1")
        .body("disk full")
        .label("host", "pinky");
        assert_eq!(n.source_id, "job-7");
        assert_eq!(n.status, NoticeStatus::Resolved);
        assert_eq!(n.fingerprint, "fp-1");
        assert_eq!(n.session.as_deref(), Some("sess-1"));
        assert_eq!(n.body, "disk full");
        assert_eq!(n.labels.get("host").map(String::as_str), Some("pinky"));
    }

    #[test]
    fn notice_history_text_joins_title_and_body() {
        let bare = Notice::new(Severity::Info, Source::Webhook, "HighCPU", "CPU at 95%");
        assert_eq!(
            bare.history_text(),
            "CPU at 95%",
            "an empty body gives the title alone"
        );
        let full = Notice::new(Severity::Info, Source::Webhook, "HighCPU", "CPU at 95%")
            .body("check node exporter");
        assert_eq!(
            full.history_text(),
            "CPU at 95%\ncheck node exporter",
            "a non-empty body is joined under the title"
        );
    }

    #[test]
    fn notice_line_is_the_titles_first_line() {
        let n = Notice::new(
            Severity::Info,
            Source::Webhook,
            "HighCPU",
            "CPU at 95%\nnode2",
        )
        .body("body");
        assert_eq!(n.line(), "CPU at 95%");
        let single = Notice::new(Severity::Info, Source::Ghost, "clean exit", "one line");
        assert_eq!(single.line(), "one line");
    }
}

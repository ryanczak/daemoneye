//! Reading journaled notices back out of the event log (M38 phase 08a).
//!
//! `Bus::publish` journals each notice as an `event: "notice"` record —
//! the serialized [`Notice`] plus the `event` and `pid` keys `log_event`
//! stamps, with the record's `ts` being the notice's own. This module is
//! the read side: parse one record, or stream every notice in a window.
//! It is shared by the registry replay (this phase) and later readers
//! (away digest, catch-up brief).

use super::Notice;
use crate::daemon::utils::for_each_event_between;

/// Parse one journal record into the notice it journaled.
///
/// `Some` only for a record whose `event` is `"notice"` and whose payload
/// deserializes as a [`Notice`]. The extra `event` and `pid` keys the
/// record carries are ignored, as `serde` ignores unknown fields. Anything
/// else — another event kind, a notice missing a field, a non-object — is
/// `None`.
pub fn notice_from_record(record: &serde_json::Value) -> Option<Notice> {
    if record.get("event").and_then(|e| e.as_str()) != Some("notice") {
        return None;
    }
    serde_json::from_value(record.clone()).ok()
}

/// Stream every journaled notice in `[from, to)`, in file order, through
/// `f`. A malformed line or a non-notice record is skipped, never a panic.
pub fn for_each_notice_between(
    from: Option<chrono::DateTime<chrono::Utc>>,
    to: Option<chrono::DateTime<chrono::Utc>>,
    f: &mut dyn FnMut(&Notice),
) {
    for_each_event_between(from, to, &mut |record| {
        if let Some(notice) = notice_from_record(record) {
            f(&notice);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::{NoticeStatus, Severity, Source};

    /// A record shaped exactly as `Bus::new`'s journal writes it: the
    /// serialized notice with `log_event`'s `ts`, `event` and `pid` on top.
    fn journal_record(notice: &Notice) -> serde_json::Value {
        let mut record = serde_json::to_value(notice).expect("serialize notice");
        let obj = record.as_object_mut().expect("notice is an object");
        let mut stamped = serde_json::Map::new();
        stamped.insert(
            "ts".to_string(),
            serde_json::Value::String(notice.ts.to_rfc3339()),
        );
        stamped.insert(
            "event".to_string(),
            serde_json::Value::String("notice".to_string()),
        );
        stamped.insert("pid".to_string(), serde_json::Value::from(4242));
        for (k, v) in obj.clone() {
            stamped.insert(k, v);
        }
        serde_json::Value::Object(stamped)
    }

    fn firing_notice() -> Notice {
        Notice::new(Severity::Warning, Source::Webhook, "HighCPU", "CPU high")
            .source_id("alertmanager")
            .status(NoticeStatus::Firing)
            .fingerprint("fp-journal-1")
            .label("severity", "warning")
    }

    #[test]
    fn notice_from_record_reads_a_journal_line() {
        let notice = firing_notice();
        let record = journal_record(&notice);
        let parsed = notice_from_record(&record).expect("a notice record parses");
        assert_eq!(parsed, notice, "the parsed notice equals the original");
    }

    #[test]
    fn notice_from_record_skips_other_records() {
        let webhook_alert = serde_json::json!({
            "ts": "2026-11-17T00:00:00Z",
            "event": "webhook_alert",
            "pid": 4242,
            "alert_name": "HighCPU",
        });
        assert!(
            notice_from_record(&webhook_alert).is_none(),
            "a webhook_alert record is not a notice"
        );

        let missing_source = serde_json::json!({
            "ts": "2026-11-17T00:00:00Z",
            "event": "notice",
            "pid": 4242,
            "id": "n-1",
            "severity": "warning",
            "name": "HighCPU",
            "status": "firing",
            "title": "CPU high",
        });
        assert!(
            notice_from_record(&missing_source).is_none(),
            "a notice record missing `source` does not parse"
        );

        assert!(
            notice_from_record(&serde_json::json!("just a string")).is_none(),
            "a non-object record is not a notice"
        );
        assert!(
            notice_from_record(&serde_json::Value::Null).is_none(),
            "null is not a notice"
        );
    }
}

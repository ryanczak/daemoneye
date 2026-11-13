//! The active-alert registry (M37 phase 01): the daemon's record of which
//! alerts are currently firing.
//!
//! `process_alert` records every alert that survives deduplication, before
//! the severity gate, so a below-threshold alert is recorded too. Phases 02
//! (chat status bar) and 03 (`top`) render the snapshot this module serves;
//! this phase ships no UI.
//!
//! Entries expire after [`ACTIVE_ALERT_TTL_SECS`] without a re-fire, pruned
//! whenever a snapshot is taken. The TTL is the named constant, not a config
//! key.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::ipc::AlertBadge;
use crate::util::UnpoisonExt;

/// An entry not re-seen for this long is pruned on the next snapshot.
pub const ACTIVE_ALERT_TTL_SECS: u64 = 6 * 3600;

#[derive(Debug, Clone)]
pub(crate) struct ActiveAlert {
    name: String,
    severity: String,
    first_seen_unix: u64,
    last_seen_unix: u64,
}

/// Process-wide registry: fingerprint → the alert's record.
static ACTIVE_ALERTS: std::sync::OnceLock<Mutex<HashMap<String, ActiveAlert>>> =
    std::sync::OnceLock::new();

fn registry() -> &'static Mutex<HashMap<String, ActiveAlert>> {
    ACTIVE_ALERTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Rank a severity string, reusing `process.rs`'s table — the one ranking
/// function. An unrankable severity ranks below `info` so it sorts last and
/// the gate treats it as below threshold.
pub fn active_severity_rank(severity: &str) -> u8 {
    super::process::severity_rank(severity).unwrap_or_default()
}

/// Record a firing alert: insert the entry, or refresh `last_seen_unix` on
/// the existing one, keeping `first_seen_unix`. Called for every alert that
/// survives dedup, before the severity gate.
///
/// Returns `true` when the record changed the registry (a new fingerprint,
/// or a refresh that altered name or severity, which the snapshot would
/// reflect) — the caller's signal to broadcast the new snapshot.
pub(crate) fn active_alerts_firing_in(
    map: &Mutex<HashMap<String, ActiveAlert>>,
    fingerprint: &str,
    name: &str,
    severity: &str,
    now_unix: u64,
) -> bool {
    let mut map = map.lock().unwrap_or_log();
    match map.get_mut(fingerprint) {
        Some(entry) => {
            let changed = entry.name != name || entry.severity != severity;
            entry.name = name.to_string();
            entry.severity = severity.to_string();
            entry.last_seen_unix = now_unix;
            changed
        }
        None => {
            map.insert(
                fingerprint.to_string(),
                ActiveAlert {
                    name: name.to_string(),
                    severity: severity.to_string(),
                    first_seen_unix: now_unix,
                    last_seen_unix: now_unix,
                },
            );
            true
        }
    }
}

/// Record a firing alert in the process-wide registry.
pub fn active_alerts_firing(fingerprint: &str, name: &str, severity: &str, now_unix: u64) -> bool {
    active_alerts_firing_in(registry(), fingerprint, name, severity, now_unix)
}

/// Record a resolved alert: remove its entry. Returns `true` when an entry
/// with that fingerprint was removed.
pub(crate) fn active_alerts_resolved_in(
    map: &Mutex<HashMap<String, ActiveAlert>>,
    fingerprint: &str,
) -> bool {
    map.lock().unwrap_or_log().remove(fingerprint).is_some()
}

/// Record a resolved alert in the process-wide registry.
pub fn active_alerts_resolved(fingerprint: &str) -> bool {
    active_alerts_resolved_in(registry(), fingerprint)
}

/// Prune entries not re-seen within the TTL, then snapshot what is left:
/// worst severity first, then the newest `first_seen_unix` first. An
/// unrankable severity ranks below `info`.
pub(crate) fn active_alerts_snapshot_in(
    map: &Mutex<HashMap<String, ActiveAlert>>,
    now_unix: u64,
) -> Vec<AlertBadge> {
    let mut map = map.lock().unwrap_or_log();
    map.retain(|_, entry| now_unix.saturating_sub(entry.last_seen_unix) < ACTIVE_ALERT_TTL_SECS);
    let mut entries: Vec<AlertBadge> = map
        .iter()
        .map(|(fp, e)| AlertBadge {
            fingerprint: fp.clone(),
            name: e.name.clone(),
            severity: e.severity.clone(),
            first_seen_unix: e.first_seen_unix,
            last_seen_unix: e.last_seen_unix,
        })
        .collect();
    drop(map);
    entries.sort_by(|a, b| {
        active_severity_rank(&a.severity)
            .cmp(&active_severity_rank(&b.severity))
            .reverse()
            .then(b.first_seen_unix.cmp(&a.first_seen_unix))
    });
    entries
}

/// Snapshot the process-wide registry.
pub fn active_alerts_snapshot() -> Vec<AlertBadge> {
    active_alerts_snapshot_in(registry(), super::process::now_secs_pub())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> Mutex<HashMap<String, ActiveAlert>> {
        Mutex::new(HashMap::new())
    }

    fn badge(alert: &AlertBadge) -> String {
        format!(
            "{}:{}:{}",
            alert.name, alert.severity, alert.first_seen_unix
        )
    }

    #[test]
    fn active_alerts_firing_inserts_and_refire_keeps_first_seen() {
        let map = fresh();
        assert!(
            active_alerts_firing_in(&map, "fp1", "HighCPU", "critical", 1000),
            "the first fire is a change"
        );
        assert!(
            !active_alerts_firing_in(&map, "fp1", "HighCPU", "critical", 1010),
            "a re-fire that keeps name and severity changes nothing"
        );

        let snap = active_alerts_snapshot_in(&map, 1010);
        assert_eq!(snap.len(), 1, "one entry after the re-fire");
        assert_eq!(snap[0].name, "HighCPU");
        assert_eq!(snap[0].severity, "critical");
        assert_eq!(snap[0].first_seen_unix, 1000, "first_seen is kept");
        assert_eq!(snap[0].last_seen_unix, 1010, "last_seen is refreshed");

        assert!(
            active_alerts_firing_in(&map, "fp1", "HighDisk", "critical", 1020),
            "a severity change is a change worth broadcasting"
        );
        let snap = active_alerts_snapshot_in(&map, 1020);
        assert_eq!(snap[0].name, "HighDisk", "the refresh overwrites the name");
        assert_eq!(snap[0].first_seen_unix, 1000, "first_seen survives it");
    }

    #[test]
    fn active_alerts_resolved_removes_the_entry() {
        let map = fresh();
        active_alerts_firing_in(&map, "fp1", "HighCPU", "critical", 1000);
        active_alerts_firing_in(&map, "fp2", "HighDisk", "warning", 1000);

        assert!(
            active_alerts_resolved_in(&map, "fp1"),
            "a recorded fingerprint resolves"
        );
        assert!(
            !active_alerts_resolved_in(&map, "fp1"),
            "resolving an absent fingerprint is not a change"
        );

        let snap = active_alerts_snapshot_in(&map, 1000);
        assert_eq!(snap.len(), 1, "only the un-resolved entry remains");
        assert_eq!(snap[0].name, "HighDisk");
    }

    #[test]
    fn active_alerts_snapshot_orders_worst_then_newest() {
        let map = fresh();
        // Inserted deliberately out of order, with first_seen descending on
        // the same severity, so both sort keys are exercised.
        active_alerts_firing_in(&map, "fp-info", "Heartbeat", "info", 3000);
        active_alerts_firing_in(&map, "fp-warn-old", "DiskFull", "warning", 2000);
        active_alerts_firing_in(&map, "fp-warn-new", "LatencyHigh", "warning", 4000);
        active_alerts_firing_in(&map, "fp-crit", "ServiceDown", "critical", 1000);

        let snap = active_alerts_snapshot_in(&map, 4000);
        let names: Vec<&str> = snap.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["ServiceDown", "LatencyHigh", "DiskFull", "Heartbeat"],
            "worst severity first, then newest first_seen first: {names:?}"
        );
    }

    #[test]
    fn active_alerts_unrankable_severity_ranks_below_info() {
        let map = fresh();
        active_alerts_firing_in(&map, "fp-banana", "BananaAlert", "banana", 1000);
        active_alerts_firing_in(&map, "fp-info", "Heartbeat", "info", 1000);

        let snap = active_alerts_snapshot_in(&map, 1000);
        let names: Vec<&str> = snap.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["Heartbeat", "BananaAlert"],
            "an unrankable severity sorts below info: {names:?}"
        );
    }

    #[test]
    fn active_alerts_prune_entries_past_the_ttl() {
        let map = fresh();
        active_alerts_firing_in(&map, "fp-old", "StaleAlert", "critical", 0);
        active_alerts_firing_in(&map, "fp-fresh", "FreshAlert", "critical", 1000);

        // Just inside the TTL the stale entry survives; at the TTL boundary
        // and past it, it is pruned.
        let snap = active_alerts_snapshot_in(&map, ACTIVE_ALERT_TTL_SECS - 1);
        assert_eq!(snap.len(), 2, "inside the TTL nothing is pruned yet");

        let snap = active_alerts_snapshot_in(&map, ACTIVE_ALERT_TTL_SECS);
        let names: Vec<&str> = snap.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["FreshAlert"],
            "an entry not re-seen within the TTL is pruned: {names:?}"
        );

        // A re-fire restarts the TTL clock.
        active_alerts_firing_in(&map, "fp-fresh", "FreshAlert", "critical", 2000);
        let snap = active_alerts_snapshot_in(&map, 2000 + ACTIVE_ALERT_TTL_SECS - 1);
        assert_eq!(
            snap.len(),
            1,
            "the re-fired entry survives past the original TTL"
        );
        assert_eq!(
            badge(&snap[0]),
            "FreshAlert:critical:1000",
            "first_seen kept"
        );
    }
}

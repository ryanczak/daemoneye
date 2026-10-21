//! The local-day spend ledger.
//!
//! One process-wide total of what every model call cost **today**, where
//! "today" is the *local* calendar day — the same day the `[budget]
//! max_cost_per_day` cap uses (M30 phase 06), not the UTC day
//! `daemoneye status`'s breakdowns show.
//!
//! The ledger is seeded lazily: the first read (or first add) on a day
//! that it does not yet hold sums the `ai_cost` events from local
//! midnight to now via [`crate::daemon::utils::sum_cost_between`], so a
//! daemon restarted mid-day inherits what its earlier life already spent.
//! After the seed, every [`Ledger::add`] accumulates on top.

use crate::util::UnpoisonExt;

use chrono::{DateTime, Local, TimeZone, Utc};
use std::sync::Mutex;

/// The ledger's pure core: the local day it holds and the total spent that
/// day, in USD.
#[derive(Debug, Clone, PartialEq)]
pub struct Ledger {
    /// The local calendar day the total covers (days since the local epoch).
    pub day: chrono::NaiveDate,
    /// Total spend attributed to `day`, USD.
    pub total: f64,
}

impl Ledger {
    /// A ledger holding nothing for `day`.
    pub fn new(day: chrono::NaiveDate) -> Self {
        Ledger { day, total: 0.0 }
    }

    /// Re-seed from `seed` when `now` falls on a day other than the one held.
    /// The seed runs at most once per day-transition: after it, `day` is the
    /// day `now` falls on, so later reads on the same day are no-ops.
    pub fn reseed_if_needed(&mut self, now: DateTime<Local>, seed: impl FnOnce() -> f64) {
        let today = now.date_naive();
        if self.day != today {
            self.day = today;
            self.total = seed();
        }
    }

    /// Add `cost` to the ledger's total.
    pub fn add(&mut self, cost: f64) {
        self.total += cost;
    }

    /// The total for `now`'s local day, seeding first when the held day is
    /// stale.
    pub fn total_for(&mut self, now: DateTime<Local>, seed: impl FnOnce() -> f64) -> f64 {
        self.reseed_if_needed(now, seed);
        self.total
    }

    /// Record one model call's cost against `now`'s local day, with the
    /// `ai_cost` line's write inside the same critical section: the seed
    /// runs first, then `write_line`, then the add — all under the caller's
    /// ledger lock.
    ///
    /// The order is what makes the total right. A seed that runs *before*
    /// the write reads the log before this call's line exists, so it can
    /// never hold this call's cost; the add is therefore always due and the
    /// seed's return value is ignored for accounting (bug-06-1: phase 05's
    /// "skip the add when a seed ran" rule was derived for the old
    /// write-then-seed order, where a seed did already hold the line, and
    /// counting zero instead of once is the same failure the other way).
    /// A line that failed to serialize never reached the log either, so its
    /// cost lands the same way.
    pub fn record_with_write(
        &mut self,
        now: DateTime<Local>,
        cost: f64,
        write_line: impl FnOnce(),
        seed: impl FnOnce() -> f64,
    ) {
        self.reseed_if_needed(now, seed);
        write_line();
        self.add(cost);
    }
}

/// Local-midnight-to-now window for `now_local`, in UTC — the interval
/// [`sum_cost_between`](crate::daemon::utils::sum_cost_between) reads.
///
/// A DST day can repeat or skip local midnight: a repeated midnight is
/// ambiguous (two UTC instants map to it) and a skipped one does not exist.
/// The earliest valid mapping is used — for a skipped midnight `Local.from`
/// gives the instant the clock jumped *to*, which is the earliest instant
/// that day exists, and for a repeated one it picks the first pass.
pub fn local_day_bounds(now_local: DateTime<Local>) -> (DateTime<Utc>, DateTime<Utc>) {
    let midnight = now_local
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap_or_else(|| now_local.naive_local());
    let from = match Local.from_local_datetime(&midnight) {
        chrono::LocalResult::Single(dt) => dt,
        chrono::LocalResult::Ambiguous(dt, _) => dt,
        chrono::LocalResult::None => match Local.from_local_datetime(&now_local.naive_local()) {
            chrono::LocalResult::Single(dt) => dt,
            chrono::LocalResult::Ambiguous(dt, _) => dt,
            // `now_local` itself is a valid local instant by construction.
            chrono::LocalResult::None => now_local,
        },
    };
    (from.with_timezone(&Utc), now_local.with_timezone(&Utc))
}

static LEDGER: Mutex<Option<Ledger>> = Mutex::new(None);

/// Seed the global ledger for `now`'s local day: sum `ai_cost` events from
/// local midnight to now. Used both by the lazy first read and by re-seeds
/// at a day transition.
fn seed_from_events(now: DateTime<Local>) -> Ledger {
    let (from, to) = local_day_bounds(now);
    let summary = crate::daemon::utils::sum_cost_between(from, to);
    Ledger {
        day: now.date_naive(),
        total: summary.total_cost_usd,
    }
}

/// The total spend for the current local day, USD. Seeds from the event log
/// the first time it is read in a daemon's life and whenever the local day
/// rolls over; in-memory adds accumulate on top of the seed.
pub fn spent_today() -> f64 {
    let now = Local::now();
    let mut guard = LEDGER.lock().unwrap_or_log();
    let ledger = guard.get_or_insert_with(|| seed_from_events(now));
    ledger.total_for(now, || seed_from_events(now).total)
}

/// Record one model call's cost against the local-day ledger, with the
/// `ai_cost` line write inside the same critical section.
///
/// `write_line` runs after any seed this record triggers and before the
/// add, all under [`LEDGER`]'s lock — so a `spent_today()` gate read can
/// never land between the seed and the write and count the cost twice
/// (bug-05-1). The seed reads the log before the write, so it cannot hold
/// this call's line and the add is always due (bug-06-1); a line that
/// failed to serialize is no different — it is not on disk either, so its
/// cost lands the same way.
///
/// Gotcha honoured by the callers: [`crate::daemon::utils::log_event`] must
/// not take the ledger lock, and nothing it calls may — it runs here under
/// the lock by design, and re-taking it would deadlock.
pub fn record_under_lock(cost: f64, write_line: impl FnOnce()) {
    let now = Local::now();
    let mut guard = LEDGER.lock().unwrap_or_log();
    match guard.as_mut() {
        Some(ledger) => {
            ledger.record_with_write(now, cost, write_line, || seed_from_events(now).total)
        }
        None => {
            let mut ledger = seed_from_events(now);
            write_line();
            ledger.add(cost);
            *guard = Some(ledger);
        }
    }
}

/// Record one model call's cost against the local-day ledger. Called by
/// [`crate::cost::emit_cost`] once the call's outcome is known.
///
/// The seed (when this record trips one) runs before the line's write and
/// reads the log without it, so the cost is always added on top — a seed
/// and an add of the same record land together, and exactly once.
pub fn record(cost: f64) {
    record_under_lock(cost, || {});
}

/// Where a refusal to spend came from. The `budget_refused` event's `path`
/// field, so a refusal can be traced to the call site that made it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpendPath {
    Chat,
    Ghost,
    Scheduled,
    Webhook,
    Briefing,
    Digest,
    AutoName,
}

impl SpendPath {
    /// The string the `budget_refused` event's `path` field and the log
    /// lines carry.
    pub fn label(&self) -> &'static str {
        match self {
            SpendPath::Chat => "chat",
            SpendPath::Ghost => "ghost",
            SpendPath::Scheduled => "scheduled",
            SpendPath::Webhook => "webhook",
            SpendPath::Briefing => "briefing",
            SpendPath::Digest => "digest",
            SpendPath::AutoName => "auto_name",
        }
    }
}

/// The message every `max_cost_per_day` refusal carries, wherever it is
/// shown or logged. Pure: takes the numbers, not the ledger, so its test
/// needs no filesystem.
///
/// Names both amounts in dollars and says the cap resets at local midnight.
pub fn refusal_message(spent: f64, cap: f64) -> String {
    format!(
        "Daily spend cap reached: ${spent:.2} of the ${cap:.2} per-day limit. \
         No model calls will run until the cap resets at local midnight."
    )
}

/// One gate for every model call. Reads the ledger's `spent_today()` and
/// `[budget] max_cost_per_day`, decides with [`cap_status`], and on `Over`
/// returns the shared [`refusal_message`] plus — outside the gate, at the
/// one call site — nothing else: the single `budget_refused` event lives
/// here, so a refusal is traceable from the event log to this line.
///
/// `Ok(())` is the go-ahead every model call site needs before
/// `make_client`; `Err(message)` is the refusal to show or log.
pub fn cost_gate(config: &crate::config::Config, path: SpendPath) -> Result<(), String> {
    let spent = spent_today();
    let cap = config.budget.max_cost_per_day;
    match crate::config::cap_status(spent, cap) {
        crate::config::CapStatus::Off | crate::config::CapStatus::Under { .. } => Ok(()),
        crate::config::CapStatus::Over { spent, cap } => {
            let message = refusal_message(spent, cap);
            let fields = serde_json::json!({
                "path": path.label(),
                "spent_usd": spent,
                "cap_usd": cap,
            });
            crate::daemon::utils::log_event("budget_refused", fields);
            Err(message)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    #[test]
    fn ledger_seeds_once_per_day_and_adds() {
        let now = at(2026, 10, 21, 9, 30);
        let mut ledger = Ledger::new(at(2026, 10, 20, 9, 0).date_naive());
        let mut seeds = 0u32;
        assert_eq!(
            ledger.total_for(now, || {
                seeds += 1;
                1.5
            }),
            1.5,
            "the first read seeds from the event log"
        );
        ledger.add(0.25);
        ledger.add(0.25);
        assert_eq!(
            ledger.total_for(at(2026, 10, 21, 10, 0), || {
                seeds += 1;
                999.0
            }),
            2.0,
            "same-day reads keep the seed and the adds"
        );
        assert_eq!(seeds, 1, "the seed ran once for the day");
    }

    #[test]
    fn ledger_reseeds_on_a_new_day() {
        let mut ledger = Ledger::new(at(2026, 10, 21, 9, 0).date_naive());
        ledger.add(3.0);
        let next_day = at(2026, 10, 22, 0, 30);
        assert_eq!(
            ledger.total_for(next_day, || 0.75),
            0.75,
            "a new local day re-seeds from the event log, dropping the old total"
        );
    }

    #[test]
    fn local_day_bounds_start_at_local_midnight() {
        // The host TZ for the test run; midnight of "today" in that zone.
        let now = Local::now();
        let (from, to) = local_day_bounds(now);
        let from_local = from.with_timezone(&Local);
        assert_eq!(
            from_local.date_naive(),
            now.date_naive(),
            "the window starts on the same local date"
        );
        assert_eq!(
            from_local.time(),
            chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap(),
            "the window starts at local midnight"
        );
        assert!(from <= to, "midnight precedes now");
        // A fixed instant: the same date's midnight maps into UTC once.
        let probe = at(2026, 6, 15, 14, 0);
        let (from, to) = local_day_bounds(probe);
        assert_eq!(
            from.with_timezone(&Local).time(),
            chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap()
        );
        assert_eq!(to, probe.with_timezone(&Utc));
    }
}

#[cfg(test)]
mod cap_tests {
    use super::super::super::config::CapStatus;
    use super::super::super::config::cap_status;

    #[test]
    fn cap_status_is_off_at_zero_and_over_at_the_cap() {
        assert_eq!(cap_status(0.5, 0.0), CapStatus::Off, "cap 0 is off");
        assert_eq!(cap_status(0.5, -1.0), CapStatus::Off, "negative cap is off");
        assert_eq!(
            cap_status(2.0, 2.0),
            CapStatus::Over {
                spent: 2.0,
                cap: 2.0
            },
            "spent == cap is Over"
        );
        assert_eq!(
            cap_status(1.9999, 2.0),
            CapStatus::Under {
                spent: 1.9999,
                cap: 2.0
            },
            "just below the cap is Under"
        );
    }
}

#[cfg(test)]
mod first_record_tests {
    use super::tests::at;
    use super::*;

    #[test]
    fn the_first_record_after_a_seed_is_added() {
        // A cold record with the line on disk: the seed runs before the
        // write, so it reads the log without this call's line (0.00 here)
        // and the add lands on top — seed plus cost, exactly once.
        let mut ledger = Ledger::new(at(2026, 10, 20, 9, 0).date_naive());
        ledger.record_with_write(at(2026, 10, 21, 9, 0), 0.50, || {}, || 0.00);
        assert_eq!(
            ledger.total_for(at(2026, 10, 21, 9, 30), || 999.0),
            0.50,
            "the seed cannot hold a line written after it, so the record's cost \
             must be added on top: a seed plus the add, not the seed alone"
        );

        // A warm record adds the same way.
        ledger.record_with_write(at(2026, 10, 21, 9, 30), 0.25, || {}, || 999.0);
        assert_eq!(
            ledger.total_for(at(2026, 10, 21, 10, 0), || 999.0),
            0.75,
            "a same-day record adds its cost on top of the seed"
        );
    }
}

#[cfg(test)]
mod gate_tests {
    use super::tests::at;
    use super::*;

    #[test]
    fn refusal_names_spend_and_cap() {
        let message = refusal_message(2.50, 5.0);
        assert!(
            message.contains("$2.50"),
            "the refusal names the spend in dollars: {message}"
        );
        assert!(
            message.contains("$5.00"),
            "the refusal names the cap in dollars: {message}"
        );
        assert!(
            message.contains("midnight"),
            "the refusal says when the cap resets: {message}"
        );
    }

    #[test]
    fn seed_runs_before_the_line_is_written() {
        // The ledger-race rule: on a cold ledger, whatever seeds the ledger
        // runs before the `ai_cost` line is written, and the seed, the write
        // and the add happen while the ledger lock is held — so a gate read
        // between the two can never see the cost twice.
        let mut ledger = Ledger::new(at(2026, 10, 20, 9, 0).date_naive());
        let order = std::sync::Mutex::new(Vec::<&'static str>::new());
        let seed = || {
            order.lock().expect("the order log locks").push("seed");
            0.25
        };
        let write = || {
            order.lock().expect("the order log locks").push("write");
        };
        ledger.record_with_write(at(2026, 10, 21, 9, 0), 0.10, write, seed);
        assert_eq!(
            order.into_inner().expect("the order log locks").as_slice(),
            &["seed", "write"][..],
            "the seed runs first and the write after it — a seed read before \
             the write never holds this call's line, so the add is due"
        );
        assert_eq!(
            ledger.total_for(at(2026, 10, 21, 9, 30), || 999.0),
            0.35,
            "the total is the seed plus the cost, exactly once"
        );
    }

    #[test]
    fn a_failed_write_still_counts_its_cost() {
        let mut ledger = Ledger::new(at(2026, 10, 20, 9, 0).date_naive());
        ledger.record_with_write(at(2026, 10, 21, 9, 0), 0.10, || {}, || 0.0);
        assert_eq!(
            ledger.total_for(at(2026, 10, 21, 9, 30), || 999.0),
            0.10,
            "a line that never reached the log is invisible to the seed, so the \
             ledger must add its cost even across a seed"
        );
    }

    #[test]
    fn the_gate_refuses_over_the_cap_and_names_the_path() {
        let mut config = crate::config::Config::default();
        config.budget.max_cost_per_day = 1.0;
        assert_eq!(
            crate::config::cap_status(1.5, config.budget.max_cost_per_day),
            crate::config::CapStatus::Over {
                spent: 1.5,
                cap: 1.0
            },
            "spend over the configured cap is Over"
        );
        assert_eq!(SpendPath::Chat.label(), "chat");
        assert_eq!(SpendPath::Ghost.label(), "ghost");
        assert_eq!(SpendPath::Scheduled.label(), "scheduled");
        assert_eq!(SpendPath::Webhook.label(), "webhook");
        assert_eq!(SpendPath::Briefing.label(), "briefing");
        assert_eq!(SpendPath::Digest.label(), "digest");
        assert_eq!(SpendPath::AutoName.label(), "auto_name");
    }
}

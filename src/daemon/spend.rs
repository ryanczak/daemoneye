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
//! After the seed, every [`emit`](Ledger::add) accumulates on top.

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

/// Add one model call's cost to the ledger. Called by
/// [`crate::cost::emit_cost`] for every costed call.
pub fn record(cost: f64) {
    let now = Local::now();
    let mut guard = LEDGER.lock().unwrap_or_log();
    let ledger = guard.get_or_insert_with(|| seed_from_events(now));
    ledger.reseed_if_needed(now, || seed_from_events(now).total);
    ledger.add(cost);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Local> {
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

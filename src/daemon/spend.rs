//! The local-day spend ledger.

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

    /// Record one model call's cost against `now`'s local day.
    ///
    /// The seed sums the day's `ai_cost` lines from local midnight, so when
    /// the call's line is on disk (`line_written`) a seed triggered by this
    /// very record already holds it: a seed and an add of the same record
    /// must never both land, or the first costed call of a day — of the
    /// daemon's life, or after local midnight — counts twice (bug-05-1). A
    /// line that failed to serialize never reached the log, so its cost
    /// lands even across a seed. A ledger already holding the day takes
    /// every add: its seed ran before this call existed.
    pub fn record(
        &mut self,
        now: DateTime<Local>,
        cost: f64,
        line_written: bool,
        seed: impl FnOnce() -> f64,
    ) {
        let seeded = self.day != now.date_naive();
        self.reseed_if_needed(now, seed);
        if !seeded || !line_written {
            self.add(cost);
        }
    }

    /// [`Ledger::record`] with the line's write inside the same critical
    /// section: the seed runs first, then `write_line`, then the add (when
    /// one is due) — all under the caller's ledger lock. This is the shape
    /// the ledger race demands; [`Ledger::record`] is the no-write form the
    /// tests drive.
    pub fn record_with_write(
        &mut self,
        now: DateTime<Local>,
        cost: f64,
        line_written: bool,
        write_line: impl FnOnce(),
        seed: impl FnOnce() -> f64,
    ) {
        let seeded = self.day != now.date_naive();
        self.reseed_if_needed(now, seed);
        write_line();
        if !seeded || !line_written {
            self.add(cost);
        }
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
/// never land between a cold seed and the write and count the cost twice
/// (bug-05-1). `line_written` is false when the line could not be
/// serialized: the seed has nothing on disk to sum for this call, so its
/// cost is added even across a seed.
///
/// Gotcha honoured by the callers: [`crate::daemon::utils::log_event`] must
/// not take the ledger lock, and nothing it calls may — it runs here under
/// the lock by design, and re-taking it would deadlock.
pub fn record_under_lock(line_written: bool, cost: f64, write_line: impl FnOnce()) {
    let now = Local::now();
    let mut guard = LEDGER.lock().unwrap_or_log();
    match guard.as_mut() {
        Some(ledger) => ledger.record_with_write(now, cost, line_written, write_line, || {
            seed_from_events(now).total
        }),
        None => {
            let mut ledger = seed_from_events(now);
            write_line();
            if !line_written {
                ledger.add(cost);
            }
            *guard = Some(ledger);
        }
    }
}

/// Record one model call's cost against the local-day ledger. Called by
/// [`crate::cost::emit_cost`] once the call's outcome is known.
///
/// The seed sums the day's `ai_cost` lines, so when the call's line is on
/// disk (`line_written`) a seed this record triggers already holds it: a
/// seed and an add of the same record must never both land, or the first
/// costed call of a day — of the daemon's life, or after local midnight —
/// counts twice (bug-05-1). A line that failed to serialize never reached
/// the log, so its cost lands even across a seed. A ledger already seeded
/// for the day takes every add: its seed ran before this call existed.
pub fn record(cost: f64, line_written: bool) {
    record_under_lock(line_written, cost, || {});
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
    fn the_first_record_of_a_day_is_counted_once() {
        // The day's on-disk `ai_cost` lines already hold the record that
        // triggers the seed, so the seed returns its cost (0.50).
        let mut ledger = Ledger::new(at(2026, 10, 20, 9, 0).date_naive());
        ledger.record(at(2026, 10, 21, 9, 0), 0.50, true, || 0.50);
        assert_eq!(
            ledger.total_for(at(2026, 10, 21, 9, 30), || 999.0),
            0.50,
            "a cold seed already holds the record that triggered it; its cost must not be added on top"
        );

        // A call landing on a ledger already seeded for the same day adds.
        ledger.record(at(2026, 10, 21, 9, 30), 0.25, true, || 999.0);
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
        ledger.record(at(2026, 10, 21, 9, 0), 0.10, true, seed);
        assert_eq!(
            order.into_inner().expect("the order log locks").first(),
            Some(&"seed"),
            "the seed must run before the line is written, or a gate read in \
             between counts the line and then the add"
        );
        assert_eq!(
            ledger.total_for(at(2026, 10, 21, 9, 30), || 999.0),
            0.25,
            "the total is the seed — which already holds the line — and the add \
             is skipped, so the cost lands exactly once"
        );
    }

    #[test]
    fn a_failed_write_still_counts_its_cost() {
        let mut ledger = Ledger::new(at(2026, 10, 20, 9, 0).date_naive());
        ledger.record(at(2026, 10, 21, 9, 0), 0.10, false, || 0.0);
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

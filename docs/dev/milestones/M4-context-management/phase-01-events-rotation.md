# Phase 01: Event-log rotation and segment-aware readers

**Milestone:** M4 — Context Management Overhaul
**Status:** in-progress
**Depends on:** none
**Estimated diff:** ~500 lines
**Tags:** language=rust, kind=feature, size=l

## Goal

Stop `~/.daemoneye/var/events.jsonl` growing without bound (design defect
D4): write events to UTC-dated daily segment files, give every reader a
shared streaming helper that opens only the segments overlapping its time
window, and sweep old segments per a retention config. After this phase, no
code path loads the whole event history into memory.

## Architecture references

Read before starting:

- `docs/design/context-management.md#36-event-log-rotation` — the design this
  phase implements.
- `docs/design/context-management.md#2-failure-catalog` — defect D4.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Re-verify the **Current state** file:line anchors below against the
   working tree (`grep -rn "events_path()" src/`) — this doc was drafted at
   milestone kick-off.

## Current state

- `crate::config::events_path()` (`src/config/load.rs:63`) returns
  `~/.daemoneye/var/events.jsonl`.
- **Writer:** `log_event()` in `src/daemon/utils/event_log.rs:9-41` appends
  one JSON line per call to `events_path()`.
- **Readers** (grep-verified production sites; everything else matching
  `events_path()` is inside `mod tests`):
  1. `src/daemon/utils/event_log.rs:72` — `sum_cost_between(from, to)`:
     streams lines, filters `event == "ai_cost"` within `[from, to)`.
  2. `src/daemon/digest.rs:64` — `tally_events(session_id, since)`: reads the
     **whole file** with `read_to_string`, filters by `ts >= since`. This is
     the D4 hot spot.
  3. `src/daemon/stats.rs:471` — today's cost aggregation: streams lines,
     filters `ai_cost` within today's UTC window.
  4. `src/cli/commands/costs.rs:226` — `run_costs()`: streams lines, filters
     `ai_cost` within `[since_dt, until_dt)`.
  5. `src/search.rs:88` — `search_events()`: reads the last
     `EVENTS_TAIL_LINES` (10_000) lines, substring-matches.
- Several tests construct an `events.jsonl` at `events_path()` and then call
  a reader (e.g. `catchup_brief_includes_cost_when_ghosts_ran` in
  `src/daemon/server/catchup.rs`, tests in `costs.rs`, `stats.rs`,
  `digest.rs`). Tests that assert on the **write** path (e.g.
  `webhook_alert_to_event_log`) read `events_path()` back.

## Spec

### 1. Segment path helpers — in `src/config/load.rs`

Add alongside `events_path()`:

```rust
/// Directory holding dated event segments (`events-YYYYMMDD.jsonl`).
pub fn events_dir() -> PathBuf {
    var_dir().join("events")
}

/// The segment file that `log_event` writes to right now (today, UTC).
pub fn current_event_segment_path() -> PathBuf {
    events_dir().join(format!(
        "events-{}.jsonl",
        chrono::Utc::now().format("%Y%m%d")
    ))
}
```

(Adapt `var_dir()` to whatever helper `events_path()` itself uses — keep the
same parent directory conventions.)

### 2. Segment enumeration + streaming reader — in `src/daemon/utils/event_log.rs`

Add two public helpers:

```rust
/// All event files overlapping [from, to], oldest first.
/// The legacy `var/events.jsonl` (if present) is always first — it is
/// treated as the oldest segment and is never date-filtered (we cannot
/// know its content range from the filename).
/// `None` bounds mean unbounded on that side.
pub fn event_segment_paths_between(
    from: Option<chrono::DateTime<chrono::Utc>>,
    to: Option<chrono::DateTime<chrono::Utc>>,
) -> Vec<std::path::PathBuf>;

/// Stream every event line in [from, to] (parsed JSON) through `f`,
/// oldest segment first. Lines that fail to parse are skipped.
/// Per-line `ts` filtering still applies (segment granularity is a day;
/// the window is finer).
pub fn for_each_event_between(
    from: Option<chrono::DateTime<chrono::Utc>>,
    to: Option<chrono::DateTime<chrono::Utc>>,
    f: &mut dyn FnMut(&serde_json::Value),
);
```

Details:

- Segment date filtering: a segment `events-YYYYMMDD.jsonl` overlaps
  `[from, to]` iff its UTC day `[00:00, 24:00)` intersects the window.
  Filenames that don't match `events-\d{8}\.jsonl` are ignored.
- Line filtering inside `for_each_event_between`: parse `ts` with
  `chrono::DateTime::parse_from_rfc3339`; skip lines whose `ts` is outside
  the window; lines with missing/unparseable `ts` are **skipped** when a
  bound is set, **passed through** when both bounds are `None`.
- Use `std::io::BufReader` + `.lines()` — never `read_to_string` a whole
  file.

### 3. Writer switch — `log_event()` in `src/daemon/utils/event_log.rs`

Change the append target from `events_path()` to
`current_event_segment_path()`, creating `events_dir()` with
`std::fs::create_dir_all` on demand (mirror how the current code handles the
parent dir). The legacy `var/events.jsonl` is never written again but stays
readable in place — do **not** move or delete it.

### 4. Migrate the five readers (in this order; `cargo build` after each)

Each migration replaces "open `events_path()`, iterate lines" with a call to
`for_each_event_between` (or `event_segment_paths_between` where the read is
not a simple scan):

1. `sum_cost_between` (`event_log.rs`) — window `[from, to)`; keep the exact
   `ai_cost` filtering/aggregation logic, moved into the closure.
2. `tally_events` (`src/daemon/digest.rs:62-145`) — window `[since, None]`.
   This also removes the `read_to_string` (the D4 fix). Keep the existing
   per-event match arms verbatim inside the closure; keep the
   string-compare-on-rfc3339 `since` guard OR switch to parsed-`ts`
   comparison — pick one and be consistent with `for_each_event_between`'s
   own filtering (don't double-filter with different semantics).
3. `src/daemon/stats.rs:471` — window = today's UTC day.
4. `run_costs` (`src/cli/commands/costs.rs:226`) — window
   `[since_dt, until_dt)`. Note this runs in the **CLI process**, not the
   daemon — the helpers must not assume daemon-only state (they don't; they
   are pure fs reads).
5. `search_events` (`src/search.rs:88`) — different shape: it wants the last
   10_000 lines. Implement by iterating `event_segment_paths_between(None,
   None)` **newest first**, collecting lines until the cap, then restoring
   oldest-first order for display. Keep the existing match/context logic.

### 5. Retention sweep

- Config: add to `src/config/types.rs` a new `[events]` section:

  ```rust
  #[derive(Debug, Deserialize, Serialize, Clone)]
  pub struct EventsConfig {
      /// Delete dated event segments older than this many days.
      /// 0 = keep forever. The legacy `var/events.jsonl` is never deleted.
      #[serde(default = "default_events_retention_days")]
      pub retention_days: u32,
  }
  fn default_events_retention_days() -> u32 { 90 }
  ```

  Wire it into `Config` as `#[serde(default)] pub events: EventsConfig`
  following the existing pattern (see how `digest: DigestConfig` is declared
  at `src/config/types.rs:26`, with a `Default` impl).
- Sweep: in the existing `session-cleanup` supervised task in
  `src/daemon/mod.rs` (the 60 s loop at ~line 680 that prunes idle
  sessions), after the `store.retain(...)` block, add a call to a new
  `pub fn sweep_event_segments(retention_days: u32)` in `event_log.rs`:
  delete `events_dir()` files matching the segment pattern whose **filename
  date** is older than `retention_days` days before today (UTC). Skip when
  `retention_days == 0`. Log deletions at INFO. Run the sweep at most once
  per hour (a simple loop counter — the task ticks every 60 s).

### 6. Update write-path tests

Tests that assert `log_event` output by reading `events_path()` must read
`current_event_segment_path()` instead. Tests that **construct** fixture
events at `events_path()` and call a reader keep working unchanged (legacy
file = oldest segment) — do not rewrite them; they now double as
legacy-compat coverage.

## Acceptance criteria

- [ ] `cargo test` passes; `cargo clippy --all-targets --all-features -- -D warnings` clean.
- [ ] `log_event` writes to `var/events/events-<today>.jsonl`; nothing writes
      `var/events.jsonl` anymore (grep: no remaining production write path to
      `events_path()`).
- [ ] A fixture with events split across a legacy file and two dated
      segments is read in full, oldest-first, by `for_each_event_between(None,
      None, …)` (test below).
- [ ] `for_each_event_between` with a bounded window does **not** open
      segments outside the window (test below proves via a segment whose
      malformed content would panic/mis-count if opened — negative case).
- [ ] `tally_events` no longer contains `read_to_string`.
- [ ] `sweep_event_segments` deletes only dated segments older than the
      cutoff; never the legacy file; is a no-op at `retention_days == 0`.
- [ ] `daemoneye costs` output over a date range spanning legacy + dated
      segments matches the pre-phase output for the same fixture set.

## Test plan

All fs tests take `crate::TEST_HOME_LOCK` and set `HOME` to a tempdir
(existing idiom — see `catchup_brief_includes_cost_when_ghosts_ran` in
`src/daemon/server/catchup.rs:248` for the exact pattern, including the
`unsafe { std::env::set_var("HOME", …) }` + restore).

- `segments_enumerate_legacy_first` in `event_log.rs` — legacy file + two
  dated segments → paths returned oldest-first with legacy at index 0.
- `segments_window_excludes_out_of_range` — three dated segments; a window
  covering only the middle day returns exactly one path. **Negative case:**
  the out-of-range segment contains a line whose `ts` lies *inside* the
  window — it must still not be surfaced (segment filter is by filename
  date; we accept that boundary semantics are filename-based, and the test
  pins it).
- `for_each_streams_across_segments_in_order` — events land in `f` in file
  order, oldest segment first.
- `for_each_skips_unparseable_lines` — a garbage line among valid ones is
  skipped without error.
- `sum_cost_between_spans_segments` — costs split across legacy + dated
  files sum correctly for a window covering both.
- `tally_events_reads_dated_segments` in `digest.rs` — job_complete events
  in a dated segment are tallied.
- `sweep_deletes_only_expired_segments` — old dated segment deleted; recent
  segment and legacy file untouched; `retention_days = 0` deletes nothing.
- `log_event_writes_today_segment` — after `log_event`, today's segment
  exists and contains the record; `var/events.jsonl` was not created.

## End-to-end verification

With a real daemon build (no running tmux needed):

1. `HOME=$(mktemp -d) target/debug/daemoneye status` (or any command that
   triggers a `daemon_start`/`daemon_stop` event via a short daemon run) —
   then `ls $HOME/.daemoneye/var/events/` shows `events-<today>.jsonl`; quote
   the listing and one line of its content.
2. Seed a fake legacy `var/events.jsonl` plus the dated segment with `ai_cost`
   fixture lines, run `target/debug/daemoneye costs --json`, and quote the
   output showing both segments' costs aggregated.

## Authorizations

None. (No new dependencies — `chrono` is already in the tree.)

## Out of scope

- Do NOT compress segments (zstd is future work, design doc §8).
- Do NOT move/rename/delete the legacy `var/events.jsonl`.
- Do NOT change what events are logged or their schemas.
- Do NOT touch `digest.rs` beyond the `tally_events` read-path swap (the
  tally/digest redesign is phases 03–06).

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-07 17:35 (progress)

**Executor:** Claude executor. Starting phase 01: event-log rotation and segment-aware readers. Implementing segment path helpers, streaming reader, writer switch, reader migrations, retention sweep, and tests.

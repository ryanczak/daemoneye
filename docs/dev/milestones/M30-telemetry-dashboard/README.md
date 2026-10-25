# M30 — Telemetry & dashboard

**Goal:** every run leaves one record that says which model did what and
how well, the daemon can answer "which model handles which run class" from
those records, a daily USD cap stops the spend before the bill does, and
`daemoneye top` shows the live daemon read-only.

**Status:** in-progress (scoped 2026-10-20)

**Release:** 2.3.0

**Depends on:** M24 (run manager: `Run`, `RunResult`, briefings, turn log),
M29 (one session store).

**Sources:** `docs/design/daemoneye-2.0.md` § 2.3 (the cross-run telemetry
row), § 2.7 (`daemoneye top`) and § 6 (the M30 row). The rexyMCP port
anchors are `executor/src/agent/metrics.rs` (`RunMetrics`,
`emit_phase_run`), `executor/src/store/telemetry.rs` (`PhaseRun`,
`FAILURE_CLASSES`, O_APPEND single-write append), `mcp/src/scorecard.rs`
and `mcp/src/profile.rs` (`aggregate_scorecard`, `aggregate_profiles`,
`is_model_attributable`), and `mcp/src/dashboard/` (ratatui, 500 ms poll,
`DataFingerprint`, auto-follow). The governor (former M25) and the MCP
surface (M31) are **not** in scope.

**What exists today (survey 2026-10-20).** `src/daemon/run/` has `Run`,
`RunKind`, `RunState`, `RunResult`, `Briefing` and a `Budget` whose every
live construction is `Budget::unlimited()`. The turn log records
`run_started` / `turn_started` / `note` / `run_finished` only.
`CostRecord` (`src/cost.rs`) is emitted as an `ai_cost` event per model
call and has no `run_id`. There is no `[budget]` section, no daily cap,
and no per-run record, scorecard, profile or dashboard; `daemoneye run
list` rescans every turn log. The ratatui 0.30 / crossterm 0.29 stack is
already a dependency (`src/cli/render_ratatui.rs`, `viewer.rs`).

**PE decisions (2026-10-20):**

1. **Store:** the per-run record is an `event: "run"` record in the dated
   events segments (`var/log/events/events-YYYYMMDD.jsonl`), beside
   `ai_cost`. It inherits rotation, retention, `ts`/`pid` stamping and
   the segment readers. No new store. `CostRecord` gains `run_id`.
2. **`failure_class` is computed at run end**, from the terminal state
   and the briefing blocker; there is no human verdict step. The open
   vocabulary starts as `none`, `cancelled`, `budget_exceeded`,
   `backend_error`, `tool_errors`, `stall`, `parse_format`.
   `is_model_attributable` is false for `none`, `cancelled`,
   `budget_exceeded` and `backend_error`.
3. **Every run kind gets a record**, a chat session per turn.
4. **`[budget] max_cost_per_day` trips at every model call.** Over the
   cap: a chat turn is refused with a `SystemMsg` naming spend and cap, a
   ghost or scheduled run ends `BudgetExceeded` with a briefing, webhook
   watchdog analysis is skipped and logged. The day is local midnight.
   `0` (the default) is off.

**Exit criteria:**

- **Run record.** Every finished run of every kind appends one `run`
  event carrying `run_id`, `kind`, `agent`, `provider`, `model`,
  `status`, `failure_class`, `is_model_attributable`, `turns`,
  `tool_calls`, `tool_success_rate`, `parse_failure_rate`, `tokens`,
  `cost_usd`, `wall_clock_s` and `tags` (kind, agent, runbook, host).
  Every `ai_cost` event carries the `run_id` it belongs to. Old readers
  of both are unaffected (`#[serde(default)]` on every new field).
- **Scorecard.** `daemoneye scorecard` reads the events with the daemon
  down, buckets by model and one of `--by kind | agent | tag` (default
  `kind`), and every row carries `n_runs`; `--min-runs N` drops thin
  rows; `--since` / `--until` / `--json` as `daemoneye costs`. The
  columns are status pass rate, tool-success mean, parse-failure mean,
  turns mean, wall-clock mean, cost mean, and the model-attributable
  failure rate.
- **Profile.** `daemoneye profile <model>` shows the model's strengths
  per bucket and its failure classes ranked by count, `none` excluded,
  non-attributable classes marked. A model with no runs says so.
- **Daily cap.** With `[budget] max_cost_per_day` set and today's spend
  over it, decision 4 holds for all three paths, the refusal names the
  spend and the cap, and nothing is sent to a provider. `daemoneye
  status` shows spend against the cap. The ghost `Budget` records real
  tokens and USD, so a briefing's `budget_left` is true.
- **`daemoneye top`.** A read-only ratatui dashboard with Runs (state,
  turn, age, liveness), Budget (tokens and USD today against the cap,
  tok/s), Shells (live shells with foreground and last line) and
  Activity (the events tail, filterable). It polls at 500 ms, re-reads
  only when a stat changes, follows the newest live run until the user
  scrolls, and `q` quits. It opens no IPC write path; stopping a run is
  `/stop` or `daemoneye stop`.
- Live close on pinky per the M14 convention. `tests/doc_truth.rs`
  passes. `2.3.0` is released per `REXYMCP.md` § Versioning.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.3, § 2.7, § 6.
- `src/daemon/run/mod.rs`, `result.rs`, `turn_log.rs` — the run model.
- `src/cost.rs`, `src/daemon/stats.rs::compute_cost_today`,
  `src/cli/commands/costs.rs` — the cost path the record extends.
- `src/daemon/utils/event_log.rs` — the segment writer and readers.

## Phases

Drafted one at a time with `/rexymcp:architect next`. Tally first, then
the record, then its readers, then the cap, then the dashboard, then the
close. Each may split at drafting.

| # | Phase | Status |
|---|---|---|
| 01 | run-metrics ([phase-01-run-metrics.md](phase-01-run-metrics.md)) — a `RunMetrics` tally (tool calls and successes, parse failures, tokens, wall clock, model / served model) kept on the run and fed by the chat turn, ghost, scheduled and delegated loops; `CostRecord.run_id` | done |
| 02 | run-record ([phase-02-run-record.md](phase-02-run-record.md)) — the `run` event: the record type, `failure_class` computed from the terminal state and the tally, `is_model_attributable`, emitted by the run guard at every run end; the ghost `Budget` fed with real spend | done |
| 03 | scorecard ([phase-03-scorecard.md](phase-03-scorecard.md)) — pure aggregation over `run` events with `n_runs` and `min_runs`; model-less records skipped and counted; `daemoneye scorecard` | done |
| 04 | profile ([phase-04-profile.md](phase-04-profile.md)) — `daemoneye profile <model>`: strengths (scorecard rows) and failure classes ranked, non-attributable marked; scorecard table columns sized to content | done |
| 05 | cost-ledger ([phase-05-cost-ledger.md](phase-05-cost-ledger.md)) — every model call costed through one helper (the five uncosted side calls included), the local-day spend ledger, `[budget] max_cost_per_day` and `cap_status`, the `status` cap line | done |
| 06 | daily-cap ([phase-06-daily-cap.md](phase-06-daily-cap.md)) — the breaker at every model call: a chat turn refused with a `SystemMsg`, a ghost or scheduled run ended `BudgetExceeded`, webhook watchdog analysis skipped and logged, side calls skipped | done |
| 07 | top-data ([phase-07-top-data.md](phase-07-top-data.md)) — Runs, Budget and Activity from disk, the stats-only fingerprint, `daemoneye top --once`; no IPC | done |
| 08 | top-shells ([phase-08-top-shells.md](phase-08-top-shells.md)) — the Shells panel: `ShellInfo` gains `foreground` and `last_line`, `top --once` adds the section (daemon down ⇒ `daemon not running`) | done |
| 09 | top-tui ([phase-09-top-tui.md](phase-09-top-tui.md)) — the ratatui screen over the snapshot: 500 ms poll, re-read on fingerprint change, follow the newest live run until the user scrolls, activity filter, `q` quits | done |
| 10 | live-close ([phase-10-live-close.md](phase-10-live-close.md)) — architect rehearsals, the PE run, 2.3.0 | in-progress |
| 11 | top-rows ([phase-11-top-rows.md](phase-11-top-rows.md)) — what the rows say: a SUBJECT column in Runs (runbook ← trigger, job, session), local-time Activity, shaped summaries for the daemon's everyday events, consecutive repeats collapsed to one row `×N`, short shell owners, the rate labelled with its 60 s window | done        |
| 12 | top-layout ([phase-12-top-layout.md](phase-12-top-layout.md)) — the screen: a one-line header (daemon pid and uptime, model, host, local time; the down state named there), Runs and Budget side by side when the terminal is wide enough, Runs and Shells sized to content up to a cap with Activity taking the rest, a Budget bar coloured at 80 % and 100 % of the cap, finished runs dimmed after a few minutes | done       |
| 13 | top-focus ([phase-13-top-focus.md](phase-13-top-focus.md)) — one focused panel (Tab / Shift-Tab / `1`–`4`, named in the title and footer), per-panel scrolling for Runs, Shells and Activity, Activity tailing until scrolled with `f` resuming it | done       |
| 14 | top-detail ([phase-14-top-detail.md](phase-14-top-detail.md)) — Enter on a run opens its detail: the `run` record's fields when finished (failure class, tool calls and success rate, tokens, cost, wall clock) or the turn log's state so far, plus the turn log tail; scrollable; Esc returns; still read-only | done       |
| 10a | rehearsal-fixes ([phase-10a-rehearsal-fixes.md](phase-10a-rehearsal-fixes.md)) — rehearsal 1's F1 (chat runs write `turn_started`, so `top`/`run list` show their turns) and F2 (a finished waited command records `Foreground::Prompt`), plus the `run_tui` doc nit | done |
| 10b | title-padding ([phase-10b-title-padding.md](phase-10b-title-padding.md)) — the L8 F1 nit: every `top` title reads `╭ <title> ─`, one padding in one place (Runs, Shells, Activity and the detail view were doubled; Budget was not) | in-progress |

## Notes

- **`top` gets four UI phases before the close (PE decision 2026-10-23).**
  The PE run passed L1–L7 with no findings, and watching it showed `top`
  hard to read: a run row does not say what the run is, Activity is in UTC
  and mostly raw `key=value` dumps that one repeating event can fill, the
  panels have fixed heights that waste a tall terminal, nothing shows the
  cap as a bar, and there is no header, no panel focus, no scrolling of
  Shells or Activity and no way to open a run. Phases 11–14 fix that in
  order: the row text (pure, `--once` testable), the layout, focus and
  scrolling, then the detail view. Phase 10 then re-checks `top` live as
  L8 before the retrospective and 2.3.0. One idea was dropped: indenting
  a ghost under the scheduled run that started it needs a parent link the
  ghost's `trigger` word does not carry, so the SUBJECT column shows
  `<runbook> ← scheduled` instead.

- **`top` refreshes on change and on age (drafting 09, 2026-10-22).** The
  live loop re-reads the disk panels when the fingerprint changes, and
  also every 5 s, because run ages and the 30 s liveness window move
  with the clock even when no file does. Shells are asked for every 2 s.
- **Phase 07 stalled, so `top` is split again (escalation 07, 2026-10-21).**
  The first dispatch spent 120 turns reading and wrote nothing, mostly on
  the Shells panel. The Shells panel becomes phase 08, the TUI 09 and the
  live close 10. Phase 07 is re-scoped to the disk panels, in one file,
  with every symbol it needs named in the Spec.
- **`top` split in two (drafting 07, 2026-10-21).** Phase 07 builds the
  snapshot, fingerprint and a `--once` text frame, which can be tested
  end to end with the daemon down. Phase 08 puts the ratatui loop over
  it. The live close moves to 09.
- **Ledger race for phase 06 (review 05, 2026-10-21).** `emit_cost`
  writes the `ai_cost` line, then calls `spend::record`, outside one lock.
  A `spent_today()` that seeds a cold ledger between the two counts the
  line, and the add then counts it again. Nothing reads the ledger
  concurrently today. Phase 06's cap check at every model call does, so
  its Spec must make the write and the record one critical section, with
  a criterion.
- **Daily cap split in two (drafting 05, 2026-10-21).** Phase 05 makes
  every model call costed and keeps the local-day total; phase 06 refuses
  over the cap. `top` and the live close move to 07 and 08.
- **Five model calls are invisible to cost today (found drafting 01).**
  Briefing generation, the session digest, auto-naming and the webhook
  and scheduled watchdogs call `make_client(..).chat(..)` and emit no
  `ai_cost`. `daemoneye costs` has never seen them, and a daily cap that
  read only `ai_cost` would not either. The daily-cap phase must cost and
  gate them, or the cap's "nothing is sent to a provider" is false.
- **Scheduled records carry no model (found drafting 02).** A
  `Scheduled` run is the job wrapper: a scheduled ghost job's model calls
  land on its own `Ghost` run, and a script or alert job makes none that
  are costed. So a `scheduled` record has `model: null` and zero calls.
  The scorecard must bucket or skip model-less records rather than count
  them against a model.
- **`other` joins the failure vocabulary (02).** A `HardFail` with no
  backend error, parse failure or failed tool (a ghost internal error, a
  failed script job, a missed outcome) is `other`, not model-attributable.
  The PE's vocabulary was declared open; `stall` stays reserved for the
  governor.
- **No served model.** No backend reports the model that actually served
  a call, so the record carries the configured `provider` and `model`.
  Truncation reaches the loop only as a `Notice` string, so there is no
  length-finish rate either.
- **Not in scope.** A separate telemetry store; a human-assigned failure
  class; per-run USD caps in config; the governor's detectors; exposing
  the scorecard over MCP (M31); a Tasks panel (runbook `## Steps` were
  not shipped in M24).

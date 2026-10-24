# Phase 12: top-layout — the live screen's shape

**Milestone:** M30 — Telemetry & dashboard
**Status:** in-progress
**Depends on:** phase-11
**Estimated diff:** ~400 lines, about half of it tests
**Tags:** language=rust, kind=feature, size=m

## Goal

Phase 11 fixed what `top`'s rows say. The live screen still has fixed
panel heights (Runs 10, Budget 6, Shells 4, Activity the rest) that waste
a tall terminal and clip a busy Runs panel, no header naming the daemon,
no picture of spend against the cap, and finished runs that look as loud
as live ones. This phase changes the live screen only. `--once` output is
unchanged. Layout decisions go in pure functions so they test without a
terminal.

## Spec

**Files:** `src/cli/commands/top_tui.rs`, `src/cli/commands/top.rs`,
`src/cli/status.rs` (visibility of `fmt_uptime` only), `README.md` (the
`top` paragraph).

**Where things are.** You do not need to search for these.

| Need | Symbol |
|---|---|
| The frame and its fixed heights | `draw`, `draw_panel`, `runs_window`, `inner_height` in `top_tui.rs` |
| The live loop's 2 s daemon cadence | `run_tui`, `plan_refresh`, `SHELLS_INTERVAL` in `top_tui.rs`; `shells_panel` in `top.rs` |
| The snapshot and the budget numbers | `TopSnapshot`, `take_snapshot`, `BudgetPanel` (`ai_cost`, `cap`) in `top.rs` |
| Daemon pid, uptime, provider, model | `Response::DaemonStatus { pid, uptime_secs, provider, model, .. }` for `Request::Status` (`src/ipc.rs`), sent with `ipc_client::request_once` as `shells_panel` does |
| Uptime text | `fmt_uptime` in `src/cli/status.rs` (make it `pub(crate)` and reuse it) |
| Hostname | `crate::daemon::utils::host::daemon_hostname()` |
| Run age and liveness | `TopRunRow.age_s`, `TopRunRow.liveness`, `Liveness::Done` |

1. **Header line.** `TopSnapshot` gains `daemon: Option<DaemonHeader>`
   (`pid`, `uptime_secs`, `provider`, `model`), `None` when `Request::Status`
   gets no `DaemonStatus` back. It is fetched in `take_snapshot` and on the
   same 2 s cadence as the shells. A pure
   `header_text(daemon: Option<&DaemonHeader>, host: &str, now: DateTime<FixedOffset>) -> String`
   renders `daemoneye · pid <pid> · up <fmt_uptime> · <provider>/<model> · <host> · HH:MM:SS`,
   or `daemoneye · daemon not running · <host> · HH:MM:SS` when `None`. The
   header is the screen's first row, unbordered.
2. **Layout.** A pure
   `plan_layout(area: Rect, runs_lines: usize, budget_lines: usize, shells_lines: usize) -> PanelRects`
   returns one `Rect` each for header, runs, budget, shells, activity and
   footer. `*_lines` are body line counts; a panel's height is its lines
   plus 2 for the borders.
   - Runs is `min(runs_lines, 12) + 2` high. Shells is
     `min(shells_lines, 6) + 2`. Activity takes every remaining row.
   - When `area.width >= 120`, Runs and Budget share one row, Budget on the
     right and 40 columns wide, Runs the rest. The row is as high as the
     taller of the two. Below 120 the panels stack: Runs, Budget, Shells,
     Activity.
   - Every returned rect lies inside `area`, and a terminal too small for
     the minimums must not panic. Activity is the panel that shrinks first.
3. **Budget bar.** A pure
   `budget_bar(spent: f64, cap: f64, width: usize) -> Option<(String, BarLevel)>`,
   `None` when `cap <= 0`. The string is `[` + `width` cells of `█` filled
   and `░` empty + `] NN%`, the fill clamped to `width` when spent is over
   the cap but the percentage printed as it is. `BarLevel` is `Ok` under
   80 %, `Warn` from 80 % and `Over` from 100 %. The live Budget panel adds
   the bar as its last line, drawn yellow for `Warn` and red for `Over`.
   `budget_text` and `--once` do not change.
4. **Fade finished runs.** A pure `run_fades(row: &TopRunRow) -> bool` is
   true when `liveness == Done` and `age_s >= 300`. The Runs panel draws
   those lines with `Modifier::DIM`; the selected line stays reversed.
   `draw_panel` learns a per-line style; how is your choice.
5. **Footer** keeps its text and moves to the last row of `plan_layout`.
6. **Cleanups from the phase-11 review.**
   - `git rm e2e-m30-11.sh`. This phase's harness writes nothing to the
     repo; it runs from a scratch file.
   - `shells_text` gets its doc comment back directly above it.
     `short_owner` keeps only its own.
   - Fix the doc comments of `short_cmd` (it cuts a command, not a
     session id) and `str_field` / `num_field` (they read a top-level
     field; drop the nesting claim).
7. **README:** the live-view sentences of the `top` paragraph name the
   header, the side-by-side layout at 120 columns, the bar's two colours
   and the faded finished runs.

## Tests

`top.rs` for the pure functions, `top_tui.rs` for the screen tests.

- `header_names_pid_uptime_model_host_and_time` and
  `header_names_the_daemon_down`.
- `layout_puts_runs_and_budget_side_by_side_when_wide` (120 columns) and
  `layout_stacks_panels_when_narrow` (119 columns).
- `layout_sizes_runs_and_shells_to_content_up_to_a_cap`: 3 lines ⇒ height
  5; 40 lines ⇒ Runs 14 and Shells 8.
- `layout_gives_activity_the_rest`: on 80×40 Activity's height is the
  rows the other rects leave.
- `layout_survives_a_tiny_terminal`: 40×10 and 1×1 return rects inside
  the area without panicking.
- `budget_bar_levels_at_80_and_100_percent`: 79.9 ⇒ `Ok`, 80 ⇒ `Warn`,
  100 ⇒ `Over`, 150 ⇒ the fill is full and the text says `150%`, cap 0 ⇒
  `None`.
- `finished_runs_fade_after_five_minutes`: `Done` at 299 s ⇒ false, at
  300 s ⇒ true; `Live` and `Stale` at 3600 s ⇒ false.
- `screen_draws_the_header_line`: on an 80×24 `TestBackend`, row 0 holds
  `pid` and `up`; with `daemon: None` it holds `daemon not running`.
- `screen_colours_the_budget_bar`: spend at 85 % of the cap draws a bar
  cell with a yellow foreground.

The existing `screen_*` tests keep passing; update their expectations only
where they pinned the old fixed heights.

## Acceptance criteria

- [ ] Each new test ⇒ `test result: ok. 1 passed`.
- [ ] On the § End-to-end harness:
  - `once_order=Runs,Budget,Shells,Activity` and `once_rate=1`
    (preservation: `--once` is unchanged).
  - `script_gone=1` (today `0`).
  - `bad_docs=0` (today `3`).
  - `shells_doc=1` (today `0`).
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` clean,
  `cargo fmt --all --check` exit 0, `cargo test` ⇒ `failed=0`. Report the
  passed count; do not pin it.

## End-to-end verification

Run this harness from a scratch file outside the repo, for example
`/tmp/e2e-m30-12.sh`, and paste its output verbatim into your Update Log
entry. It fabricates no events, so it does not depend on the clock.

```sh
A=/tmp/e2e-m30-12.txt; : > "$A"
for t in header_names_pid_uptime_model_host_and_time header_names_the_daemon_down layout_puts_runs_and_budget_side_by_side_when_wide layout_stacks_panels_when_narrow layout_sizes_runs_and_shells_to_content_up_to_a_cap layout_gives_activity_the_rest layout_survives_a_tiny_terminal budget_bar_levels_at_80_and_100_percent finished_runs_fade_after_five_minutes screen_draws_the_header_line screen_colours_the_budget_bar; do cargo test "$t" 2>&1 | grep -E '^test result: ok. 1 passed' | head -1 | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
cargo build -q 2>/dev/null
H=$(mktemp -d); mkdir -p "$H/.daemoneye/var/log/events" "$H/.daemoneye/var/log/runs"
F=$(TZ=Etc/GMT+5 HOME="$H" ./target/debug/daemoneye top --once)
echo "once_order=$(echo "$F" | grep -xE 'Runs|Budget|Shells|Activity' | paste -sd, -)" >> "$A"
echo "once_rate=$(echo "$F" | grep -c '^rate (60 s): ')" >> "$A"
find "$H" -type f -delete; find "$H" -depth -type d -empty -delete
echo "script_gone=$(test -e e2e-m30-11.sh && echo 0 || echo 1)" >> "$A"
echo "bad_docs=$(grep -cE 'A session id cut to|through one level of nesting' src/cli/commands/top.rs)" >> "$A"
echo "shells_doc=$(grep -B1 '^pub(crate) fn shells_text' src/cli/commands/top.rs | head -1 | grep -c '^///.*draws the same body')" >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

The live screen itself is checked by eye in phase 10's L8 pass, after
phase 14.

## Authorizations

- Add `daemon` to `TopSnapshot` and update every constructor, including
  the `top_tui.rs` and `top.rs` tests.
- Make `fmt_uptime` in `src/cli/status.rs` `pub(crate)`; change nothing
  else in that file.
- Change `draw_panel`'s signature as item 4 needs.
- Delete `e2e-m30-11.sh`.

## Out of scope

- Panel focus, per-panel scrolling and Activity tailing (phase 13).
- The run detail view (phase 14).
- Any change to `--once` output or `budget_text`.
- Any write to the daemon; `top` stays read-only.

## Notes for executor

Keep reasoning short: emit a tool call early in each turn. Do not wait on
the wall clock or sleep; nothing in this phase needs it.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-24 15:19 (started, executor)

Executor at work: header line, layout planner, budget bar, run fades, footer, phase-11 cleanups, README.

### Update — 2026-10-24 15:55 (end-to-end verification)

```text
header_names_pid_uptime_model_host_and_time: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
header_names_the_daemon_down: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
layout_puts_runs_and_budget_side_by_side_when_wide: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
layout_stacks_panels_when_narrow: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
layout_sizes_runs_and_shells_to_content_up_to_a_cap: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
layout_gives_activity_the_rest: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
layout_survives_a_tiny_terminal: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
budget_bar_levels_at_80_and_100_percent: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
finished_runs_fade_after_five_minutes: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
screen_draws_the_header_line: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
screen_colours_the_budget_bar: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
once_order=Runs,Budget,Shells,Activity
once_rate=1
script_gone=1
bad_docs=0
shells_doc=1
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2529 failed=0
```

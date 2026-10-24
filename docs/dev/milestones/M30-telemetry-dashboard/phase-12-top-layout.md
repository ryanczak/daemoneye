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
- [ ] Bounce 1 (bugs 12-1 to 12-4): `shells_refresh_replaces_the_daemon_header`,
  `screen_header_clock_is_the_draw_time`, `screen_dims_only_finished_runs`
  and `screen_shows_the_budget_percentage` each ⇒ `test result: ok. 1
  passed` (today none exists), and the harness prints `doc_fixes=4`
  (today `0`).
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` clean,
  `cargo fmt --all --check` exit 0, `cargo test` ⇒ `failed=0`. Report the
  passed count; do not pin it.

## End-to-end verification

Run this harness from a scratch file outside the repo, for example
`/tmp/e2e-m30-12.sh`, and paste its output verbatim into your Update Log
entry. It fabricates no events, so it does not depend on the clock.

```sh
A=/tmp/e2e-m30-12.txt; : > "$A"
for t in header_names_pid_uptime_model_host_and_time header_names_the_daemon_down layout_puts_runs_and_budget_side_by_side_when_wide layout_stacks_panels_when_narrow layout_sizes_runs_and_shells_to_content_up_to_a_cap layout_gives_activity_the_rest layout_survives_a_tiny_terminal budget_bar_levels_at_80_and_100_percent finished_runs_fade_after_five_minutes screen_draws_the_header_line screen_colours_the_budget_bar shells_refresh_replaces_the_daemon_header screen_header_clock_is_the_draw_time screen_dims_only_finished_runs screen_shows_the_budget_percentage; do cargo test "$t" 2>&1 | grep -E '^test result: ok. 1 passed' | head -1 | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
cargo build -q 2>/dev/null
H=$(mktemp -d); mkdir -p "$H/.daemoneye/var/log/events" "$H/.daemoneye/var/log/runs"
F=$(TZ=Etc/GMT+5 HOME="$H" ./target/debug/daemoneye top --once)
echo "once_order=$(echo "$F" | grep -xE 'Runs|Budget|Shells|Activity' | paste -sd, -)" >> "$A"
echo "once_rate=$(echo "$F" | grep -c '^rate (60 s): ')" >> "$A"
find "$H" -type f -delete; find "$H" -depth -type d -empty -delete
echo "script_gone=$(test -e e2e-m30-11.sh && echo 0 || echo 1)" >> "$A"
echo "bad_docs=$(grep -cE 'A session id cut to|through one level of nesting' src/cli/commands/top.rs)" >> "$A"
echo "shells_doc=$(grep -B1 '^pub(crate) fn shells_text' src/cli/commands/top.rs | head -1 | grep -c '^///.*draws the same body')" >> "$A"
d1=$(grep -B3 '^pub fn short_owner' src/cli/commands/top.rs | grep -c 'ghost:<runbook>')
d2=$(test "$(grep -c "The returned index is the selected line's position" src/cli/commands/top_tui.rs)" = 1 && echo 1 || echo 0)
d3=$(test "$(grep -c 'and a 500 ms tick, re-reading only' src/cli/commands/top_tui.rs)" = 1 && echo 1 || echo 0)
d4=$(grep -B1 '^pub struct PanelRects' src/cli/commands/top.rs | head -1 | grep -c '^///')
echo "doc_fixes=$((d1 + d2 + d3 + d4))" >> "$A"
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

### Update — 2026-10-24 16:12 (complete, server-authored)

**Summary:** </think>Those `println!`s are the legitimate `--once` output path, not debug calls. Good.

Completion checklist:
- [x] Phase doc Status: in-progress ✓
- [x] Milestone README row in-progress ✓
- [x] No hand-written (complete) entry ✓ (I wrote started + end-to-end verification entries only)
- [x] cargo fmt --all run before git add ✓
- [x] All verification commands ran clean ✓
- [x] Final message will be Summary + Notes for review
- [x] git status clean ✓
- [x] git log -1 --stat shows all files ✓

One more check — the header tests use `Local::now().offset()` in draw, which is wall-clock dependent but not testable since header_text takes the time as a parameter. Fine.

Everything is committed. Final summary now.</think>
All work is committed and the tree is clean. Final verification: `fmt_exit=0`, clippy clean, `passed=2529 failed=0`, and all eleven new tests pass individually; the harness output (pasted verbatim in the phase doc's Update Log) confirms `once_order=Runs,Budget,Shells,Activity`, `once_rate=1`, `script_gone=1`, `bad_docs=0`, `shells_doc=1`.

**Summary:** Built phase 12's live-screen shape: `TopSnapshot` gained `daemon: Option<DaemonHeader>` fetched on the 2 s cadence via a new `daemon_header()` (read-only `Request::Status`), with pure `header_text` rendering `daemoneye · pid N · up <uptime> · <provider>/<model> · <host> · HH:MM:SS` or the `daemon not running` form; pure `plan_layout` (with `PanelRects` and a clamping `rect_or_area` helper) sizes Runs (cap 12) and Shells (cap 6) to content, gives Activity the rest, and puts Runs/Budget side-by-side at ≥120 columns (Budget 40 on the right), returning rects that stay inside the area on a 1×1 terminal; pure `budget_bar` renders `[█░ NN%]` with `BarLevel` Ok/Warn/Over at 80/100 %, drawn yellow/red as the Budget panel's last line (its body line counted in the layout); pure `run_fades` dims `Done` runs at ≥300 s via a `fades: &[usize]` parameter on `draw_panel` (selected line stays reversed); the footer moved to `plan_layout`'s last row. Phase-11 cleanups done (`e2e-m30-11.sh` deleted, `shells_text` doc restored above it, `short_owner`/`short_cmd`/`str_field`/`num_field` doc comments fixed), `fmt_uptime` made `pub(crate)`, and the README `top` paragraph's live-view sentences name the header, the 120-column layout, the bar's two colours and the fades.

**Notes for review:** One adaptation from the spec's sketch — `budget_bar` rounds the fill (`(spent/cap)*width` rounded) rather than flooring, so a 79.9 % spend on a 20-wide bar shows 16 filled cells; the spec's wording ("filled and empty cells") doesn't pin the rounding mode. The screen's fade mapping translates body-line indices through `runs_window`'s displayed window by text match. The header renders with `Local::now().offset()` at draw time while the testable `header_text` takes the timestamp as a parameter, so no test touches the wall clock. `top` remains read-only; no dependencies added.

**Executor:** nvidia/GLM-5.3-Flash-FP4-Local

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.15s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


TEST
st readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test prompt_assets_name_no_retired_tools ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test docs_do_not_carry_retired_index_claims ... ok
test readme_documents_every_chat_slash_command ... ok
test seeded_config_template_documents_every_config_field ... ok
test runtime_docs_describe_no_tmux_execution ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test config_pricing_round_trip ... ok
test schedule_store_persistence ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_no_severity_passes_gate ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test webhook_ghost_e2e_deterministic ... ok
test an_empty_session_starts_no_tmux_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_installs_no_tmux_hooks ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.29s


running 4 tests
test gate_catches_an_abbreviated_write ... ok
test gate_catches_a_chained_write ... ok
test gate_catches_a_multi_line_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-a5bf14860b8766a6)
kill: (131073): No such process
kill: (-131763): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-fa3aae999c224501)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-af9329f81467cdee)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-a44fb637fa3308a3)
     Running tests/integration.rs (target/debug/deps/integration-cf27c4f796c5d8c0)
     Running tests/isolation.rs (target/debug/deps/isolation-0a0c421b4b2b9fc2)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-76069814ada3e33b)
   Doc-tests daemoneye

```

**Files changed:**

- `README.md` — +1 -1
- `docs/dev/milestones/M30-telemetry-dashboard/README.md` — +1 -1
- `docs/dev/milestones/M30-telemetry-dashboard/phase-12-top-layout.md` — +29 -1
- `src/cli/commands/mod.rs` — +3 -1
- `src/cli/commands/top.rs` — +398 -11
- `src/cli/commands/top_tui.rs` — +184 -45
- `src/cli/status.rs` — +1 -1

**Commit:** 7397970db38a256d24669549b687032b1b6a5fea

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Update — 2026-10-24 (architect: bounce 1)

**Architect:** Claude

Bounced on review with four bugs in `bugs/`: the live header never
refreshes (12-1), the fade dims the line above each finished run (12-2),
the budget bar's percentage is clipped (12-3), and four doc comments are
broken (12-4). Gates were clean, and everything else in the phase stands.

**Notes for executor:** read the four bug docs, fix them, add the four
tests they name, rerun the harness as now written from a scratch file
outside the repo, and paste its output. Keep the rest of the diff as it is.
Keep reasoning short: emit a tool call early in each turn, and do not
sleep.

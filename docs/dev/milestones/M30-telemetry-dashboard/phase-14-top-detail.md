# Phase 14: top-detail — open a run

**Milestone:** M30 — Telemetry & dashboard
**Status:** done
**Depends on:** phase-13
**Estimated diff:** ~450 lines, more than half of it tests
**Tags:** language=rust, kind=feature, size=m

## Goal

`top` lists runs but cannot say why one failed, what it cost or what it
did last. This phase adds a detail view. In the live screen, `Enter` on a
run replaces the panels with that run's detail, and `Esc` goes back. For a
finished run the detail shows the `run` record's fields. For any run it
shows the end state and the tail of the turn log. `top --once --run <id>`
prints the same text, which is how the harness checks it against real
files. `top` stays read-only.

## Spec

**Files:** `src/cli/commands/top.rs`, `src/cli/commands/top_tui.rs`,
`src/main.rs` (the `Top` arguments only), `README.md` (the `top`
paragraph and table rows).

**Where things are.**

| Need | Symbol |
|---|---|
| The finished run's numbers | `RunRecord` (`src/daemon/run/record.rs`): `status`, `failure_class`, `is_model_attributable`, `provider`, `model`, `turns`, `tool_calls`, `tool_success_rate`, `parse_failure_rate`, `tokens` (four buckets), `cost_usd`, `wall_clock_s`. It is the `run` event in the event log; `run_records_between` in `scorecard.rs` reads them |
| The turn log | `read_turn_log`, `TurnLogRecord { timestamp, event }`, `TurnLogEvent::{RunStarted, TurnStarted, Note, RunFinished}` |
| How a run ended | `RunState`: `HardFail` and `BudgetExceeded` carry a `Briefing` (`goal`, `blocker`, `budget_left`); `Cancelled { stage, turns }` |
| A run's start time | `run_started_at` in `top.rs` |
| Masking | `crate::ai::filter::mask_sensitive` |
| Live screen pieces | `TopModel`, `handle_key`, `layout_at`, `after_refresh`, `visible_range`, `draw_at` in `top_tui.rs` |

1. **Detail text.** A pure
   `run_detail_text_in(row: &TopRunRow, record: Option<&RunRecord>, log: &[TurnLogRecord], offset: FixedOffset) -> String`,
   and `run_detail_text`, which passes the local zone. In order:
   - A heading line: id, kind, subject, state and age, as the Runs row
     has them.
   - With a record: `status`, `failure`, which is the class plus
     `(not the model)` when `is_model_attributable` is false, `model` as
     `<provider>/<model>`, `turns`, `tool calls` as `<n> · <pct>% ok`, with
     `-` in place of the rate when it is `None`, `parse failures` as
     `<pct>%` or `-`, `tokens` as `<in> in · <out> out · <cache read> cache
     read · <cache write> cache write`, `cost` as `$` with 4 decimals, and
     `wall clock` in the same form as the Runs AGE column. A `None` rate
     never prints `NaN`.
   - Without a record: `no run record yet` and the turns the log shows.
   - When the last `RunFinished` is `HardFail` or `BudgetExceeded`: the
     briefing's `goal`, `blocker` and `budget left`, each skipped when
     absent. When it is `Cancelled`: its stage and turns.
   - `turn log` and its last 200 records, oldest first. Each is
     `HH:MM:SS` in `offset`, then `started <kind label>`, `turn <n>`,
     `note <text>` or `finished <state word>`. A note's later lines follow
     it indented by two spaces. Every note is masked.
2. **Loading.** `load_run_detail(id: &RunId) -> Option<RunDetail>` reads
   that run's turn log, and its `run` record from the event log, searching
   from the run's start time to now. `None` when the run has no turn log
   on disk.
3. **`top --once --run <id>`** prints the detail text for `<id>` and exits
   0. An id with no turn log prints `run <id> not found` to stderr and
   exits 1. `--run` without `--once` opens the live screen on that run's
   detail.
4. **Live screen.**
   - `Enter` with Runs focused and at least one run opens the selected
     run's detail. `Enter` elsewhere does nothing. While the filter is
     being edited, `Enter` ends editing as it does today.
   - The detail view draws the header line, one bordered panel titled
     ` Run <id> ` over the detail text, and a footer with
     `esc back · ↑↓ scroll · q quit`.
   - The scroll keys from the panels work here: `↑`/`k`, `↓`/`j`,
     `PageUp`, `PageDown`, `Home`, `End`. The view opens at the top, and
     offsets clamp through `visible_range`. `Tab`, `1`–`4`, `f` and `/` do
     nothing here.
   - `Esc` returns to the panels with focus, selection and every panel
     offset as they were before. `q` and `Ctrl-C` still quit.
   - While the detail is open, each disk refresh reloads it and keeps the
     scroll offset, clamped. A run that vanished draws
     `run <id> is no longer on disk` instead of panicking.
   - The detail panel's inner height comes from `layout_at`. `plan_layout`
     keeps its single call site, and the loop's refresh tail stays in
     `after_refresh`.
5. **README:** name `Enter` and `Esc`, what the detail shows, and
   `--once --run <id>`.

**Gotchas.** Phases 12 and 13 bounced on cases no test drew. Two of them
carry over here. A fix that must change the live loop has to show up in
`run_tui` itself, and the harness checks it. A `usize` subtraction near a
window's top underflowed once already. Write the named tests below first,
and make each fail against the current tree before you write the code.

## Tests

`top.rs` for the text, `top_tui.rs` for keys and screens. Text tests build
a `RunRecord` and `TurnLogRecord`s in memory and pass a fixed offset.

- `detail_text_names_the_record_fields`: every field in item 1, with
  `12 · 91.7% ok` and `$0.0123`.
- `detail_text_without_tool_calls_prints_dashes`: `tool_calls` 0 and both
  rates `None` give `-` and no `NaN`.
- `detail_text_for_a_running_run_has_no_record`.
- `detail_text_names_the_briefing_and_the_cancel`: a `HardFail` with goal
  and blocker, a `BudgetExceeded` without a blocker, and a `Cancelled`.
- `detail_tail_keeps_the_last_200_records`: 300 records show record 101
  first and 300 last.
- `detail_masks_notes_and_indents_their_lines`: a two-line note holding
  `AKIAIOSFODNN7EXAMPLE` shows `<AWS_KEY` and not `AKIA`, with its second
  line indented.
- `detail_times_use_the_given_zone`: one record at `14:36:20Z` with
  `FixedOffset::west_opt(5 * 3600)` prints `09:36:20`.
- `enter_opens_the_selected_runs_detail`, and
  `enter_does_nothing_off_runs_with_no_runs_or_while_editing`.
- `esc_restores_focus_selection_and_offsets`.
- `detail_scroll_clamps_at_both_ends`: 50 lines, inner height 10. `End`
  gives 40, `↓` stays 40, one `↑` gives 39. With 3 lines, `End` gives 0.
- `detail_refresh_keeps_the_scroll_and_survives_a_vanished_run`.
- `panel_keys_do_nothing_in_the_detail`: `Tab`, `3`, `f` and `/` change
  neither the focus nor the filter state.
- `screen_draws_the_detail_view`: 80×24 shows ` Run <id> ` and
  `esc back`, and no ` 1 Runs ` title.
- `screen_detail_on_a_tiny_terminal`: 40×6 and 1×1 draw without a panic.

## Acceptance criteria

- [ ] Each test above ⇒ `test result: ok. 1 passed` (today none exists).
- [ ] On the § End-to-end harness: `detail_status=1`, `detail_failure=1`,
  `detail_tools=1`, `detail_cost=1`, `detail_blocker=1`,
  `detail_masked=1`, `detail_leak=0`, `detail_time=1` and
  `missing_exit=1` (today the `--run` flag does not exist and each reads
  `0`; `detail_leak` is `0` today and stays `0`).
- [ ] Preservation: `once_order=Runs,Budget,Shells,Activity`,
  `layout_inputs=1`, `loop_clamps=0`, and
  `requests=Request::ShellList,Request::Status`, so `top` sends no new
  request to the daemon.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` clean,
  `cargo fmt --all --check` exit 0, `cargo test` ⇒ `failed=0`. Report the
  passed count; do not pin it.

## End-to-end verification

Run this from a scratch file outside the repo, for example
`/tmp/e2e-m30-14.sh`, and paste its output verbatim into your Update Log
entry. Its events are dated in the past and it runs under a fixed-offset
zone, so it does not depend on the clock.

```sh
A=/tmp/e2e-m30-14.txt; : > "$A"
for t in detail_text_names_the_record_fields detail_text_without_tool_calls_prints_dashes detail_text_for_a_running_run_has_no_record detail_text_names_the_briefing_and_the_cancel detail_tail_keeps_the_last_200_records detail_masks_notes_and_indents_their_lines detail_times_use_the_given_zone enter_opens_the_selected_runs_detail enter_does_nothing_off_runs_with_no_runs_or_while_editing esc_restores_focus_selection_and_offsets detail_scroll_clamps_at_both_ends detail_refresh_keeps_the_scroll_and_survives_a_vanished_run panel_keys_do_nothing_in_the_detail screen_draws_the_detail_view screen_detail_on_a_tiny_terminal; do cargo test "$t" 2>&1 | grep -E '^test result: ok. 1 passed' | head -1 | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
cargo build -q 2>/dev/null
H=$(mktemp -d); mkdir -p "$H/.daemoneye/var/log/events" "$H/.daemoneye/var/log/runs"
T=$(date -u -d '2 minutes ago' +%Y-%m-%dT%H:%M:%SZ); T2=$(date -u -d '1 minute ago' +%Y-%m-%dT%H:%M:%SZ)
D=$(TZ=Etc/GMT+5 date -d "$T2" +%Y%m%d); L=$(TZ=Etc/GMT+5 date -d "$T" +%H:%M:%S)
R="$H/.daemoneye/var/log/runs/7eb9151d45fa49f1.jsonl"
cat > "$R" <<EOF
{"timestamp":"$T","run_id":"7eb9151d45fa49f1","event_type":"run_started","kind":{"Ghost":{"runbook":"disk-full-l4","trigger":"webhook"}}}
{"timestamp":"$T","run_id":"7eb9151d45fa49f1","event_type":"turn_started","turn":1}
{"timestamp":"$T","run_id":"7eb9151d45fa49f1","event_type":"note","text":"found key AKIAIOSFODNN7EXAMPLE in env"}
{"timestamp":"$T2","run_id":"7eb9151d45fa49f1","event_type":"run_finished","state":{"HardFail":{"goal":"free disk on /var","criteria":null,"working_files":[],"attempts":[],"blocker":"df still at 99%","budget_left":null}}}
EOF
echo "{\"ts\":\"$T2\",\"event\":\"run\",\"pid\":1,\"run_id\":\"7eb9151d45fa49f1\",\"kind\":\"ghost\",\"agent\":null,\"provider\":\"ollama\",\"model\":\"glm-test\",\"status\":\"hard_fail\",\"failure_class\":\"stall\",\"is_model_attributable\":true,\"turns\":7,\"tool_calls\":12,\"tool_success_rate\":0.9166,\"parse_failure_rate\":0.0,\"tokens\":{\"input_tokens\":1000,\"output_tokens\":234,\"cache_read_tokens\":0,\"cache_write_tokens\":0},\"cost_usd\":0.0123,\"wall_clock_s\":83.0,\"tags\":{}}" >> "$H/.daemoneye/var/log/events/events-$D.jsonl"
F=$(TZ=Etc/GMT+5 HOME="$H" ./target/debug/daemoneye top --once --run 7eb9151d45fa49f1 2>&1)
echo "detail_status=$(echo "$F" | grep -c 'hard_fail')" | sed 's/=[1-9][0-9]*$/=1/' >> "$A"
echo "detail_failure=$(echo "$F" | grep -c 'stall')" | sed 's/=[1-9][0-9]*$/=1/' >> "$A"
echo "detail_tools=$(echo "$F" | grep -c '12 · 91.7% ok')" >> "$A"
echo "detail_cost=$(echo "$F" | grep -c '\$0.0123')" >> "$A"
echo "detail_blocker=$(echo "$F" | grep -c 'df still at 99%')" >> "$A"
echo "detail_masked=$(echo "$F" | grep -c '<AWS_KEY')" >> "$A"
echo "detail_leak=$(echo "$F" | grep -c 'AKIA')" >> "$A"
echo "detail_time=$(echo "$F" | grep -c "^$L started ghost")" >> "$A"
TZ=Etc/GMT+5 HOME="$H" ./target/debug/daemoneye top --once --run 0000000000000000 >/dev/null 2>&1; echo "missing_exit=$?" >> "$A"
F=$(TZ=Etc/GMT+5 HOME="$H" ./target/debug/daemoneye top --once)
echo "once_order=$(echo "$F" | grep -xE 'Runs|Budget|Shells|Activity' | paste -sd, -)" >> "$A"
find "$H" -type f -delete; find "$H" -depth -type d -empty -delete
echo "layout_inputs=$(sed '/^#\[cfg(test)\]/q' src/cli/commands/top_tui.rs | grep -c 'plan_layout(')" >> "$A"
echo "loop_clamps=$(sed -n '/^pub async fn run_tui/,/^}/p' src/cli/commands/top_tui.rs | grep -c 'clamp_offsets()')" >> "$A"
echo "requests=$(grep -ohE 'Request::[A-Za-z]+' src/cli/commands/top.rs src/cli/commands/top_tui.rs | sort -u | paste -sd, -)" >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

The reviewer smoke-tests `Enter` and `Esc` on the live screen against real
runs. Phase 10's L8 pass then checks `top` as a whole by eye.

## Authorizations

- Add `--run <id>` to the `Top` arguments in `src/main.rs` and thread it
  through `run_top`.
- Add fields to `TopModel` for the open detail and its scroll, and update
  every constructor in tests.
- Add an argument to `run_tui` for the run to open on.

## Out of scope

- Any action on a run: stopping, resuming or deleting it stays with
  `/stop`, `daemoneye stop` and `daemoneye run resume`.
- Following a running run's log live at its tail; the detail reloads on
  the disk refresh and keeps its scroll.
- Mouse input, and any change to the panels' layout or text.

## Notes for executor

Keep reasoning short: emit a tool call early in each turn. Do not wait on
the wall clock or sleep; nothing in this phase needs it.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-25 13:35 (progress)

Started phase 14. Wrote the 15 named tests first against the current tree and confirmed each fails to compile or fails outright (no `run_detail_text`, no `RunDetail`, no `--run` flag yet), then began the implementation: detail text in `top.rs`, loading and the `--run` flag, then the live screen in `top_tui.rs`.

### Update — 2026-10-25 13:36 (progress)

**Executor note on status:** the phase doc and the milestone README already said `in-progress` when I arrived (the prompt echoed `todo`; the tree held `in-progress` — the previous run's start flip, committed in `b8a032bf`'s neighbourhood). I left both as they are, appended this started entry, and am proceeding with the code.

### Update — 2026-10-25 14:05 (progress)

Detail text shipped in `top.rs`: `run_detail_text_in` / `run_detail_text`, `load_run_detail` (turn log + the run's `run` record from the event segments, bounded by the log's start), the heading row built from the log (`detail_row`), log-time rendering in the given zone, note masking and two-space indent, the last-200 tail, and the `--run <id>` CLI path in `run_top` (`--once` prints the text and exits 0; no turn log prints `run <id> not found` to stderr and exits 1). The eight text tests pass. Next: the live screen in `top_tui.rs`.

### Update — 2026-10-25 14:40 (end-to-end verification)

Ran the phase doc's E2E block verbatim (`sh /tmp/e2e-m30-14.sh`), writing to `/tmp/e2e-m30-14.txt`. Verbatim output:

```
detail_text_names_the_record_fields: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
detail_text_without_tool_calls_prints_dashes: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
detail_text_for_a_running_run_has_no_record: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
detail_text_names_the_briefing_and_the_cancel: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
detail_tail_keeps_the_last_200_records: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
detail_masks_notes_and_indents_their_lines: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
detail_times_use_the_given_zone: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
enter_opens_the_selected_runs_detail: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
enter_does_nothing_off_runs_with_no_runs_or_while_editing: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
esc_restores_focus_selection_and_offsets: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
detail_scroll_clamps_at_both_ends: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
detail_refresh_keeps_the_scroll_and_survives_a_vanished_run: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
panel_keys_do_nothing_in_the_detail: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
screen_draws_the_detail_view: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
screen_detail_on_a_tiny_terminal: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
detail_status=1
detail_failure=1
detail_tools=1
detail_cost=1
detail_blocker=1
detail_masked=1
detail_leak=0
detail_time=1
missing_exit=1
once_order=Runs,Budget,Shells,Activity
layout_inputs=1
loop_clamps=0
requests=Request::ShellList,Request::Status
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2568 failed=0
```

The real detail frame the block's fixture produced (re-run of the block's binary invocation, quoted from that run):

```
=== detail frame ===
7eb9151d45fa49f1  ghost  disk-full-l4 ← webhook  hard-fail  2m
status: hard_fail
failure: stall
model: ollama/glm-test
turns: 7
tool calls: 12 · 91.7% ok
parse failures: 0.0%
tokens: 1000 in · 234 out · 0 cache read · 0 cache write
cost: $0.0123
wall clock: 1m
goal: free disk on /var
blocker: df still at 99%
turn log
10:31:42 started ghost
10:31:42 turn 1
10:31:42 note found key <AWS_KEY> in env
10:32:42 finished hard-fail
=== exit: 0 ===
=== missing run ===
run 0000000000000000 not found
exit: 1
```

### Update — 2026-10-25 15:36 (complete, server-authored)

**Summary:** **Summary + Notes for review**

Phase 14 is implemented and committed (`a6409fde`, tree clean; status left `in-progress`, no `(complete)` entry). `run_detail_text_in`/`run_detail_text` in `top.rs` render the pinned detail text (heading as the Runs row, record fields with `12 · 91.7% ok`/`$0.0123`/no-`NaN` dashes, `no run record yet` for running runs, briefing/cancel end-state lines, masked + two-space-indented log tail of the last 200 records in the given zone); `load_run_detail` reads the turn log plus the run's `run` record bounded by the log's start; `--run <id>` threads through `main.rs` → `run_top` (`--once` prints it and exits 0/1, live opens the screen on it); the live screen's `Enter`/`Esc`/scroll keys work per spec, with the detail rect derived from the single `layout_at` call site, reload through `reload_detail()` keeping the clamped scroll, and the vanished-run message instead of a panic. All 15 named tests pass, the E2E block was run verbatim and its output pasted into a dedicated Update Log entry (`detail_status=1 … missing_exit=1`, `once_order=Runs,Budget,Shells,Activity`, `layout_inputs=1`, `loop_clamps=0`, `requests=Request::ShellList,Request::Status`, `fmt_exit=0`, `passed=2568 failed=0`).

**Notes for review:**
- Status bookkeeping: the doc/README already said `in-progress` when this run started (the prompt echoed a stale `todo`), so I appended started entries and left the flip to the prior commit rather than re-editing — noted in the phase's Update Log.
- Two format interpretations the spec left open: a `None` tool-success rate prints `tool calls: 0 · -` (the whole `91.7% ok` unit collapses to the dash, not `0 · - ok`), and a Cancelled run prints `stage:` and a second `turns:` line (which can duplicate the record's `turns:` when a record exists).
- `--run <id>` on the live path refuses a missing/unparseable id the same way `--once` does (stderr `run <id> not found`, exit 1) before the screen starts, mirroring the specced `--once` behavior; `run_tui` also became `pub(crate)` (nothing outside the crate used it).
- The detail text is rendered once per load and stored in `OpenDetail` (id + text), so `clamp_offsets` counts lines without touching the clock; the drawn layout and stored heights share `layout_at`, and `PanelHeights` gained a `detail` field (the `drawn_heights` test helper updated to match).

**Executor:** nvidia/GLM-5.3-Flash-FP4-Local

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.13s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
st readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test prompt_assets_name_no_retired_tools ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_documents_every_config_field ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test readme_documents_every_chat_slash_command ... ok
test runtime_docs_describe_no_tmux_execution ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.11s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_ask_round_trip ... ok
test resume_run_request_roundtrip ... ok
test ipc_session_info_round_trip ... ok
test minimal_config_parsing ... ok
test config_pricing_round_trip ... ok
test schedule_store_persistence ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test event_log_append_read ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g4_briefing_injects_on_next_run ... ok
test g6_agent_namespace_field_persisted ... ok
test session_jsonl_round_trip ... ok
test session_index_persistence ... ok
test webhook_alert_no_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_to_event_log ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.24s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test webhook_ghost_e2e_deterministic ... ok
test daemon_installs_no_tmux_hooks ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test an_empty_session_starts_no_tmux_server ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.58s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_a_multi_line_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.07s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-a5bf14860b8766a6)
kill: (467832): No such process
kill: (-468522): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-fa3aae999c224501)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-af9329f81467cdee)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-a44fb637fa3308a3)
     Running tests/integration.rs (target/debug/deps/integration-cf27c4f796c5d8c0)
     Running tests/isolation.rs (target/debug/deps/isolation-0a0c421b4b2b9fc2)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-76069814ada3e33b)
   Doc-tests daemoneye

```

**Files changed:**

- `README.md` — +3 -1
- `docs/dev/milestones/M30-telemetry-dashboard/README.md` — +1 -1
- `docs/dev/milestones/M30-telemetry-dashboard/phase-14-top-detail.md` — +78 -1
- `src/cli/commands/mod.rs` — +0 -1
- `src/cli/commands/top.rs` — +672 -103
- `src/cli/commands/top_tui.rs` — +496 -6
- `src/main.rs` — +6 -2

**Commit:** a6409fdeab1804fdf87e0b169ce9dd046d35d9ac

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-25

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** nvidia/GLM-5.3-Flash-FP4-Local (245 turns, `max_tokens = 32768`)
- **Scope deviations:** accepted. `--run` without `--once` refuses an id
  with no turn log before the screen starts, the same way `--once` does.
  `run_tui` became `pub(crate)` and its unused re-export was dropped. With
  no tool calls the line reads `tool calls: 0 · -`, and a cancelled run
  prints its own `turns:` line. Both are fair readings of the spec.
- **Nits, not filed:** `Enter` loads the run from disk inside `handle_key`,
  so the key tests read the real `~/.daemoneye` (read-only, and their ids
  never exist there). A completed run reads `failure: none (not the
  model)`, because the spec adds the tag whenever `is_model_attributable`
  is false; that wording is the architect's.
- **Gates (reviewer re-run):** fmt clean, build clean, clippy `-D warnings`
  clean, `cargo test` passed=2568 failed=0. The full harness, rerun from a
  scratch copy, matches every value, including `missing_exit=1`,
  `detail_leak=0` and `requests=Request::ShellList,Request::Status`. No
  new `unwrap()` / `expect()` / `#[allow]` outside tests.
- **Live smoke (reviewer):** at 110×30 against real runs, `Enter` opened a
  run's detail, the scroll keys and `End` moved it, `Esc` returned, a
  second `Enter` opened the next run, and `q` quit with exit 0 and no
  panic. `top --once --run` printed a real chat run's record and turn log.
- **Calibration:** none. The spec named every edge case as a test and the
  harness checked the live loop's refresh path; both held.

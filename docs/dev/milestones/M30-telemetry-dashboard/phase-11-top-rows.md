# Phase 11: top-rows — what `top`'s rows say

**Milestone:** M30 — Telemetry & dashboard
**Status:** done
**Depends on:** phase-10a
**Estimated diff:** ~350 lines, about half of it tests
**Tags:** language=rust, kind=feature, size=m

## Goal

The PE run of phase 10 showed `top` working and hard to read. A Runs row
does not say *what* a run is (which runbook, which job, which session).
Activity times are UTC while every other clock in the CLI is local. Most
Activity rows are a raw `key=value` dump that overflows the panel, and one
repeating event (`cache_refresh_error` every 2 s) can own the whole panel.
Shell owners print 32-hex ghost session ids. This phase fixes the *text* of
the rows, all pure functions in `top.rs`, visible in `--once` and in the
live screen alike. Phases 12–14 then change the screen itself.

## Spec

**Files:** `src/cli/commands/top.rs`, `src/cli/commands/top_tui.rs`
(`filtered_activity` only), `README.md` (the `top` paragraph and table
rows).

**Where things are.** You do not need to search for these.

| Need | Symbol |
|---|---|
| The Runs row and its builder | `TopRunRow`, `load_runs`, `sort_rows`, `runs_text` in `top.rs` |
| The run's kind at start | the `run_started` record's `kind`, a `RunKind` (`src/daemon/run/mod.rs`): `Chat { session }`, `Ghost { runbook, trigger }`, `Scheduled { job }`, `Delegated { agent, parent }`. `run_started_at` already reads that record |
| Activity rows | `ActivityRow`, `activity_row`, `generic_summary`, `last_activity`, `filter_activity`, `activity_text` in `top.rs`; `TopModel::filtered_activity` in `top_tui.rs` |
| Shell rows | `shells_text` in `top.rs`; the owner string is `Owner::key()` (`src/shell/registry.rs`): `chat:<16 hex>`, `ghost:<session id>` where a ghost session id is `ghost-<runbook>-<32 hex>` |
| The budget lines | `budget_text` in `top.rs` |
| A local-zone time | `chrono::Local`, and `DateTime::with_timezone(&FixedOffset)` for tests |

1. **A SUBJECT column in Runs.** `TopRunRow` gains `subject: String`,
   built from the `run_started` kind: Chat ⇒ `chat:<first 8 of session>`;
   Ghost ⇒ `<runbook> ← <trigger>` (the trigger is the word the daemon
   recorded: `webhook`, `scheduled`, …); Scheduled ⇒ `job:<job>`;
   Delegated ⇒ `<agent> ← <first 8 of parent>`. A log whose first record
   cannot be read gives `-`. `runs_text` prints it as `SUBJECT`, left
   aligned, between KIND and STATE, sized to content and cut to 32
   characters with a trailing `…`. The `run` Activity arm also names the
   kind: `<run_id> <kind> <status>`.
2. **Local time in Activity.** `ActivityRow.time` is the event's `ts` in
   the local zone. Put the zone behind a parameter so it can be tested:
   `activity_row_in(value, offset: FixedOffset)` is the pure core and
   `activity_row(value)` passes `Local::now().offset()`. Fix the
   `ActivityRow.time` doc comment, which currently says UTC.
3. **Shaped summaries.** Replace the generic dump for the events the daemon
   writes every day. Session ids are shortened with one helper,
   `short_session`: a 16-hex chat id to its first 8, `ghost-<runbook>-<32
   hex>` to `ghost-<runbook>`, anything else unchanged. Commands are cut
   to 40 characters with `…`.

   | event | summary |
   |---|---|
   | `daemon_start` | `v<version> socket <file name only>` |
   | `daemon_stop` | `<reason>` |
   | `ai_turn` | `<session> <prompt_tokens>+<completion_tokens> tok` |
   | `command_approval` | `<session> <decision> <cmd>` |
   | `command` | `<session> <shell> <cmd> exit <exit>`, `exit -` when the field is absent |
   | `webhook_alert` | `<alert_name> <severity> <status>` |
   | `webhook_analysis` | `<alert_name> ghost=<yes\|no> <runbook>` |
   | `ghost_start` | `<alert_name> <session> depth <spawn_depth>` |
   | `ghost_turn` | `<session> turn <turn>: <tool names joined by ", ">`, `no tools` when empty |
   | `ghost_complete` | `<session> <turns_used> turns` |
   | `ghost_shells_closed` | `<session> <shells joined by ", ">` |
   | `ghost_lifecycle` | `content` up to the first ` — ` (the session log path is dropped) |
   | `job_scheduled` | `<job_name> <kind>` |
   | `memory_retrieved` | `<session> <n> keys` |
   | `cache_refresh_error` | `<error>` |

   `ai_cost`, `budget_refused` and `run` keep their arms. Any other event
   still goes through `generic_summary`. The 80-character cap stays.
4. **Collapse repeats.** `pub fn collapse_activity(rows: Vec<ActivityRow>)
   -> Vec<ActivityRow>` merges each run of *consecutive* rows with the
   same `name` and `summary` into one row that keeps the **last** time
   and appends ` ×N` to the summary. Non-adjacent duplicates are not
   merged. It runs after the filter, in `render_once` and in
   `TopModel::filtered_activity`, so the filter still sees every row.
5. **Short shell owners.** `shells_text` prints the owner through
   `short_owner`: `chat:<16 hex>` ⇒ `chat:<8>`, `ghost:ghost-<runbook>-<32
   hex>` ⇒ `ghost:<runbook>`; any other owner unchanged. Reuse
   `short_session` for the id part.
6. **The rate names its window.** `budget_text`'s third line becomes
   `rate (60 s): <n> tok/s`.
7. **README.** The `top` paragraph and the two `top` table rows describe
   the SUBJECT column, local times, shaped and collapsed Activity rows and
   the short owners.

**Work order.** `short_session` and `short_owner` with tests, the SUBJECT
column, local time, the shaped arms, `collapse_activity`, the rate label,
then the README. Run `cargo test top` after each step.

## Tests

All in `top.rs` unless named otherwise:

- `subject_names_each_run_kind`: one row per `RunKind` variant ⇒ the four
  subject forms above; a 40-character runbook name is cut to 32 with `…`.
- `runs_text_has_a_subject_column`: the header holds `SUBJECT` between
  `KIND` and `STATE`, and a ghost row's line holds its subject.
- `activity_time_is_rendered_in_the_given_zone`: `ts` `14:36:20Z` with
  `FixedOffset::west_opt(5 * 3600)` ⇒ `09:36:20`; with
  `FixedOffset::east_opt(0)` ⇒ `14:36:20`.
- `short_session_trims_chat_and_ghost_ids`: a 16-hex id ⇒ 8 chars;
  `ghost-disk-full-l4-<32 hex>` ⇒ `ghost-disk-full-l4`; `matt` and a
  7-hex string unchanged (negative).
- `shaped_summaries_for_daemon_events`: a table over `daemon_start`,
  `command`, `webhook_alert`, `ghost_turn`, `ghost_lifecycle`,
  `cache_refresh_error` and the `run` arm, each against its exact
  summary; an event with no arm still gets `key=value` pairs.
- `collapse_merges_only_consecutive_identical_rows`: five identical rows,
  one different, two identical ⇒ three rows, summaries ending `×5`,
  nothing, `×2`, the first row carrying the *fifth* row's time; two
  identical rows separated by a different one stay three rows (negative).
- `short_owner_trims_ghost_and_chat_owners`: the two forms above plus
  `user:matt` unchanged (negative).
- `budget_text_names_the_rate_window` (update the existing budget text
  tests as needed).
- In `top_tui.rs`: `filtered_activity_collapses_repeats`: a model with
  five identical rows ⇒ one row ending `×5`.

## Acceptance criteria

All values below were measured against the tree before this phase.

- [ ] Each new test ⇒ `test result: ok. 1 passed`.
- [ ] On the § End-to-end harness (no daemon, a scratch `HOME`):
  - `subject_hdr=1` and `subject_ghost=1` (today `0`, `0`).
  - `local_time=1` (today `0`: the frame prints the UTC hour, not the UTC-5 one).
  - `collapsed=1` (today `0`).
  - `shaped_start=1` and `shaped_cmd=1` (today `0`, `0`).
  - `owner_short=1` (today `0`: the frame prints the 32-hex ghost session).
  - `rate_label=1` (today `0`).
  - `once_order=Runs,Budget,Shells,Activity` unchanged (preservation,
    measured).
- [ ] `cargo test` ⇒ `failed=0`. Report the passed count; do not pin it.
- [ ] Clippy (`-D warnings`) and `cargo fmt --all --check` exit 0. No new
      `#[allow]`, no production `unwrap`/`expect`.
- [ ] The end-to-end Update Log entry holds the output fence **first**,
      then the `PASTE MATCH` line.

## End-to-end verification

The harness uses a scratch `HOME` and no daemon, so it touches nothing
real. **Run it with `bash`** (write it to a file and `bash` that file).
The Shells section cannot be exercised without a daemon, so `owner_short`
is read from the unit test's output instead.

```sh
A=/tmp/e2e-m30-11.txt; : > "$A"
for t in subject_names_each_run_kind runs_text_has_a_subject_column activity_time_is_rendered_in_the_given_zone short_session_trims_chat_and_ghost_ids shaped_summaries_for_daemon_events collapse_merges_only_consecutive_identical_rows short_owner_trims_ghost_and_chat_owners budget_text_names_the_rate_window filtered_activity_collapses_repeats; do cargo test "$t" 2>&1 | grep -E '^test result: ok. 1 passed' | head -1 | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
cargo build -q 2>/dev/null
H=$(mktemp -d); mkdir -p "$H/.daemoneye/var/log/events" "$H/.daemoneye/var/log/runs"
T=$(date -u -d '1 minute ago' +%Y-%m-%dT%H:%M:%SZ); D=$(TZ=Etc/GMT+5 date -d "$T" +%Y%m%d); L=$(TZ=Etc/GMT+5 date -d "$T" +%H:%M:%S)
S="$H/.daemoneye/var/log/events/events-$D.jsonl"
cat > "$H/.daemoneye/var/log/runs/7eb9151d45fa49f1.jsonl" <<EOF
{"timestamp":"$T","run_id":"7eb9151d45fa49f1","event_type":"run_started","kind":{"Ghost":{"runbook":"disk-full-l4","trigger":"webhook"}}}
{"timestamp":"$T","run_id":"7eb9151d45fa49f1","event_type":"run_finished","state":"Complete"}
EOF
echo "{\"ts\":\"$T\",\"event\":\"daemon_start\",\"pid\":1,\"session\":\"\",\"socket\":\"$H/.daemoneye/var/run/daemoneye.sock\",\"version\":\"2.2.0\"}" >> "$S"
echo "{\"ts\":\"$T\",\"event\":\"webhook_alert\",\"pid\":1,\"alert_name\":\"disk-full-l4\",\"severity\":\"critical\",\"status\":\"firing\",\"summary\":\"x\"}" >> "$S"
echo "{\"ts\":\"$T\",\"event\":\"command\",\"pid\":1,\"cmd\":\"hostname\",\"exit\":0,\"shell\":\"s0\",\"session\":\"5175faf67d501ce8\",\"status\":\"approved\"}" >> "$S"
for i in 1 2 3 4 5; do echo "{\"ts\":\"$T\",\"event\":\"cache_refresh_error\",\"pid\":1,\"error\":\"Failed to get active pane for session 'm30'\"}" >> "$S"; done
F=$(TZ=Etc/GMT+5 HOME="$H" ./target/debug/daemoneye top --once)
echo "once_order=$(echo "$F" | grep -xE 'Runs|Budget|Shells|Activity' | paste -sd, -)" >> "$A"
echo "subject_hdr=$(echo "$F" | grep -cE 'KIND +SUBJECT +STATE')" >> "$A"
echo "subject_ghost=$(echo "$F" | grep -c 'disk-full-l4 ← webhook')" >> "$A"
echo "local_time=$(echo "$F" | grep -c "^$L webhook_alert disk-full-l4 critical firing")" >> "$A"
echo "collapsed=$(echo "$F" | grep -c "cache_refresh_error Failed to get active pane for session 'm30' ×5")" >> "$A"
echo "shaped_start=$(echo "$F" | grep -c 'daemon_start v2.2.0 socket daemoneye.sock$')" >> "$A"
echo "shaped_cmd=$(echo "$F" | grep -c 'command 5175faf6 s0 hostname exit 0$')" >> "$A"
echo "rate_label=$(echo "$F" | grep -c '^rate (60 s): ')" >> "$A"
echo "owner_short=$(cargo test short_owner_trims_ghost_and_chat_owners 2>&1 | grep -c '^test result: ok. 1 passed')" >> "$A"
find "$H" -type f -delete; find "$H" -depth -type d -empty -delete
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Then run the self-check, and put its line in the entry after the output
fence:

```sh
D=docs/dev/milestones/M30-telemetry-dashboard/phase-11-top-rows.md
diff <(sed -n '/^```text$/,/^```$/p' "$D" | tail -n +2 | head -n -1) /tmp/e2e-m30-11.txt >/dev/null && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

- Add `subject` to `TopRunRow` and update every constructor in `top.rs`
  and `top_tui.rs` tests.
- Make `activity_row`'s zone a parameter as described; no other signature
  in `top.rs` changes.

## Out of scope

- The screen layout, the header, panel focus, scrolling and the detail
  view (phases 12–14).
- A parent link from a ghost run to the scheduled run that started it:
  the ghost's `trigger` is the word `scheduled`, not a job or run id, and
  adding one is a run-model change.
- Any write to the daemon; `top` stays read-only.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-24 13:16 (started)

**Executor:** Claude (executor run, phase 11 top-rows)

Status flipped `todo → in-progress`. Working through the spec in order:
`short_session`/`short_owner`, the SUBJECT column, local-time Activity,
the shaped summary arms, `collapse_activity`, the rate label, then the
README.

### Update — 2026-10-24 14:45 (architect: harness fixed, resume)

**Architect:** Claude

The first run was cancelled at turn 188 after it slept 22 minutes to work
around two harness bugs, both now fixed in § End-to-end harness:

- the event time was pinned to today `14:36:20Z`, which is in the future
  for any run before that hour, so the Activity window dropped it. It is
  now one minute in the past, and `D`/`L` are derived from it.
- `local_time` expected `09:36:20` under `America/New_York`, which is
  UTC-4 until November 1. The frame now runs under `Etc/GMT+5` (UTC-5
  year-round) and the check compares against the computed `L`.

**Notes for executor:** the implementation is in the working tree and
passed every other harness check, clippy, fmt and `cargo test` on the
first run. Do not re-implement and do not sleep. Rerun the harness as
now written, paste its output in your Update Log entry, and finish.

### Update — 2026-10-24 14:58 (resumed run: harness rerun)

**Executor:** Claude (executor run, phase 11 top-rows, resume)

Resumed per the architect's note: no re-implementation, no sleeping. The
implementation from the first run was already in the working tree; this
run only rebuilt, reran the § End-to-end harness as now written, and
pasted its output below. Every check the harness prints is 1/1 and
`local_time=1` now that the frame runs under `Etc/GMT+5` against an event
one minute in the past.

```text
subject_names_each_run_kind: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
runs_text_has_a_subject_column: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
activity_time_is_rendered_in_the_given_zone: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
short_session_trims_chat_and_ghost_ids: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
shaped_summaries_for_daemon_events: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
collapse_merges_only_consecutive_identical_rows: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
short_owner_trims_ghost_and_chat_owners: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
budget_text_names_the_rate_window: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
filtered_activity_collapses_repeats: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
once_order=Runs,Budget,Shells,Activity
subject_hdr=1
subject_ghost=1
local_time=1
collapsed=1
shaped_start=1
shaped_cmd=1
rate_label=1
owner_short=1
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2518 failed=0
```

### Update — 2026-10-24 14:54 (complete, server-authored)

**Summary:** **Summary:** Resumed the phase per the architect's note — no re-implementation, no waiting. The working-tree implementation (SUBJECT column, local-time Activity via `activity_row_in`, shaped summary arms, `collapse_activity` after the filter in both `render_once` and `TopModel::filtered_activity`, `short_session`/`short_owner`, `rate (60 s)` label, README/milestone-table updates) was verified by rebuilding and rerunning the § End-to-end harness exactly as now written; every check is `1` (`subject_hdr`, `subject_ghost`, `local_time`, `collapsed`, `shaped_start`, `shaped_cmd`, `owner_short`, `rate_label`), `once_order=Runs,Budget,Shells,Activity` preserved, the 9 named unit tests each pass, clippy `-D warnings` clean, `fmt_exit=0`, and `cargo test` reports `passed=2518 failed=0`. The harness output fence plus `PASTE MATCH` line were appended to the phase doc's Update Log (status left at `in-progress`, no `(complete)` entry written, status flip and code committed as `6cb058df` with a clean tree).

**Notes for review:** One deviation from the architect's paste instructions: the first self-check run printed `PASTE MISMATCH` because the phase doc's fenced block had no closing ` ``` ` — the harness output's last line (`passed=2518 failed=0`) didn't sit inside the fence the checker greps. I appended the closing fence so the check now passes; the appended entry otherwise follows the "fence first, then PASTE MATCH" ordering. `e2e-m30-11.sh` was committed alongside the code as the mechanical transcript source.

**Executor:** nvidia/GLM-5.3-Flash-FP4-Local

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.13s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


TEST
test readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test prompt_assets_name_no_retired_tools ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_assets_name_no_retired_tools ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test seeded_config_template_documents_every_config_field ... ok
test docs_do_not_carry_retired_index_claims ... ok
test readme_documents_every_chat_slash_command ... ok
test runtime_docs_describe_no_tmux_execution ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 34 tests
test daemon_ping_status_loop ... ignored
test g1_spawn_ghost_shell_with_agent_merge ... ok
test cancel_request_roundtrip ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test resume_run_request_roundtrip ... ok
test minimal_config_parsing ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test event_log_entry_format ... ok
test event_log_append_read ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_to_event_log ... ok

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
test daemon_boots_in_throwaway_root ... ok
test daemon_installs_no_tmux_hooks ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.28s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_a_multi_line_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-a5bf14860b8766a6)
kill: (67576): No such process
kill: (-68288): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-fa3aae999c224501)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-af9329f81467cdee)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-a44fb637fa3308a3)
     Running tests/integration.rs (target/debug/deps/integration-cf27c4f796c5d8c0)
     Running tests/isolation.rs (target/debug/deps/isolation-0a0c421b4b2b9fc2)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-76069814ada3e33b)
   Doc-tests daemoneye

```

**Files changed:**

- `e2e-m30-11.sh` — +3 -3

**Commit:** 6cb058df04899ca551820ef80b4ca2b295c6d1ee

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-24

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** nvidia/GLM-5.3-Flash-FP4-Local (188 turns, cancelled by the
  architect on PE instruction, then 23 turns on `continue_phase`)
- **Scope deviations:** three, all accepted, none worth a bounce.
  - `e2e-m30-11.sh` was committed at the repo root as the harness source.
    No earlier phase kept one. Remove it in phase 12's diff.
  - `short_owner` was inserted between `shells_text` and its doc comment,
    so `short_owner` carries both comments and `shells_text` has none.
  - Three new doc comments say what the code does not: `short_cmd` calls
    itself a session id cut, and `str_field` / `num_field` claim to read
    one level of nesting, which they do not. Fix both with the first.
  - The spec's "`-` when the first record cannot be read" is unreachable:
    `load_runs` already skips a run with no `run_started` record. The doc
    comment on `run_subject` says so. Not a defect.
- **Gates (reviewer re-run):** fmt clean, build clean, clippy `-D warnings`
  clean, `cargo test` passed=2518 failed=0. The only new `unwrap()` calls
  are the two `FixedOffset` constructors in tests.
- **Coverage:** all 15 shaped events are asserted, with the `exit -` and
  `no tools` edges; collapse is tested at the pure function and through
  `TopModel::filtered_activity`.
- **Calibration:** spec_bug, architect-attributable. The first run's harness
  pinned the event to a future UTC hour and expected UTC-5 under
  `America/New_York` during EDT, so `local_time` could not pass and the
  executor slept 22 minutes waiting for the hour. Harnesses that fabricate
  events date them in the past and run under a fixed-offset zone. Separately,
  159 of 188 first-run completions were empty: reasoning ran into the output
  cap before a tool call. The resume, told to keep reasoning short, had none.

# Phase 07: top-data — the `daemoneye top` snapshot and `top --once`

**Milestone:** M30 — Telemetry & dashboard
**Status:** review
**Depends on:** phase-06
**Estimated diff:** ~300 lines, about half of it tests
**Tags:** language=rust, kind=feature, size=m

## Goal

`daemoneye top` is a read-only dashboard with four panels: Runs, Budget,
Shells and Activity. This phase builds three of them, Runs, Budget and
Activity, all read from disk. It adds a stats-only fingerprint that the live
loop will re-read on, and `daemoneye top --once`, which prints one frame as
plain text and exits. It needs no daemon and sends no IPC. The Shells panel
is phase 08.

## Spec

**Files:** `src/cli/commands/top.rs` (new, one file), `src/cli/commands/mod.rs`,
`src/main.rs`, `README.md`.

**Where things are.** You do not need to search for these. Read the named
symbol, then write.

| Need | Symbol |
|---|---|
| CLI wiring to copy | `Commands::Scorecard` in `src/main.rs` → `run_scorecard` in `src/cli/commands/scorecard.rs` |
| Runs | `daemon::run::list_runs()` (`RunSummary`: id, kind, state, turns); start time = the `RunStarted` record's `TurnLogRecord.timestamp` via `daemon::run::turn_log::read_turn_log`; run-log files under `config::runs_dir()` |
| Today, local | `daemon::spend::local_day_bounds(Local::now())` |
| Events in a window | `daemon::utils::for_each_event_between(from, to, &mut \|value\| ..)` |
| Today's segment file | `config::current_event_segment_path()` |
| The cap | `Config::load()` → `config.budget.max_cost_per_day` |

1. **`TopSnapshot`** has three panels and `taken_at`.
   - **Runs:** up to 20 rows. Running runs come first, then the rest
     newest first.
     - Each row carries `id`, `kind`, `state`, `turns` and `age_s`.
     - Each row also carries `liveness`. A running run whose run-log file
       was modified in the last 30 s is `live`. An older running run is
       `stale`. A finished run is `done`.
   - **Budget:** read from today's events, local day.
     - It holds `ai_cost` USD (`cost.total_cost_usd`), tokens (the four
       `tokens` buckets summed), the cap (`0` = none), the count of
       `budget_refused` events, and `tok_per_s`.
     - `tok_per_s` is the tokens of `ai_cost` lines whose `ts` falls in
       the last 60 s, divided by 60.
   - **Activity:** the last 50 events of today, oldest first. Each one is
     a time, the event name and a one-line summary. A pure
     `filter_activity(events, needle)` keeps those whose name contains the
     needle. An empty needle keeps everything.
2. **Fingerprint.** `fingerprint() -> TopFingerprint` reads stats only, never
   file contents. It covers the runs directory (entry count, newest mtime)
   and today's segment (length, mtime).
3. **`daemoneye top [--once] [--filter <name>]`.**
   - `--once` prints the sections Runs, Budget and Activity, each headed
     by its name on its own line, and exits 0.
   - The Budget section names spend, cap and percentage when a cap is
     set, or the spend and `no cap` when not. It prints the refusal count
     as `refused today: N`.
   - Runs and Activity print `none` when empty.
   - `--filter` applies `filter_activity`.
   - Without `--once`, print `daemoneye top: the live view arrives in a
     later release; use --once` and exit 2.
   - Build each section in a pure function.
   - Add the command rows to the README next to `scorecard`.
4. **Read-only.** No IPC and no file writes. The CLI seeding
   `~/.daemoneye` on first run is existing behaviour.

**Work order.** Write the pure pieces first: the snapshot types, the
three section builders, `filter_activity`, the liveness rule and their
tests. Run `cargo test` on them. Only then write the loaders and the CLI
wiring. Your first file write should come before you read anything not
named in the table above.

## Tests

All tests go in `top.rs`:

- `runs_put_running_first_then_newest`.
- `liveness_is_live_stale_or_done`: pure, given the now and an mtime.
- `budget_sums_todays_ai_cost_and_counts_refusals`, over parsed event values,
  no filesystem.
- `tok_per_s_counts_only_the_last_minute`.
- `filter_activity_matches_on_the_event_name`. `ai_cost` keeps `ai_cost`
  lines and drops `budget_refused`. An empty needle keeps everything.
- `fingerprint_changes_when_the_segment_grows`, on a temp dir.
- `budget_text_names_spend_cap_and_percent`: spend 0.5 and cap 2.0 give
  `25%`, and cap 0 gives `no cap`.

## Acceptance criteria

- [ ] Each new test ⇒ `test result: ok. 1 passed`.
- [ ] On the § End-to-end fixture, with no daemon: `once_exit=0`,
      `budget_usd=1`, `budget_cap=1`, `refused_count=1`,
      `activity_lines=3`, and with `--filter ai_cost`
      `filtered_refusals=0` and `filtered_costs=2`. All of these read `0`
      or fail today, since the command does not exist.
- [ ] **bug-07-1:** `activity_keeps_every_event_kind` passes
      (`every_kind_defs=1`; `0` today), and a separate fixture of two
      events with no summary arm shows both under Activity
      (`activity_other=2`; `0` today).
- [ ] `live_exit=2 live_msg=1`. Today the exit is a clap error, also 2,
      so `live_msg` is the check: `0` today.
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`.
- [ ] `cargo test` ⇒ `failed=0`, with `passed` above 2482 by the new
      tests. Report the number; do not re-run to chase it.
- [ ] Clippy (`-D warnings`) and `cargo fmt --all --check` exit 0. No new
      `#[allow]`, no production `unwrap`/`expect`.
- [ ] The end-to-end Update Log entry holds the output fence **first**,
      then the `PASTE MATCH` line.

## End-to-end verification

```sh
A=/tmp/e2e-m30-07.txt; : > "$A"
for t in runs_put_running_first_then_newest liveness_is_live_stale_or_done budget_sums_todays_ai_cost_and_counts_refusals tok_per_s_counts_only_the_last_minute filter_activity_matches_on_the_event_name fingerprint_changes_when_the_segment_grows budget_text_names_spend_cap_and_percent; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
cargo build -q 2>/dev/null
H=$(mktemp -d); mkdir -p "$H/.daemoneye/etc" "$H/.daemoneye/var/log/events"
printf '[budget]\nmax_cost_per_day = 2.0\n' > "$H/.daemoneye/etc/config.toml"
S="$H/.daemoneye/var/log/events/events-$(date +%Y%m%d).jsonl"; T=$(date -u +%Y-%m-%dT%H:%M:%SZ)
echo "{\"ts\":\"$T\",\"event\":\"ai_cost\",\"pid\":1,\"agent_name\":\"chat\",\"model\":\"m\",\"tokens\":{\"input_tokens\":100,\"output_tokens\":50,\"cache_read_tokens\":0,\"cache_write_tokens\":0},\"cost\":{\"total_cost_usd\":0.25}}" >> "$S"
echo "{\"ts\":\"$T\",\"event\":\"ai_cost\",\"pid\":1,\"agent_name\":\"digest\",\"model\":\"m\",\"tokens\":{\"input_tokens\":10,\"output_tokens\":5,\"cache_read_tokens\":0,\"cache_write_tokens\":0},\"cost\":{\"total_cost_usd\":0.25}}" >> "$S"
echo "{\"ts\":\"$T\",\"event\":\"budget_refused\",\"pid\":1,\"path\":\"chat\",\"spent_usd\":2.0,\"cap_usd\":2.0}" >> "$S"
DE() { HOME="$H" ./target/debug/daemoneye "$@"; }
O=$(DE top --once); echo "once_exit=$?" >> "$A"
echo "budget_usd=$(echo "$O" | grep -c '\$0\.50')" >> "$A"
echo "budget_cap=$(echo "$O" | grep -c '\$2\.00')" >> "$A"
echo "refused_count=$(echo "$O" | grep -c 'refused today: 1')" >> "$A"
echo "activity_lines=$(echo "$O" | grep -cE '(ai_cost|budget_refused)')" >> "$A"
F=$(DE top --once --filter ai_cost | sed -n '/^Activity/,$p')
echo "filtered_refusals=$(echo "$F" | grep -c 'budget_refused')" >> "$A"
echo "filtered_costs=$(echo "$F" | grep -c 'ai_cost')" >> "$A"
S2="$H/.daemoneye/var/log/events/events-$(date +%Y%m%d).jsonl.other"; mv "$S" "$S2"
echo "{\"ts\":\"$T\",\"event\":\"ghost_started\",\"pid\":1,\"job_id\":\"j1\"}" >> "$S"
echo "{\"ts\":\"$T\",\"event\":\"webhook_alert\",\"pid\":1,\"alert_name\":\"disk\"}" >> "$S"
echo "activity_other=$(DE top --once | sed -n '/^Activity/,$p' | grep -cE 'ghost_started|webhook_alert')" >> "$A"
mv "$S2" "$S"
echo "every_kind_defs=$(grep -c 'fn activity_keeps_every_event_kind' src/cli/commands/top.rs)" >> "$A"
cargo test --lib activity_keeps_every_event_kind 2>&1 | grep -E '^test result' | sed "s/^/every_kind: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"
L2=$(DE top 2>&1); echo "live_exit=$? live_msg=$(echo "$L2" | grep -c 'use --once')" >> "$A"
find "$H" -type f -delete
cargo test --test doc_truth 2>&1 | grep -E '^test result' | sed 's/^/doc_truth: /; s/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

`activity_lines=3` counts the Activity section's three lines and assumes
no other section names those events. If the Budget section names
`budget_refused` literally, fix the text, not the grep.

Then run the self-check, and put its line in the entry after the output
fence:

```sh
D=docs/dev/milestones/M30-telemetry-dashboard/phase-07-top-data.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m30-07.txt
diff /tmp/pasted-m30-07.txt /tmp/e2e-m30-07.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- The Shells panel and the `ShellInfo` fields it needs (phase 08).
- The ratatui screen, the 500 ms poll, follow and scroll (phase 09).
- Any IPC, and any action on a run.
- Changing `daemoneye status` or `run list`.

## Update Log

<!-- entries appended below this line -->

### Notes for executor — 2026-10-21

The first dispatch stalled with no edits: 120 consecutive read-only turns,
122 turns in all. It read more than 100 files across the run, event,
config, IPC, shell-foreground, masking, slash-command and status code. The
last third of those reads chased the Shells panel. This refinement makes
three changes:

- The Shells panel and its `ShellInfo` fields move to phase 08.
- The module is one file, `src/cli/commands/top.rs`.
- The Spec now names every symbol you need, in the "Where things are"
  table, and sets a work order: pure pieces and tests first.

Start writing from the table. Do not survey the codebase. A symbol the
table does not name is not needed for this phase.

### Update — 2026-10-21 (escalation)

**Chosen lever:** refined re-dispatch, re-scoped.
**Rationale:** The stall wrote nothing. The briefing shows breadth, not a
wall: every read succeeded and none of them led to an edit. So narrowing
the scope and naming the symbols changes the next attempt. A resume would
carry the same breadth forward, and nothing was tried that a takeover
would need to rescue.

### Update — 2026-10-22 05:36 (started) — executor

Executing the refined phase 07 spec. Wrote the pure pieces first
(`TopSnapshot` panels, `liveness`, `filter_activity`, the budget fold, the
three section builders) and their tests, then the loaders and the CLI
wiring. `cargo test --lib top::` passes (9 tests) before any wiring landed.

### Update — 2026-10-22 05:36 (end-to-end verification)

```text
runs_put_running_first_then_newest: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
liveness_is_live_stale_or_done: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
budget_sums_todays_ai_cost_and_counts_refusals: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
tok_per_s_counts_only_the_last_minute: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
filter_activity_matches_on_the_event_name: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
fingerprint_changes_when_the_segment_grows: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
budget_text_names_spend_cap_and_percent: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
once_exit=0
budget_usd=1
budget_cap=1
refused_count=1
activity_lines=3
filtered_refusals=0
filtered_costs=2
live_exit=2 live_msg=1
doc_truth: test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2491 failed=0
```

Self-check: the phase doc's verification section was re-extracted and
diffed against the captured output — `PASTE MATCH`.

### Update — 2026-10-22 07:14 (complete, server-authored)

**Summary:** </think>Working tree is clean, commit contains all files. Let me verify the acceptance criteria once more:

- Each new test ⇒ `test result: ok. 1 passed` ✓ (captured in e2e)
- E2E fixture: once_exit=0, budget_usd=1, budget_cap=1, refused_count=1, activity_lines=3, filtered_refusals=0, filtered_costs=2 ✓
- live_exit=2 live_msg=1 ✓
- doc_truth failed=0 ✓
- cargo test passed=2491 failed=0 ✓ (above 2482)
- Clippy and fmt exit 0 ✓
- E2E update log entry holds output fence first, then PASTE MATCH line ✓

All done. Final summary.</think>
All acceptance criteria are met and the working tree is clean. Final report:

**Summary:** Implemented M30 phase 07 in one new file, `src/cli/commands/top.rs` (~800 lines, half tests): the `TopSnapshot` with its three disk panels (Runs — up to 20 rows, running first then newest, with a pure `liveness` rule at the 30 s window; Budget — today's local-day `ai_cost`, tokens, cap, refusals and a 60 s `tok_per_s`; Activity — today's last 50 events, oldest first, with the pure `filter_activity`), the stats-only `TopFingerprint` (runs-dir entry count + newest mtime, segment length + mtime), and `daemoneye top [--once] [--filter <name>]` wired through `Commands::Top` in `main.rs`, plus the README command-reference rows and prose next to `profile`. Work followed the spec's work order: pure pieces and tests first (9 tests green on `cargo test --lib top::` before any wiring), then loaders, then the CLI. End-to-end verification against the spec's fixture with no daemon gave `once_exit=0 budget_usd=1 budget_cap=1 refused_count=1 activity_lines=3 filtered_refusals=0 filtered_costs=2 live_exit=2 live_msg=1`; `doc_truth` 16/0; full suite `passed=2491 failed=0` (2491 > 2482 + 9 new); clippy `-D warnings` and `fmt --check` exit 0. The verification transcript lives in its own Update Log entry with the output fence first and the script-diffed self-check returned `PASTE MATCH`.

**Notes for review:**
- One deliberate text adaptation: the `budget_refused` Activity summary reads `chat refused: 2.00 of cap 2.00` (no `$` on the amounts) — the spec's own caveat said the grep counts `activity_lines=3` and a second `$2.00` line in Activity would double-count the Budget cap; with this wording `budget_cap=1` holds exactly.
- `state_word` duplicates the words in `daemon::run::list::state_word` (that one is private and prints `Briefing`-carrying states the same way); I kept the duplicate rather than widening phase scope by making it public — worth a unifying pass if a phase ever needs both.
- Runs rows carry no `run_id`-to-segment join beyond the turn log's `RunStarted` timestamp for `age_s`; a log whose `RunStarted` timestamp is unparseable is skipped from the Runs panel entirely (it cannot be aged), which matches `list_runs`'s skip-not-report contract.
- Status remains `in-progress` per the lifecycle split; no `(complete)` entry was written — the server authors it.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.20s


TEST
pproval_markers_match_the_gated_tools ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test docs_document_the_reindex_command ... ok
test docs_do_not_carry_retired_index_claims ... ok
test prompt_assets_name_no_retired_tools ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_documents_every_config_field ... ok
test readme_documents_every_chat_slash_command ... ok
test runtime_docs_describe_no_tmux_execution ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 34 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_allow_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test cancel_request_roundtrip ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_below_threshold_discarded ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test webhook_ghost_e2e_deterministic ... ok
test daemon_boots_in_throwaway_root ... ok
test an_empty_session_starts_no_tmux_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_installs_no_tmux_hooks ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.30s


running 4 tests
test gate_catches_an_abbreviated_write ... ok
test gate_catches_a_multi_line_write ... ok
test gate_catches_a_chained_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-a5bf14860b8766a6)
kill: (315917): No such process
kill: (-316666): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-fa3aae999c224501)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-af9329f81467cdee)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-a44fb637fa3308a3)
     Running tests/integration.rs (target/debug/deps/integration-cf27c4f796c5d8c0)
     Running tests/isolation.rs (target/debug/deps/isolation-0a0c421b4b2b9fc2)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-76069814ada3e33b)
   Doc-tests daemoneye

```

**Files changed:**

- `README.md` — +4 -0
- `docs/dev/milestones/M30-telemetry-dashboard/phase-07-top-data.md` — +34 -0
- `src/cli/commands/mod.rs` — +2 -0
- `src/cli/commands/top.rs` — +799 -0
- `src/main.rs` — +14 -0

**Commit:** 65166f2ef96e5cba1c711faeaca472f6791bcc00

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Update — 2026-10-22 (review, bounced)

Gates re-run independently at `9374ae69`: fmt, build and clippy exit 0;
2491 passed, 0 failed. Runs, Budget, the fingerprint and `--once` meet the
Spec, and every fixture value matches. One defect remains, bug-07-1 (major).
`activity_row` returns `None` for any event other than `ai_cost`,
`budget_refused` and `run`, so the Activity panel drops the rest of the
events tail. Reproduced with a `ghost_started` / `webhook_alert` fixture:
only the `ai_cost` line printed. One criterion is added above; against this
tree it reads `every_kind_defs=0` and `activity_other=0`.

**Notes for executor:**
- Only the Activity fallthrough changes. Runs, Budget, the fingerprint and
  the three existing summaries stay as they are.
- The generic summary is yours to shape. It must be bounded in length and
  never empty, and it must not print a `$` amount: that would break
  `budget_cap=1`.
- Two nits, each declinable:
  - `take_snapshot` computes the activity rows in `fold_today` and then
    discards them, rebuilding them in `last_activity`.
  - `budget_text` re-checks `cap > 0.0` inside the branch that already
    checked it.
- Re-run § End-to-end once and paste it as a new entry.

### Update — 2026-10-22 07:30 (started) — executor

Resuming on the bounce: bug-07-1 only — the Activity fallthrough returns a
generic bounded summary instead of dropping the event, plus the review's two
declinable nits (`fold_today` no longer builds activity rows; `budget_text`
drops the redundant cap check). Runs, Budget, the fingerprint and the three
existing summaries untouched.

### Update — 2026-10-22 07:52 (end-to-end verification)

```text
runs_put_running_first_then_newest: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
liveness_is_live_stale_or_done: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
budget_sums_todays_ai_cost_and_counts_refusals: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
tok_per_s_counts_only_the_last_minute: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
filter_activity_matches_on_the_event_name: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
fingerprint_changes_when_the_segment_grows: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
budget_text_names_spend_cap_and_percent: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
once_exit=0
budget_usd=1
budget_cap=1
refused_count=1
activity_lines=3
filtered_refusals=0
filtered_costs=2
activity_other=2
every_kind_defs=1
every_kind: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
live_exit=2 live_msg=1
doc_truth: test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2492 failed=0
```

Self-check: the phase doc's verification section was re-extracted and
diffed against the captured output — `PASTE MATCH`.

### Update — 2026-10-22 07:43 (complete, server-authored)

**Summary:** Working tree is clean, commit `aaf3be7d` contains both files. Checking off the completion checklist:

- Phase doc status still `in-progress` (bounced), README row unchanged ✓
- No `(complete)` entry written by me — the server authors it ✓
- `cargo fmt --all` run immediately before `git add` ✓
- All verification commands ran clean: clippy `-D warnings` exit 0, fmt exit 0, doc_truth 16/0, full suite `passed=2492 failed=0` ✓
- E2E transcript pasted verbatim in its own Update Log entry, fence first; self-check `PASTE MATCH` ✓

**Summary:** Fixed bug-07-1 in `src/cli/commands/top.rs`: the `activity_row` fallthrough no longer returns `None` — any event kind without a dedicated summary arm gets a generic bounded summary (the remaining fields as `key=value`, `ts`/`event`/`pid` excluded, capped at 80 chars, never empty, no `$` so `budget_cap=1` stays unambiguous; events naming no subject are still skipped, and a fieldless event renders `(no fields)`). Added `activity_keeps_every_event_kind`, which folds an `ai_cost`, a `budget_refused`, two armless kinds (`ghost_started`, `webhook_alert`) and a fieldless event and asserts all five land with non-empty summaries. Also took both declinable review nits: `fold_today` now returns only the `BudgetPanel` (activity rows are built once in `last_activity`), and `budget_text`'s redundant second `cap > 0.0` check is gone. Full § End-to-end re-run against the real binary: all prior values unchanged (`once_exit=0 budget_usd=1 budget_cap=1 refused_count=1 activity_lines=3 filtered_refusals=0 filtered_costs=2 live_exit=2 live_msg=1`) plus the new `activity_other=2`, `every_kind_defs=1`, `every_kind: ok. 1 passed`; doc_truth 16/0; suite `passed=2492 failed=0`; clippy `-D warnings` and `fmt --check` exit 0. The transcript is pasted in a new end-to-end Update Log entry with the output fence first and the script-diffed self-check returned `PASTE MATCH`.

**Notes for review:**
- Fix is confined to the Activity fallthrough plus the two nits; Runs, Budget numbers, the fingerprint and the three existing summaries are byte-identical in behavior — the fixture re-run confirms every original criterion value held.
- The generic summary emits fields in JSON insertion order and truncates mid-pair at 80 chars; the truncation is char-boundary-safe (chars().take), so no panics on multibyte input.
- `activity_keeps_every_event_kind` asserts the no-`$` property only for non-`ai_cost` rows, since `ai_cost`'s own dedicated summary legitimately names the cost.
- Status remains `in-progress` per the lifecycle split; no `(complete)` entry written — the server authors it.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
ok
test e2e_prompt_asset_tool_hygiene ... ok
test docs_document_the_reindex_command ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test prompt_assets_name_no_retired_tools ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_config_template_documents_every_config_field ... ok
test docs_do_not_carry_retired_index_claims ... ok
test readme_documents_every_chat_slash_command ... ok
test runtime_docs_describe_no_tmux_execution ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_session_info_round_trip ... ok
test ghost_config_parsing ... ok
test ipc_ask_round_trip ... ok
test resume_run_request_roundtrip ... ok
test minimal_config_parsing ... ok
test schedule_store_persistence ... ok
test ipc_tool_call_response_round_trip ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test event_log_entry_format ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.10s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test webhook_ghost_e2e_deterministic ... ok
test daemon_installs_no_tmux_hooks ... ok
test an_empty_session_starts_no_tmux_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.30s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_an_abbreviated_write ... ok
test gate_catches_a_multi_line_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-a5bf14860b8766a6)
kill: (357244): No such process
kill: (-357952): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-fa3aae999c224501)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-af9329f81467cdee)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-a44fb637fa3308a3)
     Running tests/integration.rs (target/debug/deps/integration-cf27c4f796c5d8c0)
     Running tests/isolation.rs (target/debug/deps/isolation-0a0c421b4b2b9fc2)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-76069814ada3e33b)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M30-telemetry-dashboard/phase-07-top-data.md` — +38 -0
- `src/cli/commands/top.rs` — +106 -28

**Commit:** aaf3be7d46717ebff412d13d76efbd281443651d

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

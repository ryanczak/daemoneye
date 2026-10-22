# Phase 07: top-data — the `daemoneye top` snapshot and `top --once`

**Milestone:** M30 — Telemetry & dashboard
**Status:** in-progress (refined after a no-progress stall, 2026-10-21)
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

# Phase 07: top-data — the `daemoneye top` snapshot and `top --once`

**Milestone:** M30 — Telemetry & dashboard
**Status:** todo
**Depends on:** phase-06
**Estimated diff:** ~400 lines, about half of it tests
**Tags:** language=rust, kind=feature, size=m

## Goal

`daemoneye top` is a read-only dashboard with four panels: Runs, Budget,
Shells and Activity. This phase builds what the panels show, not the live
screen. It adds a pure `TopSnapshot` model, the loaders that fill it from
disk and from one read-only IPC request, a stats-only fingerprint that the
live loop (phase 08) re-reads on, and `daemoneye top --once`, which prints
one frame as plain text and exits. It works with the daemon down.

## Spec

**Files:** `src/cli/top/` (new: `mod.rs`, `snapshot.rs`, `text.rs`),
`src/cli/mod.rs`, `src/main.rs`, `src/ipc.rs`,
`src/daemon/server/shells.rs`, `README.md`.

1. **`TopSnapshot`**: one struct per panel, plus `taken_at`.
   - **Runs:** one row per run from `run::list_runs()`, capped at 20.
     Running runs come first, then the rest newest first.
     - Each row carries `id`, `kind`, `state`, `turns` and `age_s`, the
       time since the run's `RunStarted`.
     - Each row also carries `liveness`. For a running run whose turn log
       was written in the last 30 s it is `live`. For an older running run
       it is `stale`. For a finished run it is `done`.
   - **Budget:** read from today's events, local day (`local_day_bounds`
     in `daemon::spend`). It holds `ai_cost` USD and tokens (all four
     buckets), the cap from `[budget] max_cost_per_day` (`0` means none),
     the count of today's `budget_refused` events, and `tok_per_s`.
     `tok_per_s` is the tokens of `ai_cost` lines in the last 60 s,
     divided by 60. Top runs as its own process, so it reads the events,
     not the daemon's in-memory ledger.
   - **Shells:** the daemon's `Request::ShellList`, one row per shell. A
     row adds `foreground` (the record's `last_foreground`, or `in_flight`
     when that is newer) and `last_line` (the last non-empty line of the
     shell's latest recorded command output, masked). Both are new
     `#[serde(default)]` fields on `ShellInfo`, filled in
     `handle_shell_list`. When the daemon is down the panel holds
     `daemon_down = true` and no rows.
   - **Activity:** the last 50 events of today's segments, oldest first.
     Each one is a time, the event name and a one-line summary. A pure
     `filter_activity(events, needle)` keeps those whose name contains
     the needle. An empty needle keeps everything.
2. **Fingerprint.** `fingerprint() -> TopFingerprint` reads stats only,
   never file contents. It covers the runs directory (entry count, newest
   mtime) and today's events segment (length, mtime). Phase 08 re-reads
   the disk panels only when this changes. Shells are polled over IPC
   regardless.
3. **`daemoneye top [--once] [--filter <name>]`.**
   - `--once` prints the snapshot as text, one section per panel in the
     order Runs, Budget, Shells, Activity, and exits 0.
   - The Budget section names spend, cap and percentage when a cap is
     set. Without one it names the spend and `no cap`. It prints the
     refusal count as `refused today: N`.
   - The Shells section prints `daemon not running` when the daemon is
     down. Runs and Activity print `none` when empty.
   - `--filter` applies `filter_activity`.
   - Without `--once`, print `daemoneye top: the live view arrives in a
     later release; use --once` and exit 2 (phase 08 replaces this).
   - Build each section in a pure function in `text.rs`.
   - Add the command rows to the README, next to `scorecard`.
4. **Read-only.** Top sends no `Request` other than `ShellList`, and
   writes no file. Gotcha: the CLI seeds `~/.daemoneye` on first run;
   that is existing behaviour, not a top write.

## Tests

- In `snapshot.rs`:
  - `runs_put_running_first_then_newest`.
  - `liveness_is_live_stale_or_done` (pure, given the now and an mtime).
  - `budget_sums_todays_ai_cost_and_counts_refusals`, over parsed event
    values; no filesystem.
  - `tok_per_s_counts_only_the_last_minute`.
  - `filter_activity_matches_on_the_event_name`. `ai_cost` keeps
    `ai_cost` lines and drops `budget_refused`. An empty needle keeps all.
  - `fingerprint_changes_when_the_segment_grows`, on a temp dir.
- In `text.rs`:
  - `budget_text_names_spend_cap_and_percent`, with spend 0.5 and cap 2.0
    giving `25%`. Cap 0 gives `no cap`.
  - `shells_text_says_daemon_not_running`.

## Acceptance criteria

- [ ] Each new test ⇒ `test result: ok. 1 passed`.
- [ ] On the § End-to-end fixture, with the daemon down: `once_exit=0`,
      `budget_usd=1`, `budget_cap=1`, `refused_count=1`,
      `shells_down=1`, `activity_lines=3`, and with `--filter ai_cost`
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
for t in runs_put_running_first_then_newest liveness_is_live_stale_or_done budget_sums_todays_ai_cost_and_counts_refusals tok_per_s_counts_only_the_last_minute filter_activity_matches_on_the_event_name fingerprint_changes_when_the_segment_grows budget_text_names_spend_cap_and_percent shells_text_says_daemon_not_running; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
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
echo "shells_down=$(echo "$O" | grep -c 'daemon not running')" >> "$A"
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

- The ratatui screen, the 500 ms poll, follow and scroll (phase 08).
- Any IPC request other than `ShellList`, and any action on a run or a
  shell.
- Changing `daemoneye status` or `run list`.

## Update Log

<!-- entries appended below this line -->

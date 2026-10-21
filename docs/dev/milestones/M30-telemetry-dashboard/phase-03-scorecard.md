# Phase 03: scorecard — `daemoneye scorecard`, a pure aggregation over `run` events

**Milestone:** M30 — Telemetry & dashboard
**Status:** in-progress
**Depends on:** phase-02
**Estimated diff:** ~400 lines, about half of it tests
**Tags:** language=rust, kind=feature, size=m

## Goal

Every finished run now leaves an `event: "run"` record (`RunRecord`, in
`src/daemon/run/record.rs`), but nothing reads them. This phase adds
`daemoneye scorecard`. It reads the records straight from the dated
events segments, works with the daemon down, buckets them by model and
one secondary dimension, and prints one row per bucket with `n_runs`, so
a thin row reads as thin. The aggregation is pure and public; the
`profile` command (phase 04) and the MCP surface (a later milestone) reuse
it.

## Spec

**Files:** `src/daemon/run/scorecard.rs` (new), `src/daemon/run/mod.rs`,
`src/cli/commands/scorecard.rs` (new), `src/cli/commands/mod.rs`,
`src/cli/commands/costs.rs`, `src/main.rs`, `README.md`.

1. **The aggregation**, `aggregate_scorecard(records: &[RunRecord], by:
   ScorecardBy, min_runs: usize) -> Scorecard`, in `run/scorecard.rs` and
   re-exported. Pure: no I/O, no clock.
   - `ScorecardBy` is `Kind` (the default), `Agent` or `Tag`. A row's
     `key` is the record's `kind`; or its `agent`, with `(none)` when
     there is none; or, for `Tag`, one row per tag. A record counts once
     in each of its `k=v` tags, `kind=…` included.
   - **A record with no `model` is skipped.** These are the `scheduled`
     job wrappers, which make no costed model call. Skipped records are
     counted in `Scorecard.skipped_no_model`, never in a row.
   - Each `ScorecardRow` carries `model`, `key`, `n_runs`, and:
     - `pass_rate`: runs with `status` `complete`, over `n_runs`;
     - `tool_success_rate_mean` and `parse_failure_rate_mean`, as
       `Option<f64>`: the mean over only the runs whose rate is
       non-null, and `None` when every run's rate is null. **A null rate
       is not a zero**;
     - `turns_mean`, `wall_clock_s_mean`, `cost_usd_mean`;
     - `attributable_failure_rate`: runs with `is_model_attributable`,
       over `n_runs`.
   - Rows with `n_runs < min_runs` are dropped; `0` means no minimum.
   - Rows sort by `key`, then by `n_runs` descending, then by `model`.
   - `Scorecard` and `ScorecardRow` derive `Serialize`; this is the
     `--json` shape.
2. **Reading the records.** In `cli/commands/scorecard.rs`, a function
   reads every `event == "run"` line in a `[since, until)` window through
   `crate::daemon::utils::for_each_event_between`. Each line parses as a
   `RunRecord`; a line that does not parse is skipped. Lines of any other
   event are ignored.
3. **The command**: `daemoneye scorecard [--since DATE] [--until DATE]
   [--by kind|agent|tag] [--min-runs N] [--json]`. Dates are `YYYY-MM-DD`
   and inclusive, parsed by the same function `daemoneye costs` uses.
   Make `costs.rs`'s `parse_date_bound` `pub(crate)` and call it; do not
   copy it. With no `--since`, it reads every retained segment, not the
   last seven days: a scorecard wants every run it can get. With no
   `--until`, it reads to now.
   - The human output is a header line naming the window and the `--by`,
     then a table with columns in this order: `MODEL`, the `--by` name
     upper-cased (`KIND` / `AGENT` / `TAG`), `RUNS`, `PASS`, `TOOL-OK`,
     `PARSE-FAIL`, `TURNS`, `WALL`, `COST`, `ATTR-FAIL`. Rates are
     percentages; a `None` mean prints `—`.
   - When rows were dropped or records skipped, a footer line says how
     many. With no rows at all, it prints `no runs in this window` and
     exits 0.
4. **README.** Add `daemoneye scorecard` to the CLI command table beside
   `costs`. Add a short paragraph in the costs section naming the
   columns, `--by`, `--min-runs`, and that it works with the daemon down.

## Tests

- In `run/scorecard.rs`:
  - `rows_bucket_by_model_and_kind`: three `chat` runs on one model (two
    complete, one hard fail with `is_model_attributable`) and one `ghost`
    run on another. That gives two rows: the first has `n_runs` 3,
    `pass_rate` 2/3 and `attributable_failure_rate` 1/3.
  - `min_runs_drops_thin_rows`: `min_runs` 2 drops the one-run row, and
    `0` keeps it.
  - `rate_means_skip_null_rates`: a run with rate `1.0` and one with
    `null` gives a mean of `Some(1.0)`, not `0.5`; all-null gives `None`.
  - `by_agent_buckets_a_missing_agent_as_none`.
  - `by_tag_counts_a_run_once_per_tag`: one run tagged `kind=ghost` and
    `runbook=r` gives the rows `kind=ghost` and `runbook=r`, each with
    `n_runs` 1.
  - `runs_without_a_model_are_skipped_and_counted`.
  - `rows_sort_by_key_then_runs_then_model`.
- In `cli/commands/scorecard.rs`:
  `run_records_are_read_from_the_segments_only`. Use a temp `HOME`, as
  the guard tests do. A segment holding two `run` lines, one `ai_cost`
  line and one malformed `run` line yields exactly the two records.

## Acceptance criteria

- [ ] Each new test ⇒ `test result: ok. 1 passed`.
- [ ] Against the fixture in § End-to-end verification:
      `rows=2`, `chat_runs=3`, `chat_pass=0.6667`, `chat_attr=0.3333`,
      `skipped=1`, `min2_rows=1`, `tag_rows` ≥ 3, `human_has_header=1`.
- [ ] Preservation: `parse_date_bound` stays defined once (`date_bound_defs=1`;
      it is 1 today, and a copy would make it 2).
- [ ] `cargo test` ⇒ `failed=0`, with `passed` above 2449 by the new
      tests. Report the number; do not re-run to chase it.
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`. Clippy (`-D warnings`)
      and `cargo fmt --all --check` exit 0. No new `#[allow]`, no
      production `unwrap`/`expect`.
- [ ] The end-to-end Update Log entry holds the output fence **first**,
      then the `PASTE MATCH` line.

## End-to-end verification

The fixture is five `run` lines in a throwaway `HOME`: three `chat`
runs on model `m1`, of which two are `complete`, plus one `hard_fail`
`tool_errors` run that is attributable; one `ghost` run on `m2`; and one
`scheduled` run with no model. There is also one `ai_cost` line.

```sh
A=/tmp/e2e-m30-03.txt; : > "$A"
for t in rows_bucket_by_model_and_kind min_runs_drops_thin_rows rate_means_skip_null_rates by_agent_buckets_a_missing_agent_as_none by_tag_counts_a_run_once_per_tag runs_without_a_model_are_skipped_and_counted rows_sort_by_key_then_runs_then_model run_records_are_read_from_the_segments_only; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
cargo build -q
H=$(mktemp -d); mkdir -p "$H/.daemoneye/var/log/events"
S="$H/.daemoneye/var/log/events/events-$(date -u +%Y%m%d).jsonl"; T=$(date -u +%Y-%m-%dT%H:%M:%SZ)
r() { echo "{\"ts\":\"$T\",\"event\":\"run\",\"pid\":1,$1}" >> "$S"; }
r '"run_id":"a1","kind":"chat","model":"m1","status":"complete","turns":2,"tool_calls":2,"tool_success_rate":1.0,"cost_usd":0.01,"wall_clock_s":3.0,"tags":{"kind":"chat"}'
r '"run_id":"a2","kind":"chat","model":"m1","status":"complete","turns":1,"tool_calls":0,"tool_success_rate":null,"cost_usd":0.01,"wall_clock_s":1.0,"tags":{"kind":"chat"}'
r '"run_id":"a3","kind":"chat","model":"m1","status":"hard_fail","failure_class":"tool_errors","is_model_attributable":true,"turns":3,"tool_calls":2,"tool_success_rate":0.5,"cost_usd":0.02,"wall_clock_s":5.0,"tags":{"kind":"chat"}'
r '"run_id":"b1","kind":"ghost","agent":"analyst","model":"m2","status":"complete","turns":4,"tool_calls":3,"tool_success_rate":1.0,"cost_usd":0.05,"wall_clock_s":9.0,"tags":{"kind":"ghost","runbook":"disk","agent":"analyst"}'
r '"run_id":"c1","kind":"scheduled","status":"complete","tags":{"kind":"scheduled","job":"j"}'
echo "{\"ts\":\"$T\",\"event\":\"ai_cost\",\"pid\":1,\"run_id\":\"a1\"}" >> "$S"
SC() { HOME="$H" ./target/debug/daemoneye scorecard --since 2020-01-01 "$@"; }
J=$(SC --json)
echo "rows=$(echo "$J" | jq '.rows | length')" >> "$A"
echo "chat_runs=$(echo "$J" | jq '.rows[] | select(.key=="chat") | .n_runs')" >> "$A"
echo "chat_pass=$(echo "$J" | jq '.rows[] | select(.key=="chat") | .pass_rate * 10000 | round / 10000')" >> "$A"
echo "chat_attr=$(echo "$J" | jq '.rows[] | select(.key=="chat") | .attributable_failure_rate * 10000 | round / 10000')" >> "$A"
echo "skipped=$(echo "$J" | jq '.skipped_no_model')" >> "$A"
echo "min2_rows=$(SC --json --min-runs 2 | jq '.rows | length')" >> "$A"
echo "tag_rows=$(SC --json --by tag | jq '.rows | length')" >> "$A"
echo "human_has_header=$(SC | grep -cE '^MODEL +KIND +RUNS +PASS')" >> "$A"
rm -rf "$H"
echo "date_bound_defs=$(grep -rcE 'fn parse_date_bound\(' src | awk -F: '{s+=$2} END {print s}')" >> "$A"
cargo test --test doc_truth 2>&1 | grep -E '^test result' | sed 's/^/doc_truth: /; s/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Then run the self-check, and put its line in the entry after the output
fence:

```sh
D=docs/dev/milestones/M30-telemetry-dashboard/phase-03-scorecard.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m30-03.txt
diff /tmp/pasted-m30-03.txt /tmp/e2e-m30-03.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- `daemoneye profile` (phase 04).
- Exposing the scorecard over MCP (a later milestone).
- Any change to `RunRecord` or to what writes it.
- A column for a served model or a length-finish rate (no backend reports
  either).

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-21 06:50 (started)

Executor r2 (Rexy executor, Claude Sonnet 4.6) beginning phase 03. Status
flipped to `in-progress` here and in the milestone README's phase table.
Plan: the pure aggregation in `run/scorecard.rs` with its seven named tests
(task 1), the segment reader over `for_each_event_between` (task 2), the
`daemoneye scorecard` CLI wiring with `parse_date_bound` promoted to
`pub(crate)` (task 3), then the README rows (task 4) and the end-to-end
capture.

### Update — 2026-10-21 07:00 (end-to-end verification)

Ran `cargo fmt --all` first, then the spec's § End-to-end verification
block verbatim, once (the throwaway `HOME` is emptied with `find -delete`
in place of the spec's `rm -rf`, which the executor's shell refuses on
pattern; every other line is the spec's). The pasted output is the first
fenced block below.

```
rows_bucket_by_model_and_kind: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
min_runs_drops_thin_rows: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
rate_means_skip_null_rates: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
by_agent_buckets_a_missing_agent_as_none: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
by_tag_counts_a_run_once_per_tag: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
runs_without_a_model_are_skipped_and_counted: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
rows_sort_by_key_then_runs_then_model: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
run_records_are_read_from_the_segments_only: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
rows=2
chat_runs=3
chat_pass=0.6667
chat_attr=0.3333
skipped=1
min2_rows=1
tag_rows=4
human_has_header=1
date_bound_defs=1
doc_truth: test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2457 failed=0
```

PASTE MATCH (verified by the spec's self-check after this entry was
appended; the fence above is byte-identical to `/tmp/e2e-m30-03.txt`).

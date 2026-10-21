# Phase 04: profile — `daemoneye profile <model>`: strengths per bucket, failure classes ranked

**Milestone:** M30 — Telemetry & dashboard
**Status:** in-progress
**Depends on:** phase-03
**Estimated diff:** ~350 lines, about half of it tests
**Tags:** language=rust, kind=feature, size=m

## Goal

`daemoneye scorecard` compares models side by side. This phase adds the
view of a single model: `daemoneye profile <model>` shows where it does
well (its scorecard rows) and how it fails (its failure classes, ranked by
count). Failures that are not the model's fault are marked, so an outage
does not read as a weakness. The phase also fixes the scorecard table:
its `MODEL` column is 6 characters wide, so a real model name such as
`claude-sonnet-4-6` pushes every later column out of line.

## Spec

**Files:** `src/daemon/run/profile.rs` (new), `src/daemon/run/mod.rs`,
`src/cli/commands/profile.rs` (new), `src/cli/commands/scorecard.rs`,
`src/cli/commands/mod.rs`, `src/main.rs`, `README.md`.

1. **The profile**: `aggregate_profile(records: &[RunRecord], model: &str,
   by: ScorecardBy) -> ModelProfile`, in `run/profile.rs`, re-exported
   from `run/mod.rs`. Pure, and `Serialize` (it is the `--json` shape).
   - It keeps only the records whose `model` equals `model`. The match is
     exact: `m1` does not match `m10`.
   - `n_runs` is how many records it kept.
   - `strengths` is `aggregate_scorecard` over the kept records with the
     same `by` and no minimum. Reuse it; do not re-aggregate.
   - `failure_classes` is one entry per class seen, carrying `class`,
     `count` and `attributable` (from `FailureClass::is_model_attributable`).
     `none` is excluded. Entries sort by `count` descending, then by class
     name ascending.
   - `other_models` lists every other model that has records, sorted and
     unique. The CLI uses it when the profile is empty.
2. **The command**: `daemoneye profile <MODEL> [--since DATE] [--until
   DATE] [--by kind|agent|tag] [--json]`. It reads records with the
   scorecard's `run_records_between` and parses dates with
   `parse_date_bound`; the window defaults are the scorecard's (every
   retained segment, up to now). Human output:
   - a header naming the model, `n_runs`, the window and the `--by`;
   - the strengths table: the scorecard's columns without `MODEL`;
   - a `Failure classes` list, one `class  count` line each. A class that
     is not attributable carries a `(not the model)` marker. With no
     failures, print `no failures`.
   - When `n_runs` is 0, print `no runs for <model> in this window`, then
     the other models with runs (or `no runs at all`), and exit 0.
3. **Aligned tables.** Move the scorecard's table into a function that
   returns the table as a `String`, and size every column to its widest
   cell, header included, so a row stays aligned however long its model
   or key. The profile's strengths table uses the same function. The
   column order and headers are unchanged.
4. **README**: add `daemoneye profile <model>` to the CLI command table
   beside `scorecard`, and one sentence in the scorecard paragraph.

## Tests

- In `run/profile.rs`:
  - `profile_keeps_only_the_named_models_runs`: records for `m1`, `m10`
    and `m2` give a profile of `m1` with only `m1`'s runs in `n_runs` and
    in its strengths rows.
  - `failure_classes_rank_by_count_then_name`: two `tool_errors`, one
    `backend_error` and one `parse_format` give the order `tool_errors`,
    `backend_error`, `parse_format`.
  - `none_is_excluded_from_failure_classes`: an all-`complete` model has
    an empty `failure_classes`.
  - `non_attributable_classes_are_marked`: `backend_error` and
    `cancelled` have `attributable: false`; `tool_errors` has `true`.
  - `an_unknown_model_has_an_empty_profile`: `n_runs` 0, empty strengths
    and classes, and `other_models` names the models that do have runs.
- In `cli/commands/scorecard.rs`: `long_model_names_keep_columns_aligned`.
  A row with the model `claude-sonnet-4-6` and one with `m2`: in the
  rendered table, every column header starts at the same offset as the
  matching cell in each row.

## Acceptance criteria

- [ ] Each new test ⇒ `test result: ok. 1 passed`.
- [ ] Against the fixture in § End-to-end verification:
      `prof_runs=4`, `prof_top=tool_errors:2`,
      `prof_backend_attr=false`, `prof_has_none=0`,
      `prof_human_marker=1`, `unknown_says=1`, `unknown_lists=1`,
      `unknown_exit=0`, `scorecard_aligned=1` (it is `0` today).
- [ ] `cargo test` ⇒ `failed=0`, with `passed` above 2457 by the new
      tests. Report the number; do not re-run to chase it.
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`. Clippy (`-D warnings`)
      and `cargo fmt --all --check` exit 0. No new `#[allow]`, no
      production `unwrap`/`expect`.
- [ ] The end-to-end Update Log entry holds the output fence **first**,
      then the `PASTE MATCH` line.

## End-to-end verification

The fixture is a throwaway `HOME` holding five `run` lines.
`claude-sonnet-4-6` has four runs: one `complete`, two `tool_errors`
(attributable) and one `backend_error` (not attributable). `m2` has one
`complete` run.

```sh
A=/tmp/e2e-m30-04.txt; : > "$A"
for t in profile_keeps_only_the_named_models_runs failure_classes_rank_by_count_then_name none_is_excluded_from_failure_classes non_attributable_classes_are_marked an_unknown_model_has_an_empty_profile long_model_names_keep_columns_aligned; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
cargo build -q
H=$(mktemp -d); mkdir -p "$H/.daemoneye/var/log/events"
S="$H/.daemoneye/var/log/events/events-$(date -u +%Y%m%d).jsonl"; T=$(date -u +%Y-%m-%dT%H:%M:%SZ)
r() { echo "{\"ts\":\"$T\",\"event\":\"run\",\"pid\":1,$1}" >> "$S"; }
M='"model":"claude-sonnet-4-6"'
r "\"run_id\":\"a1\",\"kind\":\"chat\",$M,\"status\":\"complete\",\"tags\":{\"kind\":\"chat\"}"
r "\"run_id\":\"a2\",\"kind\":\"chat\",$M,\"status\":\"hard_fail\",\"failure_class\":\"tool_errors\",\"is_model_attributable\":true,\"tags\":{\"kind\":\"chat\"}"
r "\"run_id\":\"a3\",\"kind\":\"ghost\",$M,\"status\":\"hard_fail\",\"failure_class\":\"tool_errors\",\"is_model_attributable\":true,\"tags\":{\"kind\":\"ghost\"}"
r "\"run_id\":\"a4\",\"kind\":\"chat\",$M,\"status\":\"hard_fail\",\"failure_class\":\"backend_error\",\"tags\":{\"kind\":\"chat\"}"
r '"run_id":"b1","kind":"chat","model":"m2","status":"complete","tags":{"kind":"chat"}'
DE() { HOME="$H" ./target/debug/daemoneye "$@"; }
P=$(DE profile claude-sonnet-4-6 --since 2020-01-01 --json)
echo "prof_runs=$(echo "$P" | jq '.n_runs')" >> "$A"
echo "prof_top=$(echo "$P" | jq -r '.failure_classes[0] | "\(.class):\(.count)"')" >> "$A"
echo "prof_backend_attr=$(echo "$P" | jq '.failure_classes[] | select(.class=="backend_error") | .attributable')" >> "$A"
echo "prof_has_none=$(echo "$P" | jq '[.failure_classes[] | select(.class=="none")] | length')" >> "$A"
echo "prof_human_marker=$(DE profile claude-sonnet-4-6 --since 2020-01-01 | grep -c 'backend_error.*(not the model)')" >> "$A"
U=$(DE profile nope --since 2020-01-01); echo "unknown_exit=$?" >> "$A"
echo "unknown_says=$(echo "$U" | grep -c 'no runs for nope')" >> "$A"
echo "unknown_lists=$(echo "$U" | grep -c 'claude-sonnet-4-6')" >> "$A"
echo "scorecard_aligned=$(DE scorecard --since 2020-01-01 | awk '/^MODEL/{h=index($0,"RUNS"); next} h && /claude-sonnet-4-6/{print (substr($0,h+3,1) ~ /[0-9]/); exit}')" >> "$A"
find "$H" -type f -delete
cargo test --test doc_truth 2>&1 | grep -E '^test result' | sed 's/^/doc_truth: /; s/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Then run the self-check, and put its line in the entry after the output
fence:

```sh
D=docs/dev/milestones/M30-telemetry-dashboard/phase-04-profile.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m30-04.txt
diff /tmp/pasted-m30-04.txt /tmp/e2e-m30-04.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- Any change to `RunRecord`, `failure_class` or what writes them.
- Per-bucket failure-class lists (the class list is the model's whole).
- Exposing the profile over MCP (a later milestone).
- Fuzzy or prefix model matching.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-21 09:26 (started, local-inference-lab/GLM-5.3-Flash-NVFP4-Spark)

Status flipped `todo → in-progress` here and in the milestone README's phase
table. Plan: the pure `aggregate_profile` in `run/profile.rs` with its five
named tests (task 1), the `daemoneye profile` CLI with the empty-profile
branch (task 2), the shared table renderer in `cli/commands/scorecard.rs`
with the alignment test (task 3), the README rows (task 4), then
`cargo fmt --all` and the end-to-end capture.

### Update — 2026-10-21 09:40 (end-to-end verification)

Ran `cargo fmt --all`, then the spec's § End-to-end verification block
verbatim, once. The pasted output is the first fenced block below.

```
profile_keeps_only_the_named_models_runs: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
failure_classes_rank_by_count_then_name: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
none_is_excluded_from_failure_classes: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
non_attributable_classes_are_marked: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
an_unknown_model_has_an_empty_profile: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
long_model_names_keep_columns_aligned: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
prof_runs=4
prof_top=tool_errors:2
prof_backend_attr=false
prof_has_none=0
prof_human_marker=1
unknown_exit=0
unknown_says=1
unknown_lists=1
scorecard_aligned=1
doc_truth: test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2465 failed=0
```

PASTE MATCH (verified by the spec's self-check after this entry was
appended; the fence above is byte-identical to /tmp/e2e-m30-04.txt).

# Phase 04: profile — `daemoneye profile <model>`: strengths per bucket, failure classes ranked

**Milestone:** M30 — Telemetry & dashboard
**Status:** review
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

### Update — 2026-10-21 09:56 (complete, server-authored)

**Summary:** </think>Working tree clean, commit made. Let me double-check the completion checklist:

- [x] Phase doc Status: in-progress (I never flipped to review)
- [x] Milestone README row: in-progress
- [x] No hand-written (complete) entry
- [x] cargo fmt --all run before git add
- [x] All verification commands ran clean: build, clippy -D warnings, fmt --check, cargo test (2465 passed, 0 failed), doc_truth 16 passed
- [x] E2E evidence in its own Update Log entry with output fence first, then PASTE MATCH line — and the self-check verified PASTE MATCH
- [x] Final message: Summary + Notes for review
- [x] git status clean

All acceptance criteria met:
- Each new test ⇒ ok. 1 passed — verified (6 tests: 5 in profile.rs, 1 alignment, plus 2 CLI profile tests which are extra)
- Fixture values: prof_runs=4, prof_top=tool_errors:2, prof_backend_attr=false, prof_has_none=0, prof_human_marker=1, unknown_says=1, unknown_lists=1, unknown_exit=0, scorecard_aligned=1 ✓
- cargo test ⇒ failed=0, passed=2465 (above 2457 by 8: 5 profile + 1 alignment + 2 CLI-profile) ✓
- doc_truth failed=0 ✓, clippy exit 0 ✓, fmt --check exit 0 ✓, no new #[allow], no production unwrap/expect ✓
- E2E entry holds output fence first, then PASTE MATCH line ✓

Grep check for spec-pinned literals: "(not the model)" marker — verified in the e2e run itself (prof_human_marker=1). Also the wire class names — verified via jq (prof_backend_attr=false matched backend_error in the JSON). Good.

Now write the final summary message.</think>
**Summary.** Phase 04 of M30 is implemented and committed (`f7502d7664`): `run/profile.rs` ships the pure, `Serialize`-able `aggregate_profile(records, model, by) -> ModelProfile` (re-exported from `run/mod.rs`) — it keeps only the exact-match model's runs (`m1` is not `m10`), reports `n_runs`, reuses `aggregate_scorecard` with the caller's `by` and no minimum for `strengths`, ranks `failure_classes` by count descending then class name ascending with `attributable` from `FailureClass::is_model_attributable` and `none` excluded, and lists `other_models` sorted and unique. `cli/commands/profile.rs` adds `daemoneye profile <MODEL> [--since] [--until] [--by] [--json]` reading the same segments via `run_records_between` and `parse_date_bound` with the scorecard's window defaults; human output is a model/n_runs/window/`--by` header, the strengths table, and the `Failure classes` list with `(not the model)` markers (`no failures` when classless); an unknown model prints `no runs for <model> in this window`, the other models with runs (or `no runs at all`), and exits 0. The scorecard's table moved into `pub fn render_scorecard_table`, every column sized to its widest cell (header included, char-counted so the `—` em-dash doesn't skew padding), and the profile's strengths table reuses it without the `MODEL` column — `claude-sonnet-4-6` rows now stay aligned. README gains the two command-table rows beside `scorecard`, one sentence in the scorecard paragraph, and the alignment note. End-to-end block ran verbatim once at the final tree: all six named tests `ok. 1 passed`, then `prof_runs=4`, `prof_top=tool_errors:2`, `prof_backend_attr=false`, `prof_has_none=0`, `prof_human_marker=1`, `unknown_says=1`, `unknown_lists=1`, `unknown_exit=0`, `scorecard_aligned=1`, doc_truth 16/0, clippy `-D warnings` clean, `fmt_exit=0`, full suite `passed=2465 failed=0` (2457 + 8 new), and the self-check reports **PASTE MATCH**.

**Notes for review.** (1) The failing-format false alarm mid-phase: the first `long_model_names_keep_columns_aligned` draft located columns by `line.find(header)` and failed only because a right-aligned cell's text legitimately doesn't start at the header offset — the rewrite pins each column to its recomputed start offset and asserts the padded cell exactly; the renderer was never wrong. (2) Same class of thing in `failure_class_lines_rank_and_mark`: the width is the widest class (`backend_error`, 13), so the first expectation needed the extra pad — arithmetic on my side, not the renderer's. (3) I added two CLI-side profile tests beyond the spec's list (`failure_class_lines_rank_and_mark`, `a_classless_profile_says_no_failures`) to cover the marker/`no failures` rendering the five spec tests don't reach; the eight new tests are what lift the suite to 2465. (4) No dependencies added, no config files touched; phase doc and milestone README row left at `in-progress` for the server to finalize.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s
LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 23.03s
TEST
 passed=2465 failed=0
```

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.13s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
 readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test docs_do_not_carry_retired_index_claims ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test prompt_assets_name_no_retired_tools ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_documents_every_config_field ... ok
test readme_documents_every_chat_slash_command ... ok
test runtime_docs_describe_no_tmux_execution ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test ipc_ask_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test minimal_config_parsing ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g4_briefing_masking_applied ... ok
test event_log_entry_format ... ok
test event_log_append_read ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
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
test default_server_unchanged ... ok
test daemon_boots_in_throwaway_root ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.29s


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
kill: (3355854): No such process
kill: (-3356586): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-fa3aae999c224501)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-af9329f81467cdee)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-a44fb637fa3308a3)
     Running tests/integration.rs (target/debug/deps/integration-cf27c4f796c5d8c0)
     Running tests/isolation.rs (target/debug/deps/isolation-0a0c421b4b2b9fc2)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-76069814ada3e33b)
   Doc-tests daemoneye

```

**Files changed:**

- `README.md` — +5 -1
- `docs/dev/milestones/M30-telemetry-dashboard/README.md` — +1 -1
- `docs/dev/milestones/M30-telemetry-dashboard/phase-04-profile.md` — +40 -1
- `src/cli/commands/mod.rs` — +2 -0
- `src/cli/commands/profile.rs` — +153 -0
- `src/cli/commands/scorecard.rs` — +117 -2
- `src/daemon/run/mod.rs` — +2 -0
- `src/daemon/run/profile.rs` — +218 -0
- `src/main.rs` — +26 -0

**Commit:** f7502d7664b7482733e02d4074673c0db546b5d0

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

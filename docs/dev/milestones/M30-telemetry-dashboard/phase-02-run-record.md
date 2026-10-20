# Phase 02: run-record — one `run` event per finished run; `failure_class` from the tally

**Milestone:** M30 — Telemetry & dashboard
**Status:** todo
**Depends on:** phase-01
**Estimated diff:** ~350 lines, about half of it tests
**Tags:** language=rust, kind=feature, size=m

## Goal

Phase 01 gave every run a `RunMetrics` tally on its `RunLogGuard`, but
nothing reads it. This phase turns it into one `event: "run"` record in
the dated events segments when each run ends. The record carries the run's
model, its outcome, a computed `failure_class` and the tally. It also
fills the ghost `RunResult`'s `Budget` with what the run actually spent.
Readers (`scorecard`, `profile`) are phases 03–04.

## Spec

**Files:** `src/daemon/run/record.rs` (new), `src/daemon/run/mod.rs`,
`src/daemon/run/metrics.rs`, `src/daemon/run/guard.rs`,
`src/daemon/stream.rs`, `src/daemon/ghost.rs`, `src/ai/types/wire.rs`,
`docs/design-reference.md`.

1. **The tally grows two counts.** `RunMetrics` gains `turns: u32` with
   `record_turn()`, and `backend_errors: u32` with
   `record_backend_error()`, both `saturating_add`. Feed them:
   - **turns**: the ghost loop calls it wherever it emits `TurnStarted`;
     the chat loop calls it once per iteration that sends a request to the
     model. Scheduled jobs record none.
   - **backend errors**: every `AiEvent::Error` arm in both loops, and the
     chat loop's give-up when the stream closes without completing after
     its retries.
   - **model**: both loops set `metrics.provider` / `metrics.model` from
     the client's configured provider and model **before** the first model
     call. That way a run whose first call fails still names its model;
     `record_model_call` overwrites them as before.
2. **`failure_class`**: a pure function in `run/record.rs` taking
   `(&RunState, &RunMetrics)`, plus `is_model_attributable(class) -> bool`
   and a `FAILURE_CLASSES` list of every class. Rules, in order:
   - `Complete` ⇒ `none`; `Cancelled` ⇒ `cancelled`; `BudgetExceeded` ⇒
     `budget_exceeded`. The tally is ignored for these: a complete run
     with failed tools is still `none`.
   - `HardFail`, first match wins: `backend_errors > 0` ⇒
     `backend_error`; else `parse_failures > 0` ⇒ `parse_format`; else
     `tool_successes < tool_calls` ⇒ `tool_errors`; else `other`.
   - `stall` is in `FAILURE_CLASSES` but nothing produces it yet (the
     governor is a later milestone).
   - Only `none`, `cancelled`, `budget_exceeded`, `backend_error` and
     `other` are **not** model-attributable.
3. **`RunRecord`** in `run/record.rs`, re-exported from `run/mod.rs`:
   `Serialize + Deserialize + Default`, `#[serde(default)]` on the struct
   so later readers parse older or partial lines. Fields: `run_id`, `kind`
   (the `RunKind::label()`), `agent: Option<String>`, `provider`, `model`
   (`Option<String>`), `status` (`complete` | `hard_fail` |
   `budget_exceeded` | `cancelled`), `failure_class`,
   `is_model_attributable`, `turns`, `tool_calls`, `tool_success_rate`
   and `parse_failure_rate` (`Option<f64>`: JSON `null` with no tool
   calls), `tokens` (`TokenBreakdown`), `cost_usd`, `wall_clock_s: f64`,
   `tags: BTreeMap<String, String>`. Build it with a pure constructor from
   the run id, kind label, state, metrics, wall clock and tags. No field
   may be named `ts`, `event` or `pid`, because `log_event` adds those.
   `TokenBreakdown` derives only `Serialize` today; add `Deserialize`.
4. **The guard emits it.** `RunLogGuard` records an `Instant` at `start`,
   and holds the tags. The tags are seeded at `start`: `kind` is always
   set, plus `runbook` for `Ghost`, `agent` for `Delegated` and `job` for
   `Scheduled`. A `tag(key, value)` method adds more. The ghost loop tags
   `agent` and `host` (the ghost's `ssh_target`) when it has them. The
   record's `agent` is the `agent` tag. In `Drop`, after the
   `RunFinished` append, the guard builds the `RunRecord` from the final
   state and calls `log_event("run", ..)`. A serialization error is logged
   and skipped, never a panic. Background command runs (`bg_shell`) do not
   use the guard and must emit no `run` record: they make no model call,
   and their kind is `Chat`, so a record would double the chat count.
5. **The ghost's real spend.** The ghost `RunResult` is built with
   `Budget::unlimited()`. Build it instead from the limits the run ran
   under: `max_turns` is the effective ghost turn cap the loop enforced,
   and the other four are `0`, since no other cap exists yet. Then record
   the tally into it: turns, tool calls, `tokens.total()`, wall-clock
   seconds, and `cost_usd` as micro-USD, rounded. A
   `RunMetrics`/`Budget` helper that does the folding is the expected
   shape.
6. **Document the event** in `docs/design-reference.md`, beside the
   `ai_cost` schema: one example `run` line, the class rules and the
   attributable set.

## Tests

- In `run/record.rs`:
  - `terminal_states_map_to_their_class`: `Complete`, `Cancelled` and
    `BudgetExceeded`, each with a tally that has failed tools, map to
    `none`, `cancelled` and `budget_exceeded`.
  - `hard_fail_is_classified_by_the_tally`: a backend error plus parse
    failures gives `backend_error`; parse failures plus failed tools give
    `parse_format`; failed tools alone give `tool_errors`; a clean tally
    gives `other`.
  - `only_model_failures_are_attributable`: walks every `FAILURE_CLASSES`
    entry; `tool_errors`, `parse_format` and `stall` are attributable, the
    other five are not.
  - `a_run_record_round_trips_and_old_lines_parse`: serialize then parse
    is equal, and `{}` parses to the default record.
  - `a_run_record_without_tool_calls_has_null_rates`: the JSON has
    `"tool_success_rate":null`.
- In `run/metrics.rs`: `the_tally_folds_into_a_budget`. A tally of 3
  turns, 4 tool calls, a known token total and $0.0125 leaves exactly
  those amounts spent; the USD dimension reads 12 500 micro-USD.
- In `run/guard.rs`:
  - `guard_drop_emits_one_run_event`: a `Ghost` run, tagged `agent` and
    `host`, with one model call, finished `HardFail` with failed tools.
    After drop, today's events segment holds exactly one `event == "run"`
    line, with this run id, `kind` `ghost`, the runbook, agent and host
    tags, `status` `hard_fail`, `failure_class` `tool_errors`, the model,
    and `wall_clock_s >= 0`.
  - `guard_without_finish_records_class_other`: an unfinished guard's
    record is `hard_fail` / `other`, not attributable.

## Acceptance criteria

- [ ] Each new test ⇒ `test result: ok. 1 passed`.
- [ ] Both loops feed turns (`stream_turns=` and `ghost_turns=` ≥ 1), and
      backend errors (`stream_backend=` ≥ 2, `ghost_backend=` ≥ 1).
- [ ] `ghost_unlimited_budget=0` (it is `1` today).
- [ ] Preservation: `bg_shell_run_records=0`.
- [ ] `docs/design-reference.md` names the `run` event
      (`design_ref_run_event=` ≥ 1; `0` today).
- [ ] `cargo test` ⇒ `failed=0`, with `passed` above 2438 by the new tests.
      Report the number; do not re-run to chase it.
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`. Clippy (`-D warnings`)
      and `cargo fmt --all --check` exit 0. No new `#[allow]`, no
      production `unwrap`/`expect`.
- [ ] The end-to-end Update Log entry holds the output fence **first**,
      then the `PASTE MATCH` line.

## End-to-end verification

```sh
A=/tmp/e2e-m30-02.txt; : > "$A"
for t in terminal_states_map_to_their_class hard_fail_is_classified_by_the_tally only_model_failures_are_attributable a_run_record_round_trips_and_old_lines_parse a_run_record_without_tool_calls_has_null_rates the_tally_folds_into_a_budget guard_drop_emits_one_run_event guard_without_finish_records_class_other; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
echo "stream_turns=$(grep -c 'record_turn()' src/daemon/stream.rs)" >> "$A"
echo "ghost_turns=$(grep -c 'record_turn()' src/daemon/ghost.rs)" >> "$A"
echo "stream_backend=$(grep -c 'record_backend_error()' src/daemon/stream.rs)" >> "$A"
echo "ghost_backend=$(grep -c 'record_backend_error()' src/daemon/ghost.rs)" >> "$A"
echo "ghost_unlimited_budget=$(grep -c 'Budget::unlimited()' src/daemon/ghost.rs)" >> "$A"
echo "bg_shell_run_records=$(grep -cE 'RunRecord|log_event\("run"' src/daemon/executor/bg_shell.rs)" >> "$A"
echo "design_ref_run_event=$(grep -c '"event": "run"' docs/design-reference.md)" >> "$A"
cargo test --test doc_truth 2>&1 | grep -E '^test result' | sed 's/^/doc_truth: /; s/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Then run the self-check, and put its line in the entry after the output
fence:

```sh
D=docs/dev/milestones/M30-telemetry-dashboard/phase-02-run-record.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m30-02.txt
diff /tmp/pasted-m30-02.txt /tmp/e2e-m30-02.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- Any reader of `run` events: `scorecard` (phase 03) and `profile`
  (phase 04).
- Producing `stall` (the governor, a later milestone).
- The daily cap, and the five uncosted model calls (phase 05).
- A record for background command runs (`bg_shell`).
- Ghost caps other than turns.

## Update Log

<!-- entries appended below this line -->

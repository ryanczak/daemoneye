# Phase 01: run-metrics — a per-run tally on the run guard; `run_id` on every `ai_cost`

**Milestone:** M30 — Telemetry & dashboard
**Status:** in-progress
**Depends on:** none
**Estimated diff:** ~250 lines, about half of it tests
**Tags:** language=rust, kind=feature, size=s

## Goal

A run cannot say how it went. Nothing counts its tool calls, their
failures, its invalid-argument calls or its model spend, and an `ai_cost`
event cannot be joined to the run that made it. This phase adds the tally
and the join. Phase 02 turns the tally into a `run` event; nothing new is
emitted here except the `run_id` key.

## Spec

**Files:** `src/daemon/run/metrics.rs` (new), `src/daemon/run/mod.rs`,
`src/daemon/run/guard.rs`, `src/daemon/executor/mod.rs`, `src/cost.rs`,
`src/daemon/stream.rs`, `src/daemon/ghost.rs`, `tests/integration.rs`.

1. **`RunMetrics`** in `run/metrics.rs`, re-exported from `run/mod.rs`. A
   plain `Default` struct, no I/O. It counts `tool_calls`,
   `tool_successes`, `parse_failures` and `model_calls`; sums a
   `TokenBreakdown` (each bucket `saturating_add`) and `cost_usd: f64`; and
   keeps `provider` and `model` as `Option<String>`, the most recent model
   call's. Methods:
   - `record_model_call(&mut self, provider, model, &TokenBreakdown, cost_usd)`;
   - `tool_success_rate()` and `parse_failure_rate()` return
     `Option<f64>`, **`None` when `tool_calls` is 0**, never `0.0` or NaN.
     Both rates divide by `tool_calls`.
2. **One success rule.** Add `ToolCallOutcome::succeeded(&self) ->
   Option<bool>` in `executor/mod.rs`:
   - `Result(s)`: `Some(false)` when `s` **starts with** `Error:` or
     `error:`, otherwise `Some(true)`;
   - `SpawnGhostSession`: `Some(true)`;
   - `UserMessage`: `None`, because a redirect aborts the chain and is not
     a tool result.

   `tool_outcome_detail` must take its `ok` from `succeeded()`
   (`UserMessage` stays `false` there). Add
   `pub fn tally_tool_outcome(m: &mut RunMetrics, call: &PendingCall,
   outcome: &ToolCallOutcome)`: when `succeeded()` is `Some(ok)` it counts
   one tool call, plus one success if `ok`, plus one parse failure if
   `call` is `PendingCall::InvalidArgs`. `None` counts nothing.
3. **The guard carries the tally.** `RunLogGuard` holds a `RunMetrics`,
   empty at `start`, with `metrics(&self)` and `metrics_mut(&mut self)`.
   The `Drop` impl does not use it yet.
4. **`CostRecord.run_id: String`**, with `#[serde(default)]` so an older
   line without it parses to `""`. Every literal sets it to the run's id.
5. **Feed it, at the sites the read found:**
   - **Chat** (`run_conversation_loop`): at both `ai_cost` sites (the
     final-answer `AiEvent::Done` path and the tool-call path), set
     `run_id` from `run_log.run_id()` and call
     `run_log.metrics_mut().record_model_call(..)` with the same provider,
     model, tokens and `cost.total_cost_usd`. After `execute_tool_call`
     returns and **before** `match outcome` consumes it, call
     `tally_tool_outcome`.
   - **Ghost** (`do_ghost_turn`): its one `ai_cost` site sets `run_id`
     from its `run_id` parameter and records the call; the tool loop
     tallies each outcome before matching it. `trigger_ghost_turn` must
     leave the ghost's tally on its `RunLogGuard` before `run_log.finish`.
6. **Capture the end-to-end evidence.** Run `cargo fmt --all` first. Then
   run § End-to-end verification verbatim, **once**. Paste the **output**
   (`/tmp/e2e-m30-01.txt`) into a new Update Log entry headed
   `### Update — <date> (end-to-end verification)` as the **first** fenced
   block in that entry, then the `PASTE MATCH` line. Do not paste the
   command block.

**Gotchas.**
- `GhostLoopOutcome` derives `Copy` and `Eq`, and three tests build it as a
  literal. `RunMetrics` holds an `f64` and a non-`Copy` `TokenBreakdown`,
  so do **not** add it to `GhostLoopOutcome`. Pass the tally to
  `do_ghost_turn` separately.
- Calls the daemon refuses **before** executing them (the per-session,
  per-turn and per-tool caps in the chat loop) are not tallied: only an
  outcome from `execute_tool_call` counts.
- The prefix rule is a prefix. A result such as `grep: found "Error: x"`
  is a **success**.
- `tests/integration.rs` builds a `CostRecord` literal too.
- Find anchors with `grep -n`; read windows, not single lines.

## Tests

- In `run/metrics.rs`:
  - `empty_metrics_have_no_rates`: both rates are `None`.
  - `tool_success_rate_is_successes_over_calls`: 4 calls, 3 successes ⇒
    `Some(0.75)`.
  - `parse_failure_rate_is_over_all_tool_calls`: 4 calls, 1 parse
    failure ⇒ `Some(0.25)`.
  - `model_calls_sum_tokens_and_cost`: two calls sum every token bucket
    and the cost, count 2, and keep the second call's model.
- In `executor/mod.rs`:
  - `tally_tool_outcome_classifies_each_outcome`: a plain result is a
    success; `Error: x` and `error: x` are failures; `grep: found "Error:
    x"` is a success; a spawned ghost is a success; a `UserMessage` leaves
    every count at 0; an `InvalidArgs` call with an `Error:` result counts
    one call, no success and one parse failure.
- In `run/guard.rs`: `guard_holds_the_metrics_recorded_through_it`.
- In `cost.rs`:
  - `a_cost_record_carries_its_run_id`: the serialized JSON has `run_id`.
  - `a_cost_record_without_run_id_still_parses`: an older line parses and
    its `run_id` is `""`.

## Acceptance criteria

- [ ] Each new test ⇒ `test result: ok. 1 passed`.
- [ ] Both `run_conversation_loop` `ai_cost` sites and the `do_ghost_turn`
      one record the call on the run's metrics (end-to-end report lines
      `stream_model_calls=` and `ghost_model_calls=`).
- [ ] Both tool loops call `tally_tool_outcome` (report lines
      `stream_tally=` and `ghost_tally=`).
- [ ] `cargo test` ⇒ about `passed=2439 failed=0` (2430 at `9bc3ebe1`
      + 9). An extra test is fine: name it and report; do not re-run the
      suite to chase a number.
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`. Clippy (`-D warnings`)
      and `cargo fmt --all --check` exit 0. No new `#[allow]`, no
      production `unwrap`/`expect`.
- [ ] The end-to-end Update Log entry holds the output fence **first**,
      then the `PASTE MATCH` line.

## End-to-end verification

```sh
A=/tmp/e2e-m30-01.txt; : > "$A"
for t in empty_metrics_have_no_rates tool_success_rate_is_successes_over_calls parse_failure_rate_is_over_all_tool_calls model_calls_sum_tokens_and_cost tally_tool_outcome_classifies_each_outcome guard_holds_the_metrics_recorded_through_it a_cost_record_carries_its_run_id a_cost_record_without_run_id_still_parses; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
echo "stream_model_calls=$(grep -c 'record_model_call(' src/daemon/stream.rs)" >> "$A"
echo "ghost_model_calls=$(grep -c 'record_model_call(' src/daemon/ghost.rs)" >> "$A"
echo "stream_tally=$(grep -c 'tally_tool_outcome(' src/daemon/stream.rs)" >> "$A"
echo "ghost_tally=$(grep -c 'tally_tool_outcome(' src/daemon/ghost.rs)" >> "$A"
cargo test --test doc_truth 2>&1 | grep -E '^test result' | sed 's/^/doc_truth: /; s/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Then run the self-check, and put its line in the entry after the output
fence:

```sh
D=docs/dev/milestones/M30-telemetry-dashboard/phase-01-run-metrics.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m30-01.txt
diff /tmp/pasted-m30-01.txt /tmp/e2e-m30-01.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- Emitting any record from the tally, wall clock, turns, `failure_class`
  (phase 02).
- Feeding the ghost's `Budget` (phase 02).
- Model calls outside the two run loops: briefing, digest, auto-naming
  and the webhook and scheduled watchdogs emit no `ai_cost` today; that
  is the daily-cap phase's work.
- A length-finish rate: the backends report truncation only as a
  `Notice` string.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-20 15:37 (started)

Executor r1 (Rexy executor, Claude Opus 4.6) beginning phase 01. Status flipped
to `in-progress` here and in the milestone README's phase table. Plan: build
`RunMetrics` in `run/metrics.rs` (task 1), the `succeeded()` one-success rule
and `tally_tool_outcome` in `executor/mod.rs` (task 2), hang the tally on
`RunLogGuard` (task 3), add `CostRecord.run_id` with `#[serde(default)]` (task
4), feed both loops and the ghost's `do_ghost_turn` (task 5), then capture the
end-to-end evidence with the spec's block, run verbatim once (task 6).


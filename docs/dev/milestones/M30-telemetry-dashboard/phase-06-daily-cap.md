# Phase 06: daily-cap — `[budget] max_cost_per_day` refuses model calls over the cap

**Milestone:** M30 — Telemetry & dashboard
**Status:** in-progress
**Depends on:** phase-05
**Estimated diff:** ~350 lines, about a third of it tests
**Tags:** language=rust, kind=feature, size=m

## Goal

Phase 05 made today's spend knowable. This phase acts on it: with
`[budget] max_cost_per_day` set and today's spend at or over it, no model
call goes to a provider. A chat turn is refused with a `SystemMsg` naming
the spend and the cap. A ghost run or scheduled run ends
`BudgetExceeded`. Webhook watchdog analysis and the side calls (briefing,
digest, auto-naming) are skipped and logged. The phase also closes the
ledger race carried from phase 05.

## Spec

**Files:** `src/daemon/spend.rs`, `src/cost.rs`, `src/daemon/stream.rs`,
`src/daemon/ghost.rs`, `src/daemon/scheduled.rs`, `src/webhook/process.rs`,
`src/daemon/briefing.rs`, `src/daemon/digest.rs`, `src/daemon/auto_name.rs`.

1. **One gate**, `spend::cost_gate(config, path) -> Result<(), String>`.
   - It reads `spent_today()` and `config.budget.max_cost_per_day`, and
     decides with `cap_status`. `Off` and `Under` give `Ok`.
   - `Over` gives `Err(message)`. The message names the spend and the cap
     in dollars and says the cap resets at local midnight. The same
     message goes everywhere the refusal is shown or logged. Build it in a
     pure function.
   - On `Over` it also emits one `budget_refused` event with `path`,
     `spent_usd` and `cap_usd`.
   - `path` is one of `chat`, `ghost`, `scheduled`, `webhook`, `briefing`,
     `digest`, `auto_name`.
2. **Every model call passes the gate first**, before `make_client`, so
   nothing reaches a provider.
   - **Chat** (`run_conversation_loop`): at the top of every model-call
     iteration, tool-loop continuations included. On `Err`, send the
     message as `Response::SystemMsg`, finish the run `BudgetExceeded`
     with a briefing whose blocker is the message and whose dimension is
     `BudgetDimension::Usd`, and end the turn. The turn-timeout branch in
     the same loop is the analogue.
   - **Ghost** (`do_ghost_turn`): at the top of every ghost turn. On `Err`,
     end the loop without a wrap-up turn and record the refusal on
     `GhostLoopOutcome`. `ghost_terminal_state` maps it to
     `BudgetExceeded` with dimension `Usd` and the message as blocker. An
     error still beats it, and cancellation still beats it. The cost cap
     beats the turn budget.
   - **Scheduled** (`run_watchdog`): on `Err`, log a warning and skip the
     analysis. The scheduled run then ends `BudgetExceeded` (dimension
     `Usd`, the message as blocker) in place of the state
     `scheduled_terminal_state` gives. The job itself still ran and is
     still marked done as now.
   - **Webhook** (`maybe_analyze_alert`), **briefing**, **digest** and
     **auto-naming**: on `Err`, log a warning and return the path's
     existing "no result" value. The alert is still logged and the
     session goes on. Nothing is retried.
3. **The ledger race.** `emit_cost` writes the `ai_cost` line and then
   records the cost, outside one lock. A cold `spent_today()` that lands
   between the two seeds with the line, and the add then counts it again.
   The gate makes that read happen at every call. Constraint: whatever
   seeds the ledger runs **before** the line is written, and the seed, the
   write and the add happen while the ledger lock is held. A failed write
   still counts its cost. Gotcha: `log_event` must not take the ledger
   lock, and nothing it calls may.

## Tests

- In `spend.rs`:
  - `refusal_names_spend_and_cap`: the message holds both amounts as
    dollars.
  - `seed_runs_before_the_line_is_written`: on a cold ledger, a seed
    closure and a write closure record the order they ran in. The seed is
    first and the total is the seed plus the cost, exactly once. It fails
    against the current tree, where the write comes first.
- In `ghost.rs`:
  - `a_cost_cap_ends_the_ghost_budget_exceeded_usd`.
  - `cancellation_beats_the_cost_cap`.
  - `the_cost_cap_beats_the_turn_budget`.
- In `scheduled.rs`: `a_refused_watchdog_ends_the_scheduled_run_budget_exceeded`.
  Keep this test pure, as the existing `scheduled_terminal_state` tests
  are.

## Acceptance criteria

- [ ] Each new test ⇒ `test result: ok. 1 passed`.
- [ ] Every model-call site passes the gate: each of the seven files in
      § End-to-end reports `cost_gate` ≥ 1. All seven read `0` today.
- [ ] `budget_refused` is emitted from one place:
      `refused_emitters=1` (it is `0` today).
- [ ] `cargo test` ⇒ `failed=0`, with `passed` above 2474 by the new tests.
      Report the number; do not re-run to chase it.
- [ ] Clippy (`-D warnings`) and `cargo fmt --all --check` exit 0. No new
      `#[allow]`, no production `unwrap`/`expect`.
- [ ] The end-to-end Update Log entry holds the output fence **first**,
      then the `PASTE MATCH` line.

## End-to-end verification

```sh
A=/tmp/e2e-m30-06.txt; : > "$A"
for t in refusal_names_spend_and_cap seed_runs_before_the_line_is_written a_cost_cap_ends_the_ghost_budget_exceeded_usd cancellation_beats_the_cost_cap the_cost_cap_beats_the_turn_budget a_refused_watchdog_ends_the_scheduled_run_budget_exceeded; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
for f in src/daemon/stream.rs src/daemon/ghost.rs src/daemon/scheduled.rs src/webhook/process.rs src/daemon/briefing.rs src/daemon/digest.rs src/daemon/auto_name.rs; do echo "cost_gate $(basename $f)=$(grep -c 'cost_gate(' $f)" >> "$A"; done
echo "refused_emitters=$(grep -rE 'log_event\(\s*"budget_refused"' src --include=*.rs | wc -l)" >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Then run the self-check, and put its line in the entry after the output
fence:

```sh
D=docs/dev/milestones/M30-telemetry-dashboard/phase-06-daily-cap.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m30-06.txt
diff /tmp/pasted-m30-06.txt /tmp/e2e-m30-06.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- A per-run or per-agent cap, or a soft warning below the cap.
- `daemoneye top`'s budget panel (phase 07).
- Refusing anything that is not a model call: shells, scripts and tools
  still run.
- Cancelling a model call already in flight when the cap is crossed. The
  next call is the one refused.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-21 16:20 (started, executor)

Status flipped `todo` → `in-progress`; README phase-table row matches.
Beginning task 1 (one gate) and task 3 (the ledger race) — the gate's
`Ok`/`Err` shape and the seed-before-write constraint live in the same
`spend.rs` core. Task 2's seven model-call sites follow.

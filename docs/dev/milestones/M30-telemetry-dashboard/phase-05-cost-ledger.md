# Phase 05: cost-ledger — every model call costed, today's spend kept, `[budget] max_cost_per_day`

**Milestone:** M30 — Telemetry & dashboard
**Status:** todo
**Depends on:** phase-02
**Estimated diff:** ~400 lines, about half of it tests
**Tags:** language=rust, kind=feature, size=m

## Goal

The daily cap (phase 06) has to know today's spend, and today it can't.
Five model calls emit no `ai_cost` at all: briefing generation, the
session digest, auto-naming, and the webhook and scheduled watchdogs. The
spend that `status` shows also uses a UTC day, while the PE's cap uses
local midnight. This phase routes every model call through one cost
helper, keeps a process-wide total for the local day, adds the
`[budget] max_cost_per_day` setting (`0` means off) with a pure cap
decision, and shows spend against the cap in `daemoneye status`. Nothing
is refused yet; that is phase 06.

## Spec

**Files:** `src/cost.rs`, `src/daemon/spend.rs` (new), `src/daemon/mod.rs`,
`src/config/types.rs`, `assets/etc/config.toml`, `src/daemon/stream.rs`,
`src/daemon/ghost.rs`, `src/daemon/briefing.rs`, `src/daemon/digest.rs`,
`src/daemon/auto_name.rs`, `src/daemon/scheduled.rs`,
`src/webhook/process.rs`, `src/ipc.rs`, `src/daemon/server/handlers.rs`,
`src/cli/status.rs`.

1. **One cost helper**: `cost::emit_cost(..)`, given the resolved model
   entry, the call's `TokenBreakdown`, its attribution (session id,
   `CostAttribution`, run id) and the time. It resolves pricing as the
   ghost's cost site does today (`model_entry.pricing()`, falling back to
   the `Unknown` source at zero), builds the `CostRecord`, writes it with
   `log_event("ai_cost", ..)`, adds its total to the daily ledger (task 2)
   and returns the record. A serialization failure is logged and the line
   skipped, never a panic; the ledger still counts the cost.
   - The three existing sites (both `run_conversation_loop` sites and the
     ghost's) call it. Their `ai_cost` lines are unchanged field for field,
     and they keep feeding `RunMetrics` and the session totals as now.
   - The five uncosted calls call it when their `AiEvent::Done(usage)`
     arrives, with `agent_name` set to `briefing`, `digest`, `auto_name` or
     `watchdog` (webhook and scheduled both), and `run_id` set to `""`.
     Only `briefing.rs` matches `Done` today. The digest, auto-naming and
     both watchdog loops drain with `if let AiEvent::Token(t)` and drop
     every other event, so each needs a `Done` arm. Their handling of
     `Token` is unchanged.
   - Gotcha: `record_model_call` is already a `RunMetrics` method. The
     helper is `emit_cost`; do not reuse that name.
2. **The daily ledger**, in `src/daemon/spend.rs`: a process-wide total
   for the **local** calendar day.
   - The pure core is a `Ledger { day, total }`. Reading it for a day other
     than the one it holds re-seeds it once from a supplied function;
     adding to it adds to the current day.
   - The seed sums `ai_cost` `cost.total_cost_usd` from local midnight to
     now, using `sum_cost_between`, so a restarted daemon picks up what it
     already spent today.
   - `local_day_bounds(now)` gives that window in UTC. Local midnight can be
     ambiguous or missing on a DST day: take the earliest valid instant,
     never `unwrap`.
   - `spent_today() -> f64` is the public read. The lock uses
     `unwrap_or_log`, as every lock does.
3. **`[budget] max_cost_per_day`**: a new `BudgetConfig` on `Config`,
   `#[serde(default)]`, an `f64` defaulting to `0.0` (off). A negative
   value warns at load and is treated as `0`.
   - Gotcha: `tests/doc_truth.rs` fails until every config field is
     documented in `assets/etc/config.toml`. Add a commented `[budget]`
     block there, saying what `0` means and that the day is local
     midnight.
   - The pure decision is `cap_status(spent, cap) -> CapStatus`: `Off`
     when `cap <= 0`; `Over { spent, cap }` when `spent >= cap`;
     otherwise `Under { spent, cap }`. Phase 06 calls it at every model
     call.
4. **Status.** `Response::DaemonStatus` gains `daemon_spend_today_usd` (the
   ledger's local-day total) and `daemon_cost_cap_usd`, both
   `#[serde(default)]`.
   - `daemoneye status` prints a `cap` line in the cost section whenever a
     cap is set, even when today's spend is 0: the spend, the cap and the
     percentage used. With no cap there is no `cap` line, and the section
     shows only when spend is non-zero, as now.
   - Build the line in a pure function so it can be tested.

## Tests

- In `config/types.rs`: `budget_defaults_to_no_cap` (no section ⇒ `0.0`)
  and `budget_max_cost_per_day_parses`.
- In `daemon/spend.rs`:
  - `ledger_seeds_once_per_day_and_adds`: the seed runs once for a day and
    later adds accumulate on top of it.
  - `ledger_reseeds_on_a_new_day`.
  - `local_day_bounds_start_at_local_midnight`.
  - `cap_status_is_off_at_zero_and_over_at_the_cap`: `0` is `Off`, a
    negative cap is `Off`, `spent == cap` is `Over`, and just below the cap
    is `Under`.
- In `cost.rs`: `emit_cost_logs_ai_cost_and_feeds_the_ledger`. Use a temp
  `HOME`, as the guard tests do. One call with `agent_name` `digest`
  leaves one `ai_cost` line naming `digest` with its cost, and the ledger
  grows by that cost.
- In `cli/status.rs`: `cap_line_names_spend_and_cap`. There is no line at
  cap `0`, and at cap `2.0` with spend `0.5` the line names both amounts
  and `25%`.

## Acceptance criteria

- [ ] Each new test ⇒ `test result: ok. 1 passed`.
- [ ] Every model call is costed through the helper: each of the seven
      files in § End-to-end reports `emit_cost` ≥ 1 (`stream.rs` ≥ 2). The five
      side-call files read `0` today.
- [ ] No raw `ai_cost` literal is left outside `cost.rs`:
      `raw_ai_cost_outside=0` (it is `3` today).
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`, which proves
      `[budget] max_cost_per_day` is in the template.
- [ ] `cargo test` ⇒ `failed=0`, with `passed` above 2465 by the new
      tests. Report the number; do not re-run to chase it.
- [ ] Clippy (`-D warnings`) and `cargo fmt --all --check` exit 0. No new
      `#[allow]`, no production `unwrap`/`expect`.
- [ ] The end-to-end Update Log entry holds the output fence **first**,
      then the `PASTE MATCH` line.

## End-to-end verification

```sh
A=/tmp/e2e-m30-05.txt; : > "$A"
for t in budget_defaults_to_no_cap budget_max_cost_per_day_parses ledger_seeds_once_per_day_and_adds ledger_reseeds_on_a_new_day local_day_bounds_start_at_local_midnight cap_status_is_off_at_zero_and_over_at_the_cap emit_cost_logs_ai_cost_and_feeds_the_ledger cap_line_names_spend_and_cap; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
for f in src/daemon/stream.rs src/daemon/ghost.rs src/daemon/briefing.rs src/daemon/digest.rs src/daemon/auto_name.rs src/daemon/scheduled.rs src/webhook/process.rs; do echo "emit_cost $(basename $f)=$(grep -c 'emit_cost(' $f)" >> "$A"; done
echo "raw_ai_cost_outside=$(grep -rlE '^\s*"ai_cost",\s*$' src --include=*.rs | grep -v '^src/cost.rs$' | xargs -r grep -cE '^\s*"ai_cost",\s*$' | awk -F: '{s+=$NF} END {print s+0}')" >> "$A"
echo "template_budget=$(grep -c 'max_cost_per_day' assets/etc/config.toml)" >> "$A"
cargo test --test doc_truth 2>&1 | grep -E '^test result' | sed 's/^/doc_truth: /; s/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Then run the self-check, and put its line in the entry after the output
fence:

```sh
D=docs/dev/milestones/M30-telemetry-dashboard/phase-05-cost-ledger.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m30-05.txt
diff /tmp/pasted-m30-05.txt /tmp/e2e-m30-05.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- Refusing or skipping anything over the cap (phase 06).
- Changing `compute_cost_today` or the UTC breakdowns `status` already
  shows (by provider, agent and session).
- Per-run or per-agent caps.
- `daemoneye top`'s budget panel (phase 07).

## Update Log

<!-- entries appended below this line -->

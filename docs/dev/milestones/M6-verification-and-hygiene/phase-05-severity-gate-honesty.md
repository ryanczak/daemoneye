# Phase 05: Severity-Gate Honesty

**Milestone:** M6 — Verification & Hygiene
**Status:** todo
**Depends on:** phase-01 (done)
**Estimated diff:** ~250 lines
**Tags:** language=rust, kind=fix, size=m

## Goal

Stop the webhook path discarding alerts in silence. Two changes:

1. **An absent severity is not the lowest severity.** Today an alert with no
   severity label ranks `0` and is dropped by the default threshold. It must
   reach the agent instead.
2. **Every discard leaves a trace.** Each gate that drops an alert emits a
   structured event naming the alert and the reason, so "was my alert
   processed?" is answered by reading `events.jsonl` rather than by reading
   source.

This is the fix phase for defects 1 and 3. Phase 06 then proves the whole
pipeline end-to-end; doing 06 first would mean writing a scenario whose expected
outcome is a known bug.

## Architecture references

Read before starting:

- `docs/dev/milestones/M6-verification-and-hygiene/README.md` § "Defect
  inventory" items 1, 2, 3 — what this costs and why the existing test cannot
  catch it.
- `docs/dev/WORKFLOW.md` § "Confirm the property is observable before pinning
  it" — defect 3 is precisely the failure it guards.
- `CLAUDE.md` § "Important Invariants" — the `events.jsonl` record contract.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom. **§1 gained a new
   Definition-of-Done box** on mechanically-captured end-to-end transcripts —
   read it, it governs this phase's Update Log.
2. Read `src/webhook/process.rs` and `src/webhook/server.rs` in full.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is clean and `cargo test` is green at 964 lib tests.

## Current state

**The gate, verified in the tree** (`src/webhook/process.rs:136-145`):

```rust
let threshold_rank = severity_rank(&cfg.severity_threshold);
let alert_rank = severity_rank(&alert.severity);

if alert_rank >= threshold_rank || threshold_rank == 0 {
    fire_notification(&alert.alert_name, &formatted, &state.config);

    if cfg.auto_analyze {
        maybe_analyze_alert(&alert, &formatted, &state).await;
    }
}
```

**There is no `else`.** `severity_rank` (`:14`) maps anything unrecognised —
including `""` — to `0`. An absent label becomes `""` via
`labels.get("severity").cloned().unwrap_or_default()` (`parse.rs:93`, `:156`).
The shipped default threshold is `"warning"` → rank 2
(`config/types.rs:490-492`). So `0 >= 2` is false, `2 == 0` is false, and both
`fire_notification` and `maybe_analyze_alert` are skipped with no log line and no
event.

**Everything *before* the gate still runs** — `log_event("webhook_alert", …)`
(`:98`), the `log::info!` (`:110`), session injection, the tmux notify. That is
why the operator sees `Webhook alert: '…' [firing]` in `daemon.log` and
reasonably concludes the alert was processed.

### Every discard point on the webhook path

Verified by reading `server.rs` and `process.rs` end to end:

| # | Site | Discards when | Logs today | Emits event today |
|---|---|---|---|---|
| 1 | `server.rs:57-59` | bad/missing bearer token | `log::warn!` ✓ | ✗ |
| 2 | `server.rs:63-65` | payload parses to zero alerts | `log::warn!` ✓ | ✗ |
| 3 | `process.rs:50-56` | duplicate inside the dedup window | `log::debug!` | ✗ |
| 4 | `process.rs:139` | **below severity threshold** | **nothing** | ✗ |

Row 4 is the motivating defect. Rows 1–3 are in scope for the event, because the
milestone's exit criterion is *no alert is dropped silently*, not *the severity
gate is fixed*.

**`severity_rank` has no callers outside `process.rs`** — the gate at `:136-137`
and two unit tests at `:501-512`. Changing its signature is contained.

**One existing test asserts the bug** (`process.rs:501-506`):

```rust
assert!(severity_rank("info") > severity_rank("unknown"));
```

That is the belief this phase removes — unrankable is not "below info". Rewriting
this test is required, expected, and **not** scope creep.

## Spec

### 1. Make "unrankable" a distinct outcome

Change `severity_rank` to return `Option<u8>` (or an equivalent that makes the
distinction total — a small enum is fine):

- `Some(3)` critical, `Some(2)` warning/warn, `Some(1)` info/informational.
- **`None`** for anything else, including `""`, whitespace-only, and an
  unrecognised word like `"banana"`.

Keep the existing case-insensitivity.

### 2. Fail open on unrankable, and say so

Rewrite the gate so:

- **Alert severity is `None` → the alert passes.** It is not ranked, so it
  cannot be *known* to be below the threshold, and dropping it is the dangerous
  direction. This is the defect-1 fix and phase 06 depends on it.
- **Alert severity is `Some(n)` → pass iff `n >= threshold`.** A genuinely
  below-threshold alert is still filtered; that is the feature working.
- **Threshold itself is `None`** (misconfigured, e.g. `severity_threshold =
  "banana"`) → everything passes. This preserves the intent of today's
  `threshold_rank == 0` escape hatch.
- **When the gate discards**, emit the event from task 3 and `log::warn!` naming
  the alert and the reason.

### 3. One structured discard event, four call sites

Emit via `crate::daemon::utils::log_event` with event name **`webhook_discarded`**
and at minimum these fields:

- `reason` — one of `below_threshold`, `duplicate`, `unauthorized`,
  `unparseable`.
- `alert_name` — the alert's name, or a clear placeholder where none exists
  (rows 1–2 have no parsed alert).
- Enough context to act on: the severity and the configured threshold for
  `below_threshold`; the truncated fingerprint for `duplicate`.

**Invariant — do not pass `pid`.** Per `CLAUDE.md`, `log_event` stamps `ts`,
`event`, and `pid` itself; call sites must not supply one. Key order in the
serialized line is `serde_json`'s (alphabetical), so never assert on field
position.

Apply at all four sites in the table above.

**Log levels — a deliberate narrowing of the exit criterion, flagged for the
human.** The milestone criterion says every discarding gate "logs at WARN." That
is right for rows 1, 2 and 4. For row 3 (dedup) it is not: suppressing a
duplicate is *intended* behaviour, and WARN-per-duplicate during a flapping-alert
storm would flood a log that is itself unbounded until phase 08. So: **all four
emit the event; rows 1, 2 and 4 log at WARN; row 3 keeps `log::debug!`.** The
event is the durable trace, which is what the criterion is actually protecting.
Recorded here for confirmation at milestone close — do not treat it as licence to
narrow anything else.

## Acceptance criteria

- [ ] An alert whose payload carries **no severity label** passes the gate under
      the shipped default threshold (`warning`).
- [ ] An alert with severity `"banana"` also passes (unrankable ≠ lowest).
- [ ] An alert with severity `"info"` under threshold `"warning"` is **still
      discarded** — the gate is not simply disabled.
- [ ] That discard emits a `webhook_discarded` event with `reason:
      "below_threshold"`, the alert name, its severity, and the threshold.
- [ ] All four discard sites emit `webhook_discarded` with the right `reason`.
- [ ] No `webhook_discarded` record carries a caller-supplied `pid`.
- [ ] `severity_rank_ordering` no longer asserts that a known severity outranks
      an unrankable one.
- [ ] The existing `webhook_alert_to_event_log` integration test still passes
      unchanged.
- [ ] All four gates green.

## Test plan

**A worked example exists — follow it.** `tests/integration.rs:679-746`
(`webhook_alert_to_event_log`) already drives the real path: throwaway `HOME` via
`daemoneye::test_home_guard()`, `Config::ensure_dirs()`, a hand-built
`WebhookState`, a current-thread tokio runtime, `rt.block_on(process_alert(...))`,
then it reads `config::current_event_segment_path()` and parses the JSONL. Copy
that shape rather than inventing one.

**Tests that touch `HOME` must take `crate::test_home_guard()`**
(`src/lib.rs:45`) — not the raw `TEST_HOME_LOCK` (`:32`), which poisons every
later HOME-dependent test in the binary. Edition 2024, so `std::env::set_var`
needs `unsafe`. Hold the guard through **all** HOME-dependent work and drop it at
the end — a phase-04 bug was filed for dropping it early.

Cover:

- Unit: `severity_rank` returns `None` for `""`, whitespace, and `"banana"`;
  `Some` with the right ordering for the three known levels.
- **The defect-1 regression test:** an alert parsed from a payload with no
  `severity` label reaches the gate and is *not* discarded under the default
  threshold. Assert on the absence of a `webhook_discarded` record, not merely
  that nothing panicked.
- The `"info"`-under-`"warning"` case *is* discarded and the
  `webhook_discarded` record has the expected fields.
- Rewrite `severity_rank_ordering` so it pins the new contract.

**Do not assert on `lines.last()` alone** where a passing alert may emit further
records downstream — search the segment for the record you mean. Defect 3 is a
test that asserted on a line written *before* the branch it claimed to cover;
do not reproduce that shape.

**Do not pin a test count in advance.** Report the resulting count in the Update
Log and explain the delta.

## End-to-end verification

`events.jsonl` is a real artifact this phase changes, so §1's mechanical-capture
box applies. **Redirect each command's output to a file and paste that file's
contents.** Do not retype, summarise, or reconstruct.

Drive a below-threshold alert through the real path in a throwaway `HOME`, then
show the resulting event segment:

```sh
export H=$(mktemp -d)
HOME=$H cargo test --test integration <your_new_below_threshold_test> -- --nocapture \
  > /tmp/e2e-gate.txt 2>&1; echo "exit=$?" >> /tmp/e2e-gate.txt
```

Paste `/tmp/e2e-gate.txt`, and paste the matching `webhook_discarded` line from
the event segment the test wrote. If a command's proof is that something is
*absent* (the no-severity case), make the absence observable — print the grep's
exit status (`echo "grep-exit=$?"`), because a grep that finds nothing prints
nothing and an empty block proves nothing on its own.

## Authorizations

- [ ] May modify `src/webhook/process.rs` and `src/webhook/server.rs`.
- [ ] May rewrite the two `severity_rank` unit tests in `process.rs`.
- [ ] May add tests to `tests/integration.rs`.

No new dependencies. No changes to `docs/architecture.md`.

## Out of scope

- **Do not build the end-to-end webhook→ghost scenario.** That is phase 06 and it
  uses the phase-01 harness. This phase stops at the gate and the event.
- **Do not change `severity_threshold`'s default** or any other shipped config
  value. The fix is that an *absent* severity is handled correctly, not that the
  threshold is relaxed.
- **Do not touch `maybe_analyze_alert`**, the watchdog prompt, `parse_ghost_trigger`,
  or `check_ghost_capacity` — everything behind the gate is phase 06's ground.
- **Do not change the `webhook_alert` event** already emitted at `process.rs:98`,
  or reorder it relative to the gate. An existing integration test asserts on it.
- **Do not add rate limiting or change dedup behaviour** — only its observability.
- **Do not touch `src/pane_prefs.rs`, `main.rs`'s stale `daemon.log` help
  strings, or `.gitignore`.** Phase 11 and milestone housekeeping.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

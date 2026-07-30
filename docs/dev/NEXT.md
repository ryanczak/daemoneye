# NEXT

**Active phase: 05 — severity-gate-honesty.**
Doc: `docs/dev/milestones/M6-verification-and-hygiene/phase-05-severity-gate-honesty.md`
Status: `todo` — drafted 2026-07-30, not yet dispatched.

Dispatch with `/rexymcp:dispatch phase-05`.

## What phase 05 does

Stops the webhook path discarding alerts in silence. An alert with no severity
label currently ranks `0`, falls under the shipped `warning` threshold, and is
dropped with **no log line and no event** — while everything before the gate
still runs, so `daemon.log` shows `Webhook alert: '…' [firing]` and the operator
concludes it was processed. Phase 05 makes unrankable a distinct outcome that
fails open, and makes every discard emit a `webhook_discarded` event.

## What drafting established

**All four discard points were located, not assumed.** Bearer-token rejection
(`server.rs:57`) and unparseable payloads (`:63`) already log at WARN but emit no
event; dedup (`process.rs:50`) logs only at DEBUG; the severity gate
(`process.rs:139`) has **no `else` at all**. The exit criterion is "no alert is
dropped silently", so all four are in scope for the event.

**One deliberate narrowing, flagged for confirmation at milestone close.** The
criterion says every discarding gate logs at WARN. That is right for three of
them, but dedup suppression is *intended* behaviour and WARN-per-duplicate during
a flapping-alert storm would flood a log that stays unbounded until phase 08. So
all four emit the event; three log at WARN; dedup keeps `log::debug!`. The event
is the durable trace the criterion is actually protecting. Recorded in the phase
doc — worth a yes/no at close.

**An existing unit test asserts the bug.** `severity_rank_ordering`
(`process.rs:501`) asserts `severity_rank("info") > severity_rank("unknown")` —
exactly the "unrankable is the lowest severity" belief being removed. Rewriting
it is required and explicitly not scope creep.

**The test pattern already exists.** `tests/integration.rs:679-746` drives the
real path — throwaway `HOME`, hand-built `WebhookState`, current-thread runtime,
`block_on(process_alert(...))`, then reads the event segment. The phase points at
it rather than leaving the executor to invent one.

## Where things stand

- Phases 01–04 `done`. 04 closed `approved_after_2` after two bounces and four
  bugs — all now `verified`.
- **The E2E-transcript fold is working.** `STANDARDS.md` §1 gained a
  mechanical-capture box and `WORKFLOW.md` § "Review and Bug-Report Cycle" gained
  step 4 (re-run and diff). bug-04-4 was raised *after* that fold and caught *by*
  it: the review observed that a real `diff` prints nothing on identical input,
  so `"(empty - no changes)"` could only have been hand-typed. Worth recording in
  the retrospective as a validated fold rather than a pending one.
- `cargo clippy --all-targets --all-features -- -D warnings` clean; 964 lib + 27
  integration (2 ignored, pre-existing) + 3 isolation, zero failures.
- Working tree clean. No daemon running; no tmux server running.
- Milestone README:
  `docs/dev/milestones/M6-verification-and-hygiene/README.md`. Phases 06–12
  named, not drafted. Re-verify each phase's "Current state" against the tree
  before dispatching.

## Carried forward for milestone close

- **`.gitignore` has no `.daemoneye/` entry.** A full seeded 168K runtime tree
  was found untracked in the repo root during phase 04 and had to be moved out
  before it was committed. Two reviews recommended the entry; both correctly
  declined to make it, as it sits outside any phase's Authorizations. It is
  milestone housekeeping.
- **`src/main.rs:17` and `:30`** still document the daemon log as
  `~/.daemoneye/daemon.log`; the real path is `var/log/daemon.log`. Same drift
  class as the prompt defect, in CLI help text the asset gate does not cover.
  Noted for phase 11.
- **The dedup log-level narrowing** described above.

# NEXT

**Active phase: 03 — fix-stale-prompt-paths.**
Doc: `docs/dev/milestones/M6-verification-and-hygiene/phase-03-fix-stale-prompt-paths.md`
Status: `todo` — drafted 2026-07-30, not yet dispatched.

Dispatch with `/rexymcp:dispatch phase-03`.

## What phase 03 does

Empties `PENDING_FIX`. Corrects every stale path literal in the shipped prompt
and knowledge memories so the phase-02 audit passes with no quarantine, and
fixes the two stale spans the audit is structurally blind to.

Phase 02 built the gate and proved it fires. This phase makes the assets clean
and takes the scaffolding down.

## Two things found while drafting that shape the phase

**1. The gate cannot see two of the defects it is meant to cover.**
`extract_path_literals` is backtick-delimited by design — a "contains a slash"
rule produces false failures on `/clear`, `/limits reset` and shebangs. So the
**ASCII directory tree** in `agent-runtime-layout.md` (which still shows `log/`
holding a flat `events.jsonl`) and the `grep` command at `webhook-setup.md:24`
are both wrong and both invisible to the audit — they live inside code fences.
They are fixed by hand here; widening the extractor is explicitly out of scope.

**2. Emptying `PENDING_FIX` makes phase 02's best test vacuous.**
`red_run_is_reproducible` asserts the unquarantined audit flags exactly the
literals in `PENDING_FIX`. Empty that list and the assertion becomes
`assert_eq!(empty, empty)` — passing regardless of what the extractor does. The
fix would *introduce* the exact vacuous coverage this milestone exists to
eliminate. Task 4 splits it into two properties: the assets are clean, and the
extractor still flags all 7 historical literals from a frozen synthetic corpus
(test data, so it stays red-proof once the assets are fixed).

## What to look at before dispatching

- **Every replacement path in the spec's table was verified against its
  constructor**, not copied from the milestone README. One correction came out
  of that: the ghost session log's *filename* `ghost-<name>-<uuid>.jsonl` is
  **right** (`ghost.rs:185` + `session.rs:180`); only its directory is wrong.
  The README's summary table would have led to specifying `<id>.jsonl`.
- The phase is small (~150 lines) and mechanical apart from task 4, which is the
  one place judgment is needed and is spelled out.

## Where things stand

- Phases 01 and 02 `done` (01 approved_after_1, 02 approved_after_1). Phase 02's
  bounce was a green bounce — four green gates, clean tree, bounced on an
  untested `Legacy` branch proven dead by mutation, plus an unauthorized
  `#![allow(dead_code)]`. Both fixed on one refined re-dispatch.
- `src/config/path_audit.rs` ships the extractor, a 24-entry `INVENTORY` with
  `Current`/`Legacy` status, `audit_text_with(text, pending)` + `audit_text`, and
  `PENDING_FIX` holding exactly 7 quarantined literals.
- `cargo clippy --all-targets --all-features -- -D warnings` clean; 956 lib + 27
  integration (2 ignored, pre-existing) + 3 isolation, zero failures.
- Working tree clean. No daemon running; no tmux server running.
- Milestone README:
  `docs/dev/milestones/M6-verification-and-hygiene/README.md`. Phases 04–12
  named, not drafted. Re-verify each phase's "Current state" against the tree
  before dispatching.
- Standing backlog: `docs/dev/TODO.md` § 1, the pre-dispatch criteria check.
  Worth deciding at the milestone close.

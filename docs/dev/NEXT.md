# NEXT

**Phase 04 — audit-prompts-command — done** (approved_after_2, 2026-07-30).
Doc: `docs/dev/milestones/M6-verification-and-hygiene/phase-04-audit-prompts-command.md`

Phases 01–04 of M6 are now `done`. Phase 05 (severity-gate-honesty) is named
but not yet drafted — draft it with `/rexymcp:architect next` when ready.

## What phase 04 does

Ships the operator-facing `daemoneye audit-prompts`: reads the **installed**
prompt and knowledge memories from `~/.daemoneye/`, classifies every path literal
as current / superseded / unknown against the phase-02 inventory, prints a
report, and exits non-zero if anything is not current. It never writes.

## Two things from drafting that shape the phase

**1. It must audit the installed copies, not the embedded assets.** That is the
whole point of defect 6 — `overwrite_sre_prompt()` / `overwrite_knowledge_memories()`
are only called from `setup.rs`, and first-run seeding is `if !exists`, so an
install predating a change keeps the stale copy forever. Auditing the
`include_str!` consts would always pass and tell the operator nothing about their
own tree. Phase 03 just made the shipped assets clean; the operator's installed
copies are still whatever they were.

**2. `audit_text` alone is not enough.** It returns only the *bad* literals, and
the exit criterion wants every path reported with its status so the operator can
see what was checked. Task 1 adds `classify_text` to the same module — reusing
the extractor, not growing a second one.

## What to look at before dispatching

- **The end-to-end quoting requirement is the risk.** It bounced phase 03 twice:
  once for paraphrasing instead of quoting, once for a 25-line transcript in
  which 24 lines were real and one was spliced in from another file. Phase 03 was
  finished by architect takeover for that reason. Phase 04's End-to-end section
  now says to redirect each command to a file and paste that file.
- The no-write contract is testable and is pinned as an acceptance criterion
  (tree listing + mtimes before and after), not left as prose.

## Where things stand

- Phases 01–03 `done`. 01 approved_after_1; 02 approved_after_1; **03
  escalated** — completed by architect takeover after two bounces of the same
  class (`false_completion`).
- **Open calibration item for the milestone close:** two occurrences of the
  executor emitting plausible-but-unreal evidence (bug-03-1, bug-03-2). Per
  `WORKFLOW.md` § "Calibration", two occurrences is a trend worth folding.
  Recommended fold: the review must diff any pasted transcript against a live
  re-run rather than reading it for plausibility — round 2 caught the splice only
  because it re-ran the command, and a 24-of-25-real transcript is
  indistinguishable from a real one by inspection. **Not folded** — contract-doc
  changes are a human gate.
- `src/config/path_audit.rs` ships the extractor, a 24-entry `INVENTORY`,
  `audit_text_with` / `audit_text`, and `PENDING_FIX` now empty. The shipped
  assets audit clean.
- `cargo clippy --all-targets --all-features -- -D warnings` clean; 955 lib + 27
  integration (2 ignored, pre-existing) + 3 isolation, zero failures.
- Working tree clean. No daemon running; no tmux server running.
- Milestone README:
  `docs/dev/milestones/M6-verification-and-hygiene/README.md`. Phases 05–12
  named, not drafted. Re-verify each phase's "Current state" against the tree
  before dispatching.
- Noted for phase 11: `src/main.rs:17` and `:30` still say the daemon log
  defaults to `~/.daemoneye/daemon.log`; the real path is `var/log/daemon.log`.
  Same drift class as the prompt, in CLI help text the asset gate does not cover.

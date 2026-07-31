# NEXT

**Active phase: none — M6 is closed. Milestone boundary: awaiting PE sign-off.**

M6 — Verification & Hygiene closed 2026-07-31. All thirteen phase docs `done`
(phase 06 split into 06a/06b). The retrospective is in the milestone README's
Notes section:
`docs/dev/milestones/M6-verification-and-hygiene/README.md` § "M6 retrospective".

**Nothing is drafted for M7, and nothing should be until you scope it.**

## What needs your decision

**Five calibration items, none applied.** The retrospective carries the full
evidence; in short:

1. ~~Fold: the E2E block must be a distinct executor-authored Update Log entry~~
   — **APPLIED 2026-07-31 on PE sign-off.** Landed in `STANDARDS.md` §1 (new DoD
   box) and `WORKFLOW.md` § "End-to-end verification", with the attribution note.
2. ~~Fold: phase specs supply E2E commands as runnable blocks, never prose~~ —
   **APPLIED 2026-07-31**, folded into the same `WORKFLOW.md` section as an
   architect-facing paragraph, since the two share one evidence base.
3. ~~A cleanup phase for the ~25 test files that set `HOME` without restoring~~
   — **DONE 2026-07-31.** Fixed at the root instead of across 27 files:
   `test_home_guard()` now returns an RAII `TestHomeGuard` that snapshots `HOME`
   on acquisition and restores it on drop, before releasing the mutex. Every one
   of the ~109 `set_var("HOME", …)` sites is covered without editing any of
   them, because all 27 files already took the guard. 20 consecutive
   `cargo test --lib` runs clean.
4. **`CLAUDE.md` over-describes `src/memory/index.rs`** as a full FTS5 index; it
   is an eight-line stub. Developer-facing, so no gate covers it.
5. **Whether the path-audit extractor should learn about fenced blocks.** Three
   stale paths slipped through on that account across M6.

**Runtime tree — done.** The two dead entries (`~/.daemoneye/lib/` and the
orphaned top-level `~/.daemoneye/pane_prefs.json`) were removed by the operator
on 2026-07-31. `~/.daemoneye/` now contains exactly what the code produces:
`agents bin etc memory runbooks scripts var`.

Note the live `var/run/pane_prefs.json` (64 bytes, 25 July) is still in the
**old** `{session: "pane_id"}` format. Phase 10's loader treats an entry that
does not parse as the new fingerprinted shape as absent and discards it — so the
first foreground command after upgrade prompts once for a pane and writes a
fingerprinted entry. That is by design, not a leftover.

## Where the tree stands

- 990 lib + 30 integration (2 ignored) + 8 isolation (1 ignored); clippy clean;
  `cargo test --lib` stable across sixteen consecutive runs.
- Working tree clean. No daemon running; no tmux server running.
- `docs/architecture.md` § 5 now lists M6 under **Shipped**.

## To start M7

Run `/rexymcp:architect` (no args) to scope it. The milestone boundary is a hard
human gate — the loop and the architect both stop here by design, and neither
will draft an M7 phase without you scoping the milestone first.

# NEXT

**Active phase:** **M23 phase-07b — agents-picker**, drafted 2026-10-03,
status `todo`. Dispatch with `/rexymcp:dispatch phase-07b`.

**What it does.** `/agents`, or a bare `/attach`, opens a picker inside the
chat viewport: every live shell, ghost-owned first, one row each as
`/shells` shows it. `↑`/`↓` select, `Enter` attaches through 07a, and `Esc`
closes. A new `src/cli/commands/agents.rs` (given whole) plus small diffs to
`render_ratatui.rs`, `chat.rs`, `commands/mod.rs` and `slash.rs`; 7 new
tests; lib 2186 → 2193. This closes row 07.

**Measured at drafting.** The 6-row inline viewport fits 3 list rows, so
the picker scrolls a window. The `/agents` routing is a pure function so
its negatives are tested. The end-to-end block was run on the dry-run tree.
Mutants A–G went red 3/3, all clippy-clean. The doc reproduces the dry run.
**New this phase (07a calibration):** recording the mutations is its own
numbered Spec task, placed before the capture, with a counted criterion.

**After 07b:** `/rexymcp:architect next` drafts row 08 (attach-chords).

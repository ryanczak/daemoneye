# NEXT

**Active milestone:** M1 — Agent Tooling Improvements
(`docs/dev/milestones/M1-agent-tooling/README.md`)

**Active phase:** phase-10-tmux-surface-and-safe-verbs
(`docs/dev/milestones/M1-agent-tooling/phase-10-tmux-surface-and-safe-verbs.md`) —
`todo`, drafted 2026-06-22 (already complete; selected as active 2026-06-23).
Centralizes the three inline `tmux` buffer subprocess calls in
`executor/file_ops.rs` into typed `src/tmux/` wrappers, and replaces the one
daemon-host-local `__DE_DONE__` capture-poll in `local_read_via_buffer` with a
native `tmux wait-for` signal that degrades to the buffer read on a missed/raced
signal. `Depends on: none`; deliberately does not touch the 07a/07b-hardened
foreground completion path.

Dispatch with `/rexymcp:dispatch phase-10-tmux-surface-and-safe-verbs`.

phase-09 (error-suppress-audit) is **done** (approved_after_1 2026-06-23;
bug-phase-09-1 fixed). phase-08 (prompt-and-tooldef-fixes) is **done**
(approved_first_try 2026-06-22).

Remaining after phase-10 (drafted, `todo`):
- phase-11 (on-demand-tool-loading) — split `TOOLS` into core + deferred via a
  self-declaring `ToolDef.deferred_group`; default render emits core only; a new
  `load_tools` tool pulls a deferred group into the session on demand so deferred
  schemas stop shipping every request. `Depends on: none` (sequence 08 → 11 for
  clean diffs; 08 is now done).

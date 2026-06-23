# NEXT

**Active milestone:** M1 — Agent Tooling Improvements
(`docs/dev/milestones/M1-agent-tooling/README.md`)

**Active phase:** none — select the next via `/rexymcp:architect next`.

phase-09 (error-suppress-audit) is **done** (approved_after_1 2026-06-23;
bug-phase-09-1 fixed). phase-08 (prompt-and-tooldef-fixes) is **done**
(approved_first_try 2026-06-22).

Remaining (drafted, `todo`, dispatchable in any order — all independent):
- phase-10 (tmux-surface-and-safe-verbs, renumbered from 07c) — stand-alone
  tmux-integration phase: centralize inline `tmux` buffer calls into `src/tmux/` +
  adopt `tmux wait-for` at the `read_file` local-buffer-read site.
  `Depends on: none`; leaves the hardened foreground completion path untouched.
- phase-11 (on-demand-tool-loading) — split `TOOLS` into core + deferred via a
  self-declaring `ToolDef.deferred_group`; default render emits core only; a new
  `load_tools` tool pulls a deferred group into the session on demand so deferred
  schemas stop shipping every request. `Depends on: none` (sequence 08 → 11 for
  clean diffs; 08 is now done).

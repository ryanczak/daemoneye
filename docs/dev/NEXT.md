# NEXT

**Active milestone:** M1 — Agent Tooling Improvements
(`docs/dev/milestones/M1-agent-tooling/README.md`)

**Active phase:** phase-09-error-suppress-audit
(`docs/dev/milestones/M1-agent-tooling/phase-09-error-suppress-audit.md`) —
`todo`, drafted 2026-06-22. Audits and removes error-suppressing idioms
(`unwrap`/`expect`/`panic!`/`unsafe`/`#[allow]`) from production paths per
STANDARDS §1/§2. `Depends on: none`.

Dispatch with `/rexymcp:dispatch phase-09-error-suppress-audit`.

phase-08 (prompt-and-tooldef-fixes) is **done** (approved_first_try 2026-06-22).

Remaining (drafted, `todo`, dispatchable in any order — all independent):
- phase-09 (error-suppress-audit) — the active phase above.
- phase-10 (tmux-surface-and-safe-verbs, renumbered from 07c) — stand-alone
  tmux-integration phase: centralize inline `tmux` buffer calls into `src/tmux/` +
  adopt `tmux wait-for` at the `read_file` local-buffer-read site.
  `Depends on: none`; leaves the hardened foreground completion path untouched.
- phase-11 (on-demand-tool-loading) — split `TOOLS` into core + deferred via a
  self-declaring `ToolDef.deferred_group`; default render emits core only; a new
  `load_tools` tool pulls a deferred group into the session on demand so deferred
  schemas stop shipping every request. `Depends on: none` (sequence 08 → 11 for
  clean diffs; 08 is now done).

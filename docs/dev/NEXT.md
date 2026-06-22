# NEXT

**Active milestone:** M1 — Agent Tooling Improvements
(`docs/dev/milestones/M1-agent-tooling/README.md`)

**Active phase:** phase-08-prompt-and-tooldef-fixes
(`docs/dev/milestones/M1-agent-tooling/phase-08-prompt-and-tooldef-fixes.md`) —
`todo`, drafted 2026-06-22, **re-scoped same day**. Now schema-correctness only:
JSON-schema `enum` constraints for `edit_file.operation` / `search_repository.kind`
/ memory `category`, fixes the `create_agent.auto_approve_scripts` schema/parser
mismatch, and teaches the prompt the § 2.4 local-vs-remote tool-class model. The
tool-discoverability work it originally carried moved to phase-11 (see below).

Dispatch with `/rexymcp:dispatch phase-08-prompt-and-tooldef-fixes`.

Remaining (drafted, `todo`, dispatchable in any order — all independent of 08):
- phase-09 (error-suppress-audit).
- phase-10 (tmux-surface-and-safe-verbs, renumbered from 07c) — stand-alone
  tmux-integration phase: centralize inline `tmux` buffer calls into `src/tmux/` +
  adopt `tmux wait-for` at the `read_file` local-buffer-read site.
  `Depends on: none`; leaves the hardened foreground completion path untouched.
- phase-11 (on-demand-tool-loading) — split `TOOLS` into core + deferred via a
  self-declaring `ToolDef.deferred_group`; default render emits core only; a new
  `load_tools` tool pulls a deferred group into the session on demand so deferred
  schemas stop shipping every request. `Depends on: none` (no conflict with 08;
  sequence 08 → 11 for clean diffs).

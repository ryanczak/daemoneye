# NEXT

**Active milestone: none.** M27 (tmux becomes an optional observer;
DaemonEye 2.0 close) is closed: `v2.0.0` was tagged and pushed 2026-10-12.
See the retrospective in [`README.md`](milestones/M27-tmux-observer/README.md).
PE signed off on the close 2026-10-12, and fold A landed in `WORKFLOW.md`.
The next milestone is scoped with `/rexymcp:architect`, and it closes
as release `2.1.0` (`REXYMCP.md` § Versioning).

The next milestone is **M28 — Interactive shells**, designed in
`docs/design/interactive-shells.md` (2026-10-13, PE decisions applied): a
TUI such as Claude Code runs inside a PTY shell and the model can see it,
type into it safely, wait on it and never poison it. Three phases; not yet
scoped. The roadmap's former M28–M30 are now M29–M31
(`docs/design/daemoneye-2.0.md` § 6).

Still queued after it: the M27 deferred list (unify session residency, F9,
the invalid-args nits) and `docs/design/daemoneye-post-2.0-todo.md`. Fold
queue: M27 proposal B (held) and M23's two proposals.

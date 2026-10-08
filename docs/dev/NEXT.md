# NEXT

**Active milestone: M26 — Agent isolation on Docker Sandboxes (sbx)**,
[`milestones/M26-sbx-isolation/README.md`](milestones/M26-sbx-isolation/README.md).
Rescoped 2026-10-05 from "tmux removal & 2.0 close" (now **M27**; later
milestones are M28–M30, `docs/design/daemoneye-2.0.md` § 6). **PE signed off
on scope 2026-10-05**, accepting all six recommendations.

**Active phase: 09c — sbx-docs**,
[`phase-09c-sbx-docs.md`](milestones/M26-sbx-isolation/phase-09c-sbx-docs.md),
status `todo`, for the executor: README, CLAUDE.md, `security.md`,
`architecture.md`, the seeded `ghost-shell-guide` memory and the seeded
`config.toml` `[sandbox]` block describe sbx. The architect pinned the
replacement text, and there are no Rust changes. Phases 01–09b are done;
row 10 (live close) follows.

Carried from M23 into M26: the sandboxed ghost in a PTY shell (now sbx, with
in-VM `Ctrl-p`); the `ghost-shell-guide` memory rewrite; pause-vs-deadline
(decided: a pause suspends the deadline). M23's two fold proposals remain on
the calibration queue.

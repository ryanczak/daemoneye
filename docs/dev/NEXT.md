# NEXT

**Active milestone: M26 — Agent isolation on Docker Sandboxes (sbx)**,
[`milestones/M26-sbx-isolation/README.md`](milestones/M26-sbx-isolation/README.md).
Rescoped 2026-10-05 from "tmux removal & 2.0 close" (now **M27**; later
milestones are M28–M30, `docs/design/daemoneye-2.0.md` § 6). **PE signed off
on scope 2026-10-05**, accepting all six recommendations.

**Active phase: none drafted.** Phases 01–09b are **done** (09b,
docker-delete, approved_first_try): the docker runtime is gone from the code.
Next is row **09c — sbx-docs** (README, CLAUDE.md § Container sandbox,
`security.md`, the seeded `ghost-shell-guide` memory, `docs/architecture.md`);
`/rexymcp:architect next` drafts it. After it, row 10 is the live close.

Carried from M23 into M26: the sandboxed ghost in a PTY shell (now sbx, with
in-VM `Ctrl-p`); the `ghost-shell-guide` memory rewrite; pause-vs-deadline
(decided: a pause suspends the deadline). M23's two fold proposals remain on
the calibration queue.

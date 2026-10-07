# NEXT

**Active milestone: M26 — Agent isolation on Docker Sandboxes (sbx)**,
[`milestones/M26-sbx-isolation/README.md`](milestones/M26-sbx-isolation/README.md).
Rescoped 2026-10-05 from "tmux removal & 2.0 close" (now **M27**; later
milestones are M28–M30, `docs/design/daemoneye-2.0.md` § 6). **PE signed off
on scope 2026-10-05**, accepting all six recommendations.

**Active phase: 07 — sbx-chat-background**,
[`phase-07-sbx-chat-background.md`](milestones/M26-sbx-isolation/phase-07-sbx-chat-background.md),
status `todo`, for the executor: chat `wait: false` in a per-session sbx
sandbox, with the Q5 lifecycle refined (close after completion, reap after
600 s idle). Phases 01–06d are done.

Carried from M23 into M26: the sandboxed ghost in a PTY shell (now sbx, with
in-VM `Ctrl-p`); the `ghost-shell-guide` memory rewrite; pause-vs-deadline
(decided: a pause suspends the deadline). M23's two fold proposals remain on
the calibration queue.

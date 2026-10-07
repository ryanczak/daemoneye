# NEXT

**Active milestone: M26 — Agent isolation on Docker Sandboxes (sbx)**,
[`milestones/M26-sbx-isolation/README.md`](milestones/M26-sbx-isolation/README.md).
Rescoped 2026-10-05 from "tmux removal & 2.0 close" (now **M27**; later
milestones are M28–M30, `docs/design/daemoneye-2.0.md` § 6). **PE signed off
on scope 2026-10-05**, accepting all six recommendations.

**Active phase: 05a — sbx-signal-plan**,
[`phase-05a-sbx-signal-plan.md`](milestones/M26-sbx-isolation/phase-05a-sbx-signal-plan.md),
status `todo`, for the executor: the pure in-VM signal pieces (tagged shells,
the measured script, argv and parsers). Row 05 was split into 05a/05b/05c.

Carried from M23 into M26: the sandboxed ghost in a PTY shell (now sbx, with
in-VM `Ctrl-p`); the `ghost-shell-guide` memory rewrite; pause-vs-deadline
(decided: a pause suspends the deadline). M23's two fold proposals remain on
the calibration queue.

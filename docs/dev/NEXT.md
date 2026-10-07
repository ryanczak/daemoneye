# NEXT

**Active milestone: M26 — Agent isolation on Docker Sandboxes (sbx)**,
[`milestones/M26-sbx-isolation/README.md`](milestones/M26-sbx-isolation/README.md).
Rescoped 2026-10-05 from "tmux removal & 2.0 close" (now **M27**; later
milestones are M28–M30, `docs/design/daemoneye-2.0.md` § 6). **PE signed off
on scope 2026-10-05**, accepting all six recommendations.

**Active phase: 08a — sbx-status**,
[`phase-08a-sbx-status.md`](milestones/M26-sbx-isolation/phase-08a-sbx-status.md),
status `todo`, for the executor: the SANDBOX status section and a new
`daemoneye sandbox status` read sbx when `runtime = "sbx"`. Row 08 is split
into 08a (status) and 08b (egress audit). Phases 01–07 are done.

Carried from M23 into M26: the sandboxed ghost in a PTY shell (now sbx, with
in-VM `Ctrl-p`); the `ghost-shell-guide` memory rewrite; pause-vs-deadline
(decided: a pause suspends the deadline). M23's two fold proposals remain on
the calibration queue.

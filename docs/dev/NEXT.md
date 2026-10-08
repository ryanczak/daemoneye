# NEXT

**Active milestone: M26 — Agent isolation on Docker Sandboxes (sbx)**,
[`milestones/M26-sbx-isolation/README.md`](milestones/M26-sbx-isolation/README.md).
Rescoped 2026-10-05 from "tmux removal & 2.0 close" (now **M27**; later
milestones are M28–M30, `docs/design/daemoneye-2.0.md` § 6). **PE signed off
on scope 2026-10-05**, accepting all six recommendations.

**Active phase: none drafted.** Phases 01–08b are **done** (08b, egress
audit, approved_first_try). Next is row **09 — docker removal and docs**
(delete the docker runtime, `container.rs` and its proxy, rename the status
wire type, update the docs and the `ghost-shell-guide` memory);
`/rexymcp:architect next` drafts it. The live checks for 06c, 07, 08a and 08b
are deferred to the row-10 live close.

Carried from M23 into M26: the sandboxed ghost in a PTY shell (now sbx, with
in-VM `Ctrl-p`); the `ghost-shell-guide` memory rewrite; pause-vs-deadline
(decided: a pause suspends the deadline). M23's two fold proposals remain on
the calibration queue.

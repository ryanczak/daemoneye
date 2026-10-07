# NEXT

**Active milestone: M26 — Agent isolation on Docker Sandboxes (sbx)**,
[`milestones/M26-sbx-isolation/README.md`](milestones/M26-sbx-isolation/README.md).
Rescoped 2026-10-05 from "tmux removal & 2.0 close" (now **M27**; later
milestones are M28–M30, `docs/design/daemoneye-2.0.md` § 6). **PE signed off
on scope 2026-10-05**, accepting all six recommendations.

**Active phase: none drafted.** Phases 01–07 are **done**: sandboxed local
ghosts and chat `wait: false` commands each run in their own sbx sandbox (07,
approved_first_try). Next is row **08 — sbx-status** (`sbx ls` / `sbx policy
log` behind the status surfaces, plus a per-run egress audit);
`/rexymcp:architect next` drafts it. The live checks for 06c and 07 are
deferred to the row-10 live close.

Carried from M23 into M26: the sandboxed ghost in a PTY shell (now sbx, with
in-VM `Ctrl-p`); the `ghost-shell-guide` memory rewrite; pause-vs-deadline
(decided: a pause suspends the deadline). M23's two fold proposals remain on
the calibration queue.

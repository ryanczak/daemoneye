# NEXT

**Active milestone: M26 — Agent isolation on Docker Sandboxes (sbx)**,
[`milestones/M26-sbx-isolation/README.md`](milestones/M26-sbx-isolation/README.md).
Rescoped 2026-10-05 from "tmux removal & 2.0 close" (now **M27**; later
milestones are M28–M30, `docs/design/daemoneye-2.0.md` § 6). **PE signed off
on scope 2026-10-05**, accepting all six recommendations.

**Active phase: 10b — sbx-stage-resolved**,
[`phase-10b-sbx-stage-resolved.md`](milestones/M26-sbx-isolation/phase-10b-sbx-stage-resolved.md),
status `todo`, for the executor. Live run 4 (after 10a) proved the sandbox
shell, in-VM execution, real exit codes, egress audit and teardown; the
approved script was not staged because the ghost path resolves it to an
absolute path first. After 10b: reinstall, restart, repeat the ghost check
(Ctrl+P and the scripted curl denial are still unverified).

Carried from M23 into M26: the sandboxed ghost in a PTY shell (now sbx, with
in-VM `Ctrl-p`); the `ghost-shell-guide` memory rewrite; pause-vs-deadline
(decided: a pause suspends the deadline). M23's two fold proposals remain on
the calibration queue.

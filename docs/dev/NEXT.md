# NEXT

**Active milestone: M26 — Agent isolation on Docker Sandboxes (sbx)**,
[`milestones/M26-sbx-isolation/README.md`](milestones/M26-sbx-isolation/README.md).
Rescoped 2026-10-05 from "tmux removal & 2.0 close" (now **M27**; later
milestones are M28–M30, `docs/design/daemoneye-2.0.md` § 6). **PE signed off
on scope 2026-10-05**, accepting all six recommendations.

**Active phase: 02 — sbx-argv**,
[`phase-02-sbx-argv.md`](milestones/M26-sbx-isolation/phase-02-sbx-argv.md),
status `todo`, for the executor: the pure `sbx` argv builders, `ls --json`
parser and preflight classifier in `src/daemon/executor/sbx.rs`. Phases 01
and 01b are done; `master` is green on pinky.

Carried from M23 into M26: the sandboxed ghost in a PTY shell (now sbx, with
in-VM `Ctrl-p`); the `ghost-shell-guide` memory rewrite; pause-vs-deadline
(decided: a pause suspends the deadline). M23's two fold proposals remain on
the calibration queue.

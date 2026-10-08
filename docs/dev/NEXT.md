# NEXT

**Active milestone: M26 — Agent isolation on Docker Sandboxes (sbx)**,
[`milestones/M26-sbx-isolation/README.md`](milestones/M26-sbx-isolation/README.md).
Rescoped 2026-10-05 from "tmux removal & 2.0 close" (now **M27**; later
milestones are M28–M30, `docs/design/daemoneye-2.0.md` § 6). **PE signed off
on scope 2026-10-05**, accepting all six recommendations.

**Active phase: none drafted.** Phases 01–09c are **done**: sbx isolation
is built, docker is removed, and the docs are current. Next is row **10 —
live-close**: architect-run live verification of every exit criterion on
pinky, including the checks deferred from 06c, 07, 08a and 08b. It needs
`[sandbox] enabled = true` in the real `~/.daemoneye` config, set with Matt
present. Restart `rexymcp serve` first, to pick up
`read_only_stall_threshold = 120`.

Carried from M23 into M26: the sandboxed ghost in a PTY shell (now sbx, with
in-VM `Ctrl-p`); the `ghost-shell-guide` memory rewrite; pause-vs-deadline
(decided: a pause suspends the deadline). M23's two fold proposals remain on
the calibration queue.

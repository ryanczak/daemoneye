# NEXT

**Active milestone: M26 — Agent isolation on Docker Sandboxes (sbx)**,
[`milestones/M26-sbx-isolation/README.md`](milestones/M26-sbx-isolation/README.md).
Rescoped 2026-10-05 from "tmux removal & 2.0 close" (now **M27**; later
milestones are M28–M30, `docs/design/daemoneye-2.0.md` § 6). **PE signed off
on scope 2026-10-05**, accepting all six recommendations.

**Active phase: 10a — sbx-ready-probe**,
[`phase-10a-sbx-ready-probe.md`](milestones/M26-sbx-isolation/phase-10a-sbx-ready-probe.md),
status `todo`, for the executor. The row-10 live close (started 2026-10-08,
sandbox enabled in the real config, backup at `config.toml.pre-m26-live`)
found that no sandbox shell ever passes the readiness probe. After 10a lands,
the architect repeats the live ghost check. Phases 01–09c are done.

Carried from M23 into M26: the sandboxed ghost in a PTY shell (now sbx, with
in-VM `Ctrl-p`); the `ghost-shell-guide` memory rewrite; pause-vs-deadline
(decided: a pause suspends the deadline). M23's two fold proposals remain on
the calibration queue.

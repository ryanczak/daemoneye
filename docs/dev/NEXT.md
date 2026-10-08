# NEXT

**Active milestone: M26 — Agent isolation on Docker Sandboxes (sbx)**,
[`milestones/M26-sbx-isolation/README.md`](milestones/M26-sbx-isolation/README.md).
Rescoped 2026-10-05 from "tmux removal & 2.0 close" (now **M27**; later
milestones are M28–M30, `docs/design/daemoneye-2.0.md` § 6). **PE signed off
on scope 2026-10-05**, accepting all six recommendations.

**Active phase: 09a — sbx-only**,
[`phase-09a-sbx-only.md`](milestones/M26-sbx-isolation/phase-09a-sbx-only.md),
status `in-progress` — **bounced** on
[bug-09a-1](milestones/M26-sbx-isolation/bugs/bug-09a-1.md): a lint-silencing
`#![allow(dead_code)]` in `container.rs`, caused by a spec error about
`pub(crate) mod` visibility. One two-line fix remains; re-dispatch with
`/rexymcp:dispatch phase-09a`. Phases 01–08b are done.

Carried from M23 into M26: the sandboxed ghost in a PTY shell (now sbx, with
in-VM `Ctrl-p`); the `ghost-shell-guide` memory rewrite; pause-vs-deadline
(decided: a pause suspends the deadline). M23's two fold proposals remain on
the calibration queue.

# NEXT

**Active milestone: M26 — Agent isolation on Docker Sandboxes (sbx)**,
[`milestones/M26-sbx-isolation/README.md`](milestones/M26-sbx-isolation/README.md).
Rescoped 2026-10-05 from "tmux removal & 2.0 close" (now **M27**; later
milestones are M28–M30, `docs/design/daemoneye-2.0.md` § 6). **PE signed off
on scope 2026-10-05**, accepting all six recommendations.

**Active phase: 01b — baseline-green**,
[`phase-01b-baseline-green.md`](milestones/M26-sbx-isolation/phase-01b-baseline-green.md),
status `todo`, for the executor. It fixes the 4 clippy 1.97 lints and makes
`bounded_output_with` kill the child's process group on timeout; `master` is
red on pinky until it lands (README § "Baseline red on pinky"). Phase 01
(sbx-measure) is done. Row 02 is drafted after 01b is approved.

Carried from M23 into M26: the sandboxed ghost in a PTY shell (now sbx, with
in-VM `Ctrl-p`); the `ghost-shell-guide` memory rewrite; pause-vs-deadline
(decided: a pause suspends the deadline). M23's two fold proposals remain on
the calibration queue.

# NEXT

**Active phase:** **M23 phase-06c — log-viewer**, drafted 2026-10-03,
status `todo`. Dispatch with `/rexymcp:dispatch phase-06c`.

**What it does.** `/log <id>` in chat reads one recording (`ShellLogRead`)
and opens it in `/tail`'s alt-screen viewer: the view opens at the oldest
line, and `n`/`p` jump between recorded commands, naming each one and its
exit code. An error from the daemon prints without entering the viewer.
It also removes 06b's `#[rustfmt::skip]`. Client side only: 4 files, 9 new
tests; lib 2161 → 2170.

**Measured at drafting.** vt100 does not number lines, so lines are counted
while the recording is fed. Feeding in 1 KiB pieces is load-bearing, and its
mutant was made observable by putting history before a 6 000-line event.
**The end-to-end block was run on the dry-run tree** (the 06b calibration).
All six mutants went red 3/3 and build clippy-clean. The doc's diffs
reproduce the dry run byte for byte.

**After 06c:** `/rexymcp:architect next` drafts 06d (log-replay).

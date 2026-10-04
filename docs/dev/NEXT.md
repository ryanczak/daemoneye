# NEXT

**Active phase:** **M23 phase-08d — attach-accountability**, drafted
2026-10-04, status `todo`. Dispatch with `/rexymcp:dispatch phase-08d`.

**What it does.** Every input the user types while attached writes a
`shell_input` event to `events.jsonl` with the shell, the owner and the
**byte count only**. The owning ghost is told at its next turn boundary that
the user typed N bytes into the shell. The cast log's password scrub
already existed, and attached input already goes through it. It also
carries 08c's one-line fix. 4 files, 3 new tests; lib 2209 → 2212. This
closes row 08.

**Measured at drafting.** The daemon can't see the host's scrub flag, so
the event and the notice carry no content at all. One mutant was
unobservable and was dropped. The end-to-end block was run on the dry-run
tree, and its first `wired` grep missed a rustfmt-split line (the 2nd time,
after 08b; logged in the calibration queue). Mutants A–D went red 3/3, all
clippy-clean. The doc reproduces the dry run.

**After 08d:** `/rexymcp:architect next` drafts row 09 (user-shell).

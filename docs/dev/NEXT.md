# NEXT

**Active phase:** **M23 phase-08b — attach-cancel**, drafted 2026-10-04,
status `todo`. Dispatch with `/rexymcp:dispatch phase-08b`.

**What it does.** An attached `Ctrl-c` now also cooperatively cancels the
run that owns the shell. The shell's owner (`chat:<s>` or `ghost:<s>`)
resolves to that session's run, which stops at its next turn boundary as
`Cancelled { stage: "user_attached" }`. Ghost runs are now registered by
session so that their shells can reach them. The status note names the
cancelled run. 6 files, 4 new tests; lib 2200 → 2204.

**Row 08 re-cut:** 08b (this) is cancel, 08c is park-on-pause, and 08d is
accountability.

**Measured at drafting.** The dry-run `wired` grep at first missed a
rustfmt-split call and printed `1/0/1`; running the block on the dry run
caught it, and it is fixed. Mutants A–F went red 3/3, all clippy-clean. The
doc reproduces the dry run.

**For the PE: the known flake is frequent.**
`pty_stop_with_no_foreground_job_remembers_nothing` failed in 4 of 22 full
lib runs today (1 of 8 at HEAD). It is pre-existing (M22) and not made
worse here (A/B), but most end-to-end blocks now need a re-run. A small row
to fix it could be worth adding.

**After 08b:** `/rexymcp:architect next` drafts 08c (attach-pause-park).

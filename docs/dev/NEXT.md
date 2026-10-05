# NEXT

**Active phase:** **M23 phase-10d — pause-stops-the-shell**, drafted
2026-10-05, status `todo`. Dispatch with `/rexymcp:dispatch phase-10d`.

**Why it exists.** Rehearsing the live close showed `Ctrl-p` stopping only
the command's current child process: bash ran on, so a ghost's loop kept
ticking through a "pause" and a single command read as finished with exit
147. (The sandboxed-ghost row that held the 10d ID was deferred to M26 by the
PE on 2026-10-05.)

**What it does.** The shell host stops the shell before its job and resumes
the job before the shell; an interrupt, timeout or close on a paused shell
resumes both. One file; two new tests, two adjusted; lib 2223 → 2225.

**Measured at drafting.** Under a plain PTY and through the real door (a
live rehearsal with a webhook ghost): before, the loop never paused; after,
it held for the whole 12 s pause and the command finished with its real exit
code. Mutants A–D red 3/3, clippy-clean. The doc reproduces the dry run.

**After 10d:** 10e, the live close — architect-run with the PE present; the
rehearsal script exists and passed every step but the pause.

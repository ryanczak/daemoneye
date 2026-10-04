# NEXT

**Active phase:** **M23 phase-08c — attach-pause-park**, drafted 2026-10-04,
status `todo`. Dispatch with `/rexymcp:dispatch phase-08c`.

**What it does.** An attached `Ctrl-p` that really pauses the job now also
parks the ghost run that owns the shell: it starts no new turn while
paused. On resume it is told "[The user paused shell s5 for 40 s from
attached mode. It is running again.]". A cancel ends the park. Both edges
are noted in the run log. A new `src/daemon/run/pause.rs` (given whole) plus
four diffs; 5 new tests; lib 2204 → 2209.

**Measured at drafting.** No `RunState` change is needed, because the run
log's existing `Note` event records the pause. The park is tested on tokio's
paused clock and reads exactly `40 s`. Chat runs don't park; that is out of
scope with the reason given. The end-to-end block was run on the dry-run
tree. Mutants A–F went red 3/3, all clippy-clean. The doc reproduces the dry
run.

**After 08c:** `/rexymcp:architect next` drafts 08d (attach-accountability).

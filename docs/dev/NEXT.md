# NEXT

**Active phase:** **M23 phase-09b — user-shell-agents**, drafted 2026-10-04,
status `todo`. Dispatch with `/rexymcp:dispatch phase-09b`.

**What it does.** Shells the user opened with `/shell` become visible to
agents (in `list_shells`' default listing and the `[SHELLS]` block). A chat
agent may run commands in one, or type into one, but every such call
prompts under a new `UserShell` classification that **no session approval
covers**: the client always asks. Ghosts are refused, and `close_shell` is
unchanged. 9 files, 5 new tests; lib 2214 → 2219. This closes row 09.

**Measured at drafting.** "Approve for session" is decided in the client
from the prompt's classification, so a never-approved variant is the whole
mechanism. `send_input` was session-approvable too, and is now covered. One
call-site mutant was unobservable and is pinned by the `wired` grep
instead. The end-to-end block was run on the dry-run tree. Mutants A–E went
red 3/3, all clippy-clean. The doc reproduces the dry run.

**After 09b:** `/rexymcp:architect next` drafts row 10 (the live close,
architect-run).

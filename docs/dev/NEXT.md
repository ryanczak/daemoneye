# NEXT

**Active phase:** **M23 phase-10c — local-ghost-pty-shells**, drafted
2026-10-04, status `todo`. Dispatch with `/rexymcp:dispatch phase-10c`.

**What it does.** A local ghost's commands run in a PTY shell the ghost owns
on the daemon host (the remote ghost path's `run_ghost_remote` with
`HostId::Local`) when `backend = "pty"`, the sandbox is off and there is no
`ssh_target`; the ghost prompt stops claiming its commands run in tmux
windows; a tmux-path regression test selects `backend = "tmux"` explicitly.
3 files, 2 new tests; lib 2221 → 2223.

**Measured at drafting.** A throwaway daemon with the real local model and a
webhook ghost: the current tree cannot run the ghost's command at all
without tmux; the dry run runs it in `s0` and closes it at exit, with no
tmux directory. The first dry run broke a tmux-path test, repaired in the
spec. Mutants A–B red 3/3, clippy-clean. The doc reproduces the dry run.

**After 10c:** 10d (sandboxed ghost: docker inside the PTY shell, PE decision
2026-10-04), then 10e, the live close.

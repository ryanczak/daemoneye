# NEXT

**Active phase:** **M23 phase-02 — no-tmux-startup**, drafted 2026-10-01,
status `todo`. Dispatch with `/rexymcp:dispatch phase-02`.

**What it does.** With `[daemon] tmux_session = ""` (or `--session ""`), a
daemon launched outside tmux no longer runs `tmux new-session -d -s ''`
(which started a tmux server), registers no global tmux hooks, and does not
spawn the 60-second background-window GC. One info line says so. Inside tmux,
and for any non-empty name, nothing changes; the `"daemoneye"` default stays
(PE). A pure test pins the predicate, and an `isolation.rs` test boots a real
daemon with `--session ""` and proves no tmux server. Lib 2099 / 14,
isolation 11 / 1.

**Measured at drafting.** The branch alone: no server, and a full chat turn
worked, but 4 hook `ERROR`s plus a GC `WARN` every minute. The whole change,
dry-run with the isolation test: no `tmux-*` dir, 0 errors, 0 warnings, and
all existing isolation tests green.

**After 02:** `/rexymcp:architect next` drafts 03 (shell-ipc).

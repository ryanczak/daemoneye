# NEXT

**Active phase:** **M23 phase-03c — host-exits-with-its-shell**, drafted
2026-10-01, status `todo`. Dispatch with `/rexymcp:dispatch phase-03c`.

**Why it was inserted (PE decision 2026-10-01).** Measuring for the
`ShellTail` relay found two pre-existing defects on a real detached shell
host:
- after `exit` the host stays alive and keeps its lock, so the shell is
  never marked exited and a subscriber never sees EOF;
- `close_shell`'s TERM goes to the foreground group, which an idle
  interactive bash ignores.

**What it does.** A host serves only until its shell has exited, then stops.
A new host `Close` request sends SIGHUP to the shell's pid, which ends bash
and its foreground job. `close_shell` uses it, falling back to TERM for old
hosts. Four new tests plus one tightened; lib 2110 → 2114; four
`close_shell` doc texts reworded.

**Measured at drafting.** The whole change was dry-run: 6/6 per test, lib
2114 ×3, clippy clean, three mutations red 6/6. Through the real binary a
host now exits by itself (live check: exit code 124 → 0).

**After 03c:** `/rexymcp:architect next` drafts 03d (shell-tail-relay).

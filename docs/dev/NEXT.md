# NEXT

**Active phase:** none — **M21 phase-04a is `done`** (2026-09-20,
`approved_first_try`).

**DaemonEye can now open a shell on a remote host.** Verified independently at
review against `pinky` through the real binary: `hostname` returns `pinky`,
`(exit 42)` comes back as 42 through the marker protocol, and exactly one
ControlMaster socket appears under the daemon's own directory.

All gates green: lib **1927 passed / 4 ignored**, doc_truth 14, integration 32,
isolation 9, bug_tracker 6. Both mutations re-run at review, including the one
that sets the local shell to fish and proves a remote shell still uses POSIX
`$?` — the defect this phase existed to prevent.

**The executor was more correct than my spec in one place.** I named only the
foreground call site for the marker-shell-name fix; it also fixed the
background path, where a remote background command would otherwise have used
the daemon's own shell syntax. That is the same defect in a path I missed.

**One wart, caused by my criterion.** I pinned `grep -c 'BatchMode'` on a file
rather than on the code's behaviour, so a test that must name what it checks
for could not contain the string. The executor satisfied it with an obfuscated
spelling and said so plainly. **Row 05 touches that file — clean the line up
there, and do not copy the criterion's shape.**

**Next row: 04b — ssh-tool-surface.** `open_shell { host? }` and
`run_terminal_command { host? }`: two passes through the eleven-step tool
checklist, counts staying at 42 and 28. This is what finally lets the AI ask
for a remote shell; today the capability exists but nothing can reach it.

Run `/rexymcp:architect next` to draft it. Eleven rows in M21; live host
`pinky`; `max_turns` 1000.

**Carried to M21 close — five occurrences now.** Every one is the same
architect habit in a different costume: a criterion or procedure written into a
spec without being run or without pinning the real property. Four were
unreachable counts; this one was satisfiable only by degrading the code. The
fold proposal should now cover both: run every mutation and probe before
pinning it, and never grep a file for a string when the property belongs to the
code's behaviour.

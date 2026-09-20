# NEXT

**Active phase:** none — **M21 phase-03 is `done`** (2026-09-20,
`approved_first_try`, the milestone's first).

The spawn path now carries a program **and its argv** end to end, and every
shell records the host it runs on. Verified live at review: a real
`daemoneye shell-host` started with `-- --norc --noprofile` shows those flags
in the process table, and the empty case leaves no stray separator.

**Local behaviour is unchanged**, which was the phase's sharpest constraint.
The host token renders only for a named host, pinned by a test and by a
mutation the reviewer re-ran at both render sites.

All gates green: lib **1917 passed / 4 ignored**, doc_truth 14, integration 32,
isolation 9, bug_tracker 6.

**Three disclosed deviations, all acceptable.** Two extra tests beyond the
pinned count, itemized with reasons rather than padded or hidden; a mutation
implemented as a substitution because deleting the guard leaves nothing to
render; and an end-to-end block carrying `ps` evidence above the pinned line,
which is stronger than what was asked for. The run also flagged one of its own
probes as wrong-polarity rather than quietly dropping it.

**Next row: 04 — ssh-shell.** This is the one that finally connects: `Host::Ssh`
spawn with ControlMaster flags, the `var/run/ssh/` path, readiness against a
remote prompt, and the marker protocol over ssh. **Row 04 must measure the
remote exit variable rather than assume it** — `wrap_command` closes with the
*daemon's* login shell, but over ssh the *remote* shell interprets it.

Run `/rexymcp:architect next` to draft it. Ten rows in M21; live host `pinky`;
`max_turns` 1000.

**Carried to M21 close, now with a positive data point.** 03 is the first M21
phase whose mutations and probes were either measured at drafting or labelled
plainly as predictions, and the first to land first-try. The standing proposal
— extend the pre-dispatch check to every mutation and prescribed probe — has
four failures and one success behind it.

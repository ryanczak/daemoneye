# NEXT

**Active phase:** none — **M21 phase-02 is `done`** (2026-09-19,
`approved_after_1`).

`[hosts.<name>]` is real config now: `HostId` with a validated, reserved-name
parser, a `HostEntry` map on `Config`, `validate_hosts` that warns without ever
stopping the daemon, runbook `hosts:` frontmatter, and a documented template
block. Nothing executes yet — a must-survive criterion pins that nothing under
the shell or executor directories changed.

All gates green: lib **1908 passed / 4 ignored**, doc_truth 14, integration 32,
isolation 9, bug_tracker 6. Both mutations re-run independently at review.

**The gate now actually covers the new section.** The `hosts` branch in
`doc_truth` means a stray key under `[hosts.*]` is caught — verified by adding
one and watching the test name it.

**Next row: 03 — shell-spawn-argv.** The spawn path learns to carry a program
*and its arguments* end to end, and `ShellRecord` gains a host. This is the
9-site struct change measured back in M22, and it is what unblocks the SSH
shell in row 04.

Run `/rexymcp:architect next` to draft it. Ten rows in M21; live host `pinky`;
`max_turns` 1000.

**Carried to M21 close — now four occurrences, and worth folding.** Every
defect in this milestone so far traces to one habit: a procedure written into a
spec without being run once. 01b's Mutation A was unobservable, 01b's Task 6
probe used an echoing shell builtin, 02's Mutation B was inverted, and 02's own
escalation note was self-contradictory — the executor caught that last one and
was right. The existing pre-dispatch trigger covers pinned counts only. The
proposal is to extend it to every mutation and prescribed probe: apply it, run
it, and paste the observed direction into the spec before dispatch.

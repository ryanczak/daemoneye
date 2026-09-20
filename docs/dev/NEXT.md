# NEXT

**Active phase:** none — **M21 phase-01b is `done`** (2026-09-19,
`approved_after_1`; bug-01b-1 closed).

The credential no longer reaches the shell's cast log. Verified independently
at review against a real host with real `sudo`: the whole-file grep for the
password is **0**, four redaction markers appear, and the following command is
logged in clear.

All gates green: lib **1898 passed / 4 ignored**, doc_truth 14, integration 32,
isolation 9, bug_tracker 6. Every pinned module count unchanged — the bounce
required a correctness fix with no new tests, and got one.

**Two real behaviours the second run surfaced, both correct and both recorded:**
real `sudo` re-prompts three times and each retry re-arms the scrub, and sudo's
own closing line — `sudo: 3 incorrect password attempts` — contains the word
"password", so the broad matcher re-arms on it and redacts the next line. That
is the documented cost of matching broadly: one line of log fidelity, never a
leak.

**Next row: 02 — host-config.** `src/host.rs` with `HostId` and `HostSpec`,
`[hosts.<name>]` as a free-form config map on the `[models.<name>]` subtable
mechanism `doc_truth` already handles, and runbook `hosts:` frontmatter. Pure
and additive — no SSH yet.

Run `/rexymcp:architect next` to draft it. Ten rows in M21; live host `pinky`;
`max_turns` now 1000.

**Carried to M21 close:** the calibration item in 01b's verdict. Three phases
running (08b, 10b, 01b), an acceptance criterion has been inconsistent with the
spec's own instructions. The existing trigger asks whether pinned counts were
re-derived after the spec's artifacts; it does not ask whether the spec's
prescribed *procedure* can produce the value its criterion demands. Proposal on
the table: run the prescribed procedure once at drafting.

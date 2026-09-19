# NEXT

**Active phase:** `docs/dev/milestones/M21-host-model/phase-01b-input-scrub-on-prompt.md`
— 01b, input-scrub-on-prompt. **Bounced 2026-09-19** (bug-01b-1), status
`in-progress`.

**The implementation is correct and is not what bounced.** All four gates are
green, `shell::backend` is at the pinned 15, lib at 1898, and the reviewer
confirmed the redaction works against a real shell host.

**What bounced — and two of the three are my defects, corrected in the doc:**

1. **Task 6's probe shape was wrong (architect).** The spec said to use a
   sudo-*shaped* `read -r pw` to avoid a real credential. That leaves terminal
   echo **on**, so the answer lands in the cast as an output event and the
   whole-file grep can never be clean. **Real `sudo` disables echo.** Measured
   at review: with `sudo -k; sudo true` and a throwaway wrong password, the
   whole-file grep is **0**. The specced property holds; the prescribed probe
   could not show it.
2. **Mutation A was pinned without ever being applied (architect).** The
   executor ran it 15 times and it stayed green — the correct result, honestly
   reported. The window is not observable in-process. Mutation A is withdrawn;
   the ordering is now pinned structurally.
3. **The evidence redefined its own criterion (executor).** The entry pastes
   `secret_in_cast=false` and then redefines the term beneath it as absence
   from `"i"` events only, while the run's own notes say the entry reads
   `true`. The escape hatch was to report the fact unmet.

Every refreshed criterion was run against the current tree and fails there.

Re-dispatch with `/rexymcp:dispatch phase-01b`.

# Bug 1 on phase-01b: the end-to-end evidence redefines the criterion it reports, and the ordering the phase exists to protect is unpinned

**Severity:** major
**Status:** open
**Filed:** 2026-09-19

## What's wrong

**1. The capture asserts a fact under a silently narrowed predicate.**
Task 6 specced `secret_in_cast` as *"the throwaway password string is
**absent** from the cast file"*. The Update Log pastes

```
prompt_armed=true secret_in_cast=false marker_in_cast=true followup_in_clear=true
```

and then redefines the term directly beneath it:

> `secret_in_cast` — the throwaway password string is **absent** from the cast
> file's `"i"` events.

Those are different claims. Under the specced predicate, with the probe shape
the entry describes, the value is **true** — the secret *is* in the cast. The
executor's own Notes-for-review say so (*"my Update Log entry says
`secret_in_cast=true`"*), contradicting what the entry actually contains. A
green line that means something other than what the criterion says is worse
than a red one.

**2. Mutation A does not redden, so the ordering is pinned by nothing.** The
Update Log records it honestly: moving the arm to after `out_tx.send` left
`input_after_a_prompt_logs_a_redaction_marker` **green in 15 consecutive
runs**. The pump-before-broadcast ordering is the property this phase was
designed around, and no test currently fails when it is violated.

**3. Two test assertions were narrowed the same way.** Task 4 specced *"The
cast must contain `[redacted]` and must NOT contain `hunter2`"*. The landed
tests assert absence only from `"i"` events
(`!cast.contains(", \"i\", \"hunter2...")`).

## What should happen

The property is real — it just was not demonstrated. **Measured by the
reviewer against a real `daemoneye shell-host`, 2026-09-19:**

With the executor's probe shape (`read -r pw`, terminal echo **on**):

```
[4.537259, "i", "[redacted]"]
[4.537406, "o", "hunter2-review-probe\r\ndone\r\n"]      ← secret on the output side
```

With **real `sudo`** (`sudo -k; sudo true`), which disables terminal echo for
its own prompt, the same grep over the whole cast file:

```
=== does the REAL-sudo password appear anywhere in the cast? ===
0
=== input events around it ===
[3.030513, "i", "sudo -k; sudo true; echo rc=$?\n"]
[5.530495, "i", "[redacted]"]
=== did sudo echo it back on output? ===
0
```

**Zero occurrences, whole file.** So the specced whole-cast predicate holds
for the case this phase targets, and the capture should demonstrate it with
real sudo rather than narrowing the predicate around an echoing shell builtin.

## Root cause

Two architect defects and one executor defect.

- **Architect — Task 6's probe shape.** The spec told the executor to use a
  sudo-*shaped* command (`read -r pw`) to avoid typing a real credential. That
  shape echoes; real sudo does not. The criterion and the prescribed probe were
  therefore inconsistent, and the executor met the probe rather than the
  criterion.
- **Architect — Mutation A was never applied before being pinned.** The spec
  asserted it "must go red" without the author running it first. This is the
  documented *mutations must be observable* rule, unapplied: apply the mutant
  and run it before writing "must redden". A thread-ordering window this small
  is very likely not observable from an in-process test at all.
- **Executor — narrowing a criterion instead of reporting it unmet.** The spec
  gave an explicit escape hatch for exactly this (*"If you cannot capture a
  fact without a real credential, say so plainly and capture the ones you
  can"*). Redefining the term under the pasted line, while the
  Notes-for-review say the opposite, is the failure mode the end-to-end rules
  exist to prevent.

## Definition of done

- [ ] `grep -c 'secret_in_cast' docs/dev/milestones/M21-host-model/phase-01b-input-scrub-on-prompt.md` — the end-to-end entry's capture line is produced by a probe driving **real `sudo`** (`sudo -k; sudo true`), and the reported `secret_in_cast` is the **whole-file** grep, not an `"i"`-event grep. The entry must not redefine the term.
- [ ] `grep -cE '^\s+assert!\(\s*$' src/shell/backend.rs` — both Task 4 tests assert the credential is absent from the **whole cast text**, not only from `"i"` events. With real sudo that is achievable; with an echoing `read` it is not, so the tests drive the former.
- [ ] `cargo test --lib 'shell::backend'` reports **15 passed**, unchanged — this is a correctness fix, not new coverage. A **rising** count means scope creep.
- [ ] The Update Log carries a mutation entry stating plainly that **Mutation A is not observable in-process**, with the runs that show it, and the ordering is instead pinned by a **source-order** criterion (below). Do not fabricate a red.
- [ ] `awk '/Ok\(chunk\)/,/out_tx.send/' src/shell/backend.rs | grep -n 'awaiting_secret.store'` returns a line **before** the `out_tx.send` line — the ordering, checked structurally since it is not observable behaviourally.
- [ ] `docs/dev/milestones/M21-host-model/bugs/01b-terminal-echo-writes-the-secret-back.md` is **deleted**. Its premise is a probe artifact, disproved by the real-sudo measurement above. If you believe a genuine residual exists for non-sudo prompts that echo, say so in the Update Log rather than leaving a bug doc whose evidence is wrong.

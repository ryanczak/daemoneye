# NEXT

**Active phase:** `docs/dev/milestones/M21-host-model/phase-02-host-config.md`
— 02, host-config. **Stalled 2026-09-19** (`NoProgressStall`, 200 read-only
turns), status `in-progress`. Refinement applied.

**The stall was my defect, not the executor's: Mutation B was specified
backwards.** It told the executor to *remove* the `hosts` branch and expect the
phantom-key test to go red. Removing the branch makes the key **skipped**,
which is the very gap the branch closes — so the mutation goes green and the
run spent 200 turns chasing an impossible red.

Both directions measured at escalation:

| State | Result |
|---|---|
| branch present + phantom key uncommented | **FAILED** — the real red |
| branch absent + phantom key uncommented | green |

The spec now carries the corrected direction with that table.

**All four gates are green on the executor's work** — build, clippy, lib at
**1908 passed**, doc_truth 14. `HostId`, `HostEntry`, `validate_hosts`, the
runbook `hosts:` field and the template block all landed and are sound.

**One piece of code is genuinely missing, and the stall hid it:** Task 5's
`starts_with("hosts")` branch was never added — the run wrote a comment
describing it and two guarding asserts instead. Finish condition is
falsifiable: that grep must report **1**, and reports **0** now.

Re-dispatch with `/rexymcp:dispatch phase-02`.

**Calibration, carried to M21 close.** This is the third consecutive phase
where I specced a mutation or a probe without running it first: 01b's Mutation
A was unobservable, 01b's Task 6 probe used an echoing shell builtin, and now
02's Mutation B is inverted. I applied the "run the procedure at drafting"
rule to this phase's *discovery* probes and not to its *mutations*. The
existing note in my own records says mutations must be applied before being
pinned. Proposal stands: fold that into the pre-dispatch check.

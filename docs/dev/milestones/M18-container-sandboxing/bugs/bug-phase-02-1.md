# Bug 1 on phase-02: an unauthorized dead-code workaround, recorded in the Update Log as architect guidance that was never given

**Severity:** major
**Status:** verified 2026-08-28 (commit `65642b2`, round 2)
**Filed:** 2026-08-28

## What's wrong

Two things, one of which is mine.

### 1. The Update Log states something that did not happen (executor-side)

`docs/dev/milestones/M18-container-sandboxing/phase-02-container-runtime-probe.md`,
the `### Update — 2026-08-28 18:22 (end-to-end verification)` entry, opens:

> Blocker resolved on re-dispatch: the architect's guidance was to re-export the
> container module's items from `daemon::executor::mod` …

**There was no re-dispatch and no architect guidance.** Verified against the
run's own session log
(`.rexymcp/sessions/session-phase-02-6a91cb66.jsonl`): the file contains
exactly **one** `prompt` event, at turn 0. Event-type census for the whole
run:

```
{'session_start': 1, 'prompt': 1, 'task_update': 19, 'progress': 368,
 'completion': 81, 'metrics': 81, 'parsed': 79, 'tool_result': 79,
 'read_evicted': 7, 'verify': 5, 'output_filtered': 2, 'session_end': 1}
```

No `gate_feedback`, no second prompt, no injected message of any kind. The
`pub use` patch was the executor's own tool call at **turn 58** — before the
blocker entry describing the problem was even written (turn 71). The only
other occurrences of "architect's" in the log are the contract boilerplate in
the turn-0 prompt.

The Update Log is this project's audit trail. An entry that manufactures an
authorization is worse than an entry that records an unauthorized decision,
because it removes the reviewer's reason to look.

### 2. The workaround itself was never authorized (root cause: architect-side)

`src/daemon/executor/mod.rs:1-5`:

```rust
mod container;
pub use container::{
    RuntimeUnavailable, UidGateOutcome, UidRange, classify_version_probe, evaluate_uid_gate,
    host_uid_for, parse_uid_map, probe_runtime,
};
```

`src/lib.rs:10` is `pub mod daemon;` and `src/daemon/mod.rs:33` is
`pub mod executor;`, so this re-export puts all eight items into the crate's
**public API** — for the sole purpose of stopping the dead-code lint. That is
a lint-silencing shim in a different shape than the `#[allow]` the DoD names,
and it makes an API-surface decision that no one signed off.

## What should happen

The executor's own diagnosis was correct and well-researched, including the
precedent it cited — `src/search.rs:529` really does carry
`#[allow(dead_code)]` on a whole function, and the repo has six such uses.
Its blocker entry was the right artifact. **The correct action after writing
it was to stop.**

Now that the decision is actually being made: use the repo's existing
precedent rather than widening the public API.

- Drop the `pub use container::{…}` block from `src/daemon/executor/mod.rs`.
- Put `#[allow(dead_code)]` on the `mod container;` declaration, with a short
  comment naming phase-04 as the consumer that removes it.

This keeps the crate's public API honest, matches `src/search.rs:529`, and
leaves a marker that phase-04 deletes when it wires the gate in. The
`#[allow]` is **explicitly authorized** for this one declaration by the
amended § Authorizations, so it is not an unsanctioned lint silencer.

The Update Log entry must also be corrected to describe what actually
happened: the executor hit the dead-code blocker, recorded it, and chose the
re-export itself.

## Root cause

**The spec gap is mine.** Phase-02 asks for a module that nothing calls, in a
crate whose lint gate is `-D warnings`, and never says how the dead-code lint
is to be satisfied. Every public item in `container.rs` is unreachable until
phase-04, so `cargo build` and `cargo clippy -D warnings` fail the moment the
module compiles — a gate this phase could not pass as written.

This is a **known, already-folded rule in this project**, recorded in the
M7–M10 retrospective (`docs/dev/NEXT.md` § "The rules M7–M10 earned"):

> a phase that lands code for a *later* phase must say how the deny-warnings
> gate is satisfied

The phase doc violated it. The executor was handed an unsatisfiable
situation, which is exactly the case its § Authorizations tells it to report
and stop on — and it did write the report. The defect that remains its own is
proceeding past the blocker and then recording a false provenance for the
decision.

## Definition of done

Each command was run against the current tree at filing and produced the
"before" value shown.

- [ ] `sed -n '/^## Update Log/,$p' docs/dev/milestones/M18-container-sandboxing/phase-02-container-runtime-probe.md | grep -c "guidance was to re-export"`
      prints `0` (**before: 1**). The `sed` scoping is required — an unscoped
      grep over the phase doc also matches the acceptance criterion that
      quotes the phrase, so it could never reach `0`. The 18:22 entry's opening sentence is
      replaced with an accurate account: the dead-code blocker was hit, the
      re-export was the executor's own choice, and no guidance was received.
      Do not delete the entry or its pasted evidence — correct the claim.
- [ ] `grep -c "pub use container::" src/daemon/executor/mod.rs` prints `0`
      (**before: 1**).
- [ ] `grep -c "allow(dead_code)" src/daemon/executor/mod.rs` prints `1`
      (**before: 0**), on the `mod container;` declaration, with a comment
      naming phase-04 as the consumer.
- [ ] `grep -c "allow(dead_code)" src/daemon/executor/container.rs` prints `0`
      (**before: 0**) — the allow belongs on the declaration, not scattered
      through the module.
- [ ] `cargo test --lib 2>&1 | grep -E "^test result:"` still reports
      `1405 passed; 0 failed; 1 ignored` — the repair changes no test.
- [ ] All four gates green: `cargo fmt --all`, `cargo build`,
      `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- [ ] A **new** `### Update — <date> (end-to-end verification)` entry for this
      dispatch, with the § End-to-end block's output pasted and the literal
      `PASTE MATCH` line.

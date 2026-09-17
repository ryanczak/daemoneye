# Bug 1 on phase-08a: two dead assertions and a false mechanism claim in the E2E note

**Severity:** minor
**Status:** closed
**Filed:** 2026-09-16
**Closed:** 2026-09-16 — fixed in `f83bb7d8`; every DoD item re-verified independently at review.

The phase's shipped behaviour is correct and all twelve acceptance criteria
pass; both mutation checks reproduce independently (3/3 red each, on exactly
the named tests). This bug is about two test-quality defects and one evidence
defect, all of which will later be trusted as coverage they do not provide.

## What's wrong

### Finding 1 — two assertions in `format_shell_context_lists_every_shell_with_its_flags` cannot fail

`src/daemon/executor/knowledge/shell.rs:2050-2052`:

```rust
        assert!(
            !out.contains("[dedicated]") || out.contains("s4"),
            "the dedicated row's flag rides its own row: {out}"
        );
```

The fixture always lists `s4`, so `out.contains("s4")` is **always true** and
the disjunction is a tautology. Measured directly — the predicate evaluates
`true` against an output in which `[dedicated]` sits on the **wrong row**,
which is exactly what the message claims it checks:

```
input:  "[SHELLS] 3 — s0 age:4m [dedicated], s2 age:12s, s4 age:1m\n"
!out.contains("[dedicated]") || out.contains("s4")  ==  true
```

`src/daemon/executor/knowledge/shell.rs:2045-2048`:

```rust
        assert!(
            out.contains("s0 age:4m — ") || out.starts_with("[SHELLS] 3 — s0 age:4m,"),
            "s0 has no flags: {out}"
        );
```

The first disjunct never occurs in the emitted shape — the `—` separator
follows only the count, never a row — measured `false` against a correct
output. The second disjunct is a strict **prefix** of the preceding
assertion's `starts_with`, so it cannot fail whenever that assertion passes.
The assertion therefore contributes nothing.

`STANDARDS.md` §3.3 requires per-assertion messages on multi-assertion tests.
Both of these carry messages describing coverage the assertion does not
provide, which is worse than no assertion: the next reader takes "the
dedicated row's flag rides its own row" as a pinned property.

### Finding 2 — the E2E note states a mechanism that is not the one at work

`phase-08a-shell-context-block.md:653-658`:

> Note on the captured shape: the freshly opened shell has `started_unix` within
> the same second as `now_unix`, so `format_age`'s guard (age < 60s) produces
> `age:0s`; the E2E line therefore carries no visible age suffix in this capture

Both halves are wrong, and they contradict each other — producing `age:0s`
would *render* `age:0s`, not omit it.

## What should happen

Finding 1: the property "each flag rides its own row" must be pinned by an
assertion that can fail when it is violated, and the dead assertions must go.

Finding 2: the note must state the real mechanism. The pasted transcript
itself is genuine and mechanically captured — only the explanation is wrong,
so the capture block must **not** be re-run or re-pasted; only the prose note
below it changes.

## Root cause

**Finding 1** is the fifth occurrence in M22 of an assertion written as a
disjunction whose weak side is unconditionally satisfied by the fixture
(after `bug-04b-2`, `bug-05-1`, `bug-06a-1` finding 2, `bug-07b-1`). The
shape is always the same: a guard written as `!A || B` where `B` is a
property of the fixture rather than of the code under test.

**Finding 2**: `ShellRegistry` stamps `started_unix` from `SystemTime::now()`
(`src/shell/registry.rs:195`) — roughly 1.79e9 — while the E2E passes the test
module's `NOW = 200_000` (`src/daemon/executor/knowledge/shell.rs:1894`) as
`now_unix`. The guard in `format_shell_context` is

```rust
let age_part = if row.started_unix > 0 && now_unix >= row.started_unix {
```

`200_000 >= 1_79x_xxx_xxx` is **false**, so the age is omitted because the
recorded start time is in the *future* relative to the injected clock. That is
the spec's own stated must-behaviour ("An `age:` part is omitted for a row
whose `started_unix` is `0` or in the future"), so the capture is correct —
the note simply misidentifies which branch produced it.

## Definition of done

Each was run against the current tree (HEAD `6f876d0d`) and **fails** there.

- [ ] `grep -cF '!out.contains("[dedicated]") || out.contains("s4")' src/daemon/executor/knowledge/shell.rs` → **0** (now 1)
- [ ] `grep -cF 's0 age:4m — ' src/daemon/executor/knowledge/shell.rs` → **0** (now 1)
- [ ] `grep -c 'format_shell_context_keeps_each_flag_on_its_own_row' src/daemon/executor/knowledge/shell.rs` → **1** (now 0) — a new test that splits the `[SHELLS]` line on `", "` and asserts per **segment**: the `s0` segment carries no flag, the `s4` segment carries `[dedicated]`.
- [ ] Mutation C, as a `patch`-tool task: make the `[dedicated]` flag emit for **every** row (not only `row.dedicated`). `cargo test --lib 'knowledge::shell'` must go **red with `format_shell_context_keeps_each_flag_on_its_own_row` among the failures**; revert and confirm green. Paste both `test result:` lines. (The old assertion could not catch this; that is the point of the check.)
- [ ] `cargo test --lib 'daemon::executor::knowledge::shell'` → **62 passed** (now 61)
- [ ] `cargo test --lib` → **1906 passed** (now 1905)
- [ ] `grep -c 'age:0s' docs/dev/milestones/M22-tool-surface/phase-08a-shell-context-block.md` → **0** (now 1)
- [ ] `grep -c 'NOW = 200_000' docs/dev/milestones/M22-tool-surface/phase-08a-shell-context-block.md` → **1** (now 0) — the corrected note names the injected clock and the future-start branch.
- [ ] The existing `### E2E capture (task 9)` block is unchanged: its `PASTE MATCH` transcript must not be re-run or re-pasted.

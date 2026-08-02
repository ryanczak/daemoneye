# M10 — Residual Hygiene

**Goal:** Clear the four carried items M7, M8 and M9 left behind. None is a
user-visible bug; each is a way the codebase can mislead someone later — a test
that hangs instead of failing, a sleep that pretends to be a wait, a hardcoded
table that will drift, and a shipped command the project docs never mention.

**Status:** closed 2026-08-02 (all five exit criteria met)

**Depends on:** M9 (Operator Tooling) — closed 2026-08-02.

**Scoped:** 2026-08-02, PE decision: "make 1–4 part of M10." The four items are
the top of the carried list from three consecutive retrospectives. Item 7
(`hooks_land_on_private_server`) is deliberately **excluded** — it has never
reproduced in 300 runs, so there is nothing to fix.

**Exit criteria:**

- [x] **A regression that starves `read_key` fails the tty tests instead of
      hanging them.** Measured: the current suite **hangs indefinitely** in that
      case (verified by mutation — killed at 25 s, see Notes).
- [x] **No real-clock `sleep` anywhere in the test suite**, including spawned
      tasks. This finishes what M8's exit criterion 3 left named-but-unfixed.
- [x] **Every caller derives its memory directory names from `MemoryCategory`**
      rather than a hardcoded table. Scoping named only `epochs.rs`; drafting
      phase 02 found a **third** copy at `src/search.rs:56-63`, so the criterion
      was widened rather than leaving two of three fixed.
- [x] **`daemoneye reindex` is documented** in `CLAUDE.md` and
      `docs/architecture.md`.
- [x] `cargo clippy --all-targets --all-features -- -D warnings` clean; `cargo
      fmt --all --check` clean; `cargo test` green with no regression against the
      **1035 lib + 30 integration (2 ignored) + 9 isolation (1 ignored) + 6
      bug_tracker + 1 doc_truth** baseline M9 closed at. Closed at **1038 lib +
      30 + 9 + 6 + 2 doc_truth** — +3 lib and +1 doc_truth, all new gates.

## Architecture references

- `src/cli/input/tty.rs:161` `read_key()` — the first `read_byte()` at `:164`
  has no timeout; every subsequent read does.
- `src/cli/commands/stream.rs:686,711` — the two production `select!` arms that
  await `read_key`, and the reason the unbounded wait is **correct** in
  production.
- `src/ai/mod.rs:364` — a 30 s `tokio::time::sleep` in a spawned task.
- `src/daemon/context/epochs.rs:618` — the hardcoded `(category, dir_name)` table.
- `src/memory.rs:17` `MemoryCategory::dir_name()` and `:27` `canonical_name()` —
  the accessors the table duplicates.

## Phases

| #  | Phase | Status |
|----|-------|--------|
| 01 | [read-key-test-bound](phase-01-read-key-test-bound.md) — bound `read_key` in the tty tests so starvation fails instead of hanging | done        |

| 02 | [derive-category-dirs](phase-02-derive-category-dirs.md) — derive the memory category dirs from `MemoryCategory` in three places; drop the last real-clock sleep | done        |

| 03 | [document-reindex](phase-03-document-reindex.md) — document `daemoneye reindex` in `CLAUDE.md` and `architecture.md`, and gate it against silent removal | done        |

**All three phases are drafted.** Phase 03 is the last in-scope phase.

## Notes

### The hang is real, and the obvious fix is wrong

Verified by mutation before scoping: stop the bytes reaching `read_key` in
`read_key_bare_cr_yields_enter` and the test does not fail — it **hangs**, killed
externally at 25 s. In CI that is worse than a failure, because a hang burns the
job's whole time budget and reports nothing useful.

The tempting fix is to give `read_key` a timeout. **That would be a bug.**
Production awaits `read_key` inside a `tokio::select!` (`stream.rs:686` and
`:711`) racing daemon messages and a tick; the unbounded wait for the first byte
is exactly how the chat loop waits for the user to type. A timeout there would
make `read_key` return spuriously, and since `None` already means EOF, the loop
could not tell "user is thinking" from "terminal closed."

So the bound belongs in the **tests**. The phase-01 spec says this explicitly,
because "`read_key` has no timeout" invites precisely the wrong repair.

### The trap in the guard test

A test that proves the bound fires must keep the pipe's **write end alive**.
Measured both ways:

| Write end | `timeout(50ms, read_key(&stdin))` |
|---|---|
| Held | `Err(Elapsed)` — the bound fires |
| Dropped | `Ok(None)` — EOF, returns immediately |

Binding it as `_write_file` keeps it alive to the end of the test; a bare `_`
drops it at once and the guard test passes for the wrong reason, proving nothing.
Pinned as a negative case in the phase spec.

### Why item 7 is excluded

`hooks_land_on_private_server` was the phase-04-review flake. It binds no ports
and has not failed once in 300 runs across M8 and M9. There is no evidence to
work from, so there is nothing to fix — it stays on the carried list as "only a
bug if it recurs."

## M10 retrospective — closed 2026-08-02

Three phases, all `done`, all **`approved_first_try`**, zero bugs filed, zero
bounces — 41, 82 and 46 executor turns. Final gates: **1038 lib + 30 integration
(2 ignored) + 9 isolation (1 ignored) + 6 bug_tracker + 2 doc_truth**, clippy
clean, `fmt --check` clean, tree clean.

All four carried items are closed, and the carried list is now empty except for
one entry that has never reproduced.

### What actually changed

| Item | Before | After |
|---|---|---|
| tty tests when `read_key` is starved | **hang** — killed externally at 25 s | **fail in 5.00 s** with a message naming the cause |
| memory category → directory mapping | hardcoded in **three** places | derived from `MemoryCategory::ALL` everywhere |
| real-clock sleeps in tests | one 30 s `sleep` in a spawned task | none — `std::future::pending()` |
| `daemoneye reindex` in the docs | undocumented | documented in both, **and gated** |

### The pattern worth keeping: prototype, then mutate, then write the spec

Every phase in this milestone was measured before its spec was committed, and in
two of three cases the measurement changed the spec substantially.

**Phase 01.** The carried item said "`read_key` has no timeout". The obvious
reading — add one — would have been a **bug**: production awaits `read_key`
inside a `select!`, and the unbounded first-byte wait is how the chat loop waits
for the user to type. The spec therefore spent a whole section saying *do not fix
it there*. Verified by running the mutation both before (hang) and after (5 s
failure).

**Phase 02.** A working prototype of the entire refactor was built and mutation-
tested first. That is what surfaced the real risk: `dir_name()` and
`canonical_name()` differ for exactly one variant, **neither label had any test**,
and swapping them left all 1036 tests green while the output silently changed.
Without that check the phase would have shipped a refactor whose only failure mode
was invisible — and "the tests still pass" would have meant nothing. Two tests
became mandatory as a direct result.

**Phase 03.** `docs/architecture.md` already contained `daemoneye reindex` twice,
in the milestone-roadmap section the architect rewrites every close. A criterion
of the form `grep -c reindex >= 1` was **already satisfied before any work** and
would have kept passing after the durable docs were deleted. The gate reads only
the part above the roadmap heading, and the spec required the executor to prove
that by deleting the durable sentence while leaving the transient ones — which at
review still failed, correctly.

> **Prototype the change and mutate it before writing the spec.** Calibrating
> commands against the tree catches unsatisfiable criteria; only running the
> change catches criteria that would pass without the work being done.

### Where the architect was wrong

Two acceptance criteria in phase 02 were mis-formulated, both the same way:
asserted about a **future** tree state without executing against it.

- `grep -c '"incidents"'` must be 0 — but the tests the same spec made *mandatory*
  must name that directory to create it. Production is 0; the whole-file grep
  cannot express that.
- `grep -rl 'MemoryCategory::ALL'` must be 3 — but the declaration reads
  `pub const ALL:`, so only the two use sites match.

The executor met the intent of both and **reported the shortfall instead of
gaming the grep**, which it could have done trivially by building the path from a
variable. Worth recording as the behaviour the review gate depends on.

This is the M9 lesson recurring in a new form, and now generalised:

> **A criterion about the tree the phase will produce must be validated against
> that tree**, not the one in front of you. Calibration against the current tree
> is necessary and not sufficient.

### Calibration ledger

| Observation | Count | Status |
|---|---|---|
| Executor mislabels its own model in the Update Log | **3** (M9-01, M10-01, M10-03) | **resolved at close** — no template requests the line; nothing to fold, and review should stop correcting it |
| E2E transcript summarised instead of pasted | 2 (M8-02, M10-02) | trend; phase 03 pasted it, so no fold |
| Architect criterion invalidated by the phase's own work | 2 (M10-02, ×2 in one phase) | trend; watch |

The mislabel is cosmetic in effect — the server-authored tail records the correct
model and telemetry is unaffected — but the phase doc is the human-readable record
sitting next to it, and it has now been wrong three times.

**PE decision at close: drop the model line from the executor's own entry.** On
going to apply it, the premise turned out to be wrong: **no template asks for that
line.** `docs/dev/WORKFLOW.md` § "Update Log entries" defines three entry types —
progress, blocker, completion — and **none** carries an `**Executor:**` field. The
only `**Executor:** <model name>` in the file is at `:347`, in the **Review
verdict** template, which the *architect* writes and which has been correct every
time. The embedded executor contract does not ask for it either.

So the executor has been adding the line unprompted, and there is nothing to
delete. The decision's intent — remove the failure mode rather than ask the
executor to try harder — is therefore already satisfied by the templates as they
stand, and the practical consequence is for **review**: an unrequested,
self-reported model name in an executor entry is not a defect against any spec and
should not be corrected in place at review. It was corrected three times on the
assumption it was contract-mandated.

If the PE later wants it actively suppressed, that is an edit to
`executor/templates/executor_contract.md` in the **rexyMCP** repo, not this one —
out of bounds for a target-project architect session and needing its own.

### Carried forward — one item, unscheduled

1. **`hooks_land_on_private_server`** — the old phase-04-review flake. Binds no
   ports; **0 failures in 300 runs** across M8, M9 and M10. There is no evidence
   to work from. Only a bug if it recurs.

That is the whole list. Every other item carried out of M7 and M8 is closed.

### One architect action taken at close

`docs/architecture.md` § 2.3 was left with a 111-character line after phase 03
appended its sentence without reflowing the join. Flagged as a `nit` at review
rather than bounced, and reflowed here as part of the close — the paragraph now
wraps at ≤ 79 like its neighbours, and `doc_truth` still passes.


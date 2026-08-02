# NEXT

**Active phase: M10 phase-02 — derive-category-dirs** (`todo`, drafted 2026-08-02).

Doc: `docs/dev/milestones/M10-residual-hygiene/phase-02-derive-category-dirs.md`

Dispatch with `/rexymcp:dispatch phase-02`.

**M10 phase-01 is `done`** (`approved_first_try`). The tty tests now fail in 5 s
with a message naming the cause, where the same mutation used to hang until killed
at 25 s.

## Phase 02 — two carried items, and the one real risk

Items 2 and 3 together: replace `src/ai/mod.rs:364`'s 30 s real-clock sleep with
`std::future::pending()`, and derive the memory category→directory mapping from
`MemoryCategory` instead of hardcoding it.

Drafting found a **third** hardcoded copy at `src/search.rs:56-63`, so M10's exit
criterion was widened from "`epochs.rs` derives" to "every caller derives" — fixing
two of three would have left the drift in place.

**The mechanical part is not the risk.** A working prototype of the whole refactor
was built and mutation-tested before the spec was written:

| Mutation | Caught? |
|---|---|
| `dir_name()` Incident → `"WRONG"` | **Yes** — 2 tests fail |
| epochs label: `canonical_name()` → `dir_name()` | **NO — 1036 still pass** |
| search label: `dir_name()` → `canonical_name()` | **NO — 1036 still pass** |

`dir_name()` and `canonical_name()` differ for exactly one variant — `incidents`
vs `incident` — and **neither label has any test**. Swap them and the refactor
stays green while epochs silently prints `[incidents]` and search emits a
`memory/incident` label matching no directory on disk. So the spec makes two tests
mandatory and requires the executor to mutation-check both. A refactor whose only
failure mode is invisible to the suite is not verifiable.

The epochs test also needs a negative assertion, because `"[incidents]"` contains
`"[incident]"` as a substring — asserting only the positive would prove nothing.

Criteria calibrated against the tree: lib 1036 → **1038** (1039+ is scope creep,
1036–1037 means a mandatory test is missing), `from_secs(30)` 1 → 0, ai sleeps
3 → 2 (both remaining are production retry backoff), `"incidents"` literals 1 → 0
in epochs and 2 → 0 in search while staying at 2 in `memory.rs`, and
`MemoryCategory::ALL` in 0 → 3 files.

## Remaining in M10

| # | Item | Phase |
|---|---|---|
| 4 | `daemoneye reindex` undocumented in `CLAUDE.md` / `architecture.md` | 03 (not drafted) |

## The rules M7–M9 earned

> **Do not assert a fact about the system in a spec unless it was executed.**
> A *claimed failure mode* is such a fact — M9 justified a test with a
> compile-time impossibility one `cargo build` would have disproven.
>
> **An acceptance criterion for an intermittent failure must be a repeat count
> derived from a measured rate.** A single green run is not evidence.
>
> **Measure through the same door the user will use.** M9's in-process probe of
> `reconcile_index()` recorded a bare-`$HOME` result the shipped binary never
> produces.

Corollaries, each earned more than once: naming a false-success mode is worthless
unless the guard is checked against it; a phase that lands code for a *later*
phase must say how the deny-warnings gate is satisfied; and **a green bounce
always needs a refined re-dispatch**, never a plain one.

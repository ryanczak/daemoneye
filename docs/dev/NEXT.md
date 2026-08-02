# NEXT

**Active phase: M10 phase-03 — document-reindex** (`todo`, drafted 2026-08-02).
**This is M10's last in-scope phase.**

Doc: `docs/dev/milestones/M10-residual-hygiene/phase-03-document-reindex.md`

Dispatch with `/rexymcp:dispatch phase-03`.

**Phases 01 and 02 are `done`**, both `approved_first_try`. The tty tests now fail
in 5 s where they used to hang; the memory category→directory mapping is derived
from `MemoryCategory` in all three callers; and the last real-clock sleep is gone.

## Phase 03 — and why a plain grep would be a false green

`daemoneye reindex` shipped in M9 and neither `CLAUDE.md` nor
`docs/architecture.md` describes it. The phase documents it in both and adds a
tripwire so it cannot silently vanish again.

**The trap, measured:**

| Scope | `daemoneye reindex` mentions today |
|---|---|
| `CLAUDE.md`, whole file | **0** |
| `docs/architecture.md`, whole file | **2** — both transient |
| `docs/architecture.md`, before `## 5. Milestone roadmap` | **0** |

Both existing mentions sit inside `### Active milestone`, which **the architect
rewrites at every milestone close**. So `grep -c 'reindex' docs/architecture.md
>= 1` is *already satisfied before any work* and would keep passing after the
durable documentation was deleted. The gate and the criteria therefore read only
the part of the file **above** the roadmap heading.

The tripwire is the symmetric case to the existing `RETIRED_CLAIMS` table — a
`REQUIRED_CLAIMS` table for strings that must be *present*. It was compiled and
run against the current tree before the spec was committed, and **it fails today
naming both docs**, which is the proof it is not satisfied by the roadmap prose.
The spec requires the executor to confirm that by deleting its `architecture.md`
sentence while leaving the roadmap mentions in place: if the test still passes,
the gate is worthless.

Criteria calibrated: doc_truth 1 → **2** tests, `CLAUDE.md` 0 → ≥1, architecture
durable part 0 → ≥1, lib unchanged at **1038** (no lib tests added), `CLAUDE.md`
still **189** lines so the table row grows in place, and `RETIRED_CLAIMS` intact.
Calibration caught one bad criterion: `grep -c 'grep fallback' tests/doc_truth.rs`
is **2**, not 1 — the phrase appears as both the forbidden string and its
rationale.

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

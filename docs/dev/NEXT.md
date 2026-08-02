# NEXT

**Active phase: M10 phase-01 — read-key-test-bound** (`todo`, drafted 2026-08-02).

Doc: `docs/dev/milestones/M10-residual-hygiene/phase-01-read-key-test-bound.md`

Dispatch with `/rexymcp:dispatch phase-01`.

**M10 — Residual Hygiene** was scoped 2026-08-02 (PE: "make 1–4 part of M10").
It clears the four carried items M7, M8 and M9 left behind. None is a
user-visible bug; each is a way the codebase can mislead someone later.

| # | Item | Phase |
|---|---|---|
| 1 | `read_key` starvation hangs the tty tests instead of failing them | 01 (drafted) |
| 2 | Residual real-clock sleep at `src/ai/mod.rs:364` | 02 (not drafted) |
| 3 | `epochs.rs:618` hardcodes the category→directory map | 02 (not drafted) |
| 4 | `daemoneye reindex` undocumented in `CLAUDE.md` / `architecture.md` | 03 (not drafted) |

Carried item 7 (`hooks_land_on_private_server`) is **excluded**: 0 failures in
300 runs across M8 and M9, so there is no evidence to work from.

## Phase 01 — what it is, and the fix that would be wrong

Ten tty tests call `read_key(&stdin).await` directly. `read_key`'s **first**
`read_byte()` (`src/cli/input/tty.rs:164`) is unbounded — every subsequent read is
capped at 30 ms. So a regression that stops bytes reaching it makes the tests
**hang**, not fail. Verified by mutation before drafting: the mutated test was
killed externally at **25 s**. In CI a hang is worse than a failure.

**The obvious repair is a bug.** Production awaits `read_key` inside a
`tokio::select!` (`stream.rs:686`, `:711`) racing daemon messages and a tick; the
unbounded first-byte wait is exactly how the chat loop waits for the user to type.
A timeout there would return spuriously, and since `None` already means EOF, the
loop could not tell "the user is thinking" from "the terminal closed." The spec
says this in its own section, because "`read_key` has no timeout" invites precisely
the wrong fix.

So the bound goes in the tests: a `read_key_bounded` helper, all ten call sites
routed through it, and one new test proving the guard fires.

**The guard test has a trap, measured both ways:**

| Pipe write end | `timeout(50ms, read_key(&stdin))` |
|---|---|
| Held (`_write_file`) | `Err(Elapsed)` → helper panics → test passes |
| Dropped (bare `_`) | `Ok(None)` → EOF, no panic → the test passes for nothing |

Pinned as a negative case in the spec.

Criteria calibrated against the tree: 10 bare call sites → 0, lib 1035 → **1036**
(1037+ is scope creep), tty module 10 → 11, and production pinned by
`sed -n '164p'` plus a `from_millis(30)` count that must stay at 10.

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

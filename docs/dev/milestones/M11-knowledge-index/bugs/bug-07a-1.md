# Bug 1 on phase-07a: an unresolvable turn hit suppresses the whole turn line

**Severity:** minor
**Status:** open
**Filed:** 2026-08-06

## READ THIS FIRST — the gates are green and that is expected

All four gates pass, the tree is clean, and 1135 tests are green. **None of that
is evidence this bug is fixed**, because the defect is a missing fallback path,
not a broken one. Do not conclude there is no work to do.

**Already correct — do not touch:**

- `src/memory/index.rs` — `read_line_at_offset` extraction, the `EpochHit::kind`
  allow removal. Done, correct.
- `src/search.rs`, `src/daemon/context/recall.rs` — rewired call sites. Done.
- `src/daemon/mod.rs`, `src/daemon/prompt.rs`, `src/daemon/server/ask.rs`,
  `tests/integration.rs` — module registration and the `session_id` threading.
  Done, correct.
- The **epoch** lookup at `src/daemon/situational.rs:55-61`. It is already right;
  it is the model for the fix below.
- All seven existing tests. Do not rename, weaken, or delete any of them.

**There is exactly one edit, in one function, plus one new test.**

## What's wrong

`src/daemon/situational.rs:41-44`:

```rust
    let turn_result = turn_hits
        .iter()
        .find(|hit| current_session.is_none_or(|cs| hit.session_id != cs))
        .and_then(resolve_turn_hit);
```

`find` selects the **first** hit from another session and `and_then` then tries
to resolve it. If that one hit fails to resolve — its archive line is empty, does
not deserialize, or renders to an empty excerpt — `resolve_turn_hit` returns
`None` and the whole turn line is dropped. The remaining seven candidates
`search_turns(user_turn, 8, None)` fetched are never examined.

The epoch path immediately below does this correctly: its predicate
(`… && !hit.body.is_empty()`) lives *inside* `find`, so an unusable hit is
skipped and the next one is considered.

## What should happen

The phase spec, § Spec task 2, step 2, is explicit:

> Skip a hit whose line is empty, fails to deserialize, or renders to an empty
> excerpt, and try the next one.

A turn whose index row survives but whose archive line cannot be read (a
truncated archive, a stale offset) must not suppress an otherwise-good later
hit. This is why the limit is 8 and not 1.

Note the executor's completion summary recorded "Deviations from spec: None".
This is one; the finding is the deviation, not the summary.

## How to fix

In `src/daemon/situational.rs`, replace the `find(...).and_then(...)` pair with a
filter over the exclusion plus a `find_map` over the resolution:

```rust
    let turn_result = turn_hits
        .iter()
        .filter(|hit| current_session.is_none_or(|cs| hit.session_id != cs))
        .find_map(|hit| resolve_turn_hit(hit));
```

Write the closure form `|hit| resolve_turn_hit(hit)` rather than passing
`resolve_turn_hit` bare: after `.filter()` the iterator item is `&&TurnHit`, and
a bare function item will not coerce there, though a closure argument will.

`resolve_turn_hit` itself needs no change — it already returns `None` on each of
the three skip conditions.

**Add one test** to the module's `mod tests`, named
`unresolvable_turn_hit_falls_through_to_the_next`: seed **two** turns in session
`other`. The first is the stronger BM25 match (repeat the distinctive phrase
several times in the indexed body) but is indexed at an offset past the end of
its archive file, so it cannot resolve. The second is a weaker match that
resolves normally. Assert the returned block contains the **second** turn's
number and its excerpt text. Before the fix this test fails because the block has
no turn line at all; assert on the presence of the second hit rather than merely
on `is_some()`, so an unrelated epoch line cannot satisfy it.

## Verification

- [ ] `cargo test --lib daemon::situational` reports **8 passed**, 0 failed.
- [ ] `cargo test --lib` reports **1136** passed, 0 failed — 1135 plus exactly
      one new test. A total above 1136 means tests were added that were not
      asked for; below means one was lost.
- [ ] `grep -n "find_map" src/daemon/situational.rs` finds the new call.
- [ ] `grep -n "and_then(resolve_turn_hit)" src/daemon/situational.rs` finds
      nothing (exit 1).
- [ ] `cargo fmt --all`, `cargo build`, and
      `cargo clippy --all-targets --all-features -- -D warnings` are clean.
- [ ] **Mutation, captured mechanically into this round's own Update Log entry**
      (an entry from the first dispatch does not carry forward): revert the fix
      to `.find(...).and_then(resolve_turn_hit)`, run
      `cargo test --lib daemon::situational`, and show
      `unresolvable_turn_hit_falls_through_to_the_next` failing. Then restore and
      show it passing, plus the two greps above as restore proof.

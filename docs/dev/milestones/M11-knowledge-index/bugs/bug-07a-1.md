# Bug 1 on phase-07a: an unresolvable turn hit suppresses the whole turn line

**Severity:** minor
**Status:** fixed
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

---

## Resolution — 2026-08-06 (architect takeover)

Fixed. `src/daemon/situational.rs:43-46` now reads
`.filter(exclusion).find_map(resolve_turn_hit)`, and
`unresolvable_turn_hit_falls_through_to_the_next` covers it.

**Correction to "How to fix" above.** This doc told the executor to write
`|hit| resolve_turn_hit(hit)` because a bare function item "will not coerce"
after `.filter()`. That is wrong — clippy rejects the closure as
`redundant_closure` under `-D warnings`, and the bare `resolve_turn_hit`
compiles. Shipped form is the bare function.

**The test the executor first wrote was vacuous, and the reason was this doc's
prescription.** It said to make the unresolvable hit rank first "by repeating
the phrase several times". BM25 normalizes by document length, so the *longer*
repeated body ranks **below** the shorter exact one — `find` picked the
resolvable hit and the fallback never ran. The test passed with and without the
fix. The executor then spent ~45 consecutive turns re-running that one test
trying to make the mutation fail, and stalled. The fixture now inverts the
lengths (short exact = unresolvable, long padded = resolvable), measured first:

```
$ sqlite3 :memory: "CREATE VIRTUAL TABLE t USING fts5(body, tokenize='porter unicode61 remove_diacritics 2');
INSERT INTO t(rowid, body) VALUES(100, 'unresolvable subsystem failure');
INSERT INTO t(rowid, body) VALUES(200, 'unresolvable subsystem failure plus a great deal of additional filler text that makes this document substantially longer than the other one so bm25 length normalization penalises it');
SELECT rowid, bm25(t) FROM t WHERE t MATCH '\"unresolvable\" OR \"subsystem\" OR \"failure\"' ORDER BY bm25(t);"
100|-4.4594594594594593e-06
200|-2.26027397260274e-06
```

and the test now **asserts that ranking as a precondition**, so if BM25 ordering
ever changes it fails loudly instead of passing vacuously.

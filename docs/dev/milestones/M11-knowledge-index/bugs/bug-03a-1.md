# Bug 1 on phase-03a: appended turn is indexed at offset 0 when the archive was seeded

**Severity:** blocker
**Status:** resolved 2026-08-05 — Findings 1 and 3 fixed; **Finding 2 withdrawn (architect spec_bug)**
**Filed:** 2026-08-05

## What's wrong

### Finding 1 — the seeded-append offset is hardcoded to 0 (production defect)

`src/daemon/session.rs:306`:

```rust
let offset = if seeded {
    Some(0)
} else {
    std::fs::metadata(&archive_path)
        .ok()
        .map(|m| m.len())
        .or(Some(0))
};
```

When the seed branch runs, the offset recorded for the **newly appended** line is
`0` — which is the offset of the *first seeded line*, not the appended one. The
spec was explicit (§ 3): the offset "is the archive file's length immediately
before the append — and that means *after* the seeding copy, not before it," and
"Compute it **after** any seed and **before** opening the file for append." The
seed branch skips that computation entirely.

Consequence: the appended turn's `turns_map.offset` points at unrelated content.
Since `turns` is a contentless FTS5 table whose excerpts are re-read from the
JSONL at the stored offset, every read surface built on it in phases 04–05 will
render the wrong line for that row. It also duplicates offset 0, which already
has a row from the seed scan.

Reproduced mechanically — a probe test seeding three messages then appending a
fourth (temporarily added to the `memory::index` test module, run, then reverted):

```
PROBE rows = [(1, 0), (2, 50), (3, 106), (4, 0)]
PROBE turn=1 offset=0   -> {"role":"user","content":"first seeded","turn":1}
PROBE turn=2 offset=50  -> {"role":"assistant","content":"second seeded","turn":2}
PROBE turn=3 offset=106 -> {"role":"user","content":"third seeded","turn":3}
PROBE turn=4 offset=0   -> {"role":"user","content":"first seeded","turn":1}
assertion failed: PROBE: offsets must be distinct, got [(1, 0), (2, 50), (3, 106), (4, 0)]
```

Turn 4 seeks to turn 1's line. This directly fails the acceptance criterion
"**The seeding case is covered** … and each offset seeks to **its own** line."

### Finding 2 — `.or(Some(0))` masks a metadata failure as offset 0

Same expression, `src/daemon/session.rs:312`. The spec said to default to
**skipping the index write** if `metadata` fails ("defaulting to skipping the
index write if it fails — never unwrap"). `.or(Some(0))` instead records a
knowingly-wrong offset of 0. Same failure mode as Finding 1: a row that seeks to
the wrong line is worse than an absent row, because the absent row is repaired by
the next reconcile while the wrong row survives it.

### Finding 3 — the test for the seed case asserts a property every line satisfies

`src/memory/index.rs:2428`, inside `archive_seed_indexes_every_copied_line`:

```rust
assert!(
    line.contains("turn"),
    "offset {offset} for turn {turn} should point to a valid line, got: {line}"
);
```

`"turn"` is a JSON key present in **every** record in the file, so this assertion
holds for any offset that lands on any line — including the wrong one. That is
why Finding 1 shipped with a green suite. Per `STANDARDS.md` §3.1 and the review
gate on test realism, an assertion that cannot fail when the code under test is
broken is not coverage.

The phase doc flagged this exact test as "**the test most likely to be skipped and
the one that matters most**"; it was written, but its check was vacuous.

## What should happen

- On the seed path, the appended line's offset is the archive file's length
  **after** the `fs::copy` and **before** the append — the same computation the
  non-seed path performs. There should be no `if seeded` special case for the
  offset at all; compute the length once, after any seed.
- A `metadata` failure skips the `index_turn` call rather than substituting `0`.
- The seed test asserts each row's offset seeks to *that row's own* line, by
  content, and that the four offsets are distinct.

## How to fix

1. `src/daemon/session.rs:306–313` — replace the whole `if seeded { Some(0) } else
   { … .or(Some(0)) }` expression with a single unconditional length read placed
   after the seed block and before the append:

   ```rust
   let offset = std::fs::metadata(&archive_path).ok().map(|m| m.len());
   ```

   Keep the existing `if let Some(off) = offset` guard at the call site, which
   already skips the index write when the offset is unavailable. The `seeded`
   binding is then only needed for the seed-indexing branch above it.

2. `src/memory/index.rs`, `archive_seed_indexes_every_copied_line` — strengthen
   the per-row assertion. Match each turn to its own distinctive content
   (`"first seeded"` / `"second seeded"` / `"third seeded"` / `"appended
   fourth"`), and assert the set of offsets has no duplicates. The current
   `line.contains("turn")` must go — it passes against the broken code.

3. Add the distinct-offset assertion to `appended_turn_offset_seeks_to_its_line`
   too, or extend that test to cover the seeded case, so the regression is pinned
   in both places the spec names.

Note that `incremental_and_reconcile_agree` will not catch this on its own: the
row *counts* match a rebuild (4 either way): only the offsets differ. Do not
treat that test passing as evidence the fix landed.

## Verification

- [ ] `archive_seed_indexes_every_copied_line` asserts per-row content and offset
      distinctness, and **fails** if `let offset = if seeded { Some(0) }` is
      restored — check this by reintroducing the old expression, watching the test
      go red, then reverting.
- [ ] A seeded three-message archive plus one append yields four `turns_map` rows
      with four distinct offsets, each seeking to its own line.
- [ ] `.or(Some(0))` no longer appears in `append_archive_message`.
- [ ] `cargo fmt --all`, `cargo build`, `cargo clippy --all-targets
      --all-features -- -D warnings`, `cargo test` all clean.

---

## Resolution — 2026-08-05

**Finding 1 (seeded-append offset): fixed.** `append_archive_message` computes the
length unconditionally after the seed block. Verified by mutation at review —
restoring the `if seeded { Some(0) }` form makes
`archive_seed_indexes_every_copied_line` fail on the distinct-offset assertion:

```
assertion `left == right` failed: all offsets must be distinct,
got [(1, 0), (3, 0), (1, 50), (2, 106)]
```

**Finding 3 (vacuous test assertion): fixed.** The test now orders rows by
`offset ASC` and zips against `["first seeded", "second seeded", "third seeded",
"appended fourth"]`, keyed by file position rather than by `turn` — necessary
because the fixture writes two rows at `turn: 1`. The distinct-offset assertion
is present.

**Finding 2 (`.or(Some(0))`): WITHDRAWN — this finding was wrong.**

The bug doc asserted that `.or(Some(0))` "records a knowingly-wrong offset" and
instructed removing it, prescribing
`let offset = std::fs::metadata(&archive_path).ok().map(|m| m.len());`. Applying
that instruction verbatim at review **breaks three tests**:

```
test memory::index::tests::append_archive_message_indexes_the_turn ... FAILED
test memory::index::tests::appended_turn_offset_seeks_to_its_line ... FAILED
test memory::index::tests::incremental_and_reconcile_agree ... FAILED
  assertion failed: turns count must agree: incremental=0 reconcile=1
```

The dominant reason `metadata` fails here is that **the archive file does not
exist yet** — the common fresh-session append, where `metadata` legitimately
errors and offset `0` is the *correct* answer, because the line is about to be
written at byte 0. Without the fallback the first message of every new archive is
never indexed. The executor re-added `.or(Some(0))` on the resume run and was
right to; that was a correct restoration, not a regression.

The residual case the finding was actually reaching for — `metadata` failing on an
archive that *does* exist — is a bare IO error that would almost certainly fail the
immediately following append too, and STANDARDS §2.2 ("no error handling for cases
that can't happen") argues against branching on it. No further change.

**Classified `spec_bug`** against the architect, per the M7–M10 rule that a spec
must not assert a system fact that was not executed. Finding 2 claimed a failure
mode from reasoning about `.or(Some(0))` in isolation, without running the removal
it prescribed. One `cargo test` would have disproven it.

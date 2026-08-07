# Bug 1 on phase-07b: the signal guard is untested, and the mutation evidence overstates what failed

**Severity:** major
**Status:** open
**Filed:** 2026-08-07

## READ THIS FIRST — the gates are green and that is expected

`cargo fmt --all`, `cargo build`, `cargo clippy --all-targets --all-features --
-D warnings` and `cargo test` were all re-run independently at review and are
**all clean**; `cargo test --lib` reports **1147 passed, 0 failed**. The working
tree is clean. **None of that is evidence this phase is done**, and finding
nothing broken is not a reason to report `complete` with an empty diff.

All eight acceptance criteria were verified at review and **pass**. The shipped
production code is correct and is **not** what this bug is about.

**Already correct — do not touch:**

- `fts5_search_in_category` in `src/memory/index.rs:216` — the category clause,
  the parameter ordinals, and the `fts5_search` wrapper at `:292`. Verified
  load-bearing by mutation at review (below).
- `inject_yaml_relates_to` in `src/header.rs:339` and its four tests.
- `similar_incidents` / `add_memory` auto-linking in
  `src/daemon/executor/knowledge/memory.rs:12-60`.
- `assemble_incident_context` in `src/daemon/situational.rs:144` — its body,
  its hardcoded `&["global"]` namespace, and the `ghost.rs:199` call site. The
  namespace is what the phase spec's Task 4 prescribed verbatim; it is not a
  deviation.
- All 11 new tests except the **one** named in Finding 2.

**There are exactly two edits — one test fixture and one doc comment — plus one
Update Log entry.** No production logic changes. No new tests.

## What's wrong

### Finding 1 (major) — the required end-to-end verification entry is absent, and the mutation result recorded in its place is wrong

The phase doc's § End-to-end verification requires the block to be run verbatim
and pasted into an Update Log entry titled
`### Update — <date> (end-to-end verification)`, and states in bold: **"The
server-authored `(complete)` entry does not satisfy this."**

The Update Log contains only two entries — a `(started)` note and the
server-authored `(complete)` block. The required entry does not exist.

The mutation result was instead reported inside the excluded `(complete)` entry,
and it is **overstated**. It claims:

> Mutation proved the category filter is load-bearing: disabling it caused 3
> tests to fail (`category_filter_excludes_other_categories`,
> `adding_an_incident_links_prior_incidents`,
> `incident_context_includes_a_matching_prior_incident`)

Re-run independently at review, applying exactly the mutation the phase doc
prescribes (`cat_clause` built as though `category` were always `None`, and the
parameter push suppressed):

```
test result: FAILED. 1146 passed; 1 failed
failures:
    memory::index::tests::category_filter_excludes_other_categories
```

**One test fails, not three.** `adding_an_incident_links_prior_incidents` and
`incident_context_includes_a_matching_prior_incident` **pass** under the
mutation, because every memory their fixtures seed is already in the `incident`
category — the filter is a no-op for them, so they cannot detect its removal.

The phase's own bar ("the mutated run must show at least one failing test") *is*
met, and the filter *is* genuinely tested by
`category_filter_excludes_other_categories`. The defect is the **evidence
claim**, not the code.

### Finding 2 (major) — `incident_context_is_none_for_a_low_signal_alert` proves nothing

`src/daemon/situational.rs:557-575`. The test seeds an incident memory reading
`"The database connection pool exhausted during peak load"`, queries
`assemble_incident_context("go no")`, and asserts `None`. Its own comment states
the intent:

```rust
// Seed a non-empty matching corpus so the test is about the guard, not
// about an empty index.
```

Verified at review by deleting the guard outright:

```rust
pub fn assemble_incident_context(alert_msg: &str) -> Option<String> {
    // if !has_sufficient_signal(alert_msg) { return None; }   <- deleted
```

```
cargo test --lib daemon::situational
test result: ok. 12 passed; 0 failed
```

**All 12 situational tests stay green with the guard gone**, including this one.
The guard added by Task 4 has no test at all.

### Finding 3 (minor) — the rustdoc link on `fts5_search_in_category` points at itself

`src/memory/index.rs:212` reads:

```rust
/// As [`fts5_search_in_category`], but restricted to one memory category when
```

The phase doc's Task 1 quoted this comment as ``/// As [`fts5_search`], but
restricted to one memory category when ...``. The intra-doc link was transcribed
onto the item it documents, so it resolves to itself and tells a reader nothing.

## What should happen

- The E2E block runs verbatim and its output lands in its own
  `(end-to-end verification)` Update Log entry, per the phase doc's § End-to-end
  verification and `WORKFLOW.md` § "End-to-end verification" (the entry is
  required **per dispatch**). The tests named as failing under the mutation are
  the tests that actually failed.
- The low-signal guard is covered by a test that fails when the guard is
  removed, per `STANDARDS.md` §3 and the phase doc's own instruction that a
  filter matching nothing "would also pass".
- The doc comment links to the function it is contrasted with.

## Root cause

**Finding 2** — the fixture's premise is false. `has_sufficient_signal` requires
`MIN_QUERY_TERMS` (3) distinct terms of `MIN_TERM_LEN` (4) or more characters
(`src/daemon/situational.rs:9-25`). The query `"go no"` has two terms, both
below the length floor — so it is low-signal *and* shares no term with the
seeded body. `build_match_expr` turns it into `"go" OR "no"`, which matches the
seeded document zero times. The search therefore returns empty and
`assemble_incident_context` returns `None` **via the no-hits path**, never
reaching the guard. Seeding a non-empty corpus is not sufficient; the corpus has
to be one the low-signal query would otherwise *match*.

This is the third occurrence of the vacuous-guard family in M11 (03a's
`line.contains("turn")`, 05b's `"all"`-excludes-turns test), which is the fold
threshold.

**Finding 1** — a self-reported mutation check is not a check. The names were
recorded from expectation rather than from the failure output; two of the three
fixtures cannot observe the mutation at all. This is the fabricated-evidence-
under-green-gates shape: correct code and passing gates make the claim look
verified.

**Finding 3** — transcription drift from the spec's quoted block.

## Definition of done

- [ ] `cargo test --lib` reports **1147 passed, 0 failed** — **1147, not 1148**.
      Both edits change existing lines; neither adds a test. A rising count means
      scope creep.
- [ ] `incident_context_is_none_for_a_low_signal_alert` **fails** when the
      `has_sufficient_signal` early-return in `assemble_incident_context` is
      deleted, and **passes** when it is restored. Paste both runs.
- [ ] `grep -n 'As \[`fts5_search`\]' src/memory/index.rs` matches at the
      `fts5_search_in_category` doc comment.
- [ ] An Update Log entry titled `### Update — <date> (end-to-end verification)`
      exists and carries the verbatim output of the phase doc's E2E block,
      including a mutation section naming **only** the tests that actually
      failed.
- [ ] `cargo fmt --all`, `cargo build`, and
      `cargo clippy --all-targets --all-features -- -D warnings` stay clean.

### Worked fixture for Finding 2 — executed at review, both directions

This was run against the current tree before being written here, per
`WORKFLOW.md` § "State the symptom, the root cause and the DoD — not the fix".
It is one line: change **only** the query in
`incident_context_is_none_for_a_low_signal_alert`, keeping the existing seed.

```rust
        let result = assemble_incident_context("database pool");
```

`"database pool"` has two terms of ≥ 4 characters — below the floor of 3, so
still low-signal — and both occur in the seeded body, so the search *would*
return a hit if the guard were not there. Measured:

```
guard present:  test ...incident_context_is_none_for_a_low_signal_alert ... ok
guard deleted:  test ...incident_context_is_none_for_a_low_signal_alert ... FAILED
                low-signal alert must return None even with a non-empty index
```

Consider strengthening the assertion message or adding a comment recording why
this specific query is the right one; the query itself is the load-bearing part.

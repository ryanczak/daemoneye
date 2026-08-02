# Phase 09: Index Doc Correction

**Milestone:** M7 — Memory Search & Maintenance
**Status:** todo
**Depends on:** phase-08 (fts5-search, done) — the phase that made these docs
wrong.
**Estimated diff:** ~90 lines — five prose sites across two docs, plus one new
integration test (~70 lines).

**Tags:** language=rust, kind=docs, size=m

## Goal

Phase 08 made memory search real. `docs/architecture.md` and `CLAUDE.md` still
describe a stub, and both carry three further claims that were **never** true.
Correct them, and land a tripwire so these specific claims cannot come back.

This is M7's last in-scope phase and its third exit criterion.

## Architecture references

- `docs/architecture.md` § 1.4 (persistence bullet), § 2.3 (knowledge-flow
  paragraph), § 3 (knowledge-system bullet), § 5 (active-milestone block) — the
  four sites.
- `CLAUDE.md` § "Key files", the `src/memory/index.rs` row.
- `tests/bug_tracker.rs` — the repo-hygiene gate idiom this phase copies for its
  own gate. It is an integration test that reads the checked-in doc tree and
  locates the repo with `env!("CARGO_MANIFEST_DIR")` (`tests/bug_tracker.rs:272`).

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any file.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

**Every claim below was checked against the code before this spec was written.**
Three of the five sites are wrong in a way phase 08 caused; two were wrong all
along.

### What is actually true now

- `src/memory/index.rs` holds a real SQLite FTS5 index at
  `var/index/memory.db`: `open_index()`, `ensure_schema()` (with
  `SCHEMA_VERSION`; a mismatch drops and recreates, because the index is
  derived), `index_memory_file()` / `remove_from_index()` called best-effort
  from the `src/memory.rs` mutators, `reconcile_index()`, and `fts5_search()`
  returning BM25-ranked `(namespace, key, score)` best-first.
- `build_match_expr()` quotes each user term and joins with `OR`. This is
  load-bearing: the caller passes an entire user turn, and a whole-query phrase
  match returns **zero** rows for realistic input.
- `reconcile_index()` runs automatically when the index is empty, which is what
  indexes the memories a fresh install seeds (measured: **9** rows).

### The five sites

**Site 1 — `docs/architecture.md:139`**, § 1.4 persistence bullet:

> `memory/` (CRUD + FTS5 index with grep fallback)

**Site 2 — `docs/architecture.md:189`**, § 2.3:

> Memory is indexed in an FTS5 SQLite db with a grep fallback for search.

**There is no grep fallback, and there never was one.** Verified: `grep -c
"crate::search" src/daemon/memory_prompt.rs` returns **0**. Memory recall is a
three-way merge — tag overlap, one-hop `relates_to` expansion, and FTS5 hits
(`src/daemon/memory_prompt.rs:66-76`). The grep scan in `src/search.rs` backs the
separate `search_repository` **tool** (its only caller is
`src/daemon/executor/knowledge/memory.rs:235`); it is not a fallback path for
recall.

**Site 3 — `docs/architecture.md:302`**, § 3 knowledge-system bullet:

> persistent memory (with FTS5 index, G2 schema, and per-agent namespacing)

**The "G2 schema" does not exist.** Verified by grepping `src/` for each field
the term refers to — `volatility`, `usefulness_score`, `last_verified`,
`verified_by` — **all four return no files**. Phase 10 removed the same false
claim from `CLAUDE.md`; this is the surviving copy.

**Site 4 — `docs/architecture.md:399-412`**, § 5 active-milestone block. Three
factual errors plus the stub note:

- "nine phases named, none drafted" — there are **ten**, nine of them `done`.
- "four test sleeps that predate STANDARDS 3.3" — it is **three**; the M7 README
  records the correction ("corrected from *four* during phase-03 drafting").
- "The FTS5 note below … is accurate today and M7 exists to make it stale" — M7
  did exactly that, so the sentence and the note it introduces are both spent.
- The note itself ("the FTS5 memory index … is currently a **stub**") is now
  simply false.

**Site 5 — `CLAUDE.md:72`**, the `src/memory/index.rs` row:

> **Stub.** `fts5_search()` returns an empty `Vec` and the BM25 scoring is not
> yet wired. … Real memory search is the grep scan in `src/search.rs`. …

## Spec

### 1. `docs/architecture.md` § 1.4 — site 1

Replace the parenthetical so it names the real artifact and drops the fallback:

```
  `session_store.rs` (named-session persistence), `memory/` (CRUD + a SQLite
  FTS5 index at `var/index/memory.db`), `config.rs` (`~/.daemoneye/config.toml`),
```

### 2. `docs/architecture.md` § 2.3 — site 2

Replace the one-sentence claim with what the code does:

> Memory is indexed in a SQLite FTS5 database at `var/index/memory.db`,
> maintained best-effort on every add/update/delete and rebuilt by
> `reconcile_index()` whenever the index is found empty. Recall merges three
> candidate sources — tag overlap, one-hop `relates_to` expansion, and
> BM25-ranked FTS5 hits against the user's turn. The grep scan in
> `src/search.rs` serves the `search_repository` tool and is **not** a fallback
> for recall.

### 3. `docs/architecture.md` § 3 — site 3

Drop the non-existent schema from the bullet:

```
- **Knowledge system** — runbooks, persistent memory (with a BM25-ranked FTS5
  index and per-agent namespacing), repository search, named agents with tool
  policies and persistent briefings.
```

### 4. `docs/architecture.md` § 5 — site 4

Fix the two counts in the milestone paragraph: **ten** phases rather than nine,
"none drafted" → "nine `done`, one remaining", and **three** test sleeps rather
than four. Then **delete both** the sentence beginning "The FTS5 note below is
the milestone's headline item" **and** the entire final paragraph beginning "One
correction recorded during M4 scoping". That note existed to be deleted by this
phase.

Leave the rest of the § 5 narrative alone. **Do not rewrite the block into a
retrospective** — milestone close is a separate, human-gated step and it owns
that rewrite.

### 5. `CLAUDE.md` — site 5

Replace the `src/memory/index.rs` row. Keep it one line; the table requires it.

> `src/memory/index.rs` | SQLite FTS5 memory index at `var/index/memory.db`.
> `open_index()` / `ensure_schema()` create it — a `SCHEMA_VERSION` mismatch
> drops and recreates, since the index is derived and a rebuild is always safe.
> `index_memory_file()` / `remove_from_index()` are called **best-effort** from
> the `src/memory.rs` mutators: an index failure logs a warning and never fails
> the caller. `reconcile_index()` rebuilds from the files on disk and runs
> automatically when the index is empty, which is what indexes the memories a
> fresh install seeds. `fts5_search()` returns BM25-ranked
> `(namespace, key, score)`, best first; `build_match_expr()` quotes each user
> term and joins with `OR`, because the caller passes a whole user turn and a
> phrase match would return nothing. The grep scan in `src/search.rs` backs the
> `search_repository` tool, not memory recall.

Change no other row.

### 6. The tripwire — `tests/doc_truth.rs`

Create `tests/doc_truth.rs`, following `tests/bug_tracker.rs`'s shape: an
integration test that reads the checked-in docs and locates the repo root with
`env!("CARGO_MANIFEST_DIR")`.

One test, named `docs_do_not_carry_retired_index_claims`, driven by a table so a
failure names the offending phrase:

```rust
//! Repo-hygiene gate: fail when a doc reintroduces a claim about the memory
//! index that stopped being true when FTS5 search landed.

use std::path::Path;

/// (doc path relative to the repo root, forbidden substring, why it is wrong)
const RETIRED_CLAIMS: &[(&str, &str, &str)] = &[
    ("docs/architecture.md", "grep fallback",
     "there is no grep fallback for recall; src/search.rs backs search_repository"),
    ("docs/architecture.md", "currently a **stub**",
     "src/memory/index.rs is a real FTS5 index"),
    ("CLAUDE.md", "grep scan in `src/search.rs`. Un-stubbing",
     "the index is no longer a stub"),
    ("CLAUDE.md", "returns an empty `Vec`",
     "fts5_search returns BM25-ranked hits"),
];

#[test]
fn docs_do_not_carry_retired_index_claims() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut findings = Vec::new();
    for (doc, phrase, why) in RETIRED_CLAIMS {
        let path = root.join(doc);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        if text.contains(phrase) {
            findings.push(format!("{doc}: retired claim {phrase:?} — {why}"));
        }
    }
    assert!(
        findings.is_empty(),
        "retired index claims are back in the docs:\n{}",
        findings.join("\n")
    );
}
```

**Be honest about what this gate is.** It is a tripwire for these four specific
retired phrases, in the same spirit as the path-audit gate's stale-path list. It
does **not** detect new stale claims of other kinds, and the spec does not
pretend otherwise. Its value is that the exact sentences M7 spent a milestone
removing cannot quietly return.

Every entry's phrase must be present in the doc **before** this phase's edits and
absent after — that is what makes the table meaningful rather than decorative.
Measured before drafting: `grep -c` returns 2, 1, 1, 1 respectively.

## Acceptance criteria

- [ ] All five sites are corrected as specified.
- [ ] `grep -c "grep fallback" docs/architecture.md` returns **0**.
- [ ] `grep -c "currently a \*\*stub\*\*" docs/architecture.md` returns **0**.
- [ ] `grep -c "Un-stubbing" CLAUDE.md` returns **0**.
- [ ] `grep -c "G2 schema" docs/architecture.md` returns **0**.
- [ ] `docs/architecture.md` § 5 says ten phases with nine `done`, and **three**
      test sleeps; the "One correction recorded during M4 scoping" paragraph is
      gone; the rest of the § 5 narrative is otherwise unchanged.
- [ ] `docs_do_not_carry_retired_index_claims` passes, and **fails if any one
      retired phrase is reintroduced** — demonstrate by reinserting one, quoting
      the red run, then reverting.
- [ ] No `.rs` file under `src/` changes. This phase touches docs plus one new
      test file only.
- [ ] `cargo build` zero new warnings; `cargo clippy --all-targets
      --all-features -- -D warnings` exits 0; `cargo fmt --all` leaves the tree
      unchanged.
- [ ] `cargo test` passes: lib **1032** (unchanged — this phase adds no lib
      tests), integration **30** (2 ignored), isolation **8** (1 ignored),
      `bug_tracker` **6**, and a new `doc_truth` binary reporting **1**.

## Test plan

Spec task 6 is the test. The load-bearing demonstration is the **reinsertion red
run** required by the acceptance criteria: put one retired phrase back, watch
`docs_do_not_carry_retired_index_claims` fail and name it, revert.

**What would make this phase a false success:** correcting the prose and adding a
gate whose table lists phrases that are *already* absent — it would pass forever
while guarding nothing. That is why the spec records the pre-edit `grep -c`
counts (2, 1, 1, 1) and why the reinsertion run is required rather than optional.

A second one: rewriting § 5 into a milestone retrospective. That would look like
thoroughness and would collide with the human-gated close step, which owns that
text.

## End-to-end verification

Run this block verbatim and paste the resulting file into your Update Log.

**Two constraints carried from phase-03's post-mortem:** **no heredocs**, and
every tree-walking command wrapped in `timeout`. A phase-03 E2E block nested a
`python3` heredoc that hung and orphaned two processes at 100% CPU for 70
minutes. Do not reintroduce either pattern.

```bash
cd /home/matt/src/daemoneye
{
  echo "=== retired claims are gone ==="
  timeout 30 grep -c "grep fallback" docs/architecture.md
  echo "grep-fallback-count-above-must-be-0"
  timeout 30 grep -c "currently a \*\*stub\*\*" docs/architecture.md
  echo "stub-note-count-above-must-be-0"
  timeout 30 grep -c "Un-stubbing" CLAUDE.md
  echo "unstubbing-count-above-must-be-0"
  timeout 30 grep -c "G2 schema" docs/architecture.md
  echo "g2-schema-count-above-must-be-0"

  echo "=== the corrected claims are present ==="
  timeout 30 grep -c "var/index/memory.db" docs/architecture.md
  echo "archdoc-mentions-the-db-above-must-be-at-least-1"
  timeout 30 grep -c "BM25-ranked" CLAUDE.md
  echo "claudemd-mentions-bm25-above-must-be-at-least-1"

  echo "=== no src/ changes ==="
  timeout 30 git diff --name-only HEAD -- src/ | wc -l
  echo "src-files-changed-above-must-be-0"

  echo "=== the gate ==="
  timeout 300 cargo test --test doc_truth 2>&1 | grep -E "^test |^test result"

  echo "=== full gate ==="
  timeout 900 cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -2
  echo "clippy-exit=$?"
  timeout 900 cargo test 2>&1 | grep -E "^test result"
} > /tmp/phase09-e2e.txt 2>&1
cat /tmp/phase09-e2e.txt
```

Paste the captured file into an Update Log entry titled
`### Update — <date> (end-to-end verification)`, **and separately** paste the
reinsertion red run the acceptance criteria require. **The server-authored
`(complete)` entry does not satisfy either** — its "Command output tails" block
is the automatic gate capture every phase receives, and it shows that
build/lint/test ran, not that this phase's acceptance criteria were exercised.

**If any part of the capture block fails or hangs, stop and report it as a
blocker.** Do not re-run the surviving sections separately and paste the
result — a transcript assembled from more than one run fails `STANDARDS.md` §1
even when every claim in it is true.

## Authorizations

- [ ] May add dependencies: **none**.
- [ ] May touch `docs/architecture.md`: **yes** — sites 1–4, and only those.
- [ ] May touch `CLAUDE.md`: **yes** — the `src/memory/index.rs` row only.
- [ ] May create new files: **yes, exactly one** — `tests/doc_truth.rs`.

## Out of scope

- **Any file under `src/`.** An acceptance criterion pins this. The code is
  correct; this phase describes it.
- **Rewriting § 5's active-milestone narrative into a retrospective**, ticking
  the M7 README's exit-criteria checkboxes, or setting `NEXT.md` to "none". All
  three belong to the human-gated milestone close, which happens after this
  phase is approved.
- **The `src/memory.rs` row in `CLAUDE.md`.** Phase 10 rewrote it and it is
  accurate.
- **Widening the tripwire into a general doc-drift detector.** The table covers
  four named phrases; making it general is a different, larger design.
- **`docs/architecture.md` § 1.4's other bullets**, § 2.4, or any section not
  listed as a site.
- **The M4 design doc referenced by the deleted note.** Leave it; this phase does
  not chase citations out of scope.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

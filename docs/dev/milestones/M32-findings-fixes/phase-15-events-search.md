# Phase 15 — Events search

**Status:** todo
**Milestone:** M32 — Findings fixes
**Drafted:** 2026-10-30
**Depends on:** phase-14
**Estimated diff:** ~131 insertions, ~41 deletions
**Tags:** language=rust, kind=bugfix, size=s
**Findings:** M32 `FINDINGS.md` #16, #17; the rest of M31 `FINDINGS.md` #48

## Goal

Two defects in `search_repository`'s events search, added by the PE on
2026-10-30. After this phase:

- **M32 #16 (bug):** the events search reads its FTS index.
  `search_events` in `src/memory/index/search.rs` selected `e.event` from
  a contentless table. That column reads back NULL, so the `String`
  decode failed on every row and `filter_map` dropped them all. The
  search has worked only through its tail scan, which reads the last
  `EVENTS_TAIL_LINES` lines. Now it selects only `events_map` columns,
  and `EventHit` loses its two dead fields with their
  `#[allow(dead_code)]`.
- **What the events search returns.** It searches the whole history.
  The index is queried first, with the query's words as one FTS5 phrase
  from a new `build_phrase_expr`. The OR form `build_match_expr` builds
  would return every line holding `idx` or `0` for the query `idx=0`.
  The tail scan then adds matches the index did not return. Both paths
  keep only lines that contain the query text, as the scan always has.
  Both name a hit by its segment file, so a line both find is returned
  once.
- **M32 #17 (bug):** every events result is masked with
  `mask_sensitive`. The index masks the body it stores, but both paths
  returned the unmasked line from disk, so a key in a logged command
  reached the model.

The suite goes from **2611** to **exactly 2612**. One test is renamed and
rewritten, one is added, and one is strengthened.

## Architecture references

- `src/memory/index/mod.rs` § the `events` FTS5 table (`content=''`) and
  `build_match_expr`.
- `src/search.rs` § `search_events_fts`, `search_events_in_segments`.
- M32 README, decisions 1 and 2.

## Spec

1. **Run this, once, as your first command after the started entry:**

   ```sh
   python3 docs/dev/milestones/M32-findings-fixes/scripts/phase-15-events-search.py && cargo fmt --all
   ```

   **Do not open the script, and do not read any source file before
   running it.** It was run on a scratch worktree of `5a330f6e`: fmt and
   clippy clean, hygiene passing, the full suite at
   `passed=2612 failed=0`. It asserts its own anchors and prints four
   lines, the last `the events tests pin the whole history, one copy and
   masking`. If an assert fires, stop and report it in the Update Log.
   Do not hand-edit anything it touches, and do not edit the script.
2. **Log it at once.** Append an `### Update — <date> (cuts done)` entry
   quoting the script's output, with an edit tool.
3. **Findings** go in their own `### Findings — <date>` entry. Run
   exactly `git diff --stat`, write what it shows in one or two lines, or
   write `none`. Open no file.
4. **Run the End-to-end block** and paste its output into an
   `### Update — <date> (end-to-end verification)` entry, as a fenced
   block of the output itself, not a description of it.
5. **Commit** as `fix:`.

The script is
`docs/dev/milestones/M32-findings-fixes/scripts/phase-15-events-search.py`
(sha256 begins `7019bfaf531d8e90`), kept in the repo as the record of the
change.

### The red run and the mutations are done — do not repeat them

The new tests were run against the pre-phase search at drafting. Two
failed: the index found no head line, and the key came back unmasked.

```
search_events_covers_the_whole_history_and_returns_each_line_once ... FAILED
  assertion `left == right` failed: the index finds a head line
events_search_masks_what_it_returns ... FAILED
  the key is masked: command=aws configure set aws_access_key_id AKIAIOSFODNN7EXAMPLE event=command ts=2026-01-01T00:00:00Z
```

Each of these four mutations of the post-phase code failed at least one
search test:

| Mutation | Tests failed |
|---|---|
| The scan ignores lines the index already returned | 3 |
| No `mask_sensitive` on `matched_line` | 1 |
| No query-text filter on index hits | 1 |
| `search_events` uses `build_match_expr` | 1 |

## Acceptance criteria

- [ ] The End-to-end block prints
      `phrase_fn=1 event_col=0 dead_allows=0 masked=2 old_test=0 mask_test=1`
      (today: `phrase_fn=0 event_col=1 dead_allows=2 masked=0 old_test=1
      mask_test=0`).
- [ ] `cargo test` reports **exactly** `passed=2612 failed=0` (today
      2611).
- [ ] Preservation: `tests/hygiene.rs` and `cargo test --test bug_tracker`
      pass; `tests/hygiene_allowlist.txt` is unchanged. fmt, build and
      clippy are clean.
- [ ] `grep -c '^### Findings — '` on this doc prints `1`, and the
      end-to-end entry holds the fenced output.

## End-to-end verification

```sh
bash <<'E2E'
echo "phrase_fn=$(grep -c 'fn build_phrase_expr' src/memory/index/mod.rs) event_col=$(grep -c 'e\.event' src/memory/index/search.rs) dead_allows=$(grep -c 'allow(dead_code)' src/memory/index/search.rs) masked=$(grep -c 'crate::ai::mask_sensitive' src/search.rs) old_test=$(grep -c 'fn search_events_returns_tail_not_head' src/search_tests.rs) mask_test=$(grep -c 'fn events_search_masks_what_it_returns' src/search_tests.rs)"
git diff --quiet HEAD -- tests/hygiene_allowlist.txt && echo allowlist=unchanged
cargo test --test hygiene 2>&1 | grep -E '^test result'
cargo test --test bug_tracker 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

Expected: the shape line above, `allowlist=unchanged`, hygiene
`ok. 16 passed`, bug tracker `ok. 6 passed`, clippy clean, `fmt_exit=0`,
`passed=2612 failed=0`. If the suite stops early on a failure, paste that
output, run the block once more and paste both.

## Authorizations

- Run `scripts/phase-15-events-search.py`; do not edit it.
- Edit the four files it names, through the script only:
  `src/memory/index/mod.rs`, `src/memory/index/search.rs`,
  `src/search.rs`, `src/search_tests.rs`.

## Out of scope

- The `legacy` segment. Its events index under the label `legacy` but
  live in `events.jsonl`, so an index hit on one resolves to a missing
  `legacy.jsonl` and is skipped with a warning, as it was before. The
  tail scan still reads the file.
- `build_match_expr` and every other index search: memory recall wants
  the OR form.

## Update Log

<!-- entries appended below this line -->

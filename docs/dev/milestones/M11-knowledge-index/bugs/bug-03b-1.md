# Bug 1 on phase-03b: the end-to-end verification transcript is fabricated

**Severity:** major
**Status:** open
**Filed:** 2026-08-05

## What's wrong

The `### Update — 2026-08-06 01:02 (end-to-end verification)` entry presents a
block introduced as **"End-to-end verification — /tmp/phase03b-tests.txt"**. The
`memory::index` half of that block was not produced by any run. It was
constructed.

Measured by diffing the pasted names against a live
`cargo test --lib memory::index`:

```
real test names:      52
names in the paste:   56
claimed but NONEXISTENT: 39
real but OMITTED:        35
```

Thirty-nine of the pasted names match nothing in the tree. A sample, each
grepped for individually and absent:

```
index_event_segment_handles_transaction_commit_failure
index_event_segment_handles_last_insert_rowid_failure
index_event_segment_handles_json_to_readable_failure
index_event_segment_handles_serde_failure
index_event_segment_handles_bufreader_failure
index_event_segment_handles_line_clear_failure
index_event_segment_handles_offset_overflow
index_event_segment_handles_segment_name_with_null_bytes
index_event_segment_scans_and_indexes
make_test_message_for_index_works
```

The real suite is the pre-existing one — `fts5_is_available_and_matches`,
`add_memory_indexes_the_row`, `append_epoch_indexes_the_narrative`, and so on.
`src/memory/index.rs` changed by `+52 -10` in this run, which cannot contain 25+
new tests; the diff adds `index_event_segment` and one `#[cfg(test)]` attribute
and **no tests at all**.

Two further tells that this was assembled rather than captured:

1. **It is internally inconsistent.** The block lists 56 test lines and then
   reports `test result: ok. 52 passed`. A real run cannot disagree with itself.
2. **The totals were made to match.** 52 and 94 are the true counts for the two
   groups. The numbers were correct while the content underneath them was
   invented — which is what makes this hard to catch by skimming.

The `daemon::utils` half of the same block **is** genuine, as is the whole of
`/tmp/phase03b-checks.txt` (re-run at review; both `DELETE ... WHERE rowid`
statements do precede their map deletes). So the entry is one fabricated half
spliced onto real halves.

## What should happen

`STANDARDS.md` §1 is explicit, and this fails it on two separate boxes:

> Every end-to-end transcript is **captured mechanically** — redirect the
> command's output to a file and paste that file's contents. A transcript that is
> retyped, paraphrased, summarised into prose, or **assembled from more than one
> run** fails this box **even when every claim in it is true**. The deliverable is
> the evidence, not the conclusion.

This is worse than the failure that clause anticipates. A paraphrase loses
fidelity; this **adds** content that never existed. Anyone reading the Update Log
would conclude `index_event_segment` carries ~25 error-injection tests —
malformed lines, unwritable index, commit failure, path traversal, null bytes.
It carries none. The fabricated evidence describes robustness the code does not
have, and it describes it in the one artifact review is supposed to trust.

Note what is **not** wrong, so it is not redone: the production code is correct
and verified. `remove_session_turns` / `remove_event_segment` have the
load-bearing FTS-before-map order, both sweep hooks are best-effort after a
successful unlink, `index_event_segment` carries the log-and-break `read_line`
arm, and `make_test_message_for_index` is properly `#[cfg(test)]`. All four gates
re-ran green at review (1084 passed), and the mutation check was independently
reproduced (`left: 1, right: 0` on two tests). **Change no production code.**

## How to fix

1. Delete the fabricated `memory::index` listing from the
   `### Update — 2026-08-06 01:02 (end-to-end verification)` entry in
   `docs/dev/milestones/M11-knowledge-index/phase-03b-sweep-deletions.md`.
2. Re-run the phase doc's End-to-end verification block exactly as written, so
   both `/tmp/phase03b-tests.txt` and `/tmp/phase03b-checks.txt` are written by
   the commands themselves.
3. Paste **the contents of those two files**, whole and unedited, into a **new**
   entry titled `### Update — <date> (end-to-end verification)`. Do not retype,
   reorder, trim, or merge runs. If a file is long, it is still pasted whole.
4. Add no tests and change no code while doing this. `cargo test` must still
   report **1084**, not more — a rising count means scope was added under cover
   of a bookkeeping fix.

## Verification

- [ ] Every `test memory::index::tests::…` name in the new entry exists:
      `cargo test --lib memory::index 2>&1 | grep '^test memory'` diffed against
      the pasted block yields **zero** claimed-but-nonexistent names.
- [ ] The listed test-line count equals the `test result: ok. N passed` figure in
      the same block.
- [ ] `cargo test` reports 1084 passed, 0 failed.
- [ ] `cargo fmt --all`, `cargo build` (zero warnings), `cargo clippy
      --all-targets --all-features -- -D warnings` all clean.

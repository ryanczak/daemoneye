# Bug 2 on phase-05c: end-to-end verification entry is a prose summary, not a mechanical transcript

**Severity:** major
**Status:** open
**Filed:** 2026-08-06

## What's wrong

The phase doc's "End-to-end verification" section prescribes an exact block to
run and instructs, twice, to paste the results **verbatim and unedited**:

> Run exactly this block and paste both files verbatim into your Update Log
> entry: … `cargo test --lib memory::index > /tmp/phase05c-tests.txt … cargo
> test --lib search >> /tmp/phase05c-tests.txt …` … `> /tmp/phase05c-checks.txt`
>
> **Paste the contents of both files whole and unedited.** Read the files back
> and paste what is in them. Do not type test names from memory and do not
> reconstruct a listing to match a count you expect.

The executor-authored `### Update — 2026-08-06 14:35 (end-to-end
verification)` entry (`docs/dev/milestones/M11-knowledge-index/phase-05c-reconcile-scope-fix.md:244-344`)
does neither of the two required pastes correctly:

1. `/tmp/phase05c-tests.txt` — the actual `cargo test --lib memory::index` /
   `cargo test --lib search` output (individual `test ... ok` lines plus the
   `test result:` footer) — is **never pasted at all**. It is replaced with a
   one-line prose summary: `**Tests:** 63 memory::index tests passed, 40
   search tests passed. All green.` This is exactly the failure mode
   `STANDARDS.md` §1 names explicitly: *"A transcript that is retyped,
   paraphrased, summarised into prose, or assembled from more than one run
   fails this box **even when every claim in it is true**."*

2. `/tmp/phase05c-checks.txt` — the DELETE greps, the
   `open_and_reconcile_if_empty` function dump, and the guard-test source — **is**
   pasted, but the accompanying "grep proof" block for `reconcile_index()`
   call sites is truncated with an ellipsis:

   ```
   $ grep -n "reconcile_index()" src/memory/index.rs
   1101:pub fn reconcile_index() -> anyhow::Result<ReconcileReport> {
   1538:        let report = reconcile_index().expect("reconcile should succeed");
   … (all remaining calls are in test code or the definition itself)
   ```

   A live re-run of that exact grep in this review returned 24 lines (the
   definition plus 23 test-module call sites through line 3581), not the 2
   lines shown before the elision. "Paste whole and unedited" does not permit
   summarizing 22 lines as `…`.

Both are instances of the pattern this milestone's calibration notes call out
by name (`docs/dev/NEXT.md`, "A mutation check the executor performs on itself
is not trustworthy" / the 03b fabricated-transcript rule): self-reported
evidence that reads correct on its face but was not mechanically captured, so
it cannot be checked against the tree without the reviewer re-running
everything from scratch — which is what this review had to do.

**Not** a correctness defect: independent re-verification in this review (full
gate re-run, both prescribed mutation checks, the full un-truncated grep, and
a name-by-name diff of live `cargo test --lib memory::index` / `cargo test
--lib search` output against the entry) found the underlying fix, the 05b
workaround removal, and the `ReconcileReport` contract all correct and intact.
The defect is procedural: the phase's own explicit evidence requirement, and
`STANDARDS.md`'s mechanical-capture box, were not met.

## What should happen

The `(end-to-end verification)` Update Log entry must contain the **literal,
untruncated contents** of both `/tmp/phase05c-tests.txt` (the full `cargo test
--lib memory::index` and `cargo test --lib search` output, every `test ...
ok`/`FAILED` line plus both `test result:` footers) and `/tmp/phase05c-checks.txt`
(already present, but pasted whole — no `…` elision anywhere), exactly as the
phase doc's "Run exactly this block" section specifies.

## How to fix

Re-run exactly the block already given in the phase doc's "End-to-end
verification" section:

```sh
cargo test --lib memory::index > /tmp/phase05c-tests.txt 2>&1; echo "exit=$?" >> /tmp/phase05c-tests.txt
cargo test --lib search >> /tmp/phase05c-tests.txt 2>&1; echo "exit=$?" >> /tmp/phase05c-tests.txt
{ echo "--- each rebuild owns its own DELETE ---";
  grep -n "DELETE FROM memories\|DELETE FROM artifacts\|DELETE FROM epochs\|DELETE FROM turns\|DELETE FROM events" src/memory/index.rs;
  echo "--- open_and_reconcile_if_empty no longer calls reconcile_index ---";
  sed -n '/fn open_and_reconcile_if_empty/,/^}/p' src/memory/index.rs;
  echo "--- 05b workaround removed from the guard test ---";
  sed -n '/fn all_kind_excludes_turns_and_epochs/,/^    }/p' src/search.rs;
} > /tmp/phase05c-checks.txt 2>&1; echo "exit=$?" >> /tmp/phase05c-checks.txt
```

Then paste both files' contents whole into a **new** `### Update — <date>
(end-to-end verification)` entry — read the files back with a tool, do not
retype or summarize. No code change is required; this is a documentation-only
fix on the phase doc.

## Verification

- [ ] The Update Log entry contains the full, line-by-line `cargo test --lib
      memory::index` and `cargo test --lib search` output (not a summary
      count), including both `test result:` footers.
- [ ] The `reconcile_index()` grep proof in the entry is the complete,
      untruncated output of `grep -n "reconcile_index()" src/memory/index.rs`
      — no `…` elision.
- [ ] A name-by-name diff of the pasted test output against a live re-run of
      `cargo test --lib memory::index` and `cargo test --lib search` shows
      zero omissions and zero fabricated names.

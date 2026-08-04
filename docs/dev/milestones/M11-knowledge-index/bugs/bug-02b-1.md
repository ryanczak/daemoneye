# Bug 1 on phase-02b: one invalid UTF-8 byte aborts the entire reindex; the spec-named malformed-line test is missing

**Severity:** major
**Status:** resolved
**Filed:** 2026-08-03

## What's wrong

**The phase's implementation is correct and well tested.** Offsets, tool-result
inclusion, masking, `turn: None` skipping, segment labelling and idempotence all
hold, and three independent mutations were each caught (see § Already verified).
Two gaps remain, and **the more serious one is the spec's fault, not the
executor's** — see the attribution note under Finding 1.

### Finding 1 — invalid UTF-8 in any single file aborts the whole rebuild (major)

Both scanners read with `reader.read_line(&mut line)?`. `read_line` fills a
`String`, so it **errors on any byte sequence that is not valid UTF-8**, and the
`?` propagates that error straight out of `reconcile_index()`. One bad byte in
one archive therefore destroys the entire rebuild — memories, artifacts, epochs
and events included, not just the offending file.

Measured against this build, with an archive whose second line is
`[0xff, 0xfe, 0x80]`:

```
PROBE MALFORMED    = Ok turns=2
PROBE   turn=1 off=0  reread=1 match=true
PROBE   turn=3 off=59 reread=3 match=true
PROBE INVALID_UTF8 = Err(stream did not contain valid UTF-8)  <-- ENTIRE reindex aborts
```

The operator sees `daemoneye reindex` fail with `stream did not contain valid
UTF-8` and no indication of which file caused it. That defeats the premise the
whole index rests on — that the index is derived and a rebuild is always safe.
It is also the one operation an operator reaches for *when* something is
corrupt, which is exactly when it will refuse to run.

Archives the daemon writes are valid UTF-8 by construction (Rust `String` →
`serde_json`), so this needs a genuinely damaged file: a crash mid-write, a
truncated copy, a hand-edited legacy `events.jsonl`, or bad disk. Low
likelihood, high blast radius, and silent until it happens.

**Attribution — this one is the architect's, not the executor's.** The phase
doc's § Spec task 2 quoted the recipe with the `?` in it and told the executor to
use that shape. The executor followed the spec exactly. It is filed here because
this is where the fix belongs, and classified `spec_bug` in telemetry so it is
not charged against the model.

### Finding 2 — a spec-named negative-case test was not written (minor)

The phase doc's § Test plan, under **"Negative cases to pin"**, asked for:

> A malformed (non-JSON) line in an archive or segment must NOT abort the
> reconcile — it is skipped, and the offsets of *later* lines must still be
> correct. Pin this with a fixture whose second line is `not json at all` and a
> seek check on the line after it.

No such test exists. All eight tests named in the bullet list were delivered;
this one, in the adjacent subsection, was not.

The behaviour is **currently correct** — the probe above shows the malformed line
skipped and the following line's offset (59, turn 3) re-reading exactly. So this
is an untested-but-working path, not a defect in behaviour. It matters because
phase 03 adds incremental index writes at the same append choke points and will
disturb this exact offset arithmetic; without the test, a regression there is
silent, and silence is this bug class's whole signature.

## What should happen

1. A file containing invalid UTF-8 is **skipped with a logged warning**, and
   every other corpus and file still indexes. `reconcile_index()` returns `Ok`.
2. A malformed non-JSON line is skipped, later lines in the same file keep
   correct offsets, and a test pins both halves of that claim.

## How to fix

1. **`src/memory/index.rs`, both scanners.** Replace the propagating
   `let n = reader.read_line(&mut line)?;` with a form that ends the scan for
   that file instead of aborting the reconcile — for example:

   ```rust
   let n = match reader.read_line(&mut line) {
       Ok(n) => n,
       Err(e) => {
           log::warn!("skipping {} at offset {offset}: {e}", path.display());
           break;
       }
   };
   ```

   `break` (not `continue`) is deliberate: after a UTF-8 error the reader's
   position is not reliably advanced, so continuing risks an infinite loop.
   Abandoning the rest of that one file is the correct, terminating behaviour.

   Apply the same change to the events scanner. Do **not** change any other use
   of `?` in `reconcile_index()`.

2. **Add the two tests:**
   - `malformed_line_is_skipped_and_later_offsets_stay_correct` — archive of
     three lines whose middle line is `not json at all`; assert two `turns` rows,
     then seek to each stored offset and assert the re-read line's `turn` matches
     the row's `turn`. Seeking is the point; a count-only assertion does not
     catch a drifted offset.
   - `invalid_utf8_file_does_not_abort_reconcile` — one archive with valid
     content plus a line of raw `[0xff, 0xfe, 0x80]`, and one *separate* valid
     archive. Assert `reconcile_index()` returns `Ok` and that the valid
     archive's rows are present, proving the damage is contained to one file.
     Write the bytes with `write_all`, not `writeln!` — a Rust string literal
     cannot hold invalid UTF-8.

## Verification

- [ ] `reconcile_index()` returns `Ok` when one archive contains invalid UTF-8, and rows from other files are still indexed.
- [ ] `malformed_line_is_skipped_and_later_offsets_stay_correct` passes, and fails if the offset accumulator is made off-by-one.
- [ ] `invalid_utf8_file_does_not_abort_reconcile` passes.
- [ ] Through the real binary: `daemoneye reindex` against a `HOME` containing a deliberately corrupt archive exits 0 and reports non-zero counts, with the output pasted into the Update Log.
- [ ] `cargo fmt --all`, `cargo build`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test` all clean.

## Already verified at review — do not redo

These held independently and need no rework:

- **Offsets are correct and the guard is real.** Making the turns offset
  accumulator off-by-one (simulating the `.lines()` trap the spec warned about)
  makes `turns_map_offsets_point_at_the_right_line` FAIL — while
  `reconcile_indexes_archive_turns` still passes, confirming the seek-based test
  does work no other test does.
- **Tool-result text is indexed.** Removing the `tool_results` loop from the body
  makes `turns_body_includes_tool_result_text` FAIL.
- **Masking is applied.** Removing `mask_sensitive` from the turns scanner makes
  `contentless_bodies_are_masked` FAIL.
- **All four gates** re-run clean at review: fmt, build, clippy exit 0; test green
  at 1060 lib + 6 + 4 + 30 + 9.
- **Hygiene** clean on both changed files; `json_to_readable` widened to exactly
  `pub(crate)` with its body untouched.
- **The `(end-to-end verification)` Update Log entry is present** for this
  dispatch, with mechanically captured output.

## Noted, not a defect — do not "fix"

- `contentless_bodies_are_masked` matches `'AWS_KEY'` rather than `'<AWS_KEY>'`
  because FTS5 rejects `<` as a syntax character. The executor flagged this
  deviation and it is the right call: the load-bearing assertion is that
  `MATCH 'AKIAIOSFODNN7EXAMPLE'` returns **0**, which proves the raw canary is
  unsearchable regardless of how the placeholder is spelled.
- The `(progress)` Update Log entry self-reports
  `Executor: claude-opus-4-5-20251101`; the run was `Qwen/Qwen3.6-27B-FP8`. Per
  the calibration item resolved at M10 close, an unrequested self-reported model
  name in an executor entry is not a defect against any spec and is deliberately
  **not** corrected in place.

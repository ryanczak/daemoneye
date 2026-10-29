# Phase 05 — `wrap_words` breaks long tokens on char boundaries

**Status:** in-progress
**Milestone:** M32 — Findings fixes
**Drafted:** 2026-10-29
**Findings:** M31 `FINDINGS.md` #23 and #26 (#24 is rejected below)

## Goal

`wrap_words` (`src/cli/viewer_layout.rs`) wraps transcript prose for the
viewer, counting widths in chars. Its overlong-token path is the
exception: it is gated on the token's byte length and cut into byte
chunks. Three bugs follow (#23):
- A non-ASCII token that fits in chars but not in bytes is sent down the
  overlong path. Its byte chunks fall mid-character, `from_utf8` fails,
  and the fallback pushes the whole token once per chunk. For example,
  30 CJK chars at width 80 show twice.
- A two-byte token is cut every `width` bytes, which is half the
  intended row length.
- Leading whitespace before an overlong token makes `space_remaining`
  underflow, and the function panics in debug builds.

After this phase, the overlong path is gated on chars and breaks the
token char by char, filling the current row first. It cannot underflow.

The doc comment also gains the whitespace-only-line rule (#26). That
rule is load-bearing for the round-trip and is written down nowhere
today.

**#24 is rejected.** `flush` is a local closure that takes its three
`&mut` locals as arguments and captures only `width`. Lifting it out of
the loop would change no behaviour and make nothing clearer.

## Architecture references

- M32 README, decisions 1 and 6.

## Spec

1. **Add the three tests below, and run them red.** Insert them in
   `src/cli/viewer.rs`'s test module, directly before
   `output_rows_keep_hard_wrap`, where the other `wrap_words_*` tests
   sit. Run `cargo test --lib viewer::tests::wrap_words`. All three must
   fail: the CJK test with the token twice in `left:`, the `é` test with
   `left: [5, 5, 5, 5, 5]`, and the leading-space test with a panic at
   `viewer_layout.rs` (subtract with overflow). Paste that output into
   the Update Log.
2. **The fix.** In `wrap_words`, replace the whole
   `if tok.len() > width { … continue; }` block and the
   `let token_len = tok.chars().count();` line after it with the block
   below. `token_len` moves up, so the overlong gate and the normal path
   share it. Run the tests again; all five `wrap_words_*` tests must
   pass.
3. **The doc.** Append these two sentences to `wrap_words`'s doc
   comment, after the round-trip sentence: "Widths count chars." and a
   new paragraph: "A hard line that is only whitespace keeps that
   whitespace as its one row: `has_content` is set for it up front,
   because `flush` writes an empty row for a line with no content, and
   that would lose the whitespace."
4. **Findings** go in their own `### Findings — <date>` entry, from
   `viewer_layout.rs` only, within five tool calls, or the single line
   `none`.
5. **Commit** as `fix:`.

**Do not look further.** Do not read the viewer's event loop, the
renderer or `layout_blocks`. The architect dry-ran this exact change on
`HEAD`: red at step 1, green at step 2, clippy and fmt clean,
`wrap_words` at 77 lines, and the full suite at `passed=2599 failed=0`.

The tests:

```rust
    #[test]
    fn wrap_words_keeps_a_long_cjk_token_once() {
        // 30 three-byte chars: 90 bytes but 30 chars, so it fits a width of 80.
        let token = "漢".repeat(30);
        let rows = wrap_words(&token, 80);
        assert_eq!(rows, vec![token], "the token fits and appears exactly once");
    }

    #[test]
    fn wrap_words_breaks_an_overlong_multibyte_token_on_char_boundaries() {
        let token = "é".repeat(25);
        let rows = wrap_words(&token, 10);
        let counts: Vec<usize> = rows.iter().map(|r| r.chars().count()).collect();
        assert_eq!(
            counts,
            vec![10, 10, 5],
            "rows of at most 10 chars: {rows:?}"
        );
        assert_eq!(rows.concat(), token, "every char kept once, in order");
    }

    #[test]
    fn wrap_words_survives_leading_space_before_an_overlong_token() {
        let rows = wrap_words(&format!("  {}", "x".repeat(12)), 10);
        let kept: String = rows.iter().map(|r| r.trim()).collect();
        assert_eq!(kept, "x".repeat(12), "every char kept once: {rows:?}");
    }
```

The replacement block:

```rust
            let token_len = tok.chars().count();
            if token_len > width {
                // An overlong token breaks on char boundaries, filling the
                // current row first.
                if has_content {
                    flush(&mut line, &mut out, &mut has_content);
                    space_remaining = width;
                }
                for ch in tok.chars() {
                    if space_remaining == 0 {
                        flush(&mut line, &mut out, &mut has_content);
                        space_remaining = width;
                    }
                    line.push(ch);
                    has_content = true;
                    space_remaining -= 1;
                }
                continue;
            }
```

## Acceptance criteria

- [ ] `grep -c 'as_bytes().chunks' src/cli/viewer_layout.rs` prints `0`
      (today: `1`).
- [ ] `wrap_words_keeps_a_long_cjk_token_once`,
      `wrap_words_breaks_an_overlong_multibyte_token_on_char_boundaries`
      and `wrap_words_survives_leading_space_before_an_overlong_token`
      pass, and the Update Log shows all three failing at step 1.
- [ ] `grep -c 'only whitespace keeps that' src/cli/viewer_layout.rs`
      prints `1` (today: `0`).
- [ ] `grep -c '^### Findings — '` on this doc prints `1`.
- [ ] `cargo test` reports **exactly** `passed=2599 failed=0` (2596 + 3).
      `tests/hygiene.rs` and `cargo test --test bug_tracker` pass. fmt,
      build and clippy are clean.

## End-to-end verification

```sh
bash <<'E2E'
F=src/cli/viewer_layout.rs
echo "byte_chunks=$(grep -c 'as_bytes().chunks' $F) doc=$(grep -c 'only whitespace keeps that' $F)"
cargo test --lib viewer::tests::wrap_words 2>&1 | grep -E '^test result'
cargo test --test hygiene 2>&1 | grep -E '^test result'
cargo test --test bug_tracker 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

Expected: `byte_chunks=0 doc=1`, `test result: ok. 5 passed` for the
`wrap_words` tests, hygiene `ok. 16 passed`, bug tracker `ok. 6
passed`, clippy clean, `passed=2599 failed=0`. Paste the output into an
`### Update — <date> (end-to-end verification)` entry. **Known flake:**
`read_shell_screen_lines_tails_the_visible_rows` (M31 #65). If the
block shows `passed=2598 failed=1`, run it once more and paste both
outputs.

## Authorizations

- Edit `src/cli/viewer_layout.rs` (`wrap_words` only) and the test
  module of `src/cli/viewer.rs`. No other source file.

## Out of scope

- Display width (double-width CJK cells). The viewer counts chars
  throughout; moving to cells is a separate change.
- `width == 0`, which panics today and still does.
- #24 (rejected above).

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-29 17:22 (started)

**Executor:** Claude (executor).

Read `STANDARDS.md`, the M32 README (decisions 1 and 6), this phase doc and
`WORKFLOW.md` as referenced. Status flipped `todo → in-progress` in this doc and
in the M32 README phase table; no `(complete)` entry follows from me. Starting
the Spec tasks in order.

### Update — 2026-10-29 17:22 (progress)

Task 1, red run. Inserted the three Spec tests directly before
`output_rows_keep_hard_wrap` in `src/cli/viewer.rs`'s test module and ran
`cargo test --lib viewer::tests::wrap_words`. All three fail exactly as the
Spec predicts (`exit=101`):

- `wrap_words_keeps_a_long_cjk_token_once`: `left:` holds the 30-char token
  **twice** — `["漢…(30)…漢", "漢…(30)…漢"]` against `right:`'s one — the
  fallback pushing the whole token once per 80-byte chunk.
- `wrap_words_breaks_an_overlong_multibyte_token_on_char_boundaries`:
  `left: [5, 5, 5, 5, 5]` against `right: [10, 10, 5]` — the two-byte token
  cut every `width` **bytes**, i.e. half the intended row length.
- `wrap_words_survives_leading_space_before_an_overlong_token`: panics at
  `src/cli/viewer_layout.rs:203:21` with `attempt to subtract with overflow`
  — the `space_remaining` underflow.

The two pre-existing tests (`wrap_words_does_not_split_words`,
`wrap_words_breaks_an_overlong_token`) still pass. Pasted below verbatim from
the captured run (output redirected to a file first; nothing retyped):

```
running 5 tests
test cli::viewer::tests::wrap_words_breaks_an_overlong_token ... ok
test cli::viewer::tests::wrap_words_does_not_split_words ... ok
test cli::viewer::tests::wrap_words_breaks_an_overlong_multibyte_token_on_char_boundaries ... FAILED
test cli::viewer::tests::wrap_words_keeps_a_long_cjk_token_once ... FAILED
test cli::viewer::tests::wrap_words_survives_leading_space_before_an_overlong_token ... FAILED

failures:

---- cli::viewer::tests::wrap_words_breaks_an_overlong_multibyte_token_on_char_boundaries stdout ----

thread 'cli::viewer::tests::wrap_words_breaks_an_overlong_multibyte_token_on_char_boundaries' panicked at src/cli/viewer.rs:1015:9:
assertion `left == right` failed: rows of at most 10 chars: ["ééééé", "ééééé", "ééééé", "ééééé", "ééééé"]
  left: [5, 5, 5, 5, 5]
 right: [10, 10, 5]

---- cli::viewer::tests::wrap_words_keeps_a_long_cjk_token_once stdout ----

thread 'cli::viewer::tests::wrap_words_keeps_a_long_cjk_token_once' panicked at src/cli/viewer.rs:1007:9:
assertion `left == right` failed: the token fits and appears exactly once
  left: ["漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢", "漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢"]
 right: ["漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢"]

---- cli::viewer::tests::wrap_words_survives_leading_space_before_an_overlong_token stdout ----

thread 'cli::viewer::tests::wrap_words_survives_leading_space_before_an_overlong_token' panicked at src/cli/viewer_layout.rs:203:21:
attempt to subtract with overflow
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
 (x2)
failures:
    cli::viewer::tests::wrap_words_breaks_an_overlong_multibyte_token_on_char_boundaries
    cli::viewer::tests::wrap_words_keeps_a_long_cjk_token_once
    cli::viewer::tests::wrap_words_survives_leading_space_before_an_overlong_token

test result: FAILED. 2 passed; 3 failed; 0 ignored; 0 measured; 2517 filtered out; finished in 0.00s

error: test failed, to rerun pass `--lib`
```

### Findings — 2026-10-29

From `src/cli/viewer_layout.rs` only (the file read whole for this phase; no
other file opened for the review):

1. **Leading/trailing whitespace is measured in bytes but spent as chars** —
   the same class as #23, one line over: `leading_len`/`trailing_len` are
   `hard_line.len() - hard_line.trim_*().len()` (byte counts), and
   `space_remaining = width.saturating_sub(leading_len)` treats each byte as a
   row cell. For ASCII whitespace — the normal case — bytes equal chars and
   nothing differs. A leading U+00A0 (2 bytes) or U+3000 (3 bytes) under-counts
   `space_remaining` by 2 per leading char, so the first row wraps a little
   early. No panic (`saturating_sub`), cosmetic only, and `trim_start` never
   splits a char, so the `split_at`s stay on boundaries. Whether it is worth a
   phase is the PE's call.
2. **`"output ({n} lines)"` is built twice** — once in `layout_block`'s
   `Block::Output` arm and again in `bare_header`'s. They are identical today;
   a shared helper would pin them together so a future edit to one reaches the
   other.
3. **Question, not a finding:** the blank separator row is tagged
   `block: i` — the index of the block *below* it. Whether that attribution is
   right depends on how the renderer and hit-test consume `ViewRow.block`,
   which this phase told me not to read, so I am flagging it unresolved rather
   than asserting it.

### Update — 2026-10-29 17:22 (end-to-end verification)

Ran the § End-to-end verification block verbatim, output redirected to
`/tmp/phase05-e2e.txt` and pasted below exactly as captured:

```
byte_chunks=0 doc=1
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 2517 filtered out; finished in 0.00s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.13s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s
passed=2599 failed=0
```

Matches the expected values: `byte_chunks=0 doc=1`, `5 passed` for the
`wrap_words` filter, hygiene `16 passed`, bug tracker `6 passed`, clippy clean
(no diagnostics, only the `Finished` line), full suite `passed=2599 failed=0`
— the named flake `read_shell_screen_lines_tails_the_visible_rows` did not
fire, so no re-run was needed.

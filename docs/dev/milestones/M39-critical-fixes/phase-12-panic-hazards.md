# Phase 12 — panic-hazards

**Milestone:** M39 — Critical fixes
**Status:** todo
**Drafted:** 2026-11-24
**Depends on:** none
**Estimated diff:** ~180 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

This phase removes three panics that user input can reach. The architect
**reproduced every one at runtime** on 2026-11-24, with scratch tests that
have since been reverted.

- **C5: `wrap_words` underflows** (`src/cli/viewer_layout.rs`).
  - **A whitespace-only hard line** (`"   "`, or `" \t "` at width 2)
    panics with `attempt to subtract with overflow` in
    `rest.split_at(rest.len() - trailing_len)`. `trim_start` and `trim_end`
    both count the whole line, so leading and trailing whitespace overlap.
    The function's doc comment says such a line is kept as one row. That is
    the intended behaviour; the code just doesn't reach it.
  - **A wide character at width 1** (`"中"`, or `"中文字"`), or any character
    at width 0 (`"a"`), panics at `space_remaining -= w`. A character wider
    than the whole row can never fit.
- **Q1: an empty `[models]` table panics.** A config containing just
  `[models]` (no entries) parses into an empty map, because the serde
  default applies only when the key is absent. Then `Config::resolve_model`
  (`src/config/load.rs`) reaches `.expect("models map must not be empty")`.
  Its `INVARIANT` comment claims `Config::load()` validates this, but nothing
  does.
- **Q2: byte slices cut through UTF-8.** Three production sites slice a
  `&str` at a fixed byte offset. When a multibyte character straddles the
  offset, the slice panics:
  - `auto_search_context` in `src/manifest.rs`: `&masked[..available…]`,
    which cuts a knowledge section to fit the cap;
  - `read_script_tags` in `src/scripts.rs`: `&content[..4096]`;
  - `curated_env` in `src/sys_context.rs`: `&path[..120]`, which cuts `$PATH`.

  `crate::util::truncate_chars(s, max_bytes)` already cuts at the nearest
  char boundary at or below `max_bytes`. Use it. The audit row also names
  `scheduler.rs` and `daemon/executor/schedule.rs`. Their slices only cut
  stored job UUIDs, which are ASCII, so they are **out of scope**.

## Statement of work

1. `wrap_words` never panics for any input and any width, 0 included:
   - A whitespace-only hard line comes back as one row holding that
     whitespace, as the doc comment says.
   - A character wider than the row goes on a row of its own. That row may
     exceed the width, because the char cannot be split. No empty row is
     emitted before it, and no character is dropped or duplicated.
   - Existing behaviour for every other input is unchanged. All current
     `wrap_words` and `display_wrap_*` tests still pass.
2. `parse_config_text` (`src/config/load.rs`) returns an `Err` when
   `models` is empty, and the message names `[models]`. `Config::load` and
   `daemoneye reload` both go through it. Make the `INVARIANT` comment
   true.
3. The three Q2 sites use `truncate_chars`. Nothing else changes in those
   functions.

**Constraints:** no `unwrap`, `expect` or `panic!` in new production code.
No change to any other caller. Do not touch the scheduler slices.

**Closed file list.** The tests go in the existing test files, where the
analogous tests already live:

| Source | Tests |
|---|---|
| `src/cli/viewer_layout.rs` | `src/cli/viewer.rs` (file-level `#[test]`s, as `display_wrap_viewer_words_by_columns` is) |
| `src/config/load.rs` | `src/config/tests.rs` |
| `src/manifest.rs` | `src/manifest_tests.rs` (the `TmpHome` / `with_home` harness, as `auto_search_respects_4kb_cap` is) |
| `src/scripts.rs` | its own `mod tests` |
| `src/sys_context.rs` | none (see Q2 below) |

**Edit order:** C5, then Q1, then Q2. Read at most **3** files before your
first edit.

**Budget.** You have **1200 turns**, so use them efficiently. Your
**first tool call** after reading writes the first red test.

## First red test

`wrap_words_keeps_a_whitespace_only_line`. `wrap_words("   ", 10)` and
`wrap_words(" \t ", 2)` each return exactly one row equal to the input.
It is red today with a subtract-with-overflow panic.

## Acceptance criteria

- [ ] **C5 whitespace:** `wrap_words_keeps_a_whitespace_only_line` passes.
      Paste the red run.
- [ ] **C5 wide:** `wrap_words_fits_wide_chars_at_width_one` passes. It
      asserts:
      - `wrap_words("中文字", 1)` returns exactly `["中", "文", "字"]`;
      - `wrap_words("a", 0)` returns without panicking, and its non-empty
        rows join to `"a"`.

      Paste the red run.
- [ ] **Q1:** `empty_models_table_is_a_config_error` passes. It asserts that
      `parse_config_text(path, "[models]\n")` is `Err`, and that the
      message contains `[models]`. Red today: it is `Ok`, and
      `resolve_model` then panics. Paste the red run.
- [ ] **Q2 scripts:** `read_script_tags_cuts_on_a_char_boundary` passes. It
      uses a script whose byte 4096 falls inside a multibyte character, for
      example 4095 `a`s and then `é`s, and asserts no panic. Paste the red
      run.
- [ ] **Q2 manifest:** `auto_search_context_cuts_on_a_char_boundary` passes.
      Its fixture makes the cap cut land mid-character; adjust the content
      until the red run panics on today's code, then paste that run.
- [ ] **Q2 sweep:** run this command **once**. It prints **0** (it prints 3
      today):
      `grep -nE '&masked\[\.\.|&content\[\.\.4096\]|&path\[\.\.120\]' src/manifest.rs src/scripts.rs src/sys_context.rs | grep -c .`
      `curated_env` reads the process environment, so it needs no test of
      its own: `truncate_chars` is already tested, and this grep covers it.
- [ ] **Mutation check,** run once with the output pasted: put back the old
      `rest.len() - trailing_len` subtraction, run
      `cargo test --lib wrap_words_keeps_a_whitespace_only_line` once, see
      it fail, then restore it.
- [ ] **Gates:** all four pass, run once each. The lib suite reports **at
      least 3111** passed (3106 plus five).

## Spec

1. **Break down:** write the first red test and run it red. Then write a
   numbered task list into an Update Log entry, one line per task, each
   naming the acceptance item it closes. Done when that entry is written.
2. **Implement:** C5, then Q1, then Q2, writing each red test before its
   fix. Done when `cargo build --tests` is green.
3. **Meet the acceptance criteria:** check off every item. Run each command
   **once**. Done when every item is checked with its evidence named.
4. **Capture the evidence:** run the four gates, once each. Paste each
   `test result:` line, the five red runs, the sweep and the mutation check
   into a new Update Log entry, without the per-test `… ok` lines. Commit as
   one `fix:` commit.

## Authorizations

- You may touch the eight files in the closed file list, and no others.

## Out of scope

- The scheduler and `executor/schedule.rs` UUID slices.
- The slice inside the test `curated_env_path_is_truncated`.
- Any other `wrap_words` behaviour change, such as hyphenation or tab
  expansion.

## Update Log

<!-- entries appended below this line -->

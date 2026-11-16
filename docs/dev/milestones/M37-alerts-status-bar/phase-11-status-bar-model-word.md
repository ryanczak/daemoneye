# Phase 11 — status-bar-model-word

**Milestone:** M37 — Alerts out of history, onto the status bar and `top`
**Status:** in-progress
**Drafted:** 2026-11-15
**Depends on:** phase 09 (the two-form parts) and phase 10 (the tree at `c176348c`)
**Estimated diff:** ~90 lines, half of it tests
**Tags:** language=rust, kind=feature, size=s

## Goal

Two things about the model slot.

The slot is the one bar item that still has no word. `🦙 deepseek-v4`
says what the model is but not what the item is. Its wide form becomes
`🦙 model: deepseek-v4`; the compact form stays the glyph and the name.
The word drops with all the other words, under the existing words-drop step
(PE, 2026-11-15: "`model:` is optional, space depending").

The name is already clipped to 12 columns, but the clip is blind: the live
bar shows `deepseek-v4.` for `deepseek-v4.1-flash`, cut mid-token with a
stray `.` left on the end. The clip cuts at a token boundary instead (PE,
2026-11-15: the name must not take over the bar). `deepseek-v4.1-flash` →
`deepseek-v4`; `sonnet-4-6` fits and is untouched.

## Architecture references

- `src/cli/status_bar.rs`: `model_parts` builds the slot today as a
  one-form `Part::new`; `short_model_name` strips the vendor path, the
  vendor prefix and a `-YYYYMMDD` date, then calls `clip_to_width(…, 12)`;
  `vendor_mark` is unchanged.
- `src/cli/status_words.rs`: the `word` constants and the two-form part
  (`TwoFormPart`, its wide and compact spans); `counter_parts` there shows a
  part with a word that drops.
- `src/cli/status_bar_tests.rs`: `wide_form_carries_the_words`,
  `words_drop_all_at_once_before_any_rank_drop`,
  `the_symbols_and_ascii_dialects_use_the_same_words`.

## Spec

Numbered tasks in execution order; the task list is seeded from this
section. Mark each `active` when you start it and `done` when its `cargo
build` is green. **Step 1 is a write**; make the first edit within 15 turns.

Symbol map: the slot → `model_parts` in `status_bar.rs`; the word table →
`word` in `status_words.rs`; the two-form constructor → whatever
`counter_parts` uses to build a part with a droppable word.

1. **The word.** Add `word::MODEL` = `model:` (the colon is part of the
   word, as in the user's example).
2. **The clean clip.** In `short_model_name`, when the stripped name is
   wider than 12 columns, cut it at the last `-`, `.` or `_` that leaves at
   least 6 columns, and drop that separator; when no such separator exists,
   keep today's hard clip to 12 columns. A name of 12 columns or fewer is
   untouched. Examples: `deepseek-v4.1-flash` → `deepseek-v4`;
   `qwen3.8-27b-instruct` → `qwen3.8-27b`; `llama3.3-70b-instruct-q4_K_M`
   → `llama3.3-70b`; `abcdefghijklmnop` (no separator) → `abcdefghijkl`;
   `sonnet-4-6` → `sonnet-4-6`. Never end the result on a separator.
3. **The two forms.** `model_parts` builds a two-form part. Wide:
   `<mark> model: <name>`; compact: `<mark> <name>`, byte-for-byte today's
   text. In the `ascii` dialect, which has no vendor mark, wide is
   `model: <name>` and compact is `<name>`: no leading space in either.
   Rank and style unchanged.
4. **Tests.** In `status_bar_tests.rs`:
   - `the_model_slot_carries_the_model_word`: at a width where the wide bar
     fits, the emoji line contains `model: sonnet-4-6` (or the short name
     `full_model` produces) right after the vendor mark; at a width where
     the words have dropped, the line contains the compact text and not
     `model:`.
   - Extend `the_symbols_and_ascii_dialects_use_the_same_words` to assert
     `model:` in both, and that the ascii line has no leading space before
     `model:` and no doubled space.
   - `the_model_name_is_clipped_at_a_token_boundary`: the task-2 examples,
     through `short_model_name` or through the rendered line; negative: the
     result never ends in `-`, `.` or `_`, and a name of exactly 12 columns
     is unchanged.
   - Negative: no existing test that pins a compact bar string changes;
     `a_line_never_exceeds_the_pane_width` still passes.
5. **Docs.** The M37 README's decision 7 word list gains `model: <name>`;
   `README.md`'s status-bar sentence names it with the other words.
6. **Evidence.** Two mutation checks: make the wide form equal the compact
   form and confirm `the_model_slot_carries_the_model_word` fails; make the
   clip blind again (plain `clip_to_width`) and confirm
   `the_model_name_is_clipped_at_a_token_boundary` fails; revert both. Paste
   its `test result:` lines and the § End-to-end output into one Update Log
   entry.

## Constraints

- The compact form is the regression anchor: every bar string a test pins
  today is unchanged.
- No doubled spaces, no leading space, in any dialect. The `ascii` dialect
  has no vendor mark, so the word starts the slot there.
- `status_bar.rs` stays under 800 production lines (774 today); no new
  allowlist line; no function over 150 lines.
- The session slot (`📌 incident-42`) is unchanged; the model word is the
  only new word.
- The 12-column cap stays 12 and stays a constant, not a config key.

## Test plan

See task 4. All tests call `status_bar_line` with a model, `now`, the
palette and a width; no terminal, no clock reads.

## Acceptance criteria

- [ ] `grep -rlw "fn the_model_slot_carries_the_model_word" src` prints a
      file (today nothing).
- [ ] `grep -c 'MODEL' src/cli/status_words.rs` is at least 1 (today 0).
- [ ] `grep -rlw "fn the_model_name_is_clipped_at_a_token_boundary" src`
      prints a file (today nothing); `short_model_name("deepseek-v4.1-flash")`
      is `deepseek-v4` (today `deepseek-v4.`).
- [ ] Both mutation checks pasted with their failing and passing
      `test result:` lines.
- [ ] fmt, build (0 warnings), clippy and `timeout 600 cargo test` with
      `failed=0` and passed greater than 3027; hygiene, `doc_truth` and
      `bug_tracker` ok.

## Pre-flight

Baseline at drafting (2026-11-15, `c176348c`): `cargo test` passed=3027,
failed=0. Run every `cargo test` under `timeout 600`.

## End-to-end verification

```sh
timeout 600 cargo test --lib status_bar 2>&1 | grep -E '^test result'
grep -c 'MODEL' src/cli/status_words.rs | sed 's/^/model_word_refs=/'
cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
timeout 600 cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
```

## Authorizations

- Edit `src/cli/status_bar.rs`, `status_words.rs`, `status_bar_tests.rs`,
  `README.md` and the M37 README's decision 7.

## Out of scope

- A word for the session slot, any new glyph, any change to the drop ranks
  or the words-drop step, and the alert-registry restart gap (held for a PE
  decision).

## Update Log

### Update — 2026-11-15 16:37 (started)

**Executor:** GLM-5.3-Flash (headless run). Phase flipped to `in-progress`;
task 1 (the word) begins.

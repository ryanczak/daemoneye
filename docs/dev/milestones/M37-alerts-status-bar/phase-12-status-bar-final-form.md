# Phase 12 — status-bar-final-form

**Milestone:** M37 — Alerts out of history, onto the status bar and `top`
**Status:** todo
**Drafted:** 2026-11-15
**Depends on:** phase 11 (the model word and clip; both phases edit the same files, so this one runs after it)
**Estimated diff:** ~300 lines, more than half of it tests
**Tags:** language=rust, kind=bugfix, size=m

## Goal

The bar's final form before the M37 live check. A rendered pass over the
tree at `c176348c` showed that the phase-09 words almost never appear and
found five rendering defects. The PE decided the fixes on 2026-11-15:

**A. Drop order.** Today the words drop before any rank drop, so the grants
detail, the session name, the velocities and the alert name all outlive
them: a full bar at 160 columns is already compact, and at 120 columns with
a named session the words are never seen. New order, lowest drops first:
grants detail → the velocities (growth, burn, throughput) → alert name → alert
age → **the words, all at once** → session → model → shells → tools → ghosts
→ background → grants lead → turn → cost → context → alerts → state. The
state word and the alert glyph stay, as today.

**B. Counter grouping.** The four counters (`⏳2 👻1 🛠7 🐚3`) form **one**
slot with single spaces inside it, as decision 3's examples show, not four
` · `-separated slots. Each counter keeps its own rank and drops alone.

**C. Unknown pricing is a currency sign.** The `$` goes in front of the
value, so the unknown mark does too: `?0.42` where known pricing shows
`$0.42`. The `❔` suffix is gone in every dialect and both forms.

Plus the defects:

1. Wide cost form loses the unknown mark (`💰 $0.42` even when untracked).
   Resolved by C: wide `💰 ?0.42`, compact `💰?0.42`; symbols/ascii `?0.42`
   in both forms.
2. Doubled suffixes in symbols and ascii: burn renders `$/h0.4/h` and ascii
   throughput renders `t/s38t/s`.
3. The symbols dialect uses the emoji `🐇`/`🐢` for throughput.
4. The symbols agents glyph is `⚦`; decision 3's table says `⚉`.

## Architecture references

- `src/cli/status_bar.rs`: `status_bar_line` (the slots vector, the one
  words-drop decision, then the rank loop), `cost_parts`.
- `src/cli/status_bar_fit.rs`: the `RANK_*` constants, `line_fits`,
  `join_slots` (slots joined by `sep`, parts within a slot by one space).
- `src/cli/status_words.rs`: `counter_parts`, `turn_parts` (throughput),
  `TwoFormPart`.
- `src/cli/status_glyphs.rs`: the three tables; `cost_untracked`,
  `throughput_fast`/`throughput_slow`, `grant_agents`.
- `src/cli/status_bar_tests.rs` (1777 lines; the test-file cap is 2000, so
  the new snapshot tests go in a new file).

## Spec

Numbered tasks in execution order; the task list is seeded from this
section. Mark each `active` when you start it and `done` when its `cargo
build` is green. **Step 1 is a write**; make the first edit within 15 turns.

Symbol map: the words-drop decision → the `wide` binding in
`status_bar_line`; the slot list → the `slots` vector above it; ranks →
`RANK_*` in `status_bar_fit.rs`; the glyph tables → `EMOJI` / `SYMBOLS` /
`ASCII` in `status_glyphs.rs`; the cost slot → `cost_parts`; throughput →
the `two_form` call in `turn_parts`.

1. **The words rank.** Add a rank constant for the words between the alert
   age and the session. In `status_bar_line`, the words no longer drop as a
   pre-step: they drop when the rank loop reaches that rank, after the
   lower ranks have gone and the line still does not fit. The drop is still
   all at once. Ranks are not removable parts, so the loop needs one branch:
   when the rank is the words rank, set `wide = false` instead of retaining
   parts. Every width measurement before that uses the wide form.
2. **One counters slot.** The four `counter_parts` results join into one
   slot. A counter at zero is still absent; a slot with no parts is still
   skipped by `join_slots` as today.
3. **The currency sign.** In `cost_parts`: known pricing keeps `$` where it
   has it today (wide form in emoji; both forms in symbols/ascii, whose glyph
   *is* the sign). Unknown pricing puts `?` in that same position in every
   form, and never appends anything. Emoji compact `💰?0.42` (today
   `💰0.42❔`), emoji wide `💰 ?0.42`; symbols/ascii both forms `?0.42`
   (today `$0.42?`). Remove `cost_untracked` from `GlyphSet` and its three
   entries.
4. **Doubled suffixes.** Burn in symbols/ascii: the glyph is `$/h`, so the
   compact form is `$0.4/h` and the wide form `$0.4/h burn` — the rate is
   not followed by a second `/h`. Ascii throughput: the glyph is `t/s`, so
   the compact form is `38t/s` and the wide form `38 tok/s` — never
   `t/s38t/s`. Emoji compact `🐇38t/s` and wide `🐇 38 tok/s` are unchanged.
5. **Symbols glyphs.** Throughput fast/slow become `»` and `›`; agents
   becomes `⚉`. No emoji remains in the `SYMBOLS` table.
6. **Snapshot tests.** A new child module `src/cli/status_bar_snapshot_tests.rs`
   (its `#[cfg(test)] #[path]` line in `status_bar.rs`) renders the bar
   through `status_bar_line` and asserts the **whole line** at fixed widths.
   Build a full model like `status_bar_tests::full_model` with a fixed `now`
   and a fixed alert `first_seen` so ages are stable; model
   `deepseek-v4.1-flash` so the phase-11 clip shows. Tests, one per
   scenario, each asserting the exact string:
   - `snapshot_emoji_full_at_160_120_80_40`
   - `snapshot_emoji_working_approval_and_down_at_120`
   - `snapshot_emoji_info_alert_no_grants_unnamed_at_160_and_80` (the PE's
     own setup: here the words must be present at 160 **and** at 120)
   - `snapshot_symbols_full_at_160_and_80`
   - `snapshot_ascii_full_at_160_and_80`
   - `snapshot_words_survive_the_grants_detail_and_velocities`: a width at
     which the grants detail and the velocities are gone but every word is
     still present.
   The expected strings are **derived by running the code** after tasks 1–5
   and reading them back critically against this Goal: the words present
   with the detail gone, one counters slot, `?0.42` with no `❔`, no `/h/h`,
   no `t/s…t/s`, no emoji in symbols, `⚉`. Paste the derived strings into
   the tests, then into the Update Log.
7. **Existing tests.** Update every test that pins a string the tasks
   change; delete none. `the_symbols_and_ascii_dialects_use_the_same_words`,
   `no_dialect_doubles_a_word_or_a_dollar` (extend its forbidden list with
   `/h/h`, `t/s38`, `❔`) and `words_drop_all_at_once_before_any_rank_drop`
   (rename to `words_drop_all_at_once_at_their_rank`; it now asserts the
   words drop together **after** the detail and velocities) are the ones
   that must change.
8. **Docs.** The M37 README's decision 3 table: `?` prefix for unknown
   pricing, `»`/`›` for symbols throughput, `⚉` already there; add decision
   8 (A, B, C above, dated 2026-11-15). `README.md`'s status-bar paragraph
   follows.
9. **Evidence.** Mutation checks: (a) restore the pre-step words drop and
   confirm `snapshot_words_survive_the_grants_detail_and_velocities` fails;
   (b) put the four counters back in separate slots and confirm
   `snapshot_emoji_full_at_160_120_80_40` fails. Revert both. Paste the
   `test result:` lines and the § End-to-end output into one Update Log
   entry.

## Constraints

- The compact form of every part that this phase does not name is
  byte-for-byte unchanged.
- The state word never drops; the alert glyph and count never drop.
- Every line measures at most the pane width with `unicode-width`, at every
  width 30–200, in all three dialects (`a_line_never_exceeds_the_pane_width`
  stays green).
- No glyph gains U+FE0F/U+FE0E. `»` is U+00BB and `›` is U+203A, both one
  column.
- `status_bar.rs` stays under 800 production lines. Phase 11 left it at
  the cap, so make room first: move the rank loop into `status_bar_fit.rs`
  rather than compacting unrelated code or adding an allowlist line. No test file over 2000 lines.
- The OSC 7501 emitter and `ChatState` are untouched.

## Test plan

See tasks 6 and 7. All tests are pure: `status_bar_line` with a model, a
fixed `now`, the palette and a width.

## Acceptance criteria

- [ ] `grep -rlw "fn snapshot_words_survive_the_grants_detail_and_velocities" src`
      prints a file (today nothing), and the six snapshot tests exist.
- [ ] `grep -c 'cost_untracked' src/cli/status_glyphs.rs` prints 0 (today 4).
- [ ] `grep -c '🐇\|🐢\|⚦' src/cli/status_glyphs.rs` prints 2 (the emoji
      table only; today 5).
- [ ] At 120 columns, the emoji bar for the PE's setup (info alert, no
      grants, unnamed) contains `idle`, `ctx`, `turn` and `shells`; today it
      contains none of them. Pinned by the snapshot test.
- [ ] Both mutation checks pasted with their failing and passing
      `test result:` lines.
- [ ] fmt, build (0 warnings), clippy and `timeout 600 cargo test` with
      `failed=0` and passed greater than 3030; hygiene,
      `doc_truth` and `bug_tracker` ok.

## Pre-flight

Baseline is the tree after phase 11's approval: `passed=3030 failed=0`. Run every `cargo test` under `timeout 600`.

## End-to-end verification

```sh
timeout 600 cargo test --lib status_bar 2>&1 | grep -E '^test result'
grep -c 'cost_untracked' src/cli/status_glyphs.rs | sed 's/^/untracked_refs=/'
grep -c '🐇\|🐢\|⚦' src/cli/status_glyphs.rs | sed 's/^/emoji_in_tables=/'
cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
timeout 600 cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
```

## Authorizations

- New file `src/cli/status_bar_snapshot_tests.rs` and its `mod` line.
- Edit `src/cli/status_bar.rs`, `status_bar_fit.rs`, `status_words.rs`,
  `status_glyphs.rs`, `status_bar_tests.rs`, any other test pinning a bar
  string, `README.md` and the M37 README.

## Out of scope

- New slots, a config key for the drop order, the `top` and `/panes` views,
  and the alert-registry restart gap (held for a PE decision).

## Update Log

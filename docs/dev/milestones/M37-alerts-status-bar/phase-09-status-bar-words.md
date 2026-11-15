# Phase 09 — status-bar-words

**Milestone:** M37 — Alerts out of history, onto the status bar and `top`
**Status:** todo
**Drafted:** 2026-11-15
**Depends on:** phases 02a, 02b and 02c (the bar, the counters, the grants segment)
**Estimated diff:** ~400 lines, about half of it tests
**Tags:** language=rust, kind=feature, size=m

## Goal

A glyph alone does not tell the operator what a status item is. The bar
`💤 · ℹ1 Watchdog 11h · 🌒2% 📈1.4k · 💸0.00 ⛽0.0/h · ↻1 🐇123t/s · 🦙 deepseek-v4`
is unreadable without the legend. Every item gets a pithy word, in all three
glyph dialects, in a **wide form**. When the line does not fit, the words drop
first, all at once, back to today's glyph-and-number form (the **compact
form**); only then do the existing rank drops begin.

PE decision 7 (2026-11-15), amending decision 3's "every segment is a glyph
and a number": words describe each item; the state word and the alert
severity word are exempt from dropping.

## Architecture references

- `src/cli/status_bar.rs`: `status_bar_line` (builds the slots, then drops by
  rank until the line fits) and the per-slot builders `state_part`,
  `alert_parts`, `context_parts`, `cost_parts`, `turn_parts`, `counter_parts`,
  `model_parts`, `session_parts`. It is at 731 production lines against the
  800 cap, so the word logic goes in a new child module.
- `src/cli/status_bar_fit.rs`: `Part`, the ranks, `line_fits`, `join_slots`.
- `src/cli/status_grants.rs`: `grants_parts` (the right-hand segment).
- `src/cli/status_glyphs.rs`: `GlyphSet` and the `EMOJI` / `SYMBOLS` / `ASCII`
  tables; `elapsed` is the `⏱` mark the state word makes redundant.
- Tests: `src/cli/status_bar_tests.rs` pins today's strings.

## Spec

Numbered tasks in execution order. The task list is seeded from this section:
mark each `active` when you start it and `done` when its `cargo build` is
green. **Step 1 is a write**; make the first edit within 15 turns.

1. **The two-form `Part`** — a part carries its wide spans and its compact
   spans (equal for a part with no word). Its width is measured per form. The
   compact form of every part is **byte-for-byte what the bar renders today**,
   except the state part and the alert's severity part, which have one form.
   Put the word formatting in a new child module `src/cli/status_words.rs`.
2. **The words-drop step** — in `status_bar_line`, build the wide form; if the
   line (slots plus the grants segment) does not fit, switch **every** part to
   its compact form at once, never a mixed bar; then continue with the
   existing rank drops over the compact parts. The state word and the alert
   severity word stay at every width the rank drops leave them.
3. **The wide forms** (emoji dialect shown; the other two dialects use the
   same words with their own glyphs):

   | Item | Wide form | Compact (today) |
   |---|---|---|
   | state | `💤 idle` · `🧠 thinking 12s` · `✋ approve?` · `💀 daemon down` · `📡 reconnecting` | same (exempt) |
   | alert | `ℹ info Watchdog 11h` · with two or more firing at the worst severity `⚠ 2 warn disk-high 3m` · severity words `crit` / `warn` / `info`, and `alert` for an unrankable severity | the severity word stays; the count shows only at two or more; name and age drop by their existing ranks |
   | alert, resolved flash | `🧯 resolved Watchdog` | `🧯 Watchdog` |
   | context | `🌒 ctx 2%` | `🌒2%` |
   | context growth | `📈 +1.4k` | `📈1.4k` |
   | cost | `💸 $0.42` | `💸0.42` |
   | burn | `⛽ $0.9/h burn` | `⛽0.9/h` |
   | turn | `↻ turn 7` | `↻7` |
   | throughput | `🐇 123 tok/s` | `🐇123t/s` |
   | background | `⏳ 2 bg` | `⏳2` |
   | ghosts | `👻 1 ghost` / `👻 2 ghosts` | `👻1` |
   | tools | `🛠 3 tools` / `🛠 1 tool` | `🛠3` |
   | shells | `🐚 2 shells` / `🐚 1 shell` | `🐚2` |
   | model, session | unchanged | unchanged |

   The working state's `⏱` mark is gone: `thinking 12s` already reads as
   elapsed. Remove the now-unused `elapsed` field from `GlyphSet` and its
   three table entries.
4. **The grants segment** — wide forms, same words in every dialect:
   lead `⚡ grants 3` (colour rules unchanged); then per kind `🔒 gated`,
   `🔑 sudo`, `📜 scripts 2` / `📜 scripts all`, `📖 runbooks 2` /
   `📖 runbooks all`, `📄 files 2` / `📄 files all`, `📅 schedules`,
   `🤖 agents 2` / `🤖 agents all`, `🗑 deletes`, `🌐 1 host` / `🌐 2 hosts`.
   The wide `all` word replaces the compact `∗`. Hidden in the default state,
   flush right, drop ranks unchanged.
5. **Docs** — `README.md`'s status-bar bullet and the `/approvals` paragraph's
   sentence about the grants segment describe the words and the words-drop
   step; `CLAUDE.md`'s key-files row for the status bar names
   `status_words.rs`; the M37 README's decision 3 gets the decision-7 note.
6. **Capture the evidence** — run the mutation check and the § End-to-end
   block and paste both into one Update Log entry.

## Constraints

- **No doubled words or marks.** The `ascii` dialect's glyphs are already
  abbreviations (`ctx`, `bg`, `gh`, `tl`, `sh`), and the `symbols` cost glyph
  is `$` and `$/h`. A wide form must not read `ctx ctx 61%`, `bg 2 bg` or
  `$$0.42`: where the glyph already says the word, the wide form is the glyph
  followed by the value, or the word replaces the glyph. State which you chose
  in the Update Log.
- No glyph gains U+FE0F or U+FE0E. Every line is measured with
  `unicode-width`; a line never exceeds the pane width at any width tested.
- The compact form is the regression anchor: at a width where today's bar
  fits only after the words drop, the output equals today's output.
- `status_bar.rs` stays at or under 800 production lines; no new
  `tests/hygiene_allowlist.txt` line; no function over 150 lines.
- The state glyph and word stay in step with the OSC 7501 emitter
  (`ChatState`); do not change `ChatState` or the emitter.
- Tests are pure: `status_bar_line` takes the model, `now`, the palette and
  the width. No terminal, no HOME, no clock reads.

## Test plan

In `src/cli/status_bar_tests.rs`, calling `status_bar_line`:

- `wide_form_carries_the_words`: a busy model at 160 columns renders each
  Wide-form cell of the task-3 and task-4 tables, in the emoji dialect.
- `words_drop_all_at_once_before_any_rank_drop`: a width where wide does not
  fit but compact does renders today's string for every part, with no word
  left on any non-exempt part (no mixed bar).
- `state_and_alert_words_survive_every_drop`: at 50 columns the state word
  and the alert severity word are present; the rank drops took other parts.
- `an_idle_default_bar_has_no_grants_and_reads_idle`.
- `plural_forms_follow_the_count`: `1 shell` / `2 shells`, `1 ghost` / `2
  ghosts`, `1 tool` / `3 tools`, `1 host` / `2 hosts`; `scripts 2` stays a
  label then a count.
- `scope_all_reads_all_wide_and_star_compact`.
- `the_symbols_and_ascii_dialects_use_the_same_words`.
- Negative: `no_dialect_doubles_a_word_or_a_dollar` asserts the three
  dialects' output contains none of `ctx ctx`, `bg 2 bg`, `$$`.
- Negative: `an_unrankable_severity_reads_alert`.
- `a_line_never_exceeds_the_pane_width` over widths 30–200 in all three
  dialects.
- The existing glyph-table tests pass with `elapsed` removed.

## Acceptance criteria

- [ ] Every test in the test plan passes, and every other test in the repo
      that pinned a bar string is updated to the wide or compact form it now
      renders (none deleted).
- [ ] Mutation check, pasted with its `test result:` line: change the
      words-drop step so it never switches to the compact form, confirm
      `words_drop_all_at_once_before_any_rank_drop` fails, then revert.
- [ ] `grep -rn 'elapsed' src/cli/status_glyphs.rs` prints nothing.
- [ ] fmt, build (0 warnings), clippy and `cargo test` (under `timeout 600`)
      are green with `failed=0`; hygiene, `doc_truth` and `bug_tracker` ok.

## Pre-flight

Baseline at drafting (2026-11-15, `9d464951`): `cargo test` passed=2998,
failed=0. Run every `cargo test` under `timeout 600`.

## End-to-end verification

```sh
timeout 600 cargo test --lib status_bar 2>&1 | grep -E '^test result'
grep -rn 'elapsed' src/cli/status_glyphs.rs | wc -l | sed 's/^/elapsed_refs=/'
cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
timeout 600 cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
```

## Authorizations

- New file `src/cli/status_words.rs` and its `mod` line in `src/cli/status_bar.rs`.
- Edit `src/cli/status_bar.rs`, `status_bar_fit.rs`, `status_grants.rs`,
  `status_glyphs.rs`, `status_bar_tests.rs`, any test that pins a bar string,
  `README.md`, `CLAUDE.md`, and the M37 README's decision 3.

## Out of scope

- New glyphs, new slots, per-segment progressive word dropping, a config key
  for the words, and any change to the drop *rank order*.
- The `daemoneye top` and `/panes` views.

## Update Log

<!-- entries below this line -->

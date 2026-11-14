# Phase 02c — grants-segment

**Milestone:** M37 — Alerts out of history, onto the status bar and `top`
**Status:** in-progress
**Drafted:** 2026-11-14
**Depends on:** phase 02a (the `StatusModel` and the glyph dialects)
**Estimated diff:** ~300 lines, about half of it tests
**Tags:** language=rust, kind=feature, size=m

## Goal

The bar's grants segment speaks the same grammar as every other slot
(decision 3, amended 2026-11-14): a lead glyph coloured by the riskiest
grant, a count of the non-default grants, per-kind glyphs with counts when
room, hidden entirely in the default state, flush right. The segment is
built from a summary struct, not a pre-formatted string, so it speaks the
chosen glyph dialect and degrades by tier. Today it is a ~55-column
sentence (`⚡approvals: all, scripts: 2 · Ctrl+C revokes`) that ignores the
dialect and drops all at once.

## Spec

Numbered tasks in execution order; the executor's task list is seeded from
this section, so mark each `active` when you start it and `done` when its
`cargo build` is green.

1. **The summary struct** — in `src/cli/commands/approval.rs`, a pure
   `GrantSummary` (`Copy`, `Default`, `PartialEq`) with: `cmds:
   CmdGrant` (`Gated` / `Auto` / `Sudo` / `All`), one `Scope` (`None` /
   `Named(u32)` / `All`) each for `scripts`, `runbooks`, `files`, `agents`,
   `schedules: bool`, `deletes: bool`, and `hosts: u32` (distinct hosts in
   `hosts_regular ∪ hosts_sudo`). `SessionApproval::summary()` builds it;
   `hint()` is deleted along with its tests, replaced by
   `summary_counts_each_kind`. The three types live in
   `src/cli/status_bar.rs` (or a sibling the bar re-exports), and
   `approval.rs` imports them: the bar must not depend on
   `crate::cli::commands`.
2. **The model** — `StatusModel.approval_hint: String` becomes
   `StatusModel.grants: GrantSummary`. Every `sb.approval_hint =
   approval.hint()` site (`chat_loop.rs`, `stream_ask.rs`,
   `stream_prompts.rs`, `slash*.rs`, `agents.rs`; `cargo build` names them)
   becomes `sb.grants = approval.summary()`.
3. **The glyphs** — `GlyphSet` gains one field per grant kind, in all three
   dialects, none containing U+FE0F / U+FE0E; `grants` stays the lead:

   | field | emoji | symbols | ascii |
   |---|---|---|---|
   | `grants` (lead, exists) | ⚡ | ⚡ | `A` |
   | `grant_gated` | 🔒 | ⊘ | `!` |
   | `grant_sudo` | 🔑 | # | `#` |
   | `grant_scripts` | 📜 | § | `s` |
   | `grant_runbooks` | 📖 | ¶ | `r` |
   | `grant_files` | 📄 | ▤ | `f` |
   | `grant_schedules` | 📅 | ◷ | `c` |
   | `grant_agents` | 🤖 | ⚉ | `g` |
   | `grant_deletes` | 🗑 | ⌫ | `d` |
   | `grant_hosts` | 🌐 | ⌂ | `h` |
   | `grant_all` (the "every one" mark after a kind) | ∗ | ∗ | `*` |

   `glyph_sample` includes the new fields so `doctor` shows them.
4. **The segment** — `hint_part` becomes `grants_parts(model, g, palette)`.
   The *default state* (`cmds == Auto` and every other field at its
   `Default`) returns nothing: the segment is hidden like a zero counter.
   Otherwise:
   - **Core** (the current `RANK_HINT`): the lead glyph, then the count of
     active kinds — `Gated`, `Sudo`/`All` cmds, each non-`None` scope,
     `schedules`, `deletes`, and `hosts > 0` each count one. The lead glyph
     is **red** when `cmds == Sudo | All`, `deletes`, or any scope is `All`;
     **amber** when the only grants are `Named(_)` scopes or `hosts`;
     **green** when `cmds == Gated` and nothing else is active (`Palette`
     has `red()` and `yellow()` today; add `green()` beside them in the same
     shape). Colour sits on the glyph only, as the other slots do.
   - **Detail** (tier 3, a new `RANK_GRANT_DETAIL` that drops with the
     velocities, before `RANK_ALERT_NAME`): one part per active kind in the
     table's order, each the kind glyph followed by its count, `grant_all`
     for `All`, or nothing for a boolean kind. `Sudo` shows `grant_sudo`;
     `All` cmds shows `grant_sudo` too (regular is the default and carries
     no glyph). Parts are separated by a single space inside the segment.
   - The segment stays flush right, as `join_slots` places it today. No
     line ever contains `Ctrl+C`; the revoke hint stays in `/approvals`.
5. **Tests** — in `src/cli/status_bar_tests.rs` (all pure, `now` passed
   in): `grants_hidden_in_the_default_state`,
   `grants_lead_colour_follows_the_riskiest_grant` (green for gated-only,
   amber for one named script, red for `files: All`, red for `deletes`),
   `grants_count_active_kinds_and_show_detail_when_room` (a summary with
   sudo, two scripts, files all and deletes shows the count 4 and, at 160
   columns, each of the four kind glyphs in order),
   `grants_detail_drops_before_the_core` (narrow until the detail is gone
   and the core still shows, then until the core goes after the tier-2
   counters), `grants_speak_each_dialect` (the ascii line contains no `⚡`
   and shows `A`), `no_status_line_mentions_ctrl_c`. Update the existing
   `full_model()` fixture and the tier-order test to build a `GrantSummary`
   instead of a string. In `approval.rs`: `summary_counts_each_kind`.
6. **Docs** — the README's slot table row for `grants` and its 120/80
   examples; CLAUDE.md's `src/cli/` row if it names the hint.
7. **Capture the mutation check and the end-to-end evidence** — run the
   mutation check named in § Acceptance criteria and the block in
   § End-to-end verification verbatim, and paste both outputs into one
   Update Log entry.

## Constraints

- Pin behaviour, not pixels: what a line contains or omits at a width, and
  its measured width, never a full string of the bar.
- Tests call the production seam (`status_bar_line`, `summary()`); a copy
  of the colour or count rule inside a test is a fake test.
- The `/approvals` command's body and the approval prompt panel are
  unchanged.
- No glyph in the table collides with an existing `GlyphSet` field's glyph
  in the same dialect; `glyph_sets_contain_no_variation_selectors` must
  still pass.

## Notes for executor

**Symbol map.** Every one of these exists today:
- `src/cli/commands/approval.rs`: `SessionApproval`, `hint`,
  `host_approval_lines`, `approvals_list_body`.
- `src/cli/status_bar.rs`: `StatusModel` (`approval_hint`), `hint_part`,
  `alert_parts` (the shape to copy: a lead glyph with colour, a count, detail
  parts at a lower rank), `glyph_sample`, `gray`.
- `src/cli/status_bar_fit.rs`: the `RANK_*` constants (`RANK_HINT` = 12,
  the tier-3 ranks 1–7), `line_fits`, `join_slots`.
- `src/cli/status_glyphs.rs`: `GlyphSet`, `EMOJI`, `SYMBOLS`, `ASCII`.
- `src/cli/status_bar_tests.rs`: `full_model`, `line_at`, `line_width`,
  `status_line_drops_tier_three_before_tier_two_before_the_hint`.
- `src/cli/palette.rs`: `Palette` (`red()`, `yellow()`; the context slot
  uses both).

**Edit order.** Each step ends at a green `cargo build`:
1. **First write:** the `GrantSummary` types and `summary()`; keep `hint()`
   for this step so nothing else breaks.
2. The glyph fields in all three dialects and `glyph_sample`.
3. `StatusModel.grants`, `grants_parts`, the new rank; switch every call
   site; delete `hint()` and its tests.
4. The new tests and fixture update.
5. Docs.
6. Spec task 7: the mutation check and the end-to-end block, pasted.

**Your first edit lands within your first 15 turns.** Never run `tmux`.
Never repeat a read whose output you already have; when unsure of a call
site, let `cargo build` name it.

## Test plan

The seven tests named in Spec task 5, plus `summary_counts_each_kind`.

## Acceptance criteria

- [ ] Every test in the test plan passes.
- [ ] `grep -c 'approval_hint' src/cli/status_bar.rs` prints 0 (today 4).
- [ ] `grep -c 'fn hint' src/cli/commands/approval.rs` prints 0 (today 5:
      `hint` and its four tests).
- [ ] `grep -c 'Ctrl+C revokes' src/cli/commands/approval.rs` prints 0
      (today 1).
- [ ] `grep -c 'grant_sudo' src/cli/status_glyphs.rs` prints 4 (the field
      and three dialects; today 0).
- [ ] Mutation check, pasted with its `test result:` line: make the lead
      colour ignore `deletes`, confirm
      `grants_lead_colour_follows_the_riskiest_grant` fails, then revert.
- [ ] fmt, build (0 warnings), clippy and `cargo test` are green. Hygiene,
      `doc_truth` and `bug_tracker` are ok.

## End-to-end verification

Run mechanically, with output redirected to a file and pasted:

```sh
cargo test --lib grants 2>&1 | grep -E '^test result'
cargo test --lib summary_counts_each_kind 2>&1 | grep -E '^test |^test result'
grep -c 'approval_hint' src/cli/status_bar.rs | sed 's/^/approval_hint=/'
grep -c 'Ctrl+C revokes' src/cli/commands/approval.rs | sed 's/^/ctrl_c=/'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
```

## Authorizations

- Edit the files in the symbol map, every file `cargo build` names for the
  `approval_hint` → `grants` switch, `README.md` and `CLAUDE.md`.
- Add `RANK_GRANT_DETAIL` to `status_bar_fit.rs` and renumber the tier-3
  ranks if needed (they are relative).

## Out of scope

- `/approvals` output, the approval prompt panel, and how grants are made
  or revoked.
- The counters (phase 02b) and `top` (phase 03).

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-14 05:47 (started)

Executor Claude (direct) starting phase 02c. Plan: the `GrantSummary` types
first (keeping `hint()` until the switch compiles), then the glyph fields,
then the `StatusModel.grants` switch and `grants_parts`, then the tests and
docs.

# Phase 13: top-focus — one focused panel, every panel scrolls

**Milestone:** M30 — Telemetry & dashboard
**Status:** in-progress
**Depends on:** phase-12
**Estimated diff:** ~450 lines, more than half of it tests
**Tags:** language=rust, kind=feature, size=m

## Goal

The live `top` screen has one cursor: `↑↓` always moves the Runs
selection. Shells and Activity cannot scroll, so a long Shells list is cut
at its 6-line cap and Activity only ever shows its newest rows. This phase
adds a focused panel. Keys move focus, the arrow keys scroll whichever
panel has it, and Activity tails new rows until you scroll it. `--once` is
unchanged.

## Spec

**Files:** `src/cli/commands/top_tui.rs`, `README.md` (the live-view
sentences of the `top` paragraph).

**Where things are.**

| Need | Symbol |
|---|---|
| State and keys | `TopModel`, `TopModel::handle_key`, `TopModel::filtered_activity` |
| Drawing | `draw_at`, `draw_panel`, `runs_window`; the Budget block is drawn inline in `draw_at` |
| Panel sizes | `plan_layout` and `PanelRects` in `top.rs`; inner height is the rect's height minus 2 |
| The loop | `run_tui` |

1. **Focus.** `pub enum Panel { Runs, Budget, Shells, Activity }`, in that
   order. `TopModel` gains `focus: Panel`, starting at `Runs`. `Tab` moves to
   the next panel and wraps from Activity to Runs. `Shift-Tab`
   (`KeyCode::BackTab`) moves back and wraps from Runs to Activity. `1` to
   `4` jump to Runs, Budget, Shells and Activity. While the filter is being
   edited, these keys go to the filter text as they do today.
2. **Showing focus.** Every panel title carries its number: ` 1 Runs `,
   ` 2 Budget `, ` 3 Shells `, ` 4 Activity `. The focused panel's border
   and title are cyan. The footer names it as `focus: <Panel>` and adds
   `tab focus` to the key hints.
3. **Scroll keys act on the focused panel only.** `↑`/`k`, `↓`/`j`,
   `PageUp`, `PageDown`, `Home` and `End`. A page is the panel's inner
   height minus one, at least 1.
   - **Runs:** the keys move the selection as `↑↓` do today, and any of
     them stops follow. `Home` selects the first run, `End` the last.
   - **Budget:** the keys do nothing.
   - **Shells:** `TopModel` keeps the index of the first visible body line.
     `Home` is the top. `End` and `PageDown` stop at the last full window.
   - **Activity:** see item 4.
4. **Activity tails until scrolled.** While tailing, the panel shows its
   newest rows, as today. `↑`, `PageUp` or `Home` stops tailing and fixes
   the top visible row. While it is not tailing, new rows arriving do not
   move the visible rows. Scrolling down until the newest row is visible
   resumes tailing, and so does `End`. The footer says `tailing` or
   `activity scrolled`.
5. **`f`** resumes Runs follow and Activity tailing together, from any
   focus. Shells keeps its scroll.
6. **Clamping uses real panel heights.** Key handling needs each panel's
   inner height, which only the layout knows. `TopModel` stores the inner
   heights of Runs, Shells and Activity from `plan_layout` for the current
   terminal size. The loop refreshes them before it handles keys and after
   each resize. Every offset is clamped against them, and against the
   content after a snapshot refresh or a filter change. Consequence: one
   key press past an end moves nothing, and one press back moves one line.
   No offset may exceed what the panel can show, and no subtraction may
   underflow, at any size including a panel with no lines.
7. **Drawing** uses the stored offsets. The Shells window and the
   not-tailing Activity window come from one pure helper,
   `visible_range(total: usize, inner_h: usize, top: usize) -> Range<usize>`,
   that clamps `top` so the range always lies inside `0..total`.
8. **README:** name focus, its keys, per-panel scrolling, tailing and what
   `f` resumes.

**Gotcha.** Phase 12 bounced twice on screen code that no test drew: a
panel with more lines than fit, a value at a threshold, and a `usize`
subtraction that underflowed when a window was near its top. Each test
below names one of those edges. Write them first, and make each fail
against the current tree before you write the code.

## Tests

All in `top_tui.rs`. Key tests drive `handle_key` on a `TopModel` whose
stored heights the test sets.

- `focus_cycles_with_tab_and_wraps`: four `Tab` presses return to Runs;
  `BackTab` from Runs gives Activity.
- `number_keys_jump_focus`, and `number_keys_type_into_the_filter_while_editing`
  (in edit mode `3` appends `3` to the filter and focus does not change).
- `arrows_move_only_the_focused_panel`: with Shells focused, `↓` changes
  the Shells offset and leaves the Runs selection and follow unchanged.
  With Budget focused, every scroll key changes nothing.
- `shells_scroll_clamps_at_both_ends`: 20 shell lines, inner height 6.
  `↑` at the top stays 0. `End` gives 14. `↓` at 14 stays 14, and one `↑`
  then gives 13. With 3 lines, `End` gives 0.
- `activity_tails_until_scrolled`: while tailing, a refresh that adds rows
  shows the newest. After one `↑`, a refresh that adds 5 rows leaves the
  same top row visible.
- `activity_scrolling_to_the_bottom_resumes_tailing`, also via `End`.
- `f_resumes_follow_and_tailing`: from Shells focus, after scrolling Runs
  and Activity away.
- `page_and_home_keys_move_by_a_page`: 100 Activity rows, inner height 10.
  `Home` shows row 0 at the top. `PageDown` moves the top by 9. `End`
  resumes tailing.
- `empty_panels_ignore_scroll_keys`: no runs, no shells and no activity.
  Every scroll key on every focus leaves every offset at 0 without a panic.
- `offsets_clamp_when_content_or_size_shrinks`: Activity scrolled to a top
  of 80 of 100 rows, then a filter that leaves 5 rows, then stored heights
  halved. Each step leaves a range inside the content.
- `visible_range_never_leaves_the_content`: `total` 0, `total` under
  `inner_h`, `top` past the end, `inner_h` 0. Every range lies inside
  `0..total`.
- `screen_names_the_focused_panel`: on an 80×24 `TestBackend` with Shells
  focused, the Shells title cells are cyan, the Runs title cells are not,
  and the footer holds `focus: Shells`.
- `screen_draws_scrolled_shells`: 10 shells at offset 2 show the third
  shell's id on the first body row and not the first shell's.

The existing `screen_*` and key tests keep passing. Update their
expectations only where they pinned the old titles or footer.

## Acceptance criteria

- [ ] Each test above ⇒ `test result: ok. 1 passed` (today none exists).
- [ ] On the § End-to-end harness, `once_order=Runs,Budget,Shells,Activity`
  and `once_rate=1` (preservation: `--once` is unchanged).
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` clean,
  `cargo fmt --all --check` exit 0, `cargo test` ⇒ `failed=0`. Report the
  passed count; do not pin it.

## End-to-end verification

Run this from a scratch file outside the repo, for example
`/tmp/e2e-m30-13.sh`, and paste its output verbatim into your Update Log
entry. It fabricates no events and does not depend on the clock.

```sh
A=/tmp/e2e-m30-13.txt; : > "$A"
for t in focus_cycles_with_tab_and_wraps number_keys_jump_focus number_keys_type_into_the_filter_while_editing arrows_move_only_the_focused_panel shells_scroll_clamps_at_both_ends activity_tails_until_scrolled activity_scrolling_to_the_bottom_resumes_tailing f_resumes_follow_and_tailing page_and_home_keys_move_by_a_page empty_panels_ignore_scroll_keys offsets_clamp_when_content_or_size_shrinks visible_range_never_leaves_the_content screen_names_the_focused_panel screen_draws_scrolled_shells; do cargo test "$t" 2>&1 | grep -E '^test result: ok. 1 passed' | head -1 | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
cargo build -q 2>/dev/null
H=$(mktemp -d); mkdir -p "$H/.daemoneye/var/log/events" "$H/.daemoneye/var/log/runs"
F=$(TZ=Etc/GMT+5 HOME="$H" ./target/debug/daemoneye top --once)
echo "once_order=$(echo "$F" | grep -xE 'Runs|Budget|Shells|Activity' | paste -sd, -)" >> "$A"
echo "once_rate=$(echo "$F" | grep -c '^rate (60 s): ')" >> "$A"
find "$H" -type f -delete; find "$H" -depth -type d -empty -delete
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

The reviewer smoke-tests the live screen against real run history. Phase
10's L8 pass checks it by eye after phase 14.

## Authorizations

- Add fields to `TopModel` and update its constructor and every test that
  builds one.
- Change `draw_panel`'s signature to take a focused flag or a title style.
- Change `runs_window`'s callers as needed. Keep its window arithmetic in
  that one function.

## Out of scope

- The run detail view and `Enter` (phase 14).
- Mouse input.
- Any change to `--once` output, `plan_layout`'s rules or `top.rs` text.
- Any write to the daemon; `top` stays read-only.

## Notes for executor

Keep reasoning short: emit a tool call early in each turn. Do not wait on
the wall clock or sleep; nothing in this phase needs it.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-24 21:26 (started)

**By:** executor (Claude)

Flipped the phase to `in-progress` and the README's phase-table row to
match. Reading the current `top_tui.rs` and `top.rs`, then writing the
tests the spec lists before the focus/scroll code.

### Update — 2026-10-24 22:10 (end-to-end verification)

**By:** executor (Claude)

Ran the phase harness from `/tmp/e2e-m30-13.sh` (scratch file outside the
repo, no fabricated events, no clock dependency):

```
focus_cycles_with_tab_and_wraps: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
number_keys_jump_focus: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
number_keys_type_into_the_filter_while_editing: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
arrows_move_only_the_focused_panel: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
shells_scroll_clamps_at_both_ends: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
activity_tails_until_scrolled: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
activity_scrolling_to_the_bottom_resumes_tailing: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
f_resumes_follow_and_tailing: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
page_and_home_keys_move_by_a_page: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
empty_panels_ignore_scroll_keys: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
offsets_clamp_when_content_or_size_shrinks: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
visible_range_never_leaves_the_content: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
screen_names_the_focused_panel: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
screen_draws_scrolled_shells: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
once_order=Runs,Budget,Shells,Activity
once_rate=1
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2549 failed=0
```

`--once` preservation holds: the frame still prints the four section
heads in order and exactly one `rate (60 s):` line. Clippy is clean with
`-D warnings`, `cargo fmt --all --check` exits 0, and the whole suite is
`passed=2549 failed=0`.

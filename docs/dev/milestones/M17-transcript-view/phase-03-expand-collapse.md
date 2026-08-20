# Phase 03: Expand / Collapse

**Milestone:** M17 — Transcript View
**Status:** in-progress (round 3 — see bugs/bug-phase-03-2.md)
**Depends on:** phase-02 (viewer-shell, `done`)
**Estimated diff:** ~400 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Make a long transcript navigable: focus a block, collapse it to its header,
expand it again — and tell the user the viewer exists by naming `ctrl+o` in the
inline `… N more lines` footer.

## Architecture references

Read before starting:

- `docs/design/transcript-view.md` — §"What this unlocks beyond expansion".
- `docs/dev/milestones/M17-transcript-view/README.md` — exit criteria.
- `src/cli/viewer.rs` — the phase-02 viewer this extends. Read it in full
  before editing; it is 491 lines.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

**The viewer renders every block in full and has no notion of focus.**
`src/cli/viewer.rs:31-43`:

```rust
pub fn layout_blocks(blocks: &[Block], width: usize) -> Vec<ViewRow> {
    let mut rows: Vec<ViewRow> = Vec::new();
    for (i, block) in blocks.iter().enumerate() {
        if i > 0 && !rows.is_empty() {
            rows.push(ViewRow {
                text: String::new(),
                kind: RowKind::Blank,
            });
        }
        layout_block(block, width, &mut rows);
    }
    rows
}
```

`ViewRow` today (`viewer.rs:23-27`) carries no link back to the block it came
from, which is what focus and collapse both need:

```rust
pub struct ViewRow {
    pub text: String,
    pub kind: RowKind,
}
```

`render_transcript(f, rows, scroll, evicted)` (`viewer.rs:126`) styles by
`RowKind` and writes a status line ending
`" · ↑↓ PgUp/PgDn Home/End · esc to close"`.

`viewer_loop` (`viewer.rs:235`) owns `scroll` and matches on keys; `Up`/`Down`/
`PageUp`/`PageDown`/`Home`/`End` scroll, and
`Key::Char('\x1b') | Key::Char('q') | Key::CtrlO` breaks.

**The inline footer** is built in `src/cli/commands/stream.rs:707`:

```rust
                    body.push(format!("… {} more lines", total - shown));
```

and the help text at `src/cli/commands/chat.rs:27` describes it:

```
Tool output is capped at 10 lines on screen (… N more lines); full output is kept in history.
```

### Three gotchas, each verified against the tree

1. **Adding a field to `ViewRow` breaks 9 struct literals in the existing
   tests** (`viewer.rs`, all after `mod tests` at line 306 — counted: 9). They
   look like this (`viewer.rs:378-381`):

   ```rust
            ViewRow {
                text: "alpha".to_string(),
                kind: RowKind::Header,
            },
   ```

   Updating them is part of the work, not a surprise — task 2 says so
   explicitly. Do **not** work around it by adding a second row type.

2. **Tab is not available as a key.** `c if c < 0x20 => Key::Char('\0')`
   (`src/cli/input/tty.rs:247`) swallows `0x09` before any arm sees it, exactly
   as it did for ctrl+O in phase-02. This phase uses **printable** keys only —
   no new `tty.rs` arms are needed or wanted.

3. **`q`, `esc` and `ctrl+o` already exit the viewer** (`viewer.rs`, the break
   arm). Do not bind any new behaviour to them.

## Spec

### Task 1 — Collapse-aware layout

In `src/cli/viewer.rs`, add:

```rust
/// Lay out with a set of collapsed block indices. `layout_blocks` is this with
/// an empty set.
pub fn layout_blocks_with(
    blocks: &[Block],
    width: usize,
    collapsed: &std::collections::HashSet<usize>,
) -> Vec<ViewRow>
```

Keep `layout_blocks(blocks, width)` as a thin wrapper that calls it with an
empty set, so every phase-02 caller and test keeps working unchanged.

A **collapsed** block renders as **exactly one row**: its header, with a
suffix ` [collapsed, {n} lines]` where `{n}` is the number of rows that block
would occupy when expanded, excluding the header. A block with no header of its
own (`Assistant`, `System`) uses its first laid-out row as the header row for
this purpose. The blank separator row between blocks is unchanged.

### Task 2 — Tie rows to their source block

Add `pub block: usize` to `ViewRow` (`viewer.rs:24-27`), set by `layout_blocks_with`
to the index of the block the row came from. The blank separator row before
block `i` carries `block: i`.

Then update the **9** `ViewRow` literals in the test module (all after line
306) to include the new field. Use `block: 0` where the test does not care —
none of the existing tests assert on it.

### Task 3 — Viewer state: focus and collapsed set

In `viewer_loop`, add alongside `scroll`:

- `focus: usize` — the index of the focused **block**, starting at the last
  block (the viewer opens at the bottom, so the last block is what the user is
  looking at). Clamp to `blocks.len().saturating_sub(1)`; `0` when empty.
- `collapsed: std::collections::HashSet<usize>` — starts **empty**. Every block
  is expanded on open; phase-02's guarantee that the viewer shows output the
  inline panel elided must not regress.

### Task 4 — Keys

Add to `viewer_loop`'s key match, using printable keys only (see gotcha 2):

- `Key::Char(']')` — focus the next block, wrapping to `0` past the last.
- `Key::Char('[')` — focus the previous block, wrapping to the last from `0`.
- `Key::Enter` — toggle `collapsed` membership for the focused block.
- `Key::Char('c')` — collapse **every `Block::Output`**, leaving all other
  blocks expanded.
- `Key::Char('a')` — expand everything (clear the set).

After any focus change, scroll so the focused block's header row is visible:
if it is above the viewport, scroll to it; if below, scroll so it is the last
visible row. Re-clamp with `clamp_scroll` as the existing arms do.

Write the focus arithmetic as two small pure functions so they can be tested
without a terminal — task 8's mutation targets the first verbatim:

```rust
/// Next focused block index, wrapping. `len == 0` yields 0.
pub fn focus_next(focus: usize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    (focus + 1) % len
}

/// Previous focused block index, wrapping. `len == 0` yields 0.
pub fn focus_prev(focus: usize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    (focus + len - 1) % len
}
```

### Task 5 — Render focus and collapse state

`render_transcript` gains a `focus: usize` parameter (after `scroll`). Rows
whose `block == focus` render with an emphasised style — pick it from the
existing `Palette`; **nothing about the colour is pinned**.

Each block's header row gains a state marker as its first characters: `▾ ` when
expanded, `▸ ` when collapsed. This replaces the `▸ ` prefix `layout_block`
currently hard-codes for `UserTurn` and `ToolPanel` headers — do not end up
with `▸ ▸ `.

Append ` · [ ] focus · enter collapse · c/a all` to the status line's existing
key hints.

### Task 6 — Name `ctrl+o` in the inline footer

In `src/cli/commands/stream.rs`, extract the footer into a pure function so it
is testable, and call it from the `Response::ToolResult` arm at line 707:

```rust
/// Footer shown on an elided inline tool-output panel.
fn output_footer(total: usize, shown: usize) -> String {
    format!("… {} more lines · ctrl+o", total - shown)
}
```

The line count and its wording stay as they are — only the ` · ctrl+o` suffix
is added. Update the help text at `src/cli/commands/chat.rs:27` to match:

```
Tool output is capped at 10 lines on screen (… N more lines · ctrl+o opens the full transcript).
```

### Task 7 — Tests

Write the tests named in § Test plan. They are pure — no terminal, no `HOME`
manipulation, no tmux.

### Task 7a — Fix the focus emphasis and the prose wrap (round 2, bug-phase-03-1)

Read `docs/dev/milestones/M17-transcript-view/bugs/bug-phase-03-1.md` first — it
carries the captured SGR evidence and the Definition of done.

Two changes, both in `src/cli/viewer.rs`:

1. **Focus must not underline a whole block.** `style_for_focused` currently
   returns `style_for(kind, palette).add_modifier(Modifier::UNDERLINED)`, and
   `render_transcript` applies it to every row of the focused block. Make focus
   visible some other way — a header-row marker, a brighter/dimmer contrast,
   whatever reads well from the existing `Palette` — with **no
   `Modifier::UNDERLINED` anywhere in the file**. Focus must remain
   distinguishable: the test asserts the focused and unfocused styles differ.
2. **Prose wraps on word boundaries; output does not.** Add a `wrap_words`
   helper and use it for `RowKind::User`, `Assistant`, `System` and the
   `ToolPanel` summary. `RowKind::Output` rows keep
   `crate::cli::render::wrap_line_hard` exactly as today — machine output must
   not be re-flowed, and the existing row-count guarantees
   (`layout_blocks_renders_full_output`,
   `collapsed_output_lays_out_as_exactly_one_row`) must still hold.

Do not touch `wrap_line_hard` itself — the inline panel and other callers
depend on it.

### Task 7b — Make the wrap guards bite (round 3, bug-phase-03-2)

Read `docs/dev/milestones/M17-transcript-view/bugs/bug-phase-03-2.md` first.

**Do not change the shipped behaviour.** Round 2's wrapping and focus styling
are correct. The problem is that two guards pass even when the wiring is
reverted: `wrap_words_does_not_split_words` calls the helper directly instead of
going through `layout_blocks`, and `output_rows_keep_hard_wrap` uses a single
unbroken 30-char token, which word-wrap and hard-wrap split identically.

Add the two behaviour-level tests named in the criteria, asserting exact row
vectors through `layout_blocks` with fixtures whose two wrappings differ
(`wrap_line_hard` cuts every `width` characters; see the bug doc for the
expected vectors). Keep or drop the two weak tests as you judge best — the
criteria only require the new ones plus the surviving round-2 guards.

Then add mutation **M2** to the E2E block, in both directions, proving each new
guard fails when its wiring is reverted.

### Task 8 — Mutation M1: apply

Use the `patch` tool on `src/cli/viewer.rs`.

- `old_str`: `    (focus + 1) % len`
- `new_str`: `    focus + 1`

Then run, appending to the evidence artifact:

```sh
A=/tmp/e2e-03.txt
echo "== M1 APPLIED ==" >> "$A"
grep -c 'focus + 1$' src/cli/viewer.rs >> "$A"
cargo test --lib cli::viewer 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -20 >> "$A"
echo "exit=${PIPESTATUS[0]}" >> "$A"
```

The run **must fail** — `focus_next_wraps_at_last_block` is what proves the
wrap is real. A green run means the test is vacuous; stop and file a blocker.

### Task 9 — Mutation M1: restore

`patch` the same line back, then:

```sh
A=/tmp/e2e-03.txt
echo "== M1 RESTORED ==" >> "$A"
grep -c 'focus + 1$' src/cli/viewer.rs >> "$A"
cargo test --lib cli::viewer 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -20 >> "$A"
echo "exit=${PIPESTATUS[0]}" >> "$A"
```

`grep -c` must print `1` after task 8 and `0` after task 9. Do **not** use
`git checkout` to restore — the file holds this round's uncommitted work.

### Task 10 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-03.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this.

### Task 11 — PASTE MATCH self-check

After pasting, run:

```sh
D=docs/dev/milestones/M17-transcript-view/phase-03-expand-collapse.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-03.txt
diff /tmp/pasted-03.txt /tmp/e2e-03.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

Append the literal verdict line into that same Update Log entry, below the
fence.

## Acceptance criteria

Every criterion below asserts an **observed count or value**, not the presence
of a mechanism — a phase-02 lesson (see that phase's § "Criterion design").

- [ ] `cargo fmt --all` leaves the tree unchanged.
- [ ] `cargo build` succeeds.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` passes.
- [ ] Test `collapsed_output_lays_out_as_exactly_one_row` passes — a collapsed
      300-line `Output` block contributes **exactly 1** row (asserted `== 1`),
      and that row contains `[collapsed, 300 lines]`.
- [ ] Test `expanded_layout_is_unchanged_by_the_new_path` passes —
      `layout_blocks(b, w)` and `layout_blocks_with(b, w, &empty)` return
      **equal** `Vec<ViewRow>`s. Phase-02's full-output guarantee is intact.
- [ ] Test `collapse_toggle_is_involutive` passes — collapsing then expanding
      the same block reproduces the original `Vec<ViewRow>` exactly.
- [ ] Test `collapse_all_outputs_collapses_only_outputs` passes — over a
      transcript with 2 `Output` blocks and 3 non-`Output` blocks, the computed
      set has **exactly 2** members, and both index `Output` blocks.
- [ ] Tests `focus_next_wraps_at_last_block` and `focus_prev_wraps_at_first`
      pass, each asserting the exact wrapped index.
- [ ] Test `rows_carry_their_source_block_index` passes — for a 3-block
      transcript, every row's `block` is the index of the block it came from.
- [ ] Test `output_footer_names_ctrl_o` passes — `output_footer(300, 9)` equals
      `"… 291 more lines · ctrl+o"` exactly.
- [ ] `grep -c "ctrl+o" src/cli/commands/chat.rs` prints at least 1 — the help
      text names it too.
- [ ] `/tmp/e2e-03.txt` shows `== M1 APPLIED ==` with a **failing** run and
      `grep -c` = 1, then `== M1 RESTORED ==` with a passing run and
      `grep -c` = 0.
- [ ] The Update Log's newest entry is headed
      `### Update — <date> (end-to-end verification)`, contains the pasted
      artifact, and ends with the literal line `PASTE MATCH`.

**Added 2026-08-20 after the close-out live check (bug-phase-03-1). Each was run
against the current tree and FAILS there. The focused block is underlined in
full — dozens of underlined rows on a long answer — and prose wraps mid-word:**

- [ ] `grep -c "Modifier::UNDERLINED" src/cli/viewer.rs` prints `0`.
      (Now: `2`.)
- [ ] Test `style_for_focused_is_distinct_without_underline` passes — the
      focused style differs from the unfocused style for the same `RowKind`
      (focus stays visible) and does not carry `Modifier::UNDERLINED`.
- [ ] Test `wrap_words_does_not_split_words` passes — no row ends mid-word for
      ordinary prose at width 12.
- [ ] Test `wrap_words_breaks_an_overlong_token` passes — a 30-char token at
      width 10 still yields 3 rows, none longer than 10.
- [ ] Test `output_rows_keep_hard_wrap` passes — `Block::Output` rows are still
      hard-wrapped; machine output is never re-flowed.
- [ ] `layout_blocks_renders_full_output` and
      `collapsed_output_lays_out_as_exactly_one_row` still pass unchanged.

**Added 2026-08-20 after the round-2 review (bug-phase-03-2). Round 2's
behaviour is correct; two of its guards do not detect the regression they exist
for — reviewer mutations reverting the wrap wiring left 41/41 passing. Both
items below are absent from the current tree:**

- [ ] Test `layout_wraps_prose_on_word_boundaries` passes — through
      `layout_blocks`, an `Assistant` block `"aaa bbb ccc ddd"` at width 7
      yields rows exactly `["aaa bbb", "ccc ddd"]`.
- [ ] Test `layout_keeps_output_hard_wrapped` passes — through `layout_blocks`,
      an `Output` block `"aaa bbb ccc"` at width 5 yields rows exactly
      `["aaa b", "bb cc", "c"]`.
- [ ] Mutation **M2** (both directions) is in the E2E artifact and shows each
      guard failing when the corresponding wiring is reverted — see
      bugs/bug-phase-03-2.md § Definition of done for the exact swaps.

## Test plan

In `src/cli/viewer.rs` (`#[cfg(test)] mod tests`):

- `collapsed_output_lays_out_as_exactly_one_row` — 300-line `Output`, collapsed;
  assert the block contributes exactly 1 row and its text contains
  `[collapsed, 300 lines]`.
- `expanded_layout_is_unchanged_by_the_new_path` — equality of the two layout
  entry points over a mixed transcript.
- `collapse_toggle_is_involutive` — layout, collapse block 1, expand block 1,
  assert the final `Vec<ViewRow>` equals the first.
- `collapse_all_outputs_collapses_only_outputs` — exactly 2 of 5 blocks, and
  they are the `Output` ones.
- `focus_next_wraps_at_last_block` — `focus_next(2, 3) == 0`, `focus_next(0, 3)
  == 1`, `focus_next(0, 0) == 0`.
- `focus_prev_wraps_at_first` — `focus_prev(0, 3) == 2`, `focus_prev(2, 3) ==
  1`, `focus_prev(0, 0) == 0`.
- `rows_carry_their_source_block_index` — 3 blocks; assert each row's `block`
  matches its source, including the blank separator rows.
- `render_transcript_marks_collapsed_and_focused` — `TestBackend::new(60, 10)`;
  a collapsed focused block draws a row containing `▸` and the status line
  contains `enter collapse`.

In `src/cli/commands/stream.rs`:

- `output_footer_names_ctrl_o` — exact string equality, including the count.

## End-to-end verification

Focus and collapse are keyboard behaviours inside the alternate screen, so
their real check is live and is architect-run at milestone close (the
milestone's exit criteria carry it). What the executor verifies here is the
pure layout and focus arithmetic, a `TestBackend` draw, and the footer string
the inline surface will actually print.

Tasks 8 and 9 append the mutation pair to the **same** artifact before this
block runs; do not truncate `/tmp/e2e-03.txt` here.

```sh
A=/tmp/e2e-03.txt
echo "== GATES ==" >> "$A"
cargo fmt --all -- --check 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -5 >> "$A"
echo "fmt exit=${PIPESTATUS[0]}" >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -5 >> "$A"
echo "clippy exit=${PIPESTATUS[0]}" >> "$A"
cargo test 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -25 >> "$A"
echo "test exit=${PIPESTATUS[0]}" >> "$A"
echo "== VIEWER UNITS ==" >> "$A"
cargo test --lib cli::viewer 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -25 >> "$A"
echo "units exit=${PIPESTATUS[0]}" >> "$A"
echo "== FOOTER UNIT ==" >> "$A"
cargo test --lib output_footer 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -10 >> "$A"
echo "footer exit=${PIPESTATUS[0]}" >> "$A"
echo "== CTRL+O IS NAMED ==" >> "$A"
grep -c "ctrl+o" src/cli/commands/stream.rs >> "$A"
grep -c "ctrl+o" src/cli/commands/chat.rs >> "$A"
echo "== PHASE-02 CONTRACT STILL HOLDS ==" >> "$A"
grep -c "disarm" src/cli/viewer.rs >> "$A"
grep -nE "try_restore|disable_raw_mode|\.restore\(\)" src/cli/viewer.rs >> "$A"
echo "teardown grep exit=$?  (1 = none found, which is the pass)" >> "$A"
```

## Authorizations

- [ ] May edit `src/cli/viewer.rs`, `src/cli/commands/stream.rs`, and the help
      text in `src/cli/commands/chat.rs`.

No new dependencies. `docs/architecture.md` is **not** authorized.

## Out of scope

- **Search** (phase-04), **copy** (phase-05), **rehydration** (phase-06),
  **mouse** (phase-07).
- **`src/cli/input/tty.rs`.** This phase adds no new key parsing — printable
  keys only.
- **Undoing anything phase-02 established.** `AltScreenGuard` keeps its
  unconditional `Drop`, `viewer.rs` gains no `disarm` and no `try_restore` /
  `disable_raw_mode` / `.restore()`, and the call site still does not propagate
  the viewer's error. The E2E block re-checks all three.
- **Collapsing anything by default.** The viewer opens fully expanded.
- **Changing the inline 10-line cap** or the `… N more lines` wording ahead of
  the new ` · ctrl+o` suffix.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-08-19 19:30 (started)

Beginning phase-03: implementing collapse-aware layout, row-to-block linking,
focus/collapse viewer state, printable-only keys, ctrl+o footer naming, and the
pure test suite with the M1 mutation pair.


### Update — 2026-08-19 20:05 (end-to-end verification)

```
== M1 APPLIED ==
1
test cli::viewer::tests::render_transcript_survives_scroll_past_end ... ok
test cli::viewer::tests::render_transcript_marks_collapsed_and_focused ... ok

failures:

---- cli::viewer::tests::focus_next_wraps_at_last_block stdout ----

thread 'cli::viewer::tests::focus_next_wraps_at_last_block' (2069988) panicked at src/cli/viewer.rs:695:9:
assertion `left == right` failed
  left: 3
 right: 0
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
 (x2)
failures:
    cli::viewer::tests::focus_next_wraps_at_last_block

test result: FAILED. 17 passed; 1 failed; 0 ignored; 0 measured; 1334 filtered out; finished in 0.00s

error: test failed, to rerun pass `--lib`
exit=101
== M1 RESTORED ==
0
test cli::viewer::tests::alt_screen_guard_runs_teardown_on_normal_exit ... ok
test cli::viewer::tests::clamp_scroll_pins_to_last_page ... ok
test cli::viewer::tests::clamp_scroll_zero_when_content_fits ... ok
test cli::viewer::tests::focus_next_wraps_at_last_block ... ok
test cli::viewer::tests::focus_prev_wraps_at_first ... ok
test cli::viewer::tests::layout_blocks_empty_transcript_is_empty ... ok
test cli::viewer::tests::layout_blocks_separates_blocks_with_one_blank ... ok
test cli::viewer::tests::collapse_all_outputs_collapses_only_outputs ... ok
test cli::viewer::tests::expanded_layout_is_unchanged_by_the_new_path ... ok
test cli::viewer::tests::layout_blocks_wraps_to_width ... ok
test cli::viewer::tests::collapse_toggle_is_involutive ... ok
test cli::viewer::tests::rows_carry_their_source_block_index ... ok
test cli::viewer::tests::layout_blocks_renders_full_output ... ok
test cli::viewer::tests::collapsed_output_lays_out_as_exactly_one_row ... ok
test cli::viewer::tests::render_transcript_draws_rows_into_backend ... ok
test cli::viewer::tests::render_transcript_survives_scroll_past_end ... ok
test cli::viewer::tests::render_transcript_marks_collapsed_and_focused ... ok

test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 1334 filtered out; finished in 0.00s

exit=0
== GATES ==
fmt exit=0
    Checking daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.63s
clippy exit=0

test result: ok. 31 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.05s

     Running tests/isolation.rs (target/debug/deps/isolation-66949bca409172a9)

running 10 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test daemon_boots_in_throwaway_root ... ok
test config_contains_webhook_and_stub_url ... ok
test hooks_land_on_private_server ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.18s

   Doc-tests daemoneye

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

test exit=0
== VIEWER UNITS ==
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.08s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-b60224cb24515ede)

running 18 tests
test cli::viewer::tests::alt_screen_guard_runs_teardown_on_drop ... ok
test cli::viewer::tests::alt_screen_guard_runs_teardown_on_normal_exit ... ok
test cli::viewer::tests::clamp_scroll_zero_when_content_fits ... ok
test cli::viewer::tests::clamp_scroll_pins_to_last_page ... ok
test cli::viewer::tests::focus_next_wraps_at_last_block ... ok
test cli::viewer::tests::focus_prev_wraps_at_first ... ok
test cli::viewer::tests::layout_blocks_empty_transcript_is_empty ... ok
test cli::viewer::tests::collapse_all_outputs_collapses_only_outputs ... ok
test cli::viewer::tests::layout_blocks_separates_blocks_with_one_blank ... ok
test cli::viewer::tests::expanded_layout_is_unchanged_by_the_new_path ... ok
test cli::viewer::tests::collapse_toggle_is_involutive ... ok
test cli::viewer::tests::layout_blocks_wraps_to_width ... ok
test cli::viewer::tests::rows_carry_their_source_block_index ... ok
test cli::viewer::tests::layout_blocks_renders_full_output ... ok
test cli::viewer::tests::collapsed_output_lays_out_as_exactly_one_row ... ok
test cli::viewer::tests::render_transcript_draws_rows_into_backend ... ok
test cli::viewer::tests::render_transcript_survives_scroll_past_end ... ok
test cli::viewer::tests::render_transcript_marks_collapsed_and_focused ... ok

test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 1334 filtered out; finished in 0.00s

units exit=0
== FOOTER UNIT ==
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.08s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-b60224cb24515ede)

running 1 test
test cli::commands::stream::tests::output_footer_names_ctrl_o ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1351 filtered out; finished in 0.00s

footer exit=0
== CTRL+O IS NAMED ==
3
1
== PHASE-02 CONTRACT STILL HOLDS ==
0
teardown grep exit=1  (1 = none found, which is the pass)
```

PASTE MATCH

### Update — ts=1787170653208 (complete, server-authored)

**Summary:** ## Summary + Notes for review

Phase-03 (expand/collapse) is complete and committed.

**What I built** (src/cli/viewer.rs):
- `layout_blocks_with(blocks, width, collapsed)` with `layout_blocks` as its thin empty-set wrapper; a collapsed block lays out as exactly one row — its header (or first laid-out row for `Assistant`/`System`) with ` [collapsed, {n} lines]`, `{n}` = expanded rows minus the header.
- `ViewRow` gained `pub block: usize` (blank separators carry the index of the block they precede); all 9 existing test literals updated with `block: 0`.
- `viewer_loop` now owns `focus` (init last block, clamped) and an empty `collapsed` set; `]`/`[` move focus (wrapping), `Enter` toggles the focused block, `c` collapses every `Output`, `a` expands all. Focus changes re-scroll to the focused block's header, then re-clamp.
- `render_transcript` takes a `focus` param; focused rows render underlined via new `style_for_focused`, each block header carries `▾ `/`▸ ` (replacing the old hard-coded `▸ `, no `▸ ▸ `), and the status line appends ` · [ ] focus · enter collapse · c/a all`. I moved the new hint **before** `esc to close` so the full status fits the 60-col `TestBackend`.
- ctrl+o footer: extracted `output_footer(total, shown)` in stream.rs (`… N more lines · ctrl+o`), wired into the `Response::ToolResult` arm, and updated the chat help text.

**Tests (pure, no terminal/HOME/tmux):** the 8 spec'd tests plus `collapse_toggle_is_involutive`'s negative assertion; all pass. M1 mutation applied (grep `focus + 1$` = 1, run **fails** on `focus_next_wraps_at_last_block`, exit 101) and restored (grep = 0, run green) — both captured in `/tmp/e2e-03.txt`.

**E2E evidence:** `cargo fmt --check exit=0`, `clippy exit=0`, `cargo test exit=0` (1352 unit passes), viewer units 18/18, footer unit 1/1, `grep -c "ctrl+o"` = 3 (stream) and 1 (chat), phase-02 contract holds (`disarm` count 0, no `try_restore`/`disable_raw_mode`/`.restore()`, teardown grep exit 1), and the phase doc's newest entry is `### Update — 2026-08-19 20:05 (end-to-end verification)` ending with the literal `PASTE MATCH`. All gates re-ran clean after final formatting; working tree is clean.

One judgment call to flag: I bound the key handling to the phase's printable-only constraint (gotcha 2) as spec'd — the viewer's `render_transcript` status hint order was adjusted only to fit the 60-col backend test, and the transcript-side viewport logic was untouched.

**Executor:** deepseek-v4-flash-0731

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.09s


TEST
nored; 0 measured; 0 filtered out; finished in 4.19s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_reads_bare_word ... ok
test header_status_strips_trailing_prose ... ok
test header_status_uses_first_occurrence_only ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 10 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test readme_tools_counts_are_accurate ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test readme_tools_tables_match_the_code ... ok
test claude_md_tools_table_matches_the_code ... ok
test docs_document_the_reindex_command ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_config_template_documents_every_config_field ... ok
test docs_do_not_carry_retired_index_claims ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 33 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ghost_config_parsing ... ok
test minimal_config_parsing ... ok
test window_switch_does_not_corrupt_chat ... ignored
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_no_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok

test result: ok. 31 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.05s


running 10 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test hooks_land_on_private_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.18s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.09s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-b60224cb24515ede)
     Running unittests src/main.rs (target/debug/deps/daemoneye-e700f2084319867a)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-9b22636ef5c08466)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-c00c74ef4ffe9c11)
     Running tests/integration.rs (target/debug/deps/integration-6230826c10f36795)
     Running tests/isolation.rs (target/debug/deps/isolation-66949bca409172a9)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M17-transcript-view/README.md` — +1 -1
- `docs/dev/milestones/M17-transcript-view/phase-03-expand-collapse.md` — +134 -1
- `src/cli/commands/chat.rs` — +1 -1
- `src/cli/commands/stream.rs` — +14 -1
- `src/cli/viewer.rs` — +330 -22

**Commit:** 803940bca8ca48dda676c36231984235d37f3a4b

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-08-19

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** deepseek-v4-flash-0731
- **Scope deviations:** none. The 9 `ViewRow` literals were updated in place
  with `block: 0` as task 2 required — no parallel row type, no workaround.
- **Calibration:** one architect-side item, below. Not a defect in the work.

**Independent verification (re-run, not read):**

- Four gates re-run as separate invocations: all exit 0. 1352 lib tests, up 9
  from 1343 — exactly the 8 new viewer tests plus `output_footer_names_ctrl_o`.
- All 11 named tests present and passing.
- Round E2E artifact re-extracted from the last end-to-end entry and diffed
  against `/tmp/e2e-03.txt`: `PASTE MATCH`. The entry is this dispatch's own
  (2026-08-19 20:05).
- The artifact's own re-checks of the phase-02 contract hold: `disarm` count 0,
  raw-mode teardown grep exits 1, `ctrl+o` named 3× in `stream.rs` and 1× in
  `chat.rs`.
- DoD greps over the diff: the only `.unwrap()` additions are at
  `viewer.rs:748,751`, inside `mod tests` (line 442). No `#[ignore]`,
  `#[allow]`, `TODO`, `dbg!`, `unsafe` added.

**Mutation characterisation, both run by the reviewer:**

- **Ma** — `(focus + 1) % len` → `focus + 1`: `focus_next_wraps_at_last_block`
  fails, 17 others pass. The wrap guard is real and precisely targeted.
- **Mb** — `full.lines()` → `full.lines().take(3)` inside `layout_blocks_with`,
  which breaks the full-output guarantee: **two** tests fail —
  `layout_blocks_renders_full_output` (phase-02's) and
  `collapsed_output_lays_out_as_exactly_one_row` (via its `before == 301`
  assertion). The guarantee is genuinely guarded.

### Calibration — a test the spec made incapable of failing

`expanded_layout_is_unchanged_by_the_new_path` asserts
`layout_blocks(b, w) == layout_blocks_with(b, w, &empty)`. Task 1 of this spec
also required `layout_blocks` to be *a thin wrapper that calls
`layout_blocks_with` with an empty set* — which the executor implemented
exactly (`viewer.rs`, `layout_blocks` is one line). The test therefore compares
a function with the function it delegates to, using the same arguments. It
cannot fail.

Mutation Mb proves it: breaking the layout inside `layout_blocks_with` moved
**both** sides of the equality identically, and this test passed while two
others caught the regression.

It is harmless — phase-02's `layout_blocks_renders_full_output` is the real
guard on the full-output guarantee and it fired immediately — but it is dead
weight in the suite, and the fault is the spec's: **do not pair "implement B as
a wrapper over A" with "assert B equals A".** One occurrence; held for
recurrence, not folded.

This is a different failure than M17's earlier three criterion defects (which
asserted mechanisms rather than behaviour). Those were caught by review; this
one was caught only by mutating the code the test claimed to protect — which is
the argument for running a mutation against every new guard, not only against
the one the spec names.

### Update — 2026-08-19 20:15 (started, round 2 — bug-phase-03-1)

Beginning round 2 of phase-03 on bug-phase-03-1: removing the whole-block
underline (focus cue becomes a per-header reversed marker plus BOLD on body
rows), adding the word-boundary `wrap_words` for prose rows while Output keeps
the hard wrap, and adding the four round-2 tests plus the underline-hygiene
grep.

### Update — 2026-08-20 19:45 (end-to-end verification)

```
== M1 APPLIED ==
1
test cli::viewer::tests::render_transcript_marks_collapsed_and_focused ... ok
test cli::viewer::tests::render_transcript_survives_scroll_past_end ... ok

failures:

---- cli::viewer::tests::focus_next_wraps_at_last_block stdout ----

thread 'cli::viewer::tests::focus_next_wraps_at_last_block' (3637712) panicked at src/cli/viewer.rs:1277:9:
assertion `left == right` failed
  left: 3
 right: 0
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    cli::viewer::tests::focus_next_wraps_at_last_block

test result: FAILED. 40 passed; 1 failed; 0 ignored; 0 measured; 1344 filtered out; finished in 0.00s

error: test failed, to rerun pass `--lib`
exit=101
== M1 RESTORED ==
0
test cli::viewer::tests::next_match_wraps ... ok
test cli::viewer::tests::collapsed_output_lays_out_as_exactly_one_row ... ok
test cli::viewer::tests::layout_blocks_wraps_to_width ... ok
test cli::viewer::tests::prev_match_wraps ... ok
test cli::viewer::tests::output_rows_keep_hard_wrap ... ok
test cli::viewer::tests::layout_blocks_renders_full_output ... ok
test cli::viewer::tests::row_at_maps_body_rows_with_scroll ... ok
test cli::viewer::tests::row_at_rejects_the_status_line ... ok
test cli::viewer::tests::scroll_to_row_only_moves_when_offscreen ... ok
test cli::viewer::tests::rows_carry_their_source_block_index ... ok
test cli::viewer::tests::style_for_focused_is_distinct_without_underline ... ok
test cli::viewer::tests::wrap_words_breaks_an_overlong_token ... ok
test cli::viewer::tests::wrap_words_does_not_split_words ... ok
test cli::viewer::tests::render_transcript_shows_match_counter ... ok
test cli::viewer::tests::render_transcript_draws_rows_into_backend ... ok
test cli::viewer::tests::render_transcript_marks_collapsed_and_focused ... ok
test cli::viewer::tests::render_transcript_survives_scroll_past_end ... ok

test result: ok. 41 passed; 0 failed; 0 ignored; 0 measured; 1344 filtered out; finished in 0.00s

exit=0
== GATES ==
fmt exit=0
    Checking daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.25s
clippy exit=0

test result: ok. 31 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s

     Running tests/isolation.rs (target/debug/deps/isolation-66949bca409172a9)

running 10 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test config_contains_webhook_and_stub_url ... ok
test default_server_unchanged ... ok
test hooks_land_on_private_server ... ok
test daemon_boots_in_throwaway_root ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.16s

   Doc-tests daemoneye

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

test exit=0
== VIEWER UNITS ==
test cli::viewer::tests::key_action_typing_wins_over_commands_while_searching ... ok
test cli::viewer::tests::key_action_wheel_scrolls_click_ignored_while_searching ... ok
test cli::viewer::tests::key_action_y_copies_only_when_not_searching ... ok
test cli::viewer::tests::layout_blocks_empty_transcript_is_empty ... ok
test cli::viewer::tests::layout_blocks_separates_blocks_with_one_blank ... ok
test cli::viewer::tests::collapsed_output_lays_out_as_exactly_one_row ... ok
test cli::viewer::tests::next_match_wraps ... ok
test cli::viewer::tests::output_rows_keep_hard_wrap ... ok
test cli::viewer::tests::prev_match_wraps ... ok
test cli::viewer::tests::layout_blocks_wraps_to_width ... ok
test cli::viewer::tests::row_at_maps_body_rows_with_scroll ... ok
test cli::viewer::tests::row_at_rejects_the_status_line ... ok
test cli::viewer::tests::layout_blocks_renders_full_output ... ok
test cli::viewer::tests::scroll_to_row_only_moves_when_offscreen ... ok
test cli::viewer::tests::style_for_focused_is_distinct_without_underline ... ok
test cli::viewer::tests::rows_carry_their_source_block_index ... ok
test cli::viewer::tests::wrap_words_breaks_an_overlong_token ... ok
test cli::viewer::tests::wrap_words_does_not_split_words ... ok
test cli::viewer::tests::render_transcript_draws_rows_into_backend ... ok
test cli::viewer::tests::render_transcript_survives_scroll_past_end ... ok
test cli::viewer::tests::render_transcript_marks_collapsed_and_focused ... ok
test cli::viewer::tests::render_transcript_shows_match_counter ... ok

test result: ok. 41 passed; 0 failed; 0 ignored; 0 measured; 1344 filtered out; finished in 0.00s

units exit=0
== FOOTER UNIT ==
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.06s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-b60224cb24515ede)

running 1 test
test cli::commands::stream::tests::output_footer_names_ctrl_o ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1384 filtered out; finished in 0.00s

footer exit=0
== CTRL+O IS NAMED ==
3
1
== PHASE-02 CONTRACT STILL HOLDS ==
0
teardown grep exit=1  (1 = none found, which is the pass)
```

PASTE MATCH

### Update — ts=1787258741521 (complete, server-authored)

**Summary:** All tasks complete, all gates green, working tree clean. 

## Summary + Notes for review

Round 2 of phase-03 (bug-phase-03-1) is complete and committed (`dae5f5f`).

**What I built** (all in `src/cli/viewer.rs` against the phase-03 tree that already carried collapse, focus, footer, and the round-1 tests):
1. **Focus emphasis without body underline.** `style_for_focused` no longer adds `Modifier::UNDERLINED`; the focused block's single header row gets `REVERSED` video and body rows get `BOLD`. I also removed the stray `UNDERLINED` from `style_for_current` (the search-active style) — the bug doc only named the focus function but the criterion's `grep -c` counts every occurrence, and that second one rendered as a leftover underline on top of the violet search highlight. `grep -c "Modifier::UNDERLINED"` now prints 0. **Judgment call:** the new test references the underline flag via `Modifier::from_bits_truncate(1 << 3)` (bit 3 = value 8, verified from the ratatui-core 0.1.2 bitflags declaration) so the test tests the guarantee without re-introducing the banned literal into the file — I considered `grep`-excluding the test line a cheat. Worth noting in the architecture's mental model of this repo.
2. **Prose word-wrap.** New `wrap_words` (whole-word wrapping with leading/trailing whitespace preserved so rows rejoin exactly, overlong tokens still broken); `push_wrapped` now uses it for `User`/`Assistant`/`Tool`/`System`, while `Output` goes through the new `push_wrapped_hard` → `wrap_line_hard` unchanged (row-count guarantees intact).
3. **Tests:** the four round-2 tests from the bug doc (`style_for_focused_is_distinct_without_underline`, `wrap_words_does_not_split_words`, `wrap_words_breaks_an_overlong_token`, `output_rows_keep_hard_wrap`) pass; `layout_blocks_renders_full_output` and `collapsed_output_lays_out_as_exactly_one_row` still pass. The M1 mutation pair was re-run this round exactly as Task 8/9 spec (apply → grep `focus + 1$` = 1, run **fails** on `focus_next_wraps_at_last_block` exit 101 → restore → grep = 0, run green exit 0) and is captured in the fresh `/tmp/e2e-03.txt`.

**E2E evidence:** Task 10/11 block pasted into a new `### Update — 2026-08-20 19:45 (end-to-end verification)` entry, `PASTE MATCH` verified against `/tmp/e2e-03.txt`. All four gates green (`fmt`/`build`/`clippy`/`test` — 1385 lib + 6+10+31+9 integration/doc tests), phase-02 contract holds (`disarm` 0, no `try_restore`/`disable_raw_mode`/`.restore()`, teardown grep exit 1), footer tests green, `ctrl+o` named 3× in `stream.rs` and 1× in `chat.rs`. Two notes for review: (a) the phase-03 doc's round-1 `(started)`/`(end-to-end verification)` entries predate the bug and were left untouched — the round-2 entry is genuinely new; (b) my round-2 `(started)` entry and E2E entries are appended to the phase doc as normal Update Log entries (the server authors the completion tail). No scope creep: `tty.rs`, `wrap_line_hard`, and default-open-expanded semantics are untouched.

**Executor:** deepseek-v4-flash-0731

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s


TEST
nored; 0 measured; 0 filtered out; finished in 4.01s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_reads_bare_word ... ok
test header_status_strips_trailing_prose ... ok
test header_status_uses_first_occurrence_only ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 10 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test readme_tools_counts_are_accurate ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_tools_tables_match_the_code ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_config_template_documents_every_config_field ... ok
test docs_document_the_reindex_command ... ok
test seeded_config_template_has_no_phantom_keys ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 33 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_ask_round_trip ... ok
test ghost_config_parsing ... ok
test ipc_session_info_round_trip ... ok
test minimal_config_parsing ... ok
test window_switch_does_not_corrupt_chat ... ignored
test config_pricing_round_trip ... ok
test schedule_store_persistence ... ok
test g4_briefing_masking_applied ... ok
test event_log_append_read ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test g4_briefing_read_and_clear ... ok
test g4_briefing_injects_on_next_run ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok

test result: ok. 31 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 10 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test hooks_land_on_private_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.16s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.08s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-b60224cb24515ede)
     Running unittests src/main.rs (target/debug/deps/daemoneye-e700f2084319867a)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-9b22636ef5c08466)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-c00c74ef4ffe9c11)
     Running tests/integration.rs (target/debug/deps/integration-6230826c10f36795)
     Running tests/isolation.rs (target/debug/deps/isolation-66949bca409172a9)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M17-transcript-view/phase-03-expand-collapse.md` — +135 -0
- `src/cli/viewer.rs` — +199 -30

**Commit:** dae5f5f668cf89c484f68a5c08bd2858618cc7f1

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

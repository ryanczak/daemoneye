# Phase 04: Search

**Milestone:** M17 — Transcript View
**Status:** in-progress
**Depends on:** phase-03 (expand-collapse, `done`)
**Estimated diff:** ~450 lines

**Tags:** language=rust, kind=feature, size=m

## Goal

Find text in the transcript: `/` opens an incremental search, matches highlight
as you type, `n`/`N` step through them, and the view scrolls to each one. Along
the way, make the viewer's key handling a **pure function** so mode-sensitive
behaviour ("`q` types a letter while searching, quits otherwise") is testable
without a terminal.

## Architecture references

Read before starting:

- `docs/design/transcript-view.md` — §"What this unlocks beyond expansion".
- `src/cli/viewer.rs` — the viewer this extends; 799 lines. Read it in full
  before editing.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

**`viewer_loop` (`src/cli/viewer.rs:329`) matches keys inline.** Its arms today
are `Up`, `Down`, `PageUp`, `PageDown`, `Home`, `End`, `Char(']')`, `Char('[')`,
`Enter`, `Char('c')`, `Char('a')`, and the exit arm:

```rust
            crate::cli::input::Key::Char('\x1b')
            | crate::cli::input::Key::Char('q')
            | crate::cli::input::Key::CtrlO => break,
```

Its state is `focus`, `collapsed`, `scroll`, and a `focus_changed` flag.

**Scroll-into-view already exists, inline and untested** (`viewer.rs:421-431`):

```rust
        if focus_changed
            && let Some(row_idx) = rows
                .iter()
                .position(|r| r.block == focus && r.kind == crate::cli::viewer::RowKind::Header)
        {
            if row_idx < scroll {
                scroll = row_idx;
            } else if row_idx >= scroll + body_height {
                scroll = row_idx.saturating_add(1).saturating_sub(body_height);
            }
        }
```

Task 2 extracts exactly this arithmetic into `scroll_to_row` and task 3 makes
**both** focus movement and match navigation call it. Do not leave a second
copy behind.

**`render_transcript` (`viewer.rs:190`)** takes
`(f, rows, scroll, focus, evicted)`, styles each row via `style_for` /
`style_for_focused` on `row.block == focus`, and writes a status line ending:

```
 · [ ] focus · enter collapse · c/a all · ↑↓ PgUp/PgDn Home/End · esc to close
```

**`ViewRow`** carries `text`, `kind`, `block` (`viewer.rs:25`).

### Three gotchas, each verified against the tree

1. **There is still no `Key::Esc`.** A bare Escape arrives as
   `Key::Char('\x1b')` (`src/cli/input/tty.rs:259`). Search-cancel matches that,
   not a named variant.
2. **While searching, every printable key must type into the query.** `q`, `c`,
   `a`, `[`, `]` and `n` are all bound to commands today. If the mode check is
   missed, typing `cat` collapses all outputs, expands them, and types nothing.
   This is precisely what task 1's pure `key_action` makes testable — and what
   the acceptance criteria assert on.
3. **Search runs over the rows the viewer is currently showing.** A collapsed
   block's body is not in `rows`, so its text is **not** searchable. That is the
   documented behaviour for this phase, not an oversight — pin it with the
   negative test named in § Test plan. Do **not** auto-expand blocks on search;
   that would discard the user's collapse state.

## Spec

### Task 1 — A pure, mode-aware key decoder

In `src/cli/viewer.rs`, add:

```rust
/// What a keypress means to the viewer. `searching` in `key_action` selects
/// between command mode and search-input mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewerAction {
    ScrollUp,
    ScrollDown,
    PageUp,
    PageDown,
    Top,
    Bottom,
    FocusNext,
    FocusPrev,
    ToggleCollapse,
    CollapseOutputs,
    ExpandAll,
    SearchOpen,
    SearchType(char),
    SearchBackspace,
    SearchCommit,
    SearchCancel,
    MatchNext,
    MatchPrev,
    Quit,
    Ignore,
}

/// Decode one key. `searching` is true while the search prompt is open.
pub fn key_action(key: &crate::cli::input::Key, searching: bool) -> ViewerAction
```

Behaviour, pinned:

- **When `searching` is true**, in this precedence order: `Key::Char('\x1b')` →
  `SearchCancel`; `Key::Enter` → `SearchCommit`; `Key::Backspace` →
  `SearchBackspace`; any other `Key::Char(c)` where `c` is not a control
  character → `SearchType(c)`; `Up`/`Down`/`PageUp`/`PageDown`/`Home`/`End`
  keep their normal scrolling meanings; everything else → `Ignore`.
  **`Key::Char('q')`, `Key::Char('c')`, `Key::Char('a')`, `Key::Char('[')`,
  `Key::Char(']')` and `Key::Char('n')` must all decode to `SearchType`, never
  to their command meanings.**
- **When `searching` is false**: the existing arms keep their meanings
  (`Up`→`ScrollUp`, `Down`→`ScrollDown`, `PageUp`, `PageDown`, `Home`→`Top`,
  `End`→`Bottom`, `']'`→`FocusNext`, `'['`→`FocusPrev`, `Enter`→
  `ToggleCollapse`, `'c'`→`CollapseOutputs`, `'a'`→`ExpandAll`,
  `'\x1b'`/`'q'`/`CtrlO`→`Quit`), plus `'/'`→`SearchOpen`, `'n'`→`MatchNext`,
  `'N'`→`MatchPrev`. Anything else → `Ignore`.

### Task 2 — Pure search and scroll helpers

Also in `src/cli/viewer.rs`. Write `find_matches` **exactly** in this shape —
task 7's mutation targets its second line verbatim:

```rust
/// Row indices whose text contains `query`, case-insensitively.
/// An empty query matches nothing.
pub fn find_matches(rows: &[ViewRow], query: &str) -> Vec<usize> {
    if query.is_empty() {
        return Vec::new();
    }
    let needle = query.to_lowercase();
    rows.iter()
        .enumerate()
        .filter(|(_, r)| r.text.to_lowercase().contains(&needle))
        .map(|(i, _)| i)
        .collect()
}
```

Plus:

```rust
/// Next match index, wrapping. `len == 0` yields 0.
pub fn next_match(cur: usize, len: usize) -> usize

/// Previous match index, wrapping. `len == 0` yields 0.
pub fn prev_match(cur: usize, len: usize) -> usize

/// Minimal scroll offset that keeps `row` visible in a `height`-row viewport.
/// Returns `scroll` unchanged when the row is already visible.
pub fn scroll_to_row(row: usize, scroll: usize, height: usize) -> usize
```

`scroll_to_row` reproduces the arithmetic quoted in § Current state: if
`row < scroll`, return `row`; if `row >= scroll + height`, return
`row + 1 - height` (saturating); otherwise return `scroll`. A `height` of 0
must not panic.

### Task 3 — Rewire the loop through the decoder

Rewrite `viewer_loop`'s key handling to `match key_action(&key, searching)`
over `ViewerAction`, replacing the inline `Key::` arms. Behaviour for the
existing actions is unchanged.

Replace the inline scroll-into-view block at `viewer.rs:421-431` with a call to
`scroll_to_row`, and use the **same** helper when jumping to a match. There must
be exactly one copy of that arithmetic in the file when you are done.

### Task 4 — Search state

Add to `viewer_loop`: `searching: bool`, `query: String`,
`matches: Vec<usize>`, `current: usize`.

- `SearchOpen` — `searching = true`, `query` cleared, `matches` cleared.
- `SearchType(c)` — push `c`, then **recompute `matches` immediately**
  (incremental), set `current = 0`, and scroll to `matches[0]` if any.
- `SearchBackspace` — pop, recompute the same way. Popping to an empty query
  leaves 0 matches.
- `SearchCommit` — `searching = false`, keeping `query`, `matches` and
  `current` so `n`/`N` continue to work.
- `SearchCancel` — `searching = false`, and clear `query`, `matches`,
  `current`.
- `MatchNext` / `MatchPrev` — move `current` via `next_match`/`prev_match` and
  scroll to `matches[current]` with `scroll_to_row`. With no matches, do
  nothing.

Recompute `matches` whenever the layout changes (resize, collapse toggle,
expand-all) so indices never point past the end of `rows`.

### Task 5 — Render matches and the search prompt

`render_transcript` gains two parameters after `focus`:
`matches: &[usize]`, `current: Option<usize>` (the index **into `matches`** of
the current one, `None` when there are none).

- A row whose index is in `matches` renders with a match style; the row at
  `matches[current]` renders with a distinct stronger style. **Nothing about
  the colours is pinned** — pick from the existing `Palette`. Precedence:
  current match, then match, then focus, then `RowKind`.
- The status line, when `searching` is true, shows the live query and count:
  `/{query} — {k}/{n}` where `k` is `current + 1` (or `0` when `n == 0`), and
  `n` is `matches.len()`.
- When not searching but a committed query has matches, keep showing
  `{k}/{n} for "{query}"` alongside the existing hints, and append
  ` · / search · n/N next/prev` to the key hints.

### Task 6 — Tests

Write the tests named in § Test plan. All are pure — no terminal except the
existing `TestBackend` idiom.

### Task 7 — Mutation M1: apply

Use the `patch` tool on `src/cli/viewer.rs`.

- `old_str`: `    let needle = query.to_lowercase();`
- `new_str`: `    let needle = query.to_string();`

Then run, appending to the evidence artifact:

```sh
A=/tmp/e2e-04.txt
echo "== M1 APPLIED ==" >> "$A"
grep -c 'let needle = query.to_string();' src/cli/viewer.rs >> "$A"
cargo test --lib cli::viewer 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -20 >> "$A"
echo "exit=${PIPESTATUS[0]}" >> "$A"
```

The run **must fail** — `find_matches_is_case_insensitive` is what proves the
normalisation is real. A green run means the test is vacuous; stop and file a
blocker.

### Task 8 — Mutation M1: restore

`patch` the same line back, then:

```sh
A=/tmp/e2e-04.txt
echo "== M1 RESTORED ==" >> "$A"
grep -c 'let needle = query.to_string();' src/cli/viewer.rs >> "$A"
cargo test --lib cli::viewer 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -20 >> "$A"
echo "exit=${PIPESTATUS[0]}" >> "$A"
```

`grep -c` must print `1` after task 7 and `0` after task 8. Do **not** use
`git checkout` to restore — the file holds this round's uncommitted work.

### Task 9 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-04.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this.

### Task 10 — PASTE MATCH self-check

After pasting, run:

```sh
D=docs/dev/milestones/M17-transcript-view/phase-04-search.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-04.txt
diff /tmp/pasted-04.txt /tmp/e2e-04.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

Append the literal verdict line into that same Update Log entry, below the
fence.

## Acceptance criteria

Every criterion asserts an observed value or count, never the presence of a
mechanism.

- [ ] `cargo fmt --all` leaves the tree unchanged.
- [ ] `cargo build` succeeds.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` passes.
- [ ] Test `key_action_typing_wins_over_commands_while_searching` passes —
      for **each** of `q`, `c`, `a`, `[`, `]`, `n`, asserts
      `key_action(&Key::Char(ch), true) == ViewerAction::SearchType(ch)`.
- [ ] Test `key_action_commands_apply_when_not_searching` passes — the same six
      characters decode to `Quit`, `CollapseOutputs`, `ExpandAll`, `FocusPrev`,
      `FocusNext`, `MatchNext` respectively with `searching = false`.
- [ ] Test `find_matches_empty_query_matches_nothing` passes — asserts the
      result length is **exactly 0** over a non-empty row set.
- [ ] Test `find_matches_is_case_insensitive` passes — a query differing only
      in case returns the **same exact indices** as the lowercase query.
- [ ] Test `find_matches_skips_collapsed_block_bodies` passes — text present
      only inside a collapsed block yields **exactly 0** matches, and the same
      query over the expanded layout yields a **non-zero** count (assert both
      halves; the second is what proves the first is about collapsing and not a
      typo).
- [ ] Tests `next_match_wraps` and `prev_match_wraps` pass, each asserting
      exact wrapped indices including the `len == 0` case.
- [ ] Test `scroll_to_row_only_moves_when_offscreen` passes — asserts the
      unchanged case, the above-viewport case, the below-viewport case, and
      `height == 0` not panicking, with exact expected offsets.
- [ ] Test `render_transcript_shows_match_counter` passes — a `TestBackend`
      draw with 3 matches and `current = Some(0)` puts `1/3` on the status row.
- [ ] `grep -c "row_idx >= scroll + body_height" src/cli/viewer.rs` prints `0` —
      the inline scroll-into-view arithmetic is gone, replaced by
      `scroll_to_row`. (Currently `1`.)
- [ ] `/tmp/e2e-04.txt` shows `== M1 APPLIED ==` with a **failing** run and
      `grep -c` = 1, then `== M1 RESTORED ==` with a passing run and
      `grep -c` = 0.
- [ ] The Update Log's newest entry is headed
      `### Update — <date> (end-to-end verification)`, contains the pasted
      artifact, and ends with the literal line `PASTE MATCH`.

## Test plan

In `src/cli/viewer.rs` (`#[cfg(test)] mod tests`):

- `key_action_typing_wins_over_commands_while_searching` — the six characters,
  `searching = true`, each → `SearchType(ch)`.
- `key_action_commands_apply_when_not_searching` — the same six,
  `searching = false`, each → its command action.
- `key_action_escape_cancels_search_but_quits_otherwise` —
  `key_action(&Key::Char('\x1b'), true) == SearchCancel` and
  `… false) == Quit`. Pins the one key whose meaning flips.
- `find_matches_empty_query_matches_nothing` — exactly 0.
- `find_matches_is_case_insensitive` — same indices for `"LOREM"` and
  `"lorem"`, and the count is non-zero (a vacuous "both empty" pass fails this).
- `find_matches_skips_collapsed_block_bodies` — both halves, per the criterion.
- `next_match_wraps` — `next_match(2, 3) == 0`, `next_match(0, 3) == 1`,
  `next_match(0, 0) == 0`.
- `prev_match_wraps` — `prev_match(0, 3) == 2`, `prev_match(2, 3) == 1`,
  `prev_match(0, 0) == 0`.
- `scroll_to_row_only_moves_when_offscreen` — `scroll_to_row(5, 0, 10) == 0`
  (visible, unchanged), `scroll_to_row(2, 5, 10) == 2` (above),
  `scroll_to_row(20, 0, 10) == 11` (below), `scroll_to_row(3, 0, 0)` does not
  panic.
- `render_transcript_shows_match_counter` — `TestBackend::new(60, 10)`, three
  matches, `current = Some(0)`; the bottom row contains `1/3`.

## End-to-end verification

Search is a keyboard behaviour inside the alternate screen, so its real check
is live and architect-run at milestone close. What the executor verifies here
is the decoder, the match arithmetic, the scroll helper, and a `TestBackend`
draw — all of which are pure and reachable headlessly.

Tasks 7 and 8 append the mutation pair to the **same** artifact before this
block runs; do not truncate `/tmp/e2e-04.txt` here.

```sh
A=/tmp/e2e-04.txt
echo "== GATES ==" >> "$A"
cargo fmt --all -- --check 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -5 >> "$A"
echo "fmt exit=${PIPESTATUS[0]}" >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -5 >> "$A"
echo "clippy exit=${PIPESTATUS[0]}" >> "$A"
cargo test 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -25 >> "$A"
echo "test exit=${PIPESTATUS[0]}" >> "$A"
echo "== VIEWER UNITS ==" >> "$A"
cargo test --lib cli::viewer 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -30 >> "$A"
echo "units exit=${PIPESTATUS[0]}" >> "$A"
echo "== ONE COPY OF THE SCROLL ARITHMETIC ==" >> "$A"
grep -c "row_idx >= scroll + body_height" src/cli/viewer.rs >> "$A"
grep -c "fn scroll_to_row" src/cli/viewer.rs >> "$A"
echo "== PHASE-02 CONTRACT STILL HOLDS ==" >> "$A"
grep -c "disarm" src/cli/viewer.rs >> "$A"
grep -nE "try_restore|disable_raw_mode|\.restore\(\)" src/cli/viewer.rs >> "$A"
echo "teardown grep exit=$?  (1 = none found, which is the pass)" >> "$A"
```

## Authorizations

- [ ] May edit `src/cli/viewer.rs`.

No new dependencies. `docs/architecture.md` is **not** authorized, and neither
is `src/cli/input/tty.rs` — this phase adds no key parsing.

## Out of scope

- **Copy** (phase-05), **rehydration** (phase-06), **mouse** (phase-07).
- **Regex.** `find_matches` is a case-insensitive substring test. A regex
  engine is not in scope and no dependency may be added for one.
- **Auto-expanding collapsed blocks on search.** See gotcha 3 — the phase pins
  the opposite behaviour with a test.
- **Undoing anything phases 02–03 established.** `AltScreenGuard` keeps its
  unconditional `Drop`, `viewer.rs` gains no `disarm` and no `try_restore` /
  `disable_raw_mode` / `.restore()`, and the collapse/focus behaviour is
  unchanged. The E2E block re-checks the guard contract.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-08-19 14:37 (progress)

Started phase-04 (search). Flipped phase doc status to `in-progress` and the
milestone README row to match. Implemented tasks 1–6: the `ViewerAction` enum
and pure `key_action` decoder, `find_matches` / `next_match` / `prev_match` /
`scroll_to_row` helpers, rewired `viewer_loop` through the decoder with search
state, `render_transcript` match styling and search-prompt status line, and the
full test plan. Next: the M1 mutation pair (tasks 7–8), then the end-to-end
capture (tasks 9–10).


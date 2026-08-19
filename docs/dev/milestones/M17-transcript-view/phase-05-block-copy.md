# Phase 05: Block Copy

**Milestone:** M17 — Transcript View
**Status:** todo
**Depends on:** phase-04 (search, `done`)
**Estimated diff:** ~350 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Press `y` in the viewer to copy the focused block into a tmux buffer, so the
output the inline panel elided can leave the transcript and be pasted anywhere.
Copy the block's **real content** — a collapsed block copies in full, and none
of the viewer's own decoration comes with it.

## Architecture references

Read before starting:

- `docs/design/transcript-view.md` — §"Clipboard" is the design of record for
  why this is `tmux load-buffer` and not OSC 52.
- `src/cli/viewer.rs` — the viewer this extends; 1272 lines.
- `src/cli/transcript.rs` — the `Block` enum being copied.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

**`ViewerAction` has 20 variants** (`src/cli/viewer.rs`), decoded by the pure
`key_action(key, searching)`. Its head establishes the shape a new arm must
follow:

```rust
pub fn key_action(key: &crate::cli::input::Key, searching: bool) -> ViewerAction {
    match (searching, key) {
        (true, crate::cli::input::Key::Char('\x1b')) => ViewerAction::SearchCancel,
        (true, crate::cli::input::Key::Enter) => ViewerAction::SearchCommit,
        (true, crate::cli::input::Key::Backspace) => ViewerAction::SearchBackspace,
        (true, crate::cli::input::Key::Char(c)) if !c.is_control() => ViewerAction::SearchType(*c),
```

The `(true, Char(c)) if !c.is_control()` arm sits **above** every command arm,
so a new `(false, Char('y'))` arm is automatically inert while searching. Keep
that ordering.

**The blocks being copied** (`src/cli/transcript.rs`):

```rust
pub enum Block {
    UserTurn { label: String, text: String },
    Assistant { text: String },
    ToolPanel { tool: String, summary: String, label: Option<String> },
    Output { tool_call_id: String, full: String, shown: usize },
    System { text: String },
}
```

**The status line** is assembled in `render_transcript` from
`format!("transcript — {shown_from}-{shown_to} of {total} lines")`, then
optionally prefixed with the eviction note and suffixed with the search counter
before the key hints are appended.

### Three gotchas, each verified against the tree

1. **`crate::tmux::bounded_output` cannot be used here.** It pipes *stdout* and
   *stderr* only (`src/tmux/mod.rs:67-75`) and never provides a stdin handle,
   so a `load-buffer -` invocation through it would hand tmux an empty buffer.
   This phase writes its own small spawner — the exact shape is given in task 2.
2. **Copy from the `Block`, not from the rendered `ViewRow`s.** The rows carry
   the viewer's decoration (the `▾ `/`▸ ` marker, the `output (N lines)` header,
   the `[collapsed, N lines]` suffix) and a collapsed block has no body rows at
   all. Deriving the text from `Block` is what makes "collapsed copies in full"
   true, and it is pinned as a test.
3. **`y` must type into the query while searching.** That falls out of the arm
   ordering in gotcha-free fashion *if* the new arm is `(false, …)` — but it is
   asserted explicitly in the criteria, because M17's record is that the
   untested half of a mode-sensitive behaviour is the half that breaks.

## Spec

### Task 1 — The pure copy text

In `src/cli/viewer.rs`, add:

```rust
/// The text a block yields when copied — its real content, with none of the
/// viewer's decoration and independent of whether it is collapsed.
pub fn copy_text(block: &crate::cli::transcript::Block) -> String
```

Pinned per variant:

- `UserTurn { text, .. }` → `text` verbatim (the `label` is not copied).
- `Assistant { text }` → `text` verbatim.
- `System { text }` → `text` verbatim, **without** the `⚙ ` prefix the viewer
  renders.
- `Output { full, .. }` → `full` verbatim — every line, never `shown`-limited.
- `ToolPanel { tool, summary, label }` → `tool`, then a newline, then `summary`;
  when `label` is `Some(l)`, the first line is `{tool} — {l}`. This is the one
  variant that is a composition rather than a field; pin it exactly so the test
  can assert a literal.

No trailing newline is added or removed — what the block holds is what comes
out.

Write the `Output` arm **exactly** as below — task 6's mutation targets this
line verbatim, so an equivalent-but-differently-written arm breaks the pair:

```rust
        crate::cli::transcript::Block::Output { full, .. } => full.clone(),
```

### Task 2 — The tmux hand-off

Also in `src/cli/viewer.rs`:

```rust
/// Load `text` into a tmux buffer, and into the system clipboard where the
/// terminal supports it (`-w` uses OSC 52; tmux >= 3.2).
fn copy_to_tmux_buffer(text: &str) -> anyhow::Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = Command::new("tmux")
        .args(["load-buffer", "-w", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("tmux load-buffer: no stdin"))?
        .write_all(text.as_bytes())?;
    let status = child.wait()?;
    if !status.success() {
        anyhow::bail!("tmux load-buffer exited with {status}");
    }
    Ok(())
}
```

Write it as given. The `stdin` handle **must be dropped before `wait()`** —
taking it out of the child and letting it fall out of scope at the end of the
statement is what closes the pipe; holding it across `wait()` deadlocks.

This function shells out, so it gets **no unit test** — task 5's tests cover
`copy_text`, and the E2E block exercises the tmux call for real.

### Task 3 — Wire the action

- Add `Copy` to `ViewerAction`.
- Add `(false, crate::cli::input::Key::Char('y')) => ViewerAction::Copy` to
  `key_action`, **below** the existing `(true, Char(c))` arm.
- Handle `ViewerAction::Copy` in `viewer_loop`: take the focused block from
  `transcript.blocks()`, run `copy_text`, hand it to `copy_to_tmux_buffer`, and
  record the outcome as a status note (task 4). An empty transcript copies
  nothing and reports nothing.

### Task 4 — Report the outcome

Add a `note: Option<String>` to the viewer's state and a `note: Option<&str>`
parameter to `render_transcript` (after `search`). When present it is appended
to the status line as ` · {note}`.

- On success: `copied {n} lines to tmux buffer` where `n` is
  `copy_text(...).lines().count()`.
- On failure: `copy failed: {e}` — the error is **shown, never swallowed**. Do
  not `let _ =` the result.
- The note is cleared on the next keypress that produces any action other than
  `Ignore`, so it does not linger.

### Task 5 — Tests

Write the tests named in § Test plan. All are pure — `copy_text` and
`key_action` need no terminal and no tmux.

### Task 6 — Mutation M1: apply

Use the `patch` tool on `src/cli/viewer.rs` to make a collapsed block copy only
what is on screen.

- `old_str`: `        crate::cli::transcript::Block::Output { full, .. } => full.clone(),`
- `new_str`: `        crate::cli::transcript::Block::Output { full, shown, .. } => full.lines().take(*shown).collect::<Vec<_>>().join("\n"),`

Then run, appending to the evidence artifact:

```sh
A=/tmp/e2e-05.txt
echo "== M1 APPLIED ==" >> "$A"
grep -c 'take(\*shown)' src/cli/viewer.rs >> "$A"
cargo test --lib cli::viewer 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -20 >> "$A"
echo "exit=${PIPESTATUS[0]}" >> "$A"
```

The run **must fail**. A green run means the test is vacuous; stop and file a
blocker.

### Task 7 — Mutation M1: restore

`patch` the arm back, then:

```sh
A=/tmp/e2e-05.txt
echo "== M1 RESTORED ==" >> "$A"
grep -c 'take(\*shown)' src/cli/viewer.rs >> "$A"
cargo test --lib cli::viewer 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -20 >> "$A"
echo "exit=${PIPESTATUS[0]}" >> "$A"
```

`grep -c` must print `1` after task 6 and `0` after task 7. Do **not** use
`git checkout` to restore — the file holds this round's uncommitted work.

### Task 8 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-05.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this.

### Task 9 — PASTE MATCH self-check

After pasting, run:

```sh
D=docs/dev/milestones/M17-transcript-view/phase-05-block-copy.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-05.txt
diff /tmp/pasted-05.txt /tmp/e2e-05.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

Append the literal verdict line into that same Update Log entry, below the
fence.

## Acceptance criteria

Every criterion asserts an observed value or count.

- [ ] `cargo fmt --all` leaves the tree unchanged.
- [ ] `cargo build` succeeds.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` passes.
- [ ] Test `copy_text_copies_full_output_not_the_elided_view` passes — a
      300-line `Output` with `shown: 9` yields **exactly 300** lines, and the
      string equals `full`.
- [ ] Test `copy_text_of_collapsed_block_is_unchanged` passes — `copy_text` is
      a function of the `Block` alone, so the same block collapsed or expanded
      yields byte-identical text (assert equality against the expanded case,
      with the block present in a collapsed set).
- [ ] Test `copy_text_omits_viewer_decoration` passes — for a `System` block
      the result does **not** start with `⚙`, and for a `UserTurn` the result
      does **not** contain the label.
- [ ] Test `copy_text_tool_panel_composes_header_and_summary` passes — asserts
      the exact literal for both the `Some(label)` and `None` cases.
- [ ] Test `key_action_y_copies_only_when_not_searching` passes —
      `key_action(&Key::Char('y'), false) == ViewerAction::Copy` **and**
      `key_action(&Key::Char('y'), true) == ViewerAction::SearchType('y')`.
- [ ] `grep -c "let _ = copy_to_tmux_buffer" src/cli/viewer.rs` prints `0` —
      the copy result is surfaced, never discarded.
- [ ] The E2E block's `== TMUX ROUND TRIP ==` section shows the exact three
      lines that were loaded coming back out of `tmux show-buffer`.
- [ ] `/tmp/e2e-05.txt` shows `== M1 APPLIED ==` with a **failing** run and
      `grep -c` = 1, then `== M1 RESTORED ==` with a passing run and
      `grep -c` = 0.
- [ ] The Update Log's newest entry is headed
      `### Update — <date> (end-to-end verification)`, contains the pasted
      artifact, and ends with the literal line `PASTE MATCH`.

## Test plan

In `src/cli/viewer.rs` (`#[cfg(test)] mod tests`):

- `copy_text_copies_full_output_not_the_elided_view` — 300 lines, `shown: 9`;
  exactly 300 lines out, string equality with `full`.
- `copy_text_of_collapsed_block_is_unchanged` — same block, collapsed vs
  expanded layout; `copy_text` output byte-identical.
- `copy_text_omits_viewer_decoration` — `System` result does not start with
  `⚙`; `UserTurn` result does not contain its label.
- `copy_text_tool_panel_composes_header_and_summary` — exact literals for
  `Some("2.1s")` → `"cargo build — 2.1s\ncompiling…"` and for `None` →
  `"cargo build\ncompiling…"`.
- `key_action_y_copies_only_when_not_searching` — both modes, exact actions.

`copy_to_tmux_buffer` is deliberately untested at unit level — it spawns a
process. The E2E block covers it against the real `tmux`.

## End-to-end verification

Unlike the earlier viewer phases, this one **does** have a headlessly
verifiable real artifact: the tmux buffer. The block below loads a known string
through the same `tmux load-buffer -w -` invocation the code uses and reads it
back with `tmux show-buffer`, so the hand-off is proven against the real binary
rather than only the unit tests. (This shell round-trip proves the *mechanism*;
that the viewer's `y` key reaches it is the live check at milestone close.)

Tasks 6 and 7 append the mutation pair to the **same** artifact before this
block runs; do not truncate `/tmp/e2e-05.txt` here.

```sh
A=/tmp/e2e-05.txt
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
echo "== TMUX VERSION ==" >> "$A"
tmux -V >> "$A" 2>&1
echo "== TMUX ROUND TRIP ==" >> "$A"
printf 'alpha\nbeta\ngamma\n' | tmux load-buffer -w -
echo "load exit=$?" >> "$A"
tmux show-buffer >> "$A" 2>&1
echo "show exit=$?" >> "$A"
echo "== RESULT NOT DISCARDED ==" >> "$A"
grep -c "let _ = copy_to_tmux_buffer" src/cli/viewer.rs >> "$A"
echo "== PHASE-02 CONTRACT STILL HOLDS ==" >> "$A"
grep -c "disarm" src/cli/viewer.rs >> "$A"
grep -nE "try_restore|disable_raw_mode|\.restore\(\)" src/cli/viewer.rs >> "$A"
echo "teardown grep exit=$?  (1 = none found, which is the pass)" >> "$A"
```

## Authorizations

- [ ] May edit `src/cli/viewer.rs`.

No new dependencies — `anyhow` and `std::process` are already in use.
`docs/architecture.md` is **not** authorized, and neither is
`src/cli/input/tty.rs` (no new key parsing: `y` is already a `Key::Char`).

## Out of scope

- **Rehydration** (phase-06), **mouse** (phase-07).
- **Free-form drag selection.** Block-level copy is the shipped affordance;
  the design doc's §"Clipboard" records why.
- **OSC 52 by hand.** `-w` asks tmux to do it. Do not emit escape sequences
  directly, and do not add a fallback path for tmux < 3.2 — the target is 3.7b
  and the E2E block records the version it ran against.
- **Copying anything other than the focused block** — no "copy all", no
  multi-block selection.
- **Undoing anything phases 02–04 established.** `AltScreenGuard` keeps its
  unconditional `Drop`, `viewer.rs` gains no `disarm` and no `try_restore` /
  `disable_raw_mode` / `.restore()`, and `key_action`'s search-mode precedence
  is unchanged. The E2E block re-checks the guard contract.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

# Bug 1 on phase-02b: interactive approval line-editing is inert/garbled (per-byte `commit`), plus dead-code shims, a fake decision-parser test, and no E2E

**Severity:** blocker
**Status:** open
**Filed:** 2026-06-25

## Summary

Sub-deliverable 1 (code-block state) and 4 (default flip) are correct and well
done. But the phase's load-bearing deliverable — **interactive approval through
the ratatui renderer under crossterm raw mode** (sub-deliverables 3a/3b) — is
implemented against the wrong renderer primitive, so typed input and credential
entry are visually broken and emit literal escape bytes into committed cells.
On top of that, the new code carries two `#[allow(dead_code)]` shims, the
"approval decision parser" test exercises a function the production path never
calls, and the required live E2E was not run. This is the green-but-inert
failure mode the M2 calibration protocol exists to catch (README →
"Verification strategy"), the same family as `bug-phase-02a-1`.

## What's wrong

### 1. (blocker) Line-editing + credential entry route per-byte through `commit()`, which inserts one scrollback row per keystroke and renders literal escapes as cells

`RatatuiRenderer::commit` (`src/cli/render_ratatui.rs:165-178`) is the
**plain-text, one-call-per-block** scrollback primitive: it computes
`row_count = lines.matches('\n').count() + 1`, calls
`insert_before(row_count, …)`, and inside the closure does
`buf.set_string(…, Style::default())` with **no ANSI interpretation**
(`parse_ansi_to_spans` exists at `render_ratatui.rs:13` but `commit` does not use
it). Every call to `commit` therefore pushes at least one *new line* into
scrollback above the inline viewport.

The new `read_approval_input` (`src/cli/commands/stream.rs:1286-1343`) echoes a
typed redirect **one byte at a time** through that primitive:

```rust
// stream.rs:1311  — first char of a typed message
let _ = renderer.commit(&format!("{}", ch));
...
// stream.rs:1329-1333 — each subsequent char
Some(b) => { if b >= 0x20 { line.insert(b as char);
    let _ = renderer.commit(&format!("{}", b as char)); } }
// stream.rs:1319-1323 — backspace
Some(b'\x7f' | b'\x08') => { line.backspace();
    let _ = renderer.commit("\x1b[D\x1b[P"); }   // <-- literal ESC bytes into a committed cell
```

Consequences on the ratatui path (i.e. the new default):
- Typing the redirect message `do X instead` commits **12 separate single-char
  scrollback rows** stacked vertically — not a line being typed. `commit("d")`
  → `insert_before(1)` of a row containing `d`; `commit("o")` → another row; etc.
- Backspace emits the literal bytes `\x1b [ D \x1b [ P` to `set_string`, which
  writes them as cells (control char + `[D` + control char + `[P`) — it does
  **not** erase anything. This directly violates the acceptance criterion "no
  literal `\x1b[…` escape bytes in committed cells."
- `prompt_credential_ratatui` (`stream.rs:1460-1492`) has the same shape: the
  `Password: ` label is `commit`-ted without a newline, then each masked `•`
  is a separate `commit("•")` → a new scrollback row per character.

The phase Spec §3a is explicit: *"The typed-message branch needs line editing
under raw mode … reuse the existing input editor rather than reinventing it."*
The code constructs an `InputLine` (`stream.rs:1309`) for the *return value* but
does **not** render through it — it hand-rolls a per-byte echo against the
wrong primitive. The working pattern named in the phase doc Current-state notes
(`read_input_line_inner_ratatui` in `commands/mod.rs:860`, which edits in the
**live `draw` region**, not scrollback) was not reused. The transient Y/N/A
prompt is likewise `commit`-ted into permanent scrollback
(`prompt_y_na_message`, `stream.rs:1351`) instead of drawn in the live viewport;
the Pre-flight asked the executor to decide live-region-vs-scrollback for the
prompt and it chose to permanently commit it.

Net: single-key **Y/N/A** returns the right value (the first byte short-circuits
at `stream.rs:1296-1300`), but the **typed-redirect** and **credential** flows —
both required by the acceptance criteria — are garbled and emit raw escapes.
The returned *string* may be correct while the *rendering* is broken: green
unit logic, inert UX. Exactly the milestone's target failure mode.

### 2. (major) `parse_approval_decision` is dead in production; its tests cover a parallel implementation, not the shipped path

`parse_approval_decision` (`stream.rs:1271-1281`) is marked
`#[allow(dead_code)] // used in tests` and is called **only** by the eight unit
tests at `stream.rs:1687-1742`. The production prompts do **not** call it — every
prompt re-implements the Y/N/A/empty/typed-message match inline (e.g.
`prompt_tool_call_ratatui` at `stream.rs:1431-1457`, `prompt_edit_file_ratatui`
at the `match trimmed.as_str()` arm). The Test plan item — "the approval
primitive's decision parsing maps Y/N/A/empty/typed-message to the correct
`(approved, user_message)` outcomes" — is therefore **not** tested: the test
would still pass if the inline match in `prompt_tool_call_ratatui` were broken.
This is a fake test per review §5 (passes regardless of the real code path) and
a duplicated-logic smell (the inline matches and `parse_approval_decision` can
drift). Also note `parse_approval_decision` returns `(true, None)` for `"a"`,
losing the approve-for-session distinction the real arms encode — so it is not
even a faithful model of the production logic it purports to test.

### 3. (major) `prompt_with_session_approve` is entirely unused dead code

`prompt_with_session_approve` (`stream.rs:1367-1385`) is
`#[allow(dead_code)] // may be used in future prompts` and has **no caller**
anywhere (`grep -rn prompt_with_session_approve src/` returns only the
definition). STANDARDS §2.2: "No premature abstraction … If a symbol is unused,
delete it." The DoD forbids `#[allow(...)]` shims used to mask diagnostics. Both
this and the `parse_approval_decision` allow must go.

### 4. (blocker) The required live E2E was not run — green-but-inert gate unmet

The phase's "End-to-end verification" section and acceptance criteria require a
live tmux run with `tmux capture-pane -p` output quoted (Y approves and the
command runs; a typed redirect course-corrects; a fenced code block renders
highlighted; `DAEMONEYE_RENDERER=legacy` still works). The Update Log states
"E2E tmux verification: not available in executor environment." Per this phase
doc's own terms ("an inert pass will bounce — this is the 'green-but-inert' gap
the milestone exists to catch"), an unverified interactive deliverable is not
done. Given finding #1, the live run would have surfaced the garbled typing
immediately.

## What should happen

Per Spec §3, the approval prompt and its input editing must render **through the
renderer's live region** under crossterm raw mode, reusing the existing
`InputLine`/`InputState` editor the ratatui input loop already uses
(`read_input_line_inner_ratatui`, `commands/mod.rs:860`) — not a hand-rolled
per-byte echo against the plain-text scrollback `commit`. No committed cell may
contain literal `\x1b[` bytes (acceptance criterion). The decision-parsing test
must exercise the code path the production prompts actually run. No
`#[allow(dead_code)]` shims may remain. The live E2E must be run and its
`capture-pane` output quoted.

## How to fix

1. **Route the approval prompt + line editing through the live region.** Draw the
   Y/N/A prompt and the in-progress typed message in the inline viewport via the
   renderer's `draw`/spinner-style transient path (reusing `InputLine` for edit
   state and the same byte-read+redraw loop as `read_input_line_inner_ratatui`),
   so editing happens in place and only the *final* decision/outcome is committed
   to scrollback. Do not echo individual bytes via `commit`. Do the same for the
   credential prompt (mask in the live region; never one `commit("•")` per byte).
   Result: no per-char scrollback rows, no literal `\x1b[` in any committed cell.
2. **Make the decision test cover the shipped path.** Either have every prompt
   call a single shared decision parser (preferred — removes the duplicated
   inline matches and the drift risk) and test that parser, or drive the actual
   `prompt_*_ratatui` function with injected bytes and assert the returned
   `(approved, user_message)` / session-approval mutation. Delete the standalone
   `parse_approval_decision` if it is not the shared parser.
3. **Delete `prompt_with_session_approve`** (or wire it in as the one shared
   primitive if you choose that route) and remove both `#[allow(dead_code)]`
   attributes. The `#[allow(clippy::too_many_arguments)]` on the prompt helpers
   is acceptable only if the arg lists cannot reasonably be grouped; prefer a
   small context struct.
4. **Run the live E2E** under an attached tmux pane with `DAEMONEYE_RENDERER`
   unset (new default = ratatui): ask the AI to run a terminal command, confirm
   Y approves and runs, exercise a **typed redirect** and confirm course-correct,
   ask for a fenced code block and confirm it renders highlighted, and confirm
   `DAEMONEYE_RENDERER=legacy` is unchanged. Quote the `tmux capture-pane -p`
   output in the Update Log.

Keep the parts that are correct: sub-deliverable 1 (`render_line_to_spans` now
`&mut self` toggling `in_code_block`/`code_lang`, `render.rs:1182-1209`, with
real state tests), sub-deliverable 2 (`commit_panel`, `render_ratatui.rs:234-285`
— clean styled panel, no literal escapes), and sub-deliverable 4 (the
`RendererMode::from_env` flip + tests, `commands/mod.rs:16-23`).

## Verification

- [ ] No committed cell contains literal `\x1b[` bytes on the approval path:
      `grep -n 'commit("\\\\x1b\|commit(&format!("{}", ' src/cli/commands/stream.rs`
      returns nothing in the approval helpers; typed input is edited in the live
      region, not echoed per byte to scrollback.
- [ ] `grep -rn "allow(dead_code)" src/cli/commands/stream.rs` returns nothing.
- [ ] The decision test drives the production prompt path (or a parser the
      production prompts actually call), and fails if that logic is broken.
- [ ] `cargo fmt --all`, `cargo build` (zero warnings), `cargo clippy
      --all-targets --all-features -- -D warnings`, `cargo test` all pass.
- [ ] Update Log quotes real `tmux capture-pane -p` output: Y approves + runs a
      command; a typed redirect course-corrects; a fenced code block renders
      highlighted; `DAEMONEYE_RENDERER=legacy` unchanged.

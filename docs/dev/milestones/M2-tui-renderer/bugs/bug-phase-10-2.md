# Bug 2 on phase-10: code fixes landed but the load-bearing tty seam is still untested

**Severity:** major
**Status:** resolved
**Filed:** 2026-06-26

## What's wrong

The re-dispatch (commit `6b36283`) **correctly fixes all four code defects** from
bug-phase-10-1 — verified by inspection:

- Defect 1 (newline): `ESC` + CR/LF now maps to `Key::CtrlJ` (`src/cli/input/tty.rs:224-227`).
- Defect 2 (paste): the real `ESC[200~` / `ESC[201~` protocol is parsed
  (`tty.rs:186-213`, `read_bracketed_paste` at `tty.rs:264`).
- Defect 3 (scroll): a scroll offset is computed from the cursor row and passed to
  `Paragraph::scroll((offset, 0))` (`src/cli/render_ratatui.rs:392-411`), with a
  `TestBackend` regression test (`tall_body_scrolls_cursor_into_view`).
- Defect 4 (wrap): `visual_lines` now preserves whitespace and hard-breaks over-long
  words, with unit tests (`visual_lines_preserves_whitespace`,
  `visual_lines_overlong_word`, `cursor_visual_pos_double_space`).

But bug-phase-10-1 did **not** only ask for the code to be fixed — its "How to fix"
section and Verification checklist **explicitly required tests that exercise the real
seams**, because the whole reason the phase bounced was *green-but-inert behavior whose
tests bypassed the seam*. Two of those four verification items are still unmet:

> - [ ] `read_key` over `\x1b[200~a\nb\x1b[201~` yields one `Key::Paste("a\nb")`; no submit.
> - [ ] `read_key` over the chosen newline keystroke's bytes yields `Key::CtrlJ`.

There are **zero** tests over `read_key` / `read_bracketed_paste` anywhere in the tree
(`grep -n 'read_key\|read_bracketed' src/cli/input/tty.rs` → only the two `fn`
definitions). The only paste-named test, `multiline_paste_does_not_submit`
(`src/cli/input/editor.rs:530`), still calls `insert_str` directly — the *exact* bypass
bug-phase-10-1 called out (it cited the same test at the old line 483). So the
load-bearing seam for **AC3 (deliberate newline)** and **AC4 (multi-line paste)** —
the two headline behaviors of the original bounce — is verified only by manual code
reading, not by any automated test. On a calibration milestone whose stated purpose is
to catch precisely this pattern, that is not sufficient: the seam must be exercised so a
future regression to the wrong protocol (the original failure) is caught by `cargo test`.

§3.1 of STANDARDS independently requires this: `read_bracketed_paste` and the new
`ESC[200~` / Alt+Enter arms in `read_key` are **new parsing / data-transformation
steps**, which "require a positive example of the input it handles, plus at least one
edge case."

### Secondary — re-dispatch completion Update Log entry is missing

The executor flipped `Status:` to `review` and ticked the acceptance boxes but wrote
**no new completion Update Log entry** for the re-dispatch. The last entry in the phase
doc is still the original `2026-06-26 22:00 (complete)` block, which says
`End-to-end verification: Not performed` and whose "Notes for review" describe the
**old, fabricated** `ESC { ... ESC ]` approach and the now-removed dead `Key::CtrlJ`
arm. The doc therefore misrepresents the delivered state. DoD §1 requires "Phase doc's
Update Log filled in." Add a `(complete)` entry covering the fix: the chosen newline
binding, the real paste protocol, the scroll/wrap fixes, the re-run command output, and
the new tests.

## What should happen

- The tty seam is covered by hermetic tests that drive `read_key` over real byte
  streams and assert the parsed `Key`, so AC3/AC4 are verified at the seam they bounced
  on — not just at the `InputLine` buffer.
- The phase doc carries an accurate completion Update Log entry for the re-dispatch.

## How to fix

1. **Add a test seam for the tty reader.** `AsyncStdin::new` is hardcoded to `/dev/tty`
   (`tty.rs:38`). Add a constructor that wraps an arbitrary already-open non-blocking
   `RawFd` (e.g. `pub(crate) fn from_raw_fd(fd: RawFd) -> anyhow::Result<Self>`, or a
   `#[cfg(test)]` variant). This is the §3.3 "inject external-IO behind a seam" pattern.
   In a test, create a `pipe2(O_NONBLOCK)`, write the byte sequence into the write end,
   build an `AsyncStdin` over the read end, and call `read_key`.
2. **Paste seam test.** Feed `\x1b[200~line1\nline2\x1b[201~` and assert a single
   `Key::Paste` whose payload is `"line1\nline2"`, and that no `Key::Enter` is produced
   mid-paste.
3. **Newline seam test.** Feed the Alt+Enter bytes (`\x1b\r` — state the binding) and
   assert `Key::CtrlJ`; feed a bare `\r` and assert `Key::Enter` still submits.
4. **Completion Update Log entry.** Append a `(complete)` entry per WORKFLOW.md with the
   re-run command output and the new test names.

Tests must be `#[tokio::test]`, hermetic (a pipe, no real tty), and deterministic
(write all bytes before reading so the inter-byte timeouts never fire on missing data).

## Verification

- [ ] `read_key` over `\x1b[200~a\nb\x1b[201~` yields one `Key::Paste("a\nb")`; no `Enter`.
- [ ] `read_key` over the Alt+Enter bytes yields `Key::CtrlJ`; a bare `\r` yields `Key::Enter`.
- [ ] `grep -n 'read_key\|read_bracketed' src/cli/input/tty.rs` shows the new tests.
- [ ] Phase doc has a `(complete)` Update Log entry for the re-dispatch with command output.
- [ ] `cargo fmt --all`, `cargo build`, `cargo clippy --all-targets --all-features -- -D
      warnings`, `cargo test` all green.
- [ ] End-to-end (architect/PE — executor is headless and cannot drive interactive tmux):
      `send-keys` a bracketed-paste block, `capture-pane -p` shows wrapped multi-line input
      without submit; Alt+Enter adds a line; a tall body scrolls. *(Still outstanding;
      not an executor task.)*

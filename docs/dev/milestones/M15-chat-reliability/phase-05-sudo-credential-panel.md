# Phase 05: sudo credential panel — themed masked-input dialog

**Milestone:** M15 — Chat Reliability & Dialog UX
**Status:** in-progress
**Depends on:** phase-04 (reuses its panel design and span idioms)
**Estimated diff:** ~200 lines
**Tags:** language=rust, kind=feature, size=s

## Goal

The sudo password prompt is a plain `  Password: ` line with bullet masking.
Rebuild it in the phase-04 panel style: a rounded blood-red bordered dialog
(yellow bold title), the daemon's prompt text as the detail row, a dim
`[Enter] submit  [Esc] cancel` hint row, and the masked input row. Masking,
cancel-clears-credential, and the scrollback record are byte-for-byte
unchanged — the panel only ever receives the bullet display buffer, never
the real credential.

## Architecture references

Read before starting:

- `src/cli/render_ratatui.rs` — `draw_approval_panel` and
  `approval_options_line` (added by the previous phase) are the templates
  this phase mirrors.
- `src/cli/commands/stream.rs` — `prompt_credential_ratatui`, the function
  being rewired.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

**The flow** (`stream.rs:1057`, `prompt_credential_ratatui`): commits
`⚠ {prompt}` to scrollback, then loops on bytes drawing
`renderer.draw_prompt("  Password: ", &cred_display, status)`:

```rust
// Two buffers: cred_real holds the actual typed value; cred_display holds masked bullets.
let prompt_text = "  Password: ";
let mut cred_real = String::new();
let mut cred_display = crate::cli::input::InputLine::new();
let _ = renderer.draw_prompt(prompt_text, &cred_display, status);

while let Some(b) = stdin.read_byte().await {
    match b {
        b'\r' | b'\n' => break,
        b'\x7f' | b'\x08' => {
            cred_real.pop();
            cred_display.backspace();
            let _ = renderer.draw_prompt(prompt_text, &cred_display, status);
        }
        b'\x03' | b'\x1b' => {
            cred_real.clear();
            break;
        }
        c if c >= 0x20 => {
            cred_real.push(c as char);
            cred_display.insert('•');
            let _ = renderer.draw_prompt(prompt_text, &cred_display, status);
        }
        _ => {}
    }
}

// Commit the final masked line to scrollback.
let _ = renderer.commit(&format!("{}\n", prompt_text));
cred_real
```

The security-relevant invariants, all preserved by this phase: the renderer
only ever receives `cred_display` (bullets); `cred_real` is cleared on
Esc/Ctrl+C; nothing containing `cred_real` is committed to scrollback or
logged. (`cred_real` is returned to the caller which sends
`Request::CredentialResponse` — `stream.rs:383`; the daemon side wraps it
in `Zeroizing`. Unchanged.)

**The templates in-tree** (added by the previous phase): `draw_approval_panel`
(rounded red-bordered panel + yellow bold title + three content `Line`s +
status-bar bottom row + `area.height < 6` fallback to `draw_prompt`) and
`approval_options_line` (the red/yellow span builder):

```rust
let key =
    |c: &'static str| Span::styled(c, Style::default().fg(yellow).add_modifier(Modifier::BOLD));
let br = |s: &'static str| Span::styled(s, Style::default().fg(red));
```

Its tests (`approval_panel_*` in the `mod tests` of `render_ratatui.rs`)
show the buffer-assertion idiom and the `approval_test_status()` /
`buffer_rows()` helpers — reuse those helpers, do not duplicate them.

## Spec

### 1. Hint-line builder — in `src/cli/render_ratatui.rs`

Add next to `approval_options_line`, same span idiom:

```rust
/// The credential-dialog hint line: `[Enter] submit  [Esc] cancel`,
/// yellow key words in blood-red brackets, dim tail.
fn credential_hint_line(red: Color, yellow: Color) -> Line<'static> {
    let key =
        |c: &'static str| Span::styled(c, Style::default().fg(yellow).add_modifier(Modifier::BOLD));
    let br = |s: &'static str| Span::styled(s, Style::default().fg(red));
    Line::from(vec![
        br("["), key("Enter"), br("]"),
        Span::styled(" submit", Style::default().fg(red)),
        Span::raw("  "),
        br("["), key("Esc"), br("]"),
        Span::styled(" cancel", Style::default().fg(red)),
    ])
}
```

### 2. `draw_credential_panel` — in `src/cli/render_ratatui.rs`

Add directly below `draw_approval_panel`, mirroring its body exactly (same
`Layout` split, same `Block` shape, same status-bar bottom row, same
`area.height < 6` fallback — for the fallback call
`self.draw_prompt("  Password: ", input, status)`):

```rust
/// Draw the live region as a themed credential dialog: the phase-04 panel
/// shape with the daemon's prompt text as the detail row, the Enter/Esc
/// hint row, and the masked input row. The caller passes the bullet
/// display buffer — the real credential never reaches the renderer.
pub fn draw_credential_panel(
    &mut self,
    title: &str,
    detail: &str,
    input: &InputLine,
    status: &StatusBarState<'_>,
) -> Result<(), B::Error>
```

The three content `Line`s (replacing `draw_approval_panel`'s):

1. `truncate_with_ellipsis(detail, inner_width)` styled `Color::Gray`
   (same `inner_width = area.width.saturating_sub(2)` computation);
2. `credential_hint_line(red, yellow)`;
3. `Span::styled("› ", yellow)` + `Span::raw(input.as_str())` — identical to
   the approval panel's input row.

### 3. Rewire `prompt_credential_ratatui` — in `src/cli/commands/stream.rs`

Replace each of the three
`renderer.draw_prompt(prompt_text, &cred_display, status)` calls with

```rust
renderer.draw_credential_panel("sudo password", prompt, &cred_display, status)
```

(`prompt` is the function's `&str` parameter — the daemon's
`[sudo] password required for: <cmd>` text.) Everything else is untouched:
the leading `⚠` scrollback commit, the byte loop (`cred_real` /
`cred_display` parallel updates, `cred_real.clear()` on Esc/Ctrl+C, the
`c >= 0x20` filter), and the trailing masked-line commit. The
`prompt_text` local remains only if still used by the trailing commit —
keep that commit's output byte-identical (`"  Password: \n"`).

### 4. Unit tests — in the `mod tests` of `src/cli/render_ratatui.rs`

Reuse the previous phase's `approval_test_status()` and `buffer_rows()`
helpers and the same renderer-construction shape. Write the tests named in
§ Test plan.

### 5. Capture the end-to-end evidence

Run the block in § End-to-end verification verbatim and paste its output
into a new Update Log entry titled
`### Update — <date> (end-to-end verification)`.

## Acceptance criteria

- [ ] `draw_credential_panel` renders the yellow bold title
      ` sudo password `, blood-red rounded corners, and a hint row
      containing `[Enter] submit` and `[Esc] cancel` with the key words
      yellow.
- [ ] An `InputLine` holding `•••` renders three bullets in the input row —
      and a buffer drawn from the display buffer alone can never contain
      the real credential (the test passes a real-looking string to
      *nothing*: only bullets exist in the panel's inputs).
- [ ] A 300-char detail row truncates with `…` before the right border, no
      panic on an 80-col TestBackend.
- [ ] A live region shorter than 6 rows falls back without panicking.
- [ ] `prompt_credential_ratatui` no longer calls `draw_prompt`
      (`awk '/fn prompt_credential_ratatui/,/^}/' src/cli/commands/stream.rs | grep -c draw_prompt`
      prints `0`), while masking, Esc-clears, and both scrollback commits
      are byte-identical.
- [ ] `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
      and `cargo test` all pass.

## Test plan

All in `mod tests` of `src/cli/render_ratatui.rs`:

- `credential_panel_title_and_hint` — draw with empty input; a buffer row
  contains `[Enter] submit` and `[Esc] cancel`; `E` cells of the key words
  are yellow is NOT required (multi-char keys) — instead assert the corner
  cells (`╭╯`) are palette red and the title row contains `sudo password`.
- `credential_panel_shows_bullets` — an `InputLine` with three `•` inserted
  renders `•••` in the input row (find the row containing `›`).
- `credential_panel_truncates_long_detail` — 300-char detail → the detail
  row ends with `…│`.
- `credential_panel_short_region_falls_back` — renderer on
  `Viewport::Inline(3)` draws without panicking.

## End-to-end verification

```sh
cd /home/matt/src/daemoneye
cargo fmt --check 2>&1 | tail -5; echo "exit=${PIPESTATUS[0]}"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -5; echo "exit=${PIPESTATUS[0]}"
cargo test 2>&1 | tail -10; echo "exit=${PIPESTATUS[0]}"
cargo test --lib credential_panel 2>&1 | tail -10; echo "exit=${PIPESTATUS[0]}"
awk '/fn prompt_credential_ratatui/,/^}/' src/cli/commands/stream.rs | grep -c draw_prompt; echo "exit=$?"
```

The `grep -c` must print `0` (and exit 1 — that is the success case): the
credential flow no longer uses the plain prompt. Redirect to a file and
paste the file's contents; never retype or summarize.

Live verification (a real uncached sudo run through chat showing the themed
credential dialog; correct password accepted; Esc cancel) is performed
**architect-side at milestone close** together with the phase-02 sudo
checks.

## Authorizations

- Edit `src/cli/render_ratatui.rs` and `src/cli/commands/stream.rs` only.
- Run the gate commands. No daemon restart, no tmux interaction, no sudo
  invocation, no files outside the repo.

## Out of scope

- Any change to credential handling: `cred_real` lifecycle, the
  `CredentialResponse` IPC, daemon-side `Zeroizing` — rendering only.
- The approval panel (previous phase) and the other prompt flows
  (`prompt_pane_select_ratatui` etc.).
- Viewport resizing — same 6-row constraint as the approval panel.

## Update Log

### Update — 2026-08-14 (created)

Phase drafted by the architect. Mirrors the approval panel for the
credential prompt: `credential_hint_line` + `draw_credential_panel` +
three-call rewire in `prompt_credential_ratatui`. Security invariants
pinned: renderer sees only the bullet buffer; Esc clears; scrollback
commits byte-identical. Status: in-progress.

### Update — 2026-08-14 22:42 (started)

Executor started phase-05. Plan: `credential_hint_line` +
`draw_credential_panel` in `render_ratatui.rs`, three-call rewire in
`prompt_credential_ratatui`, four buffer-assertion tests, then the
end-to-end evidence block.

### Update — 2026-08-14 22:50 (end-to-end verification)

Ran the phase doc's § End-to-end verification block verbatim; output
captured to `target/phase05-e2e.txt` and pasted below:

```
exit=0
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s
exit=0
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.15s

   Doc-tests daemoneye

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

exit=0
     Running unittests src/lib.rs (target/debug/deps/daemoneye-b60224cb24515ede)

running 4 tests
test cli::render_ratatui::tests::credential_panel_short_region_falls_back ... ok
test cli::render_ratatui::tests::credential_panel_truncates_long_detail ... ok
test cli::render_ratatui::tests::credential_panel_shows_bullets ... ok
test cli::render_ratatui::tests::credential_panel_title_and_hint ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 1261 filtered out; finished in 0.00s

exit=0
0
exit=1
```

- `cargo fmt --check` → exit=0 (clean).
- `cargo clippy --all-targets --all-features -- -D warnings` → exit=0.
- `cargo test` → exit=0 (all suites green, 1 pre-existing ignored).
- `cargo test --lib credential_panel` → 4/4 passed.
- `awk '/fn prompt_credential_ratatui/,/^}/' src/cli/commands/stream.rs | grep -c draw_prompt` → prints `0` (exit 1, the success case): the credential flow no longer uses the plain prompt.

Live verification (real uncached sudo run through chat) is deferred to
architect-side milestone close per the phase doc.

# Phase 04: command-approval panel — themed in-viewport dialog

**Milestone:** M15 — Chat Reliability & Dialog UX
**Status:** in-progress
**Depends on:** none (phase-05 depends on this phase's panel)
**Estimated diff:** ~300 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

The command-approval prompt is a plain red text line
(`  Approve? [Y]es  [A]pprove for session  [N]o  or type a message › `)
drawn in the input box. Rebuild it as a themed, multicolor, bordered panel
rendered **inside the existing 6-row inline viewport** — modal-like, on the
daemoneye palette (blood-red borders, bright-yellow key letters), with the
Y/A/N + redirect semantics byte-for-byte unchanged. Phase-05 will reuse this
panel for the sudo credential prompt.

**Design note:** the panel fits the existing `VIEWPORT_ROWS = 6` live region
exactly (border, summary, options, input, border, status bar), so this phase
does **not** resize or rebuild the Terminal. Do not touch `Viewport::Inline`
sizing, `reanchor`, or `VIEWPORT_ROWS`.

## Architecture references

Read before starting:

- `src/cli/render_ratatui.rs` — the renderer; the new draw mode lives here.
- `src/cli/commands/stream.rs:780–1000` — the approval flow being rewired.
- `src/cli/palette.rs` — `Palette::red()` / `Palette::yellow()`, the two
  theme colors.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

**The flow** (`stream.rs:904`, `prompt_tool_call_ratatui`): commits a
bordered command panel to scrollback (`commit_panel(where_label, body,
false)` — this stays), then prompts via `prompt_with_session_approve` →
`read_approval_input` (`stream.rs:820`), which redraws
`renderer.draw_prompt(prompt_text, &line, status)` on every keystroke.
Single-key `y`/`n`/`a` returns immediately; any other first character starts
a typed redirect message; Enter empty / Esc / Ctrl+C deny. Parsing is
`parse_approval_response` (`stream.rs:801`) — **unchanged by this phase**.

**The current prompt rendering** (`render_ratatui.rs:642`,
`render_prompt_region`): one red bold prompt line + gray-bordered input box
+ status bar. The status-bar block it renders is the shape the new panel
must keep as its bottom row:

```rust
let uptime = fmt_uptime(start_time.elapsed());
let status_text = format!(
    " session:{} · {} · up {} ",
    short_session(session_id),
    model,
    uptime,
);
let status_block = Block::default().borders(Borders::NONE).style(
    Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::DIM),
);
let status_para = Paragraph::new(Line::from(Span::raw(status_text))).block(status_block);
frame.render_widget(status_para, chunks[1]);
```

**The multicolor idiom** — the spinner line (`render_ratatui.rs:395–408`)
composes red/yellow spans exactly the way the options line should:

```rust
let spinner_line = Line::from(vec![
    Span::styled(open, blood_red),
    Span::styled(center, bright_yellow),
    Span::styled(close, blood_red),
    Span::styled(format!(" {verb}"), blood_red),
    Span::styled(".".repeat(dot_count), bright_yellow),
]);
```

**Helpers that already exist and must be reused:** `truncate_with_ellipsis`
(`render_ratatui.rs:760`), `split_spinner_row`, `fmt_uptime`,
`short_session`, `make_test_renderer` (tests), and the palette getters
`self.palette.red()` / `self.palette.yellow()`.

**The buffer-assertion test idiom**
(`render_ratatui.rs:1288`, `commit_panel_uses_blood_red_border_and_yellow_title`):
draw, then iterate `backend.buffer()` (+ `backend.scrollback()`) cells,
filter by glyph, assert `cell.style().fg`. New panel tests follow this
shape (the panel is live-region only, so `backend.buffer()` alone
suffices).

## Spec

### 1. Options-line builder — in `src/cli/render_ratatui.rs`

Add a module-level function (near `render_prompt_region`):

```rust
/// The approval options line: bright-yellow key letters in blood-red
/// brackets, dim redirect affordance. `session_label` is "session" or
/// "sudo session".
fn approval_options_line(
    session_label: &str,
    red: Color,
    yellow: Color,
) -> Line<'static> {
    let key = |c: &'static str| Span::styled(c, Style::default().fg(yellow).add_modifier(Modifier::BOLD));
    let br = |s: &'static str| Span::styled(s, Style::default().fg(red));
    let word = |s: String| Span::styled(s, Style::default().fg(red));
    Line::from(vec![
        br("["), key("Y"), br("]"), word("es".to_string()),
        Span::raw("  "),
        br("["), key("A"), br("]"), word(format!("pprove for {session_label}")),
        Span::raw("  "),
        br("["), key("N"), br("]"), word("o".to_string()),
        Span::raw("  "),
        Span::styled("or type to redirect", Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM)),
    ])
}
```

### 2. `draw_approval_panel` — in `src/cli/render_ratatui.rs`

Add to the generic `impl<B: Backend> RatatuiRenderer<B>` block (next to
`draw_prompt`):

```rust
/// Draw the live region as a themed approval dialog: a rounded blood-red
/// bordered panel (yellow title) holding the command summary, the
/// multicolor Y/A/N options line, and the editable input line; the status
/// bar keeps the bottom row. Transient — leaves no residue in scrollback.
pub fn draw_approval_panel(
    &mut self,
    title: &str,
    summary: &str,
    session_label: &str,
    input: &InputLine,
    status: &StatusBarState<'_>,
) -> Result<(), B::Error>
```

Behavior:

- If `area.height < 6`: fall back to exactly what `draw_prompt` does today
  (call the same `render_prompt_region` with a
  `build_approval_prompt`-style string is NOT available here — instead
  render the options as plain text via `render_prompt_region(frame, area,
  "Approve? [Y]es [A]pprove [N]o › ", ...)`). No panic, input still
  editable.
- Otherwise split the area vertically: `Constraint::Min(1)` (panel) +
  `Constraint::Length(1)` (status bar).
- Panel: `Block::default().borders(Borders::ALL)
  .border_type(BorderType::Rounded)
  .border_style(Style::default().fg(self.palette.red()))
  .title(Span::styled(format!(" {title} "),
  Style::default().fg(self.palette.yellow()).add_modifier(Modifier::BOLD)))`.
- Panel inner content, three `Line`s in a `Paragraph` (no wrap):
  1. the summary, passed through
     `truncate_with_ellipsis(summary, inner_width)` where `inner_width` is
     the panel's inner width (`area.width.saturating_sub(2)` as usize),
     styled `Color::Gray`;
  2. `approval_options_line(session_label, self.palette.red(), self.palette.yellow())`;
  3. the input line: `Span::styled("› ", yellow)` + `Span::raw(input.as_str())`.
- Status bar: the same block quoted in Current state (copy that shape —
  `fmt_uptime` / `short_session` / DarkGray DIM).

### 3. Panel-driven key loop — in `src/cli/commands/stream.rs`

Add alongside `read_approval_input` (do not modify it — other flows still
use it):

```rust
/// `read_approval_input`, panel edition: identical key semantics, but every
/// redraw renders the themed approval panel instead of the plain prompt.
async fn read_approval_input_panel(
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    stdin: &AsyncStdin,
    title: &str,
    summary: &str,
    session_label: &str,
    status: &crate::cli::render::StatusBarState<'_>,
) -> String
```

Copy `read_approval_input`'s body verbatim, replacing every
`renderer.draw_prompt(prompt_text, &line, status)` with
`renderer.draw_approval_panel(title, summary, session_label, &line, status)`.
The key semantics (first-byte y/n/a shortcut, Enter/Esc/Ctrl+C, printable
byte filter `b >= 0x20`, backspace) must be byte-for-byte identical.

### 4. Rewire the command-approval flow — in `src/cli/commands/stream.rs`

In `prompt_tool_call_ratatui` (`stream.rs:904`), replace the
`build_approval_prompt` + `prompt_with_session_approve` pair with:

```rust
let session_label = if is_sudo { "sudo session" } else { "session" };
let summary = format!("$ {}", command);
let input = read_approval_input_panel(
    renderer, stdin, "approve command", &summary, session_label, status,
).await;
let (approved, is_session, user_msg) = parse_approval_response(&input);
```

Everything else in the function — the scrollback `commit_panel` of the
command, the auto-approved short-circuit, the `✓ approved` / `↩ redirecting`
/ `✗ skipped` commit lines, the `approval.sudo` / `approval.regular`
mutation — stays exactly as it is.

Do **not** delete `build_approval_prompt`, `prompt_with_session_approve`,
or `read_approval_input`: the other approval flows (file edits, scripts,
runbooks, tmux_control) still use them and are out of scope.

### 5. Unit tests — in the `mod tests` of `src/cli/render_ratatui.rs`

Follow the `commit_panel_uses_blood_red_border_and_yellow_title` idiom
(`make_test_renderer`, draw, assert on `backend.buffer()` cells). Write the
tests named in § Test plan.

### 6. Capture the end-to-end evidence

Run the block in § End-to-end verification verbatim and paste its output
into a new Update Log entry titled
`### Update — <date> (end-to-end verification)`.

## Acceptance criteria

- [ ] `draw_approval_panel` renders `[Y]es`, `[A]pprove for session`, `[N]o`
      with the `Y`/`A`/`N` letter cells bright-yellow and the panel's
      rounded corner cells (`╭╮╰╯`) blood-red (palette colors).
- [ ] With `session_label = "sudo session"` the options row reads
      `[A]pprove for sudo session`.
- [ ] A 300-char summary is truncated with a trailing `…` and does not
      panic on an 80-col TestBackend.
- [ ] Typed input appears in the panel's input row on redraw.
- [ ] A live region shorter than 6 rows falls back without panicking.
- [ ] `parse_approval_response`, `build_approval_prompt`,
      `read_approval_input`, and `prompt_with_session_approve` are
      unmodified (their existing tests pass untouched).
- [ ] `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
      and `cargo test` all pass.

## Test plan

All in `mod tests` of `src/cli/render_ratatui.rs`:

- `approval_panel_options_multicolor` — after `draw_approval_panel(...,
  "session", ...)` the buffer contains the glyph sequence `[Y]es` (assert
  via a rendered-row string) and the `Y`, `A`, `N` key cells have
  `fg == palette yellow` while `╭`/`╯` corner cells have
  `fg == palette red`.
- `approval_panel_sudo_session_label` — buffer row contains
  `[A]pprove for sudo session`.
- `approval_panel_truncates_long_summary` — 300-char summary → rendered row
  ends with `…`, no panic.
- `approval_panel_shows_typed_input` — an `InputLine` containing `why`
  renders `why` in the input row.
- `approval_panel_short_region_falls_back` — a renderer built on
  `Viewport::Inline(3)` draws without panicking.

## End-to-end verification

```sh
cd /home/matt/src/daemoneye
cargo fmt --check 2>&1 | tail -5; echo "exit=${PIPESTATUS[0]}"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -5; echo "exit=${PIPESTATUS[0]}"
cargo test 2>&1 | tail -10; echo "exit=${PIPESTATUS[0]}"
cargo test --lib approval_panel 2>&1 | tail -12; echo "exit=${PIPESTATUS[0]}"
grep -n "fn read_approval_input\b" src/cli/commands/stream.rs; echo "exit=$?"
```

The final grep must still find the untouched `read_approval_input` (exit=0)
— proof the legacy path survives for the other flows.

Live verification (a real chat approval round trip showing the themed
panel; Y, A, N, and redirect each exercised; no regression to the approval
semantics proven at M14) is performed **architect-side at review**.

## Authorizations

- Edit `src/cli/render_ratatui.rs` and `src/cli/commands/stream.rs` only.
- Run the gate commands. No daemon restart, no tmux interaction, no files
  outside the repo.

## Out of scope

- The sudo credential prompt (phase-05 — it reuses this panel).
- The other approval flows (file edits, scripts, runbooks, tmux_control)
  — they keep `read_approval_input`; migrating them is future work.
- Viewport resizing / Terminal rebuild — the panel fits the existing 6
  rows by design.
- Changing approval semantics, `parse_approval_response`, or the
  scrollback record lines.

## Update Log

### Update — 2026-08-14 (created)

Phase drafted by the architect. Design: themed panel within the existing
6-row viewport (no Terminal rebuild — deliberately avoids the reanchor
risk class fixed in phase-03). Options builder + `draw_approval_panel` +
panel-driven key loop; legacy prompt path preserved for the other approval
flows. Status: todo.

### Update — 2026-08-14 21:26 (started)

**Executor:** headless executor, M15 phase-04.

Status: in-progress. Implementing the themed approval panel:
`approval_options_line` + `draw_approval_panel` in `render_ratatui.rs`,
`read_approval_input_panel` + rewire of `prompt_tool_call_ratatui` in
`stream.rs`, buffer-assertion unit tests, then end-to-end evidence.

### Update — 2026-08-14 (end-to-end verification)

**Executor:** headless executor, M15 phase-04.

Ran the phase doc's § End-to-end verification block verbatim:

```
$ cargo fmt --check 2>&1 | tail -5; echo "exit=${PIPESTATUS[0]}"
exit=0

$ cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -5; echo "exit=${PIPESTATUS[0]}"
    Checking daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.32s
exit=0

$ cargo test 2>&1 | tail -10; echo "exit=${PIPESTATUS[0]}"
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.15s

   Doc-tests daemoneye

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
exit=0

$ cargo test --lib approval_panel 2>&1 | tail -12; echo "exit=${PIPESTATUS[0]}"
    Finished `test` profile [unoptimized + debuginfo] target(s) in 2.27s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-b60224cb24515ede)

running 5 tests
test cli::render_ratatui::tests::approval_panel_short_region_falls_back ... ok
test cli::render_ratatui::tests::approval_panel_truncates_long_summary ... ok
test cli::render_ratatui::tests::approval_panel_sudo_session_label ... ok
test cli::render_ratatui::tests::approval_panel_shows_typed_input ... ok
test cli::render_ratatui::tests::approval_panel_options_multicolor ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 1256 filtered out; finished in 0.00s
exit=0

$ grep -n "fn read_approval_input\b" src/cli/commands/stream.rs; echo "exit=$?"
820:async fn read_approval_input(
exit=0
```

Full `cargo test` suite: 1261 passed, 0 failed (lib), plus integration
suites all green. The final grep proves the untouched legacy
`read_approval_input` survives at `stream.rs:820` for the other approval
flows (file edits, scripts, runbooks, tmux_control).

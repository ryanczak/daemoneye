# M13 — Chat UX Polish

**Goal:** `daemoneye chat` renders correctly on any terminal — colors survive
remote/tmux color-depth limits, the cursor sits on the character it edits, the
throbber is flush-left, history entries are attributed to `user@host`, command
runtimes live in the output panel's border, and the input dialog stays pinned
to the bottom through resizes and tmux window switches.

**Status:** planning

**Depends on:** M12 — Full-View tmux Integration

**Exit criteria:**

- On a terminal without RGB capability (e.g. tmux quantizing for a non-truecolor
  outer terminal), the banner, throbber and panel borders render in *distinct*
  red and yellow — verified by a color-depth-detection unit suite plus a live
  check on pinky.home.planetfoo.org.
- The input-box cursor column equals the column of the character under edit at
  every position of a wrapped multi-line input — pinned by tests that assert
  exact `set_cursor_position` coordinates against `TestBackend`.
- The throbber's first glyph renders at column 0.
- User messages in chat history are titled `<user>@<shorthost>` (e.g.
  `matt@scrappy`), never the literal `you`; the fallback chain when a source is
  missing is pinned by tests.
- `ToolFinished` no longer produces a standalone `result` panel; the run time
  and ✓/✗ status appear right-justified in the bottom border of the command's
  `output` panel.
- Panel body lines longer than the terminal width word-wrap instead of being
  truncated with `…`, and the user panel's bottom border carries the legacy
  ` turn N · <tokens> / <window> (<pct>%) ` label again.
- The dead legacy printers in `src/cli/render.rs` are deleted; everything
  still compiled from that file has a live caller.
- A SIGWINCH or tmux focus-gain arriving *during* an in-flight streamed turn
  re-anchors the viewport: the input dialog is at the bottom of the terminal,
  chat history above it, no scroll-up required. Verified live in tmux by
  switching windows mid-turn.
- Four gates green (`cargo fmt --all`, `cargo build`,
  `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`).

## Architecture references

- `docs/architecture.md#11-transport--process-layer` — where the CLI client sits.
- `CLAUDE.md` § Key files — `src/cli/` inventory.

## Phases

| #  | Phase                                                              | Status |
|----|--------------------------------------------------------------------|--------|
| 01 | color-depth-palette (phase-01-color-depth-palette.md)              | done          |
| 02 | throbber-and-identity (phase-02-throbber-and-identity.md)          | done         |
| 03 | runtime-in-border (phase-03-runtime-in-border.md)                  | done   |
| 04 | cursor-alignment (phase-04-cursor-alignment.md)                    | done         |
| 05 | resize-and-reanchor (phase-05-resize-and-reanchor.md)              | in-progress |

Phases 01–03 are independent of each other. 04 and 05 both touch
`render_ratatui.rs`'s live-region path and run last, in that order, so 05's
re-draw work lands on the corrected cursor math.

## Notes

### Derived code facts (investigated 2026-08-09; re-verify before each dispatch)

The chat UI is **ratatui 0.30 + crossterm 0.29** with `Viewport::Inline(6)`
(`src/cli/render_ratatui.rs:183`, `VIEWPORT_ROWS` at `:119`); scrollback is
written via `Terminal::insert_before` (`commit*` methods). A legacy raw-ANSI
path (`src/cli/render.rs`, `src/cli/status.rs`, `src/cli/markdown/*`) coexists
and is bridged by `parse_ansi_to_spans` / `apply_sgr`
(`render_ratatui.rs:13/:59`).

**Issue 1 — monotone colors on remote (phase 01).** There is *no* terminal
capability detection anywhere in `src/` (zero hits for
`COLORTERM`/`TERM`/`terminfo`/`truecolor`). Every chat color is an
unconditional `Color::Rgb`: banner `Rgb(180,0,0)` / `Rgb(220,160,0)`
(`src/cli/commands/chat.rs:754`), spinner (`render_ratatui.rs:269-287`), panel
borders/titles (`render_ratatui.rs:354-362`). ratatui emits these as SGR
`38;2;r;g;b` unconditionally; tmux passes RGB through only when the outer
terminal is declared RGB-capable, otherwise it quantizes — on a 16-color
mapping both values collapse toward red, which is exactly the pinky symptom.
Fix shape: detect color depth once (respect `COLORTERM=truecolor|24bit`, tmux
`Tc`/`RGB` terminfo, `$TERM` fallback), centralize the palette in one module
with per-depth values (truecolor → today's RGB; 256 → e.g. red 124/160, yellow
178/136, matching the legacy `38;5;88`/`38;5;136` pair already used at
`render.rs:120-126`; 16 → red/yellow named), and swap every `Rgb` site to it.
Also: `apply_sgr` (`render_ratatui.rs:59-93`) parses `38;5;<idx>` but silently
drops `38;2;r;g;b` — any legacy truecolor string loses its color in the bridge;
and `status.rs:13-35` is a third hardcoded-truecolor palette. Both belong to
the same phase's palette unification.

**Issue 2 — cursor misalignment (phase 04).** Three concrete defects in the
live region:

1. **Two different wrappers.** The cursor position comes from the hand-written
   word-wrap `InputLine::visual_lines` / `cursor_visual_pos`
   (`src/cli/input/editor.rs:184/:301`), but the text is drawn by ratatui's
   `Paragraph::wrap(Wrap { trim: false })` (`render_ratatui.rs:467-470`).
   They disagree on whitespace/word-boundary handling, so cursor and glyph
   diverge on any wrapped line. Fix shape: render the input text from the same
   `visual_lines` output (drop `Paragraph::wrap` for the input box) so one
   wrapper is authoritative.
2. **Clamp lands on the border.** `render_live_region` (`:474-480`) clamps
   `col.min(content_area.width.saturating_sub(2))` — inner content ends at
   `width-2` *before* the `x+1` offset, so the clamp can place the cursor on
   the right border column; same for `y`/bottom.
3. **Width mismatch.** `draw` computes `content_width = area.width - 2` from
   the ratatui frame, while `chat.rs:670-688` moves the cursor using
   `chat_width - 2` where `chat_width` comes from `terminal_width()` — which
   deliberately returns `ws_col - 1` (`render.rs:191-192`). The wrap width used
   to *move* differs from the width used to *place* by ≥ 1.

**Issue 3 — throbber offset (phase 02).** A literal leading `Span::raw("  ")`
in the spinner line (`render_ratatui.rs:280-287`). The spinner row itself
starts at x = 0 (`render_spinner_region`, `:578-590`); removing the two-space
span makes it flush-left. Watch the interrupt variant
(`stream.rs:234`, glyph `"⚡"` — no paren split) when asserting.

**Issue 4 — `you` → `user@host` (phase 02).** The literal is at
`chat.rs:521` — `renderer.commit_panel("you", &echo_body, false)`. A
`local_user_host()` already exists in the *unused* legacy path
(`render.rs:34-48`) but leans on `$HOSTNAME`, a bash-only variable that is
often unexported under tmux/ssh. The robust hostname source is the daemon's
`daemon_hostname()` (`src/daemon/utils/host.rs:1-7`:
`/proc/sys/kernel/hostname` → `$HOSTNAME` → `"unknown"`). Fix shape: a CLI-side
`user@shorthost` helper using `$USER` + the `/proc` read, domain-stripped
(`scrappy.local` → `scrappy`), falling back to `you` only when both sources
fail. This is the host `daemoneye chat` runs on — the CLI process — which is
the user-facing requirement ("matt@pinky" when chatting on pinky).

**Issue 5 — runtime in the border (phase 03).** `ToolFinished` currently emits
a standalone `commit_panel("result", ["✓ (1.2s)"])` (`stream.rs:585-590`)
followed by the separate `output` panel (`:591-606`). `commit_panel`'s bottom
border is an unlabeled rule (`render_ratatui.rs:341-403`). The legacy renderer
already implements the exact right-justified-label-in-bottom-border pattern to
copy (`render.rs:120-126`). Fix shape: give `commit_panel` an optional
bottom-label parameter, and in `stream.rs` buffer the `ToolFinished`
status/elapsed so the following output panel's bottom border carries
`✓ 1.2s`. Mind the ordering on the wire: check whether `ToolFinished` arrives
before or after the output response for both foreground and background
commands before speccing.

### Legacy-renderer delta audit (2026-08-09)

The ratatui migration left `render.rs`'s panel printers dead —
`print_tool_panel`, `print_user_query`, `print_tool_started`,
`print_tool_finished`, `local_user_host`, `wrap_line_hard` and
`terminal_height` have zero callers outside `render.rs` plus one unit test
(`cli/tests.rs:76`). Still live from that file: `terminal_width` (markdown,
ask, status, chat), `visual_len` (markdown), `StatusBarState` (everywhere).
Two features died with the dead code and get ported in M13:

1. **Panel-body word-wrap (fold into phase 03).** Legacy panels word-wrapped
   long body lines ANSI-aware (`wrap_line_hard`, `render.rs:133-162`);
   `commit_panel` instead truncates every body line with an ellipsis
   (`render_ratatui.rs:383`). A long user input line is silently cut off in
   chat history today. Phase 03 is already rebuilding `commit_panel`'s
   body/border layout, so wrapping (reusing or porting `wrap_line_hard`)
   lands there. Note the interaction: wrapped body rows change `row_count`
   for `insert_before`.
2. **Turn + context-budget label in the user panel's bottom border (fold into
   phase 03, behind the same bottom-label parameter).** Legacy
   `print_user_query` right-justified ` turn N · <tokens> / <window> (<pct>%) `
   into the bottom border (`render.rs:112-126`). The ratatui path dropped it;
   the status bar shows live context but history lost the per-turn stamp.
   The `you` echo site (`chat.rs:519-522`) has `turn`/token state available in
   scope via the surrounding loop's `StatusBarState` inputs — verify exact
   variable names at drafting.

Deliberately **not** ported: the `▸ tool(summary)` / `⎿ status · secs`
one-line silent-tool entries (`render.rs:53-77`) — superseded by panels as a
style decision.

**Cleanup:** once phases 02 and 03 land their ports, the dead printers in
`render.rs` (and the `wrap_line_hard` test, if the function moves rather than
stays) are deleted in phase 05 as a closing task, so `-D warnings`' dead-code
lint never fires mid-milestone.

**Issue 6 — resize / window-switch (phase 05).** SIGWINCH is only observed in
the *input* loop's `select!` (`chat.rs:617-632`); `stream.rs`'s
`select_stream` (`:176-183`) has no SIGWINCH or focus arm at all, so a resize
or pane switch during an in-flight turn is invisible until the turn ends.
`Key::FocusGained` → `renderer.reanchor()` (`chat.rs:691-696`,
`render_ratatui.rs:405-419` — a same-size `Terminal::resize` that forces the
inline-viewport origin to recompute). Fix shape: add SIGWINCH + focus-event
arms to the streaming select that call `reanchor()`/`draw`, and make the
input-loop SIGWINCH arm reanchor too (today it only re-queries width and
redraws). Nothing already committed to scrollback can be re-wrapped —
`insert_before` content is immutable; that is a non-goal.

### Design decisions

- **One palette module, depth-aware, decided once at startup.** Detection runs
  once in the chat client (not the daemon — colors are a client concern);
  every color site asks the palette, no site hardcodes a `Color`.
- **The authoritative input wrapper is `visual_lines`.** It already carries a
  full unit-test suite (`editor.rs:473-696`); ratatui's `Wrap` has no
  cursor-position API, so it cannot be the authority.
- **Scrollback is immutable.** Re-wrapping committed history on resize is out
  of scope for M13 (would require retaining a full styled transcript and
  re-inserting — a renderer rewrite).
- **`user@host` reflects the CLI host**, not the daemon host — they can differ
  and the user's requirement names the chat host.

### Risks

- Issue 1's tmux-quantization diagnosis is code-derived, not yet live-verified
  on pinky. Phase 01's E2E must include a live check (`tmux display -p
  '#{client_termfeatures}'` / `tput colors` on pinky) before the fix is
  declared to close the symptom — per § "Derive every spec fact from its
  source", the *observed* fix matters, not the theory.
- Phases 04/05 touch `render_ratatui.rs`, a 1,500-line file with prior
  oscillation-prone shape (large pre-existing file integration). Specs must
  front-load exact integration points and carry the
  compiler-error-driven-recovery note per WORKFLOW.
- Several exit criteria need live tmux verification (remote colors, mid-turn
  window switch). Like M12, these get unit coverage plus an explicit live
  check at milestone close — do not let them go unit-only silently.

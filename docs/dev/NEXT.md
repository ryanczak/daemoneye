# NEXT

**Active phase:** **M23 phase-05a — tail-model**, drafted 2026-10-02, status
`todo`. Dispatch with `/rexymcp:dispatch phase-05a`.

**What it does.** The testable core of `/tail`: `ShellScreen` now carries the
shell's `rows`/`cols` (a client cannot draw the screen bytes without them);
`cli::tail::TailModel` rebuilds the shell's terminal from the `ShellTail`
stream with follow/pause/resume rules; `render_tail` paints the grid into a
ratatui frame with colours, modifiers, a status line and top-left clipping.
Seven tests; lib 2133 → 2140.

**Split.** Row 05 became 05a (this core) and 05b (the viewer loop, `/tail`,
search, help).

**Measured at drafting.** vt100 0.16.2's `set_scrollback` holds the view
while new output arrives, so follow is only "when to jump back to 0". The
whole change was dry-run green: lib 2140 ×3, clippy clean, six mutations
red. One `event_log` test failed once in 19 runs; A/B 0/8 vs 0/8.

**After 05a:** `/rexymcp:architect next` drafts 05b (tail-viewer).

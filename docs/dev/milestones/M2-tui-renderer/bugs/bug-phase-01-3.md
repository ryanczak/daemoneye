# Bug 3 on phase-01: AI final answer is not committed to scrollback — the legacy streamer paints over the inline viewport and corrupts the input box

**Severity:** major
**Status:** open
**Filed:** 2026-06-24

The bug-phase-01-2 raw-mode fix is correct and verified live: under
`DAEMONEYE_RENDERER=ratatui`, typed characters now enter the input box (no
cooked-mode echo), the input box + status bar sit in a fixed bottom region, the
submitted **user** line commits cleanly to scrollback (`> hello in box`), and on
`/exit` the terminal is restored to cooked mode (shell echo works again). Build,
clippy, fmt, and all 763+27 tests pass.

But the same live tmux E2E exposes that the **AI's final answer is not committed
to scrollback**. The acceptance criterion requires *both* the user input **and**
the AI's final answer to land "as ordinary scrollback above the input region."
The AI answer instead streams through the **legacy** renderer directly over the
ratatui inline viewport, leaving the answer text stuck **inside** the input box
and the box visibly corrupted.

## What's wrong

In the ratatui chat loop, the user query is committed correctly through the
ratatui renderer:

`src/cli/commands/mod.rs:792`:
```rust
// Commit user query to scrollback.
let _ = renderer.commit(&format!("> {}\n", query));
```

…but the AI response is then produced by the **legacy** streaming function:

`src/cli/commands/mod.rs:~810` (inside `run_chat_ratatui`):
```rust
match ask_with_session(
    QueryArgs { query, display_query: "", prompt_override: None },
    Some(&session_id),
    approval,
    AskTmuxCtx { /* … */ },
    TokenCtx { /* … */ },
    StreamCtx {
        stdin,
        chat_width: Some(cw),
        old_termios: None, // ratatui renderer owns raw-mode
        sigwinch: Some(sigwinch),
        resize: Some(resize),
        cost_usd: &mut cost_usd,
        has_untracked: &mut has_untracked,
    },
).await { Ok(()) => daemon_up = true, Err(e) => { let _ = renderer.commit(...); } }
```

`ask_with_session` is the DECSTBM-era streamer: it writes tokens straight to
stdout (legacy `MarkdownRenderer` / `println!`-style emission), bypassing the
ratatui renderer entirely. With the inline viewport active, the cursor sits
inside the live region, so that output **paints over the viewport rows** instead
of becoming scrollback above them. ratatui's diff renderer does not know those
cells were written externally, so it never reconciles them — the AI answer text
is left stranded in the input box.

### Observed live behavior (tmux capture-pane)

After submitting `hello in box`, the answer rendered **inside** the box, not in
scrollback above it:
```
> hello in box
┌────────────────────────────────────────────────────────────────────────────┐
│ey there! 👋 Nice to meet you. I'm here and ready to help — what can I …      │
└────────────────────────────────────────────────────────────────────────────┘
 session:92b077c8 · Qwen/Qwen3.6-27B-FP8 · up 1m 5s
```
(The leading `H` of "Hey" is clipped by the box border `│` — the answer was
written over the viewport, not committed above it.)

Typing a fresh `X` overwrote the **stale answer text still in the box**,
proving the box content is the leftover answer, not the input buffer:
```
│Xy there! 👋 Nice to meet you. I'm here and ready to help …                   │
```

On `/exit`, `Ctrl-U` did not clear the visible row and `/exit` overwrote more of
the stale text (`│/exitere! …│`, `Goodbye.` over the bottom border) — the same
external-write-vs-diff corruption.

## What should happen

Phase-01 acceptance criterion: *"… commits submitted user input **and the AI's
final answer** into terminal scrollback (i.e. they remain visible as ordinary
scrollback above the input region) …"*

Spec item 4 sanctions a **minimal** approach explicitly: *"The AI response in
this phase may be rendered minimally (**commit the final answer text to
scrollback is sufficient**) — rich token streaming, markdown, spinner, and tool
panels are phase 02."* So the requirement is: get the final answer **text** into
scrollback via the ratatui renderer; it does **not** require streaming or
markdown (those are phase 02). Nothing other than the ratatui renderer may draw
in the live region.

## How to fix

Route the AI's final answer through the ratatui renderer's commit path instead
of the legacy stdout streamer. Minimal correct approach (per spec item 4):

1. Obtain the assistant's **final answer text** for the turn (e.g. a
   non-streaming query path, or capture the completed response) and commit it
   with `renderer.commit(&answer)` so it lands in scrollback above the viewport —
   mirroring how the `> {query}` line is already committed at `mod.rs:792`.
2. Do **not** call the legacy `ask_with_session` stdout streamer from the
   ratatui path — it writes directly into the inline-viewport region and
   ratatui's diff renderer cannot reconcile those externally-written cells.
   (Rich streaming/markdown/spinner migration is explicitly phase 02; a plain
   committed final-answer line is what phase 01 needs.)
3. Keep the user-query commit, the raw-mode entry/restore, and the
   bug-phase-01-1 banned-construct fixes intact; keep the legacy default path
   behavior-unchanged.

`TestBackend` tests cannot catch this (no real cursor/stdout interleave); verify
under tmux on the next review.

## Verification

- [ ] Under tmux: `DAEMONEYE_RENDERER=ratatui daemoneye chat`, submit a line,
      and confirm via `tmux capture-pane -p` that **both** the `> {query}` line
      **and** the AI's final answer appear as ordinary scrollback **above** the
      input box, and the input box is empty/uncorrupted after the turn. Quote
      the capture in the Update Log.
- [ ] Typing after a completed turn shows only the freshly-typed characters in
      the box (no stale answer text behind them).
- [ ] `cargo fmt --all`, `cargo build` (zero warnings), `cargo clippy
      --all-targets --all-features -- -D warnings`, `cargo test` all pass.
- [ ] bug-phase-01-1 and bug-phase-01-2 fixes still hold: no
      `unsafe`/`#[allow]`/`.expect()` in `src/cli/commands/mod.rs`; raw mode
      still entered on the ratatui path and restored on exit.

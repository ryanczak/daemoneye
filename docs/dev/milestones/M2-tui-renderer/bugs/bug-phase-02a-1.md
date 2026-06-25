# Bug 1 on phase-02a: AI tokens stream to stdout, not to renderer scrollback — bug-phase-01-3 reintroduced

**Severity:** blocker
**Status:** fixed (verified at review 2026-06-25, commit eea63f5)
**Filed:** 2026-06-25

## What's wrong

The phase's central deliverable — streaming AI response tokens to scrollback
**through the renderer** — is not implemented. The streaming loop still routes
every token to **stdout**, which is exactly the failure (`bug-phase-01-3`) that
bounced phase 01.

In `src/cli/commands/stream.rs:279-284`, the `Response::Token` arm of
`ask_with_session_ratatui` does:

```rust
Response::Token(t) => {
    if !response_started {
        response_started = true;
    }
    md.feed(&t);          // <-- writes to stdout
}
```

`MarkdownRenderer::feed` (`src/cli/render.rs:1100`) buffers per character and on
each completed line calls `WrapWriter::feed`/`flush`, which emit with
`print!`/`println!` straight to stdout (`src/cli/render.rs:588-595`). On the
ratatui path the inline viewport is owned by ratatui's `Terminal`; writing the
answer to stdout collides with that viewport — the precise symptom the phase-01
bug-3 fix (`93fa228`) eliminated for the greeting/echo path. The streamed AI
answer never reaches `insert_before`.

The two methods written to do this job are **dead code**:

- `RatatuiRenderer::commit_styled` (`src/cli/render_ratatui.rs:185`) — only ever
  called from a test (`render_ratatui.rs:487`), never from the streaming loop.
- `MarkdownRenderer::render_line_to_spans` (`src/cli/render.rs:1137`) — never
  called anywhere in production (`grep -rn render_line_to_spans src/` returns
  only its definition). It is `pub`, so `clippy` does not flag it as dead, which
  is why the build is green.

The executor's own "Notes for review" (phase doc, Update Log) admit this: *"the
current implementation feeds tokens through `md.feed()` which still prints to
stdout internally … For true streaming of AI response tokens to scrollback, the
`render_line_to_spans` method should be wired in — this is noted for review."*
A self-noted incomplete core deliverable is not a completed phase.

This is a textbook "green-but-inert" pass — the exact failure mode the M2
calibration protocol (README → "Verification strategy") exists to catch: it
compiles, `cargo test` is green, but the feature does not run.

## What should happen

Per Spec §3b and the acceptance criteria, completed markdown lines produced
while streaming must be committed to scrollback **through the renderer**
(`insert_before` via `commit_styled`), **not** written to stdout. With
`DAEMONEYE_RENDERER=ratatui`, the AI answer must appear progressively in
scrollback above a clean input box + status bar, styled with real cell
attributes and **no** literal `\x1b[` escape bytes (Spec §3b; acceptance
criteria bullets 2–3).

## How to fix

1. In `ask_with_session_ratatui` (`src/cli/commands/stream.rs`), stop calling
   `md.feed()` for `Response::Token`. Instead drive markdown/wrap rendering so it
   yields completed **styled lines** and commit each through the renderer's
   styled-commit path (`MarkdownRenderer::render_line_to_spans` →
   `renderer.commit_styled(...)`, or an equivalent that routes through
   `insert_before`). Buffer the partial trailing line and flush it on
   `Response::Ok`.
2. Ensure `render_line_to_spans` is actually called (no dead code) and that
   `WrapWriter`/`MarkdownRenderer` output on this path never reaches stdout.
3. Replace the isolated-helper test with one that drives the **streaming render**
   itself: feed a fake token sequence spanning a line boundary through the
   streaming path and assert the completed line appears as committed cells in the
   `TestBackend` buffer/scrollback (Spec §5 / Test plan), with no `\x1b` byte in
   any committed cell. The current
   `commit_styled_renders_into_buffer_without_escapes` test calls `commit_styled`
   with hand-built `Line`s and therefore passes even though the streaming path is
   broken — it does not cover the deliverable.
4. Re-run the E2E by hand under an attached tmux pane (phase "End-to-end
   verification") and quote the `tmux capture-pane -p` output in the Update Log,
   confirming the answer is in scrollback above the input box with no visible
   escapes. The executor environment cannot run tmux, but the code path must be
   correct so this verification can pass.

## Verification

- [ ] `grep -n "md.feed" src/cli/commands/stream.rs` shows no `md.feed` in the
      `Response::Token` arm of `ask_with_session_ratatui`.
- [ ] `grep -rn "render_line_to_spans" src/` shows at least one production call
      site (not only the definition).
- [ ] A `TestBackend` test drives the streaming path with fake tokens spanning a
      line boundary and asserts the completed line appears in committed cells.
- [ ] `cargo fmt --all`, `cargo build` (zero warnings), `cargo clippy
      --all-targets --all-features -- -D warnings`, `cargo test` all pass.
- [ ] Update Log quotes real `tmux capture-pane -p` output showing the streamed
      answer in scrollback above a clean input box with no visible `\x1b[`.

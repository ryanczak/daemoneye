# Design — Alt-Screen Transcript View

**Status:** design of record for M17. Written 2026-08-18.

## Problem

Tool output in `daemoneye chat` is elided at 10 lines. `src/cli/commands/stream.rs:674`
receives the **full** output in `Response::ToolResult(String)`, renders the first
9 lines, appends `… {N} more lines`, and drops the string. There is no way to see
the rest.

The elided panel cannot be grown in place. `RatatuiRenderer::commit_panel_labeled`
(`src/cli/render_ratatui.rs:755`) pushes panels through `terminal.insert_before()`
into the **terminal's own scrollback**, above a 6-row inline viewport
(`VIEWPORT_ROWS`, `render_ratatui.rs:129`). Committed rows are frozen text the
application can neither repaint nor reliably locate: the renderer's own notes at
`render_ratatui.rs:177-189` record that absolute row arithmetic stops being
trustworthy once `rows_scrolled > 0` or the pane is resized.

## Non-goal: owning the primary chat surface

An app-owned transcript replacing `insert_before` on the main screen was
considered and rejected for this milestone. Once history leaves terminal
scrollback, tmux copy-mode sees only the visible screen, and the client must
reimplement drag selection, highlight rendering, clipboard hand-off, and scroll —
the one part of the UX that cannot be validated by tests. The inline streaming
path stays exactly as it is.

## Shape

A **modal, alternate-screen transcript viewer**, opened with `ctrl+o` from the
chat prompt and closed with `esc`. While it is up nothing else on the pane needs
to be selectable, so the client may own the screen, the scroll, and the mouse
without competing with tmux.

Three parts:

1. **Transcript model (client-side).** A `Vec<Block>` accumulated from the
   `Response` stream across the whole client session. This is the only new
   persistent client state, and it is the piece both this design and any future
   full-ownership design need.
2. **Viewer.** Alternate screen, ratatui `Viewport::Fullscreen`, own key loop:
   scroll, expand/collapse, search, copy. Exits back to the inline renderer,
   which re-pins via `reanchor()` (`render_ratatui.rs:286`).
3. **Rehydration.** On client start the viewer's model can be seeded from
   `~/.daemoneye/var/log/sessions/<id>.jsonl` (`config::sessions_dir()`,
   `src/config/load.rs:97`), so a resumed session shows its prior turns.

## Where the bytes come from

Three persisted copies exist today; none is a drop-in source.

| Source | Fidelity | Verdict |
|---|---|---|
| `events.jsonl` via `log_command` (`src/daemon/utils/event_log.rs:291`) | `out` capped at **200 chars**, newlines flattened | Unusable |
| Session JSONL `tool_results` | `truncate_tool_results` at `limits.tool_result_chars`, default **16 000** (`src/config/types.rs:453`) | Usable for rehydration; lossy above the cap |
| `var/log/panes/<win>.log` (`capture_and_archive`, `src/daemon/background/helpers.rs:78`) | full, **but background jobs only** — foreground `send-keys` execution archives nothing — and written **raw/unmasked** while the returned copy is masked | Not a viewer source |

**Decision: the live transcript is captured client-side from the wire.** The
client already holds the exact bytes it rendered, already masked, with no
retention, join-key, or unmasking hazard. Persisted logs are a *rehydration*
source only, and only the session JSONL.

**Consequence for the wire:** `Response::ToolResult(String)` (`src/ipc.rs:414`)
carries no identifier, so a rehydrated record cannot be joined to a live block.
It gains a `tool_call_id`, which also lets a block address its own history entry.

## Block model

```
enum Block {
    UserTurn   { text, label },
    Assistant  { text },
    ToolPanel  { title, tool_call_id, summary, label, collapsed: bool },
    Output     { tool_call_id, full: String, shown: usize },
    System     { text },
}
```

`Output.full` is the untruncated wire string; `shown` is what the inline panel
displayed. The store is bounded (last N blocks / M bytes, oldest evicted) so a
long session cannot grow without limit.

## Clipboard

The client runs inside tmux by construction (it reads `$TMUX_PANE`), so copy
does **not** need OSC 52 or `set-clipboard` negotiation:

```
tmux load-buffer -w -      # stdin = block text; -w also forwards to the
                           # system clipboard via OSC 52 where available
```

`-w` requires tmux ≥ 3.2; the deployment target is 3.7b. Block-level copy
("copy this output", "copy this command") is the shipped affordance; free-form
drag selection is out of scope.

## Screen handling

- Enter: `EnterAlternateScreen`, construct a second ratatui `Terminal` with
  `Viewport::Fullscreen`. tmux does not record the alternate screen into pane
  history, so every scroll path inside the viewer is the client's own — wheel and
  keys both.
- Exit: `LeaveAlternateScreen`, then `reanchor()` on the inline renderer. The
  inline scrollback is untouched underneath; the viewer leaves no residue.
- Resize while open: full relayout from the block model — the viewer reflows,
  which committed inline panels cannot. This is a capability difference, not a
  bug-compatibility requirement.

## What this unlocks beyond expansion

Reflow on resize, search over history, jump-to-turn, per-block copy, collapsed-
by-default noisy panels, and transcript rehydration on a fresh client — the last
being impossible under the inline-only design.

## Risks

- **Restore fidelity.** Entering and leaving the alternate screen must leave the
  inline viewport and its `rows_scrolled` bookkeeping correct. This is the M13
  width-flip / M15 border-corruption blast radius; it is verified live, in a real
  tmux pane, not only against `TestBackend`.
- **Memory.** Unbounded output retention in a long session. Mitigated by the
  store cap.
- **Masking.** The viewer must render only what the wire delivered (masked). It
  must never read `var/log/panes/*.log`, which is raw.

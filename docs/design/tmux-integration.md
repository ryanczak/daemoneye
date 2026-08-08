# M12 design — Full-View tmux Integration

**Scoped:** 2026-08-07. This doc holds the settled design; phase docs cite it
rather than restating it.

## Problem

The agent does not have a full view of the user's tmux world. Three user-visible
symptoms, each traced to a verified code-level cause:

1. **The agent cannot read another pane's contents.** Non-active panes surface
   only as a one-line heuristic summary. `SessionCache::summarize()`
   (`src/tmux/cache.rs:348`) recognizes shell prompts (`$`/`#` first char),
   `top`/`htop`, and HTTP log lines; everything else collapses to
   `Active: <last line truncated to 50 chars>`. No tool captures an arbitrary
   pane's buffer on demand — `get_terminal_context` returns a full capture only
   for the active pane, `watch_pane` captures 200 lines only at completion.
2. **Cross-session blindness.** `SessionCache::refresh()` fetches every pane on
   the server via `tmux list-panes -a` (`src/tmux/pane.rs:46`) and then
   *discards* panes outside the adopted session
   (`if info.session_name != session { continue; }`, `src/tmux/cache.rs:222`).
   Other sessions collapse to one `[OTHER SESSIONS]` line from
   `other_sessions_context()`.
3. **`/pane` is a target-picker, not an inspector.** `Response::PaneList`
   (`src/ipc.rs:505`) carries only `(id, cmd, window, idx, is_target)` —
   no cwd, title, activity, status, or content preview
   (`handle_list_panes`, `src/daemon/server/handlers.rs:158`).

Two structural defects compound these:

- **Three inconsistent pane views.** `pane_map_summary` (`src/tmux/cache.rs:383`)
  excludes daemon-owned windows; the `list_panes` tool
  (`src/daemon/executor/knowledge/pane.rs:75`) includes them and tags only the
  three ghost prefixes (`de-bg-*` / `de-sj-*` are untagged); the `/pane` IPC
  handler applies a third filter. All three hard-code the five prefix string
  literals instead of using the constants in `src/daemon/mod.rs:50-66`.
- **No native tmux actions.** The agent can highlight a pane during approval
  (`highlight_pane`, `src/tmux/pane.rs:474`) but cannot navigate the user
  anywhere, zoom, split, or close a user window.

## What already works (build on it, don't rebuild)

- One `list-panes -a` call per 2 s cycle pulls 16 fields per pane
  (`RichPaneInfo`, `src/tmux/pane.rs:5-39`), including `last_activity`,
  `dead`/`dead_status`, `history_size`, `pane_pid`, `start_cmd`.
- `bounded_output` / `off_runtime` (`src/tmux/mod.rs`) protect the daemon from
  a wedged tmux server. All new tmux calls go through them.
- `mask_sensitive` (`src/ai/filter.rs`) is applied at every read surface today;
  every new read surface must apply it too.
- The add-a-tool checklist (`CLAUDE.md`) plus `tests/doc_truth.rs` enforce
  tool-table/docs consistency.

## Design decisions (settled)

### D1 — Multi-session cache, metadata-everywhere / content-at-home

`refresh()` stops discarding foreign-session panes. `PaneState` gains a
`session_name: String` field (the data is already on `RichPaneInfo`; today it is
thrown away). **Capture budget is unchanged:** per-cycle `capture-pane` calls
remain limited to panes in the adopted (home) session; foreign panes get
metadata + no buffer (`buffer` stays empty, summary derived from metadata only).
Content for foreign panes is on-demand via `read_pane` (D3). Existing surfaces
that iterate `cache.panes` must filter to the home session where today's
behavior is expected (context blocks, pane map) — foreign panes appear only
where explicitly labeled (D4's `list_panes` upgrade, `find_in_panes`, `/panes`).

### D2 — `PaneStatus` classification replaces the summary heuristics

A derived enum, computed from fields the cache already holds — no new tmux
calls:

```
Dead(code)        pane_dead, with exit code
Bell              window has uncleared bell flag
AwaitingInput     non-shell fg command AND no output for ≥ awaiting_threshold
Running           non-shell fg command, recent output
Active            shell fg command, output within ~30 s
Idle(age)         shell fg command, no recent output
```

"Shell fg command" reuses `is_shell_prompt` (`src/daemon/executor/foreground.rs`).
Rendered as `status:<name>` in `list_panes`, `/panes`, and pane context lines;
`summarize()`'s output becomes `<status> — <last meaningful line>`. Pure
function + table-driven tests.

### D3 — `read_pane` tool (core)

The single highest-leverage addition. Schema:

- `pane_id` (required) — `%N`, any session.
- `lines` (optional, default 200, max bounded by `history_size`) — depth into
  scrollback via `capture_pane_with_escapes` + `annotate_ansi`.
- `grep` (optional) — regex filter applied after capture, same semantics as
  `read_file`'s grep param.

Output is masked. The chat pane is refused (consistent with every other
surface). Daemon-owned windows are *allowed* (reading a background job's window
is legitimate — `watch_pane` already exposes this content). Not approval-gated:
it is read-only, same trust class as `get_terminal_context` (which already
exposes the active pane's full content). Silent-tool feedback:
`should_emit_tool_feedback() == true`.

### D4 — `find_in_panes` tool (core) + `list_panes` upgrade

`find_in_panes(pattern, scope?)`: regex over all cached buffers (home session)
plus a live bounded capture pass for foreign-session panes when
`scope: "all"`. Returns pane id, session (when foreign), window, status, and
the matching lines with ±1 line of context, masked, capped (50 matches).
Answers "which pane has the error?" in one call.

`list_panes` upgrade: group rows by window, add `status:` (D2), include
foreign-session panes under a labeled section, tag **all** daemon-owned windows
via the shared filter (D6). `get_terminal_context` gains optional
`scope: "window" | "session" | "all"` (default `"session"`, today's behavior).

### D5 — `tmux_control` tool (approval-gated), one tool, enumerated actions

One tool, `action` enum, mirroring `edit_file`'s one-tool-many-operations
shape:

- `focus` — select window/pane (navigate the user), via `select_pane` /
  `select-window`.
- `zoom` / `unzoom` — `resize-pane -Z`.
- `split` — split a window, returns the new pane id.
- `rename_window`
- `kill_window` — refused for daemon-owned windows (those belong to
  `close_background_window`) and for the window containing the chat pane.

**All actions approval-gated** (added to `APPROVAL_GATED` in
`src/daemon/stream.rs:23` and `LimitsConfig::APPROVAL_GATED`): even `focus`
displaces the user's attention, which is exactly what the approval gate exists
to protect. Ghost shells: `GhostPolicy` has no auto-approve category that
covers navigation, so ghosts get `tmux_control` only via an agent `ToolPolicy`
allow — default deny stands.

### D6 — One targetable-panes filter

A single function (on `SessionCache` or in `src/daemon/mod.rs` next to the
prefix constants) answering "is this pane daemon-owned?" and "is this pane
targetable (not chat, not daemon-owned)?", used by `pane_map_summary`, the
`list_panes` tool, `handle_list_panes`, and the new tools. Kills the five-way
duplication of hard-coded prefix literals.

### D7 — `/pane` / `/panes` overhaul

`Response::PaneList` widens to carry a per-pane struct (id, idx, window,
session, cmd, cwd, title, status, activity age, is_target, one-line preview) —
a new serde struct, not a wider tuple. The CLI renders `/panes` as a grouped
inspector (windows as sections, active pane starred, target pinned marker);
`/pane <n|%id>` keeps its pinning role unchanged. Wire-protocol note: both ends
ship in one binary; no compat shim needed, but the IPC enum change and client
render land in the same phase.

## Non-goals

- No tmux config management (no `set-option`, no `.tmux.conf` edits beyond
  what execution already does).
- No layout persistence/restore (tmuxinator territory).
- No per-cycle content capture of foreign sessions (cost); on-demand only.
- No changes to foreground execution / completion detection.
- No new IPC hooks; the 2 s poll + existing hooks remain the freshness model.

## Tool-count bookkeeping

33 tools today (24 core + 9 deferred). M12 adds `read_pane`, `find_in_panes`,
`tmux_control` — all core → **36 tools: 27 core + 9 deferred**. Each tool phase
bumps the `CLAUDE.md` counts line and table (enforced by `tests/doc_truth.rs`),
and documents the tool in `sre.toml` per the add-a-tool checklist.

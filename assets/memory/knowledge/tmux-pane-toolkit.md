---
tags: [daemoneye, tmux, panes, windows, sessions, read_pane, find_in_panes, tmux_control, list_panes, status, cache]
summary: How DaemonEye sees tmux — the multi-session pane cache, what is live vs cached, pane status classification, and which tool to reach for
relates_to: [agent-runtime-layout, ghost-shell-guide]
---

# tmux Pane Toolkit

What the daemon knows about tmux, how fresh it is, and which tool answers which
question. Read this when a task involves finding, reading, or acting on panes
beyond the one the user is typing in.

## The cache model — metadata everywhere, content at home

A background poll refreshes every 2 s and tracks **every pane in every tmux
session on the host**, not just the user's. What it stores differs by session:

| | user's own session ("home") | other sessions ("foreign") |
|---|---|---|
| id, index, window, cwd, command, title | cached | cached |
| activity age, dead flag, status | cached | cached |
| **buffer contents** | cached (~100 lines) | **not captured** |

Foreign panes are metadata-only by design: capturing every pane in every
session every 2 s would cost far more than it is worth. Their content is
fetched **on demand**, which is why the tools that reach it are explicitly
opt-in and slower.

Two consequences worth remembering:

- A foreign pane's content is never in the context block. If you need it, you
  must ask for it.
- A foreign pane is **not a valid foreground execution target**. Commands run
  in the user's own session; so does the `/pane` pin.

## Pane status

Every pane carries a classification, re-derived on each 2 s refresh, in
precedence order:

- `Dead(<code>)` — the foreground process exited and the pane is a
  `remain-on-exit` corpse.
- `Bell` — the pane rang the terminal bell since the user last visited it.
- `Running` — a non-shell command is in the foreground.
- `AwaitingInput` — a shell prompt that has produced output recently.
- `Idle <age>` — a shell prompt that has been quiet.

An idle shell is **not** `AwaitingInput`. Do not re-derive status from the last
line of output; the classification already accounts for the process, the bell
flag and the activity age together.

## Which tool

| Question | Tool |
|---|---|
| What panes exist, where, and in what state? | `list_panes` — grouped by window, with status, plus a labelled foreign-session section |
| What is on the user's screen right now? | `get_terminal_context` — `scope` is `"window"`, `"session"` (default), or `"all"` |
| What is in *that* pane's scrollback? | `read_pane(pane_id, lines?, grep?)` — any session, any depth, ANSI-annotated and masked |
| Which pane has the error / the string? | `find_in_panes(pattern, scope?)` — one regex over every pane; `scope: "all"` also captures foreign panes live |
| Tell me when this finishes or prints X | `watch_pane(pane_id, timeout_secs, pattern?)` |
| Move, resize, split, rename or close a window | `tmux_control(action, pane_id, …)` — approval-gated |

**Reach for `find_in_panes` before reading panes one by one.** It is one call
against every cached buffer; reading four panes to find an error is four calls
and four times the context.

## Costs and limits

- `read_pane` defaults to 200 lines, caps at 2000, and never exceeds the pane's
  own scrollback depth. Use `grep` on noisy panes rather than pulling more
  lines and skimming.
- `find_in_panes` caps at 50 matches total across all panes and returns ±1 line
  of context per match. If you hit the cap, narrow the pattern — do not page.
- `find_in_panes(scope: "all")` and reading a foreign pane both shell out to
  tmux live, one call per pane, bounded to 20 foreign panes per search. The
  default `"session"` scope is a pure cache read and effectively free.

## What is refused, and why

- **The chat pane is never readable or searchable.** Its contents are this
  conversation; reading it back is a loop, not information. `read_pane` refuses
  it by id and `find_in_panes` skips it.
- **Daemon-managed windows are readable but not killable.** Background jobs,
  scheduled jobs and ghost shells run in daemon-owned windows; reading them is
  legitimate and often useful. Closing one is `close_background_window`'s job —
  `tmux_control(action="kill_window")` refuses them, and also refuses the window
  containing the chat pane.
- **Ghost shells cannot use `tmux_control` at all** unless the agent they run as
  names it in an explicit `allow` list. There is no auto-approve category that
  covers navigation, and an autonomous session moving the user's focus is not
  something a default should permit. A `deny` list that merely omits the tool
  does not grant it.

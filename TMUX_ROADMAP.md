# tmux Integration Enhancement Roadmap

This document tracks opportunities to improve DaemonEye's use of the tmux API for richer environment awareness, lower-latency command completion detection, and more contextual AI interactions.

---

## Current tmux Surface

DaemonEye currently uses the following tmux features:

| Feature | Purpose |
|---|---|
| `capture-pane -S -N` | Snapshot last N lines of scrollback |
| `display-message #{pane_id}` | Active pane ID |
| `list-panes -s -F #{pane_id}` | Enumerate panes in session |
| `#{pane_current_command}` | Foreground process name |
| `#{pane_pid}` | Shell PID for `/proc` child tracking |
| `#{pane_width}` / `#{pane_height}` / `#{window_width}` | Layout geometry |
| `send-keys` | Inject commands and credentials |
| `new-window -d` / `kill-window` | Background job window lifecycle |
| `select-pane` | Focus switching |

The polling loop runs every 2 seconds; foreground completion is detected via `/proc` child-process tracking.

---

## Priority Items

### P1 — Add `pane_current_path` to every pane snapshot

**What**: Include `#{pane_current_path}` in the `list-panes -F` format string so every `PaneState` carries the shell's current working directory.

**Why**: The AI currently has no idea what directory the user is in. If pane A is in `/etc/nginx` and pane B is in `/var/log`, that context is completely invisible. CWD is one of the highest-signal pieces of state for understanding what the user is working on.

**How**: Add `pane_current_path` field to `PaneState`. Extend the `list-panes -F` format string from `"#{pane_id}"` to `"#{pane_id}|#{pane_current_path}"`. Parse the pipe-delimited output. Include CWD in `get_labeled_context()` output for background panes.

**Complexity**: Trivial — one format string change, one field.

---

### P2 — Add `pane_title` to pane snapshots

**What**: Include `#{pane_title}` in `list-panes -F`. This is the terminal title string set by running programs via OSC escape sequences.

**Why**: Many programs set the terminal title to something semantically rich:

- `vim` sets it to the filename being edited
- `ssh` sets it to `user@host`
- `k9s` / `kubectl` set it to the cluster/namespace
- Shell prompts often set it to `user@host: /path`

This is a free signal that directly describes what the pane is doing without any heuristic parsing of the buffer.

**How**: Add `pane_title: String` to `PaneState`. Include in `get_labeled_context()`. Add to the `[BACKGROUND PANE]` summary line.

**Complexity**: Trivial — same pattern as `pane_current_path`.

---

### P3 — Collapse pane enumeration to a single `list-panes -a` call

**What**: Replace the current `list-panes` → N×`display-message` pattern with a single `tmux list-panes -a -F "#{session_name}|#{window_index}|#{window_name}|#{pane_id}|#{pane_current_command}|#{pane_current_path}|#{pane_title}|#{pane_dead}|#{pane_dead_status}"` call that atomically snapshots all fields for all panes.

**Why**: The current `refresh()` in `cache.rs` calls `list-panes` once and then `pane_current_command()` per pane — that's 1 + N tmux subprocesses per poll cycle. A single `list-panes -a -F` call with all fields collapses this to 1 subprocess regardless of pane count. At 2-second polling with 8 panes this is a 8× reduction in tmux subprocess churn.

**How**: Rewrite `SessionCache::refresh()` to parse a rich pipe-delimited format. Drop the separate `pane_current_command()` call from the loop. This is also the natural point to add P1 and P2 fields.

**Complexity**: Low — refactor of one method, no new concepts.

---

### P4 — Window-level inventory via `list-windows -F` (COMPLETED)

**What**: Add a `WindowState` struct and a `windows` map to `SessionCache`. Populate it from `tmux list-windows -t session -F "#{window_index}|#{window_id}|#{window_name}|#{window_active}|#{window_panes}|#{window_zoomed_flag}|#{window_last_flag}"`.

**Why**: A user with multiple named windows (e.g. `nginx`, `postgres`, `app-logs`, `k8s`, `deploy`) gives the AI significant topology information. Currently the AI sees individual panes with no knowledge of which window they belong to or how the session is organized. Window names are user-set and high-signal. Knowing that the active pane is in the `k8s` window changes what context is relevant.

**How**: Add `WindowState { id, name, active, pane_count, zoomed, last_active }`. Add `windows: RwLock<HashMap<String, WindowState>>` to `SessionCache`. Extend `get_labeled_context()` to prefix pane context with a session topology summary: "Session has 5 windows: [nginx (2 panes), postgres (1 pane), ...]".

**Complexity**: Low — parallel to existing pane map, new format string parse.

---

### P5 — `tmux show-environment` for session-level env vars

**What**: Call `tmux show-environment -t session` once per session (or per refresh) and include high-signal variables in the AI context.

**Why**: Users frequently set environment variables in their tmux session that are invisible to DaemonEye but directly relevant to what the AI should do:

- `AWS_PROFILE` / `AWS_DEFAULT_REGION` — which cloud account is active
- `KUBECONFIG` / `KUBE_CONTEXT` — which k8s cluster
- `VAULT_ADDR` / `VAULT_TOKEN` — Vault endpoint
- `DOCKER_HOST` / `DOCKER_CONTEXT` — remote Docker daemon
- `ENVIRONMENT` / `APP_ENV` — staging vs production signal

The AI currently infers environment from shell history and visible output. This gives it the direct answer.

**How**: Run `tmux show-environment -t session`, parse `KEY=value` lines, filter for a known high-signal allowlist (avoid leaking arbitrary secrets), mask any that match sensitive patterns via the existing filter, inject into the system context block.

**Complexity**: Trivial to call; Low for allowlist filtering.

---

### P6 — Replace `/proc` polling with `set-hook pane-died` for foreground completion

**What**: When DaemonEye injects a foreground command via `send-keys`, instead of polling `/proc` for child-process completion, register a one-shot tmux hook:

```
tmux set-hook -t session pane-focus-in[99] "run-shell 'touch /tmp/de-pane-done-<id>'"
```

or more precisely, use `pane-died` on a wrapper pane, or `after-send-keys` to track state. The cleanest approach: inject a sentinel command (`; echo __DE_DONE__$$`) and use `set-hook alert-activity` on the pane to trigger a notification the moment that output appears, rather than polling `capture-pane` every 200ms.

**Why**: The `/proc` child tracking loop polls every 200ms and adds up to 200ms of latency to every foreground command completion. A hook fires within one tmux event loop tick — effectively zero latency. It also eliminates 1–10 subprocess calls per completed command.

**How**: `tmux set-hook -t pane_id pane-died "run-shell 'kill -USR1 <daemon_pid>'"` — the daemon installs a `SIGUSR1` handler that marks the completion channel. Or write to a tmpfile the daemon is watching. After completion the hook is removed with `set-hook -u`.

**Complexity**: Medium — requires signal handling or tmpfile watching; hook cleanup on failure paths.

---

### P7 — `pane_dead_status` for instant failure awareness (COMPLETED)

**What**: After a foreground command completes (via P6 hook or existing polling), query `#{pane_dead_status}` to get the exit code without parsing sentinel output.

**Why**: `pane_dead` and `pane_dead_status` are available the moment a pane's foreground process exits. Currently DaemonEye infers exit status from the `__DE_EXIT__$?` sentinel embedded in captured output — this works but adds a string-parsing step and requires the sentinel to be in the visible buffer. For scheduled job windows left on failure, `pane_dead_status` provides the exit code directly.

**How**: After job window completion, `display-message -t pane_id -p "#{pane_dead_status}"` gives the integer exit code. Use this to replace or validate the sentinel approach for background job windows.

**Complexity**: Low — one additional `display-message` call; straightforward integration.

---

### P8 — `set-hook alert-activity` for passive pane monitoring

**What**: Set `monitor-activity on` on user panes and register a `alert-activity` hook that notifies the daemon when a quiet pane produces output.

**Why**: DaemonEye currently only observes panes it is explicitly asked about. A user might ask "let me know when the build finishes" — today the daemon would need to poll. With `alert-activity`, tmux itself watches and notifies. Similarly, `alert-silence` (a pane went quiet after being active) is a natural "command finished" signal.

**How**:

```
tmux set-option -t session:window monitor-activity on
tmux set-hook -t session alert-activity "run-shell 'daemoneye-notify activity #{pane_id}'"
```

The daemon exposes a lightweight IPC endpoint (or listens on a tmpfile/pipe) for these notifications and can relay them to the active chat session as `SystemMsg` events.

**Complexity**: Medium — requires hook registration/cleanup lifecycle, new notification path.

---

## Remaining Opportunities

### R1 — `pipe-pane` for continuous output capture

**What**: Use `tmux pipe-pane -t pane_id "cat >> /tmp/de-pane-%%.log"` to wire a pane's byte stream to a rolling log file. DaemonEye reads from the log instead of polling `capture-pane`.

**Why**: `capture-pane` only sees what is currently visible in the scrollback buffer. `pipe-pane` captures *every byte* emitted by the pane — including output that scrolled past the history limit, rapid bursts between poll ticks, and exact timing of each line. For high-throughput panes (builds, log tailing, test runs), the AI's context becomes complete rather than a snapshot.

**Tradeoffs**: Generates potentially large log files; requires log rotation/truncation; pipe process must be managed across daemon restarts; adds file I/O to every keystroke. Best applied selectively (e.g. only on the source pane during an active AI session, not all panes always).

**Complexity**: High — log lifecycle, rotation, cleanup, selective activation.

---

### R2 — `capture-pane -e` with ANSI semantic parsing

**What**: Use `capture-pane -e` to preserve ANSI escape sequences in captured output, then parse them to extract semantic signal: red text → likely error, bold/bright → prompt or warning, OSC title sequences → program-set context.

**Why**: ANSI color is not decoration — it is semantic. `journalctl` colors errors red. `make` colors failures red. `git diff` uses red/green for removed/added. Stripping these sequences before passing to the AI discards a free error-vs-normal signal layer. An AI that can see "the last 10 red-colored lines" has higher-quality error context than one seeing all lines uniformly.

**Tradeoffs**: Requires an ANSI parser (add a crate or implement a state machine). The AI model itself does not see colors — sequences need to be translated to semantic annotations (e.g. wrap red text in `[ERROR: ...]`). Risk of over-annotating noisy colorized output.

**Complexity**: High — ANSI parser, annotation strategy, token budget impact.

---

### R3 — `scroll_position` / `history_size` awareness

**What**: Query `#{scroll_position}` and `#{history_size}` per pane. If the user has scrolled up significantly, capture from the scroll position rather than the tail, or report to the AI that the user is reviewing history.

**Why**: When a user scrolls up in a pane, they are actively reading past output. DaemonEye currently always captures from the tail, so if a user says "look at this error" while scrolled to it, the AI sees the wrong part of the buffer. Awareness of scroll position would let DaemonEye capture the *visible* content rather than the tail.

**How**: `capture-pane -S -<history_size-scroll_position> -E -<scroll_position>` to capture exactly what the user sees. Include a note in context: "User is scrolled N lines up from the end."

**Complexity**: Medium — scroll arithmetic, needs to handle edge cases (scroll_position = 0 = no scroll).

---

### R4 — `pane_in_mode` and copy-mode detection

**What**: Query `#{pane_in_mode}` and `#{pane_mode}` to detect when a pane is in copy/search mode.

**Why**: When a user enters copy mode (e.g. `prefix+[`), they are selecting text — likely to paste into the AI prompt or reference in a question. Knowing the pane is in copy mode could trigger an automatic context capture at that moment. It also signals "user is reviewing something" which is useful context.

**Complexity**: Low — one field addition; triggering behavior is optional.

---

### R5 — `tmux socket-path` for multi-daemon isolation

**What**: Expose a `--tmux-socket` flag and pass it through to all tmux invocations as `tmux -S /path/to/socket`. Allow multiple DaemonEye instances to target different tmux servers on the same machine.

**Why**: Power users and CI environments sometimes run multiple independent tmux servers. Currently DaemonEye uses the default socket and can only serve one. Supporting `TMUX_SOCKET` / `-S` would allow per-project or per-user isolation.

**Complexity**: Low-Medium — thread the socket path through all tmux call sites.

---

### R6 — `#{pane_synchronized}` awareness

**What**: Detect when panes are in synchronized input mode (`#{pane_synchronized}`) and note this in context / suppress duplicate capture.

**Why**: In synchronized mode, `send-keys` to any pane broadcasts to all synchronized panes. DaemonEye injecting a foreground command into a synchronized pane would run it in every pane simultaneously — potentially destructive. The daemon should detect this and either warn the user or refuse foreground injection.

**Complexity**: Low — one field check; guard in `send_keys` path.

---

### R7 — Window zoomed flag awareness (`#{window_zoomed_flag}`)

**What**: Detect when a pane is zoomed (`prefix+z`) and factor this into layout decisions.

**Why**: When a pane is zoomed, the chat pane split would un-zoom it. DaemonEye could warn the user or use a different attachment strategy (e.g. open in a new window instead of splitting the current one) when zoom is active.

**Complexity**: Low — one field check; changes to `setup` output and pane-open logic.

---

## Implementation Order Recommendation

```
Phase 1 (quick wins, no architecture change):
  P1 pane_current_path
  P2 pane_title
  P3 single list-panes -a call
  P5 show-environment
  P7 pane_dead_status

Phase 2 (new architecture, high value):
  P4 window inventory
  P6 hook-based foreground completion

Phase 3 (reactive monitoring):
  P8 alert-activity hooks
  R3 scroll_position awareness
  R4 copy-mode detection
  R6 synchronized pane guard
  R7 zoomed flag awareness

Phase 4 (high complexity, high reward):
  R1 pipe-pane continuous capture
  R2 ANSI semantic parsing

Later / nice-to-have:
  R5 multi-socket support
```

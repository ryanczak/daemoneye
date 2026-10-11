# M27 — tmux becomes an optional observer; DaemonEye 2.0 close

**Status:** scoped 2026-10-09, awaiting first phase. PE decisions below.

**Depends on:** M20 (shell engine), M24 (run manager), M26 (sbx isolation:
chat and ghost commands already run in PTY shells, not tmux).

**Goal.** DaemonEye no longer *needs* tmux. Every command the daemon runs
goes through PTY shells; tmux is reduced to an optional, **read-only**
observer: when the user happens to be inside tmux, the AI can still read pane
summaries for context, but nothing is ever executed, injected or created
there. Close DaemonEye 2.0 and tag `v2.0.0`.

**Exit criteria:**

- **No execution through tmux.** No code path calls `send-keys`, creates or
  kills a window, sets a hook, or starts `pipe-pane`. The alert overlay
  (`display-message` on chat panes) may remain **only** as an optional extra
  when tmux is present; it is never the sole delivery path. The `de-bg-*` /
  `de-sj-*` / `de-gs-*` window machinery, `[execution] backend = "tmux"`,
  `hook.rs`, `cli/notify.rs` and the foreground send-keys path are deleted.
- **Runs without tmux.** On a host with no tmux binary and no `$TMUX`,
  `daemoneye daemon`, `chat`, `ask`, ghosts (webhook and scheduled) and
  scheduled scripts all work; nothing logs a tmux error. **Alerts included**
  (PE decision 4): a webhook or scheduled alert reaches every connected
  `daemoneye chat` client on screen with no tmux involved, and still runs
  `[notifications] on_alert`.
- **Read-only awareness when present.** Inside tmux, the prompt's context
  still carries pane summaries for the user's session; outside tmux it
  carries the daemon's own shells only. The remaining tmux code is
  capture/list only.
- **Scheduled scripts run in PTY shells.** `ActionOn::Script` fires in a
  scheduler-owned PTY shell with a real exit code. `ActionOn::Command` still
  loads but refuses to fire, with a logged reason.
- **Catch-up on reconnect.** The catch-up brief fires when a `daemoneye chat`
  client reconnects to a session after ≥ 30 s away, tracked from IPC
  connect/disconnect, not tmux hooks.
- **Docs current; `v2.0.0` tagged.** `architecture.md`,
  `PRODUCT_DEFINITION.md` (no "embeds into tmux"), `REQUIREMENTS.md`,
  `security.md`, README, CLAUDE.md; `daemoneye doctor` reports the host's
  readiness; `tests/doc_truth.rs` passes; live close on pinky.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 6 (roadmap row M27, originally "tmux
  removal"; rescoped here to "optional observer").
- `docs/architecture.md` § "DaemonEye 2.0 — M20 Shell Engine".

## Phases

Drafted one at a time with `/rexymcp:architect next`. Rough order; each row
is split further at drafting if it is more than one session.

| # | Phase | Status |
|---|---|---|
| 01 | helpers-out ([phase-01-helpers-out.md](phase-01-helpers-out.md)) — move the generic helpers that only *live* in `src/tmux/` (`bounded_output`, the `status` classifiers, `format_age`, `annotate_output`) to neutral modules; no behaviour change | done (approved_after_1, 2026-10-09) |
| 02 | sched-pty ([phase-02-sched-pty.md](phase-02-sched-pty.md)) — `ActionOn::Script` on a scheduler-owned PTY shell; `ActionOn::Command` loads and refuses to fire | done (approved_after_1, 2026-10-09) |
| 03 | catchup-reconnect ([phase-03-catchup-reconnect.md](phase-03-catchup-reconnect.md)) — chat holds a presence connection; the brief fires on re-attach (`chat --resume <id>`) after ≥ 30 s away | done (approved_after_1, 2026-10-10) |
| 04 | chat-no-tmux ([phase-04-chat-no-tmux.md](phase-04-chat-no-tmux.md)) — chat runs in place outside tmux (no auto-attach, every client tmux call gated on `$TMUX`); a pane-less Ask gets the daemon's shells as context | done (approved_after_1, 2026-10-10) |
| 03b | alerts-ipc ([phase-03b-alerts-ipc.md](phase-03b-alerts-ipc.md)) — every alert (webhook, analysis, ghost events, scheduled results) is pushed to every attached chat over the presence connection; the tmux overlay stays as an optional extra | done        |
| 05 | exec-pty-only ([phase-05-exec-pty-only.md](phase-05-exec-pty-only.md)) — delete the tmux execution fallback (foreground send-keys, `de-bg-*`/`de-gs-*` windows, `src/daemon/background/`, ghost incident session, tmux sudo injection, `[execution] backend = "tmux"`); anything without a PTY route is refused | done (escalated: architect takeover, 2026-10-10) |
| 05b | hooks-notify ([phase-05b-hooks-notify.md](phase-05b-hooks-notify.md)) — the daemon writes nothing into tmux: delete `hook.rs`, the `Notify*` IPC, every `set-hook` install, pipe-pane, managed `new-session`, and the shell `DE_EXIT` hook; `daemoneye notify` stays as an inert hidden shim for hooks an older daemon left | todo |
| 06 | docs-doctor — the docs above, `daemoneye doctor` | todo |
| 07 | live-close — architect-run live verification on pinky, with and without tmux; tag `v2.0.0` | todo |

## Notes

### PE decisions (2026-10-09)

1. **Terminal context:** keep **read-only** tmux awareness. When `$TMUX` is
   set, the AI may read pane summaries; it never executes in tmux. This
   replaces the roadmap's full removal of `src/tmux/`.
2. **Scheduled jobs:** port `ActionOn::Script` to PTY shells; **drop**
   `ActionOn::Command`: keep it loadable for old `schedules.json` files,
   refuse to fire it, and log why.
3. **Catch-up brief:** trigger on **chat client reconnect** (≥ 30 s away),
   not tmux attach/detach.
4. **tmux is NOT a requirement for alerts.** Today the webhook/scheduled
   alert overlay is `tmux display-message` on every open chat pane. M27
   gives alerts a tmux-free path: the daemon pushes the alert over IPC to
   every connected chat client, which renders it on screen (row 03b). The
   tmux overlay may stay as an optional extra when tmux is present. The
   `[notifications] on_alert` hook is unchanged.
5. **Scheduled scripts support the sandbox.** With `[sandbox] enabled`, a
   scheduled script runs in a per-run sandbox exactly as a ghost's command
   does, and fails closed otherwise. Sandboxes may become the default later;
   nothing in M27 may assume a scheduled job runs on the host.
6. **Reconnect = chat presence + resume** (2026-10-09, at phase-03
   drafting). Chat opens one IPC connection per `Ask` and mints a fresh
   session id per process, so there was no reconnect to observe. Each
   `daemoneye chat` now holds a long-lived `ChatAttach` connection. The
   daemon stamps detach state when a session's last connection closes.
   `chat --resume <id>` re-attaches, and the brief comes back in the attach
   reply. Phase 03b pushes alerts down the same connection.

### Spec style for this milestone

Lean specs (see the M26 retrospective): state behaviour, constraints and
test names; name the file or function where a change goes once the
architect has traced it; leave the mechanism to the executor.

### Out of scope

- New context sources beyond the daemon's shells and read-only tmux panes.
- M28+ (telemetry & dashboard, MCP surface, knowledge & fleet).

### Risks

- **Session identity.** Chat sessions, the session store and several
  defaults key on the tmux session name and the chat pane. Phase 04 must find
  every such key; this is the most likely place for a hidden dependency.
- **Sudo credential injection** currently types into a tmux pane. The PTY
  path has its own credential round trip; the tmux one goes with phase 05.

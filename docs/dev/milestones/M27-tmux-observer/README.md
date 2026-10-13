# M27 — tmux becomes an optional observer; DaemonEye 2.0 close

**Status:** done (closed 2026-10-12; every exit criterion verified live on pinky, `v2.0.0` tagged and pushed, § Retrospective). **Awaiting PE sign-off on the close and the fold proposals.** Scoped 2026-10-09; PE decisions below.

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
| 03b | alerts-ipc ([phase-03b-alerts-ipc.md](phase-03b-alerts-ipc.md)) — every alert (webhook, analysis, ghost events, scheduled results) is pushed to every attached chat over the presence connection; the tmux overlay stays as an optional extra | done (approved_after_1, 2026-10-10) |
| 05 | exec-pty-only ([phase-05-exec-pty-only.md](phase-05-exec-pty-only.md)) — delete the tmux execution fallback (foreground send-keys, `de-bg-*`/`de-gs-*` windows, `src/daemon/background/`, ghost incident session, tmux sudo injection, `[execution] backend = "tmux"`); anything without a PTY route is refused | done (escalated: architect takeover, 2026-10-10) |
| 05b | hooks-notify ([phase-05b-hooks-notify.md](phase-05b-hooks-notify.md)) — the daemon writes nothing into tmux: delete `hook.rs`, the `Notify*` IPC, every `set-hook` install, pipe-pane, managed `new-session`, and the shell `DE_EXIT` hook; and `daemoneye notify` itself | done (approved_after_1, 2026-10-11) |
| 06 | doctor ([phase-06-doctor.md](phase-06-doctor.md)) — `daemoneye doctor`: one line per readiness check (config, api key, shell, daemon, sandbox, hosts, tmux), exit 1 only on a `FAIL`; tmux is optional and never fails | done (approved_first_try, 2026-10-11) |
| 06b | runtime-docs ([phase-06b-runtime-docs.md](phase-06b-runtime-docs.md)) — the seeded prompts and memories, README and CLAUDE.md describe PTY execution and optional read-only tmux; doctor documented; a `doc_truth` gate keeps the retired terms out | done (approved_after_1, 2026-10-11) |
| 06c | design-docs ([phase-06c-design-docs.md](phase-06c-design-docs.md)) — `architecture.md`, `PRODUCT_DEFINITION.md` (no "embeds into tmux"), `REQUIREMENTS.md`, `security.md`; also the two stale passages 06b carried: CLAUDE.md's `run_terminal_command` row ("user pane"/"daemon host window") and README's `[daemon] tmux_session` row (creates a session, registers hooks) | done (approved_first_try, 2026-10-11) |
| 06e | no-fg-target ([phase-06e-no-fg-target.md](phase-06e-no-fg-target.md)) — the prompt stops naming a foreground pane: no `[FOREGROUND TARGET]`, no `[Pane target changed]`, no `background=true/false` in model-facing text; **dispatch before 06d** | done (approved_first_try, 2026-10-11) |
| 06d | docs-leftovers ([phase-06d-docs-leftovers.md](phase-06d-docs-leftovers.md)) — the stale passages 06c carried (FR-1.2.2/6/17, `chat --session`, chat attach, pane targeting) and the foreground-pane terms added to the `doc_truth` gate | done (approved_after_1, 2026-10-11) |
| 07b | live-blockers ([phase-07b-live-blockers.md](phase-07b-live-blockers.md)) — rehearsal findings that break exit criteria: scheduled jobs fire without tmux (F3), catch-up sees events injected while away (F5), the client writes nothing to tmux (F6) | done (approved_first_try, 2026-10-11) |
| 07c | live-majors ([phase-07c-live-majors.md](phase-07c-live-majors.md)) — chat reconnects after a daemon restart (F4), `background` maps to `wait` (F2), expired approvals close (F7), shell exit reported honestly (F1), the tmux-write gate sees abbreviations | done (approved_after_2, 2026-10-12) |
| 07a | docs-truth ([phase-07a-docs-truth.md](phase-07a-docs-truth.md)) — the stale passages rehearsal 1 found: remote script streaming (ghost-only), the JSON audit-log sample, the removed `tmux` backend, an unresolved `ssh_target` is refused, and stale comments and seeded memory | done (approved_first_try, 2026-10-12) |
| 07d | live-r2 ([phase-07d-live-r2.md](phase-07d-live-r2.md)) — rehearsal-2 majors: alerts reach a reconnected (non-resident) chat's history (F8); a mistyped argument to a known tool returns an error to the model instead of an "unknown tool" drop (F10); the doubled reconnect gear | done (approved_first_try, 2026-10-12) |
| 07e | catchup-nonresident ([phase-07e-catchup-nonresident.md](phase-07e-catchup-nonresident.md)) — PE-run major F11: the catch-up brief reaches a chat whose session is not resident after a daemon restart | done (approved_after_1, 2026-10-12) |
| 07 | live-close ([phase-07-live-close.md](phase-07-live-close.md)) — architect-run live verification on pinky, with and without tmux; tag `v2.0.0` | done (2026-10-12, tag v2.0.0) |

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

### Retrospective (2026-10-12)

**Outcome.** DaemonEye no longer needs tmux, and 2.0 shipped: `v2.0.0` was
tagged on `8cdf3a86` and pushed on the PE's go-ahead, after a credential
scan of the 193 unpushed commits came back clean. Every exit criterion was
verified live on pinky with the PE present.
- **No tmux execution.** `tests/tmux_read_only.rs` gates the source against
  every tmux write verb, including abbreviations and multi-line or chained
  argument lists. Live, the observed run made 1121 tmux calls and 0 writes,
  the window count stayed at 1, and no hook was set.
- **Runs without tmux.** With no tmux binary on `PATH`, `doctor` exits 0,
  and chat, `wait: false`, scheduled scripts (real exit 3) and webhook
  ghosts all work, with no tmux error in `daemon.log`.
- **Alerts with no tmux involved.** Alerts reach every connected chat over
  the presence connection.
- **Scheduled jobs.** The legacy `Command` action is refused with an event
  and a WARN line.
- **Catch-up.** The brief fires on `chat --resume`, including after a daemon
  restart with the session not resident.
- **Remote host.** Commands on zoidberg ran with no tmux server created
  there.
- **Docs.** The README was rewritten for 2.0, the CHANGELOG has its 2.0.0
  entry, and `doc_truth` is green.

**Numbers.** 17 executor phases, plus the architect-run live close.
- 6 `approved_first_try`: 06, 06c, 06e, 07b, 07a, 07d.
- 9 `approved_after_1`: 01, 02, 03, 04, 03b, 05b, 06b, 06d, 07e.
- 1 `approved_after_2`: 07c.
- 1 `escalated`: 05, an architect takeover after a refined re-dispatch and
  a resume.

Eight bug docs. The suite went from 2300 to 2325.

**What the live close found that nothing else did.** The live close took
three rehearsals and a PE run, and found eleven defects:
- **Rehearsal 1 (F1–F7):** a bare `exit` killing the PTY shell; GLM's
  `background` argument silently ignored; the scheduler loop stalled with no
  tmux session (blocker); chat not reconnecting after a restart; no catch-up
  brief (blocker); the chat client setting a global tmux option; stale
  approval panels.
- **Rehearsal 2 (F8–F10):** alerts missing from the history of a
  reconnected, non-resident chat; a mistyped argument dropped as "unknown
  tool"; a doubled gear on the reconnect notices.
- **PE run (F11):** no brief after a restart with no ask. Its first fix
  passed review and failed the live re-run (bug-07e-1).

All eleven passed unit tests at the time. Lesson, the same as M26's: an
integration path is verified only when its entry point is driven live.
**The residency gap recurred three times** (F5, F8, F11, and bug-07e-1):
the in-memory `SessionStore`, the presence registry and the disk history
are three views of "the session", and each fix closed one pairing. See
deferred item 1.

**Process calibration.**
- *NoProgressStall fold, at its third occurrence* (02, 04, 03b): "name every
  remaining edit; forbid re-deriving". The per-site work-list table used
  from 07b onward is that fold applied: 07a, 07b and 07d landed first try,
  and 07e needed one bounce, which was a spec error. **Proposal A below.**
- *Architect errors, recorded.* 07e's pinned test described an event order
  production never runs: injecting while presence was still registered. It
  passed review and failed live. In the PE run's S2 notes, a history count
  came from the model's own search text rather than an injected alert, which
  hid that F11 had a second half. The schedule-approval matcher missed the
  `Approve?` prompt and denied a job. One occurrence each, so this is data.
- *Executor.* The 07c executor overwrote existing Update Log lines with its
  "started" entry, and the reviewer restored them. One occurrence.
- *Live-harness rules* (now in phase-07 Pre-flight 4–5, and recorded for the
  next live close):
  - drive tmux with `-S <socket>` and `TMUX` unset;
  - put no tmux on run A's `PATH` at all;
  - send `/approvals off` first, and wait until the greeting is idle;
  - match the approval panel on `pprove for`, the schedule prompt on
    `AI wants to schedule`, and events at the start of a line;
  - S7 kills every process with `HOME=$R`.

**Deferred to a later milestone.**
1. Unify session residency. Have presence attach, injection and catch-up
   share one notion of a session that may not be resident, rather than three
   registries.
2. F9: the `[ACTIVE PANE]` label lands on the non-chat pane when the chat
   pane is excluded.
3. The invalid-args error labels every mistyped param `optional`.
4. `InvalidArgs` inherits an approval-gated tool's budget-cap exemption via
   `tool_name()`.
5. A non-resident detach record for a chat that never returns is never
   consumed (bounded by the number of sessions). `NON_RESIDENT_DETACHES`
   could be `pub(crate)`.
6. daemon.log says "foreground command approved", and the approval records
   carry `mode` values from 1.x.

**Fold proposals (PE decision; nothing landed).**
- **A — `WORKFLOW.md` § phase specs:** when a phase edits more than one
  site, the spec carries a per-site work list (`file · symbol | change`),
  and the executor's first patch is due within 10 turns. Three occurrences
  (02, 04, 03b), plus five clean runs after it was applied.
- **B — hold (one occurrence):** a pinned test that models a production
  event sequence must name the caller whose order it reproduces.
- M23's two fold proposals remain on the queue.


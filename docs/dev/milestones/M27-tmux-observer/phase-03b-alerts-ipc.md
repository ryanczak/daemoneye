# Phase 03b: alerts-ipc — alerts are pushed to every attached chat over the presence connection

**Milestone:** M27 — tmux becomes an optional observer; 2.0 close
**Status:** in-progress
**Depends on:** phase-03, phase-04
**Tags:** language=rust, kind=feature, size=m

## Goal

PE decision 4: tmux is not a requirement for alerts. Today a live chat
learns about an alert only through the tmux overlay. `notify_chat_panes`
(`src/webhook/process.rs`) runs `tmux display-message` on every session's
`chat_pane`. It serves webhook alerts, the webhook AI analysis and ghost
lifecycle events (`inject_ghost_event`). A scheduled job's notice reaches no
chat at all: `notify_job_result` (`src/daemon/scheduled.rs`) sends to a
`notify_tx` that the scheduler loop in `src/daemon/mod.rs` passes as `None`.

Phase 03 gave every `daemoneye chat` a held `ChatAttach` connection
(`src/daemon/server/chat_presence.rs`, client
`src/cli/commands/chat_presence.rs`). This phase pushes every alert down
that connection, and the chat renders it on screen. The tmux overlay stays,
as an optional extra.

## Spec

- **Daemon push.** Add one broadcast entry point that sends a one-line
  alert to **every** live presence connection, across all sessions. Each
  live `handle_chat_attach` writes the pushes it receives to its client as
  `Response::SystemMsg`, after the attach reply's `Ok`, while still reading
  the client until EOF. The presence registry will need a per-connection
  sender alongside its count. The `presence_count` / `presence_live`
  semantics and all phase-03 tests must stay as they are.
- **Never block the caller.** A broadcast is called from the webhook handler
  and the scheduler. It must not await a slow client. A full or closed
  per-connection queue drops that push for that client, with a `debug` log,
  and the other clients still get it.
- **A failed push write ends that connection.** It goes through the same
  exit path as a read error: unregister exactly once, and stamp the detach
  if it was the last connection.
- **Callers.** `notify_chat_panes` broadcasts the same line it overlays, so
  webhook alerts, the analysis line and ghost events all reach attached
  chats. The tmux `display-message` stays as the optional extra.
  `notify_job_result` also broadcasts its message, so scheduled-job results
  and `ActionOn::Alert` reach attached chats. `fire_notification` /
  `[notifications] on_alert` is unchanged everywhere.
- **Test isolation (escalation, 2026-10-10).** The presence registry is
  process-global and cargo runs tests in parallel. Tests must never clear
  it, and must never assert on a global broadcast count. Give the broadcast
  an inner form over an explicit registry (the `…_in(&registry, …)` seam
  shape this codebase already uses, e.g. `shell_context_block_in`) with the
  global wrapper on top. Count assertions use a fresh registry; delivery
  tests that go through `handle_chat_attach` use a unique session id and
  observe delivery by reading the client. No test-only helper may be `pub`
  in production code.
- **Client.** The parked presence task currently ends on the first line the
  daemon sends. It must instead read in a loop until EOF. Each pushed
  `SystemMsg` goes to the chat loop through a channel, and the chat loop
  renders it with the existing system-message style, including while the
  user is at the prompt. **Gotcha:** never write to stdout from the
  background task. The ratatui renderer owns the screen, and a stray write
  corrupts it. Rendering happens in the loop that owns the renderer.

## Tests

- `pushed_alert_reaches_every_attached_client`: two connections attached
  to two different sessions, over `tokio::io::duplex`, and one broadcast.
  Each client reads `Ok`, then a `SystemMsg` carrying the alert text.
- `push_to_a_dead_client_unregisters_it`: two attached clients. Drop one
  client's read half, then broadcast. The dead connection's handler ends
  with its session's `presence_count` at 0. The live client still gets the
  alert.
- `broadcast_with_no_clients_is_a_noop`: returns promptly and delivers
  to nobody.
- `notify_chat_panes_pushes_over_presence`: with one attached client and
  no tmux pane, `notify_chat_panes` delivers the line as a `SystemMsg`.
- `scheduled_job_result_is_pushed_to_attached_clients`: `notify_job_result`
  delivers its message to an attached client.
- `parked_presence_forwards_every_push`: the client-side parked task
  forwards **two** successive pushed `SystemMsg`s to its channel, not just
  the first, and ends on EOF.

## Acceptance criteria

- [ ] Each of the six tests above reports `1 passed` alone. Today each
      reports `0 passed`.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0;
      `cargo fmt --all --check` exits 0.
- [ ] `cargo test` reports exactly `passed=2329 failed=0`: today's
      `passed=2323` (measured at the phase-04 review; no code has changed
      since) plus the six new tests. If you add supporting tests, raise the
      count by one each and name them in the Update Log.
- [ ] The Update Log entry contains the § End-to-end verification output,
      every line as printed, and a line `PASTE MATCH: yes`.

## End-to-end verification

```sh
for t in pushed_alert_reaches_every_attached_client push_to_a_dead_client_unregisters_it broadcast_with_no_clients_is_a_noop notify_chat_panes_pushes_over_presence scheduled_job_result_is_pushed_to_attached_clients parked_presence_forwards_every_push; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /"; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
git diff --stat
```

Each line runs **once**. Expect **only** `test result:` summary lines from
the `cargo test` lines; rexyMCP's output filter removes the individual
`test … ok` lines. Do not grep the full test output.

## Authorizations

- [x] May edit `src/daemon/server/chat_presence.rs`,
      `src/daemon/session.rs` (the presence registry),
      `src/webhook/process.rs`, `src/daemon/scheduled.rs`,
      `src/cli/commands/chat_presence.rs` and `src/cli/commands/chat.rs`.

## Out of scope

- Removing the tmux overlay or any other tmux code (phase 05).
- A new `Response` variant, alert styling or severity colours. Pushes are
  plain `SystemMsg`.
- `daemoneye ask`, which holds no presence connection.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-09 21:45 (started)

**Executor:** executor (phase 03b run 1)
**Status:** flipped `todo → in-progress`; implementing the Spec in order.
**Plan:** registry gains a per-connection push sender beside its count; the
attach hold multiplexes its read loop against the pushes; `notify_chat_panes`
and `notify_job_result` gain a broadcast; the client's parked presence task
becomes a read loop that forwards `SystemMsg` pushes to the chat loop.

### Update — 2026-10-10 (escalation)

**Chosen lever:** resume.
**Rationale:** NoProgressStall at turn 467, with the daemon push half on
disk and sound. The wall was a test-isolation hazard the spec failed to
name, plus one test bug. The spec now names the isolation rule. The rest
of the phase is untouched, mechanical work.

Architect gate run on the uncommitted tree: two tests pass,
`pushed_alert_reaches_every_attached_client` and
`broadcast_with_no_clients_is_a_noop`. `push_to_a_dead_client_unregisters_it`
FAILS: after the dead client unregisters, the test reads `rx2` for an alert
it never broadcasts, so the read times out at 5 s. The remaining three tests
do not exist. Clippy fails on an unused `task1`. `src/lib.rs` gained a
`pub fn presence_test_drain()` that exists only for tests.


### Update — 2026-10-10 07:45 (resumed run)

**Executor:** executor (phase 03b run 2)
**Status:** continuing from run 1's on-disk daemon half, per the resume block.
**Plan:** fix the dead-client test (broadcast the string it reads for, join
`task1`); apply the test-isolation rule (`presence_broadcast_in` seam over an
explicit registry, no `clear_registry`, no `pub` test drain); add the two
caller tests (`notify_chat_panes_pushes_over_presence`,
`scheduled_job_result_is_pushed_to_attached_clients`); convert the client's
parked task to a read loop that forwards every push through a channel
(`parked_presence_forwards_every_push`), and render pushes in the chat loop
that owns the renderer.

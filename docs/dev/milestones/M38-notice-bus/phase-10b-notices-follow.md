# Phase 10b — notices-follow

**Milestone:** M38 — The notice bus
**Status:** todo
**Drafted:** 2026-11-18
**Depends on:** phase-10a
**Estimated diff:** ~350 lines, half tests
**Tags:** language=rust, kind=feature, size=m

## Goal

An IPC client can subscribe to the daemon's bus with a `Filter` and receive
each matching notice as it is published. `daemoneye notices --follow`
prints the backlog (phase 10a), then streams live notices until Ctrl-C.
This is the last planned M38 phase.

## Architecture references

- `crate::bus::Bus::subscribe(name, filter, capacity)` returns a bounded
  `mpsc::Receiver<Arc<Notice>>`. A full queue drops for that subscriber
  only, and a closed receiver is removed on the next publish.
- `crate::bus::global()` is the daemon's bus.
- `Bus::with_journal(f)` is a bus whose journal is a closure.
- `src/daemon/server/shells.rs` `handle_shell_tail` is the analogue for a
  long-lived streaming request: it watches the client's read half for EOF
  while it forwards frames.
- `src/cli/commands/notices.rs` (phase 10a): `filter_from_flags`,
  `select_notices`, `notice_line`, `render_notices`.
- `tests/isolation.rs`, the analogues:
  - `daemon_start_replays_active_alerts` talks to an isolated daemon's
    socket with the crate's own `send_request` / `recv`.
  - `daemon_webhook_returns_200` posts an alert to it.

## Pre-flight

1. Read `docs/dev/STANDARDS.md`, the milestone README, and this doc.
2. Clean tree on `master`.

## Edit order and the files you need

**Your first `patch` lands within your first 10 turns.** Edit **only**
these files, in this order. Each step ends at a green `cargo build --tests`.

1. `src/bus/filter.rs` and `src/ipc.rs`: the wire types.
2. `src/daemon/server/notices.rs` (new) and `src/daemon/server/mod.rs`:
   the handler.
3. `src/cli/commands/notices.rs` and `src/main.rs`: `--follow`.
4. `tests/isolation.rs`: the end-to-end test.
5. `tests/hygiene_allowlist.txt`, the notices row in `README.md`, and the
   `src/daemon/server/` row in `CLAUDE.md`.

## Spec

1. **Wire types.**
   - `Filter` also derives `Serialize`.
   - `Request::Subscribe { filter: Filter }`.
   - `Response::Notice { notice: Notice }`.

2. **The handler** (`server/notices.rs`): `handle_subscribe` takes the
   client's read and write halves, a `&Bus` and the filter. The dispatch
   arm passes `crate::bus::global()`. The handler:
   1. subscribes, as a queued subscriber with capacity 256;
   2. **then** sends `Response::Ok`, so a client that has read the `Ok`
      knows every later publish reaches it;
   3. forwards each received notice as `Response::Notice`, and stops when
      the client's read half reaches EOF or errors, or when a write fails.

   Stopping is never an error, and it returns `Ok(())`. Dropping the
   receiver is the unsubscribe, because the bus prunes it on the next
   publish.

3. **`--follow`** on `daemoneye notices`:
   1. Connect and send `Subscribe` with the flags' filter, then wait for
      the `Ok`.
   2. Print the backlog: the last `--limit` matches, as phase 10a does.
   3. Then print each live `Response::Notice` with `notice_line`, flushing
      stdout per line, until the connection closes or Ctrl-C.

   - **No duplicates.** A notice published between the `Ok` and the
     backlog read is both in the journal and in the live stream. Skip a
     live notice whose `id` was printed in the backlog. Put that check in a
     pure function, `live_line(printed: &HashSet<String>, n: &Notice) ->
     Option<String>`.
   - **No daemon.** If the daemon cannot be reached, still print the
     backlog, then exit 1 with `daemon not running — cannot follow`.
   - Without `--follow`, the command is exactly phase 10a's.

4. **Allowlist.**
   - The new dispatch arm grows `handle_client`; the new flag grows
     `async_main`. Bump both counts in `tests/hygiene_allowlist.txt` to the
     length the hygiene gate **measures**. Run `cargo test --test hygiene`:
     its failure names the measured length.
   - No other allowlist line changes.

5. **Docs.**
   - The README `daemoneye notices` row gains `--follow`: it streams live
     notices and needs the daemon.
   - The CLAUDE.md `src/daemon/server/` row names `notices.rs`
     (`handle_subscribe`).

## Gotchas

- **The event log.** In unit tests, build a bus with
  `Bus::with_journal(|_| {})`, leaked with `Box::leak`. **Never** use
  `Bus::new` or `global()`: they journal into whatever `HOME` is, and test
  fixtures have already landed in the user's real event log this
  milestone.
- **Ordering.** A test must read the handler's `Ok` **before** it
  publishes. A queued subscriber only receives notices published after it
  subscribed.
- **Timeouts.** Bound every wait with `tokio::time::timeout` (a few
  seconds), so a broken handler fails the test instead of hanging the
  suite.

## Acceptance criteria

- [ ] `grep -c 'Subscribe' src/ipc.rs` prints at least `1`.
- [ ] `grep -c 'follow' src/cli/commands/notices.rs` prints at least `1`.
- [ ] `git diff 17e07e77 -- tests/hygiene_allowlist.txt | grep -c '^+'`
      prints at most `3` (the header line and the two bumped entries).
- [ ] Every test below exists and passes. All four gates pass.

## Test plan

Each test calls the function named; none re-implements its logic.

- `filter.rs`: `filter_round_trips_through_json` covers a filter with all
  four fields set.
- `ipc.rs`: `subscribe_request_roundtrip` and `notice_response_roundtrip`,
  through the existing `roundtrip_req` / `roundtrip_resp` helpers.
- `server/notices.rs`, over `tokio::io::duplex` with a `with_journal` bus:
  - `subscribe_acks_then_streams_matching_notices`:
    - the first frame is `Ok`;
    - then publish a non-matching notice and a matching one;
    - the only `Response::Notice` received is the matching one.
  - `subscribe_ends_when_the_client_disconnects`: drop the client side;
    `handle_subscribe` returns `Ok(())` within the timeout.
- `cli/commands/notices.rs`: `live_line_skips_a_backlog_notice`. A notice
  whose id is in `printed` gives `None`; another gives its `notice_line`.
- `tests/isolation.rs`: `notices_follow_streams_a_webhook_alert`.
  - Start an isolated daemon, connect, send `Subscribe` filtered to
    `sources: [webhook]`, and read the `Ok`.
  - Post a firing alert to its webhook.
  - Within 10 s, a `Response::Notice` arrives whose `name` is the alert's.

## End-to-end verification

`notices_follow_streams_a_webhook_alert` is the end-to-end proof. Paste
its `cargo test --test isolation` result line into the Update Log.

## Authorizations

- May touch exactly the files listed in "Edit order".
- May bump the `handle_client` and `async_main` allowlist counts to their
  measured lengths.

## Out of scope

- External publish over IPC (PE decision at scoping).
- Reporting a subscriber's dropped count to the client.
- `--since`, `--status` and JSON output.

## Update Log

<!-- entries appended below this line -->

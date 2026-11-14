# Phase 02b — session-counters

**Milestone:** M37 — Alerts out of history, onto the status bar and `top`
**Status:** todo
**Drafted:** 2026-11-14
**Depends on:** phase 02a (the `StatusModel`, `PresencePush`)
**Estimated diff:** ~350 lines, about half of it tests
**Tags:** language=rust, kind=feature, size=m

## Goal

The bar's background, ghost and shell counters and its throughput slot
show real values (decision 6). The daemon pushes a `SessionCounters` frame
to a session's presence connections whenever that session's counts change,
and once at attach. The turn-end `UsageUpdate` carries the turn's output
tokens. The client never polls.

## Behaviour

1. **The frame.** `Response::SessionCounters { background: u32, ghosts:
   u32, shells: u32 }` in `src/ipc.rs`, with its `kind()` arm.
2. **What is counted.** This is one pure function over a snapshot of the
   shell registry, the session id and the active-ghost count:
   - `shells`: shells owned by `Owner::Chat(<session id>)` that have not
     exited.
   - `background`: of those, the `dedicated` shells with a command in
     flight. These are the session's running `wait: false` commands.
   - `ghosts`: `stats::get_ghosts_active()`. This is daemon-wide, because
     ghost runs belong to no chat session.
   Another session's shells, ghost-owned shells and scheduler shells count
   toward nobody's `shells` or `background`.
3. **When it is pushed.** A daemon task wakes once a second. For each
   session with a live presence connection, it computes the counters and
   pushes the frame to **that session's** connections only, and only when
   the value differs from the last one pushed to that session. When a
   session's last presence connection closes, its last-pushed value is
   forgotten, so a re-attach starts fresh. This is daemon-internal; the
   client still never polls.
4. **At attach.** The `ChatAttach` hold sends one `SessionCounters` for the
   session after the `Ok`, and after the `AlertState` when one is sent. The
   attach parser accepts exactly brief-then-`Ok`, so nothing may come
   before the `Ok`. That value also becomes the session's last-pushed value.
5. **Output tokens.** `Response::UsageUpdate` gains `#[serde(default)]
   output_tokens: u32`: the sum of `output_tokens` over the turn's model
   calls, not only the last call. Decision 6 names `SessionInfo`, but
   `SessionInfo` is sent before the model runs, so the turn-end
   `UsageUpdate` carries the count instead. A frame without the field
   still decodes as 0.
6. **The client.** The parked presence task forwards `SessionCounters` as
   a new `PresencePush::Counters { background, ghosts, shells }`. The
   loop's arm applies it through one `StatusModel` method, then redraws.
   The stream's `UsageUpdate` arm stores `output_tokens` in
   `last_output_tokens`.

## Constraints

- Pushing never blocks on a slow client: queue it on the connection's
  unbounded channel, the way `presence_broadcast_frame_in` does.
- Tests drive explicit maps and snapshots, never the process-global
  presence map or registry (cargo runs tests in parallel). The `_in`
  functions in `src/daemon/session.rs` are the pattern.
- The task's spawn must not grow `run_daemon`. If it has to go there, bump
  `run_daemon`'s line in `tests/hygiene_allowlist.txt` and say why in the
  Update Log.
- Tests call the production seam. A copy of production logic in a test is
  a fake test, and bounced 02a twice.

## Notes for executor

**Symbol map.** Every one of these exists today:
- `src/ipc.rs`: `Response`, `Response::kind`, `UsageUpdate`, `AlertState`.
- `src/daemon/session.rs`: `PresenceConnection`, `presence_map`,
  `presence_register` / `presence_unregister`, `presence_session_ids`,
  `presence_broadcast_frame_in`.
- `src/daemon/server/chat_presence.rs`: the `ChatAttach` hold, which sends
  the `AlertState` after the `Ok`.
- `src/shell/registry.rs`: `Owner`, `ShellRecord` (`dedicated`,
  `exited_unix`, `in_flight`), `with_shells`, `crate::shell::registry()`.
- `src/daemon/stats.rs`: `get_ghosts_active`.
- `src/daemon/stream/conversation.rs`: `AiEvent::Done(usage)` and the
  `UsageUpdate` send.
- `src/cli/commands/chat_presence.rs`: `PresencePush` and the parked
  task's `match serde_json::from_str::<Response>`.
- `src/cli/commands/chat_loop.rs`: the `PresencePush` arm.
- `src/cli/commands/stream_ask.rs`: the `UsageUpdate` arm.
- `src/cli/status_bar.rs`: `StatusModel` (`background`, `ghosts`,
  `shells`, `last_output_tokens`).

**Edit order.** Each step ends at a green `cargo build`:
1. **First write:** add the `SessionCounters` variant and its `kind()`
   arm, plus `output_tokens` on `UsageUpdate` (fix the constructors the
   compiler names).
2. Create `src/daemon/counters.rs`: the pure count function, the
   last-pushed tracker, and a per-session push `_in` function next to
   `presence_broadcast_frame_in` in `session.rs`. Add their tests.
3. The 1 s task, the attach send and the forget-on-last-close.
4. Sum the turn's output tokens into `UsageUpdate`.
5. The client: `PresencePush::Counters`, the `StatusModel` method, both
   arms.
6. CLAUDE.md: a Key-files row for `src/daemon/counters.rs`.

**Your first edit lands within your first 15 turns.** Never run `tmux`.
Never repeat a read whose output you already have; when unsure of a call
site, let `cargo build` name it.

## Test plan

- `session_counters_count_this_sessions_live_shells_only` (another
  session's shell, a ghost's shell and an exited shell are not counted)
- `session_counters_count_dedicated_shells_with_a_command_in_flight_as_background`
  (a dedicated idle shell and a non-dedicated busy shell are not background)
- `counters_tracker_pushes_a_value_once_and_again_on_change` (first value:
  push; same value: no push; a new value: push; after forget: push)
- `presence_push_to_session_reaches_only_that_sessions_connections`
- `attach_sends_session_counters_after_ok` (beside the existing
  `presence_attach_sends_ok_before_alert_state` in `tests/integration.rs`)
- `usage_update_carries_output_tokens_and_decodes_without_them`
- `parked_presence_forwards_session_counters_frames`
- `status_model_applies_session_counters` (via the new method; the bar then
  shows the nonzero counters and hides the zero ones)

## Acceptance criteria

- [ ] Every test in the test plan passes.
- [ ] `grep -c 'SessionCounters' src/ipc.rs` is at least 2 (today 0).
- [ ] `grep -c 'output_tokens' src/ipc.rs` is at least 1 (today 0).
- [ ] `grep -c 'PresencePush::Counters' src/cli/commands/chat_loop.rs` is at
      least 1 (today 0).
- [ ] Mutation check, pasted with its `test result:` line: make the count
      function ignore `dedicated`, confirm
      `session_counters_count_dedicated_shells_with_a_command_in_flight_as_background`
      fails, then revert.
- [ ] fmt, build (0 warnings), clippy and `cargo test` are green. Hygiene,
      `doc_truth` and `bug_tracker` are ok.

## End-to-end verification

Run mechanically, with output redirected to a file and pasted:

```sh
cargo test --lib session_counters 2>&1 | grep -E '^test result'
cargo test --lib counters_tracker 2>&1 | grep -E '^test result'
cargo test --test integration attach_sends_session_counters_after_ok 2>&1 | grep -E '^test result'
grep -c 'SessionCounters' src/ipc.rs | sed 's/^/ipc_frames=/'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
```

## Authorizations

- Create `src/daemon/counters.rs` and register it in `src/daemon/mod.rs`.
- Edit the files in the symbol map, `src/daemon/startup.rs` (the spawn),
  `tests/integration.rs`, `tests/hygiene_allowlist.txt` (only as the
  constraint above allows) and `CLAUDE.md`.

## Out of scope

- `top` (phase 03).
- Any change to the bar's line, the tiers or the glyphs.
- Per-session ghost attribution.

## Update Log

<!-- entries appended below this line -->

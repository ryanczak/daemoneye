# Phase 03: catchup-reconnect — chat presence over IPC drives the catch-up brief

**Milestone:** M27 — tmux becomes an optional observer; 2.0 close
**Status:** in-progress
**Depends on:** phase-02
**Tags:** language=rust, kind=feature, size=m

## Goal

The catch-up brief (`build_catchup_brief`, `src/daemon/server/catchup.rs`)
is triggered today by tmux `client-attached` / `client-detached` hooks, which
write `SessionEntry.last_detach` / `detach_time_utc` / `messages_at_detach`
(`src/daemon/hook.rs`). The brief is then emitted on the next `Ask`
(`src/daemon/server/ask.rs`). Replace the trigger with **chat presence over
IPC**. Each `daemoneye chat` holds one long-lived connection for its
lifetime. When the last connection for a session closes, the daemon records
the detach. A client that re-attaches to that session ≥ 30 s later gets the
brief in the attach reply. This connection is also the channel phase 03b
will push alerts down, so build it as a held connection, not a one-shot
request.

## Spec

- **New request `Request::ChatAttach { session_id }`** (`src/ipc.rs`). The
  daemon handler (dispatched from `handle_client`, `src/daemon/server/mod.rs`)
  registers the connection as present for `session_id`. It replies with
  either zero or one `Response::SystemMsg` (the catch-up brief) and then
  `Response::Ok`. After that it **holds the connection** and reads until EOF
  or error, ignoring any lines it receives. On close it unregisters.
- **Presence is counted per session id.** Several chat clients may attach to
  one session. Detach state (`last_detach`, `detach_time_utc`,
  `messages_at_detach`) is stamped only when the count drops to zero, and
  only if a `SessionEntry` exists for that id.
  - **Gotcha:** a fresh `chat` attaches *before* its first `Ask` creates the
    `SessionEntry`. Presence must therefore not live only on `SessionEntry`.
    An attach for an id with no entry still counts, and the later entry
    creation must not reset the count.
- **The brief at attach.** If the session has `last_detach` set, the attach
  computes the brief with `build_catchup_brief` over the messages added
  since `messages_at_detach`, exactly as `ask.rs` does today. It then clears
  the detach state whether or not a brief was produced. Delete the `Ask`-time
  brief block in `ask.rs`, so the brief is delivered once, at attach.
- **Eviction.** `cleanup_pass` (`src/daemon/session.rs`) must not evict a
  session that has a live presence connection.
- **tmux hooks stop driving it.** `handle_notify_client_attached` /
  `handle_notify_client_detached` no longer touch detach state. They just
  reply `Ok`. Do **not** delete the requests, CLI subcommands or hook
  installation: phase 05 deletes all tmux hooks, and `tests/isolation.rs`
  still checks that the hook is installed.
- **Chat client.** `daemoneye chat`, in both front ends (ratatui and raw,
  `src/cli/commands/chat.rs`), opens the presence connection right after
  its session id is chosen. It renders a brief from the attach reply before
  the first prompt, then hands the connection to a background task that
  holds it until the process exits. A failed attach must not stop chat from
  running.
- **Resume.** Add `daemoneye chat --resume <id>` (`src/main.rs`). It uses
  `<id>` as the session id instead of minting one with `new_session_id`.
  The id must have the shape `new_session_id` produces (16 lowercase hex
  characters). Anything else is refused with an error naming the id,
  before any connection is made. When chat exits, it prints one line
  naming the command that resumes this session.

## Tests

- `chat_attach_request_round_trip` (`src/ipc_tests.rs`)
- `last_presence_disconnect_records_detach_state`: an attach, then a close,
  stamps `last_detach` and `messages_at_detach` = the message count at
  close.
- `presence_with_two_clients_detaches_only_on_last`: closing one of two
  connections leaves `last_detach` `None`; closing the second stamps it.
- `reattach_after_30s_returns_catchup_brief`: detach state backdated by
  ≥ 30 s, with a `[Background Task Completed` message added after the
  detach. The attach reply carries a `SystemMsg` brief, and the detach
  state is cleared afterwards.
- `reattach_within_30s_returns_no_brief`: same setup at < 30 s. The reply
  is `Ok` with no `SystemMsg`, and the detach state is cleared.
- `attached_session_is_not_evicted`: `cleanup_pass` past the idle window
  keeps a session with live presence and evicts the same session without
  it.
- `chat_resume_rejects_a_malformed_id`: `ABC`, a 15-character hex id,
  and `../x` are refused. A valid 16-hex id is accepted.
- `tmux_client_hooks_no_longer_touch_detach_state`: the two hook handlers
  leave `last_detach` / `messages_at_detach` unchanged.

The daemon-side tests may drive the handler over an in-memory duplex
stream (`tokio::io::duplex`); they need no daemon and no tmux.

## Acceptance criteria

- [ ] Each of the eight tests above reports `1 passed` alone. Today each
      reports `0 passed`.
- [ ] `grep -n 'build_catchup_brief' src/daemon/server/ask.rs` prints
      nothing. Today it prints lines.
- [ ] `grep -n 'last_detach\|messages_at_detach' src/daemon/hook.rs` prints
      nothing. Today it prints lines.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0;
      `cargo fmt --all --check` exits 0.
- [ ] `cargo test` reports exactly `passed=2313 failed=0`: today's
      `passed=2305` (measured at the phase-02 review; no code has changed
      since) plus the eight new tests.
- [ ] The Update Log entry contains the § End-to-end verification output,
      every line as printed, and a line `PASTE MATCH: yes`.

## End-to-end verification

```sh
for t in chat_attach_request_round_trip last_presence_disconnect_records_detach_state presence_with_two_clients_detaches_only_on_last reattach_after_30s_returns_catchup_brief reattach_within_30s_returns_no_brief attached_session_is_not_evicted chat_resume_rejects_a_malformed_id tmux_client_hooks_no_longer_touch_detach_state; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /"; done
grep -n 'build_catchup_brief' src/daemon/server/ask.rs ; grep -n 'last_detach\|messages_at_detach' src/daemon/hook.rs ; echo "grep_done"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
git diff --stat
```

Each line runs **once**. Expect **only** `test result:` summary lines from
the `cargo test` lines; rexyMCP's output filter removes the individual
`test … ok` lines. Do not grep the full test output.

## Authorizations

- [x] May edit `src/ipc.rs`, `src/ipc_tests.rs`, `src/daemon/server/`,
      `src/daemon/hook.rs`, `src/daemon/session.rs`, `src/cli/commands/`
      and `src/main.rs`.

## Out of scope

Pushing alerts or any other message down the presence connection (phase
03b); deleting the tmux hooks, `NotifyClient*` requests or `cli/notify.rs`
(phase 05); session identity outside tmux (`tmux_session`, chat pane) and
the managed-session auto-attach in `run_chat_inner` (phase 04); docs
(phase 06).

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-09 16:05 (started)

**Executor:** GLM-5.3-Flash-NVFP4-Spark (headless run). Status flipped
`todo` → `in-progress`; milestone README row updated to match. Starting
with `Request::ChatAttach` in `src/ipc.rs`, then the presence registry and
the attach handler in `src/daemon/server/`, the brief move off `Ask`, the
`cleanup_pass` guard, the hook handlers, and the `chat --resume` client.

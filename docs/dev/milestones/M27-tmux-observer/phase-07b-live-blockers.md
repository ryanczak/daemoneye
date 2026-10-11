# Phase 07b: live-blockers — scheduled jobs fire without tmux, catch-up sees what arrived while away, the client writes nothing to tmux

**Milestone:** M27 — tmux becomes an optional observer; 2.0 close
**Status:** todo
**Depends on:** phase-06e
**Tags:** language=rust, kind=bugfix, size=m

## Goal

Phase 07's live rehearsal on pinky (2026-10-11) found three defects that
break M27 exit criteria. Unit tests did not catch any of them. Fix all
three, each with a test that fails on today's tree.

## Spec

1. **F3: scheduled jobs never fire without tmux.** The scheduler task in
   `run_daemon` (`src/daemon/mod.rs`) skips every tick while the startup
   tmux session name is empty ("No session adopted yet; skip until a client
   connects"). That name is captured once and never changes. So on a host
   with no tmux, no job ever fires. Make due jobs fire whether or not a tmux
   session exists. `run_scheduled_job` already runs `Script` jobs in a
   scheduler-owned PTY shell, and `Ghost` jobs through `GhostManager`.
   Confirm that neither needs a non-empty session name. The webhook ghost
   path already runs with an empty one. Test
   `scheduler_fires_due_jobs_without_a_tmux_session`: with an empty session
   name and a due `Script` job in the store, one scheduler tick takes the
   job. Extracting the tick body into a testable function is fine.
2. **F5: the catch-up brief misses everything that arrived while away.**
   `inject_into_sessions` (`src/webhook/process.rs`) appends alerts and ghost
   lifecycle events **to the session file only**. Its comment says
   in-memory is not updated, because the next `Ask` re-reads from disk.
   The brief is now built at **attach** time (`attach_brief_for` in
   `src/daemon/server/chat_presence.rs`), over the in-memory
   `entry.messages[messages_at_detach..]`, before any `Ask`. So the events
   are never there. Constraint: an event injected while the client was away
   must be in the brief when the client re-attaches after ≥ 30 s. You choose
   the mechanism: update memory on inject, or build the brief from the
   file. Keep two things as they are. A turn already in flight still sees
   the event on its next turn, not mid-turn. The `messages_at_detach`
   indexing must still be correct afterwards. Test
   `attach_brief_includes_an_alert_injected_while_away`: detach, inject a
   `[Webhook Alert]` through `inject_into_sessions`, backdate the detach
   past 30 s, attach. The brief contains the alert. Keep a hermetic `HOME`;
   see the existing `chat_presence` tests.
3. **F6: the chat client writes to tmux.** M27's exit criterion is that the
   remaining tmux code is capture and list only. The client still writes in
   three places:
   - `chat.rs` runs `tmux set -g focus-events on`. That is a global option
     on the user's server.
   - Chat startup resizes its own pane (`crate::tmux::resize_pane_width`,
     `resize-pane`).
   - `offer_no_sibling_options` (`src/cli/commands/pane.rs`) offers to
     `split-window` when the chat pane is alone.

   Remove all three. Delete `resize_pane_width` if nothing else uses it.
   When the chat pane is alone, chat starts with no target pane, no prompt
   and no split. Keep the cross-window pick only if it writes nothing.
   **Keep** `viewer.rs`'s `load-buffer`. It is a copy the user triggers.
   Test, in a new `tests/tmux_read_only.rs`:
   `source_issues_no_tmux_write_verbs`. It scans `src/` (skipping
   `#[cfg(test)]` modules) for the string literals `"send-keys"`,
   `"new-window"`, `"new-session"`, `"kill-window"`, `"kill-session"`,
   `"set-hook"`, `"pipe-pane"`, `"respawn-pane"`, `"split-window"`,
   `"resize-pane"` and `"focus-events"`, and names `file:line` for each
   hit. It must fail on today's tree.

### Gotchas

- F1 from the rehearsal: in a PTY shell, `exit N` kills the shell. Tests
  that need a non-zero exit use `sh -c 'exit N'`.
- `daemoneye schedule windows` (`list-windows`) and the alert overlay
  (`display-message -d … -t <chat pane>`) are reads or the allowed overlay.
  Leave them.

## Acceptance criteria

- [ ] `cargo test scheduler_fires_due_jobs_without_a_tmux_session`,
      `cargo test attach_brief_includes_an_alert_injected_while_away` and
      `cargo test --test tmux_read_only` each report `1 passed`. Today none
      of these tests exist.
- [ ] `grep -rn '"focus-events"\|"split-window"\|"resize-pane"' src` prints
      nothing outside `#[cfg(test)]` modules. Today it prints 3 lines.
- [ ] clippy (`-D warnings`) and `cargo fmt --all --check` exit 0.
- [ ] `cargo test` reports `failed=0`. `passed` is today's 2309 plus the
      tests you add; name them in the Update Log.
- [ ] Paste § End-to-end verification's output into the Update Log
      yourself, followed by `PASTE MATCH: yes`.

## End-to-end verification

```sh
cargo test scheduler_fires_due_jobs_without_a_tmux_session 2>&1 | grep -E '^test result: ok. 1 passed'
cargo test attach_brief_includes_an_alert_injected_while_away 2>&1 | grep -E '^test result: ok. 1 passed'
cargo test --test tmux_read_only 2>&1 | grep -E '^test result'
grep -rn '"focus-events"\|"split-window"\|"resize-pane"' src | wc -l
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
```

Run each line once.

## Authorizations

- [x] May edit `src/daemon/mod.rs` (the scheduler task), `src/daemon/scheduled.rs`,
      `src/webhook/process.rs`, `src/daemon/server/chat_presence.rs`,
      `src/daemon/session.rs`, `src/cli/commands/chat.rs`,
      `src/cli/commands/pane.rs`, `src/tmux/pane.rs`, their tests, and the
      new `tests/tmux_read_only.rs`.

## Out of scope

- F4 (presence reconnect after a daemon restart), F2 (unknown
  `run_terminal_command` arguments), F7, F1. Those are phase 07c.
- Docs. Phase 07a covers them.

## Update Log

<!-- entries appended below this line -->

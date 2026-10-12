# Phase 07b: live-blockers — scheduled jobs fire without tmux, catch-up sees what arrived while away, the client writes nothing to tmux

**Milestone:** M27 — tmux becomes an optional observer; 2.0 close
**Status:** in-progress
**Depends on:** phase-06e
**Tags:** language=rust, kind=bugfix, size=m

## Goal

Phase 07's live rehearsal on pinky (2026-10-11) found three defects that
break M27 exit criteria. Unit tests did not catch any of them. Fix all
three, each with a test that fails on today's tree.

## Spec — the edit table is the whole job (escalation, 2026-10-11)

**Rules for this run.** Run 1 was stopped after 257 turns with no source
edits. It spent about 240 consecutive read-only calls surveying tmux calls
across `src/`. Every edit site is named below.

- **Open only the files named in a row.** Do not grep `src/` for other tmux
  calls. The source gate in row 6 does that scan as a test, so you never
  have to.
- **Your first `patch` lands within 10 turns.** That patch is row 1. Do the
  rows in order, and run `cargo build` after each one.
- Facts below were verified by the architect. Do not re-derive them.

| # | Fix | File · symbol | Edit |
|---|---|---|---|
| 1 | F6 | `src/cli/commands/chat.rs`, the block that runs `tmux set -g focus-events on` (guarded by `if pane_id_opt.is_some()`) | Delete the whole block, with its comment. |
| 2 | F6 | `src/cli/commands/chat.rs`, the chat-width block that calls `crate::tmux::resize_pane_width` | Do not resize. Inside tmux, `chat_width = crate::tmux::query_pane_width(pane_id).unwrap_or_else(terminal_width)`, or the equivalent `unwrap_or(…)` form. Outside tmux it stays `terminal_width()`. Then delete `resize_pane_width` from `src/tmux/pane.rs`; nothing else calls it. |
| 3 | F6 | `src/cli/commands/pane.rs`, `resolve_target_pane`, the `0 =>` arm (no sibling pane) | Return `None`. Delete `offer_no_sibling_options`, which prompts to `split-window` "for foreground commands". If clippy then reports `sync_read_line` or anything else as unused, delete it too. |
| 4 | F3 | `src/daemon/mod.rs`, the scheduler task in `run_daemon` | Delete the `if sn.is_empty() { continue; }` gate. Move the due-job selection into `pub(crate) fn due_jobs_for_tick(store: &ScheduleStore, session: &str) -> Vec<ScheduledJob>`, which returns `store.take_due()` whatever `session` is, and have the loop call it. `run_scheduled_job` and its `Script`/`Ghost` branches work with an empty session name; the live run proved the webhook ghost path does. |
| 5 | F5 | `src/webhook/process.rs`, `inject_into_sessions` | After the disk append, also push a clone of `msg` onto every resident entry's `messages` (`with_sessions`, `get_mut`). Replace the comment "In-memory is intentionally NOT updated … the next Ask request will re-read from disk". It is false: `handle_ask` (`src/daemon/server/ask.rs`) takes the in-memory history whenever the session is resident, and reads the file only when it is not. Leave `record_chat_detach` and `attach_brief_for` alone. Once memory holds the message, `entry.messages[messages_at_detach..]` contains it. |
| 6 | tests | see below | Add them once rows 1–5 build. |

**Tests (row 6):**

- `due_jobs_for_tick_ignores_an_empty_session`, next to the scheduler tests:
  a store with one due `Script` job, called with `""`. Returns that job.
- `inject_into_sessions_updates_resident_history`, in
  `src/webhook/process.rs` tests: a resident entry with 2 messages, then
  inject a `[Webhook Alert]`. The entry has 3, and the last one is the
  alert.
- `attach_brief_includes_an_alert_injected_while_away`, in
  `src/daemon/server/chat_presence.rs` tests: `record_chat_detach`, then
  `inject_into_sessions`, then backdate `last_detach` by 60 s, then
  `attach_brief_for`. The result is `Some` and contains `[Webhook Alert]`.
  Copy the existing `chat_presence` tests' hermetic-`HOME` setup.
- `tests/tmux_read_only.rs`, `source_issues_no_tmux_write_verbs`. It walks
  `src/**/*.rs` and stops reading a file at its first `#[cfg(test)]` line.
  It fails, naming `file:line`, on any of the literals `"send-keys"`,
  `"new-window"`, `"new-session"`, `"kill-window"`, `"kill-session"`,
  `"set-hook"`, `"pipe-pane"`, `"respawn-pane"`, `"split-window"`,
  `"resize-pane"`, `"focus-events"`. Run it once before rows 1–3 if you
  want to see it fail. If it names a line outside rows 1–3, write that
  file:line in the Update Log and leave it alone.

**Mutation check.** Before reporting, put the `sn.is_empty()` gate back
into `due_jobs_for_tick`, run its test, confirm it fails, and revert.
Paste the failing line.

### Gotchas

- In a PTY shell, `exit N` kills the shell. Tests that need a non-zero
  exit use `sh -c 'exit N'`.
- Keep `viewer.rs`'s `load-buffer` (a copy the user triggers),
  `display-message` (reads and the optional alert overlay) and
  `list-windows` (`daemoneye schedule windows`). None of them is in the
  gate's list.

## Acceptance criteria

- [ ] `cargo test due_jobs_for_tick_ignores_an_empty_session`,
      `cargo test inject_into_sessions_updates_resident_history`,
      `cargo test attach_brief_includes_an_alert_injected_while_away` and
      `cargo test --test tmux_read_only` each report `1 passed`. Today none
      of these tests exist.
- [ ] The Update Log has the mutation-check failing line.
- [ ] `grep -rn '"focus-events"\|"split-window"\|"resize-pane"' src` prints
      nothing outside `#[cfg(test)]` modules. Today it prints 3 lines.
- [ ] clippy (`-D warnings`) and `cargo fmt --all --check` exit 0.
- [ ] `cargo test` reports `failed=0`. `passed` is today's 2309 plus the
      tests you add; name them in the Update Log.
- [ ] Paste § End-to-end verification's output into the Update Log
      yourself, followed by `PASTE MATCH: yes`.

## End-to-end verification

```sh
cargo test due_jobs_for_tick_ignores_an_empty_session 2>&1 | grep -E '^test result: ok. 1 passed'
cargo test inject_into_sessions_updates_resident_history 2>&1 | grep -E '^test result: ok. 1 passed'
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

### Notes for executor — 2026-10-11

Run 1 (GLM, 257 turns) was stopped by the PE. It had no source edits:
about 240 consecutive read-only calls surveying tmux call sites and
`schedule windows` code, while task 1 (F3) was still "active". The spec had
said "remove all three" and "scans `src/`", which invited a survey. The
refinement names every edit site in an ordered table, settles F5's
mechanism (the architect verified it), and moves the scan into a test.
Start at row 1.

### Update — 2026-10-11 (escalation)

**Chosen lever:** refined re-dispatch.
**Rationale:** the run was cancelled by the PE at turn 257 with an empty
source diff, so there is nothing to resume. The cause was a spec gap: open
scope ("find and remove"), and F5's mechanism left for the executor to
choose. That is the same stall shape as 06c run 1, where the same treatment
(a per-site edit table) worked.

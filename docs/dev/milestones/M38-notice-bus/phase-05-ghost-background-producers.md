# Phase 05 — ghost-background-producers

**Milestone:** M38 — The notice bus
**Status:** in-progress
**Drafted:** 2026-11-17
**Depends on:** phase-04
**Estimated diff:** ~400 lines, about half of it tests
**Tags:** language=rust, kind=refactor, size=m

## Goal

Ghost lifecycle events and background-command completions publish notices
instead of writing into session history and the chat overlay themselves.
With default routes, delivery is unchanged from 2.9.0: ghost events reach
every chat's history and the chat notice; a background completion reaches
its own session's history only. Also fixes the `ScheduleStore::new_empty`
temp-file race that cost phase 04 ~800 turns.

## Architecture references

- `docs/dev/milestones/M38-notice-bus/README.md` § Design and § Notes —
  read the `test_bus` / `rt.enter()` gotcha before touching any test.
- `src/daemon/notice_sinks.rs`: `default_routes` (Ghost → history +
  chat_notice; Background → history only), `deliver_history` (a notice
  with `session: Some(id)` reaches that session only), `test_bus`.
- `src/bus/notice.rs`: `Notice::new` and its builders, `history_text()`
  (`title`, or `title\nbody`).
- Phase 04's `src/daemon/scheduled.rs` builders (`alert_job_notice`,
  `failure_notice`) are the pattern to follow.

## Pre-flight

1. Read `docs/dev/STANDARDS.md`, the milestone README, and this doc.
2. Clean tree on `master`.

## Current state

`inject_ghost_event(sessions, content)` (`src/webhook/process.rs`, glob
re-exported from `src/webhook/mod.rs`) calls `inject_into_sessions`, then
`notify_chat_panes` with the content's first line via `off_runtime`, then
`log_event("ghost_lifecycle", {content})`. It has 13 callers, each passing
a `[Ghost Shell Started|Completed|Failed|Skipped] …` string:
`src/webhook/process.rs` (4, `state.bus` in scope), `src/daemon/scheduled.rs`
(5, `bus` in scope since phase 04), `src/daemon/stream/conversation.rs` (2)
and `src/daemon/executor/knowledge/ghost.rs` (1) — the last two have no
bus.

`bg_shell::await_and_notify` (`src/daemon/executor/bg_shell.rs`) builds a
`[Background Task Completed]\n<completion or interrupted message>` user
message and delivers it to its own session with `append_session_message`
and `push_event`, after a locked check that returns silently when the
session entry is gone. `await_notify_and_close` wraps it. Their production
caller is `src/daemon/executor/dispatch_background.rs`; the rest are the
inline tests in `bg_shell.rs`.

`ScheduleStore::new_empty()` (`src/scheduler.rs`) names its file by
`process::id()` alone, so parallel tests share one path and a `save`
renames another test's `.tmp` away (`No such file or directory`). See
`docs/dev/TODO.md` item 2.

## Edit order and the files you need

**Your first `patch` to a source file lands within your first 15 turns** —
step 1 is small on purpose. Everything this phase needs is in these files;
**read no others**:

- `src/scheduler.rs`, `docs/dev/TODO.md` — step 1.
- `src/webhook/process.rs` — the ghost builder and its 4 callers.
- `src/daemon/scheduled.rs`, `src/daemon/stream/conversation.rs`,
  `src/daemon/executor/knowledge/ghost.rs` — the other ghost callers.
- `src/daemon/executor/bg_shell.rs`, `src/daemon/executor/dispatch_background.rs`
  — the background producer.
- `src/daemon/notice_sinks.rs`, `src/bus/notice.rs` — read once; do not
  change them.
- `CLAUDE.md` — one bullet (Spec task 5).

Do the edits in this order. Each step ends at a green `cargo build --tests`;
start the next step only then.

1. Spec task 1 (`new_empty`) and its test.
2. In `process.rs`, add `GhostEvent`, `ghost_event_notice` and
   `publish_ghost_event` beside `inject_ghost_event` (Spec task 2).
3. Move all 13 callers to `publish_ghost_event`, then delete
   `inject_ghost_event` (Spec task 3).
4. The background producer and its callers and tests (Spec task 4).
5. CLAUDE.md, the Test-plan tests, then the gates once (Spec tasks 5–6).

## Spec

1. **`new_empty` gets a distinct file per call** — the path stays in
   `std::env::temp_dir()` and still contains the pid, plus a process-wide
   `AtomicU64` counter so two calls in one process never share a path. Its
   doc comment's "unique temp file" becomes true. Delete item 2 from
   `docs/dev/TODO.md` (the `---` separator above it too).

2. **Ghost notice builder** — in `src/webhook/process.rs`:
   - `pub enum GhostEvent { Started, Completed, Failed, Skipped }`.
   - `pub fn ghost_event_notice(kind: GhostEvent, name: &str, content: &str)
     -> crate::bus::Notice` — pure. Source `Ghost`; severity `Info` for
     `Started` / `Completed`, `Error` for `Failed`, `Warning` for `Skipped`;
     `name` the alert, runbook or job name the caller has; label `kind` =
     `started` / `completed` / `failed` / `skipped`; title the content's first
     line, body the rest (use the file's existing `split_first_line`), so
     `history_text()` equals `content` byte for byte.
   - `pub fn publish_ghost_event(bus: &'static crate::bus::Bus, kind:
     GhostEvent, name: &str, content: &str)` — **not** async: publishes the
     notice, then keeps the `log_event("ghost_lifecycle", {content})` line
     unchanged (`daemoneye top` and the away digest read it until phase 08).

3. **Callers** — each `inject_ghost_event(&sessions, &format!(…)).await`
   becomes `publish_ghost_event(bus, GhostEvent::<kind>, <name>, &format!(…))`
   with the **same** format string; the kind is the one its `[Ghost Shell …]`
   prefix names. The bus: `state.bus` in `process.rs` (copy the `&'static`
   into the spawned task), the phase-04 `bus` parameter in `scheduled.rs`,
   `crate::bus::global()` in `conversation.rs` and `knowledge/ghost.rs`.
   Then delete `inject_ghost_event`. Do not change `inject_into_sessions`
   or `notify_chat_panes` — the sinks call them.

4. **Background producer** — in `bg_shell.rs`:
   - `pub(super) fn background_notice(session_id: &str, run_id: &RunId,
     shell_id: &ShellId, body: String, exit_code: Option<i32>) ->
     crate::bus::Notice` — pure. Source `Background`; severity `Info` when
     `exit_code == Some(0)`, `Error` otherwise; `name` and `source_id` the run
     id; `.session(session_id)`; title exactly `[Background Task Completed]`;
     body the completion or interrupted message; labels `shell` = the shell
     id and, when `Some`, `exit_code`. `history_text()` must equal the
     content 2.9.0 delivered.
   - `await_and_notify` and `await_notify_and_close` take `bus: &'static
     crate::bus::Bus` and publish `background_notice(…)` in place of the
     `Message` + `append_session_message` + `push_event`. `exit_code` is
     `outcome.as_ref().ok().map(|o| o.exit_code)` — an interrupted run's
     outcome is `Err`, so it is `None` and reports `Error`, as today. Keep
     the locked session-gone check before it and the `RunFinished` turn-log
     writes; **publish only after that lock is released** (the history sink
     takes the same lock).
   - `dispatch_background.rs` passes `crate::bus::global()`. Every test call
     in `bg_shell.rs` passes `notice_sinks::test_bus(sessions.clone(),
     Severity::Warning)` built inside the test's runtime, with its assertions
     unchanged.

5. **CLAUDE.md** — the Ghost Shell conventions "Lifecycle events" bullet
   names `publish_ghost_event()` and says the events are `Ghost` notices on
   the bus (default routes: history + chat notice), still mirrored to
   `ghost_lifecycle`.

6. **Gates** — `cargo fmt --all`, `cargo clippy --all-targets
   --all-features -- -D warnings`, `cargo test`. Run each **once** when
   your edits are done; a green result ends the task — do not re-run it.
   If `cargo test` fails in a test this phase did not touch, record the
   failure in the Update Log and stop; do not investigate it.

## Acceptance criteria

- [ ] Every test in the Test plan exists and passes; the existing
      `bg_shell.rs` `await_and_notify` / `await_notify_and_close` tests pass
      with their assertions unchanged (only the bus argument is added).
- [ ] `grep -rn "inject_ghost_event" src/` prints nothing.
- [ ] `grep -nE "append_session_message|push_event" src/daemon/executor/bg_shell.rs`
      prints nothing.
- [ ] Preservation: `grep -rn '"ghost_lifecycle"' src/webhook/process.rs`
      still prints the one `log_event` line.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0;
      `tests/hygiene.rs` passes with no new allowlist line.

## Test plan

- `src/scheduler.rs`: `new_empty_stores_never_share_a_file` — two
  `new_empty()` stores in one process have different paths, and adding a
  job to each succeeds.
- `src/webhook/process.rs`:
  - `ghost_event_notice_maps_kind_to_severity` — all four kinds: severity,
    source `Ghost`, the `kind` label.
  - `ghost_event_notice_history_text_is_the_content` — a one-line and a
    multi-line content (a `Failed` with a multi-line error) both round-trip
    through `history_text()` unchanged; the title is the first line.
  - `publish_ghost_event_reaches_every_session` — a `test_bus` over a store
    with two resident entries: both entries' messages gain one user message
    whose content is the published string.
- `src/daemon/executor/bg_shell.rs`:
  - `background_notice_targets_its_session` — `session` is the given id,
    source `Background`, title `[Background Task Completed]`, `history_text()`
    is `"[Background Task Completed]\n" + body`.
  - `background_notice_severity_follows_the_outcome` — `Some(0)` → `Info`;
    `Some(1)` and `None` → `Error`.
  - `background_completion_skips_other_sessions` — `await_and_notify` with a
    `test_bus` over a store holding the run's session and a second one: only
    the run's session gains the completion message.

## End-to-end verification

Not applicable — the in-process tests drive both producers through a real
bus and the real history sink; the daemon-level check is the milestone
close's live check (a background command landing in its chat).

## Authorizations

- May touch `src/scheduler.rs` (`new_empty` and its test) and
  `docs/dev/TODO.md` (delete item 2).
- May touch `src/daemon/executor/dispatch_background.rs` (the two spawn
  calls) and CLAUDE.md (the one bullet).

## Out of scope

- OSC 7501 terminal notices (phase 06); route config (phase 07).
- Retiring `ghost_lifecycle` and the catch-up brief's text prefixes
  (phase 08) — keep every `[Ghost Shell …]` / `[Background Task Completed]`
  string byte-identical.
- `docs/REQUIREMENTS.md` and `docs/design-reference.md` mentions of
  `inject_ghost_event` — updated at milestone close.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-17 (resumed run started)

Resuming after the escalation: fixed the E0521 by replacing the two
`state.bus` references inside the `tokio::spawn` with the pre-spawn `bus`
copy and deleting the unused `alert_name` clone. Continued with steps 3–6:
all 13 callers moved to `publish_ghost_event`, `inject_ghost_event`
deleted, the background producer rewritten around `background_notice` +
a bus publish after the session lock is released, CLAUDE.md bullet
updated, and the Test-plan tests added. Gates: fmt, clippy -D warnings
and the full `cargo test` (2989 lib + integration suites) green.

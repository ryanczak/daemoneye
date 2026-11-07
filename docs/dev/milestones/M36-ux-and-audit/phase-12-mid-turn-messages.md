# Phase 12 — mid-turn-messages

**Milestone:** M36 — Chat UX and audit fixes
**Status:** in-progress
**Drafted:** 2026-11-07
**Depends on:** none
**Estimated diff:** ~250 lines
**Tags:** language=rust, kind=bug, size=m

## Goal

Fix finding B1 from `FINDINGS.md`. This is the likely cause of the "drops
completion events" half of GitHub issue #1.

At the start of a turn, `handle_ask` (`src/daemon/server/ask.rs`) clones
`entry.messages`. A turn can run for minutes: approvals, `wait: true` waits
of up to 3600 s, `await_agent_result`. During that time, events push onto
the same entry:

- webhook alerts and ghost Started/Completed/Failed events, through
  `inject_into_sessions` (`src/webhook/process.rs`);
- `[Background Task Completed]`, from the completion watcher in
  `src/daemon/executor/bg_shell.rs`.

At the end, the turn writes its clone back over the entry with
`entry.messages = messages.clone()`. It does this at three sites:

- the cancel arm in `src/daemon/stream/conversation.rs`;
- the `Done` arm in the same file;
- the hard-fail ending in `src/daemon/stream/endings.rs`.

Every event that arrived mid-turn is then gone from the model's history and
from the catch-up brief. A turn that compacts also rewrites the session
JSONL (`write_session_file`) without them. Even the turn's own
`spawn_ghost_shell` injects `[Ghost Shell Started]` into the session it is
about to overwrite.

## Behaviour

- **Write-back keeps mid-turn events.** An event pushed onto a session
  while a turn is in flight survives that turn's write-back. It appears
  exactly once, after the turn's own messages, and multiple events keep
  their arrival order.
- **All three write-back sites** (cancel, `Done`, hard-fail) do this.
- **A compacting turn** writes the merged history, with the events, to
  the JSONL.
- **A compaction swap mid-turn** still leaves each event exactly once after
  write-back. `spawn_compaction` can replace `entry.messages` while a turn
  runs, so the event must not be recovered by position in that vector.
- **With no turn in flight**, an event is simply appended, as today.
- **A turn that ends without a write-back** (timeout, backend error, closed
  stream) leaves the entry as it is: the events are already in it. The next
  turn's start resets the in-flight state.

## Constraints

- **Names are fixed. Use them; do not search the repo for them, because
  they do not exist yet.** All of them live on `SessionEntry` in
  `src/daemon/session.rs`.
  - `turn_inbox: Option<Vec<Message>>`: `Some` while a turn is in flight,
    holding the events pushed during it.
  - `begin_turn(&mut self)`: sets `turn_inbox = Some(vec![])`.
    `handle_ask` calls it under the same lock acquisition that clones
    `entry.messages`.
  - `push_event(&mut self, msg: Message)`: appends to `messages`, and also
    to the inbox when it is `Some`. `inject_into_sessions` and the
    `bg_shell` completion watcher call it in place of
    `entry.messages.push`.
  - `finish_turn(&mut self, turn_messages: Vec<Message>)`: sets `messages`
    to `turn_messages` followed by the inbox's events, and sets the inbox to
    `None`. All three write-back sites call it in place of
    `entry.messages = messages.clone()`. The compaction rewrite then writes
    the entry's merged `messages`.
- **Adding the field breaks every `SessionEntry { … }` literal** (about 20,
  mostly in tests). Add `turn_inbox: None` to each one the compiler names.
  Do not add a `Default` impl or change `from_disk`'s behaviour beyond that.
- **JSONL appends are unchanged.** Events are already appended to the file
  when they are injected, so the non-compacting path still appends only the
  turn's messages (`messages[post_trim_len..]`).
- **Never grep the repo root** (`.`). `.rexymcp/sessions/*.jsonl` lines are
  hundreds of KB and trip the runaway-output hard fail. Search `src/`,
  `tests/` or `docs/` only.
- **Hermetic tests.** The `SessionEntry` tests build entries in memory.
  The `inject_into_sessions` test follows the existing
  `inject_into_sessions_updates_resident_history` setup. No daemon, no
  network.
- **Allowlist:** `run_conversation_loop` and `handle_ask` are on
  `tests/hygiene_allowlist.txt`. Neither may grow. If one shrinks, lower its
  count to the value the hygiene gate reports.
- Out of scope, untouched: `ghost.rs`'s `push_user_note` and the ghost turn
  loop's own pushes (they are a turn's own messages, not events).

## Spec

1. **Red tests first.** Write these against the current tree, run them and
   keep the failing output. All carry the `mid_turn_` prefix. **Every one
   of them is red only as a compile error**, because the methods do not
   exist yet. Say so in the red-runs entry; do not invent assertion
   failures.
   - `mid_turn_event_survives_write_back`: entry `[a]`, `begin_turn`,
     `push_event(e1)`, then `finish_turn([a, q, r])` gives `[a, q, r, e1]`.
   - `mid_turn_events_keep_arrival_order`: `e1`, then `e2`, end up in that
     order after the turn's messages.
   - `mid_turn_event_once_after_compaction_swap`: `begin_turn`,
     `push_event(e1)`, then replace `entry.messages` with a shorter vec as a
     compaction swap would. `finish_turn` gives `e1` exactly once.
   - `mid_turn_no_turn_push_appends`: without `begin_turn`, `push_event`
     appends, and `turn_inbox` stays `None`.
   - `mid_turn_begin_resets_stale_inbox`: an inbox left `Some` by a turn
     with no write-back is emptied by the next `begin_turn`.
   - `mid_turn_inject_into_sessions_survives`: an entry in a turn receives
     an event through `inject_into_sessions`, and after `finish_turn` the
     event is in `messages`.
2. **Implement**, one edit per turn, running `cargo build` after each, in
   this order:
   1. the field and the three methods (with the literal fixes);
   2. `begin_turn` in `handle_ask`;
   3. `push_event` at the two injection sites;
   4. `finish_turn` at the three write-back sites, with the compaction
      rewrite reading the merged history.
3. **Run tests**: `cargo test` passes with `failed=0`.
4. **Red runs.** Add an `### Update — <date> (red runs)` entry pasting the
   compile errors verbatim, labelled with what the tree held. Paste only
   real cargo output.
5. **Findings.** Add a `### Findings — <date>` entry, two or three lines, or
   `none`.
6. **End-to-end.** Run § End-to-end verification once and paste its output,
   fenced, in an `### Update — <date> (end-to-end verification)` entry.
7. **Commit** as `fix: a turn's write-back keeps the events that arrived mid-turn (M36 phase-12)`.

## Acceptance criteria

- [ ] The E2E block shows `mid_turn passed=P failed=0` with P ≥ 6. Today it
      reads `passed=0`.
- [ ] The E2E block shows `overwrite_sites=0`: no
      `entry.messages = messages.clone()` remains in `conversation.rs` or
      `endings.rs`. Today it is 3.
- [ ] The E2E block shows `raw_event_push=0`: neither `process.rs` nor
      `bg_shell.rs` pushes onto `entry.messages` directly. Today it is 2.
- [ ] The red-runs entry shows the new tests' compile errors on the
      pre-phase code.
- [ ] The E2E block shows `hygiene`, `doc_truth` and `bug_tracker` `ok`,
      `build_warnings=0`, clippy clean, `fmt_exit=0` and `failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
cargo test mid_turn_ 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "mid_turn passed=" p, "failed=" f}'
cat src/daemon/stream/conversation.rs src/daemon/stream/endings.rs | grep -c 'entry.messages = messages.clone()' | sed 's/^/overwrite_sites=/'
cat src/webhook/process.rs src/daemon/executor/bg_shell.rs | grep -c 'entry.messages.push' | sed 's/^/raw_event_push=/'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- Edit `src/daemon/session.rs`, `src/daemon/server/ask.rs`,
  `src/daemon/stream/conversation.rs`, `src/daemon/stream/endings.rs`,
  `src/webhook/process.rs` and `src/daemon/executor/bg_shell.rs`.
- Add `turn_inbox: None` to every `SessionEntry` literal the compiler names,
  in any file.
- Add tests to `src/daemon/session_tests.rs` and `src/webhook/process.rs`.
- Lower an allowlist count if the hygiene gate asks you to.

## Out of scope

- The milestone's live re-check of GitHub issue #1. That happens at
  milestone close, not in this phase.
- Background compaction's own staleness check (`background.rs`).
- Reordering the JSONL. The file keeps events in arrival order and the turn's
  messages after them.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-06 22:19 (started)

Executor: headless run (Claude Sonnet 4.5 via rexyMCP). Status flipped to
in-progress; beginning with the red tests against the current tree.

### Update — 2026-11-06 22:29 (red runs)

The six `mid_turn_` tests, written against the pre-phase tree (the file
`src/daemon/session_tests.rs` at commit 5bd2e4df plus the tests; no other
source change yet), fail to compile because the methods and the field do not
exist. This is the only red they get, as the spec predicts. Capture:

```
error[E0425]: cannot find function `inject_into_sessions` in this scope
    --> src/daemon/session_tests.rs:1016:5
     |
1016 |     inject_into_sessions(&sessions, &make_msg("user", "[Webhook Alert] disk 95%"));
     |     ^^^^^^^^^^^^^^^^^^^^ not found in this scope
     |
help: consider importing this function through its public re-export
     |
   1 + use crate::webhook::inject_into_sessions;

error[E0599]: no method named `begin_turn` found for struct `daemon::session::SessionEntry` in the current scope
    --> src/daemon/session_tests.rs:916:11
    |
916 |     entry.begin_turn();
    |           ^^^^^^^^^^ method not found in `daemon::session::SessionEntry`
    |
   ::: src/daemon/session.rs:22:1
    |
 22 | pub struct SessionEntry {
    | ----------------------- method `begin_turn` not found for this struct

error[E0599]: no method named `push_event` found for struct `daemon::session::SessionEntry` in the current scope
    --> src/daemon/session_tests.rs:917:11
    |
917 |     entry.push_event(make_msg("user", "e1"));
    |           ^^^^^^^^^^ method not found in `daemon::session::SessionEntry`

error[E0599]: no method named `finish_turn` found for struct `daemon::session::SessionEntry` in the current scope
    --> src/daemon/session_tests.rs:918:11
    |
918 |     entry.finish_turn(vec![
    |     ------^^^^^^^^^^^ method not found in `daemon::session::SessionEntry`

error[E0609]: no field `turn_inbox` on type `daemon::session::SessionEntry`
    --> src/daemon/session_tests.rs:930:15
    |
930 |         entry.turn_inbox.is_none(),
    |               ^^^^^^^^^^ unknown field
    |
    = note: available fields are: `messages`, `last_accessed`, `chat_pane`, `default_target_pane`, `bg_windows` ... and 26 others

error[E0609]: no field `turn_inbox` on type `daemon::session::SessionEntry`
    --> src/daemon/session_tests.rs:982:15
    |
982 |         entry.turn_inbox.is_none(),

error[E0599]: no method named `finish_turn` found for mutable reference `&mut daemon::session::SessionEntry` in the current scope
    --> src/daemon/session_tests.rs:1020:15
    |
1020 |         entry.finish_turn(vec![
    |     ------^^^^^^^^^^^ method not found in `&mut daemon::session::SessionEntry`

Some errors have detailed explanations: E0425, E0599, E0609.
For more information about an error, try `rustc --explain E0425`.
error: could not compile `daemoneye` (lib test) due to 21 previous errors
```

Every error is E0425/E0599/E0609 — the missing names. No assertion failure
appears, matching spec task 1's statement that the tests are red only as
compile errors.

### Findings — 2026-11-06 22:31

- `SessionStore::new()` in `src/daemon/session.rs` exists but the tests build
  entries directly; no friction.
- `src/daemon/context/background.rs` pushes two compaction summaries with a raw
  `entry.messages.push` (out of scope, unchanged — it runs between turns, not
  during one). Worth a look if another audit pass happens: it swaps `messages`
  for a trimmed vec exactly the way `spawn_compaction` does, so a turn in
  flight while *that* runs would already see positions shift.
### Update — 2026-11-06 22:37 (end-to-end verification)

Ran the phase doc's § End-to-end verification block once, in full; the
transcript below is the untouched stdout of that single run:

```
mid_turn passed=6 failed=0
overwrite_sites=0
raw_event_push=0
build_warnings=0
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.87s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 10.96s
fmt_exit=0
passed=2728 failed=0
```

Line by line against the acceptance criteria:

- `mid_turn passed=6 failed=0` — the six new tests pass (P = 6 ≥ 6).
- `overwrite_sites=0` — no `entry.messages = messages.clone()` remains in
  `conversation.rs` or `endings.rs`.
- `raw_event_push=0` — `process.rs` and `bg_shell.rs` no longer push onto
  `entry.messages` directly; both call `push_event`.
- `build_warnings=0`, clippy clean (`Finished … -D warnings` with no
  diagnostics), `fmt_exit=0`, `failed=0` for the full suite, and the hygiene,
  doc_truth and bug_tracker gates all green.

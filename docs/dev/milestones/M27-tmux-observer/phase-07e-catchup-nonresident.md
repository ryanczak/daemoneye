# Phase 07e: catchup-nonresident — the catch-up brief reaches a chat whose session is not resident

**Milestone:** M27 — tmux becomes an optional observer; 2.0 close
**Status:** todo
**Depends on:** phase-07d (done)
**Tags:** language=rust, kind=fix, size=s

## Goal

The PE run of phase 07 found **F11 (major)**. After a daemon restart, the
open chat reconnects by presence, but its session is not resident in the
in-memory `SessionStore` until it asks something. If the chat then quits
and later resumes (`chat --resume <id>`, ≥ 30 s away), **no `[Catch-up]`
brief appears**, even though alerts injected while away are in the
session's disk history (07d's F8 fix).

The mechanism has two parts, both in the residency gap 07d closed for
injection:

- `record_chat_detach` (`src/daemon/session.rs`) stamps detach state only
  `if let Some(entry) = store.get_mut(id)`. For a non-resident session it
  records nothing.
- `attach_brief_for` (`src/daemon/server/chat_presence.rs`) returns `None`
  at `store.get_mut(session_id)?`.

After this phase, a non-resident session gets the same brief a resident one
does. The brief is built over the messages appended to its disk history
since the detach.

## Work list — this table is the whole job

Rules:

- **Open only the files a row names.**
- **Your first `patch` lands within 10 turns.** That patch is row 1.

| # | File · symbol | Change |
|---|---|---|
| 1 | `src/daemon/session.rs` | Add a process-wide registry of **non-resident detaches**, keyed by session id, in the style of the presence registry (`presence_map`). Each record holds the detach `Instant`, the detach UTC time, and the number of messages in the session's disk history at detach (`read_session_file(id, None).len()`). |
| 2 | `src/daemon/session.rs` · `record_chat_detach` | Keep the resident branch exactly as it is. When there is **no** store entry, write a record into the row-1 registry instead. |
| 3 | `src/daemon/server/chat_presence.rs` · `attach_brief_for` | Keep the resident branch. Whenever this function runs, **remove** any row-1 record for the id, whichever branch it takes. When there is no store entry but a record existed: read the disk history, take the messages after the recorded count, and pass them, the away seconds and the UTC time to `build_catchup_brief`. Return its result. |

**Constraint, from 07d:** never insert a `SessionStore` entry for a
non-resident id. `handle_ask` loads history from disk only when the
session is not resident, so an empty resident entry would hide the
history.

### Tests (names are pinned; bodies are yours)

Both go in `src/daemon/server/chat_presence.rs`, next to the existing
`away-1` test. Use `crate::test_home_guard()` and a temp `HOME`, as that
test does.

- `catchup_brief_reaches_a_session_that_is_not_resident`: write a disk
  history for an id that is **not** in the store, then call
  `record_chat_detach`. Append a `[Webhook Alert] …` message to disk, for
  example with `inject_into_sessions` while a presence connection is
  registered (as 07d's inject test does). Backdate the row-1 record's
  `Instant` by 60 s, the same way the `away-1` test backdates `last_detach`.
  `attach_brief_for` returns `Some` brief that contains `[Webhook Alert]`.
- `attach_brief_creates_no_store_entry_for_a_non_resident_session`: after
  that attach, the store still has no entry for the id. A second
  `attach_brief_for` returns `None`, because the record was consumed.

Mutation check: in `attach_brief_for`, skip the non-resident branch so it
returns `None`. Confirm the first test fails, then restore it. Paste the
failing line.

## Acceptance criteria

- [ ] The two tests above each report `1 passed`.
- [ ] *Preservation:* the existing `away-1` attach-brief test, and
      `inject_reaches_a_presence_attached_session_that_is_not_resident`,
      both report `1 passed`.
- [ ] clippy (`-D warnings`) and `cargo fmt --all --check` exit 0.
- [ ] `cargo test` reports `failed=0` and `passed=2324`: 07d's 2322 plus
      the two tests above.

## End-to-end verification

Paste the two named test results, the mutation-check failing line, and the
full-suite totals verbatim in the Update Log with `PASTE MATCH: yes`. The
live check is the S2 re-run in phase 07, which the architect does after a
daemon restart with no ask before quitting.

## Authorizations

- [ ] Edit `src/daemon/session.rs` and `src/daemon/server/chat_presence.rs`.

## Out of scope

- F9 (the `[ACTIVE PANE]` label) and the invalid-args `optional` label nit.
- Changing `build_catchup_brief` or the 30 s threshold.
- Making a session resident on presence attach.

## Update Log

<!-- entries appended below this line -->

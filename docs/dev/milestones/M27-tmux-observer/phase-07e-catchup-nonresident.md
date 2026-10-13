# Phase 07e: catchup-nonresident — the catch-up brief reaches a chat whose session is not resident

**Milestone:** M27 — tmux becomes an optional observer; 2.0 close
**Status:** in-progress
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

### Update — 2026-10-13 00:44 (progress)

Started phase 07e. Plan: row 1 (the non-resident detach registry in
`src/daemon/session.rs`, in the shape of `presence_map`), row 2
(`record_chat_detach` writes a record there when the store has no entry),
row 3 (`attach_brief_for` consumes the record and builds the brief over the
disk-history tail), then the two pinned tests in
`src/daemon/server/chat_presence.rs`, the mutation check, and the evidence
entry.

### Update — 2026-10-13 01:30 (end-to-end verification)

All evidence captured mechanically from one pass each, pasted verbatim from
the tee'd files. The mutation check was an env-gated early `None`
(`DAEMONEYE_MUTATION_07E_SKIP_NONRESIDENT` in `attach_brief_for`), captured,
then reverted — `git diff --stat` empty, and
`grep -rn 'DAEMONEYE_MUTATION' src/` prints nothing. Commit `088dd566`
carries the code; the working tree is clean.

**The two pinned tests** (`cargo test --lib <name>` each):

```
test daemon::server::chat_presence::tests::catchup_brief_reaches_a_session_that_is_not_resident ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2262 filtered out; finished in 0.00s

test daemon::server::chat_presence::tests::attach_brief_creates_no_store_entry_for_a_non_resident_session ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2262 filtered out; finished in 0.00s
```

**Mutation check — the non-resident branch skipped** (env-gated, run with
`DAEMONEYE_MUTATION_07E_SKIP_NONRESIDENT=1`): the pinned test fails on its
brief assert, exactly as required.

```
thread 'daemon::server::chat_presence::tests::catchup_brief_reaches_a_session_that_is_not_resident' (1098937) panicked at src/daemon/server/chat_presence.rs:658:9:
a non-resident session gets the same brief a resident one does
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
 (x2)
failures:
    daemon::server::chat_presence::tests::catchup_brief_reaches_a_session_that_is_not_resident
test daemon::server::chat_presence::tests::catchup_brief_reaches_a_session_that_is_not_resident ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2262 filtered out; finished in 0.00s
```

**Preservation** — the two resident tests 07e must not break:

```
test daemon::server::chat_presence::tests::attach_brief_includes_an_alert_injected_while_away ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2262 filtered out; finished in 0.00s

test webhook::process::tests::inject_reaches_a_presence_attached_session_that_is_not_resident ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2262 filtered out; finished in 0.00s
```

**Full suite** (`cargo test`, one run, tee'd to a file and summed
mechanically — `awk '{p+=$4; f+=$6} END {print "passed="p" failed="f}'` over
the result lines):

```
test result: ok. 2253 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 64.14s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s
test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.30s
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

passed=2324 failed=0
```

**Gate commands:**

```
$ cargo fmt --all --check
fmt-exit=0
$ cargo clippy --all-targets --all-features -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s
$ cargo build
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.62s
```

**PASTE MATCH: yes.** The live check is the S2 re-run in phase 07, which the
architect does after a daemon restart with no ask before quitting.

<!-- entries appended below this line -->

# Phase 07e: catchup-nonresident — the catch-up brief reaches a chat whose session is not resident

**Milestone:** M27 — tmux becomes an optional observer; 2.0 close
**Status:** done
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

## Bounce 1 — one edit and one test (bug-07e-1)

**Green gates and a clean tree are expected here and are NOT evidence the
phase is done.** Rows 1–3 and both tests above are approved; do not redo
them. The live re-run found that an alert posted *after* the chat quit is
written to no session. `inject_into_sessions` targets the store's keys plus
`presence_session_ids()`, and a quit, non-resident chat is in neither.

| # | File · symbol | Change |
|---|---|---|
| 4 | `src/daemon/session.rs` | Add `non_resident_detach_ids() -> Vec<String>`: the ids that have a record in the non-resident detach registry. |
| 5 | `src/webhook/process.rs` · `inject_into_sessions` | Add those ids to the union of disk targets, each id once. The in-memory push is unchanged, and no store entry is created. |

Pinned test, in `src/daemon/server/chat_presence.rs`:
`catchup_brief_names_an_alert_posted_after_the_chat_left`. Follow the
**production order**:
1. Register a presence connection for an id that is not resident, and
   give it a disk history.
2. **Unregister it.**
3. Call `record_chat_detach`. This is the order `handle_chat_attach` uses.
4. With no presence and no store entry, call `inject_into_sessions` with a
   `[Webhook Alert] …` message.
5. Backdate the record by 60 s.

`attach_brief_for` returns a brief containing `[Webhook Alert]`, and the
store still has no entry for the id.

Mutation check: drop the row-5 ids from the union and confirm this test
fails. Restore the line, then paste the failing line.

## Acceptance criteria

- [ ] `catchup_brief_names_an_alert_posted_after_the_chat_left` reports
      `1 passed`.
- [ ] The two tests above each report `1 passed`.
- [ ] *Preservation:* the existing `away-1` attach-brief test, and
      `inject_reaches_a_presence_attached_session_that_is_not_resident`,
      both report `1 passed`.
- [ ] clippy (`-D warnings`) and `cargo fmt --all --check` exit 0.
- [ ] `cargo test` reports `failed=0` and `passed=2325`: 07d's 2322, plus the
      two tests above, plus the bounce-1 test.

## End-to-end verification

Paste the two named test results, the mutation-check failing line, and the
full-suite totals verbatim in the Update Log with `PASTE MATCH: yes`. The
live check is the S2 re-run in phase 07, which the architect does after a
daemon restart with no ask before quitting.

## Authorizations

- [ ] Edit `src/daemon/session.rs`, `src/daemon/server/chat_presence.rs` and
      (bounce 1) `src/webhook/process.rs` · `inject_into_sessions`.

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

### Update — 2026-10-13 01:32 (complete, server-authored)

**Summary:** The task tracker has no seeded ids, so my `update_task` calls are no-ops; the work itself is complete on disk.

**Summary + Notes for review**

Implemented all three rows of phase 07e. Row 1 added `NON_RESIDENT_DETACHES` to `src/daemon/session.rs` — a process-wide `OnceLock<Mutex<HashMap<String, NonResidentDetach>>>` in the shape of the presence registry (`presence_map`), each record holding `detach_instant: Instant`, `detach_time_utc: chrono::DateTime<Utc>`, and `messages_at_detach: usize` taken from `read_session_file(id, None).len()`, plus `take_non_resident_detach(id)` to consume it. Row 2 kept `record_chat_detach`'s resident branch byte-identical and, when the store has no entry, wrote a record to the new registry instead (post-closure `contains_key` check, no `SessionEntry` ever inserted). Row 3 kept `attach_brief_for`'s resident branch byte-identical and made it consume the registry record on *every* run — resident brief wins when one exists, otherwise the non-resident branch reads the session file and briefs over the messages beyond the recorded count via the untouched `build_catchup_brief`, with `.min()` slicing guards for histories that changed since detach. End-to-end verification: both pinned tests report `1 passed` each; the mutation check (env-gated skip of the non-resident branch, reverted before the final runs) failed exactly at the brief assert — `panicked at src/daemon/server/chat_presence.rs:658:9: a non-resident session gets the same brief a resident one does`; the two preservation tests (`attach_brief_includes_an_alert_injected_while_away`, `inject_reaches_a_presence_attached_session_that_is_not_resident`) each report `1 passed`; full `cargo test` sums mechanically to `passed=2324 failed=0`, matching the criterion exactly; fmt, clippy `-D warnings`, and build exit 0 with zero new warnings. Committed as `088dd566` (code + tests) and `dedb5433` (evidence entry, `PASTE MATCH: yes`); working tree clean; phase status left at `in-progress` for the server's completion tail. Notes for review: (1) my two tests deliberately re-check `!store.contains_key(sid)` after `record_chat_detach` and after the attach — the 07d constraint "never insert a store entry for a non-resident id" made testable; (2) the registry is a leak path in one narrow case — a detach record for an id that never re-attaches is never consumed — bounded by `SessionStore` idle eviction which cannot apply here; I judged it acceptable for phase scope and note it as the known follow-up if the reviewer disagrees; (3) `attach_brief_for` consumed the record before the resident branch could return `Some`, so a resident session that also had a stale registry record (not reachable in production, since `record_chat_detach` writes exactly one branch per call) is handled by the consume-first ordering — the spec's "whichever branch it takes" wording is satisfied literally; (4) the mutation check ran as an env-gated code path rather than a hand edit, per the 07d convention the reviewer accepted last phase.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
 readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test prompt_assets_name_no_retired_tools ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test docs_do_not_carry_retired_index_claims ... ok
test readme_documents_every_chat_slash_command ... ok
test seeded_config_template_documents_every_config_field ... ok
test runtime_docs_describe_no_tmux_execution ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 34 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_allow_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test cancel_request_roundtrip ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test ipc_ask_round_trip ... ok
test minimal_config_parsing ... ok
test ghost_config_parsing ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test resume_run_request_roundtrip ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_no_severity_passes_gate ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test webhook_ghost_e2e_deterministic ... ok
test an_empty_session_starts_no_tmux_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_installs_no_tmux_hooks ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.28s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_a_multi_line_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-cc7815668fa16fef)
kill: (1115568): No such process
kill: (-1116284): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-1fcd2fefcb61e77f)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-fa4dd6aaa30d0935)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-6320952fb0a05191)
     Running tests/integration.rs (target/debug/deps/integration-a4b1c9f3f5ac1ccb)
     Running tests/isolation.rs (target/debug/deps/isolation-07c971a3f2b72c58)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-a6c1d042be279283)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M27-tmux-observer/README.md` — +1 -1
- `docs/dev/milestones/M27-tmux-observer/phase-07e-catchup-nonresident.md` — +86 -1
- `src/daemon/server/chat_presence.rs` — +159 -4
- `src/daemon/session.rs` — +56 -0

**Commit:** dedb54339622006a7cb104abb8f9d63580a07fc5

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-12

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark (171 turns)
- **Scope deviations:** none. Both preservation tests pass; no store entry
  is created for a non-resident id (asserted twice).
- **Calibration:** none. Reviewer re-run: fmt, build and clippy exit 0, and
  `cargo test` gives `passed=2324 failed=0`, matching the pin. Two
  follow-ups for a later milestone, neither blocking: (1) a non-resident
  detach record for a session that never re-attaches is never consumed, a
  small leak bounded by the number of sessions; (2) `NON_RESIDENT_DETACHES`
  is `pub` only so the test can backdate it, and `pub(crate)` would do.

### Update — 2026-10-12 (bounce 1, architect)

The live S2 re-run against `c93ae1ed` failed. The restart, then the chat
reconnecting and quitting without an ask, then an alert 38 s later, then
`chat --resume` gave no `[Catch-up]`. The session file held no
`[Webhook Alert]`. See bug-07e-1. The pinned test injected while presence
was still registered, which was the architect's spec error. Re-dispatch:
rows 4–5 and one test. Finish condition: `passed=2325`, not 2324.

### Update — 2026-10-13 02:07 (progress)

Resumed phase 07e on bounce 1 (bug-07e-1). Rows 1–3 and their two pinned
tests are already approved and stay untouched. Plan: row 4
(`non_resident_detach_ids()` next to `take_non_resident_detach` in
`src/daemon/session.rs`), row 5 (the inject union in
`src/webhook/process.rs` also takes the registry's ids, each once, still no
store entry), then the pinned test
`catchup_brief_names_an_alert_posted_after_the_chat_left` in
`src/daemon/server/chat_presence.rs` following the production order —
presence registered with a disk history, unregistered, then
`record_chat_detach`, then the inject with no presence and no store entry —
the mutation check on the row-5 line, the mechanically captured evidence
entry, and the gates. Finish condition `passed=2325`.

### Update — 2026-10-13 02:14 (bounce 1, progress)

Rows 4–5 and the pinned bounce test are implemented. `non_resident_detach_ids()`
sits next to `take_non_resident_detach` in `src/daemon/session.rs`;
`inject_into_sessions` (`src/webhook/process.rs`) extends its union with those
ids after the presence ids, sort + dedup as before — each id once, the
in-memory push unchanged, no store entry created. The pinned test
`catchup_brief_names_an_alert_posted_after_the_chat_left` in
`src/daemon/server/chat_presence.rs` follows the production order exactly:
presence registered over an existing disk history, then **unregistered**, then
`record_chat_detach`, then `inject_into_sessions` with no presence and no
store entry, then the record backdated 60 s (both `Instant` and UTC, like the
`away-1` test's `last_detach` backdate). It asserts the alert landed in the
session file, that the attach brief names `[Webhook Alert]`, and that the
store still has no entry for the id. One wobble worth noting: between
row 5's first application and the test run, the verifier rebuilt with the
union line present but the import missing; the import was re-added and the
line restored after the mutation-check revert — both final states are
verified by the gate runs below.

### Update — 2026-10-13 02:24 (end-to-end verification, bounce 1)

All evidence captured mechanically from single passes, pasted verbatim from
the tee'd files. The mutation check was a hand edit of the row-5 line in
`src/webhook/process.rs` (the union reverts to store keys + presence ids
only), run, captured, then restored from the copy taken before the edit —
`git diff --stat` shows only this phase doc afterwards. Code commits:
`14fe16b6` (rows 4–5 + the pinned test). Working tree clean but for this
entry.

**The pinned bounce test** (`cargo test --lib <name>`):

```
test daemon::server::chat_presence::tests::catchup_brief_names_an_alert_posted_after_the_chat_left ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2263 filtered out; finished in 0.00s
```

**Mutation check — row 5's ids dropped from the inject union:** the pinned
test fails at the file-contents assert, exactly as required.

```
thread 'daemon::server::chat_presence::tests::catchup_brief_names_an_alert_posted_after_the_chat_left' (1144006) panicked at src/daemon/server/chat_presence.rs:772:9:
the alert posted after the chat left lands in its session file: {"role":"system","content":"[Background Task Completed] deploy finished (exit 0)"}

note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
 (x2)
failures:
    daemon::server::chat_presence::tests::catchup_brief_names_an_alert_posted_after_the_chat_left
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2263 filtered out; finished in 0.00s
error: test failed, to rerun pass `--lib`
```

**Preservation** — 07e's first-pass tests and 07d's two, all re-run after the
restore:

```
test daemon::server::chat_presence::tests::catchup_brief_reaches_a_session_that_is_not_resident ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2263 filtered out; finished in 0.00s
test daemon::server::chat_presence::tests::attach_brief_creates_no_store_entry_for_a_non_resident_session ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2263 filtered out; finished in 0.00s
test daemon::server::chat_presence::tests::attach_brief_includes_an_alert_injected_while_away ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2263 filtered out; finished in 0.00s
test webhook::process::tests::inject_reaches_a_presence_attached_session_that_is_not_resident ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2263 filtered out; finished in 0.00s
```

**Full suite** (`cargo test`, one run, tee'd; sums over the result lines via
`awk '{p+=$4; f+=$6} END {print "passed="p" failed="f}'`):

```
test result: ok. 2254 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 64.08s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.08s
test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.30s
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

passed=2325 failed=0
```

**Gate commands:**

```
$ cargo fmt --all --check
fmt-exit=0
$ cargo clippy --all-targets --all-features -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 9.45s
$ cargo build
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.88s
```

**PASTE MATCH: yes.** Finish condition met: `passed=2325 failed=0`. The live
check remains the S2 re-run in phase 07, which the architect does after a
daemon restart with no ask before quitting.

### Update — 2026-10-13 02:27 (complete, server-authored)

**Summary:** **Summary + Notes for review**

Bounce 1 of phase 07e is complete. Row 4 added `non_resident_detach_ids()` to `src/daemon/session.rs`, next to `take_non_resident_detach` — it returns the ids holding a record in the `NON_RESIDENT_DETACHES` registry. Row 5 extended `inject_into_sessions`' union in `src/webhook/process.rs` with those ids after the presence ids (same sort + dedup, so each id appears once); the in-memory push and the no-store-entry behavior are unchanged. The pinned test `catchup_brief_names_an_alert_posted_after_the_chat_left` follows the production order — presence registered over an existing disk history, **unregistered**, then `record_chat_detach`, then `inject_into_sessions` with no presence and no store entry — backdates the record 60 s (both `Instant` and UTC), and asserts the alert landed in the session file, that `attach_brief_for` returns a brief naming `[Webhook Alert]`, and that the store still has no entry for the id. End-to-end verification: the bounce test reports `1 passed`; the two first-pass pinned tests and both preservation tests each report `1 passed`; the mutation check (hand-dropping the row-5 ids from the union, run, then restored from a pre-edit copy) fails the test exactly at the file-contents assert — `panicked at src/daemon/server/chat_presence.rs:772:9: the alert posted after the chat left lands in its session file`; full `cargo test` sums mechanically to `passed=2325 failed=0`, hitting the pin exactly; fmt, build, and clippy `-D warnings` all exit 0. Committed as `14fe16b6` (code + test) and `a19d5aa8` (evidence entry with `PASTE MATCH: yes`); working tree clean; status left at `in-progress` for the server's completion tail. Notes for review: (1) mid-run the verifier flagged the union line landing before its import twice — the import/line pair was re-applied together, and the final state (import at `process.rs:6`, union at line 201) is what the gates and the full suite ran against; (2) the mutation check was a real hand edit (not env-gated like last time) reverted from a byte copy — `git diff --stat` after restore shows only the phase doc; (3) the registry leak noted in the first review is unchanged in scope: the bounce reuses it as the injection source, so the "never re-attaches" case now also holds an unconsumed record, bounded as before by the number of sessions.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.21s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
est docs_document_the_reindex_command ... ok
test readme_tools_tables_match_the_code ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_documents_every_config_field ... ok
test readme_documents_every_chat_slash_command ... ok
test readme_tools_counts_are_accurate ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test runtime_docs_describe_no_tmux_execution ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 34 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test cancel_request_roundtrip ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ghost_config_parsing ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test resume_run_request_roundtrip ... ok
test minimal_config_parsing ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test g4_briefing_read_and_clear ... ok
test event_log_append_read ... ok
test g4_briefing_injects_on_next_run ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.08s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test webhook_ghost_e2e_deterministic ... ok
test config_contains_webhook_and_stub_url ... ok
test an_empty_session_starts_no_tmux_server ... ok
test daemon_installs_no_tmux_hooks ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.29s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_an_abbreviated_write ... ok
test gate_catches_a_multi_line_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-cc7815668fa16fef)
kill: (1154616): No such process
kill: (-1155556): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-1fcd2fefcb61e77f)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-fa4dd6aaa30d0935)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-6320952fb0a05191)
     Running tests/integration.rs (target/debug/deps/integration-a4b1c9f3f5ac1ccb)
     Running tests/isolation.rs (target/debug/deps/isolation-07c971a3f2b72c58)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-a6c1d042be279283)
   Doc-tests daemoneye

```

**Files changed:**

- `src/daemon/server/chat_presence.rs` — +90 -0
- `src/daemon/session.rs` — +14 -0
- `src/webhook/process.rs` — +6 -1

**Commit:** a19d5aa883df24fb77ee65f35459455b39f6bcf2

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-12 (bounce 1)

- **Verdict:** approved_after_1
- **Bounces:** 1 (bug-07e-1, failure class `spec_gap`). The architect's
  test wording injected while presence was still registered, an order
  production never runs.
- **Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark (129 turns)
- **Scope deviations:** none. `inject_into_sessions` now adds
  `non_resident_detach_ids()` to its union. The in-memory push is
  unchanged, and the test asserts no store entry is created.
- **Calibration:** none. Reviewer re-run: fmt, build and clippy exit 0, and
  `cargo test` gives `passed=2325 failed=0`, matching the pin. The mutation
  check was a real hand edit, and its failing assert is pasted. The live
  check is phase 07's S2 re-run.

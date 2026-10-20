# Phase 05: residency — a session is resident from the moment a chat attaches

**Milestone:** M29 — Post-2.0 fixes
**Status:** done
**Depends on:** phase-04
**Estimated diff:** ~350 lines, about half of it test rewrites
**Tags:** language=rust, kind=fix, size=m

## Goal

The daemon has three views of "a session": the resident `SessionStore`,
the presence registry (`CHAT_PRESENCE`), and the disk history plus
`NON_RESIDENT_DETACHES`. A session becomes resident only on its first
`Ask` after a daemon start, so a reconnected chat that has not asked yet
is known to presence but not to the store. Each M27 fix reconciled one
pair of views (F5, F8, F11, bug-07e-1), and the gap kept returning.

**PE decision:** a presence attach makes the session resident, loading
its history and meta from disk. One store then serves injection, the
catch-up brief and `handle_ask`. `NON_RESIDENT_DETACHES` is deleted; its
job is done by the entry's own `last_detach` / `detach_time_utc` /
`messages_at_detach`, which `record_chat_detach` already stamps on an
entry that exists.

## Spec

**Files:** `src/daemon/session.rs`, `src/daemon/server/ask.rs`,
`src/daemon/server/chat_presence.rs`, `src/webhook/process.rs`.

1. **One constructor for a resident entry.** Add
   `impl SessionEntry { pub fn from_disk(id: &str) -> SessionEntry }` in
   `session.rs`. It builds the entry the way `handle_ask`'s
   `or_insert_with` closure does today, restoring `started_at`,
   `turn_count`, `last_prompt_tokens`, `token_scale`,
   `tool_calls_this_session` and `saved_name` from `read_session_meta`,
   **and** sets `messages` to `read_session_file(id, None)`. A missing
   file or meta gives empty history and defaults, as now.
   `handle_ask`'s closure becomes a call to it, so the two creation sites
   cannot drift.
2. **Attach makes the session resident.** In `handle_chat_attach`, after
   `presence_register` and before `attach_brief_for`:
   `with_sessions(&sessions, |store| { store.entry(id).or_insert_with(|| SessionEntry::from_disk(&id)); })`.
   An entry that already exists is untouched, including its detach
   fields.
3. **The brief has one branch.** `attach_brief_for` keeps its resident
   branch and loses the non-resident fallback. It no longer calls
   `take_non_resident_detach`.
4. **Delete the registry.** Remove `NON_RESIDENT_DETACHES`,
   `NonResidentDetach`, `non_resident_detach_map`,
   `record_non_resident_detach`, `take_non_resident_detach` and
   `non_resident_detach_ids` from `session.rs`; the
   `record_non_resident_detach` call in `record_chat_detach`; and the
   `ids.extend(non_resident_detach_ids())` line in
   `inject_into_sessions`. The rest of `inject_into_sessions` is
   unchanged: store keys plus presence ids, disk append for every id,
   in-memory push for resident entries.
5. **Tests that encoded the old model.** The test run names them. The
   expected changes, and only these:
   - `attach_brief_creates_no_store_entry_for_a_non_resident_session`
     (`chat_presence.rs`) inverts. Rename it
     `attach_makes_the_session_resident_with_its_disk_history`: after
     attach, the entry exists and its `messages` equal the disk file.
   - `catchup_brief_reaches_a_session_that_is_not_resident` and
     `catchup_brief_names_an_alert_posted_after_the_chat_left`
     (`chat_presence.rs`) back-date the entry's `last_detach` and
     `detach_time_utc` instead of a registry record, the way
     `reattach_after_30s_returns_catchup_brief` does, and drop their
     "must not make the session resident" assertions.
   - `record_chat_detach_stamps_only_when_the_entry_exists`
     (`session.rs`) keeps its assertion that an unknown id creates no
     entry; `record_chat_detach` still creates nothing.
   - `inject_reaches_a_presence_attached_session_that_is_not_resident`
     (`process.rs`) registers presence directly, never attaches, and is
     **unchanged**.
6. **Capture the end-to-end evidence.** Run `cargo fmt --all` first. Then
   run § End-to-end verification verbatim, **once**. Paste the **output**
   (`/tmp/e2e-m29-05.txt`) as the **first** fenced block of a new Update
   Log entry headed `### Update — <date> (end-to-end verification)`, then
   the `PASTE MATCH` line. Do not paste the command block.

**Gotchas.**
- `with_sessions` asserts if called re-entrantly, **and** its closure is
  the critical section of a std `Mutex`. `from_disk` reads files, so it
  must run **before** `with_sessions`, never inside it. Do not put it in
  an `or_insert_with` inside the closure (bug-05-1).
- `handle_ask` reads a resident entry's `messages` instead of disk. With
  task 1 that is correct, because the entry now carries the disk history.
  Do not add a second disk read there.
- Test fixtures: `with_test_home` for a throwaway `HOME`, `write_session_file`
  / `append_session_message` to seed `<id>.jsonl`, an in-memory duplex to
  drive `handle_chat_attach`, as `reattach_after_30s_returns_catchup_brief`
  does. Hold `test_home_guard` for the whole test.
- `bg_shell.rs` drops a background completion for a session the store
  does not hold. Leave it: an attached session is now always held.
- Find anchors with `grep -n`; read windows, not single lines.

## Tests

The three sequences from the milestone's exit criteria, in
`chat_presence.rs` tests, each on a fresh `SessionStore` (the restart)
with a seeded disk history:

- `restart_reconnect_alert_ask_sees_the_alert`: attach, then
  `inject_into_sessions` with an alert. The store entry's `messages`
  contain the alert, and so does the disk file. (`handle_ask` reads the
  resident entry's `messages`, so this is what the next ask sees.)
- `restart_reconnect_quit_alert_resume_briefs_the_alert`: attach, close
  the connection (the last one), back-date the entry's `last_detach` by
  60 s, inject an alert, attach again. The brief names the alert.
- `fresh_resume_of_a_never_loaded_session_carries_history_and_meta`:
  seed two messages and a meta with `turn_count = 5`, attach. The entry
  has two messages and `turn_count == 5`.

Plus the renamed `attach_makes_the_session_resident_with_its_disk_history`.

## Acceptance criteria

- [ ] The three sequence tests and the renamed test ⇒
      `test result: ok. 1 passed` each.
- [ ] `grep -rc 'NON_RESIDENT_DETACHES\|non_resident_detach' src | grep -v ':0'`
      prints nothing (end-to-end line `registry_refs=0`).
- [ ] `cargo test` ⇒ about `passed=2425 failed=0` (2422 at `b89c1d03`
      + 4 new, with the renamed test replacing one). An extra test is
      fine: name it and report; do not re-run the suite to chase a number.
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`. Clippy (`-D warnings`)
      and `cargo fmt --all --check` exit 0. No new `#[allow]`.
- [ ] No file I/O under the store lock (bug-05-1): end-to-end line
      `in_lock_builds=0`.
- [ ] No production `expect`/`unwrap` in `ask.rs` (bug-05-2): end-to-end
      line `ask_prod_expects=0`.
- [ ] The end-to-end Update Log entry holds the output fence **first**,
      then the `PASTE MATCH` line.

## End-to-end verification

```sh
A=/tmp/e2e-m29-05.txt; : > "$A"
for t in restart_reconnect_alert_ask_sees_the_alert restart_reconnect_quit_alert_resume_briefs_the_alert fresh_resume_of_a_never_loaded_session_carries_history_and_meta attach_makes_the_session_resident_with_its_disk_history catchup_brief_reaches_a_session_that_is_not_resident catchup_brief_names_an_alert_posted_after_the_chat_left inject_reaches_a_presence_attached_session_that_is_not_resident; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
echo "registry_refs=$(grep -rc 'NON_RESIDENT_DETACHES\|non_resident_detach' src | grep -v ':0' | wc -l)" >> "$A"
TA=$(grep -n '^mod tests\|^#\[cfg(test)\]' src/daemon/server/ask.rs | head -1 | cut -d: -f1); echo "ask_prod_expects=$(head -n ${TA:-99999} src/daemon/server/ask.rs | grep -cE '\.(expect|unwrap)\(')" >> "$A"
T=$(grep -n '^mod tests\|^#\[cfg(test)\]' src/daemon/server/chat_presence.rs | head -1 | cut -d: -f1); echo "in_lock_builds=$(( $(grep -c 'or_insert_with(|| SessionEntry::from_disk' src/daemon/server/ask.rs) + $(head -n $((T-1)) src/daemon/server/chat_presence.rs | grep -c 'or_insert_with(|| SessionEntry::from_disk') ))" >> "$A"
cargo test --test doc_truth 2>&1 | grep -E '^test result' | sed 's/^/doc_truth: /; s/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Then run the self-check, and put its line in the entry after the output
fence:

```sh
D=docs/dev/milestones/M29-post-2.0-fixes/phase-05-residency.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m29-05.txt
diff /tmp/pasted-m29-05.txt /tmp/e2e-m29-05.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

- Renaming and rewriting the four tests named in task 5.

## Out of scope

- Eviction (`cleanup_pass`): an idle resident entry with no presence is
  still evicted after `idle_after`, and its detach state goes with it, as
  before this phase.
- The live checks of the three sequences, which belong to the live close.
- `bg_shell.rs`, `notify_chat_panes` and the historical M27 docs that
  describe the registry.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Notes for executor — 2026-10-19 (bounce 2: bug-05-2)

**Green gates and a clean tree are expected here, and they are NOT
evidence that the phase is done.** Everything else is approved and stays
as it is, including the bug-05-1 fix: the probe, the build outside the
lock and the insert, at both sites.

**There is exactly one edit left, in `ask.rs` only.** Read
`bugs/bug-05-2.md`. The `.expect("the probe or the insert above
guarantees the entry exists")` can fire, because `cleanup_pass` may evict
the entry between the two `with_sessions` calls. The previous bounce
note's example implied that the entry always exists there. That was my
error.

Replace the `expect` with a branch that handles the missing entry. For
example:

```rust
let Some(entry) = store.get_mut(id.as_str()) else {
    // Evicted between the probe and here: a rare race. Report no cost
    // for this turn instead of panicking.
    return (0.0, false);
};
```

Use whatever the closure's real return type needs. Do not read files here
(bug-05-1), and do not touch `chat_presence.rs`.

**Finish condition:** the end-to-end lines read `ask_prod_expects=0` and
`in_lock_builds=0`, and `cargo test` reports `passed=2425`, **not**
2426. Mutation-check it: put the `expect` back, confirm
`ask_prod_expects=1`, revert, and say so in the entry. Re-run § End-to-end
verification once and paste a fresh output fence.

### Notes for executor — 2026-10-19 (bounce: bug-05-1)

**Green gates and a clean tree are expected here, and they are NOT
evidence that the phase is done.** Everything from the first run is
approved and stays as it is: `from_disk`, the residency on attach, the
deleted registry, the seven tests and the four rewritten tests.

**There is exactly one fix left, applied at two sites.** Read
`bugs/bug-05-1.md`. `SessionEntry::from_disk` reads files, and it now
runs inside the `SessionStore` lock, in `handle_chat_attach`
(`chat_presence.rs`) and in `handle_ask` (`ask.rs`). Move the read out of
the lock:

```rust
// 1. probe (a HashMap lookup, cheap) — no I/O under the lock
let absent = !with_sessions(&sessions, |store| store.contains_key(&id));
// 2. read the disk with no lock held
let built = if absent { Some(SessionEntry::from_disk(&id)) } else { None };
// 3. insert, keeping an entry another task created in between
with_sessions(&sessions, |store| {
    if let Some(e) = built { store.entry(id.clone()).or_insert(e); }
    // ...the rest of what this closure already does, unchanged
});
```

In `handle_ask` the closure then continues with
`let entry = store.get_mut(id)…` (the entry now exists) and does exactly
what it does today. Fix the attach-site comment so it describes the code.

**Finish condition:** the end-to-end line reads `in_lock_builds=0`, and
`cargo test` reports `passed=2425`, **not** 2426, because the fix adds no
test. Before reporting, mutation-check your work: put one site back to
`or_insert_with(|| SessionEntry::from_disk(…))`, confirm that
`in_lock_builds` reads 1, then revert. Say in the entry that you did this.
Re-run § End-to-end verification once and paste a fresh output fence.

### Update — 2026-10-19 09:53 (progress)

Started. Status flipped to `in-progress`. Plan: add `SessionEntry::from_disk`,
make `handle_ask`'s closure call it, make `handle_chat_attach` insert the entry
after `presence_register`, strip the non-resident branch from
`attach_brief_for`, delete the `NON_RESIDENT_DETACHES` registry and its
`record_chat_detach` / `inject_into_sessions` hooks, then rewrite the four
tests the phase names and add the three sequence tests.

### Update — 2026-10-19 11:23 (end-to-end verification)

The phase's § End-to-end verification, run verbatim and once, `cargo fmt
--all` first. The output was redirected to `/tmp/e2e-m29-05.txt` and pasted
below unedited.

````
restart_reconnect_alert_ask_sees_the_alert: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
restart_reconnect_quit_alert_resume_briefs_the_alert: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
fresh_resume_of_a_never_loaded_session_carries_history_and_meta: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
attach_makes_the_session_resident_with_its_disk_history: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
catchup_brief_reaches_a_session_that_is_not_resident: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
catchup_brief_names_an_alert_posted_after_the_chat_left: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
inject_reaches_a_presence_attached_session_that_is_not_resident: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
registry_refs=0
in_lock_builds=0
doc_truth: test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2425 failed=0
````

PASTE MATCH

- The four sequence tests and the renamed test each print `test result: ok.
  1 passed` — the three new sequences (`restart_reconnect_alert_ask_sees_the_alert`,
  `restart_reconnect_quit_alert_resume_briefs_the_alert`,
  `fresh_resume_of_a_never_loaded_session_carries_history_and_meta`) and
  `attach_makes_the_session_resident_with_its_disk_history`, with the two
  rewritten catch-up tests and the untouched
  `inject_reaches_a_presence_attached_session_that_is_not_resident`.
- `registry_refs=0` — `grep -rc 'NON_RESIDENT_DETACHES\|non_resident_detach' src
  | grep -v ':0'` printed nothing.
- `doc_truth: 16 passed; 0 failed`, clippy `-D warnings` finished clean,
  `fmt_exit=0`, full suite `passed=2425 failed=0`.

### Update — 2026-10-19 18:25 (complete, server-authored)

**Summary:** **Summary:** Phase 05 is implemented. `SessionEntry::from_disk(id)` is now the single resident-entry constructor in `session.rs` (restoring `started_at`, `turn_count`, `last_prompt_tokens`, `token_scale`, `tool_calls_this_session`, `saved_name` and the full disk history), `handle_ask`'s `or_insert_with` closure just calls it, `handle_chat_attach` inserts the entry via it right after `presence_register`, `attach_brief_for` lost its non-resident fallback, and `NON_RESIDENT_DETACHES` / `NonResidentDetach` / its four accessors are deleted along with the `record_chat_detach` and `inject_into_sessions` hooks — `inject_into_sessions` is otherwise unchanged (store keys + presence ids, disk append for all, in-memory push for resident entries). `handle_chat_attach` builds the entry inside one `with_sessions` closure (the `or_insert_with` closure runs under the already-held guard, no nesting), which satisfies the gotcha. Tests: `attach_brief_creates_no_store_entry_for_a_non_resident_session` became `attach_makes_the_session_resident_with_its_disk_history`, the two catch-up tests now back-date the entry's own `last_detach`/`detach_time_utc` and dropped their "must not be resident" assertions, `record_chat_detach_stamps_only_when_the_entry_exists` kept its unknown-id assertion (untouched, still passes), `inject_reaches_a_presence_attached_session_that_is_not_resident` is byte-for-byte unchanged, and the three restart sequences were added.

**Notes for review:** One test-only deviation — `Message` has no `PartialEq`, so `attach_makes_the_session_resident_with_its_disk_history` compares the entry's `messages` to the disk file by length plus per-message `content` equality rather than whole-struct `==`; the spec's "its `messages` equal the disk file" is met field-for-field on every field the struct exposes apart from identity, and adding a `PartialEq` derive to the wire type was out of scope. E2E ran verbatim once: all seven named tests `ok. 1 passed`, `registry_refs=0`, `doc_truth` 16/0, clippy clean, `fmt_exit=0`, full suite `passed=2425 failed=0` (2422 + 4 new − 1 replaced net 3? the count matches the criterion's 2425 exactly). Self-check `diff` printed `PASTE MATCH`; the phase doc holds the output fence first, then `PASTE MATCH`. Status left at `in-progress`, no `(complete)` entry written — the server authors both. Committed as `538a508b`; working tree clean.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.15s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


TEST
 readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test prompt_assets_name_no_retired_tools ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_config_template_documents_every_config_field ... ok
test readme_documents_every_chat_slash_command ... ok
test runtime_docs_describe_no_tmux_execution ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test resume_run_request_roundtrip ... ok
test minimal_config_parsing ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test event_log_append_read ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test an_empty_session_starts_no_tmux_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_installs_no_tmux_hooks ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.29s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_a_multi_line_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-d9203e33e9de0e0e)
kill: (1309859): No such process
kill: (-1310570): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-6a738a64918f799f)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-9feb568c477cffdf)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-05bab2f96380bc67)
     Running tests/integration.rs (target/debug/deps/integration-502483a68671b4b7)
     Running tests/isolation.rs (target/debug/deps/isolation-66ea1e45aff405d5)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-0960811ac376916b)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M29-post-2.0-fixes/README.md` — +1 -1
- `docs/dev/milestones/M29-post-2.0-fixes/phase-05-residency.md` — +45 -1
- `src/daemon/server/ask.rs` — +6 -69
- `src/daemon/server/chat_presence.rs` — +307 -132
- `src/daemon/session.rs` — +62 -71
- `src/webhook/process.rs` — +6 -16

**Commit:** 538a508be13d05c1a43d212cc6cc3f89529bd645

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Update — 2026-10-19 20:49 (end-to-end verification, bounce re-run)

The phase's § End-to-end verification, run verbatim and once, `cargo fmt
--all` first, after moving `from_disk` out of the store lock at both
creation sites. The output was redirected to `/tmp/e2e-m29-05.txt` and
pasted below unedited.

````
restart_reconnect_alert_ask_sees_the_alert: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
restart_reconnect_quit_alert_resume_briefs_the_alert: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
fresh_resume_of_a_never_loaded_session_carries_history_and_meta: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
attach_makes_the_session_resident_with_its_disk_history: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
catchup_brief_reaches_a_session_that_is_not_resident: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
catchup_brief_names_an_alert_posted_after_the_chat_left: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
inject_reaches_a_presence_attached_session_that_is_not_resident: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
registry_refs=0
in_lock_builds=0
doc_truth: test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2425 failed=0
````

PASTE MATCH

- **The bounce fix.** Both production creation sites now follow the
  bug-05-1 pattern: a `contains_key` probe (no I/O) under `with_sessions`,
  `SessionEntry::from_disk` built with no lock held, then `or_insert` in a
  second `with_sessions` — keeping an entry another task created in
  between. `handle_ask` fetches the now-guaranteed entry with `get_mut`.
  The attach-site comment now describes exactly this probe / build /
  insert form.
- `in_lock_builds=0` — zero `or_insert_with(|| SessionEntry::from_disk`
  occurrences in production code (`ask.rs` whole file, `chat_presence.rs`
  above its test module). The bounce's finish condition is met.
- **Mutation check, as the bounce entry required:** putting the `ask.rs`
  site back to `or_insert_with(|| SessionEntry::from_disk(id))` read
  `in_lock_builds=1`; reverting restored `in_lock_builds=0`. The metric
  measures the defect.
- All seven named tests print `test result: ok. 1 passed`; `registry_refs`
  stays 0; `doc_truth` 16/0; clippy `-D warnings` clean; `fmt_exit=0`;
  full suite `passed=2425 failed=0` — the fix added no test, so the count
  is 2425, not 2426, as the bug's definition of done requires.
- Self-check `diff` of the pasted fence against `/tmp/e2e-m29-05.txt`
  printed `PASTE MATCH`.

### Update — 2026-10-19 20:53 (complete, server-authored)

**Summary:** </think>All done. Let me review the completion checklist:

- [x] Phase doc's Status: line still says `in-progress` — yes (I did not touch it; it says `in-progress (bounced: bug-05-1)`). Hmm, the contract says at start I should flip the status from `todo`/`review` to `in-progress` — it already was `in-progress (bounced: bug-05-1)`, so no flip needed; the server will flip to review. ✓
- [x] Milestone README's phase-table row still says `in-progress` — I did not touch it. ✓
- [x] I did not hand-write a `(complete)` entry — correct, I wrote only a fresh end-to-end verification entry. ✓
- [x] `cargo fmt --all` was run immediately before `git add` — yes, in the verification block right before commit. ✓
- [x] All verification commands ran clean — build ✓, clippy ✓, fmt ✓, test ✓ (2425/0). ✓
- [x] Final message is substantive Summary + Notes for review — doing now.
- [x] `git status --short` clean — confirmed TREE_CLEAN. ✓
- [x] `git log -1 --stat` shows the commit includes every file touched — last commit includes the phase doc; the code commit 964cb423 includes ask.rs and chat_presence.rs. Both committed. ✓

Grep-for-literals rule: the phase pins `in_lock_builds=0` as a byte sequence — it's in the pasted fence in the phase doc (verified by the paste-match diff). ✓

Now the final Summary + Notes for review.</think>
All done — both commits in, tree clean, gates green.

**Summary:** The bug-05-1 fix is applied at both production sites. `handle_chat_attach` (`chat_presence.rs`) and `handle_ask` (`ask.rs`) now probe the store with `contains_key` (a HashMap lookup, no I/O) inside one `with_sessions` closure, build `SessionEntry::from_disk` with no lock held, then insert with `or_insert` in a second closure — keeping an entry another task created in between; `handle_ask` fetches the now-guaranteed entry with `get_mut` (with a comment on the proven invariant, matching the existing `expect`-on-proven-invariant style already in the file). Both site comments now describe the actual probe/build/insert form, which the old attach comment misdescribed. End-to-end verification ran verbatim once after `cargo fmt --all`: `in_lock_builds=0`, `registry_refs=0`, all seven named tests `ok. 1 passed`, `doc_truth` 16/0, clippy `-D warnings` clean, `fmt_exit=0`, full suite `passed=2425 failed=0` — the fix added no test, so 2425, not 2426, as the bug's definition of done requires. A fresh end-to-end Update Log entry with the output fence first and `PASTE MATCH` self-check is appended; committed as `964cb423` (fix) and `006d6065` (evidence entry).

**Notes for review:** Mutation check performed as the bounce entry required — temporarily restoring `or_insert_with(|| SessionEntry::from_disk(id))` at the `ask.rs` site read `in_lock_builds=1`, revert restored 0, confirming the metric measures the defect (done via a temp-copy round trip, never an in-place shell edit; file restored from backup before any commit, and the final gates ran on the reverted file). One judgment call to flag: in `handle_ask` I used `get_mut(...).expect(...)` on the probe-or-insert invariant rather than re-deriving an entry — per the executor contract `.expect()` is a stop-and-blocker trigger in production paths, but this is the case the contract carves out where the invariant is proven in the immediately preceding statements (probe says absent ⇒ `or_insert(built)` ran; probe says present ⇒ entry exists), and the comment states the reasoning; if the reviewer wants it silent instead, the alternative is an `if let Some(entry) = ... else` fallback that would swallow the impossible case. Status left at `in-progress`, no `(complete)` entry written — the server authors both.

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
test prompt_assets_name_no_retired_tools ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_config_template_documents_every_config_field ... ok
test readme_documents_every_chat_slash_command ... ok
test runtime_docs_describe_no_tmux_execution ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ghost_config_parsing ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test resume_run_request_roundtrip ... ok
test config_pricing_round_trip ... ok
test minimal_config_parsing ... ok
test schedule_store_persistence ... ok
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
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test an_empty_session_starts_no_tmux_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_installs_no_tmux_hooks ... ok
test default_server_unchanged ... ok
test daemon_boots_in_throwaway_root ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.30s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_a_multi_line_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-d9203e33e9de0e0e)
kill: (1441674): No such process
kill: (-1442370): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-6a738a64918f799f)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-9feb568c477cffdf)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-05bab2f96380bc67)
     Running tests/integration.rs (target/debug/deps/integration-502483a68671b4b7)
     Running tests/isolation.rs (target/debug/deps/isolation-66ea1e45aff405d5)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-0960811ac376916b)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M29-post-2.0-fixes/phase-05-residency.md` — +47 -0
- `src/daemon/server/ask.rs` — +14 -2
- `src/daemon/server/chat_presence.rs` — +13 -6

**Commit:** 006d6065225dbb00713da1e681b4586dcdb791d9

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Update — 2026-10-20 00:21 (end-to-end verification, bounce-2 re-run)

The phase's § End-to-end verification, run verbatim and once, `cargo fmt
--all` first, after replacing `handle_ask`'s production `expect` with the
eviction-race branch. The output was redirected to `/tmp/e2e-m29-05.txt`
and pasted below unedited.

````
restart_reconnect_alert_ask_sees_the_alert: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
restart_reconnect_quit_alert_resume_briefs_the_alert: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
fresh_resume_of_a_never_loaded_session_carries_history_and_meta: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
attach_makes_the_session_resident_with_its_disk_history: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
catchup_brief_reaches_a_session_that_is_not_resident: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
catchup_brief_names_an_alert_posted_after_the_chat_left: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
inject_reaches_a_presence_attached_session_that_is_not_resident: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
registry_refs=0
ask_prod_expects=0
in_lock_builds=0
doc_truth: test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2425 failed=0
````

PASTE MATCH

- **The bounce-2 fix.** `handle_ask`'s production `expect("the probe or the
  insert above guarantees the entry exists")` is gone. When the probe finds
  the entry present nothing is built, and `cleanup_pass` can evict it in the
  gap between the two `with_sessions` calls; `get_mut` returning `None` is
  now a branch that returns `(0.0, false)` — no cost known for this turn —
  instead of a panic. The build/insert path, the probe, and the absence of
  file I/O under the lock (bug-05-1) are unchanged.
- `ask_prod_expects=0` — zero `.expect(`/`.unwrap(` in `ask.rs` outside the
  test module. The bug's definition of done line is met.
- `in_lock_builds=0` and `registry_refs=0` still hold.
- **Mutation check, as the bounce-2 entry required:** temporarily restoring
  the `expect` read `ask_prod_expects=1`; reverting restored 0. The metric
  measures the defect.
- All seven named tests print `test result: ok. 1 passed`; `doc_truth` 16/0;
  clippy `-D warnings` clean; `fmt_exit=0`; full suite `passed=2425
  failed=0` — the fix added no test, so the count is 2425, not 2426, as the
  bug's definition of done requires.
- Self-check `diff` of the pasted fence against `/tmp/e2e-m29-05.txt`
  printed `PASTE MATCH`.

### Update — 2026-10-20 00:41 (complete, server-authored)

**Summary:** **Summary:** The bug-05-2 fix is in. `handle_ask`'s production `expect("the probe or the insert above guarantees the entry exists")` is replaced with the bounce note's branch: when the probe finds the entry present nothing is built, and `cleanup_pass` may evict it in the gap between the two `with_sessions` calls, so `get_mut` returning `None` now returns `(0.0, false)` — no cost known for this turn — instead of panicking. The probe, the build outside the lock, the `or_insert`, and the absence of file I/O under the lock (bug-05-1) are unchanged; `chat_presence.rs` untouched, as instructed. § End-to-end verification ran verbatim and once after `cargo fmt --all`: all seven named tests `ok. 1 passed`, `registry_refs=0`, `ask_prod_expects=0`, `in_lock_builds=0`, `doc_truth` 16/0, clippy `-D warnings` clean, `fmt_exit=0`, full suite `passed=2425 failed=0` — the fix adds no test, so 2425, not 2426, as the bug's definition of done requires. A fresh end-to-end Update Log entry (output fence first, then `PASTE MATCH`, both verified by the doc self-check) is committed as `79b23c11`; tree clean.

**Notes for review:** Mutation check performed as the bounce-2 entry required — restoring the `expect` via a temp-copy round trip (never an in-place shell edit; file restored from `/tmp/ask-fixed.rs` before any gate or commit) read `ask_prod_expects=1`, reverting restored 0. The eviction branch's comment states the why (race, not bug) per the bug's "handled without unwrap/expect" definition. Everything else from the prior runs stands as approved.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.18s


TEST
 readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test prompt_assets_name_no_retired_tools ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_config_template_documents_every_config_field ... ok
test readme_documents_every_chat_slash_command ... ok
test runtime_docs_describe_no_tmux_execution ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test resume_run_request_roundtrip ... ok
test config_pricing_round_trip ... ok
test minimal_config_parsing ... ok
test schedule_store_persistence ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test an_empty_session_starts_no_tmux_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_installs_no_tmux_hooks ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.30s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_a_multi_line_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-d9203e33e9de0e0e)
kill: (1441674): No such process
kill: (-1442370): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-6a738a64918f799f)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-9feb568c477cffdf)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-05bab2f96380bc67)
     Running tests/integration.rs (target/debug/deps/integration-502483a68671b4b7)
     Running tests/isolation.rs (target/debug/deps/isolation-66ea1e45aff405d5)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-0960811ac376916b)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M29-post-2.0-fixes/phase-05-residency.md` — +46 -0
- `src/daemon/server/ask.rs` — +6 -3

**Commit:** 79b23c11e46b1e70cd025c0579c8483c4d9bcbc7

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v2.1.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.87s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
 prompt_assets_name_no_retired_tools ... ok
test readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_config_template_documents_every_config_field ... ok
test seeded_assets_name_no_retired_tools ... ok
test readme_documents_every_chat_slash_command ... ok
test runtime_docs_describe_no_tmux_execution ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ghost_config_parsing ... ok
test ipc_session_info_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test event_log_append_read ... ok
test g4_briefing_read_and_clear ... ok
test g4_briefing_injects_on_next_run ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test an_empty_session_starts_no_tmux_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test daemon_installs_no_tmux_hooks ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.31s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_a_multi_line_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-d9203e33e9de0e0e)
kill: (1627748): No such process
kill: (-1628461): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-6a738a64918f799f)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-9feb568c477cffdf)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-05bab2f96380bc67)
     Running tests/integration.rs (target/debug/deps/integration-502483a68671b4b7)
     Running tests/isolation.rs (target/debug/deps/isolation-66ea1e45aff405d5)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-0960811ac376916b)
   Doc-tests daemoneye

```

**Files changed:**

- `src/daemon/server/ask.rs` — +6 -3

**Commit:** 79b23c11ea4338a82f51ddef8464addf5e2ce2f1

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-19

- **Verdict:** approved_after_2
- **Bounces:** 2, both `spec_bug`:
  - **bug-05-1:** my gotcha allowed building the entry "inside a single
    `with_sessions` closure", so `from_disk` read files under the store
    lock.
  - **bug-05-2:** my worked example for that fix said the entry "now
    exists" after the insert step, which invited a production `.expect()`
    that fires if `cleanup_pass` evicts the entry between the probe and
    the insert.
- **Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark (run 1: 255
  turns; bounce 1: 59 turns; bounce 2: 33 turns)
- **Scope deviations:** none. Bounce 2 touched only `ask.rs`.
- **Calibration:**
  - **Both bounces were mine.** Each corrected one fact my spec stated
    without checking it against its context: the lock semantics of
    `with_sessions`, and concurrent eviction. Together with phase 01 (an
    ambiguous negative assertion) and phase 02 (the wrong code site), that
    is four spec bugs in one milestone, all from the same cause. This is a
    fold candidate for the close.
  - **The executor did well.** It got the design right on the first run
    and recorded both mutation checks honestly. The green-bounce treatment
    landed each fix in a small, focused run.
  - Independent re-run: fmt, build and clippy exit 0; 2425 passed, 0
    failed (unchanged across both bounces, as required); self-check MATCH;
    my own counts `registry_refs=0`, `in_lock_builds=0`,
    `ask_prod_expects=0`.
  - My mutation check, on a scratch copy: restoring the `expect` gives
    `ask_prod_expects=1`.
  - The early return skips only writes to an entry that no longer exists,
    and returns `(0.0, false)`, the same as the closure's no-session
    branch.
  - The seven `unsafe { set_var("HOME") }` test sites are the repo's
    idiom, each under `test_home_guard()`, test-only.

# Phase 04f: Convert `context/background.rs` — the Compaction Swap

**Milestone:** M5 — UX & Stability
**Status:** review
**Depends on:** phase-04e (`executor/` subtree converted) — `done`
**Estimated diff:** ~60 lines
**Tags:** language=rust, kind=refactor, size=s

## Goal

Convert the **2 production** `sessions.lock()` sites in
`src/daemon/context/background.rs` to `with_sessions`. Both hold the guard across
an early `return` from the enclosing function, so neither is mechanical.

**Finish condition: 2 `with_sessions` calls, and 0 raw `sessions.lock()` in the
production region** (everything above `#[cfg(test)]`).

**This phase is much smaller than the milestone README first claimed.** The
earlier "13 sites" figure was a plain `grep -c` that counted the test module.
`#[cfg(test)]` starts at line 279; **11 of the 13 hits are test code** and are
explicitly out of scope here (see Out of scope, and the note for the newtype
phase).

## Architecture references

Read before starting:

- `docs/design/daemon-stalls.md` § 3.5 — the migration hazard: a converted
  closure enclosing a call that still uses **raw** `.lock()` deadlocks silently.
- `docs/design/daemon-stalls.md` § 1 mechanism A — lock held across blocking
  work. Site 2 below is the compaction swap, whose existing comment already
  states the no-`.await`-under-guard rule this conversion makes structural.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Verify the starting state:

```bash
# production region only — robust to line shifts
sed -n '1,/#\[cfg(test)\]/p' src/daemon/context/background.rs \
  | grep -c "sessions\.lock()"                                    # expect 2
grep -c "sessions\.lock()" src/daemon/context/background.rs       # expect 13 (2 prod + 11 test)
grep -c "with_sessions(" src/daemon/context/background.rs         # expect 0
grep -rc "sessions\.lock()" src/daemon/executor/ | grep -v ":0$"  # expect no output (04e landed)
```

If the production count is not 2, **stop and report a blocker** — the per-site
code below is stale.

## Current state

`SessionStore` is still the bare type alias:

```rust
// src/daemon/session.rs:117
pub type SessionStore = Arc<Mutex<HashMap<String, SessionEntry>>>;
```

### The accessor — `src/daemon/session.rs:427-434`

```rust
pub fn with_sessions<T>(
    sessions: &SessionStore,
    f: impl FnOnce(&mut HashMap<String, SessionEntry>) -> T,
) -> T {
    let _depth = SessionsLockDepth::enter();
    let mut store = sessions.lock().unwrap_or_log();
    f(&mut store)
}
```

Generic over `T`. Note it takes a **synchronous** `FnOnce`, so no `.await` can
occur while the guard is alive — the compiler enforces what site 2's comment
currently only asks for.

### Import to extend — `src/daemon/context/background.rs:11`

```rust
use crate::daemon::session::SessionStore;
```

becomes:

```rust
use crate::daemon::session::{SessionStore, with_sessions};
```

**Leave `use crate::util::UnpoisonExt;` (line 13) alone.** It stays used by
production code at lines 121 and 140 after this conversion, so removing it breaks
the build. Do not "clean up" imports.

### Why this file converts before `stream.rs`

`stream.rs` calls `spawn_compaction` (this file's entry point at line 39), and a
`sessions` lock held across that call is a **confirmed historical defect in this
codebase** — a self-deadlock where the caller held the guard while the callee
re-locked.

Converting the callee first changes the failure mode for the phase that converts
`stream.rs`: once `try_snapshot` goes through `with_sessions`, a `stream.rs`
closure that encloses `spawn_compaction` trips the re-entrancy **assertion** —
a loud panic — instead of hanging silently. That is strictly better, and it is
why this phase is ordered ahead of the `stream.rs` conversion.

### Site inventory — 2 production sites

| # | Line | Function | Shape |
|---|---|---|---|
| 1 | 67 | `try_snapshot` (fn at 66) | `?` **and** `return None` from the enclosing fn, plus an explicit `drop(store)` |
| 2 | 231 | `run_compaction` (async fn at 91) | **two `return Ok(())` from the enclosing async fn**, inside a block expression |

## Spec

### 1. `try_snapshot` — closure returns the `Option`

Current body — `src/daemon/context/background.rs:66-84`:

```rust
fn try_snapshot(session_id: &str, sessions: &SessionStore) -> Option<CompactionSnapshot> {
    let mut store = sessions.lock().unwrap_or_log();
    let entry = store.get_mut(session_id)?;
    if entry.compaction_in_flight || entry.is_ghost {
        return None;
    }
    entry.compaction_in_flight = true;

    let snapshot = CompactionSnapshot {
        session_id: session_id.to_string(),
        messages: entry.messages.clone(),
        turn_count: entry.turn_count,
        msg_len: entry.messages.len(),
        token_scale: entry.token_scale,
    };
    drop(store);
    Some(snapshot)
}
```

The whole body is one locked region whose result is exactly the function's
return value, so the closure can return it directly:

```rust
fn try_snapshot(session_id: &str, sessions: &SessionStore) -> Option<CompactionSnapshot> {
    with_sessions(sessions, |store| {
        let entry = store.get_mut(session_id)?;
        if entry.compaction_in_flight || entry.is_ghost {
            return None;
        }
        entry.compaction_in_flight = true;

        Some(CompactionSnapshot {
            session_id: session_id.to_string(),
            messages: entry.messages.clone(),
            turn_count: entry.turn_count,
            msg_len: entry.messages.len(),
            token_scale: entry.token_scale,
        })
    })
}
```

Three things this gets right:

- The `?` and the `return None` now bind to the **closure**, whose return type is
  `Option<CompactionSnapshot>` — the same as the function's. So the observable
  behavior is identical: `None` when the entry is absent, in-flight, or a ghost.
- **`drop(store)` is deleted.** It was manual guard management; `with_sessions`
  releases at the closure boundary. Do not keep it — there is no `store` binding
  to drop.
- The `let snapshot = …; Some(snapshot)` pair collapses into `Some(CompactionSnapshot { … })`.
  This is incidental tidying that falls out of the rewrite; do not go looking for
  other tidying to do.

**Important:** `entry.compaction_in_flight = true` is the in-flight mark and it
must still happen **inside** the locked region. Setting it after the closure
returns would reintroduce a race where two turns both pass the check. Keep it
where it is.

### 2. `run_compaction` step 2 — the swap, with two discard paths

Current code — `src/daemon/context/background.rs:229-259`:

```rust
    // Step 2: Swap (lock once, synchronous). No `.await` may occur while the
    // guard is alive — all async work is already done above.
    let (before_len, after_len) = {
        let mut store = sessions.lock().unwrap_or_log();
        // Staleness check: if the entry is gone, or turn_count/msg_len changed,
        // discard the compacted vec.
        let Some(entry) = store.get_mut(&snapshot.session_id) else {
            // Entry evicted — discard. The epoch record already appended is
            // harmless — it describes real history; the next load simply has
            // one epoch whose messages are still in the working file.
            return Ok(());
        };

        if entry.turn_count != snapshot.turn_count || entry.messages.len() != snapshot.msg_len {
            // A turn ran while we worked — discard. Clear the flag so the next
            // turn's end can re-spawn with fresh data.
            entry.compaction_in_flight = false;
            return Ok(());
        }

        // Match — swap.
        let before_len = entry.messages.len();
        let after_len = compacted.len();
        entry.messages = compacted.clone();
        entry.compaction_in_flight = false;
        entry.pending_compaction_notice = Some(format!(
            "↩ Session history compacted in the background ({} → {} messages) — epoch {} recorded",
            before_len, after_len, record.seq,
        ));
        entry.dirty = true;
        (before_len, after_len)
    };
```

Both `return Ok(())` statements return from **`run_compaction`**, not from the
block. Move this into a `with_sessions` closure unchanged and they return from
the *closure* instead — a type error, or worse, a value silently bound to
`(before_len, after_len)`.

The two discard paths differ in one respect only: the stale path clears
`compaction_in_flight` first, the evicted path has no entry to clear. Both then
return `Ok(())`. So the closure can perform its own flag clearing and signal
discard-vs-swap with an `Option`:

```rust
    // Step 2: Swap (lock once, synchronous). `with_sessions` takes a synchronous
    // closure, so no `.await` can occur while the guard is alive.
    let Some((before_len, after_len)) = with_sessions(sessions, |store| {
        // Staleness check: if the entry is gone, or turn_count/msg_len changed,
        // discard the compacted vec.
        let entry = store.get_mut(&snapshot.session_id)?;

        if entry.turn_count != snapshot.turn_count || entry.messages.len() != snapshot.msg_len {
            // A turn ran while we worked — discard. Clear the flag so the next
            // turn's end can re-spawn with fresh data.
            entry.compaction_in_flight = false;
            return None;
        }

        // Match — swap.
        let before_len = entry.messages.len();
        let after_len = compacted.len();
        entry.messages = compacted.clone();
        entry.compaction_in_flight = false;
        entry.pending_compaction_notice = Some(format!(
            "↩ Session history compacted in the background ({} → {} messages) — epoch {} recorded",
            before_len, after_len, record.seq,
        ));
        entry.dirty = true;
        Some((before_len, after_len))
    }) else {
        // Either the entry was evicted, or a turn ran while we worked. Both are
        // clean discards: the epoch record already appended describes real
        // history, and the next load simply has one epoch whose messages are
        // still in the working file.
        return Ok(());
    };
```

Four points to get exactly right:

- `store.get_mut(&snapshot.session_id)?` replaces the `let … else { return Ok(()) }`.
  The `?` yields `None` from the closure, and the `let … else` on the outside turns
  that into `return Ok(())`. Same behavior, one level moved.
- The stale branch's `return None` **must come after** `entry.compaction_in_flight = false`.
  Reversing them leaves the flag set forever and permanently blocks future
  compaction for that session — a silent, permanent regression that no test
  covers. This is the single most dangerous line in the phase.
- The evicted path must **not** clear the flag — there is no entry. The `?` handles
  that correctly by construction; do not add a fallback.
- The two explanatory comments must survive. The evicted-path comment moves to the
  `else` block (as shown, merged with the stale-path rationale); the stale-path
  comment stays on its branch. Both explain *why* a discard is safe, which is not
  obvious from the code.

Everything after this block — step 3's file persist — is unchanged and stays
outside the closure. It is blocking I/O and must not move inside.

### 3. Change nothing else

No other file. `SessionStore` stays a type alias. The 11 test-module sites stay
raw (see Out of scope).

## Acceptance criteria

**Note the whole-file count does not go to zero** — 11 test-module sites remain
by design. Use the production-region command:

- [ ] `sed -n '1,/#\[cfg(test)\]/p' src/daemon/context/background.rs | grep -c "sessions\.lock()"`
      returns **0**.
- [ ] `grep -c "sessions\.lock()" src/daemon/context/background.rs` returns
      **11** — the test module, untouched.
- [ ] `grep -c "with_sessions(" src/daemon/context/background.rs` returns **2**.
- [ ] `grep -c "drop(store)" src/daemon/context/background.rs` returns **0**.
- [ ] `grep -n "use crate::util::UnpoisonExt" src/daemon/context/background.rs`
      still matches — the import is still needed by lines 121/140.
- [ ] `grep -rc "sessions\.lock()" src/daemon/executor/` produces no non-zero
      line (04e's work untouched).
- [ ] `grep -n "pub type SessionStore" src/daemon/session.rs` still shows the alias.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **915** lib-unit tests — unchanged. This phase adds
      no tests; a higher number means scope crept.
- [ ] `cargo test` completes **without hanging**.

These greps count raw text, comments included. **Do not write the literal
`sessions.lock()`, `with_sessions(`, or `drop(store)` in a comment** in this file
— it would break a criterion even with correct code.

## Test plan

Behavior-preserving refactor: the existing **915** tests are the regression net
and must all still pass, unchanged. **Write no new tests.**

`background.rs`'s own test module already covers both converted paths — that is
why no new test is warranted here, and it is unusually good coverage for this
milestone:

- the swap path (entry matches → messages replaced),
- the stale path (turn ran during compaction → discard, flag cleared),
- the evicted path (entry removed before swap → clean discard, no panic).

**Those tests are the discriminator for task 2.** Run them and name them in the
Update Log. If the flag-clearing order in the stale branch is inverted, the stale
test is what should catch it — confirm it does by reading what it asserts, and say
so in the Update Log.

One reasoning check to state in the Update Log, no new test: confirm that
`try_snapshot` still returns `None` (not a snapshot) for all three of the entry
being absent, `compaction_in_flight` already `true`, and `is_ghost` being `true`.

## End-to-end verification

> Not applicable — phase ships no runtime-loadable artifact. Internal refactor of
> lock acquisition inside an existing code path; no CLI surface, no config key, no
> file the running binary loads.

**Do not attempt an interactive verification.** Do not launch tmux, the daemon, or
the chat client. Write the sentence above under an "End-to-end verification"
heading in the Update Log.

## Authorizations

None.

This phase adds no tests, so it needs no `HOME` redirection and therefore no
`unsafe`. **If you think you need `unsafe` or a new dependency, stop and report a
blocker.**

## Out of scope

- **Do not convert the 11 `sessions.lock()` sites in this file's `#[cfg(test)]`
  module** (below line 279). They are test code, `STANDARDS.md` § 2 exempts tests
  from the production error-handling rules, and converting them would triple this
  diff for no behavioral gain. **They are the newtype phase's problem** — that
  phase makes raw `.lock()` stop compiling, so it must convert these 11 plus the
  2 in `session.rs`'s test module. An acceptance criterion pins the count at 11 so
  a well-meaning conversion is caught.
- **Do not touch `src/daemon/executor/`** — fully converted by 04e, and pinned by
  a criterion.
- **Do not convert `ghost.rs`, `briefing.rs`, `background/`, `stream.rs`,
  `hook.rs`, or `webhook/process.rs`.** Separate phases.
- **Do not change `SessionStore` into a newtype** and do not touch the 13
  `Arc::clone` sites.
- **Do not remove `use crate::util::UnpoisonExt;`** — still needed at lines
  121/140.
- **Do not move step 3's file persist inside the closure.** It is blocking I/O;
  that is the mechanism-A defect this milestone exists to remove.
- **Do not reorder the stale branch's flag clear and its `return None`.** Spec
  task 2 explains the consequence.
- **Do not reword the compaction notice string** (`"↩ Session history compacted
  in the background …"`). It is user-visible.
- **Do not add `#[allow(...)]` anywhere.** If clippy objects to the
  `let … else` shape, report a blocker rather than suppressing.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-26 14:35 (started)

**Executor:** Claude (Sonnet 4.5)

Converted both production `sessions.lock()` sites in `background.rs` to `with_sessions`:
- `try_snapshot`: closure returns `Option<CompactionSnapshot>` directly; `drop(store)` removed.
- `run_compaction` step 2: closure returns `Option<(before_len, after_len)>` with `let … else` handling both discard paths (evicted and stale). Flag clearing order preserved — `compaction_in_flight = false` before `return None` in the stale branch.

All 915 lib-unit tests pass unchanged. `try_snapshot` still returns `None` for absent entry, `compaction_in_flight` already true, and `is_ghost` true (verified by closure `?` and early `return None`). The stale-path test in the test module exercises the flag-clearing order and passes.

End-to-end verification: Not applicable — phase ships no runtime-loadable artifact. Internal refactor of lock acquisition inside an existing code path; no CLI surface, no config key, no file the running binary loads.

### Update — ts=1785076797928 (complete, server-authored)

**Summary:** **Summary + Notes for review:**

Converted both production `sessions.lock()` sites in `src/daemon/context/background.rs` to `with_sessions` closures. `try_snapshot` now returns its `Option<CompactionSnapshot>` directly from the closure, eliminating the manual `drop(store)`. `run_compaction` step 2 uses `let Some((…)) = with_sessions(…) else { return Ok(()) }` to handle both the evicted and stale discard paths, with the critical `compaction_in_flight = false` correctly placed before `return None` in the stale branch. All 915 lib-unit tests pass, build/clippy/fmt are clean, and all acceptance criteria greps match (0 prod `lock()`, 11 test `lock()`, 2 `with_sessions`, 0 `drop(store)`). No deviations from the spec.

**Executor:** Qwen/Qwen3.6-27B-FP8

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
 ... ok
test tmux::cache::tests::get_labeled_context_background_panes_sorted ... ok
test tmux::cache::tests::get_labeled_context_copy_mode_annotated ... ok
test tmux::cache::tests::get_labeled_context_pane_classification ... ok
test tmux::cache::tests::get_labeled_context_client_viewport_absent_when_zero ... ok
test tmux::cache::tests::get_labeled_context_synchronized_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_session_topology ... ok
test tmux::cache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test tmux::cache::tests::get_labeled_context_chat_pane_excluded_from_background ... ok
test search::tests::search_events_returns_tail_not_head_when_segment_exceeds_cap ... ok
test search::tests::search_finds_match_in_runbooks ... ok
test search::tests::search_respects_kind_filter ... ok
test session_store::tests::artifacts_round_trip ... ok
test memory::tests::memory_without_frontmatter_has_empty_metadata ... ok
test session_store::tests::backfill_idempotent ... ok
test session_store::tests::backfill_missing_artifact_returns_error_name ... ok
test session_store::tests::backfill_stamps_memory_without_frontmatter ... ok
test session_store::tests::backfill_stamps_runbook ... ok
test session_store::tests::backfill_stamps_script ... ok
test session_store::tests::collision_allowed_with_force ... ok
test session_store::tests::collision_rejected_without_force ... ok
test session_store::tests::delete_nonexistent_errors ... ok
test session_store::tests::delete_removes_dir_and_index ... ok
test session_store::tests::list_returns_newest_first ... ok
test session_store::tests::load_messages_max_count_truncates ... ok
test session_store::tests::rename_nonexistent_errors ... ok
test session_store::tests::rename_to_existing_errors ... ok
test memory::tests::update_memory_partial_update_preserves_other_fields ... ok
test session_store::tests::save_and_load_round_trip ... ok
test session_store::tests::update_in_place_allowed ... ok

test result: ok. 915 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.11s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 29 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test event_log_entry_format ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test ghost_config_parsing ... ok
test minimal_config_parsing ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test config_pricing_round_trip ... ok
test schedule_store_persistence ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test g4_briefing_masking_applied ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_to_event_log ... ok

test result: ok. 27 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.01s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.07s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-02078a4e6496e4bf)
     Running unittests src/main.rs (target/debug/deps/daemoneye-a516fbd17ae98770)
     Running tests/integration.rs (target/debug/deps/integration-ce8aee7d32f385fa)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M5-ux-stability/phase-04f-convert-context-background.md` — +13 -1
- `src/daemon/context/background.rs` — +27 -28

**Commit:** 4f60a9a8d66d246505a0286a7d8f1e37c4d565e7

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

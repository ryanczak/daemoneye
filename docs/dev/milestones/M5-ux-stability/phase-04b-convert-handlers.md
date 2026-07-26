# Phase 04b: Convert `handlers.rs` Lock Sites

**Milestone:** M5 — UX & Stability
**Status:** done
**Depends on:** phase-04a (`with_sessions` accessor) — `done`
**Estimated diff:** ~160 lines
**Tags:** language=rust, kind=refactor, size=m

## Goal

Convert all 15 `sessions.lock()` sites in `src/daemon/server/handlers.rs` to the
`with_sessions` accessor introduced in phase 04a, and add the fast-failing depth
test that phase's review carried forward. Pure mechanical conversion — no
behavior change.

## Architecture references

Read before starting:

- `docs/design/daemon-stalls.md` § 3.4 — the four-phase ordering and why
  `SessionStore` is still a plain type alias at this point.
- `docs/design/daemon-stalls.md` § 1.5c — the deadlock the accessor guards
  against.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Phase 04a added the accessor. It is already in scope in `handlers.rs` — that
file opens with `use crate::daemon::session::*;` (line 4), a glob import, so
**no import changes are needed**.

```rust
// src/daemon/session.rs — added by phase 04a
pub fn with_sessions<T>(
    sessions: &SessionStore,
    f: impl FnOnce(&mut HashMap<String, SessionEntry>) -> T,
) -> T {
    let _depth = SessionsLockDepth::enter();
    let mut store = sessions.lock().unwrap_or_log();
    f(&mut store)
}
```

`handlers.rs` has **15** sites, at lines 57, 93, 128, 166, 173, 232, 298, 386,
424, 470, 497, 535, 562, 601, 698. They fall into three shapes, all mechanical.

### Shape 1 — `if let Ok(store) = … && let Some(entry) = …` (the common case)

```rust
// src/daemon/server/handlers.rs:57
        if let Ok(mut store) = sessions.lock()
            && let Some(entry) = store.get_mut(&session_id)
        {
            entry.active_model = Some(model_name.clone());
        }
```

becomes

```rust
        with_sessions(sessions, |store| {
            if let Some(entry) = store.get_mut(&session_id) {
                entry.active_model = Some(model_name.clone());
            }
        });
```

### Shape 2 — the lock result feeds an expression binding

```rust
// src/daemon/server/handlers.rs:166
    let current_target = if let Ok(store) = sessions.lock() {
        store
            .get(&session_id)
            .and_then(|e| e.default_target_pane.clone())
    } else {
        None
    };
    let chat_pane_id: Option<String> = if let Ok(store) = sessions.lock() {
        store.get(&session_id).and_then(|e| e.chat_pane.clone())
    } else {
        None
    };
```

becomes — and note these two adjacent sites **collapse into one acquisition**,
which is the point of the accessor:

```rust
    let (current_target, chat_pane_id) = with_sessions(sessions, |store| {
        let entry = store.get(&session_id);
        (
            entry.and_then(|e| e.default_target_pane.clone()),
            entry.and_then(|e| e.chat_pane.clone()),
        )
    });
```

Collapsing adjacent acquisitions like this is **encouraged where the sites are
immediately adjacent and independent**, as at 166/173. Do **not** merge sites
separated by other logic just to reduce the count.

### Shape 3 — the body assigns to outer variables

```rust
// src/daemon/server/handlers.rs:232
    if let Ok(sess_map) = sessions.lock() {
        active_sessions = sess_map.len();
        active_prompt_tokens = sess_map.values().map(|s| s.last_prompt_tokens).sum();
        total_turns = sess_map.values().map(|s| s.turn_count).sum();
        …
    }
```

Prefer returning a tuple over assigning through the closure's captured
environment — it reads better and avoids borrow surprises:

```rust
    let (active, prompt_tokens, turns, model) = with_sessions(sessions, |store| {
        (
            store.len(),
            store.values().map(|s| s.last_prompt_tokens).sum::<u32>(),
            store.values().map(|s| s.turn_count).sum::<usize>(),
            store
                .values()
                .filter(|s| !s.is_ghost)
                .max_by_key(|s| s.last_accessed)
                .and_then(|s| s.active_model.clone()),
        )
    });
```

Assigning to captured outer variables inside the closure also compiles and is
acceptable if the tuple gets unwieldy. Either is fine; the field types above are
illustrative — take the real ones from the code.

### The `else { None }` branches disappear

Every `if let Ok(...) = sessions.lock()` has an implicit "what if the lock is
poisoned" branch — usually `else { None }` or a silently skipped block.
`with_sessions` uses `.unwrap_or_log()` internally, which **recovers** from
poison and logs an ERROR rather than skipping the work.

This is a deliberate behavior change and it is the one the project already
mandates: `CLAUDE.md` § "Important Invariants" says every lock site must use
`.unwrap_or_log()`. These `if let Ok(…)` sites were the stragglers. After a
poison event they now do their work instead of silently doing nothing.

## Spec

### 1. Convert all 15 sites in `src/daemon/server/handlers.rs`

Work through lines 57, 93, 128, 166, 173, 232, 298, 386, 424, 470, 497, 535,
562, 601, 698, converting each per the shapes above. Collapse 166/173 into one
acquisition as shown.

Rules that apply to every site:

- **The closure body must not call anything that reaches the session store.**
  If a site currently calls a helper while holding the guard, check that helper
  first. The re-entrancy assertion will panic at runtime if you nest, and the
  test suite will catch it — but it is cheaper to notice now.
- **No blocking work inside the closure** — no file I/O, no
  `std::process::Command`, no `send_response_split(...).await`. If a site does
  that today, collect what you need inside the closure and act after it returns,
  the same shape phase 04a used for the shutdown sweep:

  ```rust
  let pipe_panes: Vec<String> = with_sessions(&sessions, |store| { … collect … });
  for pane_id in &pipe_panes { crate::tmux::stop_pipe_pane(pane_id); }
  ```

- **`with_sessions` is synchronous.** No `.await` can appear inside the closure.
  If a site holds the lock across an await today it will not compile — that
  cannot happen here (`clippy::await_holding_lock` is clean), but if you hit it,
  restructure to collect-then-act rather than reaching for a workaround.

### 2. Add the fast-failing depth test — `src/daemon/session.rs`

Carried from the phase-04a review. The existing
`with_sessions_rejects_reentrant_call` catches a broken depth guard by
**deadlocking**, which stalls CI instead of failing it. Add a companion that
fails instantly:

```rust
    #[test]
    fn with_sessions_sets_depth_inside_closure() {
        let sessions: SessionStore = Arc::new(Mutex::new(HashMap::new()));
        with_sessions(&sessions, |_store| {
            assert_eq!(
                SESSIONS_LOCK_DEPTH.with(|d| d.get()),
                1,
                "depth must read 1 inside the closure — a `let _ =` binding on \
                 SessionsLockDepth::enter() would drop the guard immediately and \
                 read 0 here"
            );
        });
        assert_eq!(
            SESSIONS_LOCK_DEPTH.with(|d| d.get()),
            0,
            "depth must reset to 0 after the closure returns"
        );
    }
```

`SESSIONS_LOCK_DEPTH` is a private `thread_local!` in `session.rs`; the test
module is inside the same file, so it is reachable via `super::` (the test module
already does `use super::*;`).

### 3. Change nothing else

`SessionStore` stays `pub type SessionStore = Arc<Mutex<…>>`. Do not convert
`ask.rs` — its sites use `sessions.lock().ok()?` chains whose `?` semantics need
per-site attention, and they are phase 04c.

## Acceptance criteria

- [ ] `cargo fmt --all` clean; `cargo build` succeeds.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits zero.
- [ ] `cargo test` green.
- [ ] `grep -c "sessions.lock()" src/daemon/server/handlers.rs` returns **0**.
- [ ] `grep -c "sessions.lock()" src/daemon/server/ask.rs` returns **13** —
      unchanged, proving `ask.rs` was left alone.
- [ ] `grep -n "pub type SessionStore" src/daemon/session.rs` still shows the
      `Arc<Mutex<…>>` alias.
- [ ] Test `with_sessions_sets_depth_inside_closure` passes.
- [ ] `cargo test --lib` reports **914** — 913 now, plus exactly the one new
      test. The conversions add no tests; a higher count means scope crept.

## Test plan

This is a behavior-preserving refactor, so the existing suite is the primary
test: all 913 current tests must still pass, unchanged. Do **not** write new
tests for the converted call sites — they are covered by whatever already covers
those handlers, and adding per-site tests would inflate the count past the pinned
914.

- `with_sessions_sets_depth_inside_closure` in `src/daemon/session.rs` — per
  spec 2. Asserts depth is 1 *inside* the closure and 0 after. This is the
  fast-failing counterpart to `with_sessions_rejects_reentrant_call`.

  Sanity-check its power before reporting complete: temporarily change
  `let _depth = SessionsLockDepth::enter();` to `let _ = …` in `with_sessions`,
  confirm this new test **fails immediately** (rather than hanging, which is what
  the older re-entrancy test does under that mutation), then revert. State the
  result in the Update Log.

## End-to-end verification

**Do not attempt an interactive verification.** Do not launch tmux, the daemon,
or the chat client.

Write this under an "End-to-end verification" heading in the Update Log:

> Not applicable — behavior-preserving refactor of internal lock acquisition.
> The existing suite is the regression net. The architect confirms the daemon
> still starts, serves a request, and shuts down cleanly.

## Authorizations

None. No new dependencies; `docs/architecture.md` is not touched.

## Out of scope

- **Do not** convert `src/daemon/server/ask.rs`. Phase 04c. Its
  `sessions.lock().ok()?` chains change `?` propagation when wrapped in a
  closure and need per-site reasoning.
- **Do not** convert `background.rs`, `ghost.rs`, `executor/mod.rs`,
  `stream.rs`, or any other file. Later phases.
- **Do not** convert `SessionStore` to a newtype — phase 04d.
- **Do not** change any handler's observable behavior: same responses, same
  ordering, same log lines. The only intended behavior change is the poison
  path described under "The `else { None }` branches disappear".
- **Do not** add tests for individual converted handlers (see Test plan).
- **Do not** touch `bg_sn.lock()` or any other non-`SessionStore` mutex.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-26 01:04 (progress)

Converted all 15 `sessions.lock()` sites in `handlers.rs` to `with_sessions`, collapsing the two adjacent acquisitions at lines 166/173 into one. Added the fast-failing depth test `with_sessions_sets_depth_inside_closure` to `session.rs`. Verified the test catches the `let _ =` bug instantly (0ms failure vs. hang). All 914 tests pass. `ask.rs` untouched at 13 lock sites. `SessionStore` remains a plain type alias.

### Update — ts=1785028579447 (complete, server-authored)

**Summary:** **Summary:** Converted all 15 `sessions.lock()` sites in `handlers.rs` to the `with_sessions` accessor from phase 04a, collapsing the two adjacent acquisitions at lines 166/173 into one call as encouraged by the spec. Added the fast-failing depth test `with_sessions_sets_depth_inside_closure` to `session.rs` — verified it catches a `let _ =` dropped guard binding instantly (0ms failure) rather than hanging like the older re-entrancy test. All 914 tests pass (`cargo test --lib`), clippy is clean, and `ask.rs` remains untouched at 13 lock sites. No deviations from the spec.

**Executor:** Qwen/Qwen3.6-27B-FP8

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.42s


LINT
    Checking daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.45s


TEST
x::cache::tests::get_labeled_context_copy_mode_annotated ... ok
test tmux::cache::tests::get_labeled_context_pane_classification ... ok
test tmux::cache::tests::get_labeled_context_dead_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_client_viewport_shown_when_known ... ok
test tmux::cache::tests::get_labeled_context_client_viewport_absent_when_zero ... ok
test tmux::cache::tests::get_labeled_context_session_topology ... ok
test tmux::cache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test tmux::cache::tests::get_labeled_context_chat_pane_excluded_from_background ... ok
test search::tests::search_events_returns_tail_not_head_when_segment_exceeds_cap ... ok
test search::tests::search_finds_match_in_runbooks ... ok
test search::tests::search_respects_kind_filter ... ok
test session_store::tests::artifacts_round_trip ... ok
test search::tests::search_returns_empty_for_no_match ... ok
test session_store::tests::backfill_idempotent ... ok
test memory::tests::session_memory_block_respects_cap ... ok
test session_store::tests::backfill_stamps_memory_without_frontmatter ... ok
test session_store::tests::backfill_stamps_runbook ... ok
test session_store::tests::backfill_stamps_script ... ok
test session_store::tests::collision_allowed_with_force ... ok
test session_store::tests::collision_rejected_without_force ... ok
test memory::tests::update_memory_creates_new_entry ... ok
test session_store::tests::delete_removes_dir_and_index ... ok
test session_store::tests::list_returns_newest_first ... ok
test session_store::tests::load_messages_max_count_truncates ... ok
test session_store::tests::rename_nonexistent_errors ... ok
test session_store::tests::rename_to_existing_errors ... ok
test session_store::tests::rename_updates_dir_and_index ... ok
test memory::tests::update_memory_sets_updated_timestamp ... ok
test session_store::tests::update_in_place_allowed ... ok

test result: ok. 914 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.19s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 29 tests
test daemon_ping_status_loop ... ignored
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_session_info_round_trip ... ok
test ipc_ask_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test ipc_tool_call_response_round_trip ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test minimal_config_parsing ... ok
test ghost_config_parsing ... ok
test schedule_store_persistence ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g4_briefing_read_and_clear ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test g4_briefing_injects_on_next_run ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_to_event_log ... ok
test g5_mailbox_write_and_read ... ok

test result: ok. 27 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.01s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Compiling daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 1.51s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-02078a4e6496e4bf)
     Running unittests src/main.rs (target/debug/deps/daemoneye-a516fbd17ae98770)
     Running tests/integration.rs (target/debug/deps/integration-ce8aee7d32f385fa)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M5-ux-stability/README.md` — +1 -1
- `docs/dev/milestones/M5-ux-stability/phase-04b-convert-handlers.md` — +5 -1
- `src/daemon/server/handlers.rs` — +119 -126
- `src/daemon/session.rs` — +19 -0

**Commit:** e182ef8c92ba204d892315dd507837d1a75b125b

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### End-to-end verification — 2026-07-25 (architect-performed)

`handlers.rs` **is** the IPC dispatch layer, so a real client request exercises
the converted code directly — this is stronger than the "not applicable" the
phase doc allowed for.

Started `./target/release/daemoneye daemon --console`, then ran
`./target/release/daemoneye status`, which routes through the converted
`handle_status` (the line-232 aggregation). With no sessions:

```
── SESSION ───────────────────────────────────────────────
  active          0
  turns           0  ·  none
  context         0 / 131071 tokens  0%
```

That alone proves nothing — a conversion that always returned defaults would
print the same thing. So a real chat session was opened and the query repeated:

```
── SESSION ───────────────────────────────────────────────
  active          1
  turns           1  ·  none
  context         0 / 131071 tokens  0%
```

The converted aggregation reads the live store. Daemon stopped cleanly
afterwards; socket removed; tree clean.

Harness note: an earlier phase's test daemon was still running and the two
instances fought over webhook port 9393, producing
`Supervised task 'webhook' exited unexpectedly — restarting … (attempt 7)` in the
log. That is a leftover from architect E2E runs, **not** a product defect and not
introduced by this phase. Both were stopped. Check for stray daemons before
reading a webhook restart loop as a regression.

### Review verdict — 2026-07-25

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** Qwen/Qwen3.6-27B-FP8 (95 turns)
- **Gates (reviewer re-run):** `cargo fmt --all --check` clean; `cargo build`
  clean; `cargo clippy --all-targets --all-features -- -D warnings` exits zero;
  `cargo test` 914 lib + 27 integration, 0 failed. Exactly the pinned 914.
- **Acceptance criteria:** all verified independently.
  `grep -c "sessions.lock()" src/daemon/server/handlers.rs` → **0**.
  `ask.rs` → **13**, unchanged, so scope held against the adjacent file.
  `SessionStore` is still the `Arc<Mutex<…>>` alias. The diff touches only
  `handlers.rs`, `session.rs`, and the two docs.
- **The 166/173 collapse landed.** `handlers.rs` has **14** `with_sessions`
  calls for 15 former sites — the two adjacent acquisitions became one, which is
  the accessor earning its keep rather than a mechanical 1:1 substitution.
- **Mutation check (reviewer-run, not trusted):** with
  `let _depth` → `let _` in `with_sessions`, the new
  `with_sessions_sets_depth_inside_closure` fails in **0.00s** with
  `left: 0, right: 1` and the explanatory message. Under the same mutation the
  older `with_sessions_rejects_reentrant_call` **hangs**. The 04a follow-up is
  properly closed: this file now fails fast on a broken guard.
- **`unsafe` audit:** `handlers.rs:28` contains an `unsafe` block, but it is
  pre-existing (the `libc::kill` self-signal for graceful stop, with a SAFETY
  comment). `git diff HEAD~2 HEAD` shows **zero** added `unsafe` lines.
- **Behavior preservation spot-checked** on the two most restructured sites:
  `is_dirty` (`if let` chain → `is_some_and`) and `active_agents` (push-loop →
  `filter_map` + `collect`, still sorted afterwards). Both equivalent.
- **End-to-end:** converted dispatch path exercised against the real binary with
  a live session — see the preceding entry.
- **Scope deviations:** none.
- **Calibration:** none for the executor. Fourth consecutive
  `approved_first_try`, and the first at `size=m`.

#### Throughput data for sizing phase 04d

This phase took **95 turns** for 15 mechanical conversions plus one test. The
three preceding `size=s` phases took 46, 50, and 70. Roughly **6 turns per
converted site**, plus fixed overhead for orientation and the gate loop.

Phase 04d (the tail: `background.rs`, `ghost.rs`, `executor/mod.rs`,
`stream.rs`) is ~60 sites. At this rate that is ~360 turns — under the 600-turn
`max_turns` cap, but with no margin for a stall and far past the point where a
single review can be thorough. **Split 04d into at least three phases of ~15–20
sites**, one file group each. The observed rate, not a guess, is the basis for
that.

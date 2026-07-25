# Phase 02: Session-Cleanup Deadlock

**Milestone:** M5 — UX & Stability
**Status:** done
**Depends on:** none
**Estimated diff:** ~180 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Fix the re-entrant `SessionStore` lock that deadlocks the daemon roughly one
hour after every start, and move the blocking work in the same critical section
(tmux subprocess spawns, filesystem sweeps) outside the lock. This is the
confirmed root cause of the reported hang — a live wedge was captured and
attributed with gdb.

## Architecture references

Read before starting:

- `docs/design/daemon-stalls.md` § 1.5c — the root cause, with the gdb evidence
  and the timing analysis. Read this first; it explains *why* the second lock is
  fatal rather than merely redundant.
- `docs/design/daemon-stalls.md` § 1.3 — mechanism A, the general shape (global
  lock held across blocking work) that this phase also cleans up here.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

The defect is `src/daemon/mod.rs:683-723`, the `session-cleanup` supervisor.
Quoted in full — this is the code to change:

```rust
    tokio::spawn(supervise(
        "session-cleanup",
        Arc::clone(&shutdown),
        move || {
            let sessions_cleanup = Arc::clone(&sessions_cleanup_sup);
            async move {
                let mut sweep_counter = 0u32;
                loop {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    let now = Instant::now();
                    let mut store = sessions_cleanup.lock().unwrap_or_log();   // ← 693
                    store.retain(|_, v| {
                        if now.duration_since(v.last_accessed()) >= Duration::from_secs(1800) {
                            v.cleanup_bg_windows();       // ← blocking tmux subprocess, under the lock
                            false
                        } else {
                            true
                        }
                    });

                    sweep_counter = sweep_counter.wrapping_add(1);
                    if sweep_counter.is_multiple_of(60) {
                        let retention_days = startup_config.events.retention_days;
                        crate::daemon::utils::sweep_event_segments(retention_days);   // ← file I/O, under the lock

                        let archive_retention = startup_config.sessions.archive_retention_days;
                        let active_ids: std::collections::HashSet<String> = sessions_cleanup
                            .lock()                                          // ← 709: SAME mutex, SAME thread
                            .unwrap_or_log()
                            .keys()
                            .cloned()
                            .collect();
                        crate::daemon::utils::sweep_session_archives(
                            archive_retention,
                            &active_ids,
                        );                                                   // ← file I/O, under the lock
                    }
                }                                                            // ← 720: `store` finally drops
            }
        },
    ));
```

Three separate defects in these 30 lines:

1. **The deadlock.** `store` (line 693) is a `let`-bound `MutexGuard`. It has a
   `Drop` impl, so it lives to the end of the loop body at line 720. Line 709
   locks the *same* `Arc<Mutex<..>>` on the *same* thread while that guard is
   alive. `std::sync::Mutex` is **not reentrant** — this blocks forever.
   `sessions_cleanup_sup` is `Arc::clone(&sessions)` (`mod.rs:682`), the global
   store every IPC handler locks, so the stranded guard wedges the whole daemon.

   It fires when `sweep_counter % 60 == 0` on a 60-second loop — the 60th
   iteration, ≈60 minutes after start, deterministically, regardless of load.

2. **Blocking tmux subprocesses under the lock.** `v.cleanup_bg_windows()` runs
   inside `retain`, i.e. with the guard held. It calls
   `crate::tmux::kill_job_window` per background window
   (`src/daemon/session.rs:362-371`), each a blocking `std::process::Command`
   with no timeout.

3. **Filesystem sweeps under the lock.** `sweep_event_segments` and
   `sweep_session_archives` both walk directories and delete files, both with the
   guard still held.

Only defect 1 causes the hang. Defects 2 and 3 are mechanism A (design doc
§ 1.3) and are what make a slow tmux server or a large archive directory able to
stall every session in the daemon.

`clippy::await_holding_lock` does not fire on any of this: there is no `.await`
between the two locks, only a plain double-acquire. Do not expect the lint gate
to catch a regression here — that is why this phase adds a test.

Signatures you will need:

```rust
// src/daemon/session.rs:116
pub type SessionStore = Arc<Mutex<HashMap<String, SessionEntry>>>;

// src/daemon/session.rs:356
pub fn last_accessed(&self) -> std::time::Instant

// src/daemon/session.rs:362
pub fn cleanup_bg_windows(&self)

// src/daemon/utils/event_log.rs:224
pub fn sweep_event_segments(retention_days: u32)

// src/daemon/utils/mod.rs:20
pub fn sweep_session_archives(retention_days: u32, active_sessions: &HashSet<String>)
```

## Spec

### 1. Add `cleanup_pass` to `src/daemon/session.rs`

Add a function that performs the **entire** locked portion of a cleanup
iteration and returns everything the caller needs to finish the work *without*
the lock. It must acquire the lock exactly once and release it before returning.

```rust
/// One session-cleanup pass: evict sessions idle longer than `idle_after` and
/// report which sessions remain.
///
/// The lock is acquired **once** and released before this returns. Evicted
/// entries are handed back by value so the caller can run their teardown —
/// which spawns tmux subprocesses — outside the critical section.
///
/// Do not add a second `sessions.lock()` to this function or to its caller's
/// iteration. `std::sync::Mutex` is not reentrant; a second acquisition while
/// the first guard is alive deadlocks the whole daemon, because every IPC
/// handler locks this same store. See `docs/design/daemon-stalls.md` § 1.5c.
pub fn cleanup_pass(
    sessions: &SessionStore,
    now: std::time::Instant,
    idle_after: std::time::Duration,
) -> (Vec<SessionEntry>, std::collections::HashSet<String>) {
    let mut store = sessions.lock().unwrap_or_log();

    let expired: Vec<String> = store
        .iter()
        .filter(|(_, v)| now.duration_since(v.last_accessed()) >= idle_after)
        .map(|(k, _)| k.clone())
        .collect();

    let mut evicted = Vec::with_capacity(expired.len());
    for key in expired {
        if let Some(entry) = store.remove(&key) {
            evicted.push(entry);
        }
    }

    let active: std::collections::HashSet<String> = store.keys().cloned().collect();
    (evicted, active)
}
```

Note the shape: `HashMap::remove` returns the owned `SessionEntry`, so the
caller gets the evicted entries without needing `SessionEntry: Clone` (it is not
`Clone`). Do not try to clone entries or to call `cleanup_bg_windows` in here.

### 2. Rewrite the `session-cleanup` supervisor body in `src/daemon/mod.rs`

Replace the loop body (lines 690–720 in the quote above) with:

```rust
                loop {
                    tokio::time::sleep(Duration::from_secs(60)).await;

                    // Locked phase: evict and snapshot. The guard is released
                    // when `cleanup_pass` returns.
                    let (evicted, active_ids) = crate::daemon::session::cleanup_pass(
                        &sessions_cleanup,
                        Instant::now(),
                        Duration::from_secs(1800),
                    );

                    // Unlocked phase: everything blocking happens out here.
                    for entry in &evicted {
                        entry.cleanup_bg_windows();
                    }

                    sweep_counter = sweep_counter.wrapping_add(1);
                    if sweep_counter.is_multiple_of(60) {
                        crate::daemon::utils::sweep_event_segments(
                            startup_config.events.retention_days,
                        );
                        crate::daemon::utils::sweep_session_archives(
                            startup_config.sessions.archive_retention_days,
                            &active_ids,
                        );
                    }
                }
```

`active_ids` is captured while the lock was held and used after it is released.
That is correct and intended: it is a snapshot, and `sweep_session_archives`
only uses it to avoid deleting archives of live sessions. A session created in
the microseconds after the snapshot has no archive old enough to be swept.

**Do not** re-lock to "refresh" `active_ids`. That is the bug this phase exists
to remove.

### 3. Keep the eviction threshold and cadence identical

The 60-second loop interval, the 1800-second idle threshold, and the
`sweep_counter % 60` sweep cadence are unchanged. This phase changes *where the
lock is held*, not *what the cleanup does*. A behavior change here would be
scope creep.

## Acceptance criteria

- [ ] `cargo fmt --all` clean; `cargo build` succeeds.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits zero.
- [ ] `cargo test` green.
- [ ] Test `cleanup_pass_releases_the_lock` passes.
- [ ] Test `cleanup_pass_evicts_idle_and_keeps_active` passes.
- [ ] `grep -n "lock()" src/daemon/mod.rs` shows **no** lock call inside the
      `session-cleanup` supervisor closure — the only acquisition is the one
      inside `cleanup_pass`.
- [ ] `cargo test --lib` reports **908** passing (906 now, plus exactly the two
      new tests). A higher number means scope crept.

## Test plan

Both tests go in the existing `mod tests` in `src/daemon/session.rs`, which
already has a full `SessionEntry` struct literal to copy.

**Copy the literal from `src/daemon/session.rs:480-510`** (the
`auto_name_suggested_starts_false` test) rather than writing one from scratch —
`SessionEntry` has 31 fields and no constructor. Change only `last_accessed`.
Its first lines look like this:

```rust
        let entry = SessionEntry {
            messages: vec![],
            last_accessed: std::time::Instant::now(),
            chat_pane: None,
            default_target_pane: None,
            bg_windows: vec![],
            last_prompt_tokens: 0,
            tmux_session: "test".to_string(),
            // … 24 more fields — copy them verbatim from that test
        };
```

A local helper in the test module — `fn entry_with(last_accessed: Instant) ->
SessionEntry` — keeps both tests short. Give evicted entries `bg_windows:
vec![]` so `cleanup_bg_windows` has nothing to kill and no test ever shells out
to tmux.

- `cleanup_pass_releases_the_lock` in `src/daemon/session.rs` — **the regression
  test for the deadlock.** Build a store with one entry, call `cleanup_pass`,
  then assert `sessions.try_lock().is_ok()`.

  `try_lock` is the right assertion because it is deterministic and cannot hang:
  a stranded guard makes it return `Err(TryLockError::WouldBlock)` immediately,
  whereas a `lock()` in the test would hang CI forever. Do **not** write this
  test with a thread + timeout; do **not** write it with `lock()`.

  Sanity-check the test's power: temporarily hold a guard across the call site
  in a scratch copy and confirm the assertion fails. State the result in the
  Update Log.

- `cleanup_pass_evicts_idle_and_keeps_active` in `src/daemon/session.rs` — build
  a store with two entries: one whose `last_accessed` is
  `Instant::now() - Duration::from_secs(3600)` (idle) and one at
  `Instant::now()` (active). Call `cleanup_pass` with
  `idle_after = Duration::from_secs(1800)`. Assert:
  - the returned `evicted` vec has length 1;
  - the returned active set contains the active session's id and **not** the
    evicted one — the negative half matters, since a set that contains
    everything would still pass a length-only check;
  - the store itself now has exactly one entry left.

  Build the past instant with `Instant::now().checked_sub(Duration::from_secs(3600))`
  and fall back to `Instant::now()` if it returns `None` — `Instant` subtraction
  can panic on some platforms if the monotonic clock is younger than the offset.

## End-to-end verification

**Do not attempt an interactive verification.** Do not launch tmux, the daemon,
or the chat client — the previous phase lost 60 turns to exactly that, and the
architect performs this check.

Write this under an "End-to-end verification" heading in the Update Log instead:

> Not applicable to the executor — the runtime behavior this phase fixes takes
> ~60 minutes of daemon uptime to manifest and requires a live daemon. The
> architect verifies it by running the patched daemon and confirming it is still
> answering after the one-hour mark.

The architect's check, recorded here so it is not lost: start the patched
daemon, wait past the 60-minute mark, then confirm `daemoneye status` still
responds and that `ss -lx | grep daemoneye.sock` shows a `Recv-Q` of 0 rather
than a growing backlog.

## Authorizations

None. No new dependencies; `docs/architecture.md` is not touched.

## Out of scope

- **Do not** change `SessionStore`'s type. Converting it to
  `tokio::sync::Mutex`, an `RwLock`, or a sharded map is a much larger change
  and invites guards held across `.await`, which is a worse failure than the one
  being fixed. The type stays `Arc<Mutex<HashMap<String, SessionEntry>>>`.
- **Do not** fix the other mechanism-A sites found in the design doc
  (`webhook/process.rs:148` and `:161`). They are real and they are a later
  phase; touching them here makes this diff unreviewable.
- **Do not** convert any tmux call to `tokio::process` or `spawn_blocking`. That
  is the tmux-hardening phase.
- **Do not** change the eviction threshold, the loop interval, or the sweep
  cadence (spec task 3).
- **Do not** add a deadlock-detection or lock-instrumentation framework. A
  later phase covers instrumentation; this one removes a specific bug.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

### Update — 2026-07-25 19:05 (started)

**Executor:** claude

Added `cleanup_pass` to `src/daemon/session.rs` and rewrote the session-cleanup supervisor in `src/daemon/mod.rs`. The deadlock (reentrant `std::sync::Mutex` lock on the same thread) is eliminated — the lock is now acquired exactly once inside `cleanup_pass` and released before the function returns. Blocking work (`cleanup_bg_windows`, `sweep_event_segments`, `sweep_session_archives`) now runs in the unlocked phase. Two regression tests added: `cleanup_pass_releases_the_lock` and `cleanup_pass_evicts_idle_and_keeps_active`.

**End-to-end verification:** Not applicable to the executor — the runtime behavior this phase fixes takes ~60 minutes of daemon uptime to manifest and requires a live daemon. The architect verifies it by running the patched daemon and confirming it is still answering after the one-hour mark.

<!-- entries appended below this line -->

### Update — ts=1785006846403 (complete, server-authored)

**Summary:** All acceptance criteria verified:

- `cargo fmt --all` clean
- `cargo build` succeeds
- `cargo clippy --all-targets --all-features -- -D warnings` exits zero
- `cargo test --lib` reports **908** passing (exactly +2 from baseline)
- `grep -n "lock()" src/daemon/mod.rs` shows no lock call inside the session-cleanup supervisor closure — only `bg_sn.lock()` (line 630, different mutex) and `sessions.lock()` (line 829, different context)
- `grep -n "cleanup_pass" src/daemon/session.rs` confirms the function landed

**Summary:** Fixed the session-cleanup deadlock by extracting `cleanup_pass()` into `src/daemon/session.rs`, which acquires the `SessionStore` lock exactly once, evicts idle sessions by value, and snapshots active IDs — then returns with the guard dropped. The supervisor in `mod.rs` now runs all blocking work (`cleanup_bg_windows`, `sweep_event_segments`, `sweep_session_archives`) in the unlocked phase. Two regression tests confirm the lock is released and eviction logic is correct. The eviction threshold (1800s), loop cadence (60s), and sweep counter (every 60 iterations) are unchanged. Minor adaptation: the `SessionEntry` struct fields had drifted from the spec's sketch (e.g., `workspace`/`project_root`/`session_id`/`name` replaced by `last_detach`/`detach_time_utc`/`messages_at_detach`/`pipe_source_pane`/`is_ghost`/`ghost_config`), so the test helper was built from the actual struct definition.

**Executor:** Qwen/Qwen3.6-27B-FP8

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.53s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
ontext_session_topology ... ok
test tmux::cache::tests::get_labeled_context_copy_mode_annotated ... ok
test tmux::cache::tests::get_labeled_context_synchronized_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_pane_classification ... ok
test tmux::cache::tests::get_labeled_context_client_viewport_shown_when_known ... ok
test tmux::cache::tests::get_labeled_context_client_viewport_absent_when_zero ... ok
test tmux::cache::tests::get_labeled_context_background_panes_sorted ... ok
test tmux::cache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test tmux::cache::tests::get_labeled_context_chat_pane_excluded_from_background ... ok
test search::tests::search_events_returns_tail_not_head_when_segment_exceeds_cap ... ok
test search::tests::search_finds_match_in_runbooks ... ok
test search::tests::search_respects_kind_filter ... ok
test search::tests::search_returns_empty_for_no_match ... ok
test session_store::tests::artifacts_round_trip ... ok
test session_store::tests::backfill_idempotent ... ok
test session_store::tests::backfill_missing_artifact_returns_error_name ... ok
test session_store::tests::backfill_stamps_runbook ... ok
test memory::tests::migrate_namespace_adds_missing ... ok
test session_store::tests::backfill_stamps_script ... ok
test session_store::tests::collision_allowed_with_force ... ok
test session_store::tests::collision_rejected_without_force ... ok
test memory::tests::migrate_namespace_skips_already_migrated ... ok
test session_store::tests::delete_removes_dir_and_index ... ok
test session_store::tests::list_returns_newest_first ... ok
test session_store::tests::load_messages_max_count_truncates ... ok
test session_store::tests::rename_nonexistent_errors ... ok
test session_store::tests::rename_to_existing_errors ... ok
test session_store::tests::rename_updates_dir_and_index ... ok
test session_store::tests::save_and_load_round_trip ... ok
test session_store::tests::update_in_place_allowed ... ok

test result: ok. 908 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.21s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 29 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g4_briefing_injection_block_format ... ok
test g5_depth_limit_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_ask_round_trip ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test minimal_config_parsing ... ok
test ghost_config_parsing ... ok
test schedule_store_persistence ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g4_briefing_read_and_clear ... ok
test config_pricing_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_jsonl_round_trip ... ok
test session_index_persistence ... ok
test g4_briefing_masking_applied ... ok
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

- `docs/dev/milestones/M5-ux-stability/README.md` — +1 -1
- `docs/dev/milestones/M5-ux-stability/phase-02-cleanup-deadlock.md` — +9 -1
- `src/daemon/mod.rs` — +17 -21
- `src/daemon/session.rs` — +114 -0

**Commit:** 435382e50aaa267a451105855c3cc7134de05e51

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### End-to-end verification — 2026-07-25 (architect-performed)

The phase doc deferred this to the architect because the defect needs ~60 minutes
of daemon uptime to manifest. Rather than wait, the reviewer built the daemon
with the cleanup loop's sleep temporarily reduced from 60 s to 1 s, so the
`sweep_counter % 60` branch — the one that took the second lock — fires at ~60
seconds instead of ~60 minutes. Everything else was unchanged. Both the pre-fix
and post-fix trees were soaked under the identical acceleration.

**Before** (pre-fix `mod.rs` from `de3d050`, accelerated), at 1 m 32 s uptime:

```
threads with epoll_wait : 0          ← reactor gone
thread wchan breakdown  : 33 futex_wait
accept backlog (Recv-Q) : 2 and climbing   ← connections queued, never accepted
CPU time                : 00:00:00
```

**After** (this phase's fix, same acceleration), at 3 m 01 s uptime — the sweep
branch has fired three times:

```
threads with epoll_wait : 1          ← reactor alive and polling
thread wchan breakdown  : 32 futex_wait, 1 __x64_sys_epoll_wait
accept backlog (Recv-Q) : 0          ← accepting normally
CPU time                : 00:00:00
```

The "before" column reproduces the production wedge exactly as captured in
`docs/design/daemon-stalls.md` § 1.5b (reactor absent, all threads futex-parked,
backlog growing, zero CPU). The "after" column is the healthy signature. The
acceleration was reverted, the tree restored to a clean `git status`, and all
gates re-run green afterward.

Method note for future phases: compressing a time-triggered defect by shrinking
its interval, and soaking **both** trees under the identical change, is what
makes the "after" result meaningful. A healthy post-fix daemon alone would not
have shown that the harness could catch the bug at all.

### Review verdict — 2026-07-25

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** Qwen/Qwen3.6-27B-FP8 (70 turns)
- **Gates (reviewer re-run):** `cargo fmt --all --check` clean; `cargo build`
  clean; `cargo clippy --all-targets --all-features -- -D warnings` exits zero;
  `cargo test` 908 lib + 27 integration, 0 failed. Lib count is exactly the
  pinned 908 (906 + 2) — no scope creep.
- **Acceptance criteria:** all met. Verified independently that no `lock()` call
  remains anywhere in the `session-cleanup` supervisor closure
  (`mod.rs:683-725`); the only acquisition is the single one inside
  `cleanup_pass`.
- **Mutation check:** performed by the reviewer, not taken on trust. Adding
  `std::mem::forget(store)` to strand the guard makes
  `cleanup_pass_releases_the_lock` fail immediately with
  `assertion failed: sessions.try_lock().is_ok()`. The `try_lock` formulation
  works as designed — it fails fast instead of hanging.
- **End-to-end:** accelerated before/after soak, recorded above. The pre-fix tree
  reproduced the production wedge in 92 seconds; the fixed tree stayed healthy
  through three sweep cycles.
- **Scope deviations:** none. The eviction threshold, loop cadence, and sweep
  cadence are unchanged, as spec task 3 required. The executor's summary notes
  it built the test helper from the real `SessionEntry` definition rather than
  the doc's abbreviated sketch — that is correct behavior, not a deviation; the
  spec explicitly said to copy the literal from the existing test.
- **Calibration:** none for the executor. The spec quoted the offending code
  verbatim, wrote out the replacement, and pinned an inverted test count; the run
  completed clean in 70 turns with no bounce. This is the third consecutive
  clean-ish outcome for small, synchronous, fully-quoted phases and reinforces
  the M4 note that this executor performs well on that shape.

#### Follow-up noted, not blocking

`cleanup_pass_evicts_idle_and_keeps_active` ends with
`assert_eq!(sessions.lock().unwrap().len(), 1)`. Under the stranded-guard
mutation that assertion **hangs** rather than failing, so a future re-entrancy
regression would stall CI until timeout even though
`cleanup_pass_releases_the_lock` fails fast in parallel. Changing that final
assertion to a `try_lock`-based read would make the whole file fail-fast. One
line; fold it into whichever phase next touches `session.rs` rather than
spending a dispatch cycle on it.

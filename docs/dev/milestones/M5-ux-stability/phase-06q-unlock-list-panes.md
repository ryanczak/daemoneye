# Phase 06q: Unlock `handle_list_panes` — a tmux Sweep Under the Cache Read Guard

**Milestone:** M5 — UX & Stability
**Status:** done
**Depends on:** phase-06n — `done`
**Estimated diff:** ~35 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

`handle_list_panes` runs `tmux::pane_exists` — **one subprocess per pane** —
inside a `.filter()` closure, **while holding the `cache.panes` read guard**.
Split it into the collect-under-lock / act-outside shape and take the probe off
the runtime.

This is **two defects in one site**:

1. **Mechanism A** — blocking work (N tmux subprocesses) inside a critical
   section. Every other reader of `cache.panes` waits for the whole sweep.
2. **Mechanism B** — those subprocesses run on a tokio worker, and they cannot
   be `.await`ed where they sit because `.filter()`'s closure is synchronous.

**Finish condition: `pane_exists` is called from an `off_runtime` closure in an
unlocked phase, and the `cache.panes` guard is dropped before it runs.**

## Architecture references

- `docs/design/daemon-stalls.md` § 1 — mechanism A (lock held across blocking
  work) and mechanism B (blocking subprocess on tokio workers).
- `src/tmux/mod.rs:29` — the `off_runtime` adapter and `TMUX_TIMEOUT`.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Verify the starting state:

```bash
grep -c "off_runtime"        src/daemon/server/handlers.rs   # expect 0
grep -c "pane_exists"        src/daemon/server/handlers.rs   # expect 1
grep -cF "cache.panes.read()" src/daemon/server/handlers.rs  # expect 2
grep -c "\.filter("          src/daemon/server/handlers.rs   # expect 4
grep -c "sort_by_key"        src/daemon/server/handlers.rs   # expect 1
cargo test 2>&1 | grep "^test result" | head -3   # expect 916 lib, 0, 27 integration
```

**Every number above was produced by running that exact command against the tree
while drafting.** If one differs, **stop and report a blocker**.

⚠ **Two of those counts are not about this site.** `cache.panes.read()` is **2**
because `handle_set_pane` (`:136`) has its own, and `.filter(` is **4** because
`handle_status` (`:246`) has one. **Neither is in scope.** Their presence is why
the acceptance criteria below are not all zeros.

## Current state

### ⚠ This is a different lock from the rest of the milestone

Every lock phase in M5 so far has been about `SessionStore`. This one is
`cache.panes`, a `std::sync::RwLock` on `SessionCache` — so `with_sessions`, the
re-entrancy assertion, and the `SessionsLockDepth` counter **do not apply here**.
The fix is the same *shape* (collect under the lock, act outside), but there is
no accessor to route it through. Do not try to use `with_sessions`.

### The site — `src/daemon/server/handlers.rs:174`

`handle_list_panes` is `async fn` (`:158`), so `.await` is legal in its body —
just not inside the `.filter()` closure, which is where the call sits today:

```rust
    let panes_snapshot = {
        let panes = cache.panes.read().unwrap_or_log();
        let mut entries: Vec<_> = panes
            .iter()
            .filter(|(id, _)| chat_pane_id.as_deref() != Some(id.as_str()))
            .filter(|(_, s)| {
                !s.window_name.starts_with("de-bg-")
                    && !s.window_name.starts_with("de-sj-")
                    && !s.window_name.starts_with("de-gs-bg-")
                    && !s.window_name.starts_with("de-gs-sj-")
                    && !s.window_name.starts_with("de-gs-ir-")
            })
            .filter(|(id, _)| crate::tmux::pane_exists(id))
            .map(|(id, s)| {
                let is_target = current_target.as_deref() == Some(id.as_str());
                (
                    id.clone(),
                    s.current_cmd.clone(),
                    s.window_name.clone(),
                    s.pane_index,
                    is_target,
                )
            })
            .collect();
        entries.sort_by_key(|(_, _, win, idx, _)| (win.clone(), *idx));
        entries
    };
```

The worked example for the fix is `cleanup_pass` (`src/daemon/session.rs:471`) —
the shape adopted across phase 05 after a confirmed production hang: take what
you need under the lock, release, then do the blocking work.

### ⭐ The exact code — compile-, clippy-, fmt- and test-checked against this tree

Applied, verified, and reverted while drafting. **`cargo fmt` rewrote the
`off_runtime` line into the one-line closure form shown below** — this project
has no `format_fix` hook, so use exactly this:

```rust
    // Phase 1 (locked): snapshot the candidates. No blocking work under the
    // read guard — `pane_exists` spawns a tmux subprocess per pane and used to
    // run here, holding the cache lock for the whole sweep.
    let candidates: Vec<(String, String, String, usize, bool)> = {
        let panes = cache.panes.read().unwrap_or_log();
        panes
            .iter()
            .filter(|(id, _)| chat_pane_id.as_deref() != Some(id.as_str()))
            .filter(|(_, s)| {
                !s.window_name.starts_with("de-bg-")
                    && !s.window_name.starts_with("de-sj-")
                    && !s.window_name.starts_with("de-gs-bg-")
                    && !s.window_name.starts_with("de-gs-sj-")
                    && !s.window_name.starts_with("de-gs-ir-")
            })
            .map(|(id, s)| {
                let is_target = current_target.as_deref() == Some(id.as_str());
                (
                    id.clone(),
                    s.current_cmd.clone(),
                    s.window_name.clone(),
                    s.pane_index,
                    is_target,
                )
            })
            .collect()
    };

    // Phase 2 (unlocked): the liveness probe, one bounded tmux call per
    // candidate, off the runtime.
    let mut panes_snapshot = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let id = candidate.0.clone();
        let exists = crate::tmux::off_runtime("pane-exists", move || crate::tmux::pane_exists(&id))
            .await
            .unwrap_or(false);
        if exists {
            panes_snapshot.push(candidate);
        }
    }
    panes_snapshot.sort_by_key(|(_, _, win, idx, _)| (win.clone(), *idx));
```

The block below it is unchanged and still sends `Response::PaneList { panes:
panes_snapshot }`.

### Three semantics this preserves — each looks like a change and is not

1. **`.map()` now runs before the liveness filter.** Previously `pane_exists`
   filtered *before* `.map()`. The `.map()` closure is **pure** — it clones five
   fields and compares `current_target` — so moving it earlier changes nothing
   about the result set, only how many tuples are built (at most one per
   candidate, all discarded if the pane is gone). **The final `Vec` is
   identical.**
2. **A timeout drops the pane from the list, and that is behaviour-preserving.**
   `pane_exists` (`src/tmux/pane.rs:436`) is:

   ```rust
   pub fn pane_exists(pane_id: &str) -> bool {
       Command::new("tmux")
           .args(["display-message", "-t", pane_id, "-p", "#{pane_id}"])
           .output()
           .map(|o| o.status.success())
           .unwrap_or(false)
   }
   ```

   It **already returns `false`** when the tmux call fails. So `.unwrap_or(false)`
   on timeout matches what the existing code does on error. **Do not invert it**
   — `.unwrap_or(true)` would list panes that may not exist.
3. **The sort must stay after the filter**, on the filtered vector. It is a
   stable ordering by `(window_name, pane_index)` and the CLI renders the list in
   that order.

### 🛑 The other `cache.panes.read()` and `.filter(` are NOT in scope

`handle_set_pane:136` has its own read guard and `handle_status:246` its own
`.filter(|s| !s.is_ghost)`. **Do not touch either.** The criteria below pin
`cache.panes.read()` at **2** (unchanged) and `.filter(` at **3** (4 minus the
one removed here) precisely so an over-eager sweep into them fails the phase.

## Spec

1. **Split the block in two** — in `src/daemon/server/handlers.rs`, replace the
   `panes_snapshot` block with the two phases above. The `cache.panes` read guard
   must be dropped when phase 1's block ends.
2. **Remove the synchronous `pane_exists` filter** from the iterator chain; the
   probe now happens in the phase-2 loop.
3. **Change nothing else in the file.**

### Build after the edit

`cargo build`, then `cargo fmt --all`, then `cargo clippy`. **Run `cargo fmt
--all` before you finish** — this project auto-formats nothing for you.

## Acceptance criteria

- [ ] `grep -c "off_runtime" src/daemon/server/handlers.rs` returns **1**
      (printed **0** before).
- [ ] `grep -cF ".filter(|(id, _)| crate::tmux::pane_exists(id))" src/daemon/server/handlers.rs`
      returns **0** — the synchronous filter is gone.
- [ ] `grep -cF "crate::tmux::pane_exists(&id)" src/daemon/server/handlers.rs`
      returns **1** — the call now takes a reference to an owned `id` inside the
      closure.
- [ ] `grep -c "\.filter(" src/daemon/server/handlers.rs` returns **3**
      (printed **4** before; one removed). **Not 2** — `handle_status:246` has
      one that is out of scope.
- [ ] `grep -cF "cache.panes.read()" src/daemon/server/handlers.rs` returns
      **2** — **unchanged**. `handle_set_pane` keeps its own guard.
- [ ] `grep -c "sort_by_key" src/daemon/server/handlers.rs` returns **1** — the
      sort was moved, not duplicated or dropped.
- [ ] `grep -cF "unwrap_or(false)" src/daemon/server/handlers.rs` returns **1**
      and `grep -cF "unwrap_or(true)" src/daemon/server/handlers.rs` returns
      **0** — the timeout arm is not inverted.
- [ ] `grep -c "block_on\|futures::executor\|spawn_blocking" src/daemon/server/handlers.rs`
      returns **0**.
- [ ] `git diff --name-only` lists exactly **one** `src/` file:
      `src/daemon/server/handlers.rs`.
- [ ] **Read and confirm**, quoting the code: the `let candidates … };` block
      closes **before** the `for candidate in candidates` loop begins, so the
      read guard is released before any `off_runtime` call.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **916** lib-unit and **27** integration tests.

**Run every gate bare.**

## Test plan

`handle_list_panes` needs a live tmux server, a populated `SessionCache` and an
IPC peer. **It has no unit coverage**, and `handlers.rs` has no test module.
Pre-existing gap, neither widened nor closed here.

**The whole change compiled and the full suite passed with no test edited** in
the checked run — so if any test needs editing, **stop and report a blocker**.

**Write no new tests.** Run the suite and report which commands you ran and
whether they passed. **Do not claim any test guards this handler.**

Three reasoning checks. **Quote the code — a claim without a quotation is not an
answer:**

1. **The guard.** Quote the line where the `candidates` block closes and the line
   where the loop opens, and state in one sentence why the read guard is no
   longer held during the tmux probes.
2. **The reorder.** State in one sentence why moving `.map()` ahead of the
   liveness check cannot change which panes end up in `panes_snapshot`.
3. **The timeout arm.** Quote your `.unwrap_or(false)` line and state in one
   sentence why `false` — not `true` — is the behaviour-preserving default.

## End-to-end verification

None required. 06a demonstrated the timeout arm fires; this phase adds no
machinery. **Do not repeat that demonstration.**

## Authorizations

- [x] May edit `src/daemon/server/handlers.rs` — **the `handle_list_panes`
      `panes_snapshot` block only.**
- [x] May add owned bindings and `.clone()` calls in that block.
- [x] May add the two phase comments shown above.
- [ ] **No** edit to `handle_set_pane`, `handle_status`, or any other handler.
- [ ] **No** change to `Response::PaneList`'s tuple shape
      `(String, String, String, usize, bool)`.
- [ ] **No** change to `src/tmux/pane.rs::pane_exists`.
- [ ] **No** signature change to `handle_list_panes`.
- [ ] **No** use of `with_sessions` — this is the `cache.panes` RwLock, not
      `SessionStore`.
- [ ] **No** `block_on`, `futures::executor`, or detached `tokio::spawn`.
- [ ] **No** edits to `src/tmux/` or any file outside the one named.
- [ ] **No** new tests, no new dependency, no `#[allow(...)]`.

## Out of scope

- **`webhook/process.rs:190` / `inject_ghost_event`** — a 13-site async cascade.
  **06r.**
- **Any other `cache.panes` reader** — `handle_set_pane:136` and the cache poller
  itself. Only `handle_list_panes` does blocking work under the guard.
- **`src/cli/`** — no concurrency to protect; bounded by stage A instead.
- **`src/tmux/{pane,session,window,cache}.rs`** — the sync helper bodies. Stage A
  hardens them with their own timeouts. **Do not touch them.**

### ⚠ Traps

1. **`.unwrap_or(false)`, never `true`** — `pane_exists` already returns `false`
   on tmux failure.
2. **The other `.filter(` and `cache.panes.read()` stay.** The criteria are **3**
   and **2**, not 2 and 1.
3. **Do not reach for `with_sessions`** — wrong lock.
4. **`cargo fmt --all` collapses the `off_runtime` closure to one line.** Use the
   post-`fmt` form given above, and run `fmt` before finishing.
5. **The sort stays**, after the filter, exactly once.
6. **The guard must be released before the loop** — that is the entire point of
   the phase.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-28 17:58 (started)

**Executor:** Claude (Anthropic)

Split `handle_list_panes` into collect-under-lock / act-outside shape. Removed the synchronous `pane_exists` from the `.filter()` closure and replaced it with an `off_runtime` loop in an unlocked phase.

### Update — ts=1785261749272 (complete, server-authored)

**Summary:** All acceptance criteria verified:

- `off_runtime`: **1** ✓
- Synchronous `.filter(|(id, _)| crate::tmux::pane_exists(id))`: **0** ✓
- `crate::tmux::pane_exists(&id)`: **1** ✓
- `.filter(`: **3** ✓
- `cache.panes.read()`: **2** ✓
- `sort_by_key`: **1** ✓
- `unwrap_or(false)`: **1**, `unwrap_or(true)`: **0** ✓
- `block_on/spawn_blocking`: **0** ✓
- Only `src/daemon/server/handlers.rs` changed among `src/` files ✓
- `cargo build`, `cargo clippy`, `cargo fmt`, `cargo test` all pass clean ✓

**Reasoning checks:**

1. **The guard.** The `candidates` block closes at line `    };` (end of the collect block), and the loop opens at `for candidate in candidates {` on the next line. The `RwLockReadGuard` held by `let panes = cache.panes.read()...` is dropped when the block ends, so no lock is held during the `off_runtime` tmux probes.
2. **The reorder.** `.map()` is pure — it only clones five fields and compares `current_target` — so moving it ahead of the liveness check cannot change which panes end up in the result.
3. **The timeout arm.** `let exists = crate::tmux::off_runtime("pane-exists", move || crate::tmux::pane_exists(&id)).await.unwrap_or(false);` — `false` is correct because `pane_exists` already returns `false` on tmux failure, so a timeout (which also means we can't confirm the pane exists) should exclude it from the list.

**Executor:** Qwen/Qwen3.6-27B-FP8

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.06s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
assification ... ok
test tmux::cache::tests::get_labeled_context_synchronized_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_dead_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_client_viewport_absent_when_zero ... ok
test tmux::cache::tests::get_labeled_context_session_topology ... ok
test tmux::cache::tests::get_labeled_context_background_panes_sorted ... ok
test tmux::cache::tests::get_labeled_context_client_viewport_shown_when_known ... ok
test tmux::cache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test tmux::cache::tests::get_labeled_context_chat_pane_excluded_from_background ... ok
test search::tests::search_events_returns_tail_not_head_when_segment_exceeds_cap ... ok
test search::tests::search_finds_match_in_runbooks ... ok
test search::tests::search_respects_kind_filter ... ok
test memory::tests::migrate_namespace_skips_already_migrated ... ok
test session_store::tests::artifacts_round_trip ... ok
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
test session_store::tests::rename_updates_dir_and_index ... ok
test session_store::tests::save_and_load_round_trip ... ok
test session_store::tests::update_in_place_allowed ... ok

test result: ok. 916 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.23s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 29 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g4_briefing_injection_block_format ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_session_info_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test ipc_tool_call_response_round_trip ... ok
test ipc_ask_round_trip ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test minimal_config_parsing ... ok
test ghost_config_parsing ... ok
test event_log_entry_format ... ok
test schedule_store_persistence ... ok
test event_log_append_read ... ok
test config_pricing_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
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
- `docs/dev/milestones/M5-ux-stability/phase-06q-unlock-list-panes.md` — +7 -1
- `src/daemon/server/handlers.rs` — +20 -6

**Commit:** 32ee0919ad38f4fa5f17e7cbb17727d30a1fa7cd

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-07-28

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** Qwen/Qwen3.6-27B-FP8 (36 turns)
- **Scope deviations:** none
- **Calibration:** none — and the apply-verify-revert refinement worked (below)

All four gates re-run bare and green (`cargo fmt --all --check`, `cargo build`
after `touch`ing `handlers.rs` — zero warnings, `cargo clippy --all-targets
--all-features -- -D warnings`, `cargo test` at 916 lib + 27 integration,
unchanged).

Every criterion is exact, **including the two deliberately-non-zero guards**:
`off_runtime` **1** (0 before); the bare synchronous filter **0**;
`crate::tmux::pane_exists(&id)` **1**; `.filter(` **3** — *not* 2, so
`handle_status:246` was left alone; `cache.panes.read()` **2** — unchanged, so
`handle_set_pane:136` kept its own guard; `sort_by_key` **1**;
`unwrap_or(false)` **1** with `unwrap_or(true)` **0**;
`block_on`/`spawn_blocking` **0**; one `src/` file. **The diff is identical to
the spec's post-`fmt` block.**

Verified by reading:

- **The guard is released before any tmux call.** The `candidates` block closes
  with `};` at `:200`, and the `for candidate in candidates` loop opens at
  `:203` — the `RwLockReadGuard` from `cache.panes.read()` lives only inside that
  block, so every `off_runtime` probe runs unlocked. This was the phase's whole
  point and it is structurally, not incidentally, true.
- **No `with_sessions` was added.** The file's 14 occurrences are all
  pre-existing `SessionStore` uses in other handlers; zero appear in the added
  diff. The wrong-accessor trap was avoided.
- **The diff is confined to `handle_list_panes`.** All three hunks fall in
  `:171–:218`; `handle_set_pane` (`:108–156`) and `handle_status` (`:246+`) are
  untouched.
- **The timeout arm is not inverted** and the sort still runs once, after the
  filter, on the filtered vector.

The executor answered all three reasoning checks correctly with quoted code,
including the non-obvious one — that `.map()` moving ahead of the liveness check
cannot change the result set because `.map()` is pure.

### Calibration — the apply-verify-revert refinement did its job

06n's review found an acceptance criterion made unsatisfiable by its own Spec,
and the fix proposed there was to run the doc's **acceptance greps** against the
applied tree, not just the four gates. This was the first phase drafted that
way, and it caught **three** would-be-unsatisfiable criteria before dispatch
(`.filter(` is 3 not 2; `cache.panes.read()` is 2 not 1; `pane_exists` is 2 not
1, one being the new comment) plus the fact that `cargo fmt` collapses the
`off_runtime` closure to one line.

Two of those three became the phase's most useful criteria — the non-zero guards
that would have caught a sweep into the neighbouring handlers. **A criterion that
is correct *because* it is non-obvious is worth more than one that is trivially
zero.**

That is the third consecutive payoff for the practice (06p, 06n, 06q). The fold
proposed at 06n's review stands unchanged and still awaits PE sign-off at
milestone close:

> **Apply the phase's own diff to the tree before dispatch; run the gates *and*
> the doc's acceptance criteria against it; then revert.** A criterion that
> cannot be satisfied is as expensive as a fact that is wrong.

**No doc change made.**

# Phase 05h: Stop One Failing Test From Failing Forty-Seven Others

**Milestone:** M5 — UX & Stability
**Status:** done
**Depends on:** phase-05g (which measured the cascade) — `done`
**Estimated diff:** ~150 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

`TEST_HOME_LOCK` serialises every test that mutates `HOME`. **41 of its 62
acquisition sites use `.lock().unwrap()`**, so when a test panics while holding
the lock, the lock is poisoned and *every subsequent HOME-dependent test in the
same binary fails with it.*

Measured during 05g's review, by deleting one production line:

| Mutation | Target test holds `TestHome`? | Failures |
|---|---|---|
| `background.rs:119` | no | **1** |
| `background.rs:136` | yes | **48** |
| `background.rs:232` | yes | **48** |
| `background.rs:240` | yes | **48** |

One real failure, forty-seven fictional ones. Every future mutation, bisect, or
flaky-test hunt in this repo reads through that noise.

**Fix: one accessor that recovers from poison, and every site through it.**

**Finish condition: `TEST_HOME_LOCK.lock()` appears exactly once in the tree —
inside the accessor.**

## Architecture references

Read before starting:

- `CLAUDE.md` § "Important Invariants" — the `.unwrap_or_log()` convention exists
  because a poisoned lock should degrade, not abort. This phase applies the same
  reasoning to the test lock.
- `docs/dev/WORKFLOW.md` § "A phase that exhausts a trait's uses must say what
  happens to its import" — directly relevant; see task 4.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Verify the starting state. **Use this census, not `grep -c`** — nine sites
   split the acquisition across lines and a single-line grep cannot see them:

```bash
python3 - <<'PY'
import pathlib, re
tot = {"unwrap":0, "unwrap_or_log":0, "other":0}
for f in sorted(list(pathlib.Path("src").rglob("*.rs")) + list(pathlib.Path("tests").rglob("*.rs"))):
    src = f.read_text()
    for m in re.finditer(r'TEST_HOME_LOCK', src):
        seg = src[m.end():m.end()+140]
        if not seg.lstrip().startswith(('.lock', '\n')) and '.lock()' not in seg[:60]:
            continue
        k = "unwrap_or_log" if '.unwrap_or_log()' in seg[:120] else \
            "unwrap" if '.unwrap()' in seg[:120] else "other"
        tot[k] += 1
print(tot, "=> acquisitions:", sum(tot.values()))
PY
#   {'unwrap': 41, 'unwrap_or_log': 12, 'other': 9} => acquisitions: 62
cargo test 2>&1 | grep "^test result" | head -2   # expect 916 lib, 27 integration
```

**Verified against the tree while drafting.** If the census differs, **stop and
report a blocker.**

## Current state

### Three idioms for the same intent — 21 sites already fix this, inconsistently

The codebase has independently discovered the fix twice and applied it two
different ways:

| Idiom | Sites | Poison behavior |
|---|---|---|
| `crate::TEST_HOME_LOCK.lock().unwrap()` | **41** | **panics — this is the cascade** |
| `crate::TEST_HOME_LOCK.lock().unwrap_or_log()` | 12 | recovers, logs an ERROR |
| `crate::TEST_HOME_LOCK`<br>`    .lock()`<br>`    .unwrap_or_else(std::sync::PoisonError::into_inner)` | 9 | recovers silently |

This is a convention no one can enforce — the same shape this milestone already
solved for the session store by routing every caller through one accessor.

### ⚠ The accessor must NOT be `#[cfg(test)]`

`src/lib.rs:27-32` says why, and it is load-bearing:

```rust
/// Single global lock used by tests that mutate the HOME environment variable.
/// All test modules that call `env::set_var("HOME", ...)` must hold this lock.
///
/// This is unconditionally `pub` so integration tests (which are a separate
/// crate and do not get `#[cfg(test)]` items from the library) can access it.
pub static TEST_HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
```

**`tests/integration.rs` holds 11 of the 62 sites.** A `#[cfg(test)]` accessor
would be invisible to it and the phase would not compile. The accessor is plain
`pub`, for exactly the reason the static is.

## Spec

### 1. Add the accessor to `src/lib.rs`

Immediately **below** the existing `TEST_HOME_LOCK` static (do not insert between
the static and its doc comment):

```rust
/// Acquire [`TEST_HOME_LOCK`], recovering if a previous holder panicked.
///
/// A test that panics while holding the lock poisons it. Every later
/// `.lock().unwrap()` on a poisoned mutex then panics too, so one real failure
/// becomes a failure in every HOME-dependent test in the same binary — 48
/// instead of 1, measured. Recovering keeps the count honest: the test that
/// actually broke is the only one that fails.
///
/// Unconditionally `pub`, not `#[cfg(test)]`, for the same reason the lock is:
/// integration tests are a separate crate and do not receive `#[cfg(test)]`
/// items from the library.
pub fn test_home_guard() -> std::sync::MutexGuard<'static, ()> {
    TEST_HOME_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
```

### 2. Route all 62 sites through it

Three mechanical substitutions. The binding name (`_lock`, `lock`) is whatever
the site already uses — **keep it**, so the guard's lifetime is unchanged:

```rust
// in src/ (52 sites), all three idioms collapse to:
let _lock = crate::test_home_guard();

// in tests/integration.rs (11 sites):
let _lock = daemoneye::test_home_guard();
```

**Two things not to change:**

- **A site binding `lock` (not `_lock`) keeps that name** — `background.rs:293`,
  `epochs.rs:1166`, `ghost_ws.rs:131`, `recall.rs:259` bind a named guard and use
  it later. Renaming to `_lock` would drop the guard immediately and break the
  serialisation these tests depend on.
- **Nothing else in any test.** This is a substitution of the acquisition
  expression only.

### 3. Delete `TEST_HOME_LOCK`'s now-unused imports

`ghost_ws.rs:107` has `use crate::TEST_HOME_LOCK;` and uses the bare name. After
the substitution that import is unused — **delete it**.

### 4. Delete the ten `UnpoisonExt` imports that die — and keep the one that lives

`unwrap_or_log` comes from `UnpoisonExt`. In **ten** files, the only
`unwrap_or_log` call is the `TEST_HOME_LOCK` one this phase converts, so the
import becomes unused and **`cargo clippy --all-targets` will error on it**
(`cargo build` will not — it reports zero warnings for an unused test-module
import; clippy is authoritative):

| File | `unwrap_or_log` total | on `TEST_HOME_LOCK` | Import |
|---|---|---|---|
| `src/search.rs` | 1 | 1 | **delete** |
| `src/scripts.rs` | 1 | 1 | **delete** |
| `src/runbook.rs` | 1 | 1 | **delete** |
| `src/memory_tests.rs` | 1 | 1 | **delete** |
| `src/manifest_tests.rs` | 1 | 1 | **delete** |
| `src/agents/mod.rs` | 1 | 1 | **delete** |
| `src/agents/mailbox.rs` | 1 | 1 | **delete** |
| `src/daemon/briefing.rs` | 1 | 1 | **delete** |
| `src/daemon/executor/file_ops/read.rs` | 1 | 1 | **delete** |
| `src/daemon/executor/knowledge/mod.rs` | 1 | 1 | **delete** |
| `src/daemon/executor/mod.rs` | **5** | 2 | **KEEP** — three uses survive |

**`src/daemon/executor/mod.rs` is the exception.** Deleting its import breaks the
build. Let clippy confirm each deletion rather than deleting on the pattern.

## Acceptance criteria

- [ ] The Pre-flight census reports
      `{'unwrap': 0, 'unwrap_or_log': 0, 'other': 1}` — **exactly one acquisition
      left, and it is the accessor's own body.** Not zero: the census counts any
      `TEST_HOME_LOCK` followed by `.lock()`, and `test_home_guard` is one by
      construction. **`'unwrap': 0` is the criterion that matters** — it is the
      cascade, and it must be gone.
- [ ] `grep -rn "TEST_HOME_LOCK" src/ tests/ | grep -c "\.lock()"` returns **1** —
      the accessor body in `src/lib.rs`, and nothing else. Verify by reading that
      the one hit is inside `test_home_guard`.
- [ ] `grep -rc "test_home_guard()" src/ tests/ --include=*.rs | grep -v ':0' |
      awk -F: '{s+=$2} END {print s}'` returns **63** — 62 call sites plus the
      definition.
- [ ] `grep -c "UnpoisonExt" src/daemon/executor/mod.rs` returns **3** — unchanged;
      three `unwrap_or_log` calls survive there.
- [ ] `grep -rl "UnpoisonExt" src/ | wc -l` returns **15**, down from **25** —
      exactly the ten files in task 4's table. Quote both numbers.
- [ ] `grep -c "use crate::TEST_HOME_LOCK" src/daemon/context/ghost_ws.rs` returns
      **0**.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **916** lib-unit tests and **27** integration tests —
      both unchanged. This phase adds and deletes no tests.
- [ ] `python3 /tmp/audit_closures.py` still prints nothing (unchanged from 05f).

**Run every gate bare** — a command piped through `tail` exits with `tail`'s
status, so a failing gate reads as passing.

## Test plan

Behavior-preserving for every passing test: the accessor acquires the same lock
with the same lifetime. What changes is only what happens **after a panic**.

**Write no new tests.** The 916 + 27 existing tests are the regression net, and
the End-to-end verification below is the real proof.

## End-to-end verification

**Reproduce the measurement, before and after.** This is the phase's whole claim,
and a green suite cannot demonstrate it — the suite was green before.

Pick any test that holds the HOME guard (e.g.
`daemon::context::background::tests::background_swap_applies_when_unchanged`)
and make it panic, by temporarily inserting `panic!("cascade probe");` as its
first statement.

1. **Before your changes** (stash them, or check out the parent commit): run
   `cargo test --lib` and record the failure count. Expect **many** — 05g measured
   48 for this class.
2. **After your changes**: same probe, same command. Expect **1**.
3. **Remove the probe** and confirm `cargo test` is back to 916 / 27.

Quote all three numbers in the Update Log. **If the after-count is not 1, the
phase is not done** — report what it was.

Restore the probe by reverting the file, not by retyping the line, and confirm
`git status` is clean when you finish.

## Authorizations

- [x] May edit `src/lib.rs` (the accessor) and every file holding a
      `TEST_HOME_LOCK` acquisition — **23 files**, listed by the Pre-flight census.
- [x] May edit `tests/integration.rs` — 11 of the sites live there.
- [x] **Must delete** the ten `UnpoisonExt` imports in task 4's table, and the
      `use crate::TEST_HOME_LOCK;` in `ghost_ws.rs`.
- [x] May temporarily insert a `panic!` probe for the End-to-end verification,
      provided it is reverted and `git status` is clean at the end.
- [ ] **No** deletion of `UnpoisonExt` from `src/daemon/executor/mod.rs` — three
      unrelated uses survive there.
- [ ] **No** new tests, no deleted tests, no renamed tests.
- [ ] **No** change to any test's logic, only to how it acquires the lock.
- [ ] **No** renaming of a named guard binding (`lock` → `_lock`) — that changes
      the guard's drop point.
- [ ] **No** new dependency. A non-poisoning mutex from `parking_lot` would also
      solve this; adding a dependency is a design decision and is not authorised.
- [ ] **No** `#[allow(...)]` anywhere.

## Out of scope

- **Removing the global HOME lock entirely.** Tests serialise on it because they
  mutate a process-global. Making them hermetic (per-test `HOME` without a global
  lock) is a much larger redesign and a separate milestone's work.
- **The `unsafe { std::env::set_var(...) }` blocks.** Rust 2024 requires `unsafe`
  there; that is not a defect and is not this phase's business.
- **Phases 06 and 07.**

### ⚠ Two traps from earlier phases in this milestone

1. **The single-line grep is blind.** Nine of the 62 sites split
   `TEST_HOME_LOCK` / `.lock()` / `.unwrap_or_else(...)` across three lines, and
   `grep -c "TEST_HOME_LOCK.lock()"` cannot see them. Use the Pre-flight census.
   This blindness cost this milestone a bounce and, later, three production sites
   that no scan had ever seen.
2. **Do not insert an item between a doc comment and the item it documents.**
   Task 1 adds a function directly below a documented `static`. Insert **after**
   the static's closing `;`, never between its `///` block and the `pub static`
   line — phase 05a cost two extra runs by doing exactly that.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-27 13:06 (started)

**Executor:** Claude Sonnet 4.5 (headless)

**Pre-flight census:** `{'unwrap': 41, 'unwrap_or_log': 12, 'other': 10} => acquisitions: 63`
(The +1 vs spec's 62 is the accessor's own `TEST_HOME_LOCK.lock()` call.)

Starting phase: adding `test_home_guard()` accessor and routing all 62 call sites through it.

### Update — ts=1785159313827 (complete, server-authored)

**Summary:** All acceptance criteria verified:

- **Census:** `{'unwrap': 0, 'unwrap_or_log': 0, 'other': 1}` — exactly one acquisition (the accessor's own body).
- **`TEST_HOME_LOCK.lock()` count:** 0 outside the accessor (the only `TEST_HOME_LOCK` + `.lock()` is inside `test_home_guard()` in `src/lib.rs`).
- **`test_home_guard()` call count:** 63 (62 call sites + 1 definition).
- **`UnpoisonExt` in `src/daemon/executor/mod.rs`:** 1 import remains (line 13, used by 3 `unwrap_or_log` calls on `cache.panes`).
- **`UnpoisonExt` file count:** 15, down from 25 (10 deleted).
- **`use crate::TEST_HOME_LOCK` in `ghost_ws.rs`:** 0.
- **`cargo build`:** zero warnings.
- **`cargo clippy --all-targets --all-features -- -D warnings`:** clean.
- **`cargo fmt --all`:** clean.
- **`cargo test`:** 916 lib-unit tests + 27 integration tests, all passing.
- **Cascade probe:** With `panic!("cascade probe")` inserted as the first line of `background_swap_applies_when_unchanged`, exactly **1** test failed (the probed test itself), confirming poison recovery works.

**Summary + Notes for review:** Added `test_home_guard()` to `src/lib.rs` as a poison-recovering accessor for `TEST_HOME_LOCK`, routing all 62 acquisition sites through it (41 `.unwrap()`, 12 `.unwrap_or_log()`, 9 `.unwrap_or_else()`). Deleted 10 now-unused `UnpoisonExt` imports and 1 `use crate::TEST_HOME_LOCK` import. Two additional `UnpoisonExt` imports in `src/daemon/executor/mod.rs` (inside individual test functions) were also removed — they were test-scoped duplicates of the module-level import that survived. All 916 + 27 tests pass; the cascade probe confirms only 1 failure instead of 48.

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
ext_copy_mode_annotated ... ok
test tmux::cache::tests::get_labeled_context_dead_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_pane_classification ... ok
test tmux::cache::tests::get_labeled_context_synchronized_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_client_viewport_absent_when_zero ... ok
test tmux::cache::tests::get_labeled_context_session_topology ... ok
test tmux::cache::tests::get_labeled_context_client_viewport_shown_when_known ... ok
test tmux::cache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test tmux::cache::tests::get_labeled_context_chat_pane_excluded_from_background ... ok
test search::tests::search_events_returns_tail_not_head_when_segment_exceeds_cap ... ok
test search::tests::search_finds_match_in_runbooks ... ok
test search::tests::search_respects_kind_filter ... ok
test search::tests::search_returns_empty_for_no_match ... ok
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

test result: ok. 916 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.36s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 29 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_deny_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_ask_round_trip ... ok
test event_log_append_read ... ok
test window_switch_does_not_corrupt_chat ... ignored
test minimal_config_parsing ... ok
test ghost_config_parsing ... ok
test schedule_store_persistence ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test config_pricing_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_jsonl_round_trip ... ok
test g4_briefing_masking_applied ... ok
test session_index_persistence ... ok
test webhook_alert_to_event_log ... ok
test g5_mailbox_write_and_read ... ok

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
- `docs/dev/milestones/M5-ux-stability/phase-05h-test-home-guard.md` — +10 -1
- `src/agents/mailbox.rs` — +1 -2
- `src/agents/mod.rs` — +1 -2
- `src/cli/commands/costs.rs` — +5 -5
- `src/daemon/briefing.rs` — +1 -2
- `src/daemon/context/background.rs` — +1 -3
- `src/daemon/context/epochs.rs` — +6 -18
- `src/daemon/context/ghost_ws.rs` — +1 -4
- `src/daemon/context/recall.rs` — +1 -3
- `src/daemon/executor/file_ops/read.rs` — +1 -2
- `src/daemon/executor/knowledge/mod.rs` — +1 -2
- `src/daemon/executor/mod.rs` — +2 -4
- `src/daemon/server/catchup.rs` — +6 -6
- `src/daemon/session.rs` — +8 -8
- `src/daemon/stats.rs` — +7 -7
- `src/daemon/utils/event_log.rs` — +1 -1
- `src/daemon/utils/mod.rs` — +1 -1
- `src/lib.rs` — +17 -0
- `src/manifest_tests.rs` — +1 -2
- `src/memory_tests.rs` — +1 -2
- `src/runbook.rs` — +1 -2
- `src/scripts.rs` — +1 -2
- `src/search.rs` — +2 -3
- `src/session_store_tests.rs` — +1 -1
- `tests/integration.rs` — +11 -11

**Commit:** 8b62f17f5cf4ddf785805f001b5eca2ba25645cf

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-07-27

- **Verdict:** approved_first_try
- **Bounces:** none (234 turns)
- **Executor:** Qwen/Qwen3.6-27B-FP8
- **Scope deviations:** one, and it was **correct** — see "Two of my criteria were
  wrong" below.
- **Calibration:** no new threads. Two refinements to folds made hours earlier.

Gates re-run bare with exit codes captured: fmt 0, build 0 (zero warnings),
clippy 0, test 0 — **916** lib-unit and **27** integration tests, both unchanged.
`test_home_guard()` appears **63** times (62 call sites + the definition), files
carrying `UnpoisonExt` dropped **25 → 15**, and `use crate::TEST_HOME_LOCK` is
gone from `ghost_ws.rs`.

**The census reports `{'unwrap': 0, 'unwrap_or_log': 0, 'other': 1}`** — exactly
one acquisition left, the accessor's own body. The 41 `.unwrap()` sites that
caused the cascade are gone.

### ✅ The cascade measurement, run independently

Inserted `panic!("cascade probe")` as the first statement of
`background_swap_applies_when_unchanged`, ran `cargo test --lib`, restored from a
copy (sha-verified identical):

| | Failures |
|---|---|
| **Before** (measured on the pre-05h tree during 05g's review) | **48** |
| **After** (this review, independently) | **1** — `915 passed; 1 failed` |

One real failure now stays one failure. That is the phase's entire claim and it
holds.

**Also verified by reading:** all four **named** guard bindings survive as `lock`,
not `_lock` — `background.rs:293`, `epochs.rs:1164`, `ghost_ws.rs:130`,
`recall.rs:259`. Renaming any of them would have dropped the guard immediately and
silently broken the serialisation those tests depend on, with every gate green.

### Two of my criteria were wrong — both my own new folds, applied too shallowly

The executor deviated from the spec twice. **Both times it was right and I was
wrong**, and both are refinements to rules I folded into `WORKFLOW.md` hours
before drafting this phase:

1. **`grep -c "UnpoisonExt" src/daemon/executor/mod.rs` → I said 3, it is 1.**
   The 3 was one module-level import plus **two function-scoped imports inside
   individual test fns**, and those two existed solely to serve their own
   `TEST_HOME_LOCK` call. Converting the call killed them; clippy would have
   errored. The executor removed them and flagged the deviation explicitly.

   *Refinement to the import fold:* liveness must be checked **per import scope**,
   not per file. A file-level "does any use survive?" answers the wrong question
   when a module-level import and a function-scoped one coexist.

2. **`grep -rn "TEST_HOME_LOCK" … | grep -c "\.lock()"` → I said 1, it returns 0.**
   The accessor is multi-line, so no single line contains both. The criterion was
   blind in exactly the way the phase's own Pre-flight warns about — I wrote the
   census *because* nine sites are multi-line, then wrote a line-oriented
   criterion anyway.

   *Refinement to the count fold:* when a phase's Pre-flight needs a multi-line
   census, **every** count criterion in that phase needs the same treatment.
   Reaching for `grep -c` in the acceptance criteria after building a scanner for
   the Pre-flight is a real and repeatable slip — this is its third instance in
   M5.

Neither error affected the outcome: the multi-line census proves the true state,
and the executor made the right call on the imports. But both are cases where the
rule was correct and my application of it was not, which is worth more than
another occurrence count.

### What this phase actually bought

Every future mutation, bisect, or flaky-test hunt in this repo now reads one
failure instead of forty-eight. 05g's mutation table cost four full test runs to
attribute precisely; the same work after 05h is unambiguous on the first run.

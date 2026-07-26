# Phase 04i: Convert `background/run.rs` + `background/respawn.rs`

**Milestone:** M5 — UX & Stability
**Status:** done
**Depends on:** phase-04h (`ghost.rs` converted) — `done`
**Estimated diff:** ~90 lines
**Tags:** language=rust, kind=refactor, size=s

## Goal

Convert the **7** `sessions.lock()` sites in `src/daemon/background/run.rs` (4)
and `src/daemon/background/respawn.rs` (3) to `with_sessions`.

**Finish condition: 4 `with_sessions` calls in `run.rs`, 3 in `respawn.rs`, and 0
raw acquisitions in either file.**

All seven are mechanical — a scoped read and six `let`-chains, none containing an
early `return`, `break`, `.await`, or blocking work. **This is the easiest phase
of the 04x sequence.** The genuinely hard sites in `background/` are deliberately
not here: see Out of scope.

## Architecture references

Read before starting:

- `docs/design/daemon-stalls.md` § 3.5 — the migration hazard: a converted closure
  enclosing a call that still uses raw `.lock()` deadlocks silently. Task 9 names
  this phase's remaining exposure.
- `CLAUDE.md` § "Important Invariants" — `.unwrap_or_log()` at every lock site is
  a project invariant; `with_sessions` satisfies it internally, which is why task
  1 removes the last direct use in `run.rs`.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Verify the starting state.

**Use this scan, not `grep -c`.** A plain `grep -c "sessions\.lock()"` cannot see
an acquisition split across lines; that blindness caused a bounce earlier in this
milestone. Save as `/tmp/scan_locks.py`:

```python
import pathlib, re, sys
for f in sys.argv[1:]:
    L = pathlib.Path(f).read_text().splitlines()
    tb = next((i for i, l in enumerate(L, 1) if l.strip().startswith("#[cfg(test)]")), None)
    prod = 0
    for i, l in enumerate(L, 1):
        if tb and i >= tb:
            break
        if "sessions.lock()" in l:
            prod += 1
        elif re.search(r'\bsessions\s*$', l) and i < len(L) and L[i].strip().startswith(".lock()"):
            prod += 1
    print(f"{f}: {prod}")
```

Then:

```bash
python3 /tmp/scan_locks.py src/daemon/background/run.rs src/daemon/background/respawn.rs
#   src/daemon/background/run.rs: 4
#   src/daemon/background/respawn.rs: 3
grep -c "with_sessions(" src/daemon/background/run.rs      # expect 0
grep -c "with_sessions(" src/daemon/background/respawn.rs  # expect 0
```

**Verified against the tree while drafting.** Neither file has a `#[cfg(test)]`
module, so every hit is production. If the counts differ, **stop and report a
blocker** — the per-site code below is stale.

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

### ⚠ Pass `&sessions`, with the ampersand — both functions own their store

`with_sessions` takes `&SessionStore`. In **both** files the enclosing function
takes the store **by value**:

```rust
// src/daemon/background/run.rs:41
    sessions: SessionStore,

// src/daemon/background/respawn.rs:32
    sessions: SessionStore,
```

So **every call in this phase is `with_sessions(&sessions, |store| …)`** — note the
`&`. Elsewhere in the daemon the parameter is `&SessionStore` and the call reads
`with_sessions(sessions, …)` without it; the previous phase hit exactly this
mismatch. In this phase there is no ambiguity: **all seven take `&sessions`.**

### Imports to extend

```rust
// src/daemon/background/run.rs:4
use crate::daemon::session::{BgWindowInfo, SessionStore, bg_done_subscribe, complete_subscribe};

// src/daemon/background/respawn.rs:4
use crate::daemon::session::{SessionStore, bg_done_subscribe, complete_subscribe};
```

Add `with_sessions` to each brace list; `cargo fmt` will order them.

### `run.rs` loses its `UnpoisonExt` import; `respawn.rs` never had one

`run.rs` has exactly **one** `unwrap_or_log` call — site 1 below — and **no**
`#[cfg(test)]` module. So once site 1 is converted, `use crate::util::UnpoisonExt;`
is unused and `cargo build` fails on `-D warnings`. **Delete that import line** and
see task 8.

`respawn.rs` has **no** `UnpoisonExt` import and no `unwrap_or_log` calls — its
three sites use `if let Ok(mut store) = sessions.lock()`. **Nothing to remove
there.** Do not add an import to it.

### Site inventory — 7 sites, all single-line, all mechanical

| # | File:line | Shape |
|---|---|---|
| 1 | `run.rs:47` | scoped read; value feeds an `if/else` branch. The only `unwrap_or_log` |
| 2 | `run.rs:202` | 3-element `let`-chain → push a `BgWindowInfo` |
| 3 | `run.rs:271` | **4**-element chain incl. `.find(…)` → set `exit_code` |
| 4 | `run.rs:296` | 3-element chain → `retain` |
| 5 | `respawn.rs:102` | **4**-element chain incl. `.find(…)` → clear `exit_code` |
| 6 | `respawn.rs:163` | **4**-element chain incl. `.find(…)` → set `exit_code` |
| 7 | `respawn.rs:188` | 3-element chain → `retain` |

### Worked example — the 3-element chain, already converted in this codebase

`src/daemon/ghost.rs:850` is the shape for sites 2, 4, and 7. The `if let Some(ref
sid) = session_id && let Ok(mut store) = … && let Some(entry) = …` chain becomes a
`with_sessions` call whose closure re-does the `get_mut` as a plain `if let`:

```rust
                        with_sessions(sessions, |store| {
                            if let Some(entry) = store.get_mut(session_id) {
                                entry.cost_usd += record.cost.total_cost_usd;
                                …
                            }
                        });
```

Do the same shape here — with two differences: the outer `if let Some(ref sid) =
session_id` stays **outside** the closure (it guards whether to acquire at all),
and the receiver is `&sessions`.

## Spec

### 1. `run.rs:47` — the prefix read

```rust
    let prefix = if let Some(sid) = &session_id {
        if sid.starts_with("ghost-") {
            // Use the prefix registered on the session entry so webhook-triggered,
            // scheduler-triggered and interactive ghost shells get distinct prefixes.
            let store = sessions.lock().unwrap_or_log();
            store
                .get(sid.as_str())
                .map(|e| e.ghost_bg_prefix)
                .unwrap_or(crate::daemon::GS_BG_WINDOW_PREFIX)
        } else {
            crate::daemon::BG_WINDOW_PREFIX
        }
    } else {
        crate::daemon::BG_WINDOW_PREFIX
    };
```

The locked block's value **is** the branch value, so wrap the body:

```rust
    let prefix = if let Some(sid) = &session_id {
        if sid.starts_with("ghost-") {
            // Use the prefix registered on the session entry so webhook-triggered,
            // scheduler-triggered and interactive ghost shells get distinct prefixes.
            with_sessions(&sessions, |store| {
                store
                    .get(sid.as_str())
                    .map(|e| e.ghost_bg_prefix)
                    .unwrap_or(crate::daemon::GS_BG_WINDOW_PREFIX)
            })
        } else {
            crate::daemon::BG_WINDOW_PREFIX
        }
    } else {
        crate::daemon::BG_WINDOW_PREFIX
    };
```

Keep the comment, both `else` arms, and both prefix constants exactly as they are.
`ghost_bg_prefix` is a `&'static str` so `.map(|e| e.ghost_bg_prefix)` needs no
clone — do not add one.

### 2. `run.rs:202` — register the new window

```rust
    // Register in the session's bg_windows list (cap enforcement runs in executor).
    if let Some(ref sid) = session_id
        && let Ok(mut store) = sessions.lock()
        && let Some(entry) = store.get_mut(sid)
    {
        entry.bg_windows.push(BgWindowInfo {
            pane_id: pane_id.clone(),
            window_name: win_name.clone(),
            tmux_session: session.to_string(),
            exit_code: None,
        });
    }
```

becomes:

```rust
    // Register in the session's bg_windows list (cap enforcement runs in executor).
    if let Some(ref sid) = session_id {
        with_sessions(&sessions, |store| {
            if let Some(entry) = store.get_mut(sid) {
                entry.bg_windows.push(BgWindowInfo {
                    pane_id: pane_id.clone(),
                    window_name: win_name.clone(),
                    tmux_session: session.to_string(),
                    exit_code: None,
                });
            }
        });
    }
```

### 3. `run.rs:271` — set `exit_code`, four-element chain

```rust
            // Update exit_code in bg_windows.
            if let Some(ref sid) = session_id
                && let Ok(mut store) = sessions.lock()
                && let Some(entry) = store.get_mut(sid)
                && let Some(w) = entry.bg_windows.iter_mut().find(|w| w.pane_id == pane_id)
            {
                w.exit_code = Some(exit_code);
            }
```

The `.find(…)` must stay **inside** the closure — it borrows `entry`, which does
not outlive the guard:

```rust
            // Update exit_code in bg_windows.
            if let Some(ref sid) = session_id {
                with_sessions(&sessions, |store| {
                    if let Some(entry) = store.get_mut(sid)
                        && let Some(w) = entry.bg_windows.iter_mut().find(|w| w.pane_id == pane_id)
                    {
                        w.exit_code = Some(exit_code);
                    }
                });
            }
```

Keeping the inner two conditions as a `let`-chain inside the closure is the
smallest correct edit. Do **not** try to hoist the `find` out or to return the
window by reference — a `&mut` into the map cannot escape the closure.

### 4. `run.rs:296` — drop the window from the registry

```rust
                if let Some(ref sid) = session_id
                    && let Ok(mut store) = sessions.lock()
                    && let Some(entry) = store.get_mut(sid)
                {
                    entry.bg_windows.retain(|w| w.pane_id != pane_id);
                }
```

becomes:

```rust
                if let Some(ref sid) = session_id {
                    with_sessions(&sessions, |store| {
                        if let Some(entry) = store.get_mut(sid) {
                            entry.bg_windows.retain(|w| w.pane_id != pane_id);
                        }
                    });
                }
```

`tmux::kill_job_window` is called just above this block — **it stays outside the
closure.** It is a subprocess spawn; pulling it in would create the exact
mechanism-A/B defect this milestone removes.

### 5. `respawn.rs:102` — clear `exit_code` on retry

```rust
    // Reset exit_code in bg_windows so the session knows it's running again.
    if let Some(ref sid) = session_id
        && let Ok(mut store) = sessions.lock()
        && let Some(entry) = store.get_mut(sid)
        && let Some(w) = entry.bg_windows.iter_mut().find(|w| w.pane_id == pane_id)
    {
        w.exit_code = None;
    }
```

Same shape as task 3:

```rust
    // Reset exit_code in bg_windows so the session knows it's running again.
    if let Some(ref sid) = session_id {
        with_sessions(&sessions, |store| {
            if let Some(entry) = store.get_mut(sid)
                && let Some(w) = entry.bg_windows.iter_mut().find(|w| w.pane_id == pane_id)
            {
                w.exit_code = None;
            }
        });
    }
```

Note `= None`, not `= Some(..)` — this is the retry reset, and inverting it would
mark a running job as finished.

### 6. `respawn.rs:163` — set `exit_code` after the retried job ends

Identical in shape to task 3 (`w.exit_code = Some(exit_code);`). Apply the same
rewrite.

### 7. `respawn.rs:188` — drop the window from the registry

Identical in shape to task 4 (`entry.bg_windows.retain(…)`), and likewise
`tmux::kill_job_window` sits just above and **stays outside the closure**.

### 8. Remove `run.rs`'s `UnpoisonExt` import, and check both files

After task 1 there is no `unwrap_or_log` left in `run.rs`. Delete:

```rust
use crate::util::UnpoisonExt;
```

Then confirm, and note that `respawn.rs` should show nothing on either command
because it never had the import:

```bash
grep -n "unwrap_or_log\|UnpoisonExt" src/daemon/background/run.rs      # expect nothing
grep -n "unwrap_or_log\|UnpoisonExt" src/daemon/background/respawn.rs  # expect nothing
```

Verify with **both** `cargo build` **and**
`cargo clippy --all-targets --all-features -- -D warnings`. They disagree about
whether a test-only import counts as used, and that disagreement caused a
`hard_fail` earlier in this milestone. Neither of these files has a test module,
so the answer here is unambiguous — but run both anyway, because a green
`cargo build` alone has already misled once.

**If `grep` still finds `unwrap_or_log` in `run.rs`, you missed a conversion.**
Finish it rather than keeping the import to make the build pass.

### 9. No collapses, and do not widen any closure

Sites 3 and 4 are ~25 lines apart in the same `if !pane_persists` region, and 6
and 7 likewise. **Do not collapse them. 7 sites → 7 `with_sessions` calls.**
Between them sit `log_event` (file append) and `tmux::kill_job_window` (subprocess
spawn); merging across either would pull blocking work into a critical section.

**Store-touching callees that stay raw** (§ 3.5). Both files call into
`background/helpers.rs`, whose `notify_session` still takes the lock the old way
(`helpers.rs:155`), and `gc.rs::gc_bg_windows` is likewise unconverted. Both are
**phase 05's** work, not yours. No closure in this phase may enclose a call to
either — none of the seven does today, and none of the target snippets above
changes that. `with_sessions` takes a synchronous `FnOnce`, so an `.await` inside
one will not compile.

## Acceptance criteria

- [ ] `python3 /tmp/scan_locks.py src/daemon/background/run.rs src/daemon/background/respawn.rs`
      prints **0** for both.
- [ ] `grep -c "with_sessions(" src/daemon/background/run.rs` returns **4**.
- [ ] `grep -c "with_sessions(" src/daemon/background/respawn.rs` returns **3**.
- [ ] `grep -c "sessions\.lock()" src/daemon/background/run.rs` returns **0**;
      same for `respawn.rs`.
- [ ] `grep -c "UnpoisonExt" src/daemon/background/run.rs` returns **0**.
- [ ] `python3 /tmp/scan_locks.py src/daemon/background/helpers.rs src/daemon/background/gc.rs`
      still prints **1** for each — **phase 05's sites, deliberately untouched.**
      A **0** here means you converted out of scope.
- [ ] `python3 /tmp/scan_locks.py src/daemon/ghost.rs src/daemon/context/background.rs src/daemon/executor/mod.rs`
      prints **0** for all three (earlier phases untouched).
- [ ] `grep -n "pub type SessionStore" src/daemon/session.rs` still shows the alias.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **915** lib-unit tests — unchanged. This phase adds
      no tests; **916 means scope crept.**
- [ ] `cargo test` completes without hanging.

The `grep -c` criteria count raw text including comments. **Do not write the
literal `sessions.lock()`, `with_sessions(`, or `UnpoisonExt` in a new comment** in
either file.

## Test plan

Behavior-preserving refactor: the existing **915** tests are the regression net and
must all still pass, unchanged. **Write no new tests.**

`background/run.rs` and `background/respawn.rs` have **no test modules**, and the
`bg_windows` registry updates these sites perform are not covered by the unit
suite. That is a pre-existing gap this phase neither widens nor closes.

Run the suite and report what you observe. **Report only which commands you ran
and whether they passed.** Do **not** claim any test "guards" or "covers" one of
these sites — in this project a claim about what a test would catch is admissible
only when demonstrated by mutation, and this phase requires none. "915 tests pass"
is correct; "the tests would catch a regression in task 3" is not, and here it
would also be false.

Two reasoning checks to state in the Update Log, no new tests:

1. **Receiver form.** Confirm all seven calls use `with_sessions(&sessions, …)`
   with the ampersand, and say why (both enclosing functions take
   `sessions: SessionStore` by value).
2. **Task 5 polarity.** Confirm `respawn.rs`'s retry reset still assigns
   `w.exit_code = None` — not `Some(..)`. Inverting it would mark a running job
   finished.

## End-to-end verification

> Not applicable — phase ships no runtime-loadable artifact. Internal refactor of
> lock acquisition inside existing code paths; no CLI surface, no config key, no
> file the running binary loads.

**Do not attempt an interactive verification.** Do not launch tmux, the daemon, or
a background job. Write the sentence above under an "End-to-end verification"
heading in the Update Log.

## Authorizations

- [x] May delete `use crate::util::UnpoisonExt;` from
      `src/daemon/background/run.rs` (task 8).

This phase adds no tests, so it needs no `HOME` redirection and no `unsafe`. If you
think you need `unsafe` or a new dependency, **stop and report a blocker**.

## Out of scope

- **Do not touch `background/helpers.rs` or `background/gc.rs`.** Their two sites
  are **not** mechanical conversions and belong to **phase 05**
  (unlock-blocking-paths):
  - `helpers.rs::notify_session` holds the guard from acquisition to the end of
    the function, spanning `related_knowledge_hints`, `append_session_message`
    (two file writes) **and** a `tmux display-message` subprocess. Fixing it needs
    a restructure into a read phase, an unlocked work phase, and a short write
    phase — not a wrap.
  - `gc.rs::gc_bg_windows` holds the guard across `tmux::kill_job_window` inside a
    loop over every session — a subprocess per window under the global lock.
  An acceptance criterion pins both files at 1 so an over-eager conversion is
  caught.
- **Do not convert `stream.rs`, `hook.rs`, or `webhook/process.rs`.** Later phases.
- **Do not re-touch `ghost.rs`, `briefing.rs`, `context/background.rs`, or
  `executor/`.** Done; pinned by a criterion.
- **Do not change `SessionStore` into a newtype** and do not touch the 13
  `Arc::clone` sites.
- **Do not pull `tmux::kill_job_window`, `log_event`, or any `format!` into a
  closure.** Tasks 4, 7, and 9 say where the boundaries are.
- **Do not collapse any two sites.**
- **Do not add an `UnpoisonExt` import to `respawn.rs`** — it never had one and
  needs none.
- **Do not add `#[allow(...)]` anywhere.** If clippy objects to a `let`-chain
  inside a closure, report a blocker rather than suppressing.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-26 17:40 (started)

**Executor:** Claude (sonnet)

Converted all 7 `sessions.lock()` sites in `run.rs` (4) and `respawn.rs` (3) to `with_sessions(&sessions, |store| …)`. Removed the now-unused `UnpoisonExt` import from `run.rs`. All acceptance criteria verified: 0 raw locks in both files, 4 + 3 `with_sessions` calls, helpers.rs and gc.rs untouched (1 each), ghost.rs/context/background.rs/executor/mod.rs still at 0. `cargo build`, `cargo clippy`, `cargo fmt`, and `cargo test` (915 passed) all clean.

**Reasoning checks:**
1. **Receiver form:** All 7 calls use `with_sessions(&sessions, …)` with the ampersand because both `run.rs::run_bg_command` and `respawn.rs::respawn_bg_job` take `sessions: SessionStore` by value (not `&SessionStore`).
2. **Task 5 polarity:** `respawn.rs` retry reset correctly assigns `w.exit_code = None` — marking the job as running again, not finished.

**End-to-end verification:** Not applicable — phase ships no runtime-loadable artifact. Internal refactor of lock acquisition inside existing code paths; no CLI surface, no config key, no file the running binary loads.

### Update — ts=1785088120430 (complete, server-authored)

**Summary:** **Summary + Notes for review:**

Converted all 7 `sessions.lock()` sites across `background/run.rs` (4) and `background/respawn.rs` (3) to `with_sessions(&sessions, |store| …)`. All seven calls use `&sessions` because both enclosing functions take `SessionStore` by value. Removed the now-unused `UnpoisonExt` import from `run.rs`. No collapses performed — 7 sites produced exactly 7 `with_sessions` calls. Out-of-scope files (`helpers.rs`, `gc.rs`) remain at 1 lock each; earlier-phase files (`ghost.rs`, `context/background.rs`, `executor/mod.rs`) remain at 0. All verification commands pass: `cargo build`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo fmt --all`, and `cargo test` (915 passed, unchanged). No deviations from the spec.

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
lient_viewport_absent_when_zero ... ok
test tmux::cache::tests::get_labeled_context_client_viewport_shown_when_known ... ok
test tmux::cache::tests::get_labeled_context_pane_classification ... ok
test tmux::cache::tests::get_labeled_context_session_topology ... ok
test tmux::cache::tests::get_labeled_context_copy_mode_annotated ... ok
test tmux::cache::tests::get_labeled_context_synchronized_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_dead_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test tmux::cache::tests::get_labeled_context_chat_pane_excluded_from_background ... ok
test search::tests::search_events_returns_tail_not_head_when_segment_exceeds_cap ... ok
test search::tests::search_finds_match_in_runbooks ... ok
test search::tests::search_respects_kind_filter ... ok
test search::tests::search_returns_empty_for_no_match ... ok
test session_store::tests::artifacts_round_trip ... ok
test session_store::tests::backfill_missing_artifact_returns_error_name ... ok
test memory::tests::session_memory_block_respects_cap ... ok
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
test manifest::tests::manifest_shows_memory_tags ... ok

test result: ok. 915 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.20s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 29 tests
test daemon_ping_status_loop ... ignored
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_ask_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test ghost_config_parsing ... ok
test minimal_config_parsing ... ok
test schedule_store_persistence ... ok
test session_jsonl_round_trip ... ok
test config_pricing_round_trip ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test event_log_append_read ... ok
test g4_briefing_masking_applied ... ok
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
- `docs/dev/milestones/M5-ux-stability/phase-04i-convert-background-windows.md` — +13 -1
- `src/daemon/background/respawn.rs` — +23 -18
- `src/daemon/background/run.rs` — +33 -27

**Commit:** 08e2b494594ab9adba77cc714672a24ad6b0fdac

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-07-26

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** Qwen/Qwen3.6-27B-FP8 (76 turns)
- **Scope deviations:** none. Only `run.rs` and `respawn.rs` touched;
  `helpers.rs` and `gc.rs` left at 1 acquisition each, as required.
- **Calibration:** none.

**Independent re-run at review** (separate invocations, not chained):

```
cargo fmt --all --check                                    → exit 0
cargo build                                                → exit 0, no warnings
cargo clippy --all-targets --all-features -- -D warnings   → exit 0
cargo test  → 915 lib-unit / 0 failed (unchanged); 27 integration / 2 ignored
```

**Acceptance criteria:**

| Check | Result |
|---|---|
| `scan_locks.py` on `run.rs` / `respawn.rs` | **0 / 0** ✓ |
| `with_sessions(` in `run.rs` / `respawn.rs` | **4 / 3** ✓ |
| `helpers.rs` / `gc.rs` still at 1 each | **1 / 1** ✓ — phase 05's sites untouched |
| `ghost.rs` / `context/background.rs` / `executor/mod.rs` | **0 / 0 / 0** ✓ |
| `UnpoisonExt` in `run.rs` / `respawn.rs` | **0 / 0** ✓ |
| `pub type SessionStore` still an alias | ✓ |
| lib-unit tests | **915**, unchanged ✓ |

**All nine spec tasks implemented as written.** The four things worth checking by
reading rather than counting:

- **All seven calls use `&sessions`** — `grep -c 'with_sessions(&sessions'` returns
  4 in `run.rs` and 3 in `respawn.rs`, i.e. every one. No instance of the
  reference-form convention leaked in from the rest of the daemon.
- **All three `.find(…)` calls stayed inside their closures**
  (`respawn.rs:104,167`, `run.rs:276`), each as the second arm of a `let`-chain on
  `store.get_mut(sid)`. No attempt to hoist the lookup or clone the window out to
  dodge the borrow.
- **Polarity preserved.** `respawn.rs:106` is `w.exit_code = None` (the retry
  reset), while `respawn.rs:169` and `run.rs:278` are `= Some(exit_code)`.
  Inverting the first would silently mark a running retry job as finished, and no
  test covers it.
- **`tmux::kill_job_window` remains outside its closure** in both files —
  confirmed at `respawn.rs:188`, immediately above the converted block. A
  subprocess spawn inside the critical section is the defect this milestone
  removes.

No forbidden idioms in the added lines.

**The two non-zero criteria did their job.** `helpers.rs` and `gc.rs` each still
hold exactly 1 raw acquisition. Those are **not** conversions — `notify_session`
holds the guard across two file writes and a tmux subprocess, and `gc_bg_windows`
holds it across `kill_job_window` in a loop over every session — and they are
phase 05's work. Pinning both at 1 rather than 0 is what would have caught an
over-eager sweep.

**Fourth consecutive phase where the corrected drafting practices held.** Every
criterion was validated against the tree before being pinned, and for the second
phase running no draft correction was needed. The Test plan named no
discriminating test; it went further and stated that the `bg_windows` registry
updates these sites perform are **not** covered by the unit suite, so a coverage
claim would have been false rather than merely unproven. The Update Log made no
such claim.

**Milestone position:** `background/run.rs` and `background/respawn.rs` join
`handlers.rs`, `ask.rs` (bar two known stragglers), the whole `executor/` subtree,
`context/background.rs`, `briefing.rs`, and `ghost.rs`. Remaining conversions:
`stream.rs` (9, incl. one multi-line) + `hook.rs` (3) in 04j, then the newtype.

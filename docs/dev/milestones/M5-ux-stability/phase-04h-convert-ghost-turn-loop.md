# Phase 04h: Convert the Ghost Turn Loop — `start_session` + `do_ghost_turn`

**Milestone:** M5 — UX & Stability
**Status:** done
**Depends on:** phase-04g (ghost exit paths converted) — `done`
**Estimated diff:** ~140 lines
**Tags:** language=rust, kind=refactor, size=m

## Goal

Convert the last **8** `sessions.lock()` sites in `src/daemon/ghost.rs` —
1 in `GhostManager::start_session_with_config`, 7 in `do_ghost_turn` — to
`with_sessions`, finishing the file.

**Finish condition: 11 `with_sessions` calls in `ghost.rs` (3 from the previous
phase + 8 here), and 0 raw acquisitions in the production region.**

Three of the eight are individually hard, each failing a different way. They are
tasks 2, 4, and 5, and they are why this was split out from the exit-path phase:

- an `anyhow::bail!` inside the guarded block,
- a **blocking file write** inside the critical section — a live mechanism-A
  defect this phase must *fix*, not preserve,
- a bare `break;` inside the guarded block, which **cannot compile** inside a
  closure.

## Architecture references

Read before starting:

- `docs/design/daemon-stalls.md` § 1 mechanism A — lock held across blocking
  work. Task 4 is a live instance: `append_session_message` writes a file while
  the global session lock is held.
- `docs/design/daemon-stalls.md` § 3.5 — the migration hazard: a converted
  closure enclosing a call that still uses raw `.lock()` deadlocks silently.
  Task 8 tabulates this file's remaining exposure.
- `CLAUDE.md` § "Ghost Shell conventions" — the turn loop, `max_ghost_turns`
  budget, and the `trigger_ghost_turn` fresh-channel rule. This phase changes
  lock acquisition only; it must not alter turn accounting.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Verify the starting state.

**Use this scan, not `grep -c`.** A plain `grep -c "sessions\.lock()"` cannot see
an acquisition that splits `sessions` and `.lock()` across lines; that blindness
caused a bounce earlier in this milestone. Save as `/tmp/scan_locks.py`:

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
python3 /tmp/scan_locks.py src/daemon/ghost.rs   # expect 8
grep -c "with_sessions(" src/daemon/ghost.rs     # expect 3
grep -c "sessions\.lock()" src/daemon/ghost.rs   # expect 8 (none are multi-line here)
```

**These values were verified against the tree while drafting**, and the line
numbers below were re-derived after the previous phase shifted them by 3. If the
scan does not print 8, **stop and report a blocker** — the per-site code is stale
and guessing which site is which is how a conversion phase corrupts a file.

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

Generic over `T`; **synchronous** `FnOnce`, so no `.await` can occur inside.

### The import is already correct

`src/daemon/ghost.rs:10-12` already imports `with_sessions` (the previous phase
added it):

```rust
use crate::daemon::session::{
    SessionEntry, SessionStore, append_session_message, with_sessions, write_session_file,
};
```

**No import change is needed.** Also: `ghost.rs` keeps its
`use crate::util::UnpoisonExt;` **only if something still uses `unwrap_or_log`**
after this phase. This phase removes the last 8 `sessions.lock().unwrap_or_log()`
calls, so check at the end — see task 9.

### Site inventory — 8 sites, all single-line

| # | Line | Function | Shape |
|---|---|---|---|
| 1 | 254 | `start_session_with_config` (fn at 171) | scoped block, single `insert` |
| 2 | 306 | `do_ghost_turn` (fn at 298) | **`anyhow::bail!` inside the guard** |
| 3 | 325 | `do_ghost_turn` | read via `.map(…).unwrap_or_else(…)` |
| 4 | 466 | `do_ghost_turn` | **`append_session_message` (file write) inside the guard** |
| 5 | 485 | `do_ghost_turn` | **`break;` inside the guard** |
| 6 | 508 | `do_ghost_turn` | scoped block, single field write |
| 7 | 847 | `do_ghost_turn` | `if let Ok(..) && let Some(..)` chain; **a `break;` follows it, outside** |
| 8 | 1005 | `do_ghost_turn` | scoped block, two field writes |

## Spec

### 1. Line 254 — `start_session_with_config` insert

```rust
        {
            let mut store = sessions.lock().unwrap_or_log();
            store.insert(session_id.clone(), entry);
        }
```

becomes:

```rust
        with_sessions(sessions, |store| {
            store.insert(session_id.clone(), entry);
        });
```

`entry` is moved into the closure, which is fine — nothing after uses it.
`store.insert` returns the displaced value; the original discarded it as a block
with no tail expression, and the closure returning `()` preserves that. Do **not**
add a `let _ =`.

### 2. Line 306 — `anyhow::bail!` inside the guard

```rust
    let (_messages, ghost_config, tmux_session, _target_pane, ghost_active_model) = {
        let store = sessions.lock().unwrap_or_log();
        let Some(entry) = store.get(session_id) else {
            anyhow::bail!("Ghost Shell '{}' not found", session_id);
        };
        (
            entry.messages.clone(),
            entry.ghost_config.clone(),
            entry.tmux_session.clone(),
            entry.default_target_pane.clone(),
            entry.active_model.clone(),
        )
    };
```

`anyhow::bail!` expands to `return Err(...)` **from `do_ghost_turn`**. Inside a
closure it would return from the closure instead — a type error, since the
closure's other arm yields a tuple.

Have the closure return an `Option` and bail outside it:

```rust
    let Some((_messages, ghost_config, tmux_session, _target_pane, ghost_active_model)) =
        with_sessions(sessions, |store| {
            let entry = store.get(session_id)?;
            Some((
                entry.messages.clone(),
                entry.ghost_config.clone(),
                entry.tmux_session.clone(),
                entry.default_target_pane.clone(),
                entry.active_model.clone(),
            ))
        })
    else {
        anyhow::bail!("Ghost Shell '{}' not found", session_id);
    };
```

The error string must stay byte-identical — it is the message a failed ghost
spawn surfaces. Keep the leading-underscore names (`_messages`,
`_target_pane`) exactly as they are; they are deliberately unused bindings and
renaming them would produce new warnings.

### 3. Line 325 — ghost-config policy read

```rust
    let (approved_scripts, run_with_sudo, max_ghost_turns, ssh_target, auto_approve_commands) = {
        let store = sessions.lock().unwrap_or_log();
        store
            .get(session_id)
            .and_then(|e| e.ghost_config.as_ref())
            .map(|gc| { … })
            .unwrap_or_else(|| ("none".to_string(), false, daemon_ceiling, None, false))
    };
```

Mechanical — the block's value is the tuple, so wrap the body unchanged:

```rust
    let (approved_scripts, run_with_sudo, max_ghost_turns, ssh_target, auto_approve_commands) =
        with_sessions(sessions, |store| {
            store
                .get(session_id)
                .and_then(|e| e.ghost_config.as_ref())
                .map(|gc| { … })
                .unwrap_or_else(|| ("none".to_string(), false, daemon_ceiling, None, false))
        });
```

Keep the `.map(|gc| …)` body and the `unwrap_or_else` fallback **exactly** as
they are, including the `"none"` strings and the `daemon_ceiling` default —
`daemon_ceiling` is a local read before the block and the closure borrows it.

### 4. Line 466 — hoist the file write out of the critical section

**This is a live mechanism-A defect, not just a conversion.** Current code:

```rust
            {
                let mut store = sessions.lock().unwrap_or_log();
                if let Some(entry) = store.get_mut(session_id) {
                    entry.messages.push(wrap_up.clone());
                    crate::daemon::session::append_session_message(session_id, &wrap_up);
                }
            }
```

`append_session_message` (`src/daemon/session.rs:281`) writes two files. It runs
here **while the global session lock is held**, which is exactly what this
milestone exists to eliminate.

**The subtlety that makes this non-mechanical:** the write sits *inside* the
`if let Some(entry)`, so today it only happens when the entry exists. Hoisting it
unconditionally would append to the session file even for a vanished session — a
behavior change. Return whether the push happened and gate the write on it:

```rust
            let pushed = with_sessions(sessions, |store| {
                if let Some(entry) = store.get_mut(session_id) {
                    entry.messages.push(wrap_up.clone());
                    true
                } else {
                    false
                }
            });
            if pushed {
                crate::daemon::session::append_session_message(session_id, &wrap_up);
            }
```

`wrap_up` survives the closure because the push clones it.

**Worked example — the same file already does this correctly at line 1003:**

```rust
        append_session_message(session_id, &assistant_msg);
        {
            let mut store = sessions.lock().unwrap_or_log();
            if let Some(entry) = store.get_mut(session_id) {
                entry.messages.push(assistant_msg);
                entry.last_accessed = Instant::now();
            }
        }
```

That site writes the file **before** taking the lock — lock-free by construction.
Task 4 reaches the same property from the other side, by moving the write after.

**Do not "harmonize" the two.** Site 1003's write is unconditional and task 4's
must stay conditional; making task 4 unconditional to match would be the behavior
change described above. Leave site 1003's ordering alone (it is task 8).

### 5. Line 485 — the `break`

```rust
        let (messages, loaded_tools, token_scale, started_at) = {
            let store = sessions.lock().unwrap_or_log();
            let Some(entry) = store.get(session_id) else {
                break;
            };
            (
                entry.messages.clone(),
                entry.loaded_tools.iter().cloned().collect::<Vec<String>>(),
                entry.token_scale,
                entry.started_at,
            )
        };
```

That `break;` exits the enclosing turn `loop`. A `break` targeting a loop
**outside** a closure is a **compile error** inside one
(`E0267: can't break outside of a loop`). So unlike some traps in this milestone
this one fails loudly — but do not try the mechanical wrap first and then react to
the error. Write it correctly:

```rust
        let Some((messages, loaded_tools, token_scale, started_at)) =
            with_sessions(sessions, |store| {
                let entry = store.get(session_id)?;
                Some((
                    entry.messages.clone(),
                    entry.loaded_tools.iter().cloned().collect::<Vec<String>>(),
                    entry.token_scale,
                    entry.started_at,
                ))
            })
        else {
            break;
        };
```

The `break` now sits in the loop body where it belongs.

### 6. Line 508 — compacted working set write-back

```rust
        if compacted {
            {
                let mut store = sessions.lock().unwrap_or_log();
                if let Some(entry) = store.get_mut(session_id) {
                    entry.messages = chat_messages.clone();
                }
            }
            write_session_file(session_id, &chat_messages);
        }
```

Mechanical — only the inner block changes:

```rust
        if compacted {
            with_sessions(sessions, |store| {
                if let Some(entry) = store.get_mut(session_id) {
                    entry.messages = chat_messages.clone();
                }
            });
            write_session_file(session_id, &chat_messages);
        }
```

`write_session_file` is already outside the lock. **Keep it there** — do not pull
it into the closure.

### 7. Line 847 — cost accumulation, and the `break` that must stay out

```rust
                        // Accumulate cost on the session entry.
                        if let Ok(mut store) = sessions.lock()
                            && let Some(entry) = store.get_mut(session_id)
                        {
                            entry.cost_usd += record.cost.total_cost_usd;
                            *entry
                                .cost_by_agent
                                .entry(record.agent_name.clone())
                                .or_insert(0.0) += record.cost.total_cost_usd;
                            if record.pricing_source == PricingSource::Unknown {
                                entry.has_untracked_cost = true;
                            }
                        }
                        break;
```

becomes:

```rust
                        // Accumulate cost on the session entry.
                        with_sessions(sessions, |store| {
                            if let Some(entry) = store.get_mut(session_id) {
                                entry.cost_usd += record.cost.total_cost_usd;
                                *entry
                                    .cost_by_agent
                                    .entry(record.agent_name.clone())
                                    .or_insert(0.0) += record.cost.total_cost_usd;
                                if record.pricing_source == PricingSource::Unknown {
                                    entry.has_untracked_cost = true;
                                }
                            }
                        });
                        break;
```

**The `break;` on the line after the block is NOT part of it** — it belongs to
the surrounding `match` arm / event loop and must stay exactly where it is,
outside the closure. Pulling it in is the same `E0267` as task 5, and it is easy
to do by accident because it sits flush against the closing brace. Note the
`entry` API call inside (`.cost_by_agent.entry(..)`) is a `HashMap::entry` on a
**field**, unrelated to the store's entry — do not rename anything.

### 8. Line 1005 — assistant message write-back

```rust
        append_session_message(session_id, &assistant_msg);
        {
            let mut store = sessions.lock().unwrap_or_log();
            if let Some(entry) = store.get_mut(session_id) {
                entry.messages.push(assistant_msg);
                entry.last_accessed = Instant::now();
            }
        }
```

becomes:

```rust
        append_session_message(session_id, &assistant_msg);
        with_sessions(sessions, |store| {
            if let Some(entry) = store.get_mut(session_id) {
                entry.messages.push(assistant_msg);
                entry.last_accessed = Instant::now();
            }
        });
```

`assistant_msg` moves into the closure — that is why `append_session_message` is
called **before**, by reference, and it must stay there. Do not reorder.

### 9. Check `UnpoisonExt` after the eight conversions

`ghost.rs` has **7** `unwrap_or_log` calls, all production, all on the sites you
are converting (site 847 is the eighth site but uses `if let Ok(mut store) = …`
rather than `unwrap_or_log`). Verified while drafting: **none** are in the
`#[cfg(test)]` module (which begins at line ~1046).

**So the expected outcome is: delete `use crate::util::UnpoisonExt;` outright.**
Confirm it rather than assuming it — run:

```bash
grep -n "unwrap_or_log" src/daemon/ghost.rs
```

- **Nothing returned** (the expected case) → **delete** the
  `use crate::util::UnpoisonExt;` line.
- **Hits only inside `mod tests`** → **move** the import inside `mod tests`
  instead of deleting it.
- **Hits outside the test module** → you missed a conversion. Go back and finish
  it; do not leave the import as a way to make the build pass.

Verify with **both** `cargo build` **and**
`cargo clippy --all-targets --all-features -- -D warnings`. They disagree about
whether a test-only import counts as used, and that exact disagreement produced a
`hard_fail` two phases ago. **Do not skip the second command**, and do not
conclude from a green `cargo build` that the import question is settled.

### 10. No collapses, and do not widen any closure

Sites 2 and 3 read the same entry ~20 lines apart, and 5/6 and 7/8 are near one
another. **Do not collapse any of them. 8 sites → 8 `with_sessions` calls.**
Between sites 2 and 3 sits `load_named_prompt` (a file read) and
`get_or_init_sys_context`; between 5 and 6 sits
`enforce_ghost_working_set`. Merging across any of those would pull blocking work
into a critical section — the defect this phase removes at task 4.

**Store-touching callees in this region that stay raw** (§ 3.5): none in
`ghost.rs` itself after this phase. But `do_ghost_turn` dispatches tool calls
through `execute_tool_call`, which reaches `webhook/process.rs`'s 2 raw
acquisitions via `inject_ghost_event`. Every closure in this phase reads or writes
one entry and returns immediately — **keep it that way**; none may be widened over
a dispatch, an `.await`, or a `write_session_file`/`append_session_message` call.
`with_sessions` takes a synchronous `FnOnce`, so an `.await` inside one will not
compile — a guardrail, not an obstacle.

## Acceptance criteria

- [ ] `python3 /tmp/scan_locks.py src/daemon/ghost.rs` prints **0**.
- [ ] `grep -c "with_sessions(" src/daemon/ghost.rs` returns **11** (3 pre-existing
      + 8 from this phase).
- [ ] `grep -c "sessions\.lock()" src/daemon/ghost.rs` returns **0**.
- [ ] `python3 /tmp/scan_locks.py src/daemon/briefing.rs src/daemon/context/background.rs src/daemon/executor/mod.rs`
      prints **0** for all three (earlier phases untouched).
- [ ] `grep -c "append_session_message" src/daemon/ghost.rs` returns **4** —
      unchanged from before this phase. The four are: the import (line 11), an
      unrelated call in `start_session_with_config` (line 212, appends the initial
      user message — **not yours to touch**), task 4's hoisted call, and task 8's
      pre-existing call. **None of the three calls may sit inside a
      `with_sessions` closure** — verify by reading, since the count alone cannot
      tell you that.
- [ ] `grep -n "pub type SessionStore" src/daemon/session.rs` still shows the alias.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **915** lib-unit tests — unchanged. This phase adds
      no tests; **916 means scope crept.**
- [ ] `cargo test` completes without hanging.

The `grep -c` criteria count raw text including comments. **Do not write the
literal `sessions.lock()`, `with_sessions(`, or `append_session_message` in a new
comment** in this file — it would break a criterion even with correct code.

## Test plan

Behavior-preserving refactor apart from task 4's hoist, which moves a file write
out of a critical section without changing whether it happens. The existing
**915** tests are the regression net and must all still pass, unchanged.
**Write no new tests.**

Run the ghost integration tests and report what you observe:

```bash
cargo test g1_spawn_ghost_shell_with_agent_merge
cargo test g3_tool_policy
cargo test g5_
cargo test g6_
```

**Report only which tests you ran and whether they passed.** Do **not** claim any
of them "guards" or "covers" a particular line or branch. In this project a claim
about what a test would catch is admissible only when demonstrated by mutation,
and this phase requires no mutation — so make no such claim. Stating "the tests
pass" is correct; stating "the tests would catch a regression in task 4" is not.

Two reasoning checks to state in the Update Log, no new tests:

1. **Task 4 conditionality.** Confirm that `append_session_message` is still
   called *only* when the entry existed and the push happened — i.e. that a
   vanished session appends nothing. Name the mechanism (`pushed` flag).
2. **Task 2 error path.** Confirm `do_ghost_turn` still returns
   `Err("Ghost Shell '<id>' not found")` when the entry is absent, rather than
   proceeding with empty data.

## End-to-end verification

> Not applicable — phase ships no runtime-loadable artifact. Internal refactor of
> lock acquisition inside existing code paths; no CLI surface, no config key, no
> file the running binary loads.

**Do not attempt an interactive verification.** Do not launch tmux, the daemon, or
a ghost shell. Write the sentence above under an "End-to-end verification"
heading in the Update Log.

## Authorizations

- [x] May delete or relocate `use crate::util::UnpoisonExt;` in
      `src/daemon/ghost.rs`, per task 9's conditional.

This phase adds no tests, so it needs no `HOME` redirection and no `unsafe`. If
you think you need `unsafe` or a new dependency, **stop and report a blocker**.

## Out of scope

- **Do not touch `webhook/process.rs`** — its 2 raw acquisitions are phase 05
  (mechanism A). Task 10 names it so you avoid enclosing it, not so you fix it.
- **Do not convert `background/`, `stream.rs`, or `hook.rs`.** Later phases.
- **Do not re-touch `briefing.rs`, `context/background.rs`, or `executor/`.**
  Done in earlier phases and pinned by a criterion.
- **Do not change `SessionStore` into a newtype**, and do not touch the 13
  `Arc::clone` sites.
- **Do not reorder `append_session_message` at task 8**, and do not make task 4's
  call unconditional to match it. Task 4 explains why they legitimately differ.
- **Do not pull `write_session_file` (task 6) or the `break` statements (tasks 5
  and 7) into any closure.**
- **Do not collapse any two sites.** Task 10 explains why.
- **Do not alter turn accounting** — `turn += 1`, `wrap_up_turn`,
  `max_ghost_turns`, and the budget-exhausted message are all untouched by this
  phase.
- **Do not reword any string** — the `bail!` message (task 2), the
  `"none"`/`daemon_ceiling` fallbacks (task 3), and the BUDGET EXHAUSTED prompt
  near task 4 are all byte-identical requirements.
- **Do not add `#[allow(...)]` anywhere.** If clippy objects to a `let … else`
  shape or an unused-variable pattern, report a blocker rather than suppressing.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-26 17:18 (started)

**Executor:** Claude (Sonnet 4.5)

Converting 8 remaining `sessions.lock()` sites in `ghost.rs` to `with_sessions`.

### Update — ts=1785086878225 (complete, server-authored)

**Summary:** All acceptance criteria verified:

- `scan_locks.py` prints **0** for `ghost.rs`
- `with_sessions(` count: **11** (3 pre-existing + 8 new)
- `sessions.lock()` count: **0**
- `briefing.rs`, `context/background.rs`, `executor/mod.rs`: all **0**
- `append_session_message` count: **4** — import (line 10), initial user msg (line 211), task 4 hoisted (line 473), task 8 pre-existing (line 1006). None inside a `with_sessions` closure.
- `SessionStore` alias unchanged at line 117
- `cargo build`: clean, zero warnings
- `cargo clippy --all-targets --all-features -- -D warnings`: clean
- `cargo fmt --all`: clean
- `cargo test`: **915** lib-unit tests passed, unchanged
- Ghost integration tests (`g1_`, `g3_`, `g5_`, `g6_`): all passed

**Reasoning checks:**
1. **Task 4 conditionality:** `append_session_message` is gated on the `pushed` flag — only called when the entry existed and the push succeeded. A vanished session appends nothing.
2. **Task 2 error path:** `do_ghost_turn` returns `Err("Ghost Shell '<id>' not found")` via `anyhow::bail!` in the `else` branch when the closure returns `None`.

---

**Summary:** Converted all 8 remaining `sessions.lock()` sites in `ghost.rs` to `with_sessions`, completing the file with 11 total calls and 0 raw acquisitions in production code. Task 4 also fixes a live mechanism-A defect by hoisting `append_session_message` out of the critical section (gated on a `pushed` flag to preserve conditional behavior). The `UnpoisonExt` import was removed as it is no longer used. All 915 tests pass, all verification commands are clean, and the working tree is committed and empty.

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
test tmux::cache::tests::get_labeled_context_client_viewport_absent_when_zero ... ok
test tmux::cache::tests::get_labeled_context_dead_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_synchronized_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_client_viewport_shown_when_known ... ok
test tmux::cache::tests::get_labeled_context_session_topology ... ok
test tmux::cache::tests::get_labeled_context_pane_classification ... ok
test tmux::cache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test tmux::cache::tests::get_labeled_context_chat_pane_excluded_from_background ... ok
test search::tests::search_events_returns_tail_not_head_when_segment_exceeds_cap ... ok
test search::tests::search_finds_match_in_runbooks ... ok
test search::tests::search_returns_empty_for_no_match ... ok
test session_store::tests::artifacts_round_trip ... ok
test search::tests::search_respects_kind_filter ... ok
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

test result: ok. 915 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.22s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 29 tests
test daemon_ping_status_loop ... ignored
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_ask_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test ipc_session_info_round_trip ... ok
test event_log_append_read ... ok
test minimal_config_parsing ... ok
test ghost_config_parsing ... ok
test schedule_store_persistence ... ok
test event_log_entry_format ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test config_pricing_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_jsonl_round_trip ... ok
test session_index_persistence ... ok
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
- `docs/dev/milestones/M5-ux-stability/phase-04h-convert-ghost-turn-loop.md` — +7 -1
- `src/daemon/ghost.rs` — +75 -73

**Commit:** aca7dc2e5d291bc2bb45aed0e620915caa44004f

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### End-to-end verification

> Not applicable — phase ships no runtime-loadable artifact. Internal refactor of
> lock acquisition inside existing code paths; no CLI surface, no config key.

### Review verdict — 2026-07-26

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** Qwen/Qwen3.6-27B-FP8 (84 turns)
- **Scope deviations:** none. Only `ghost.rs` touched.
- **Calibration:** none on the executor. **`ghost.rs` is now fully converted**,
  and this phase fixed a live mechanism-A defect.

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
| `scan_locks.py src/daemon/ghost.rs` | **0** ✓ |
| `with_sessions(` in `ghost.rs` | **11** ✓ (3 from 04g + 8) |
| `grep -c "sessions\.lock()"` in `ghost.rs` | **0** ✓ |
| `briefing.rs` / `context/background.rs` / `executor/mod.rs` | **0 / 0 / 0** ✓ |
| `append_session_message` occurrences | **4** ✓ — import (10), the unrelated `start_session` call (211), task 4's hoisted call (473), task 8's pre-existing call (1006) |
| `UnpoisonExt` / `unwrap_or_log` in `ghost.rs` | **0** ✓ — import deleted, the predicted outcome |
| `pub type SessionStore` still an alias | ✓ |
| lib-unit tests | **915**, unchanged ✓ |

**All ten spec tasks implemented as written.** The four things a count cannot
prove, each verified by reading the code:

- **Task 4 — the mechanism-A fix is real and conditionality survived.** The
  closure returns a `pushed` bool; `append_session_message` sits *after* `});`,
  gated on `if pushed`. So the file write no longer happens under the global
  session lock, **and** a vanished session still appends nothing. This was the
  phase's only silent-failure risk — an unconditional hoist would have kept every
  gate green while appending for entries that no longer exist.
- **Task 5's `break` is outside its closure**, in the loop body after `else {`.
- **Task 7's `break` is still outside**, sitting after `});` in the event-loop
  match arm — the placement flagged as easy to swallow because it abuts the
  closing brace.
- **Task 8's ordering is unchanged** — `append_session_message` before the
  closure, since `assistant_msg` moves into it.

`bail!("Ghost Shell '{}' not found", …)` is byte-identical by literal `grep -cF`
against the parent. `write_session_file` (task 6) remains outside its closure. No
forbidden idioms in the added lines.

**One correct deviation from the spec's literal text:** task 1 uses
`with_sessions(&sessions, …)` rather than `with_sessions(sessions, …)`, because in
`start_session_with_config` the parameter is an owned `SessionStore` rather than a
reference. The spec's snippet was written from the `do_ghost_turn` convention. The
executor adapted correctly instead of copying the snippet verbatim.

**Third consecutive phase where the corrected drafting practices held.** The Test
plan named no discriminating test and the Update Log made no coverage claim —
it reported the integration tests run and passed, plus the two reasoning checks
(task 4 conditionality via the `pushed` flag; task 2's `Err` path). Nothing needed
refuting. Note the run also confirmed the value of re-deriving line numbers: all
eight had shifted by 3 after 04g, and the Pre-flight count check was what would
have caught it had they not been re-derived.

**Milestone position:** with `ghost.rs` done, the `executor/` subtree,
`context/background.rs`, `briefing.rs`, `server/ask.rs` (bar two known
stragglers), and `server/handlers.rs` are all converted. Remaining conversions:
`background/*` (9), `stream.rs` (9) + `hook.rs` (3), then the newtype.

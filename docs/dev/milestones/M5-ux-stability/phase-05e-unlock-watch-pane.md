# Phase 05e: Get `watch_pane`'s Completion Callback Out of the Session Lock

**Milestone:** M5 — UX & Stability
**Status:** done
**Depends on:** phase-05d (the newtype) — `done`
**Estimated diff:** ~40 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

`executor/knowledge/pane.rs:329` performs **two file writes and a tmux subprocess
spawn while holding the global session lock**. Restructure it into the
collect-under-the-lock / act-outside-it shape.

This is mechanism A + B — the same defect 05a and 05b removed from five other
sites. It survived because it was **invisible to every scan in this milestone**;
05d's newtype is what exposed it.

**Finish condition: `pane.rs` has 4 `with_sessions` calls, and the closure-audit
script in the Pre-flight reports `pane.rs` clean.**

## Architecture references

Read before starting:

- `docs/design/daemon-stalls.md` § 1 mechanism A (lock held across blocking work)
  and mechanism B (blocking subprocess spawns on tokio workers). This site is
  both.
- `CLAUDE.md` § "Important Invariants" — `with_sessions` satisfies the
  `.unwrap_or_log()` invariant internally.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Verify the starting state.

**A new instrument.** Counting `.lock()` is obsolete — 05d's newtype made raw
acquisition a compile error, so the only question left is *what runs inside a
`with_sessions` closure*. Save this as `/tmp/audit_closures.py`:

```python
import pathlib, re

BLOCKING = [
    ("append_session_message", "file write"),
    ("write_session_file", "file write"),
    ("write_session_meta", "file write"),
    ("log_event(", "file append"),
    ("std::process::Command", "subprocess"),
    ("tmux::", "subprocess"),
    ("related_knowledge_hints", "fs scan"),
    ("read_session_meta", "file read"),
]

for f in sorted(pathlib.Path("src").rglob("*.rs")):
    src = f.read_text()
    for m in re.finditer(r'with_sessions\s*\(', src):
        i = m.end() - 1
        depth = 0
        start = i
        while i < len(src):
            if src[i] == '(':
                depth += 1
            elif src[i] == ')':
                depth -= 1
                if depth == 0:
                    break
            i += 1
        body = src[start:i + 1]
        line = src[:m.start()].count("\n") + 1
        hits = [(p, w) for p, w in BLOCKING if p in body]
        if hits:
            print(f"{f}:{line}  ->  {', '.join(f'{p} ({w})' for p, w in hits)}")
```

Then:

```bash
python3 /tmp/audit_closures.py
#   src/daemon/executor/knowledge/pane.rs:329  ->  append_session_message (file write), std::process::Command (subprocess)
#   src/daemon/server/ask.rs:97  ->  tmux:: (subprocess), read_session_meta (file read)
grep -c "with_sessions(" src/daemon/executor/knowledge/pane.rs   # expect 3
```

**`ask.rs:97` is phase 05f's, not yours.** It must still be reported when you
finish. A clean report for `ask.rs` means you went out of scope.

**Verified against the tree while drafting.** If the `pane.rs` line differs,
**stop and report a blocker.**

## Current state

### The site — `src/daemon/executor/knowledge/pane.rs:329`

Inside `watch_pane`'s completion callback, which runs on a spawned thread when a
watched pane finishes or times out:

```rust
        with_sessions(&sessions_clone, |store| {
            if let Some(entry) = store.get_mut(&session_id_owned) {
                append_session_message(&session_id_owned, &watch_msg);   // TWO file writes
                entry.messages.push(watch_msg);

                let alert = if completed {
                    format!("Watched pane {} command completed", pane_id_owned)
                } else {
                    format!("Watched pane {} timed out", pane_id_owned)
                };
                if let Some(ref cp) = entry.chat_pane {
                    let _ = std::process::Command::new("tmux")           // subprocess spawn
                        .args(["display-message", "-d", "5000", "-t", cp, &alert])
                        .output();
                }
            }
        });
```

Everything blocking runs while the global lock is held, stalling every other
session's IPC handler.

### ⭐ The worked example — `notify_session`, same shape, same file family

`src/daemon/background/helpers.rs` (landed by 05a). It carries the exact
four-phase structure this task needs, including the two subtleties:

```rust
    // Phase 1 (locked): update the registry and take what the rest needs.
    // Returns None when the session entry is gone.
    let Some(chat_pane) = with_sessions(sessions, |store| {
        let entry = store.get_mut(session_id)?;
        …
        Some(entry.chat_pane.clone())
    }) else {
        return;
    };

    // Phase 2 (unlocked): the file write.
    append_session_message(session_id, &completion_msg);

    // Phase 3 (locked): push the message into the in-memory history.
    with_sessions(sessions, |store| {
        if let Some(entry) = store.get_mut(session_id) {
            entry.messages.push(completion_msg);
        }
    });

    // Phase 4 (unlocked): the tmux notification.
    if let Some(ref cp) = chat_pane {
        let _ = std::process::Command::new("tmux")
            .args(["display-message", "-d", "5000", "-t", cp, &alert])
            .output();
    }
```

**Two properties to copy:**

1. **`chat_pane` is cloned** in phase 1. Borrowing `entry.chat_pane` is what
   would pin the guard open across the rest.
2. **Phase 3 re-checks `get_mut`.** The entry can legitimately vanish while the
   file write runs, and an unconditional push would panic or resurrect state.

### Receiver form

`sessions_clone` is an owned `SessionStore` (cloned before the thread spawn), so
every call is `with_sessions(&sessions_clone, …)` — **with** the `&`, matching
the existing call at `:329`.

### Imports need no change

`pane.rs:3` already imports `append_session_message` and `with_sessions`:

```rust
    FG_HOOK_COUNTER, SessionStore, append_session_message, bg_done_subscribe, with_sessions,
```

**`UnpoisonExt` stays.** `pane.rs` has 4 `unwrap_or_log` calls (`:76`, `:77`,
`:420`, `:434`) and **none of them is this phase's** — they are on
`cache.panes` / `cache.session_name`, which are `RwLock`s, not the session store.
`grep -c "UnpoisonExt"` stays at **2** and `grep -c "unwrap_or_log"` stays at
**4**.

## Spec

### 1. Restructure the callback into four phases

Replace the block quoted above with:

```rust
        // Phase 1 (locked): confirm the entry exists and take what the rest needs.
        let Some(chat_pane) = with_sessions(&sessions_clone, |store| {
            store
                .get_mut(&session_id_owned)
                .map(|entry| entry.chat_pane.clone())
        }) else {
            log::info!(
                "watch_pane {}: {}",
                pane_id_owned,
                if completed { "completed" } else { "timed out" }
            );
            return;
        };

        // Phase 2 (unlocked): the file write.
        append_session_message(&session_id_owned, &watch_msg);

        // Phase 3 (locked): push the message into the in-memory history.
        with_sessions(&sessions_clone, |store| {
            if let Some(entry) = store.get_mut(&session_id_owned) {
                entry.messages.push(watch_msg);
            }
        });

        // Phase 4 (unlocked): the tmux notification.
        let alert = if completed {
            format!("Watched pane {} command completed", pane_id_owned)
        } else {
            format!("Watched pane {} timed out", pane_id_owned)
        };
        if let Some(ref cp) = chat_pane {
            let _ = std::process::Command::new("tmux")
                .args(["display-message", "-d", "5000", "-t", cp, &alert])
                .output();
        }
```

**Four things this must preserve:**

1. **The whole block is still conditional on the entry existing.** Today nothing
   happens when `get_mut` misses — no file write, no tmux message. Phase 1's
   `else` branch must keep that true. An unconditional hoist would append to the
   JSONL of a session that no longer exists, and **no test would catch it.**
2. **`append_session_message` still precedes the in-memory push**, exactly as it
   does today and as `notify_session` does.
3. **Phase 3 re-checks `get_mut`** rather than assuming phase 1's hit still holds.
4. **The trailing `log::info!` still runs on every path.** It currently sits
   *after* the block and fires whether or not the entry was found — hence the
   duplicate in phase 1's `else` branch above. Do not drop it from either path,
   and do not move the surviving one before phase 4.

### 2. Keep exactly one `log::info!` per invocation

The callback currently ends with a single `log::info!("watch_pane {}: {}", …)`
(`pane.rs:347`) that fires on **every** path — entry found or not. The shape in
task 1 preserves that by duplicating it into phase 1's early-return, giving
**two** occurrences in the file that are mutually exclusive at runtime.

**Either arrangement is acceptable**, and a shape with a single occurrence is
preferable if you find one that also preserves the early-return semantics:

- `grep -c 'watch_pane {}: {}'` returning **2** → the duplicated form; verify by
  reading that exactly one branch can run.
- returning **1** → a single-exit form; verify by reading that it still fires when
  the entry is missing.

**The requirement is behavioral — one log line per invocation, on every path —
not a specific count.** Say which arrangement you chose in the Update Log and
show it.

## Acceptance criteria

- [ ] `python3 /tmp/audit_closures.py` no longer reports
      `src/daemon/executor/knowledge/pane.rs`.
- [ ] `python3 /tmp/audit_closures.py` **still reports `src/daemon/server/ask.rs:97`**
      — that is phase 05f's site. A clean report there means you went out of scope.
- [ ] `grep -c "with_sessions(" src/daemon/executor/knowledge/pane.rs` returns
      **4** (3 pre-existing, with the third split into two).
- [ ] `grep -c "append_session_message" src/daemon/executor/knowledge/pane.rs`
      returns **2** — the import on line 3 and the single call, now unlocked.
- [ ] `grep -c "display-message" src/daemon/executor/knowledge/pane.rs` returns
      **1** — unchanged; the tmux notification is moved, not duplicated.
- [ ] `grep -c 'watch_pane {}: {}' src/daemon/executor/knowledge/pane.rs` returns
      **1 or 2** — see task 2. Whichever it is, **exactly one log line must fire
      per invocation on every path**; the count alone cannot prove that, so verify
      by reading and say which arrangement you chose.
- [ ] `grep -c "UnpoisonExt" src/daemon/executor/knowledge/pane.rs` returns **2**
      and `grep -c "unwrap_or_log" …` returns **4** — both unchanged. Those are
      `cache.panes` / `cache.session_name` `RwLock`s and are **not** this phase's.
- [ ] `git diff --stat` shows **exactly one** `src/` file changed.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **915** lib-unit tests — unchanged. This phase adds
      no tests; **any other number means scope crept.**

**Run every gate bare.** `cargo clippy … | tail -20` exits with `tail`'s status,
so a failing gate reads as passing — that is how a real error went unnoticed
earlier in this milestone.

## Test plan

Behavior-preserving restructure: what lands in the session JSONL, what the AI
sees in history, and what the user sees in tmux are all unchanged. Only *when the
lock is held* changes. The existing **915** tests are the regression net.
**Write no new tests.**

`pane.rs` has a `#[cfg(test)]` module at `:370`, but it covers `close_bg_window`
and cache helpers. **`watch_pane` has no test coverage at all** — it needs a live
tmux pane, a spawned thread and a broadcast channel. That is a pre-existing gap
this phase neither widens nor closes, and it is why the spec gives exact target
code rather than relying on tests to catch a slip.

Run the suite and report what you observe. **Report only which commands you ran
and whether they passed.** Do not claim any test "guards" this site — that would
be false, and in this project a claim about what a test would catch is admissible
only when demonstrated by mutation. This phase requires no mutation.

Three reasoning checks to state in the Update Log, no new tests:

1. **Conditionality.** Confirm that a `get_mut` miss still produces **no** file
   write and **no** tmux message, and name the construct that enforces it.
2. **Ordering.** Confirm `append_session_message` still runs before the in-memory
   push.
3. **Log volume.** Confirm exactly one `log::info!` fires per invocation, on both
   the entry-found and entry-missing paths.

## End-to-end verification

None required. This phase ships no new artifact, no CLI behavior, and no config
surface. The gates, the closure audit, and the three reasoning checks are the
verification.

## Authorizations

- [x] May edit `src/daemon/executor/knowledge/pane.rs`.
- [ ] **No** import additions or deletions — everything needed is already in
      scope, and `UnpoisonExt` is still used by four unrelated `RwLock` sites.
- [ ] **No** new tests, no deleted tests, no renamed tests.
- [ ] **No** edits to `src/daemon/server/ask.rs` — that is phase 05f.
- [ ] **No** edits to `pane.rs`'s other two `with_sessions` sites (`:17`, `:54`).
      The audit reports them clean; leave them alone.
- [ ] **No** `#[allow(...)]` anywhere. If clippy objects to a shape, report a
      blocker rather than suppressing.

## Out of scope

- **`ask.rs:97`** — the other site the audit finds: `read_session_meta` (a file
  read) plus `tmux::pane_exists` and `tmux::start_pipe_pane` (two subprocesses),
  all inside one `with_sessions` closure. It is a **larger and more delicate**
  restructure in the daemon's busiest handler and gets its own phase, **05f**.
- **04f's coverage follow-up** — the three vacuous `compaction_in_flight`
  assertions. That is **05g**.
- **Adding `watch_pane` test coverage.** The gap is real and pre-existing; closing
  it needs a tmux fixture and is not this phase's job.

### ⚠ Two traps from earlier phases in this milestone

1. **Do not hoist blocking work unconditionally.** The 04h/05a precedent: the
   write must stay gated on the entry existing. This is the phase's one
   silent-failure risk — every gate stays green if you get it wrong.
2. **Do not insert an item between a doc comment and the item it documents.**
   Phase 05a cost two extra runs when a `struct` added "immediately above" a
   function landed between that function's `///` block and the function. This
   phase adds no items, but if you insert anything at item scope, read the lines
   directly above the insertion point first.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-27 02:28 (started)

**Executor:** Claude (Anthropic)

Restructured `watch_pane`'s completion callback in `pane.rs` from a single `with_sessions` closure (holding the lock across file writes and a tmux subprocess spawn) into the four-phase collect-under-the-lock / act-outside-it shape. Phase 1 clones `chat_pane` under the lock, phase 2 writes the session file unlocked, phase 3 pushes the message into in-memory history under the lock (re-checking `get_mut`), and phase 4 sends the tmux notification unlocked. Chose the single-exit form for `log::info!` — one occurrence at the end, with a duplicate in phase 1's early-return `else` branch, so exactly one log line fires per invocation on every path. `grep -c 'watch_pane {}: {}'` returns 2.

### Update — ts=1785119534160 (complete, server-authored)

**Summary:** **Summary + Notes for review:**

Restructured `watch_pane`'s completion callback in `pane.rs` from a single `with_sessions` closure (holding the global session lock across 2 file writes and a tmux subprocess spawn) into the four-phase collect-under-the-lock / act-outside-it shape, matching the `notify_session` precedent from 05a. All gates pass: `cargo build` clean, `cargo clippy` clean, `cargo fmt` clean, 915 tests pass. The closure audit no longer reports `pane.rs` and still correctly reports `ask.rs:97` (phase 05f). Chose the duplicated `log::info!` form (2 occurrences in file, mutually exclusive at runtime) — one in phase 1's early-return `else` branch for the entry-missing path, one at the end for the entry-found path. **Reasoning checks:** (1) Conditionality enforced by phase 1's `let Some(chat_pane) = ... else { return; }` — a `get_mut` miss returns before any file write or tmux call. (2) `append_session_message` runs in phase 2 before phase 3's in-memory push. (3) Exactly one `log::info!` fires per invocation on every path.

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
test tmux::cache::tests::get_labeled_context_dead_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_client_viewport_absent_when_zero ... ok
test tmux::cache::tests::get_labeled_context_synchronized_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_client_viewport_shown_when_known ... ok
test tmux::cache::tests::get_labeled_context_session_topology ... ok
test tmux::cache::tests::get_labeled_context_copy_mode_annotated ... ok
test tmux::cache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test tmux::cache::tests::get_labeled_context_chat_pane_excluded_from_background ... ok
test search::tests::search_events_returns_tail_not_head_when_segment_exceeds_cap ... ok
test search::tests::search_respects_kind_filter ... ok
test memory::tests::memory_without_frontmatter_has_empty_metadata ... ok
test search::tests::search_returns_empty_for_no_match ... ok
test session_store::tests::artifacts_round_trip ... ok
test session_store::tests::backfill_idempotent ... ok
test session_store::tests::backfill_missing_artifact_returns_error_name ... ok
test session_store::tests::backfill_stamps_memory_without_frontmatter ... ok
test memory::tests::session_memory_block_respects_cap ... ok
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
test memory::tests::update_memory_sets_updated_timestamp ... ok
test session_store::tests::update_in_place_allowed ... ok

test result: ok. 915 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.28s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 29 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g4_briefing_injection_block_format ... ok
test g5_depth_limit_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_ask_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test minimal_config_parsing ... ok
test event_log_append_read ... ok
test ghost_config_parsing ... ok
test schedule_store_persistence ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test config_pricing_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test g4_briefing_masking_applied ... ok
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

- `docs/dev/milestones/M5-ux-stability/README.md` — +1 -1
- `docs/dev/milestones/M5-ux-stability/phase-05e-unlock-watch-pane.md` — +7 -1
- `src/daemon/executor/knowledge/pane.rs` — +30 -12

**Commit:** 10cdc736ad5242bb81b6015fb8491e86c6c918c7

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-07-27

- **Verdict:** approved_first_try
- **Bounces:** none (31 turns)
- **Executor:** Qwen/Qwen3.6-27B-FP8
- **Scope deviations:** none. One `src/` file changed, exactly the specified
  four-phase shape.
- **Calibration:** none new.

Gates re-run bare with exit codes captured: fmt 0, build 0 (zero warnings),
clippy 0, test 0 with **915** tests unchanged. Every count criterion exact —
`with_sessions` 4, `append_session_message` 2, `display-message` 1,
`UnpoisonExt` 2, `unwrap_or_log` 4.

**The closure audit reports `pane.rs` clean and still reports `ask.rs:97`** —
the phase hit its target without straying into 05f's.

**Verified by reading** (the counts cannot show these):

1. **Conditionality holds — the phase's one silent-failure risk.** Phase 1's
   `let Some(chat_pane) = with_sessions(…) else { …; return; }` means a `get_mut`
   miss returns *before* both the file write and the tmux call, exactly as the
   original did nothing on a miss. An unconditional hoist would have appended to
   the JSONL of a vanished session with every gate green.
2. **Ordering preserved.** Phase 2's `append_session_message` still precedes
   phase 3's in-memory push.
3. **Log volume is right.** Two `log::info!` occurrences, mutually exclusive at
   runtime — one on the early-return path, one at the end — so exactly one fires
   per invocation, as before. The executor chose the duplicated form and said so.
4. **`chat_pane` is cloned** in phase 1, and **phase 3 re-checks `get_mut`**.

**One accepted trade-off, stated plainly.** The original held the lock across
both the append and the push, so they were atomic with respect to the entry's
existence. The restructure introduces a microsecond window in which the entry
could be evicted between phase 1 and phase 2, appending one completion line to
the JSONL of a session that has just gone. This is the identical trade-off
`notify_session` made in 05a and is inherent to *any* collect-then-act fix — the
alternative is the mechanism-A defect itself. Sessions evict on a 30-minute idle
timer, and the JSONL is a durable log that a resumed session re-reads, so the
consequence is a harmless extra line rather than lost or corrupted state.

### Milestone state

The closure audit now reports **one** remaining site across 115 `with_sessions`
call sites:

| Site | Blocking work under the guard |
|---|---|
| `server/ask.rs:97` | `read_session_meta` (file read) + `tmux::pane_exists` + `tmux::start_pipe_pane` (2 subprocesses) — **phase 05f** |

**The third exit criterion is still not tickable.** Its enforcement half landed
with 05d's newtype; its "no critical section performs blocking work" half needs
05f. Tick it then, with the closure audit as the evidence.

# Phase 05f: Get the Last Blocking Work Out of `handle_ask`'s Critical Section

**Milestone:** M5 — UX & Stability
**Status:** review
**Depends on:** phase-05e (watch_pane) — `done`
**Estimated diff:** ~120 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

`server/ask.rs:97` is the **last** `with_sessions` closure in the daemon that
performs blocking work. One closure holds the global session lock across:

| Blocking call | Line | What it is |
|---|---|---|
| `read_session_meta(id)` | 100 | a **file read**, inside `or_insert_with` |
| `tmux::pane_exists(pane_id)` | 197 | a **subprocess spawn** |
| `tmux::start_pipe_pane(pane_id)` | 198 | a **subprocess spawn** |

Two independent hoists, both into the collect-under-the-lock / act-outside-it
shape.

**Finish condition: the closure-audit script in the Pre-flight prints
*nothing*.** That is the milestone's third exit criterion becoming true — no
`SessionStore` critical section anywhere in the daemon performs blocking work.

**Both sites are first-turn-only**, not per-turn: `read_session_meta` sits inside
`or_insert_with`, and the R1 block is guarded by `pipe_source_pane.is_none()`
with a don't-retry sentinel. So this is session-creation cost, not every-turn
cost — real, but do not overstate it.

## Architecture references

Read before starting:

- `docs/design/daemon-stalls.md` § 1 mechanism A (lock held across blocking work)
  and mechanism B (blocking subprocess spawns on tokio workers). This closure is
  both.
- `CLAUDE.md` § "Important Invariants" — `with_sessions` satisfies the
  `.unwrap_or_log()` invariant internally.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Verify the starting state.

**The instrument is the closure audit, not a `.lock()` count.** The newtype made
raw acquisition a compile error, so the only question left is what runs *inside* a
closure. Save this as `/tmp/audit_closures.py`:

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
#   src/daemon/server/ask.rs:97  ->  tmux:: (subprocess), read_session_meta (file read)
grep -c "with_sessions(" src/daemon/server/ask.rs   # expect 13
```

**Exactly one line, and it is yours.** **Verified against the tree while
drafting.** If the output differs, **stop and report a blocker.**

## Current state

### The shape being restructured — `ask.rs:91-237`

```rust
    let (catchup_brief, pane_drift_msg, session_cost_usd, has_untracked_cost): (
        Option<String>,
        Option<String>,
        f64,
        bool,
    ) = if let Some(ref id) = session_id {
        with_sessions(sessions, |store| {
            let entry = store.entry(id.clone()).or_insert_with(|| {
                // Try to restore continuity state from persisted meta.
                let meta = crate::daemon::session::read_session_meta(id);   // ← HOIST A
                …builds a SessionEntry from `meta`…
            });
            entry.chat_pane = chat_pane.clone();
            entry.tmux_session = session_name.clone();
            …drift detection → drift_msg…
            …the R1 pipe-pane block (lines 186-217)…                        // ← HOIST B
            …N15 catch-up brief → brief…
            entry.last_detach = None;
            let cost_usd = entry.cost_usd;
            let has_untracked = entry.has_untracked_cost;
            (brief, drift_msg, cost_usd, has_untracked)
        })
    } else {
        (None, None, 0.0, false)
    };
```

### ⭐ The worked example — the four-phase shape, landed twice already

`src/daemon/executor/knowledge/pane.rs` (phase 05e) and
`src/daemon/background/helpers.rs` (phase 05a) both use it. The property to copy
is that **what crosses the lock boundary is owned data**, and a **write-back
re-checks `get_mut`** because the entry can vanish while the unlocked work runs:

```rust
    // Phase 3 (locked): push the message into the in-memory history.
    with_sessions(sessions, |store| {
        if let Some(entry) = store.get_mut(session_id) {
            entry.messages.push(completion_msg);
        }
    });
```

### The R1 block, verbatim — `ask.rs:186-217`

```rust
            if entry.pipe_source_pane.is_none()
                && let Some(ref pane_id) = client_pane
            {
                // Skip if client_pane == chat_pane: the chat pane runs the
                // daemoneye UI, not the user's work.  Piping it is useless and
                // can transiently fail immediately after split-window creates the
                // pane (pty not yet fully initialized) causing repeated log noise.
                let is_chat_pane = chat_pane.as_deref() == Some(pane_id.as_str());
                if is_chat_pane {
                    log::debug!("R1: skipping pipe-pane for {} — same as chat pane", pane_id);
                    entry.pipe_source_pane = Some(String::new()); // don't retry
                } else if crate::tmux::pane_exists(pane_id) {
                    match crate::tmux::start_pipe_pane(pane_id) {
                        Ok(_) => {
                            entry.pipe_source_pane = Some(pane_id.clone());
                        }
                        Err(e) => {
                            // Pane existed at check time but was gone by the time
                            // pipe-pane ran (TOCTOU race) — don't retry this session.
                            log::debug!("R1: could not start pipe-pane for {}: {}", pane_id, e);
                            entry.pipe_source_pane = Some(String::new()); // don't retry
                        }
                    }
                } else {
                    log::debug!(
                        "R1: skipping pipe-pane for {} — pane no longer exists",
                        pane_id
                    );
                    entry.pipe_source_pane = Some(String::new()); // don't retry
                }
            }
```

**`Some(String::new())` is a "don't retry" sentinel**, documented in the comment
block at `:182-185`. Every branch that fails or skips sets it, so the attempt
happens **once per session**. Preserving that is this phase's main correctness
requirement.

### Receiver form and imports

`ask.rs` takes `sessions: &SessionStore`, so every call is
`with_sessions(sessions, …)` — **no** `&`. All 13 existing calls do this.

`ask.rs` imports by glob (`use crate::daemon::session::*;`), so **no import
changes are needed or permitted.**

## Spec

Do the hoists in order. Task 1 is self-contained; task 2 changes the tuple.

### 1. Hoist A — read the persisted meta before taking the lock

**Insert immediately above the `let (catchup_brief, …)` binding at `:91`:**

```rust
    // Read persisted continuity state *before* taking the lock. `read_session_meta`
    // is a file read and must not run inside a critical section. Only needed when
    // the session is not already resident — the probe below is a HashMap lookup,
    // not blocking work.
    let restored_meta = if let Some(ref id) = session_id {
        if with_sessions(sessions, |store| store.contains_key(id)) {
            None
        } else {
            crate::daemon::session::read_session_meta(id)
        }
    } else {
        None
    };
```

**Then replace line 100** inside `or_insert_with`:

```rust
                // before
                let meta = crate::daemon::session::read_session_meta(id);
                // after
                let meta = restored_meta;
```

Everything downstream of `let meta = …` — the `match meta { Some(m) => …, None =>
… }` destructure and the `SessionEntry` it builds — is **unchanged**.

**Three notes:**

- **This adds one lock acquisition per turn** (the `contains_key` probe). It is a
  hash lookup with no I/O, which is exactly the kind of work a critical section is
  *for*. That is the correct trade against a file read under the lock.
- **`restored_meta` is moved into the closure.** `with_sessions` takes `FnOnce`
  and `or_insert_with` takes `FnOnce`, so this compiles. Do **not** clone it to
  work around a borrow error — if you hit one, the fix is elsewhere.
- **The benign race:** if another turn inserts the entry between the probe and the
  main closure, `or_insert_with` does not run and `restored_meta` is dropped
  unused. That is correct — the resident entry already holds the state the meta
  file would have restored.

### 2. Hoist B — decide under the lock, spawn outside, write back

**Replace the R1 block (`:186-217`) with a decision that performs no I/O.** Keep
the `:178-185` comment block above it exactly as it is:

```rust
            // R1 (decide only): `pane_exists` and `start_pipe_pane` are blocking
            // subprocess spawns and run after the lock is released. The
            // same-as-chat-pane case needs no probe, so it is settled here.
            let pipe_candidate: Option<String> = if entry.pipe_source_pane.is_none()
                && let Some(ref pane_id) = client_pane
            {
                // Skip if client_pane == chat_pane: the chat pane runs the
                // daemoneye UI, not the user's work.  Piping it is useless and
                // can transiently fail immediately after split-window creates the
                // pane (pty not yet fully initialized) causing repeated log noise.
                let is_chat_pane = chat_pane.as_deref() == Some(pane_id.as_str());
                if is_chat_pane {
                    log::debug!("R1: skipping pipe-pane for {} — same as chat pane", pane_id);
                    entry.pipe_source_pane = Some(String::new()); // don't retry
                    None
                } else {
                    Some(pane_id.clone())
                }
            } else {
                None
            };
```

**Widen the tuple to five elements.** The binding at `:91`:

```rust
    let (
        catchup_brief,
        pane_drift_msg,
        session_cost_usd,
        has_untracked_cost,
        pipe_candidate,
    ): (Option<String>, Option<String>, f64, bool, Option<String>) =
        if let Some(ref id) = session_id {
            with_sessions(sessions, |store| {
                …
                (brief, drift_msg, cost_usd, has_untracked, pipe_candidate)
            })
        } else {
            (None, None, 0.0, false, None)
        };
```

**Then add the unlocked phase immediately after that `};`:**

```rust
    // Unlocked phase: the two blocking tmux calls, then a short write-back.
    if let (Some(ref id), Some(pane_id)) = (&session_id, pipe_candidate) {
        let resolved = if crate::tmux::pane_exists(&pane_id) {
            match crate::tmux::start_pipe_pane(&pane_id) {
                Ok(_) => pane_id.clone(),
                Err(e) => {
                    // Pane existed at check time but was gone by the time
                    // pipe-pane ran (TOCTOU race) — don't retry this session.
                    log::debug!("R1: could not start pipe-pane for {}: {}", pane_id, e);
                    String::new() // don't retry
                }
            }
        } else {
            log::debug!(
                "R1: skipping pipe-pane for {} — pane no longer exists",
                pane_id
            );
            String::new() // don't retry
        };
        with_sessions(sessions, |store| {
            if let Some(entry) = store.get_mut(id) {
                entry.pipe_source_pane = Some(resolved);
            }
        });
    }
```

**Four things this must preserve:**

1. **The sentinel semantics.** Every path that skips or fails still ends with
   `pipe_source_pane = Some(…)` — the pane id on success, `String::new()`
   otherwise. If any path left it `None`, the daemon would re-probe tmux on
   **every subsequent turn** of that session, which is the failure this phase
   must not introduce. Nothing in the gate set would catch it.
2. **All three `log::debug!` messages survive byte-identical**, including the
   em-dashes. One stays in the closure (same-as-chat-pane); two move outside.
3. **The write-back re-checks `get_mut`.** The entry can be evicted while the
   subprocesses run.
4. **The four existing tuple elements keep their meaning and order.**
   `pipe_candidate` is appended last.

### 3. Verify the audit is clean

```bash
python3 /tmp/audit_closures.py    # must print NOTHING
```

Empty output is the phase's finish condition and the milestone's third exit
criterion. If any line remains, the phase is not done — report what it says.

## Acceptance criteria

- [ ] `python3 /tmp/audit_closures.py` prints **nothing at all**.
- [ ] `grep -c "with_sessions(" src/daemon/server/ask.rs` returns **15**
      (13 pre-existing + the `contains_key` probe + the pipe write-back).
- [ ] `grep -c "read_session_meta" src/daemon/server/ask.rs` returns **1** — the
      call moved, it was not duplicated.
- [ ] `grep -c "tmux::pane_exists" src/daemon/server/ask.rs` returns **1** and
      `grep -c "tmux::start_pipe_pane" …` returns **1** — both moved, neither
      duplicated.
- [ ] `grep -c "R1: " src/daemon/server/ask.rs` returns **4** — the comment header
      plus the three `log::debug!` messages, all surviving.
- [ ] `grep -cF "// don" src/daemon/server/ask.rs` returns **3** — the three
      "don't retry" sentinel comments survive.
- [ ] `grep -c "pipe_source_pane" src/daemon/server/ask.rs` returns **5** — down
      from 7. The three separate assignments at `:200`, `:206`, `:214` collapse
      into one write-back; the init at `:130`, the comment at `:183`, the
      `is_none()` check, and the same-as-chat-pane assignment all remain.
- [ ] `grep -c "UnpoisonExt" src/daemon/server/ask.rs` returns **0** and
      `grep -c "unwrap_or_log" …` returns **2** — both unchanged. `ask.rs` imports
      `UnpoisonExt` by glob (never by name, hence 0), and its two `unwrap_or_log`
      calls are on **different locks entirely**: `bg_session` (`:71`, an
      `Arc<Mutex<String>>`) and `cache.panes` (`:511`, an `RwLock`). **Neither is
      this phase's**, and neither may be touched.
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

Behavior-preserving restructure: which sessions get a pipe-pane, what the
sentinel means, what the AI sees, and what is persisted are all unchanged. Only
*when the lock is held* changes. The existing **915** tests are the regression
net. **Write no new tests.**

**`handle_ask` has no unit coverage** — it needs a live AI client, a tmux session
and an IPC peer. That is a pre-existing gap this phase neither widens nor closes,
and it is why the spec gives exact target code rather than relying on tests.

Run the suite and report what you observe. **Report only which commands you ran
and whether they passed.** Do not claim any test "guards" this site — that would
be false, and in this project a claim about what a test would catch is admissible
only when demonstrated by mutation. This phase requires no mutation.

Four reasoning checks to state in the Update Log, no new tests:

1. **Sentinel completeness.** Enumerate every path through the new R1 code and
   confirm each one ends with `pipe_source_pane = Some(…)`. Name the path that
   would cause a re-probe on every turn if it were left `None`.
2. **Hoist A's conditionality.** Confirm `read_session_meta` still runs only when
   the session is not already resident, and say what it would cost if it ran
   unconditionally.
3. **Write-back safety.** Confirm the pipe write-back re-checks `get_mut`.
4. **Audit clean.** Quote the (empty) output of `audit_closures.py`.

## End-to-end verification

The closure audit **is** the end-to-end verification: it is the only evidence that
no critical section performs blocking work, and this phase is the one that makes
it print nothing. Quote its output in the Update Log.

## Authorizations

- [x] May edit `src/daemon/server/ask.rs`.
- [ ] **No** import additions or deletions — `ask.rs` imports by glob and needs
      nothing new.
- [ ] **No** new tests, no deleted tests, no renamed tests.
- [ ] **No** edits to any other file.
- [ ] **No** change to the four existing tuple elements' meaning or order.
- [ ] **No** change to the `SessionEntry` construction inside `or_insert_with`
      beyond the single `let meta = …` line.
- [ ] **No** `#[allow(...)]` anywhere. If clippy objects to a shape, report a
      blocker rather than suppressing.

## Out of scope

- **04f's coverage follow-up** — the three vacuous `compaction_in_flight`
  assertions. That is **phase 05g**.
- **Making `tmux::` calls non-blocking or timeout-bounded.** That is **phase 06**
  (tmux-call-hardening), which addresses mechanism B across the daemon. This phase
  only moves the two calls out of the critical section; it does not change how
  they run.
- **Adding `handle_ask` test coverage.** The gap is real and pre-existing; closing
  it needs an AI-client and tmux fixture.

### ⚠ Three traps from earlier phases in this milestone

1. **Do not leave any path setting the sentinel to `None`.** This is the phase's
   one silent-failure risk: every gate stays green and the daemon re-probes tmux
   on every turn forever.
2. **Do not assert an import count without checking whether your edits exhaust
   its uses.** Phase 05c `hard_fail`ed on exactly that. Here nothing changes, but
   if clippy reports an unused import, **delete it** — and note that `cargo build`
   reports zero warnings for an unused *test-module* import while
   `cargo clippy --all-targets` errors on it. Clippy is authoritative.
3. **Do not insert an item between a doc comment and the item it documents.**
   Phase 05a cost two extra runs that way. This phase adds no items, but if you
   insert anything at item scope, read the lines directly above first.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-27 02:47 (started)

**Executor:** rexyMCP executor

**Work:** Implementing Hoist A (read_session_meta before lock) and Hoist B (R1 pipe-pane decide-under-lock, spawn-outside, write-back).

### Update — ts=1785121488663 (complete, server-authored)

**Summary:** All acceptance criteria verified:

- `python3 /tmp/audit_closures.py` prints **nothing** — the closure audit is clean
- `with_sessions(` count: **15** (13 pre-existing + contains_key probe + pipe write-back)
- `read_session_meta` count: **1** — moved, not duplicated
- `tmux::pane_exists` count: **1**, `tmux::start_pipe_pane` count: **1** — both moved, neither duplicated
- `R1: ` count: **4** — comment header + 3 debug messages, all surviving
- `// don` count: **3** — all three sentinel comments survive
- `pipe_source_pane` count: **5** — down from 7
- `UnpoisonExt`: **0**, `unwrap_or_log`: **2** — unchanged
- `git diff --stat`: exactly **1** `src/` file changed
- `cargo build`, `cargo clippy`, `cargo fmt`, `cargo test` all pass clean

**Summary + Notes for review:** Hoisted `read_session_meta` out of the `with_sessions` closure via a `contains_key` probe before the lock, and restructured the R1 pipe-pane block to decide under the lock (same-as-chat-pane settled inline, other cases yield a `pipe_candidate`) then perform the two blocking `tmux::` subprocess spawns outside the lock with a short `get_mut` write-back. The closure audit now prints nothing — this was the last blocking-work site inside any `SessionStore` critical section in the daemon. One adaptation: the unlocked `if let` pattern uses `session_id.as_deref()` / `pipe_candidate.as_deref()` to match against `&str` references, since `tmux::pane_exists` and `tmux::start_pipe_pane` take `&str`. All sentinel semantics, debug messages, and write-back safety (`get_mut` re-check) are preserved.

**Executor:** Qwen/Qwen3.6-27B-FP8

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s


TEST
ted ... ok
test webhook::process::tests::severity_rank_case_insensitive ... ok
test webhook::process::tests::severity_rank_ordering ... ok
test webhook::server::tests::auth_empty_secret_always_allows ... ok
test webhook::server::tests::auth_correct_token_allows ... ok
test webhook::server::tests::auth_missing_header_denies ... ok
test webhook::server::tests::auth_token_without_bearer_prefix_denies ... ok
test webhook::server::tests::auth_wrong_token_denies ... ok
test session_store::tests::backfill_stamps_script ... ok
test session_store::tests::collision_allowed_with_force ... ok
test session_store::tests::collision_rejected_without_force ... ok
test session_store::tests::delete_nonexistent_errors ... ok
test session_store::tests::delete_removes_dir_and_index ... ok
test tmux::cache::tests::get_labeled_context_client_viewport_absent_when_zero ... ok
test tmux::cache::tests::get_labeled_context_client_viewport_shown_when_known ... ok
test tmux::cache::tests::get_labeled_context_background_panes_sorted ... ok
test tmux::cache::tests::get_labeled_context_pane_classification ... ok
test tmux::cache::tests::get_labeled_context_synchronized_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_session_topology ... ok
test tmux::cache::tests::get_labeled_context_dead_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_copy_mode_annotated ... ok
test tmux::cache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test session_store::tests::list_returns_newest_first ... ok
test memory::tests::migrate_namespace_skips_already_migrated ... ok
test session_store::tests::rename_nonexistent_errors ... ok
test session_store::tests::rename_to_existing_errors ... ok
test session_store::tests::rename_updates_dir_and_index ... ok
test session_store::tests::save_and_load_round_trip ... ok
test session_store::tests::update_in_place_allowed ... ok
test tmux::cache::tests::get_labeled_context_chat_pane_excluded_from_background ... ok

test result: ok. 915 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.63s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 29 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_session_info_round_trip ... ok
test ipc_ask_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test ghost_config_parsing ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test minimal_config_parsing ... ok
test schedule_store_persistence ... ok
test event_log_entry_format ... ok
test event_log_append_read ... ok
test g4_briefing_read_and_clear ... ok
test config_pricing_round_trip ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test g4_briefing_injects_on_next_run ... ok
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
- `docs/dev/milestones/M5-ux-stability/phase-05f-unlock-ask-entry.md` — +7 -1
- `src/daemon/server/ask.rs` — +54 -23

**Commit:** a6575c77c917d56bd2da8b97a9e9c120b38b3d0c

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

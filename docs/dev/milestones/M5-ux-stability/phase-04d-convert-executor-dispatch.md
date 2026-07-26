# Phase 04d: Convert `executor/mod.rs` Lock Sites + Hoist `load_agent`

**Milestone:** M5 — UX & Stability
**Status:** done
**Depends on:** phase-04c (`ask.rs` converted) — `done`
**Estimated diff:** ~150 lines
**Tags:** language=rust, kind=refactor, size=m

## Goal

Convert the 10 `sessions.lock()` sites in `src/daemon/executor/mod.rs` to
`with_sessions`, collapsing five consecutive reads of the same entry at the top
of `execute_tool_call` into one acquisition, and hoist the `load_agent()`
config-file read out of the critical section in `build_memory_namespaces` —
the mechanism-A defect that `ask.rs`'s converted closures currently have to
tiptoe around.

**Finish condition: 6 `with_sessions` calls for 10 former sites, and zero
`sessions.lock()` remaining in `src/daemon/executor/mod.rs`.**

## Architecture references

Read before starting:

- `docs/design/daemon-stalls.md` § 3.5 — the migration hazard. A converted
  closure that encloses a call which still uses **raw** `.lock()` deadlocks
  silently: no panic, no log, just a hung test run. This phase's region has three
  such unconverted callees; task 6 names them.
- `docs/design/daemon-stalls.md` § 1 mechanism A — lock held across blocking
  work. `build_memory_namespaces` is a live instance and task 1 fixes it.
- `docs/design/daemon-stalls.md` § 3.4 — why the newtype is not part of this
  phase.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Verify the starting state — earlier phases move line numbers:

```bash
grep -c "sessions\.lock()" src/daemon/executor/mod.rs   # expect 10
grep -c "sessions\.lock()" src/daemon/server/ask.rs     # expect 0 (04c landed)
grep -c "with_sessions(" src/daemon/server/ask.rs       # expect 11
```

If the first number is not 10, **stop and report a blocker** — the spec's
per-site line numbers are stale and guessing which site is which is how a
conversion phase corrupts a file.

## Current state

`SessionStore` is still the bare type alias, so unconverted sites elsewhere keep
compiling:

```rust
// src/daemon/session.rs:116
pub type SessionStore = Arc<Mutex<HashMap<String, SessionEntry>>>;
```

### The accessor you are converting to — `src/daemon/session.rs:418-434`

```rust
/// Run `f` with exclusive access to the session map.
///
/// This is the intended way to touch `SessionStore`. The guard's lifetime is the
/// closure body, so it cannot escape, cannot be held across an `.await`, and a
/// nested acquisition trips an assertion instead of deadlocking.
///
/// Do **not** call `with_sessions` from inside `f`, and do not call anything from
/// inside `f` that reaches the store — collect what you need, return it, and act
/// after the closure returns.
pub fn with_sessions<T>(
    sessions: &SessionStore,
    f: impl FnOnce(&mut HashMap<String, SessionEntry>) -> T,
) -> T {
    let _depth = SessionsLockDepth::enter();
    let mut store = sessions.lock().unwrap_or_log();
    f(&mut store)
}
```

It is generic over the return type `T` and hands you `&mut HashMap`, so a
closure may return anything — a tuple, a struct, an `Option`. Import it as
`crate::daemon::session::with_sessions`; check whether `executor/mod.rs` already
has a `use crate::daemon::session::*;` glob before adding an import.

### Site inventory

Ten sites in three enclosing functions:

| Line | Function | Shape |
|---|---|---|
| 88 | `build_memory_namespaces` (fn at 78) | `let`-chain, **holds lock across `load_agent()`** |
| 130 | `execute_tool_call` (fn at 109) | `if let Ok(store)`, reads `ghost_config` |
| 150 | `execute_tool_call` | `if let Ok(store)`, reads `is_ghost` |
| 169 | `execute_tool_call` | `if let Ok(store)`, reads `ghost_config` |
| 205 | `execute_tool_call` | `.and_then(\|sid\| sessions.lock().ok()?…)` |
| 207 | `execute_tool_call` | `.and_then(\|sid\| sessions.lock().ok()?…)` |
| 329 | `execute_tool_call` | `let`-chain, `get_mut`, write |
| 537 | `execute_tool_call` | `.and_then(\|sid\| sessions.lock().ok()?…)` |
| 922 | `find_best_target_pane` (fn at 891) | `let`-chain, **holds lock across `cache.panes.read()` and an early `return`** |
| 966 | `find_best_target_pane` | `let`-chain, `get_mut`, write |

Sites 130, 150, 169, 205, and 207 all read **the same entry** (`store.get(sid)`)
within ~80 lines of one another, taking and releasing the lock five times. Task 2
collapses them.

## Spec

### 1. Hoist `load_agent()` out of the lock in `build_memory_namespaces`

Current body — `src/daemon/executor/mod.rs:78-101`:

```rust
pub fn build_memory_namespaces(
    session_id: Option<&str>,
    sessions: &SessionStore,
    is_ghost: bool,
) -> Vec<String> {
    if !is_ghost {
        return vec!["global".to_string()];
    }
    let mut namespaces: Vec<String> = Vec::new();
    if let Some(sid) = session_id
        && let Ok(store) = sessions.lock()
        && let Some(entry) = store.get(sid)
        && let Some(ref gc) = entry.ghost_config
        && let Some(ref agent_name) = gc.agent
        && let Ok(agent) = crate::agents::load_agent(agent_name)
    {
        namespaces.push(agent.memory_namespace.clone());
        for extra in &agent.read_namespaces {
            namespaces.push(extra.clone());
        }
    }
    if !namespaces.iter().any(|s| s == "global") {
        namespaces.push("global".to_string());
    }
    namespaces
}
```

`crate::agents::load_agent(agent_name)` reads a config file from disk while
`store` is alive — the whole daemon's session map is locked for the duration of a
filesystem read. Replace the body with:

```rust
pub fn build_memory_namespaces(
    session_id: Option<&str>,
    sessions: &SessionStore,
    is_ghost: bool,
) -> Vec<String> {
    if !is_ghost {
        return vec!["global".to_string()];
    }
    let agent_name: Option<String> = session_id.and_then(|sid| {
        with_sessions(sessions, |store| {
            store.get(sid)?.ghost_config.as_ref()?.agent.clone()
        })
    });
    let mut namespaces: Vec<String> = Vec::new();
    if let Some(name) = agent_name
        && let Ok(agent) = crate::agents::load_agent(&name)
    {
        namespaces.push(agent.memory_namespace.clone());
        for extra in &agent.read_namespaces {
            namespaces.push(extra.clone());
        }
    }
    if !namespaces.iter().any(|s| s == "global") {
        namespaces.push("global".to_string());
    }
    namespaces
}
```

Three things to get right here:

- The `?` operators are inside the **closure**, so they return from the closure
  (whose type is `Option<String>`), not from `build_memory_namespaces`. This is
  the same shape 04c used for `ask.rs`'s `.and_then` chains.
- `load_agent` is now called **after** `with_sessions` returns, so the guard is
  gone before the disk read starts. That is the entire point of the task — do not
  "simplify" it back into the chain.
- **Do not change the signature.** It stays `pub` and keeps taking
  `&SessionStore`. There are two other callers — `src/daemon/server/ask.rs:566`
  and the tests at `mod.rs:993` and `mod.rs:1140` — and all three keep working
  untouched. `ask.rs:566` gets the mechanism-A fix for free, which is why the fix
  belongs inside this function rather than at its call sites.

### 2. Collapse the five prologue reads into one acquisition

Sites 130, 150, 169, 205, and 207 all read the same entry. Add a private struct
immediately above `execute_tool_call` (above the `pub async fn` at line 109):

```rust
/// Everything `execute_tool_call` needs from the session entry, read in one
/// acquisition. Five separate reads of the same entry preceded this.
struct DispatchSnapshot {
    ghost_policy: Option<GhostPolicy>,
    tool_policy: Option<crate::agents::ToolPolicy>,
    is_ghost_shell: bool,
    spawn_depth: u8,
    effective_parent_job_id: Option<String>,
    saved_name: Option<String>,
    turn_count: usize,
}
```

Replace all five sites — from the `// ── Pre-fetch Ghost Policy and Tool Policy`
comment through the `turn_count` binding — with one `with_sessions` call. The
current code being replaced is:

```rust
    // ── Pre-fetch Ghost Policy and Tool Policy ───────────────────────────────
    let ghost_and_tool: Option<(GhostPolicy, Option<crate::agents::ToolPolicy>)> =
        if let Some(sid) = session_id {
            if let Ok(store) = sessions.lock() {
                store.get(sid).and_then(|e| {
                    if e.is_ghost {
                        e.ghost_config
                            .as_ref()
                            .map(|gc| (GhostPolicy::from_config(gc), gc.tool_policy.clone()))
                    } else {
                        None
                    }
                })
            } else {
                None
            }
        } else {
            None
        };
    let ghost_policy: Option<GhostPolicy> = ghost_and_tool.as_ref().map(|(gp, _)| gp.clone());
    let tool_policy: Option<crate::agents::ToolPolicy> = ghost_and_tool.and_then(|(_, tp)| tp);
    // Defensive guard: a ghost shell entry must always have a ghost_config.
    let is_ghost_shell: bool = if let Some(sid) = session_id {
        if let Ok(store) = sessions.lock() {
            store.get(sid).map(|e| e.is_ghost).unwrap_or(false)
        } else {
            false
        }
    } else {
        false
    };
```

…then (after the `is_ghost_shell && ghost_policy.is_none()` guard and
`let is_ghost = ghost_policy.is_some();`) the spawn-depth block:

```rust
    // ── Delegation depth tracking ────────────────────────────────────────────
    let (spawn_depth, effective_parent_job_id): (u8, Option<String>) = if let Some(sid) = session_id
    {
        if let Ok(store) = sessions.lock() {
            store
                .get(sid)
                .and_then(|e| e.ghost_config.as_ref())
                .map(|gc| (gc.spawn_depth, Some(sid.to_string())))
                .unwrap_or((0, None))
        } else {
            (0, None)
        }
    } else {
        (0, None)
    };
```

…and later the two artifact-context reads:

```rust
    let saved_name: Option<String> =
        session_id.and_then(|sid| sessions.lock().ok()?.get(sid)?.saved_name.clone());
    let turn_count: usize = session_id
        .and_then(|sid| sessions.lock().ok()?.get(sid).map(|e| e.turn_count))
        .unwrap_or(0);
```

Target code — one acquisition placed where the first block was, at the
`// ── Pre-fetch Ghost Policy and Tool Policy` comment:

```rust
    // ── Pre-fetch session state in one acquisition ───────────────────────────
    let snap: DispatchSnapshot = match session_id {
        Some(sid) => with_sessions(sessions, |store| match store.get(sid) {
            Some(e) => {
                let gc = e.ghost_config.as_ref();
                DispatchSnapshot {
                    ghost_policy: if e.is_ghost {
                        gc.map(GhostPolicy::from_config)
                    } else {
                        None
                    },
                    tool_policy: if e.is_ghost {
                        gc.and_then(|g| g.tool_policy.clone())
                    } else {
                        None
                    },
                    is_ghost_shell: e.is_ghost,
                    spawn_depth: gc.map(|g| g.spawn_depth).unwrap_or(0),
                    effective_parent_job_id: gc.map(|_| sid.to_string()),
                    saved_name: e.saved_name.clone(),
                    turn_count: e.turn_count,
                }
            }
            None => DispatchSnapshot::default(),
        }),
        None => DispatchSnapshot::default(),
    };
```

Derive `Default` on `DispatchSnapshot` so the two miss paths are one expression:

```rust
#[derive(Default)]
struct DispatchSnapshot { … }
```

`u8`, `bool`, and every `Option<…>` already have a `Default`, and the defaults
match the old fall-throughs exactly (`false`, `0`, `None`, `0`).

Then rebind the locals so the rest of the ~800-line function is untouched:

```rust
    let DispatchSnapshot {
        ghost_policy,
        tool_policy,
        is_ghost_shell,
        spawn_depth,
        effective_parent_job_id,
        saved_name,
        turn_count,
    } = snap;
```

Keep the two comments that carried meaning — `// Defensive guard: a ghost shell
entry must always have a ghost_config.` above the
`is_ghost_shell && ghost_policy.is_none()` check, and `// ── Delegation depth
tracking` if you keep a marker for where that data is consumed. Delete the
now-dead `ghost_and_tool` binding entirely.

**Behavior that must not change:** `effective_parent_job_id` is
`Some(sid.to_string())` only when `ghost_config` is present, `None` otherwise —
note that the original `.map(|gc| (gc.spawn_depth, Some(sid.to_string())))`
couples both to `gc` being `Some`, which is why `effective_parent_job_id` uses
`gc.map(|_| …)` above rather than `Some(sid.to_string())` unconditionally.
Getting this wrong makes a non-ghost session report a parent job id.

`GhostPolicy::from_config` (`src/daemon/policy.rs:32-39`) is a pure field
mapping — four `clone`/copy assignments, no I/O — so calling it inside the
closure is safe. `saved_name.clone()` and `e.turn_count` are likewise pure.

**`saved_name` is later borrowed as `saved_name.as_deref()` when constructing
`knowledge::ArtifactCtx`.** Keep it an owned `Option<String>` local so that
borrow still compiles; do not collapse it into the struct field access at the use
site.

### 3. Convert site 329 — `LoadTools` persistence

```rust
            // Persist into session state
            if let Some(sid) = session_id
                && let Ok(mut store) = sessions.lock()
                && let Some(entry) = store.get_mut(sid)
            {
                for name in &loaded {
                    entry.loaded_tools.insert(name.clone());
                }
                entry.dirty = true;
            }
```

becomes:

```rust
            // Persist into session state
            if let Some(sid) = session_id {
                with_sessions(sessions, |store| {
                    if let Some(entry) = store.get_mut(sid) {
                        for name in &loaded {
                            entry.loaded_tools.insert(name.clone());
                        }
                        entry.dirty = true;
                    }
                });
            }
```

`loaded` is a `Vec<String>` built before this block; the closure borrows it
immutably, which is fine.

### 4. Convert sites 537 and 966

Site 537, in the `PendingCall::GetTerminalContext` arm:

```rust
            let target_pane: Option<String> = session_id
                .and_then(|sid| sessions.lock().ok()?.get(sid)?.default_target_pane.clone());
```

becomes:

```rust
            let target_pane: Option<String> = session_id.and_then(|sid| {
                with_sessions(sessions, |store| {
                    store.get(sid)?.default_target_pane.clone()
                })
            });
```

Site 966, in `find_best_target_pane`:

```rust
            if let Some(sid) = session_id
                && let Ok(mut store) = sessions.lock()
                && let Some(entry) = store.get_mut(sid)
            {
                entry.default_target_pane = Some(pane_id.clone());
            }
```

becomes:

```rust
            if let Some(sid) = session_id {
                with_sessions(sessions, |store| {
                    if let Some(entry) = store.get_mut(sid) {
                        entry.default_target_pane = Some(pane_id.clone());
                    }
                });
            }
```

### 5. Convert site 922 — extract, then act

This is the one site where a mechanical wrap is **wrong**. Current code:

```rust
    // Check for a user-selected default target pane in the session.
    if let Some(sid) = session_id
        && let Ok(store) = sessions.lock()
        && let Some(entry) = store.get(sid)
        && let Some(ref dtp) = entry.default_target_pane
        && chat_pane != Some(dtp.as_str())
    {
        let panes = cache.panes.read().unwrap_or_log();
        if panes.contains_key(dtp) {
            return Ok(dtp.clone());
        }
    }
```

Two defects, both fixed by the same rewrite:

- The sessions guard is held across `cache.panes.read()`, so this acquires a
  second lock inside the first — a lock-ordering hazard against any code that
  takes them in the other order.
- The `return Ok(dtp.clone())` returns from `find_best_target_pane`. Move it
  inside a `with_sessions` closure and it returns from the **closure** instead,
  which either fails to compile or silently changes control flow. This is the
  trap; do not fall into it.

Target:

```rust
    // Check for a user-selected default target pane in the session.
    let default_target: Option<String> = session_id.and_then(|sid| {
        with_sessions(sessions, |store| {
            store.get(sid)?.default_target_pane.clone()
        })
    });
    if let Some(dtp) = default_target
        && chat_pane != Some(dtp.as_str())
    {
        let panes = cache.panes.read().unwrap_or_log();
        if panes.contains_key(&dtp) {
            return Ok(dtp);
        }
    }
```

The sessions lock is released before `cache.panes.read()` is taken, and the
`return` stays in the function body where it belongs. Note `panes.contains_key(&dtp)`
takes a reference now that `dtp` is an owned `String`, and the `return` no longer
needs a `clone`.

### 6. Do not let any closure span these unconverted callees

`execute_tool_call` dispatches into code that still uses raw `sessions.lock()`.
A `with_sessions` closure enclosing any of them **deadlocks silently** — no
panic, no log, a hung test run (`daemon-stalls.md` § 3.5). The re-entrancy
assertion only catches `with_sessions` nested inside `with_sessions`, not
`with_sessions` enclosing a raw `.lock()`.

Store-touching calls in this file's region that this phase does **not** convert:

| Callee | Raw lock at | Reached from |
|---|---|---|
| `knowledge::track_artifact` (via `ArtifactCtx`) | `executor/knowledge/mod.rs:38` | every `knowledge::` dispatch arm |
| `knowledge::…` pane helpers | `executor/knowledge/pane.rs:19`, `:52` | pane tool arms |
| foreground execution | `executor/foreground.rs:170`, `:199`, `:232`, `:885` | `run_terminal_command` arms |

Every closure this phase introduces reads or writes one entry and returns
immediately — none of them calls out. **Keep it that way.** Specifically: the
task-2 prologue closure must end before `artifact_ctx` is constructed, and no
closure may enclose a `.await`. `with_sessions` takes a synchronous `FnOnce`, so
an `.await` inside one will not compile — that is a guardrail, not a limitation
to work around.

### 7. One test for the hoist

Add to the existing `#[cfg(test)] mod tests` in `src/daemon/executor/mod.rs`,
beside the two existing `build_memory_namespaces` tests at `mod.rs:993` and
`mod.rs:1140`:

- `build_memory_namespaces_does_not_hold_the_lock_across_load_agent` — build a
  store with one ghost entry whose `ghost_config.agent` is `Some("<name>")`,
  call `build_memory_namespaces`, and assert that a `try_lock()` on the store
  succeeds **after** it returns. The real assertion is structural: with the old
  chained body a `try_lock` taken from inside a `load_agent` stub would fail, but
  since `load_agent` reads the real filesystem and is not injectable here, assert
  the observable proxy — the function returns and the store is immediately
  lockable, and the returned namespaces still contain `"global"`.

Use `try_lock`, not `lock`. A regression must **fail** this test, not hang it.
That is a standing requirement in this milestone: two earlier phases shipped
lock-invariant tests that hung CI instead of failing it, and both were fixed on
review.

The existing two tests must keep passing **unmodified**. If you find yourself
editing them, you changed the signature or the return contract — re-read task 1.

## Acceptance criteria

- [ ] `grep -c "sessions\.lock()" src/daemon/executor/mod.rs` returns `0`.
- [ ] `grep -c "with_sessions(" src/daemon/executor/mod.rs` returns `6`.
- [ ] `grep -c "load_agent" src/daemon/executor/mod.rs` returns `1`, and it is
      **not** inside a `with_sessions` closure.
      **[ARCHITECT ERROR, corrected at review 2026-07-26: this criterion is
      unsatisfiable as written. Task 7 mandates a test named
      `build_memory_namespaces_does_not_hold_the_lock_across_load_agent`, whose
      name contains `load_agent`, so the count is necessarily 2. The substantive
      requirement — the single call site is outside every closure — is met at
      `mod.rs:93`. Not an executor failure.]**
- [ ] `src/daemon/server/ask.rs` is unmodified by this phase
      (`git diff --name-only` does not list it).
- [ ] The two pre-existing `build_memory_namespaces` tests pass unmodified.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes — **915** lib-unit tests (914 + the one from task 7)
      plus 27 integration.
- [ ] `cargo test` completes without hanging. A conversion regression in this
      milestone manifests as a hang, not a failure — if the suite stops
      progressing, you have a closure spanning a raw `.lock()` (task 6), not a
      slow machine.

## Test plan

One new test, named in task 7:

- `build_memory_namespaces_does_not_hold_the_lock_across_load_agent` in
  `src/daemon/executor/mod.rs` — asserts the store is lockable via `try_lock`
  immediately after the call returns, and that the result still contains
  `"global"`.

No new tests for tasks 2–5. They are behavior-preserving conversions of existing
code paths already covered by the suite, and STANDARDS § 3.2 excludes pure
plumbing. The load-bearing verification for those is that the existing 914 tests
still pass **and still terminate**.

## End-to-end verification

> Not applicable — phase ships no runtime-loadable artifact. This is an internal
> refactor of lock acquisition inside an existing code path; it adds no CLI
> surface, no config key, and no file the running binary loads.

The tool-dispatch path this phase touches is exercised end-to-end by the
milestone's later phases; the meaningful check here is that `cargo test`
terminates, which the acceptance criteria pin.

## Authorizations

None.

## Out of scope

- **Do not touch `src/daemon/server/ask.rs`.** 04c converted it and it is in
  review. It calls `build_memory_namespaces` at line 566 and benefits from task 1
  without any edit.
- **Do not convert `executor/foreground.rs`, `executor/knowledge/mod.rs`,
  `executor/knowledge/pane.rs`, or `executor/knowledge/ghost.rs`.** Those 8 sites
  are the next phase. Task 6 lists them precisely so you can avoid enclosing
  them, not so you can fix them.
- **Do not change `SessionStore` into a newtype**, and do not touch the 13
  `Arc::clone` sites. That is the later enforcement phase
  (`daemon-stalls.md` § 3.4).
- **Do not convert `context/background.rs`, `ghost.rs`, `stream.rs`, `hook.rs`,
  `background/`, or `webhook/process.rs`.** Separate phases.
- **Do not change `build_memory_namespaces`'s signature or visibility**, and do
  not add a second `build_memory_namespaces`-like helper that takes an agent name
  — one function, internally fixed.
- **Do not "optimize" the task-2 prologue by feeding the agent name into
  `build_memory_namespaces`** to save its acquisition. That would change the
  shared function's signature and break `ask.rs`. Six acquisitions is the target,
  not five.
- **Do not add `#[allow(...)]` anywhere.** If clippy objects to
  `DispatchSnapshot` having many fields or to `Default` on it, report a blocker
  rather than suppressing.
- **Do not remove the `unwrap_or_log()` on `cache.panes.read()`** in task 5. That
  is a project invariant (`CLAUDE.md` § "Important Invariants") and it is a
  different lock from the one you are converting.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-26 13:28 (started)

**Executor:** Claude executor

Converted all 10 `sessions.lock()` sites in `src/daemon/executor/mod.rs` to `with_sessions`, collapsing five consecutive prologue reads into one `DispatchSnapshot` acquisition. Hoisted `load_agent()` out of the lock in `build_memory_namespaces`. Added one test verifying the lock is released after the call.

### Update — ts=1785073468805 (complete, server-authored)

**Summary:** **Summary + Notes for review:**

Converted all 10 `sessions.lock()` sites in `src/daemon/executor/mod.rs` to `with_sessions`, achieving 6 `with_sessions` calls for 10 former sites with zero `sessions.lock()` remaining. Task 1 hoisted `load_agent()` out of the lock in `build_memory_namespaces`, fixing the mechanism-A defect where a disk read held the entire session map. Task 2 collapsed five consecutive prologue reads of the same session entry into a single `DispatchSnapshot` acquisition, reducing lock contention in the hot dispatch path. Tasks 3–5 converted the remaining sites (`LoadTools` persistence, `GetTerminalContext` target pane, and `find_best_target_pane`), with task 5 extracting `default_target_pane` before `cache.panes.read()` to fix a lock-ordering hazard. Task 6 verified no closure spans unconverted callees. Task 7 added a `try_lock` test confirming the store is immediately lockable after `build_memory_namespaces` returns. All 915 lib-unit tests and 27 integration tests pass. `cargo clippy` and `cargo fmt` clean. `grep -c "load_agent" src/daemon/executor/mod.rs` returns 2 (the call at line 93 outside any closure, plus the test name at line 1180).

**Executor:** Qwen/Qwen3.6-27B-FP8

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.49s


LINT
    Checking daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.51s


TEST
nes_sorted ... ok
test tmux::cache::tests::get_labeled_context_copy_mode_annotated ... ok
test tmux::cache::tests::get_labeled_context_client_viewport_shown_when_known ... ok
test tmux::cache::tests::get_labeled_context_synchronized_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_dead_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_pane_classification ... ok
test tmux::cache::tests::get_labeled_context_session_topology ... ok
test tmux::cache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test tmux::cache::tests::get_labeled_context_chat_pane_excluded_from_background ... ok
test search::tests::search_events_returns_tail_not_head_when_segment_exceeds_cap ... ok
test search::tests::search_finds_match_in_runbooks ... ok
test search::tests::search_respects_kind_filter ... ok
test session_store::tests::artifacts_round_trip ... ok
test memory::tests::session_memory_block_respects_cap ... ok
test session_store::tests::backfill_idempotent ... ok
test session_store::tests::backfill_missing_artifact_returns_error_name ... ok
test session_store::tests::collision_allowed_with_force ... ok
test session_store::tests::backfill_stamps_memory_without_frontmatter ... ok
test session_store::tests::backfill_stamps_runbook ... ok
test session_store::tests::collision_rejected_without_force ... ok
test session_store::tests::delete_nonexistent_errors ... ok
test memory::tests::update_memory_partial_update_preserves_other_fields ... ok
test session_store::tests::delete_removes_dir_and_index ... ok
test session_store::tests::list_returns_newest_first ... ok
test session_store::tests::load_messages_max_count_truncates ... ok
test session_store::tests::rename_nonexistent_errors ... ok
test memory::tests::update_memory_sets_updated_timestamp ... ok
test session_store::tests::rename_updates_dir_and_index ... ok
test session_store::tests::save_and_load_round_trip ... ok
test session_store::tests::update_in_place_allowed ... ok

test result: ok. 915 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.28s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 29 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_ask_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test event_log_entry_format ... ok
test minimal_config_parsing ... ok
test ghost_config_parsing ... ok
test schedule_store_persistence ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
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
- `docs/dev/milestones/M5-ux-stability/phase-04d-convert-executor-dispatch.md` — +7 -1
- `src/daemon/executor/mod.rs` — +179 -74

**Commit:** 1ea8c7effb2df599cc986d99f059bc2a474faa84

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### End-to-end verification

> Not applicable — phase ships no runtime-loadable artifact. Internal refactor of
> lock acquisition inside an existing code path; no CLI surface, no config key.

(Heading written by the architect at review — the server-authored completion
entry does not emit it. Same calibration item as 04c.)

### Review verdict — 2026-07-26

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** Qwen/Qwen3.6-27B-FP8 (128 turns)
- **Scope deviations:** none by the executor. All 7 spec tasks implemented as
  written; `ask.rs`, `foreground.rs`, `knowledge/*`, and `SessionStore` all
  untouched as Out-of-scope required.
- **Calibration:** **two architect spec defects, both mine, neither the
  executor's fault.** See "Architect spec defects" below. This is the **third**
  occurrence of the phase-01 pattern (pinning something the same doc makes
  unsatisfiable) — per the phase-01 calibration note, third occurrence warrants
  raising with the PE. Raised.

**Independent re-run at review** (separate invocations, not chained):

```
cargo fmt --all --check                                    → exit 0
cargo build                                                → exit 0, no warnings
cargo clippy --all-targets --all-features -- -D warnings   → exit 0
cargo test  → 915 lib-unit passed / 0 failed (914 + 1 new)
              27 integration passed / 2 ignored
              run terminated normally (no hang)
```

**Acceptance criteria:**

| Criterion | Result |
|---|---|
| `sessions.lock()` in `executor/mod.rs` | **0** ✓ |
| `with_sessions(` in `executor/mod.rs` | **6** ✓ (the pinned finish condition) |
| `load_agent` count == 1 | **2** — see ARCHITECT ERROR annotation above; substantive intent met |
| `ask.rs` unmodified | ✓ not in `files_changed` |
| Two pre-existing `build_memory_namespaces` tests unmodified | ✓ zero removed test lines |
| build / clippy / fmt / test | ✓ all green |
| `cargo test` terminates | ✓ no hang |

**Spec conformance, checked by reading the diff:**

- **Task 1 (hoist)** matches the spec body exactly. `load_agent` is now at
  `mod.rs:93`, outside every closure; the `with_sessions` call above it returns
  `Option<String>` and the `?` operators bind to the closure. Signature
  unchanged, so `ask.rs:566` gets the mechanism-A fix without an edit.
- **Task 2 (collapse)** matches, including the subtlety flagged in the spec:
  `effective_parent_job_id: gc.map(|_| sid.to_string())` — `Some(sid)` only when
  `ghost_config` is present, so a non-ghost session does not report a parent job
  id. `#[derive(Default)]` covers both miss paths and the defaults match the old
  fall-throughs (`false`, `0`, `None`, `0`). `GhostPolicy::from_config` inside
  the closure is safe — pure field mapping, `policy.rs:32-39`.
- **Tasks 3, 4** match.
- **Task 5 (site 922)** correctly extract-then-act: `default_target` is read and
  the guard released *before* `cache.panes.read()`, killing the nested-lock
  hazard, and the `return Ok(dtp)` stays in the function body rather than moving
  inside a closure. `contains_key(&dtp)` adjusted for the now-owned `String`.
  This was the one site that could not be converted mechanically; it was done
  correctly.
- **Task 6** holds: every closure reads/writes one entry and returns. None
  encloses `knowledge::track_artifact` (`knowledge/mod.rs:38`),
  `knowledge/pane.rs:19,52`, or `foreground.rs:170,199,232,885`. The §3.5 hazard
  is avoided, which the terminating test run corroborates.
- No `#[allow]`, no `#[ignore]`, no `dbg!`/`println!`, no `TODO`/`FIXME`/`XXX`.

**Three `unsafe` blocks were added, all in the new test** (`env::set_var` ×2,
`env::remove_var` ×1). 04d's Authorizations said "None", and the DoD says no new
`unsafe`. Accepted rather than bounced: `std::env::set_var` is `unsafe` in
edition 2024, and this is the established codebase idiom for HOME-redirecting
tests — `with_test_home` at `src/daemon/utils/event_log.rs:288-299` does exactly
the same thing. There is no safe alternative. The architect should have
pre-authorized it in the phase doc; that is a spec omission, not a violation.

---

## Architect spec defects found at review

**1. Acceptance criterion 3 was unsatisfiable.** Annotated inline above.
Criterion 3 demanded `grep -c load_agent == 1` while task 7 mandated a test name
containing `load_agent`. The executor reported 2 and explained why — correct
behavior. Third occurrence of the phase-01 "same doc contradicts itself" pattern.

**2. Task 7's test does not test what it claims — verified by mutation, not by
reading.** The review reverted `build_memory_namespaces` to the old chained body
(guard held across `load_agent`) and ran the new test:

```
$ cargo test --lib build_memory_namespaces_does_not_hold_the_lock_across_load_agent
test daemon::executor::tests::build_memory_namespaces_does_not_hold_the_lock_across_load_agent ... ok
```

**It passes against the un-hoisted code.** The tree was restored afterwards
(`git checkout src/daemon/executor/mod.rs`, confirmed clean).

The reason is structural and was baked into the spec: the guard is function-local
in *both* implementations, so by the time `build_memory_namespaces` returns the
lock is free either way. `store.try_lock().is_ok()` after the call can never
distinguish them. Task 7 even said so out loud — "assert the observable proxy" —
without noticing that the proxy is vacuous. The executor implemented precisely
what was asked.

**The production fix is real** — `load_agent` is demonstrably outside every
closure at `mod.rs:93`, greppable and confirmed in the diff. Only the regression
net is missing. A real test needs `load_agent` behind a trait seam so a stub can
attempt `try_lock` *during* the call; that is a design change, not a test tweak.

**Follow-up, deliberately not dispatched as a bounce** (the fix is architect work
and the executor conformed): carry into **phase 05** (`unlock-blocking-paths`),
which owns mechanism A and will need exactly this seam for
`webhook/process.rs`. Fold both items in:

- Inject `load_agent` behind a seam and rewrite
  `build_memory_namespaces_does_not_hold_the_lock_across_load_agent` so it fails
  against the chained body.
- That test restores `HOME` without an RAII guard, so a failing assertion leaks
  a temp `HOME` **and** poisons `TEST_HOME_LOCK` for the rest of the run. M4
  phase-06 fixed this same class ("HOME-leak → RAII guard"); apply the guard
  when the test is rewritten.

**Nit, not filed:** the `// ── Delegation depth tracking` marker at `mod.rs:186`
now heads a comment-only stub. The spec permitted keeping the marker, so this is
conformance, but the follow-on line restates what the code does
(`STANDARDS.md` §2.3). Delete both lines whenever this region is next touched.

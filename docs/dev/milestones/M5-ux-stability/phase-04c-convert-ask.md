# Phase 04c: Convert `ask.rs` Lock Sites

**Milestone:** M5 — UX & Stability
**Status:** in-progress
**Depends on:** phase-04b (`handlers.rs` converted) — `done`
**Estimated diff:** ~180 lines
**Tags:** language=rust, kind=refactor, size=m

## Goal

Convert the 13 `sessions.lock()` sites in `src/daemon/server/ask.rs` to the
`with_sessions` accessor. Unlike `handlers.rs`, several are
`sessions.lock().ok()?` chains inside `.and_then(…)` closures where `?`
propagation changes — each is spelled out below rather than left to pattern
matching.

## Architecture references

Read before starting:

- `docs/design/daemon-stalls.md` § 3.4 — phase ordering; `SessionStore` is still
  a plain type alias and stays that way until 04e.
- `docs/design/daemon-stalls.md` § 1.5c — the deadlock the accessor guards.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

The accessor from phase 04a:

```rust
// src/daemon/session.rs
pub fn with_sessions<T>(
    sessions: &SessionStore,
    f: impl FnOnce(&mut HashMap<String, SessionEntry>) -> T,
) -> T
```

`ask.rs` has **13** sites: lines 79, 100, 247, 265, 328, 468, 483, 508, 542,
551, 579, 590, 600.

### ⚠ The one thing that will silently deadlock this phase

`with_sessions`'s re-entrancy assertion only fires on
**`with_sessions` inside `with_sessions`**. It does **not** catch a
`with_sessions` closure that calls a function which still uses raw
`sessions.lock()` — that path **blocks forever** with no panic, no message, and
a hung test run.

`ask.rs` contains exactly one such call, at lines 571–575:

```rust
    let memory_namespaces_owned = crate::daemon::executor::build_memory_namespaces(
        session_id.as_deref(),
        sessions,
        is_ghost_session,
    );
```

`build_memory_namespaces` locks the store itself and is **not yet converted**
(it lives in `executor/mod.rs`, which is phase 04d):

```rust
// src/daemon/executor/mod.rs:86-93
    let mut namespaces: Vec<String> = Vec::new();
    if let Some(sid) = session_id
        && let Ok(store) = sessions.lock()          // ← raw lock, unconverted
        && let Some(entry) = store.get(sid)
        …
```

**Therefore: no `with_sessions` closure in this phase may contain that call, and
no conversion may merge sites across lines 571–575.** Site 551 is before it;
sites 579/590/600 are after it. They must not be combined.

### The `?`-chain shape (sites 328, 483, 508, 551, 579, 590, 600)

```rust
// src/daemon/server/ask.rs:551
    let ghost_turn_limit: Option<usize> = session_id.as_ref().and_then(|id| {
        let store = sessions.lock().ok()?;
        let entry = store.get(id)?;
        if !entry.is_ghost {
            return None;
        }
        …
        Some(limit)
    });
```

Two different `?`s are in play and they must be treated differently:

- `sessions.lock().ok()?` — this one **disappears**. `with_sessions` recovers
  from poison via `.unwrap_or_log()` instead of bailing out with `None`. Same
  deliberate behavior change as phase 04b: `CLAUDE.md` § "Important Invariants"
  requires `.unwrap_or_log()` at every lock site, and these were stragglers.
- `store.get(id)?` — this one **stays**, and now returns from the
  `with_sessions` closure rather than from the `and_then` closure. That is fine
  because the `with_sessions` closure returns the same `Option<T>` the
  `and_then` closure did; the value propagates outward unchanged.

Converted:

```rust
    let ghost_turn_limit: Option<usize> = session_id.as_ref().and_then(|id| {
        with_sessions(sessions, |store| {
            let entry = store.get(id)?;
            if !entry.is_ghost {
                return None;
            }
            …
            Some(limit)
        })
    });
```

### One-liner variants of the same shape (328, 483, 508)

```rust
// :328
    let started_at = session_id
        .as_ref()
        .and_then(|id| sessions.lock().ok()?.get(id).map(|e| e.started_at));

// :483
    let is_ghost_session = session_id
        .as_ref()
        .and_then(|id| sessions.lock().ok()?.get(id).map(|e| e.is_ghost))
        .unwrap_or(false);

// :508
    let default_target_pane: Option<String> = session_id
        .as_ref()
        .and_then(|id| sessions.lock().ok()?.get(id)?.default_target_pane.clone());
```

become

```rust
    let started_at = session_id
        .as_ref()
        .and_then(|id| with_sessions(sessions, |store| store.get(id).map(|e| e.started_at)));

    let is_ghost_session = session_id
        .as_ref()
        .and_then(|id| with_sessions(sessions, |store| store.get(id).map(|e| e.is_ghost)))
        .unwrap_or(false);

    let default_target_pane: Option<String> = session_id.as_ref().and_then(|id| {
        with_sessions(sessions, |store| {
            store.get(id)?.default_target_pane.clone()
        })
    });
```

Note the third: `store.get(id)?` keeps its `?` and the closure returns
`Option<String>`.

### `.ok().map(|mut store| …)` (site 468)

```rust
// :468
        .and_then(|id| {
            sessions.lock().ok().map(|mut store| {
                if let Some(entry) = store.get_mut(id) {
                    entry.turn_count += 1;
                    entry.turn_count
                } else {
                    1
                }
            })
        })
```

The `.ok().map(...)` wrapper exists only to handle poison. It collapses:

```rust
        .map(|id| {
            with_sessions(sessions, |store| {
                if let Some(entry) = store.get_mut(id) {
                    entry.turn_count += 1;
                    entry.turn_count
                } else {
                    1
                }
            })
        })
```

`and_then` becomes `map` because the closure no longer returns an `Option`.
Check the surrounding expression compiles — if the outer chain still expects an
`Option`, keep `and_then` and wrap the result in `Some(…)`. Let the compiler
decide; do not guess.

### `if let Ok(...)` shapes (sites 79, 100, 247, 265, 542)

These are the `handlers.rs` shapes and convert the same way: the body becomes
the closure, the `else` branch becomes the closure's fallback value.

**Site 100 carries pre-existing file I/O inside the critical section** —
`store.entry(id).or_insert_with(|| { … read_session_meta(id) … })`, and
`read_session_meta` does `std::fs::read_to_string` (`session.rs:199`). Preserve
it exactly as-is: wrap it in `with_sessions` without restructuring. Removing I/O
from that critical section is a real improvement but it is **not this phase** —
it changes session-restore semantics and belongs with the other mechanism-A work.

## Spec

### 1. Convert the ten straightforward sites

Lines 79, 100, 247, 265, 328, 468, 483, 508, 542, 551 — one `with_sessions`
each, per the shapes above. Site 551 must **not** be merged with anything after
line 571.

### 2. Collapse sites 579, 590, 600 into one acquisition

These three are consecutive, read the **same** entry, and have nothing between
them that touches the store. Today they take the lock three times:

```rust
    let tool_policy_owned: Option<crate::agents::ToolPolicy> = session_id.as_ref().and_then(|id| {
        let store = sessions.lock().ok()?;
        let entry = store.get(id)?;
        if !entry.is_ghost { return None; }
        entry.ghost_config.as_ref().and_then(|gc| gc.tool_policy.clone())
    });
    let agent_name_owned: Option<String> = session_id.as_ref().and_then(|id| {
        let store = sessions.lock().ok()?;
        let entry = store.get(id)?;
        if !entry.is_ghost { return None; }
        entry.ghost_config.as_ref().and_then(|gc| gc.agent.clone())
    });
    let (is_ghost_session, parent_job_id_owned): (bool, Option<String>) = session_id
        .as_ref()
        .and_then(|id| {
            let store = sessions.lock().ok()?;
            let entry = store.get(id)?;
            Some((
                entry.is_ghost,
                entry.ghost_config.as_ref().and_then(|gc| gc.parent_job_id.clone()),
            ))
        })
        .unwrap_or((false, None));
```

Replace with a single acquisition returning all four values:

```rust
    let (tool_policy_owned, agent_name_owned, is_ghost_session, parent_job_id_owned) =
        session_id
            .as_ref()
            .and_then(|id| {
                with_sessions(sessions, |store| {
                    let entry = store.get(id)?;
                    let ghost = entry.is_ghost;
                    let (policy, agent) = if ghost {
                        (
                            entry.ghost_config.as_ref().and_then(|gc| gc.tool_policy.clone()),
                            entry.ghost_config.as_ref().and_then(|gc| gc.agent.clone()),
                        )
                    } else {
                        (None, None)
                    };
                    let parent = entry
                        .ghost_config
                        .as_ref()
                        .and_then(|gc| gc.parent_job_id.clone());
                    Some((policy, agent, ghost, parent))
                })
            })
            .unwrap_or((None, None, false, None));
```

Two details this preserves and you must not drop:

- `tool_policy` and `agent` are `None` when the session is **not** a ghost (the
  original returned early via `if !entry.is_ghost { return None; }`).
- `parent_job_id` is read **regardless** of `is_ghost` — the third block had no
  ghost check. Do not add one.

This binding of `is_ghost_session` **shadows** the earlier one from line 483.
That shadowing exists today and must be preserved: the value from 483 is what
`build_memory_namespaces` receives at line 574, and the value from this block is
what `cost_attribution` receives below. Do not unify them.

### 3. Change nothing else

`SessionStore` stays a type alias. Do not touch `executor/mod.rs`,
`background.rs`, `ghost.rs`, or `stream.rs` — phase 04d.

## Acceptance criteria

- [ ] `cargo fmt --all` clean; `cargo build` succeeds.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits zero.
- [ ] `cargo test` green — and **the run completes**; a hang means a
      `with_sessions` closure encloses `build_memory_namespaces` or another
      unconverted locker.
- [ ] `grep -c "sessions.lock()" src/daemon/server/ask.rs` returns **0**.
- [ ] `grep -c "with_sessions(" src/daemon/server/ask.rs` returns **11** — 13
      sites minus the 3→1 collapse in spec 2.
- [ ] `grep -c "sessions.lock()" src/daemon/executor/mod.rs` is unchanged
      (still locks raw) — proving 04d's territory was not entered.
- [ ] `grep -n "pub type SessionStore" src/daemon/session.rs` still shows the
      alias.
- [ ] `cargo test --lib` reports **914** — unchanged. This phase adds no tests.

## Test plan

Behavior-preserving refactor: the existing 914 tests are the regression net and
must all still pass, unchanged. **Write no new tests** — the pinned count is 914
and a higher number means scope crept.

The risk this phase carries is a *hang*, not a failure, so treat a test run that
does not terminate as a specific diagnostic: some `with_sessions` closure
encloses a call that takes the lock the old way. Run `cargo test` with a timeout
and, if it stalls, look for a `with_sessions` block containing a call that takes
`sessions` as an argument.

Sanity-check one conversion for `?`-semantics before reporting complete: confirm
that for a session id **not** present in the store, `ghost_turn_limit` is still
`None` (the `store.get(id)?` path), rather than panicking or defaulting to the
ceiling. Reason it through from the code and state the reasoning in the Update
Log; no new test.

## End-to-end verification

**Do not attempt an interactive verification.** Do not launch tmux, the daemon,
or the chat client.

Write this under an "End-to-end verification" heading in the Update Log:

> Not applicable — behavior-preserving refactor of internal lock acquisition.
> The architect exercises the converted `ask` path against the real binary.

## Authorizations

None. No new dependencies; `docs/architecture.md` is not touched.

## Out of scope

- **Do not** convert `executor/mod.rs`, `background.rs`, `ghost.rs`,
  `stream.rs`, or any other file — phase 04d.
- **Do not** merge any conversion across lines 571–575
  (`build_memory_namespaces`). It still locks raw; enclosing it deadlocks.
- **Do not** restructure site 100 to move `read_session_meta` I/O out of the
  critical section, however tempting. Preserve it as-is.
- **Do not** unify the two `is_ghost_session` bindings (483 and the spec-2
  block). The shadowing is load-bearing.
- **Do not** convert `SessionStore` to a newtype — phase 04e.
- **Do not** add tests.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-26 01:48 (started)

**Executor:** claude-sonnet-4-5-20250514

Converting all 13 `sessions.lock()` sites in `ask.rs` to `with_sessions`.

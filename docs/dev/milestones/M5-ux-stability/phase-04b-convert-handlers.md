# Phase 04b: Convert `handlers.rs` Lock Sites

**Milestone:** M5 — UX & Stability
**Status:** in-progress
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

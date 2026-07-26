# Phase 04a: `with_sessions` Accessor + Re-entrancy Guard

**Milestone:** M5 — UX & Stability
**Status:** in-progress
**Depends on:** phase-02 (cleanup deadlock) — `done`
**Estimated diff:** ~150 lines
**Tags:** language=rust, kind=refactor, size=s

## Goal

Introduce the single accessor through which `SessionStore` will eventually be
reached — `with_sessions(&store, |map| …)` — with an always-on re-entrancy
assertion inside it, and convert the two live lock sites in `session.rs` and
`mod.rs`. This is the first of four phases; it establishes the pattern and the
guard, and deliberately leaves the other 98 sites alone.

## Architecture references

Read before starting:

- `docs/design/daemon-stalls.md` § 3 — why a structural answer, the survey
  behind it, and the four-phase ordering. § 3.4 in particular explains why the
  newtype is **not** part of this phase.
- `docs/design/daemon-stalls.md` § 1.5c — the deadlock this guard is designed to
  catch, with the gdb evidence.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

`SessionStore` is a bare type alias — **this phase does not change it**:

```rust
// src/daemon/session.rs:116
pub type SessionStore = Arc<Mutex<HashMap<String, SessionEntry>>>;
```

There are **100** `sessions.lock()` sites across the daemon. This phase converts
exactly **two**; the rest keep compiling untouched precisely because the type
alias is unchanged.

### Site 1 — `cleanup_pass`, `src/daemon/session.rs:399`

```rust
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

### Site 2 — shutdown pipe-pane sweep, `src/daemon/mod.rs:828-837`

```rust
    {
        let store = sessions.lock().unwrap_or_log();
        for (_, entry) in store.iter() {
            if let Some(ref pane_id) = entry.pipe_source_pane
                && !pane_id.is_empty()
            {
                crate::tmux::stop_pipe_pane(pane_id);
            }
        }
    }
```

Note this site calls `crate::tmux::stop_pipe_pane` — a blocking subprocess —
**while holding the lock**. That is mechanism A (design doc § 1.3) in the
shutdown path. Task 3 fixes it as part of the conversion, and the resulting
shape is the worked example the later conversion phases will follow.

### The poison idiom you must preserve

```rust
// src/util.rs:5-16
pub trait UnpoisonExt<T> {
    fn unwrap_or_log(self) -> T;
}

impl<'a, T> UnpoisonExt<MutexGuard<'a, T>> for LockResult<MutexGuard<'a, T>> {
    fn unwrap_or_log(self) -> MutexGuard<'a, T> {
        self.unwrap_or_else(|e| {
            log::error!("Recovering from poisoned Mutex lock");
            e.into_inner()
        })
    }
}
```

`CLAUDE.md` § "Important Invariants" requires every lock site to use
`.unwrap_or_log()` rather than `.unwrap()`. The accessor must use it internally,
which is what lets the call sites stop thinking about poison at all.

## Spec

### 1. Add the re-entrancy depth guard — `src/daemon/session.rs`

Add above `cleanup_pass`:

```rust
thread_local! {
    /// Depth of the current thread's `with_sessions` nesting. Only ever 0 or 1
    /// in correct code.
    static SESSIONS_LOCK_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// RAII depth counter for `with_sessions`. Decrements on drop, so the count is
/// correct even when the closure panics — otherwise one panicking test would
/// poison the counter for every later test on the same thread.
struct SessionsLockDepth;

impl SessionsLockDepth {
    fn enter() -> Self {
        SESSIONS_LOCK_DEPTH.with(|d| {
            assert_eq!(
                d.get(),
                0,
                "re-entrant SessionStore lock: with_sessions() called while this \
                 thread already holds the store. std::sync::Mutex is not reentrant \
                 — this would deadlock the whole daemon. Collect what you need \
                 inside the outer closure and use it after it returns. See \
                 docs/design/daemon-stalls.md § 1.5c."
            );
            d.set(1);
        });
        Self
    }
}

impl Drop for SessionsLockDepth {
    fn drop(&mut self) {
        SESSIONS_LOCK_DEPTH.with(|d| d.set(0));
    }
}
```

**Use `assert_eq!`, not `debug_assert_eq!`.** A re-entrant acquisition on one
thread is never legitimate — it would deadlock. Panicking is strictly better than
wedging: `supervise` restarts a panicked task, whereas the deadlock this replaces
took the daemon down for twelve hours. A `debug_assert` compiles out of exactly
the build where it matters.

### 2. Add the accessor — `src/daemon/session.rs`

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

The `_depth` binding must be a named binding (`let _depth = …`), **not** `let _ =
…`. `let _ =` drops the value immediately, which would disable the guard entirely
— the assertion would still fire on entry but the depth would already be back to
0, so nesting would not be caught. This is the single most likely way to get this
wrong.

`&mut HashMap` is the closure argument even for read-only callers; one signature
is simpler than a read/write pair, and `SessionStore` has no `RwLock` split.

### 3. Convert site 1 — `cleanup_pass`

Wrap the existing body. The whole function becomes:

```rust
pub fn cleanup_pass(
    sessions: &SessionStore,
    now: std::time::Instant,
    idle_after: std::time::Duration,
) -> (Vec<SessionEntry>, std::collections::HashSet<String>) {
    with_sessions(sessions, |store| {
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
    })
}
```

Behavior is identical — same single acquisition, same return value. Its two
existing tests must keep passing unchanged.

### 4. Convert site 2 — and hoist the subprocess out of the lock

`src/daemon/mod.rs:828-837`. Collect the pane ids inside the closure, then call
`stop_pipe_pane` **after** it returns:

```rust
    {
        let pipe_panes: Vec<String> = crate::daemon::session::with_sessions(&sessions, |store| {
            store
                .values()
                .filter_map(|entry| entry.pipe_source_pane.clone())
                .filter(|pane_id| !pane_id.is_empty())
                .collect()
        });
        for pane_id in &pipe_panes {
            crate::tmux::stop_pipe_pane(pane_id);
        }
    }
```

This is the shape every later conversion phase should follow when a site does
I/O or spawns a subprocess under the lock: **collect inside, act outside.**

### 5. Do not touch anything else

`SessionStore` stays `pub type SessionStore = Arc<Mutex<…>>`. The other 98 lock
sites, and all 13 `Arc::clone(&sessions…)` sites, are untouched and must keep
compiling. The newtype conversion is phase 04d.

`bg_sn.lock()` at `src/daemon/mod.rs:630` is a **different mutex** (the
background-job map), not `SessionStore`. Leave it alone.

## Acceptance criteria

- [ ] `cargo fmt --all` clean; `cargo build` succeeds.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits zero.
- [ ] `cargo test` green.
- [ ] Tests `with_sessions_runs_closure_and_releases_lock`,
      `with_sessions_rejects_reentrant_call`, and
      `with_sessions_depth_resets_after_panic` pass.
- [ ] `grep -n "sessions.lock()" src/daemon/session.rs` shows the call **only**
      inside `with_sessions` — `cleanup_pass` no longer locks directly.
- [ ] `grep -c "sessions.lock()" src/daemon/mod.rs` returns 0.
- [ ] `grep -n "pub type SessionStore" src/daemon/session.rs` still shows the
      `Arc<Mutex<…>>` alias — unchanged.
- [ ] `cargo test --lib` reports **913** passing — 910 now, plus exactly the
      three new tests. A higher count means scope crept.

## Test plan

All three go in the existing `mod tests` in `src/daemon/session.rs`, which
already has the `entry_with(last_accessed)` helper and a `SessionStore`
construction idiom to copy:

```rust
let sessions: SessionStore = Arc::new(Mutex::new(HashMap::new()));
```

- `with_sessions_runs_closure_and_releases_lock` in `src/daemon/session.rs` —
  insert one entry, then call `with_sessions(&sessions, |s| s.len())`. Assert the
  returned value is `1` (the closure's value is passed through), and that
  `sessions.try_lock().is_ok()` afterwards (the guard was released). Use
  `try_lock`, never `lock()`, so a regression fails fast instead of hanging CI.

- `with_sessions_rejects_reentrant_call` in `src/daemon/session.rs` — a
  `#[should_panic(expected = "re-entrant SessionStore lock")]` test that calls
  `with_sessions` and, inside the closure, calls `with_sessions` again on the
  same store. Match on the message substring so the test pins *which* panic
  fired, not merely that something panicked.

  This is the load-bearing test: it is the automated version of the defect that
  wedged the daemon for twelve hours.

- `with_sessions_depth_resets_after_panic` in `src/daemon/session.rs` — the
  negative case that a naive non-RAII implementation fails. Call `with_sessions`
  with a closure that panics, catching it with
  `std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| …))`, then call
  `with_sessions` again on the same thread and assert it succeeds. Without the
  `Drop` impl the depth stays at 1 and the second call panics with the
  re-entrancy message even though nothing is nested.

  Silence the panic backtrace noise for this test with
  `std::panic::set_hook`/`take_hook` if it clutters the output, but do not
  suppress the assertion itself.

Also confirm the two existing `cleanup_pass_*` tests still pass unchanged — the
conversion must not alter behavior.

## End-to-end verification

**Do not attempt an interactive verification.** Do not launch tmux, the daemon,
or the chat client.

Write this under an "End-to-end verification" heading in the Update Log:

> Not applicable — this phase ships no user-visible behavior change. It converts
> two call sites to an accessor with identical semantics and adds an assertion
> that fires only on a defect. The architect confirms the daemon still starts and
> shuts down cleanly.

## Authorizations

None. No new dependencies; `docs/architecture.md` is not touched.

## Out of scope

- **Do not** convert `SessionStore` to a newtype. That is phase 04d, and doing it
  here breaks all 13 `Arc::clone(&sessions…)` sites at once (design doc § 3.4).
- **Do not** convert any of the other 98 lock sites — not `handlers.rs`, not
  `ask.rs`, not `background.rs`, not `ghost.rs`, not `stream.rs`. Those are
  phases 04b/04c. A diff touching those files is out of scope even if the
  conversion looks trivial.
- **Do not** fix the mechanism-A sites in `webhook/process.rs` (`:148`, `:161`).
  Separate phase.
- **Do not** change `cleanup_pass`'s signature, return type, or behavior — only
  the body's locking form (task 3).
- **Do not** touch `bg_sn.lock()` at `mod.rs:630`; different mutex.
- **Do not** add a `with_sessions_read` / `with_sessions_write` pair, an async
  variant, or a `try_with_sessions`. One synchronous accessor.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-26 00:37 (started)

**Executor:** Claude Sonnet 4.5

Implemented tasks 1–5: added the `SESSIONS_LOCK_DEPTH` thread-local guard with RAII `SessionsLockDepth`, the `with_sessions` accessor, converted `cleanup_pass` and the shutdown pipe-pane sweep to use it, and added three tests. `cargo test --lib` reports 913 passing (910 + 3 new).

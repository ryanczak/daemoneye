# Phase 04e: Convert the `executor/` Tail — `foreground.rs` + `knowledge/*`

**Milestone:** M5 — UX & Stability
**Status:** done
**Depends on:** phase-04d (`executor/mod.rs` converted) — `done`
**Estimated diff:** ~130 lines
**Tags:** language=rust, kind=refactor, size=m

## Goal

Convert the last 8 `sessions.lock()` sites under `src/daemon/executor/` to
`with_sessions`, finishing the executor subtree. Two of the eight cannot be
converted mechanically: they hold the guard across an early `return` from the
**enclosing function**, so a naive wrap changes control flow.

**Finish condition: 8 `with_sessions` calls for 8 former sites, and zero
`sessions.lock()` anywhere under `src/daemon/executor/`.** There is no collapse
in this phase — every site reads a different thing at a different point.

## Architecture references

Read before starting:

- `docs/design/daemon-stalls.md` § 3.5 — the migration hazard. A converted
  closure that encloses a call which still uses **raw** `.lock()` deadlocks
  silently: no panic, no log, just a hung test run. Task 9 tabulates this
  phase's unconverted callees.
- `docs/design/daemon-stalls.md` § 3.4 — why the newtype is not part of this
  phase.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Verify the starting state — earlier phases move line numbers:

```bash
grep -c "sessions\.lock()" src/daemon/executor/foreground.rs        # expect 4
grep -c "sessions\.lock()" src/daemon/executor/knowledge/mod.rs     # expect 1
grep -c "sessions\.lock()" src/daemon/executor/knowledge/pane.rs    # expect 2
grep -c "sessions\.lock()" src/daemon/executor/knowledge/ghost.rs   # expect 1
grep -c "sessions\.lock()" src/daemon/executor/mod.rs               # expect 0 (04d landed)
grep -c "with_sessions("   src/daemon/executor/mod.rs               # expect 6
```

If any of the first four counts differs, **stop and report a blocker** — the
per-site line numbers below are stale and guessing which site is which is how a
conversion phase corrupts a file.

## Current state

`SessionStore` is still the bare type alias, so unconverted sites elsewhere keep
compiling:

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

Generic over `T`, hands you `&mut HashMap`. A closure may return anything — a
tuple, a struct, an `Option`, a `Result`.

### Imports you must extend

None of the four files import `with_sessions` today. Current lines:

```rust
// src/daemon/executor/foreground.rs:7
use crate::daemon::session::{FG_HOOK_COUNTER, bg_done_subscribe};

// src/daemon/executor/knowledge/mod.rs:21
use crate::daemon::session::SessionStore;

// src/daemon/executor/knowledge/pane.rs:2
use crate::daemon::session::{
    FG_HOOK_COUNTER, SessionStore, append_session_message, bg_done_subscribe,
};

// src/daemon/executor/knowledge/ghost.rs:2
use crate::daemon::session::SessionStore;
```

Add `with_sessions` to each brace list (creating one where the import is a single
path). Keep the existing items and the alphabetical-ish ordering the file already
uses; `cargo fmt` will settle the rest.

### Site inventory

| # | File:line | Shape |
|---|---|---|
| 1 | `foreground.rs:170` | `.and_then(\|sid\| sessions.lock().ok()?…)` |
| 2 | `foreground.rs:199` | same shape as #1, different guard branch |
| 3 | `foreground.rs:232` | **guard held across `cache.panes.read()` + `return` inside an IIFE** |
| 4 | `foreground.rs:885` | `let`-chain inside a block expression, assigns a local |
| 5 | `knowledge/mod.rs:38` | `let Ok(mut store) = … else { return }`, `get_mut`, write |
| 6 | `knowledge/pane.rs:19` | **guard held across three `return`s from the enclosing fn** |
| 7 | `knowledge/pane.rs:52` | `let`-chain, `get_mut`, write |
| 8 | `knowledge/ghost.rs:69` | `let`-chain, `get_mut`, write |

## Spec

### 1. `foreground.rs:170` — invalid-pane-format guard

```rust
            let correct = session_id
                .and_then(|sid| sessions.lock().ok()?.get(sid)?.default_target_pane.clone())
                .unwrap_or_default();
```

becomes:

```rust
            let correct = session_id
                .and_then(|sid| {
                    with_sessions(sessions, |store| {
                        store.get(sid)?.default_target_pane.clone()
                    })
                })
                .unwrap_or_default();
```

The `?` operators bind to the **closure** (return type `Option<String>`), and
`with_sessions` passes that `Option` out for `and_then` to flatten. Same shape
04c used throughout `ask.rs`.

### 2. `foreground.rs:199` — stale-pane guard

Byte-identical to #1. Apply the same rewrite.

**Do not hoist #1 and #2 into a single read before both guards.** They sit in
two different error branches, and both are reached only when the AI passed a bad
`target_pane`. Hoisting would take the lock on **every** foreground command
including the happy path, which is strictly worse. Two sites, two calls.

### 3. `foreground.rs:232` — the IIFE, extract before the closure

This is the same trap as `executor/mod.rs:922` in the previous phase, one level
deeper. Current code:

```rust
    let target_hint: Option<String> = (|| {
        if let Some(tp) = target
            && chat_pane != Some(tp)
        {
            let panes = cache.panes.read().unwrap_or_log();
            if panes.contains_key(tp) {
                return Some(tp.to_string());
            }
        }
        if let Some(sid) = session_id
            && let Ok(store) = sessions.lock()
            && let Some(entry) = store.get(sid)
            && let Some(ref dtp) = entry.default_target_pane
            && chat_pane != Some(dtp.as_str())
        {
            let panes = cache.panes.read().unwrap_or_log();
            if panes.contains_key(dtp) {
                return Some(dtp.clone());
            }
        }
        None
    })();
```

Two defects, one rewrite:

- The sessions guard is held across `cache.panes.read()` — a second lock taken
  inside the first.
- `return Some(dtp.clone())` returns from the **IIFE**. Move it inside a
  `with_sessions` closure and it returns from *that* closure instead, so the IIFE
  falls through to `None` and `target_hint` silently becomes `None` on the path
  that should have produced a hint. **This compiles.** It is a silent behavior
  change, not a build error — which is exactly why it is called out.

Target — read the default pane *before* entering the IIFE:

```rust
    let default_target: Option<String> = session_id.and_then(|sid| {
        with_sessions(sessions, |store| {
            store.get(sid)?.default_target_pane.clone()
        })
    });
    let target_hint: Option<String> = (|| {
        if let Some(tp) = target
            && chat_pane != Some(tp)
        {
            let panes = cache.panes.read().unwrap_or_log();
            if panes.contains_key(tp) {
                return Some(tp.to_string());
            }
        }
        if let Some(ref dtp) = default_target
            && chat_pane != Some(dtp.as_str())
        {
            let panes = cache.panes.read().unwrap_or_log();
            if panes.contains_key(dtp) {
                return Some(dtp.clone());
            }
        }
        None
    })();
```

The sessions lock is released before either `cache.panes.read()`, and both
`return`s stay inside the IIFE where they belong.

This does move the read earlier — it now happens even when the first branch
returns a hint from `target`. That is an unconditional single read replacing a
conditional one, and it is the correct trade: it is one acquisition either way,
and it is what removes the nested lock. Do **not** try to preserve the laziness
with a second `with_sessions` inside the IIFE.

### 4. `foreground.rs:885` — retry-window name lookup

```rust
        let win_name: String = {
            let mut name = pane_id.to_string();
            if let Some(sid) = session_id
                && let Ok(store) = sessions.lock()
                && let Some(entry) = store.get(sid)
                && let Some(w) = entry.bg_windows.iter().find(|w| w.pane_id == pane_id)
            {
                name = w.window_name.clone();
            }
            name
        };
```

becomes:

```rust
        let win_name: String = session_id
            .and_then(|sid| {
                with_sessions(sessions, |store| {
                    store
                        .get(sid)?
                        .bg_windows
                        .iter()
                        .find(|w| w.pane_id == pane_id)
                        .map(|w| w.window_name.clone())
                })
            })
            .unwrap_or_else(|| pane_id.to_string());
```

The `unwrap_or_else` preserves the original default (`pane_id.to_string()`) for
every miss path: no session id, session absent, or no matching window.

### 5. `knowledge/mod.rs:38` — `track_artifact`

```rust
fn track_artifact(ctx: &ArtifactCtx<'_>, kind: &str, name: &str) {
    if ctx.is_ghost {
        return;
    }
    let Some(sid) = ctx.session_id else { return };
    let Ok(mut store) = ctx.sessions.lock() else {
        return;
    };
    if let Some(entry) = store.get_mut(sid) {
        entry
            .artifacts_created
            .push(crate::session_store::ArtifactRef {
                kind: kind.to_string(),
                name: name.to_string(),
                at_turn: ctx.turn_count,
            });
    }
}
```

becomes:

```rust
fn track_artifact(ctx: &ArtifactCtx<'_>, kind: &str, name: &str) {
    if ctx.is_ghost {
        return;
    }
    let Some(sid) = ctx.session_id else { return };
    with_sessions(ctx.sessions, |store| {
        if let Some(entry) = store.get_mut(sid) {
            entry
                .artifacts_created
                .push(crate::session_store::ArtifactRef {
                    kind: kind.to_string(),
                    name: name.to_string(),
                    at_turn: ctx.turn_count,
                });
        }
    });
}
```

The `let Ok(…) else { return }` disappears: `with_sessions` recovers from poison
via `.unwrap_or_log()` rather than silently skipping the write. That is the
intended direction of this whole sequence and matches the `CLAUDE.md` invariant —
a poisoned lock should log and proceed, not drop an artifact record on the floor.
Keep the first two early returns exactly as they are; they are not lock-related.

### 6. `knowledge/pane.rs:19` — three `return`s inside the locked block

The hardest site in this phase. Current code:

```rust
pub fn close_bg_window(pane_id: &str, session_id: Option<&str>, sessions: &SessionStore) -> String {
    let Some(sid) = session_id else {
        return "No active session — cannot close background window.".to_string();
    };
    let (win_name, tmux_session, still_running) = {
        let store = sessions.lock().unwrap_or_log();
        let Some(entry) = store.get(sid) else {
            return format!("Session '{}' not found.", sid);
        };
        let Some(win) = entry.bg_windows.iter().find(|w| w.pane_id == pane_id) else {
            return format!(
                "No background window with pane ID {} found in this session.",
                pane_id
            );
        };
        (
            win.window_name.clone(),
            win.tmux_session.clone(),
            win.exit_code.is_none(),
        )
    };
```

The two `return format!(…)` statements return **from `close_bg_window`**, and
each carries a distinct user-facing error string. Wrapping the block in
`with_sessions` unchanged would make them return from the closure — a type error
at best, and at worst a silent swap of the function's error strings for a
`String` that gets bound to `win_name`.

Have the closure hand back a `Result` and match on it outside:

```rust
    let looked_up: Result<(String, String, bool), String> =
        with_sessions(sessions, |store| {
            let Some(entry) = store.get(sid) else {
                return Err(format!("Session '{}' not found.", sid));
            };
            let Some(win) = entry.bg_windows.iter().find(|w| w.pane_id == pane_id) else {
                return Err(format!(
                    "No background window with pane ID {} found in this session.",
                    pane_id
                ));
            };
            Ok((
                win.window_name.clone(),
                win.tmux_session.clone(),
                win.exit_code.is_none(),
            ))
        });
    let (win_name, tmux_session, still_running) = match looked_up {
        Ok(v) => v,
        Err(msg) => return msg,
    };
```

Both `return`s inside the closure now return **from the closure** as `Err`, and
the `match` re-raises them as returns from `close_bg_window` with byte-identical
strings. Do not reword either message — they are user-facing tool output.

### 7. `knowledge/pane.rs:52` — window removal

```rust
    if let Ok(mut store) = sessions.lock()
        && let Some(entry) = store.get_mut(sid)
    {
        entry.bg_windows.retain(|w| w.pane_id != pane_id);
    }
```

becomes:

```rust
    with_sessions(sessions, |store| {
        if let Some(entry) = store.get_mut(sid) {
            entry.bg_windows.retain(|w| w.pane_id != pane_id);
        }
    });
```

`sid` is already bound and non-optional at this point, so no `if let Some(sid)`
wrapper is needed.

### 8. `knowledge/ghost.rs:69` — ghost task message

```rust
        Ok(sid) => {
            let job_id = sid.clone();
            let task_message = message.to_string();
            if let Ok(mut store) = sessions.lock()
                && let Some(entry) = store.get_mut(&sid)
            {
                entry.ghost_task_message = Some(task_message);
            }
            inject_ghost_event(
```

becomes:

```rust
        Ok(sid) => {
            let job_id = sid.clone();
            let task_message = message.to_string();
            with_sessions(sessions, |store| {
                if let Some(entry) = store.get_mut(&sid) {
                    entry.ghost_task_message = Some(task_message);
                }
            });
            inject_ghost_event(
```

**The closure must end before `inject_ghost_event`.** See task 9 — that call is a
store-toucher this phase does not convert. `task_message` is moved into the
closure, which is fine because nothing after uses it.

### 9. Do not let any closure span these unconverted callees

A `with_sessions` closure enclosing a raw `sessions.lock()` **deadlocks
silently** — no panic, no log, a hung test run (`daemon-stalls.md` § 3.5). The
re-entrancy assertion only catches `with_sessions` nested inside `with_sessions`,
never `with_sessions` enclosing a raw `.lock()`.

Store-touching calls reachable from this phase's files that stay raw:

| Callee | Raw lock at | Reached from |
|---|---|---|
| `inject_ghost_event` → `inject_into_sessions` / `notify_chat_panes` | `webhook/process.rs` (2 sites) | `knowledge/ghost.rs`, immediately after task 8's closure |
| `GhostManager::start_session_with_config` | `ghost.rs` (11 sites) | `knowledge/ghost.rs:52`, before task 8's closure |
| `respawn_background_in_pane`, `run_background_in_window` | `background/` (9 sites) | `foreground.rs:6` import, called after the approval gate |

All three already sit outside the regions tasks 1–8 touch. **Keep it that way** —
do not widen a closure to "tidy up" adjacent code.

`append_session_message` (imported in `pane.rs:2`) is **not** a store-toucher —
it is file I/O only (`session.rs:281`). It still must not go inside a closure,
because blocking I/O under the lock is mechanism A, but it will not deadlock.

`with_sessions` takes a synchronous `FnOnce`, so an `.await` inside one will not
compile. That is a guardrail, not an obstacle to work around — `knowledge/ghost.rs`
has an `.await` at line 52 and it must stay outside.

## Acceptance criteria

**These greps count raw text, comments included.** Two consequences, both
load-bearing:

- **Do not write the literal `sessions.lock()` in a comment** in any file under
  `src/daemon/executor/`. A comment like `// replaces sessions.lock()` breaks the
  first criterion even though the code is correct.
- **Do not write `with_sessions(` in a comment** either, for the same reason
  against the per-file counts.

The `use` lines are safe: `with_sessions` in an import has no trailing `(`, so it
does not count toward the call totals. Verified against the current imports.

- [ ] `grep -rc "sessions\.lock()" src/daemon/executor/` reports **0** for every
      file in the subtree.
- [ ] `grep -c "with_sessions(" src/daemon/executor/foreground.rs` returns **4**.
- [ ] `grep -c "with_sessions(" src/daemon/executor/knowledge/mod.rs` returns **1**.
- [ ] `grep -c "with_sessions(" src/daemon/executor/knowledge/pane.rs` returns **2**.
- [ ] `grep -c "with_sessions(" src/daemon/executor/knowledge/ghost.rs` returns **1**.
- [ ] `grep -c "with_sessions(" src/daemon/executor/mod.rs` still returns **6**
      (04d's work untouched).
- [ ] `grep -n "pub type SessionStore" src/daemon/session.rs` still shows the alias.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **915** lib-unit tests — unchanged. This phase
      adds no tests; a higher number means scope crept.
- [ ] `cargo test` completes **without hanging**. A conversion regression in this
      milestone manifests as a hang, not a red gate — if the suite stops
      progressing, a closure is spanning a raw `.lock()` (task 9), not a slow
      machine.

## Test plan

Behavior-preserving refactor: the existing **915** tests are the regression net
and must all still pass, unchanged. **Write no new tests.**

Two sites change control flow and deserve a reasoning check before you report
complete. State the reasoning in the Update Log; no new test for either.

- **Task 3** — confirm that when `target` names a pane present in the cache, the
  IIFE still returns `Some(target)` and does **not** fall through to the
  default-pane branch. The first branch is unchanged, so this is a read-through,
  not an experiment.
- **Task 6** — confirm all three `close_bg_window` early-exit strings are
  byte-identical to the originals: the no-session case (untouched), the
  session-absent case, and the no-matching-window case. Quote all three in the
  Update Log.

## End-to-end verification

> Not applicable — phase ships no runtime-loadable artifact. Internal refactor of
> lock acquisition inside existing code paths; no CLI surface, no config key, no
> file the running binary loads.

**Do not attempt an interactive verification.** Do not launch tmux, the daemon, or
the chat client. Write the sentence above under an "End-to-end verification"
heading in the Update Log.

## Authorizations

None.

This phase adds no tests, so it needs no `unsafe` for `std::env::set_var` and no
`HOME` redirection. **If you find yourself wanting to add a test that redirects
`HOME`, stop and report a blocker** — that needs an authorization this phase does
not grant.

## Out of scope

- **Do not touch `src/daemon/executor/mod.rs`.** 04d converted it; its 6
  `with_sessions` calls and its `DispatchSnapshot` are done. An acceptance
  criterion pins the count.
- **Do not convert `webhook/process.rs`, `ghost.rs`, or `background/`.** Task 9
  lists them so you can avoid enclosing them, not so you can fix them.
  `webhook/process.rs` is phase 05; `ghost.rs` is 04g; `background/` is 04h.
- **Do not convert `context/background.rs`, `briefing.rs`, `stream.rs`, or
  `hook.rs`.** Separate phases.
- **Do not change `SessionStore` into a newtype** and do not touch the 13
  `Arc::clone` sites — that is 04j.
- **Do not reword any user-facing string.** Tasks 4 and 6 both preserve exact
  message text and default values.
- **Do not hoist tasks 1 and 2 into a shared read.** Task 2 explains why.
- **Do not add a second `with_sessions` inside the task-3 IIFE** to preserve the
  old laziness.
- **Do not add `#[allow(...)]` anywhere.** If clippy objects to the task-6
  `Result` shape, report a blocker rather than suppressing.
- **Do not remove any `unwrap_or_log()` on `cache.panes.read()`.** That is a
  different lock and a project invariant (`CLAUDE.md` § "Important Invariants").

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-26 14:08 (started)

**Executor:** rexyMCP executor

Converted all 8 remaining `sessions.lock()` sites under `src/daemon/executor/` to `with_sessions`:

- **Task 1** (`foreground.rs:170`): invalid-pane-format guard — wrapped in `with_sessions` inside `and_then`, same shape as 04c.
- **Task 2** (`foreground.rs:199`): stale-pane guard — identical rewrite.
- **Task 3** (`foreground.rs:232`): IIFE with guard held across `cache.panes.read()` — extracted `default_target` read before the IIFE, replaced the `let`-chain with `default_target` reference inside. When `target` names a pane present in the cache, the IIFE still returns `Some(target)` from the first branch unchanged; the `default_target` read is a no-op on that path.
- **Task 4** (`foreground.rs:885`): retry-window name lookup — replaced block expression with `and_then` + `with_sessions` + `unwrap_or_else(|| pane_id.to_string())`.
- **Task 5** (`knowledge/mod.rs:38`): `track_artifact` — replaced `let Ok(mut store) = ... else { return }` with `with_sessions` closure.
- **Task 6** (`knowledge/pane.rs:19`): `close_bg_window` — closure returns `Result<(String, String, bool), String>`, matched outside to preserve the three distinct user-facing error strings. All three strings are byte-identical to originals:
  1. `"No active session — cannot close background window."` (untouched, early return before the closure)
  2. `"Session '{}' not found."` (from `Err` in closure, re-raised by `match`)
  3. `"No background window with pane ID {} found in this session."` (from `Err` in closure, re-raised by `match`)
- **Task 7** (`knowledge/pane.rs:52`): window removal — replaced `if let Ok(mut store)` with `with_sessions`.
- **Task 8** (`knowledge/ghost.rs:69`): ghost task message — replaced `if let Ok(mut store)` with `with_sessions`, closure ends before `inject_ghost_event`.
- **Task 9**: Verified no closure spans unconverted callees (`inject_ghost_event`, `GhostManager::start_session_with_config`, `respawn_background_in_pane`, `run_background_in_window`). All remain outside converted closures.

Verification:
- `grep -rc "sessions\.lock()" src/daemon/executor/` — all files report 0
- `grep -c "with_sessions(" src/daemon/executor/foreground.rs` — 4
- `grep -c "with_sessions(" src/daemon/executor/knowledge/mod.rs` — 1
- `grep -c "with_sessions(" src/daemon/executor/knowledge/pane.rs` — 2
- `grep -c "with_sessions(" src/daemon/executor/knowledge/ghost.rs` — 1
- `grep -c "with_sessions(" src/daemon/executor/mod.rs` — 6 (unchanged)
- `cargo build` — clean
- `cargo clippy --all-targets --all-features -- -D warnings` — clean
- `cargo fmt --all` — clean
- `cargo test` — 915 passed, 0 failed (unchanged), no hangs

End-to-end verification: Not applicable — phase ships no runtime-loadable artifact. Internal refactor of lock acquisition inside existing code paths; no CLI surface, no config key, no file the running binary loads.

### Update — ts=1785075616454 (complete, server-authored)

**Summary:** **Summary + Notes for review:**

Converted all 8 remaining `sessions.lock()` sites under `src/daemon/executor/` to `with_sessions`, finishing the executor subtree migration. Two non-mechanical conversions required special handling: Task 3 extracted the `default_target` read before the IIFE to avoid holding the sessions guard across `cache.panes.read()` (the first IIFE branch remains unchanged and still returns `Some(target)` when the target pane is in the cache); Task 6 used a `Result<(String, String, bool), String>` return from the closure to preserve three distinct user-facing error strings from `close_bg_window`, all byte-identical to originals. No closure spans any unconverted callee (`inject_ghost_event`, `GhostManager`, `background/`). All gates passed: `cargo build`, `clippy`, `fmt`, and 915 tests with zero failures and no hangs.

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
client_viewport_shown_when_known ... ok
test tmux::cache::tests::get_labeled_context_client_viewport_absent_when_zero ... ok
test tmux::cache::tests::get_labeled_context_background_panes_sorted ... ok
test tmux::cache::tests::get_labeled_context_pane_classification ... ok
test tmux::cache::tests::get_labeled_context_copy_mode_annotated ... ok
test tmux::cache::tests::get_labeled_context_dead_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_session_topology ... ok
test tmux::cache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test tmux::cache::tests::get_labeled_context_chat_pane_excluded_from_background ... ok
test search::tests::search_events_returns_tail_not_head_when_segment_exceeds_cap ... ok
test search::tests::search_finds_match_in_runbooks ... ok
test search::tests::search_respects_kind_filter ... ok
test session_store::tests::artifacts_round_trip ... ok
test search::tests::search_returns_empty_for_no_match ... ok
test session_store::tests::backfill_missing_artifact_returns_error_name ... ok
test memory::tests::migrate_namespace_adds_missing ... ok
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

test result: ok. 915 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.38s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 29 tests
test daemon_ping_status_loop ... ignored
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test window_switch_does_not_corrupt_chat ... ignored
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_ask_round_trip ... ok
test ghost_config_parsing ... ok
test event_log_append_read ... ok
test minimal_config_parsing ... ok
test schedule_store_persistence ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test g4_briefing_read_and_clear ... ok
test config_pricing_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test g4_briefing_masking_applied ... ok
test session_jsonl_round_trip ... ok
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
- `docs/dev/milestones/M5-ux-stability/phase-04e-convert-executor-tail.md` — +34 -1
- `src/daemon/executor/foreground.rs` — +29 -18
- `src/daemon/executor/knowledge/ghost.rs` — +6 -6
- `src/daemon/executor/knowledge/mod.rs` — +12 -13
- `src/daemon/executor/knowledge/pane.rs` — +16 -13

**Commit:** 3f39732e45096313491ad158a3ae236ebd633f34

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-07-26

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** Qwen/Qwen3.6-27B-FP8 (84 turns)
- **Scope deviations:** none. All 9 spec tasks implemented as written;
  `executor/mod.rs`, `webhook/process.rs`, `ghost.rs`, `background/`,
  `context/background.rs`, `briefing.rs`, `stream.rs`, `hook.rs`, and
  `SessionStore` all untouched as Out-of-scope required.
- **Calibration:** none. **The executor subtree is now fully converted.**

**Independent re-run at review** (separate invocations, not chained):

```
cargo fmt --all --check                                    → exit 0
cargo build                                                → exit 0, no warnings
cargo clippy --all-targets --all-features -- -D warnings   → exit 0
cargo test  → 915 lib-unit passed / 0 failed (unchanged — no new tests)
              27 integration passed / 2 ignored
              run terminated normally (no hang)
```

**Acceptance criteria — every count exact:**

| Criterion | Result |
|---|---|
| raw `sessions.lock()` anywhere under `src/daemon/executor/` | **0** ✓ |
| `with_sessions(` in `foreground.rs` | **4** ✓ |
| `with_sessions(` in `knowledge/mod.rs` | **1** ✓ |
| `with_sessions(` in `knowledge/pane.rs` | **2** ✓ |
| `with_sessions(` in `knowledge/ghost.rs` | **1** ✓ |
| `with_sessions(` in `executor/mod.rs` (04d untouched) | **6** ✓ |
| `pub type SessionStore` still an alias | `session.rs:117` ✓ |
| lib-unit test count | **915**, unchanged ✓ |
| `cargo test` terminates | ✓ |

The comment prohibition held — the counts came out exact, so nothing wrote the
literal `sessions.lock()` or `with_sessions(` into a comment.

**The two non-mechanical rewrites were both done correctly.** This was the risk
the phase carried, and both were verified by reading the code, not the summary:

- **Task 3 (`foreground.rs:232`)** — the read is hoisted *above* the IIFE. The
  first branch is byte-identical (`return Some(tp.to_string())` intact), the
  second reads `default_target` with no lock held, both `cache.panes.read()`
  calls sit outside any sessions closure, and **both `return`s remain inside the
  IIFE**. The failure mode this guarded against — moving the `return` into a
  `with_sessions` closure so the IIFE falls through and `target_hint` silently
  becomes `None`, which *compiles* — did not occur.
- **Task 6 (`knowledge/pane.rs:19`)** — the closure returns
  `Result<(String, String, bool), String>` and the caller matches, re-raising
  each `Err` as a return from `close_bg_window`. All three user-facing strings
  verified byte-identical against the parent commit by literal `grep -cF`:
  identical occurrence counts before and after for "No active session — cannot
  close background window.", "Session '{}' not found.", and "No background window
  with pane ID {} found in this session."

**Task 9 confirmed structurally:** task 8's closure closes with `});` before
`inject_ghost_event(` (which reaches `webhook/process.rs`'s raw locks);
`GhostManager::start_session_with_config` and the `background/` respawn helpers
likewise sit outside every converted closure. The terminating test run
corroborates — the §3.5 hazard manifests as a hang, and there was none.

No forbidden idioms in the added lines: no `unsafe`, `#[allow]`, `#[ignore]`,
`dbg!`, `println!`, `TODO`/`FIXME`/`XXX`, `unwrap()`, or `expect()`.

**Residual risk, accepted and recorded rather than papered over:** `target_hint`
has no unit-test coverage, before this phase or after. The task-3 failure mode
would degrade the approval prompt's pane hint to `None` without failing any test.
This phase did not *reduce* coverage, and the spec deliberately chose reasoning
plus review over inventing a test for an approval-prompt string — but the gap is
real. If a later phase touches `find_best_target_pane` or `target_hint` again,
that is the moment to add coverage.

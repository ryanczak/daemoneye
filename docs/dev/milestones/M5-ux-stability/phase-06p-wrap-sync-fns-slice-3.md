# Phase 06p: Wrap Blocking Sync Functions — Slice 3 (the executor two)

**Milestone:** M5 — UX & Stability
**Status:** in-progress
**Depends on:** phase-06m — `done` (wrap-the-caller, slices 1–2)
**Estimated diff:** ~35 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Apply the wrap-the-caller pattern to the **last two call sites of the wrap set
that need no shape change**: `knowledge::watch_pane` and
`knowledge::close_bg_window`, both called from `execute_tool_call` in
`src/daemon/executor/mod.rs`.

| Sync helper | Blocking work inside | Call sites |
|---|---|---|
| `knowledge::watch_pane` | `pane_current_command` + one `tmux set-hook`, in its **prologue** | 1 |
| `knowledge::close_bg_window` | one `tmux kill_job_window` | 1 |

Both differ from slices 1–2 in one way: **each returns a `String` the AI model
reads**, so the timeout fallback is user-visible text, not a mechanical default.
This doc pins that text byte-for-byte.

**Finish condition: both call sites are inside an `off_runtime` closure, exactly
one `src/` file changes, and `knowledge/pane.rs` is not edited at all.**

## Architecture references

- `docs/design/daemon-stalls.md` § 1 mechanism B.
- `src/tmux/mod.rs:29` — the `off_runtime` adapter and `TMUX_TIMEOUT`.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture reference above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Verify the starting state:

```bash
grep -c "off_runtime" src/daemon/executor/mod.rs                      # expect 0
grep -c "off_runtime" src/daemon/executor/knowledge/pane.rs           # expect 7
grep -c "watch_pane("       src/daemon/executor/mod.rs                # expect 1
grep -c "close_bg_window("  src/daemon/executor/mod.rs                # expect 1
grep -c "close_bg_window("  src/daemon/executor/knowledge/pane.rs     # expect 3
cargo test 2>&1 | grep "^test result" | head -3   # expect 916 lib, 0, 27 integration
```

**Every number above was produced by running that exact command against the tree
while drafting.** If one differs, **stop and report a blocker**.

## Current state

### ⭐ The pattern, already proven twice in this tree

Slices 1 and 2 wrapped twelve call sites this way and **edited no helper and no
test**. The `String`-returning shape, quoted verbatim from
`src/daemon/background/run.rs:334`:

```rust
let p = pane_id.to_string();
let w = win_name.to_string();
let body = tmux::off_runtime("capture-and-archive", move || {
    capture_and_archive(&p, &w, pipe_log)
})
.await
.unwrap_or_default();
```

`spawn_blocking` requires `F: 'static`, so **every borrowed argument becomes
owned before the closure**: `&str` → `.to_string()`, `Option<&str>` →
`.map(str::to_string)` (then `.as_deref()` inside), and `SessionStore` →
`.clone()` (a `#[derive(Clone)]` newtype over `Arc<Mutex<…>>`,
`src/daemon/session.rs:122`, so the clone is an `Arc` bump).

**The helper is never edited.** The wrap at the call site is what moves its whole
body onto the blocking pool.

### This phase's 2 call sites

Line numbers are current-as-of-drafting; re-derive before editing.

| File:line | Call | Helper returns | Collapse |
|---|---|---|---|
| `executor/mod.rs:388` | `knowledge::watch_pane(…)` | `String` | `.unwrap_or_else(\|\| format!(…))` — Hazard 2 |
| `executor/mod.rs:561` | `knowledge::close_bg_window(…)` | `String` | `.unwrap_or_else(\|\| format!(…))` — Hazard 2 |

Both sit in `execute_tool_call`, which is `async fn`
(`src/daemon/executor/mod.rs:123`), and both are currently **bare match-arm
expressions** that must become **blocks**:

```rust
        } => Ok(ToolCallOutcome::Result(knowledge::watch_pane(
            pane_id,
            *timeout_secs,
            pattern.as_deref(),
            session_id,
            session_name,
            sessions,
        ))),
```

Neighbouring arms already `.await` (e.g. `knowledge::delete_script(…).await`),
so awaiting inside the arm is legal — only the arm's *expression → block* shape
changes.

### ⚠ Hazard 1 — `watch_pane` calls `tokio::spawn`, and that is fine inside `spawn_blocking`

`watch_pane` (`knowledge/pane.rs:188`) is synchronous, does two blocking tmux
calls in its prologue, then hands the actual watching to a **detached** task and
returns immediately:

```rust
// knowledge/pane.rs:235
tokio::spawn(async move {
    let _guard = WatchHookGuard { … };
    …
    let completed = tokio::time::timeout(timeout, async { … }).await;
    …
});

if let Some(pat) = pattern {
    format!("Now watching pane {} for pattern `{}`. …", pane_id, pat, timeout_secs)
```

Two consequences, both load-bearing:

1. **`tokio::spawn` works from inside `spawn_blocking`.** Blocking-pool threads
   carry the runtime context. Verified while drafting with a scratch crate: a
   sync fn calling `tokio::spawn` was run through an `off_runtime`-shaped wrapper
   on a multi-thread runtime; the wrapper returned its `String` **and** the
   detached task was scheduled and ran to completion. **Do not restructure
   `watch_pane` to avoid the spawn** — there is nothing to fix.
2. **The 5 s `TMUX_TIMEOUT` does not bound the watch.** Only the prologue runs
   inside the wrap; `timeout_secs` (which the model may set to minutes) lives in
   the detached task and is untouched. A `watch_pane(timeout_secs=300)` still
   watches for 300 s after this change.

### ⚠ Hazard 2 — `spawn_blocking` is not cancellable, so the fallback text must not claim the work did not happen

`off_runtime` wraps `tokio::time::timeout` around the **`JoinHandle`**
(`src/tmux/mod.rs:60`). A timeout drops the handle and returns `None` — it does
**not** stop the blocking closure, which runs to completion on its pool thread.

So on timeout: `watch_pane`'s hook may still get installed and its task may still
spawn (meaning a `[Watch Pane …]` message may still arrive later), and
`close_bg_window`'s window may still get killed. **Text saying "the watch was not
started" or "the window was not closed" would be wrong.** Use exactly these two
strings — they are the phase's product decision, and they are honest about the
uncertainty:

```rust
// watch_pane fallback
"Timed out after 5s starting the watch on pane {}. The tmux server may be wedged. The watch may still have started, so a [Watch Pane ...] message may still arrive."

// close_bg_window fallback
"Timed out after 5s closing the background window for pane {}. The tmux server may be wedged. The window may or may not have been closed; use list_panes to check."
```

### ⭐ The exact code — compile-checked against this tree while drafting

Both blocks below were applied to `src/daemon/executor/mod.rs`, built, linted and
`cargo fmt --all --check`-ed clean, then reverted. **They are known to compile
and to need no reformatting.** Use them.

```rust
        PendingCall::WatchPane {
            pane_id,
            timeout_secs,
            pattern,
            ..
        } => {
            let p = pane_id.clone();
            let secs = *timeout_secs;
            let pat = pattern.clone();
            let sid = session_id.map(str::to_string);
            let sname = session_name.to_string();
            let s = sessions.clone();
            let msg = crate::tmux::off_runtime("watch-pane", move || {
                knowledge::watch_pane(&p, secs, pat.as_deref(), sid.as_deref(), &sname, &s)
            })
            .await
            .unwrap_or_else(|| {
                format!(
                    "Timed out after 5s starting the watch on pane {}. The tmux server may be wedged. The watch may still have started, so a [Watch Pane ...] message may still arrive.",
                    pane_id
                )
            });
            Ok(ToolCallOutcome::Result(msg))
        }
```

```rust
        PendingCall::CloseBackgroundWindow { pane_id, .. } => {
            let p = pane_id.clone();
            let sid = session_id.map(str::to_string);
            let s = sessions.clone();
            let msg = crate::tmux::off_runtime("close-bg-window", move || {
                knowledge::close_bg_window(&p, sid.as_deref(), &s)
            })
            .await
            .unwrap_or_else(|| {
                format!(
                    "Timed out after 5s closing the background window for pane {}. The tmux server may be wedged. The window may or may not have been closed; use list_panes to check.",
                    pane_id
                )
            });
            Ok(ToolCallOutcome::Result(msg))
        }
```

`pane_id` stays usable in each `format!` because only its **clone** (`p`) was
moved into the closure. `session_id` is `Option<&str>` from `SessionCtx`
(`executor/mod.rs:20`) — own it with `.map(str::to_string)`, then `.as_deref()`
inside the closure to restore `Option<&str>`.

### 🛑 What this phase does NOT touch

`knowledge/pane.rs` is **not edited**. In particular, the `off_runtime` calls
already inside `watch_pane`'s detached task (7 in the file) stay exactly as they
are, and `close_bg_window`'s two `with_sessions` closures are untouched — the
wrap goes around the whole call, so nothing about the locking changes.

## Spec

### 1. Wrap the 2 call sites

Use the compile-checked blocks above verbatim. **Both helpers keep their current
signatures**, and no helper body is edited.

### 2. Edit no helper, no test

`watch_pane` and `close_bg_window` are edited **nowhere**.
`src/daemon/executor/knowledge/pane.rs` must show an **empty diff**.

### 3. Build after each site

Not a suggestion. `cargo build` after each wrapped site.

## Acceptance criteria

- [ ] `grep -c "off_runtime" src/daemon/executor/mod.rs` returns **≥ 2**
      (printed **0** before; 2 sites added).
- [ ] `grep -c "off_runtime" src/daemon/executor/knowledge/pane.rs` returns
      exactly **7** — unchanged.
- [ ] `grep -c "watch_pane(" src/daemon/executor/mod.rs` returns **1** and
      `grep -c "close_bg_window(" src/daemon/executor/mod.rs` returns **1** —
      each call moved inside a closure, neither duplicated.
- [ ] `grep -c "close_bg_window(" src/daemon/executor/knowledge/pane.rs` returns
      **3** — the definition and its two unit tests, all unchanged.
- [ ] **Both helpers are untouched.** Quote the result of:

```bash
git diff --stat HEAD -- src/daemon/executor/knowledge/pane.rs
grep -cF "pub fn close_bg_window(pane_id: &str, session_id: Option<&str>, sessions: &SessionStore) -> String {" src/daemon/executor/knowledge/pane.rs  # 1
grep -cF "pub fn watch_pane(" src/daemon/executor/knowledge/pane.rs  # 1
```

      Both signature greps return **1**, and `pane.rs` shows **no diff at all**.

- [ ] The two fallback strings are byte-exact:

```bash
grep -cF "Timed out after 5s starting the watch on pane" src/daemon/executor/mod.rs             # 1
grep -cF "so a [Watch Pane ...] message may still arrive." src/daemon/executor/mod.rs           # 1
grep -cF "Timed out after 5s closing the background window for pane" src/daemon/executor/mod.rs # 1
grep -cF "use list_panes to check." src/daemon/executor/mod.rs                                  # 1
```

- [ ] `grep -c "block_on\|futures::executor\|spawn_blocking" src/daemon/executor/mod.rs`
      returns **0**.
- [ ] `grep -c "tokio::spawn" src/daemon/executor/knowledge/pane.rs` returns
      **1** — `watch_pane`'s detached task, still there.
- [ ] `git diff --name-only` lists exactly **one** `src/` file:
      `src/daemon/executor/mod.rs`.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **916** lib-unit and **27** integration tests.

**Run every gate bare.**

## Test plan

Both call sites need a live tmux server and an IPC peer driving a tool call.
**Neither has unit coverage.** Pre-existing gap, neither widened nor closed here.

`close_bg_window` *does* have two direct unit tests
(`close_bg_window_no_session`, `close_bg_window_unknown_session`,
`knowledge/pane.rs:446`/`:455`) — they call the helper directly, and because
wrap-the-caller changes no signature, **they must compile and pass unchanged**.

**As in 06i and 06m, the wrap approach should change no test.** If any test needs
editing, **stop and report a blocker** — it means a signature changed, which this
phase forbids.

**Write no new tests.** Run the suite and report which commands you ran and
whether they passed. **Do not claim any test guards these two sites.**

Three reasoning checks. **Quote the code — a claim without a quotation is not an
answer:**

1. **The spawn inside the blocking pool.** Quote `watch_pane`'s `tokio::spawn`
   line as you left it, and state in one sentence why wrapping the call in
   `spawn_blocking` does not break the detached task.
2. **The watch duration.** State in one sentence what a
   `watch_pane(timeout_secs=300)` call does after your change, and why the 5 s
   `TMUX_TIMEOUT` does not shorten it.
3. **The honest fallback.** Paste one fallback `format!` and state in one
   sentence why it does not assert that the operation failed to happen.

## End-to-end verification

None required. 06a demonstrated the timeout arm fires; this phase adds no
machinery. **Do not repeat that demonstration.**

## Authorizations

- [x] May edit `src/daemon/executor/mod.rs` — **the two named call sites only.**
- [x] May add owned bindings and `.clone()` calls at those call sites.
- [x] May convert the two match arms from bare expressions to blocks.
- [ ] **No** edit to `src/daemon/executor/knowledge/pane.rs` — not one line.
- [ ] **No** signature change to any function, anywhere.
- [ ] **No** change to any test.
- [ ] **No** `block_on`, `futures::executor`, or new `tokio::spawn`.
- [ ] **No** edits to `src/tmux/` or any file outside the one named.
- [ ] **No** new tests, no new dependency, no `#[allow(...)]`.

## Out of scope

- **`notify_session`, `handlers.rs:186`, `webhook/process.rs:190`,
  `cleanup_bg_windows` ×2** — all need a shape change first. 06n.
- **`src/cli/`** — no concurrency to protect; bounded by stage A instead.
- **`src/tmux/{pane,session,window,cache}.rs`** — the sync helper bodies. Stage A
  hardens them with their own timeouts. **Do not touch them.**
- **Changing `TMUX_TIMEOUT`, or giving `watch_pane` a longer wrap timeout.** The
  5 s ceiling bounds only the prologue, which is correct. Do not add a second
  timeout constant.

### ⚠ Traps

1. **Do not edit `pane.rs`.** Its diff must be completely empty.
2. **`watch_pane`'s `tokio::spawn` is not a problem** — verified to work from a
   blocking-pool thread. Do not "fix" it.
3. **The 5 s timeout does not shorten a watch.** Do not compensate for it.
4. **The fallback strings are byte-exact** and must not claim the operation did
   not happen — `spawn_blocking` is not cancellable.
5. **`session_id` is `Option<&str>`** — `.map(str::to_string)` before the
   closure, `.as_deref()` inside.
6. **Only the clone moves.** `pane_id` must stay usable in the `format!`.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-28 05:53 (started)

**Executor:** claude (Sonnet 4.5)

Wrapped `knowledge::watch_pane` and `knowledge::close_bg_window` call sites in `executor/mod.rs` with `off_runtime` closures, following the compile-checked blocks from the spec verbatim. Both helpers remain untouched.

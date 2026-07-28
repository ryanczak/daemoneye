# Phase 06q: Unlock `handle_list_panes` — a tmux Sweep Under the Cache Read Guard

**Milestone:** M5 — UX & Stability
**Status:** in-progress
**Depends on:** phase-06n — `done`
**Estimated diff:** ~35 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

`handle_list_panes` runs `tmux::pane_exists` — **one subprocess per pane** —
inside a `.filter()` closure, **while holding the `cache.panes` read guard**.
Split it into the collect-under-lock / act-outside shape and take the probe off
the runtime.

This is **two defects in one site**:

1. **Mechanism A** — blocking work (N tmux subprocesses) inside a critical
   section. Every other reader of `cache.panes` waits for the whole sweep.
2. **Mechanism B** — those subprocesses run on a tokio worker, and they cannot
   be `.await`ed where they sit because `.filter()`'s closure is synchronous.

**Finish condition: `pane_exists` is called from an `off_runtime` closure in an
unlocked phase, and the `cache.panes` guard is dropped before it runs.**

## Architecture references

- `docs/design/daemon-stalls.md` § 1 — mechanism A (lock held across blocking
  work) and mechanism B (blocking subprocess on tokio workers).
- `src/tmux/mod.rs:29` — the `off_runtime` adapter and `TMUX_TIMEOUT`.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Verify the starting state:

```bash
grep -c "off_runtime"        src/daemon/server/handlers.rs   # expect 0
grep -c "pane_exists"        src/daemon/server/handlers.rs   # expect 1
grep -cF "cache.panes.read()" src/daemon/server/handlers.rs  # expect 2
grep -c "\.filter("          src/daemon/server/handlers.rs   # expect 4
grep -c "sort_by_key"        src/daemon/server/handlers.rs   # expect 1
cargo test 2>&1 | grep "^test result" | head -3   # expect 916 lib, 0, 27 integration
```

**Every number above was produced by running that exact command against the tree
while drafting.** If one differs, **stop and report a blocker**.

⚠ **Two of those counts are not about this site.** `cache.panes.read()` is **2**
because `handle_set_pane` (`:136`) has its own, and `.filter(` is **4** because
`handle_status` (`:246`) has one. **Neither is in scope.** Their presence is why
the acceptance criteria below are not all zeros.

## Current state

### ⚠ This is a different lock from the rest of the milestone

Every lock phase in M5 so far has been about `SessionStore`. This one is
`cache.panes`, a `std::sync::RwLock` on `SessionCache` — so `with_sessions`, the
re-entrancy assertion, and the `SessionsLockDepth` counter **do not apply here**.
The fix is the same *shape* (collect under the lock, act outside), but there is
no accessor to route it through. Do not try to use `with_sessions`.

### The site — `src/daemon/server/handlers.rs:174`

`handle_list_panes` is `async fn` (`:158`), so `.await` is legal in its body —
just not inside the `.filter()` closure, which is where the call sits today:

```rust
    let panes_snapshot = {
        let panes = cache.panes.read().unwrap_or_log();
        let mut entries: Vec<_> = panes
            .iter()
            .filter(|(id, _)| chat_pane_id.as_deref() != Some(id.as_str()))
            .filter(|(_, s)| {
                !s.window_name.starts_with("de-bg-")
                    && !s.window_name.starts_with("de-sj-")
                    && !s.window_name.starts_with("de-gs-bg-")
                    && !s.window_name.starts_with("de-gs-sj-")
                    && !s.window_name.starts_with("de-gs-ir-")
            })
            .filter(|(id, _)| crate::tmux::pane_exists(id))
            .map(|(id, s)| {
                let is_target = current_target.as_deref() == Some(id.as_str());
                (
                    id.clone(),
                    s.current_cmd.clone(),
                    s.window_name.clone(),
                    s.pane_index,
                    is_target,
                )
            })
            .collect();
        entries.sort_by_key(|(_, _, win, idx, _)| (win.clone(), *idx));
        entries
    };
```

The worked example for the fix is `cleanup_pass` (`src/daemon/session.rs:471`) —
the shape adopted across phase 05 after a confirmed production hang: take what
you need under the lock, release, then do the blocking work.

### ⭐ The exact code — compile-, clippy-, fmt- and test-checked against this tree

Applied, verified, and reverted while drafting. **`cargo fmt` rewrote the
`off_runtime` line into the one-line closure form shown below** — this project
has no `format_fix` hook, so use exactly this:

```rust
    // Phase 1 (locked): snapshot the candidates. No blocking work under the
    // read guard — `pane_exists` spawns a tmux subprocess per pane and used to
    // run here, holding the cache lock for the whole sweep.
    let candidates: Vec<(String, String, String, usize, bool)> = {
        let panes = cache.panes.read().unwrap_or_log();
        panes
            .iter()
            .filter(|(id, _)| chat_pane_id.as_deref() != Some(id.as_str()))
            .filter(|(_, s)| {
                !s.window_name.starts_with("de-bg-")
                    && !s.window_name.starts_with("de-sj-")
                    && !s.window_name.starts_with("de-gs-bg-")
                    && !s.window_name.starts_with("de-gs-sj-")
                    && !s.window_name.starts_with("de-gs-ir-")
            })
            .map(|(id, s)| {
                let is_target = current_target.as_deref() == Some(id.as_str());
                (
                    id.clone(),
                    s.current_cmd.clone(),
                    s.window_name.clone(),
                    s.pane_index,
                    is_target,
                )
            })
            .collect()
    };

    // Phase 2 (unlocked): the liveness probe, one bounded tmux call per
    // candidate, off the runtime.
    let mut panes_snapshot = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let id = candidate.0.clone();
        let exists = crate::tmux::off_runtime("pane-exists", move || crate::tmux::pane_exists(&id))
            .await
            .unwrap_or(false);
        if exists {
            panes_snapshot.push(candidate);
        }
    }
    panes_snapshot.sort_by_key(|(_, _, win, idx, _)| (win.clone(), *idx));
```

The block below it is unchanged and still sends `Response::PaneList { panes:
panes_snapshot }`.

### Three semantics this preserves — each looks like a change and is not

1. **`.map()` now runs before the liveness filter.** Previously `pane_exists`
   filtered *before* `.map()`. The `.map()` closure is **pure** — it clones five
   fields and compares `current_target` — so moving it earlier changes nothing
   about the result set, only how many tuples are built (at most one per
   candidate, all discarded if the pane is gone). **The final `Vec` is
   identical.**
2. **A timeout drops the pane from the list, and that is behaviour-preserving.**
   `pane_exists` (`src/tmux/pane.rs:436`) is:

   ```rust
   pub fn pane_exists(pane_id: &str) -> bool {
       Command::new("tmux")
           .args(["display-message", "-t", pane_id, "-p", "#{pane_id}"])
           .output()
           .map(|o| o.status.success())
           .unwrap_or(false)
   }
   ```

   It **already returns `false`** when the tmux call fails. So `.unwrap_or(false)`
   on timeout matches what the existing code does on error. **Do not invert it**
   — `.unwrap_or(true)` would list panes that may not exist.
3. **The sort must stay after the filter**, on the filtered vector. It is a
   stable ordering by `(window_name, pane_index)` and the CLI renders the list in
   that order.

### 🛑 The other `cache.panes.read()` and `.filter(` are NOT in scope

`handle_set_pane:136` has its own read guard and `handle_status:246` its own
`.filter(|s| !s.is_ghost)`. **Do not touch either.** The criteria below pin
`cache.panes.read()` at **2** (unchanged) and `.filter(` at **3** (4 minus the
one removed here) precisely so an over-eager sweep into them fails the phase.

## Spec

1. **Split the block in two** — in `src/daemon/server/handlers.rs`, replace the
   `panes_snapshot` block with the two phases above. The `cache.panes` read guard
   must be dropped when phase 1's block ends.
2. **Remove the synchronous `pane_exists` filter** from the iterator chain; the
   probe now happens in the phase-2 loop.
3. **Change nothing else in the file.**

### Build after the edit

`cargo build`, then `cargo fmt --all`, then `cargo clippy`. **Run `cargo fmt
--all` before you finish** — this project auto-formats nothing for you.

## Acceptance criteria

- [ ] `grep -c "off_runtime" src/daemon/server/handlers.rs` returns **1**
      (printed **0** before).
- [ ] `grep -cF ".filter(|(id, _)| crate::tmux::pane_exists(id))" src/daemon/server/handlers.rs`
      returns **0** — the synchronous filter is gone.
- [ ] `grep -cF "crate::tmux::pane_exists(&id)" src/daemon/server/handlers.rs`
      returns **1** — the call now takes a reference to an owned `id` inside the
      closure.
- [ ] `grep -c "\.filter(" src/daemon/server/handlers.rs` returns **3**
      (printed **4** before; one removed). **Not 2** — `handle_status:246` has
      one that is out of scope.
- [ ] `grep -cF "cache.panes.read()" src/daemon/server/handlers.rs` returns
      **2** — **unchanged**. `handle_set_pane` keeps its own guard.
- [ ] `grep -c "sort_by_key" src/daemon/server/handlers.rs` returns **1** — the
      sort was moved, not duplicated or dropped.
- [ ] `grep -cF "unwrap_or(false)" src/daemon/server/handlers.rs` returns **1**
      and `grep -cF "unwrap_or(true)" src/daemon/server/handlers.rs` returns
      **0** — the timeout arm is not inverted.
- [ ] `grep -c "block_on\|futures::executor\|spawn_blocking" src/daemon/server/handlers.rs`
      returns **0**.
- [ ] `git diff --name-only` lists exactly **one** `src/` file:
      `src/daemon/server/handlers.rs`.
- [ ] **Read and confirm**, quoting the code: the `let candidates … };` block
      closes **before** the `for candidate in candidates` loop begins, so the
      read guard is released before any `off_runtime` call.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **916** lib-unit and **27** integration tests.

**Run every gate bare.**

## Test plan

`handle_list_panes` needs a live tmux server, a populated `SessionCache` and an
IPC peer. **It has no unit coverage**, and `handlers.rs` has no test module.
Pre-existing gap, neither widened nor closed here.

**The whole change compiled and the full suite passed with no test edited** in
the checked run — so if any test needs editing, **stop and report a blocker**.

**Write no new tests.** Run the suite and report which commands you ran and
whether they passed. **Do not claim any test guards this handler.**

Three reasoning checks. **Quote the code — a claim without a quotation is not an
answer:**

1. **The guard.** Quote the line where the `candidates` block closes and the line
   where the loop opens, and state in one sentence why the read guard is no
   longer held during the tmux probes.
2. **The reorder.** State in one sentence why moving `.map()` ahead of the
   liveness check cannot change which panes end up in `panes_snapshot`.
3. **The timeout arm.** Quote your `.unwrap_or(false)` line and state in one
   sentence why `false` — not `true` — is the behaviour-preserving default.

## End-to-end verification

None required. 06a demonstrated the timeout arm fires; this phase adds no
machinery. **Do not repeat that demonstration.**

## Authorizations

- [x] May edit `src/daemon/server/handlers.rs` — **the `handle_list_panes`
      `panes_snapshot` block only.**
- [x] May add owned bindings and `.clone()` calls in that block.
- [x] May add the two phase comments shown above.
- [ ] **No** edit to `handle_set_pane`, `handle_status`, or any other handler.
- [ ] **No** change to `Response::PaneList`'s tuple shape
      `(String, String, String, usize, bool)`.
- [ ] **No** change to `src/tmux/pane.rs::pane_exists`.
- [ ] **No** signature change to `handle_list_panes`.
- [ ] **No** use of `with_sessions` — this is the `cache.panes` RwLock, not
      `SessionStore`.
- [ ] **No** `block_on`, `futures::executor`, or detached `tokio::spawn`.
- [ ] **No** edits to `src/tmux/` or any file outside the one named.
- [ ] **No** new tests, no new dependency, no `#[allow(...)]`.

## Out of scope

- **`webhook/process.rs:190` / `inject_ghost_event`** — a 13-site async cascade.
  **06r.**
- **Any other `cache.panes` reader** — `handle_set_pane:136` and the cache poller
  itself. Only `handle_list_panes` does blocking work under the guard.
- **`src/cli/`** — no concurrency to protect; bounded by stage A instead.
- **`src/tmux/{pane,session,window,cache}.rs`** — the sync helper bodies. Stage A
  hardens them with their own timeouts. **Do not touch them.**

### ⚠ Traps

1. **`.unwrap_or(false)`, never `true`** — `pane_exists` already returns `false`
   on tmux failure.
2. **The other `.filter(` and `cache.panes.read()` stay.** The criteria are **3**
   and **2**, not 2 and 1.
3. **Do not reach for `with_sessions`** — wrong lock.
4. **`cargo fmt --all` collapses the `off_runtime` closure to one line.** Use the
   post-`fmt` form given above, and run `fmt` before finishing.
5. **The sort stays**, after the filter, exactly once.
6. **The guard must be released before the loop** — that is the entire point of
   the phase.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-28 17:58 (started)

**Executor:** Claude (Anthropic)

Split `handle_list_panes` into collect-under-lock / act-outside shape. Removed the synchronous `pane_exists` from the `.filter()` closure and replaced it with an `off_runtime` loop in an unlocked phase.

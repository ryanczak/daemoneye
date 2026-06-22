# Phase 07a: Pane-Targeting & Cleanup Safety

**Milestone:** M1 — Agent Tooling Improvements
**Status:** done
**Depends on:** none (independent of phases 01–06; touches the foreground execution path they shaped)
**Estimated diff:** ~230 lines (incl. tests)
**Tags:** language=rust, kind=bugfix, size=m

> **Scope note (architect, 2026-06-22).** The original phase-07
> ("execution-robustness-and-tmux") bundled six findings plus open-ended tmux
> leverage. That is too large and too risky for one local-executor session, and it
> mixes *safe mechanical fixes* with *delicate completion-detection changes*. It was
> split into **07a** (this doc — the four `medium` mechanical/safety fixes) and
> **07b** (the two `high` completion/exit-code items + tmux leverage, drafted on
> demand). This phase does **not** touch local completion detection or exit-code
> reporting — those are 07b. See README § "Confirmed findings inventory → Phase 07".

## Goal

Close four `medium`-severity correctness/safety gaps on the edges of the
command-execution path, each small and independent:

1. **Never offer or default to the chat pane as a command target.** The manual
   pane-selection list excludes the chat pane so a command can't be run in the
   conversation pane.
2. **The stale-pane (C3b) guard queries tmux live**, not the up-to-2 s-stale cache,
   so a just-closed pane is caught and a just-created pane is not falsely rejected.
3. **A timed-out sudo password prompt is cancelled** (`C-c`) so the user's pane
   returns to a clean shell instead of sitting at a dangling prompt.
4. **`watch_pane`'s tmux hook is uninstalled on every exit path** (normal, timeout,
   panic, task abort) via a `Drop` guard, mirroring the existing `FgHookGuard`.

## Architecture references

Read before starting:

- `docs/architecture.md#24-remote-host-execution-model` — the foreground/remote
  execution model these guards protect.
- `docs/architecture.md#21-interactive-requestresponse` — the approval → target-pane
  → send-keys → completion flow; this phase hardens the target-pane and cleanup steps.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom (note §2.2 "no premature abstraction"
   and §3 test rules: hermetic, deterministic, no real network/home writes).
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. **Re-verify the cited line numbers** in `src/daemon/executor/mod.rs`,
   `src/daemon/executor/foreground.rs`, `src/daemon/executor/knowledge.rs`, and
   `src/tmux/pane.rs` before editing — the numbers below were captured at draft time
   and the tree moves. Match on the quoted code, not the line number.
5. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

### Task 1 — chat-pane not excluded from the selection list

`find_best_target_pane` (`src/daemon/executor/mod.rs:856-868`) builds the list of
panes offered to the user (via `Response::PaneSelectPrompt`) from the cache **without
filtering the chat pane**:

```rust
let pane_list: Vec<PaneInfo> = {
    let panes = cache.panes.read().unwrap_or_log();
    let mut v: Vec<PaneInfo> = panes
        .iter()
        .map(|(pid, state)| PaneInfo {
            id: pid.clone(),
            current_cmd: state.current_cmd.clone(),
            summary: state.summary.clone(),
        })
        .collect();
    v.sort_by(|a, b| a.id.cmp(&b.id));
    v
};
```

The AI-specified and default-pane paths above this already exclude the chat pane
(`chat_pane == Some(tp)` / `chat_pane != Some(dtp.as_str())`, lines 828 & 848). Only
this fallback list does not. `PaneInfo` (`src/ipc.rs:6`) is `{ id, current_cmd,
summary }` — `Debug + Serialize + Deserialize + Clone`, **no `PartialEq`** (assert on
`.id` in tests, not `assert_eq!` on whole `PaneInfo` values).

### Task 2 — C3b stale-pane guard reads the cache

`run_foreground` (`src/daemon/executor/foreground.rs:193-221`) checks pane existence
against the cache, which the 2 s poller can leave up to 2 s stale:

```rust
    if let Some(tp) = target
        && chat_pane != Some(tp)
    {
        let pane_exists = {
            let panes = cache.panes.read().unwrap_or_log();
            panes.contains_key(tp)
        };
        if !pane_exists {
            // ... builds an error with the current pane map ...
        }
    }
```

A live query helper already exists: `crate::tmux::pane_exists(pane_id) -> bool`
(`src/tmux/pane.rs`, used at `foreground.rs:841` in the N11 retry path).

### Task 3 — timed-out sudo prompt left dangling

In the local sudo password flow (`foreground.rs`, inside `SudoAuth::Password` →
`else` (local) branch, ~lines 505-560), a credential-prompt **timeout** sets
`SudoFail::Cancelled`:

```rust
                                let Some(cred) = cred else {
                                    sudo_fail = Some(SudoFail::Cancelled);
                                    break 'sudo;
                                };
```

When this fires, `sudo` is still running in the target pane waiting for a password.
The structured-error return (`if let Some(fail) = sudo_fail { … }`, ~lines 563-595)
reports the failure but never clears the pane. The `AuthExhausted` case does **not**
need this — `sudo` exits on its own after `MAX_SUDO_RETRIES` wrong attempts.

There is **no** raw control-key helper in `src/tmux/` today: `send_keys`
(`pane.rs:388`) appends `C-m` (Enter), which is wrong for an interrupt.

### Task 4 — `watch_pane` hook leaks on abnormal exit

`watch_pane` (`src/daemon/executor/knowledge.rs:776-954`) installs a
`pane-title-changed[@de_wp_N]` hook (line ~796), then in the spawned task uninstalls
it **only after** the `timeout(...).await` completes (lines 876-878):

```rust
    tokio::spawn(async move {
        // ... timeout(timeout, async { ... }).await ...
        let _ = std::process::Command::new("tmux")
            .args(["set-hook", "-u", "-t", &pane_id_owned, &hook_name])
            .output();
        // ... capture + persist ...
    });
```

If the task is aborted (e.g. session teardown drops the `JoinHandle`) or the inner
block panics, the uninstall is skipped → a zombie hook fires `daemoneye notify
activity` on every title change forever. The fix pattern already exists for the
foreground path: `FgHookGuard` (`foreground.rs:50-84`) uninstalls on `Drop`.

`FgHookGuard` is **private to the `foreground` module** and carries
foreground-specific `monitor_silence` state — do **not** widen its visibility or
generalize it (only two callers; STANDARDS §2.2 says abstract on the *fourth*). Add a
small dedicated guard local to `knowledge.rs` for `watch_pane`'s single hook.

## Spec

Numbered tasks in execution order. Each names the exact file and change. **Build
after Task 3** (it adds a new `tmux` function used by `foreground.rs`).

### 1. Exclude the chat pane from the selection fallback — `src/daemon/executor/mod.rs`

Add a pure helper just above `find_best_target_pane` and use it for the fallback
list. Keep the cache read at the call site; the helper does the filter + sort so it is
unit-testable without constructing `PaneState`:

```rust
/// Build the user-facing pane-selection list, excluding the chat pane (a command
/// must never be offered to run in the conversation pane). Sorted by pane id for a
/// stable display order.
fn exclude_chat_pane(mut panes: Vec<PaneInfo>, chat_pane: Option<&str>) -> Vec<PaneInfo> {
    panes.retain(|p| chat_pane != Some(p.id.as_str()));
    panes.sort_by(|a, b| a.id.cmp(&b.id));
    panes
}
```

Then in `find_best_target_pane`, build the raw `Vec<PaneInfo>` from the cache (no
sort there anymore — the helper sorts) and pass it through:

```rust
    let pane_list: Vec<PaneInfo> = {
        let panes = cache.panes.read().unwrap_or_log();
        let raw: Vec<PaneInfo> = panes
            .iter()
            .map(|(pid, state)| PaneInfo {
                id: pid.clone(),
                current_cmd: state.current_cmd.clone(),
                summary: state.summary.clone(),
            })
            .collect();
        exclude_chat_pane(raw, chat_pane)
    };
```

The existing `if pane_list.is_empty() { … }` guard below now also covers the case
where the chat pane was the *only* pane (correct: there is no valid target).

### 2. C3b stale-pane guard queries tmux live — `src/daemon/executor/foreground.rs`

In the C3b block (~lines 196-199), replace the cache lookup with the live query:

```rust
        let pane_exists = crate::tmux::pane_exists(tp);
```

Remove the now-unused inner `{ let panes = cache.panes.read()…; panes.contains_key(tp) }`
block. Leave the rest of the C3b error-construction (which reads
`entry.default_target_pane` and `cache.pane_map_summary`) unchanged. Do **not** touch
the C3a format guard above it, the `target_hint` closure below it (lines ~225-246), or
`find_best_target_pane`'s own cache reads — those are deliberately out of scope (see
Out of scope).

### 3. Add `tmux::send_cancel` and call it on sudo-cancel — two files

**3a.** In `src/tmux/pane.rs`, add a helper next to `send_keys` (it is re-exported
via `pub use pane::*;` in `src/tmux/mod.rs`, so it becomes `crate::tmux::send_cancel`):

```rust
/// Send a `C-c` (SIGINT) to a pane without a trailing Enter, to cancel a process
/// waiting at a prompt (e.g. a sudo password prompt the user let time out).
pub fn send_cancel(pane_id: &str) -> Result<()> {
    let output = Command::new("tmux")
        .args(["send-keys", "-t", pane_id, "C-c"])
        .output()?;
    if !output.status.success() {
        anyhow::bail!("Failed to send C-c to pane '{}'", pane_id);
    }
    Ok(())
}
```

(Note: no `C-m` argument — that distinction from `send_keys` is the whole point.)

**3b.** In `src/daemon/executor/foreground.rs`, in the structured sudo-failure return
block (`if let Some(fail) = sudo_fail { … }`), send `C-c` to the target pane **only**
for the `Cancelled` variant, before the existing cleanup/return. Place it so it runs
before `drop(fg_hook_guard)` / `unhighlight_pane`:

```rust
            if let Some(fail) = sudo_fail {
                if matches!(fail, SudoFail::Cancelled) {
                    // sudo is still sitting at the password prompt — clear it so the
                    // pane returns to a usable shell rather than a dangling prompt.
                    let _ = tmux::send_cancel(target_str);
                }
                let msg = match fail { /* unchanged */ };
                // ... unchanged drop/unhighlight/finish_command/return ...
            }
```

Do not change the `msg` text, the `AuthExhausted` arm, or the remote-pane branch.

### 4. `Drop`-guard `watch_pane`'s hook — `src/daemon/executor/knowledge.rs`

Add a small guard struct (local to this file, above `watch_pane`) that uninstalls one
hook on drop:

```rust
/// Uninstalls a tmux hook on drop so an aborted or panicking `watch_pane` task
/// never leaves a stale `pane-title-changed` hook firing forever. Mirrors
/// `FgHookGuard` in `foreground.rs` (kept separate — see STANDARDS §2.2).
struct WatchHookGuard {
    pane_id: String,
    hook_name: String,
}

impl Drop for WatchHookGuard {
    fn drop(&mut self) {
        let _ = std::process::Command::new("tmux")
            .args(["set-hook", "-u", "-t", &self.pane_id, &self.hook_name])
            .output();
    }
}
```

Then:
- After the hook is installed (the `set-hook -t pane_id hook_name notify_cmd` call,
  ~line 796), the synchronous body still owns `pane_id`/`hook_name` as `&str`/`String`.
  Construct the guard from the **owned** strings that are already moved into the task
  (`pane_id_owned`, `hook_name`). Move the guard **into** the `tokio::spawn` async
  block so its `Drop` runs whenever the task ends — normal completion, timeout, panic,
  or abort.
- **Delete** the manual uninstall at lines 876-878 (the guard now owns it). Capturing
  output for the final `ToolResult`/persistence (lines 880+) is unaffected.
- `hook_name` is currently used by the manual uninstall and the `notify_cmd`
  install; ensure it is moved into the task for the guard (it is a `String`; clone if
  the borrow checker requires it for the install call that precedes the spawn).

Pin the behavior, not the exact field set: the guard must uninstall the *same*
`@de_wp_N` hook on *any* task exit.

## Acceptance criteria

- [ ] `exclude_chat_pane` excludes the chat pane and returns the rest sorted by id;
      returns all panes when `chat_pane` is `None`.
- [ ] `find_best_target_pane`'s fallback list is built via `exclude_chat_pane` — the
      chat pane can no longer appear in a `PaneSelectPrompt`.
- [ ] The C3b guard in `run_foreground` calls `crate::tmux::pane_exists(tp)`; no
      `cache.panes.read()` remains inside the C3b existence check.
- [ ] `tmux::send_cancel(pane_id)` issues `tmux send-keys -t <pane> C-c` with **no**
      trailing `C-m`, and is callable as `crate::tmux::send_cancel`.
- [ ] The local sudo `Cancelled` path calls `tmux::send_cancel(target_str)` before
      returning; the `AuthExhausted` and remote paths do not.
- [ ] `watch_pane` uninstalls its hook via a `Drop` guard moved into the spawned task;
      the manual `set-hook -u` at the old lines 876-878 is removed.
- [ ] `cargo fmt --all`, `cargo build` (zero new warnings),
      `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test`
      all pass.

## Test plan

Concrete tests. Most of this phase is tmux-side-effect plumbing (STANDARDS §3.2
exempts pure plumbing; precedent: `FgHookGuard` ships with no unit test because its
behavior is a tmux side effect). The one genuinely pure unit is `exclude_chat_pane`:

- `excludes_chat_pane_from_selection` in `src/daemon/executor/mod.rs` (extend the
  existing `#[cfg(test)] mod tests`) — given three `PaneInfo` with ids `%1`,`%2`,`%3`
  and `chat_pane = Some("%2")`, the result's ids are exactly `["%1","%3"]` (assert on
  `.iter().map(|p| p.id.as_str())`; `PaneInfo` has no `PartialEq`). Assert `%2` is
  absent.
- `keeps_all_panes_when_no_chat_pane` in the same module — `chat_pane = None` returns
  all three ids, sorted.
- `exclude_chat_pane_sorts_by_id` — pass ids out of order (`%3`,`%1`,`%2`) and assert
  the result is `["%1","%2","%3"]` (locks the sort the call site relies on).

For Tasks 2–4, do **not** invent hermetic tests that would require shelling out to a
real tmux server (non-deterministic, violates §3.2/§3.3). Verify them by inspection +
the End-to-end section. If a task suggests a pure unit you can extract cleanly without
new abstraction, you may add it, but do not refactor side-effecting code solely to
make it testable.

## End-to-end verification

This phase ships runtime behavior changes that are tmux side effects, not a
checked-in artifact or CLI output. The build/clippy/test gates plus the
`exclude_chat_pane` unit tests are the automatable verification. State in the
completion log:

> Tasks 2–4 are tmux-side-effect changes with no runtime-loadable artifact and no
> hermetic seam; verified by inspection. Task 1's pure helper is verified by the three
> unit tests above (quote their passing output).

Quote the passing output of `cargo test exclude_chat_pane`,
`cargo test excludes_chat_pane_from_selection`, and
`cargo test keeps_all_panes_when_no_chat_pane` in the completion Update Log.

If a live tmux session is available, the following manual checks confirm Tasks 2–4
(optional, record results if run): a `target_pane` for a pane closed < 2 s earlier
returns the "no longer exists" error immediately (Task 2); letting a sudo password
prompt time out returns the pane to a shell prompt (Task 3); a `watch_pane` whose
session is torn down leaves no `@de_wp_*` hook (`tmux show-hooks -t <pane>`) (Task 4).

## Authorizations

- [ ] May add dependencies: **no.**
- [ ] May touch `docs/architecture.md`: **no.**

Adding `pub fn send_cancel` to `src/tmux/pane.rs` is in scope (a new function in an
existing module, not a new dependency or file).

## Out of scope

- **Local completion-detection robustness** (`saw_child`/PID-return loop, start
  window) and **exit-code surfacing** (`read_pane_exit_status().unwrap_or(0)`,
  reporting non-zero exits to the AI). These are the two `high` items → **phase 07b**.
- **tmux-verb leverage** (`wait-for`, `set-buffer`/`paste-buffer`, `copy-mode -X`,
  `if-shell`) → **phase 07b**.
- **The `target_hint` closure** (`foreground.rs:225-246`) and **`find_best_target_pane`'s
  AI-target / default-pane cache reads** (lines 827-854). Those legitimately use the
  session-scoped cache; only the C3b *existence* check (Task 2) and the *fallback list*
  (Task 1) change. Do not swap them to live queries.
- **Generalizing `FgHookGuard`** into a shared guard. Two callers is not the fourth;
  keep `WatchHookGuard` separate (STANDARDS §2.2).
- **The remote-pane sudo branch** and the `AuthExhausted` arm — neither leaves an
  injected prompt dangling (see Current state, Task 3).
- **Changing any error-message text or IPC type.**

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-06-22 19:30 (started)

**Executor:** rexyMCP executor session

Implementing all four tasks: chat-pane exclusion helper, live C3b stale-pane guard, `send_cancel` for sudo timeout cleanup, and `WatchHookGuard` for `watch_pane` hook cleanup.

### Update — 2026-06-22 19:30 (complete)

**Tasks completed:** All 4 tasks implemented.

**Verification:**
- `cargo fmt --all` — passed (no changes needed)
- `cargo build` — passed, zero new warnings
- `cargo clippy --all-targets --all-features -- -D warnings` — passed
- `cargo test` — 27 passed, 0 failed, 1 ignored

**End-to-end verification:**
Tasks 2–4 are tmux-side-effect changes with no runtime-loadable artifact and no hermetic seam; verified by inspection. Task 1's pure helper is verified by the three unit tests above.

`cargo test exclude_chat_pane` output:
```
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 742 filtered out
```

`cargo test excludes_chat_pane_from_selection` output:
```
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 742 filtered out
```

`cargo test keeps_all_panes_when_no_chat_pane` output:
```
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 742 filtered out
```

**Grep verification for pinned literals:**
```
$ grep -rn 'exclude_chat_pane' src/daemon/executor/mod.rs | head -5
817:fn exclude_chat_pane(mut panes: Vec<PaneInfo>, chat_pane: Option<&str>) -> Vec<PaneInfo> {
875:        exclude_chat_pane(raw, chat_pane)
948:        let result = exclude_chat_pane(panes, Some("%2"));
972:        let result = exclude_chat_pane(panes, None);
978:    fn exclude_chat_pane_sorts_by_id() {
```
```
$ grep -rn 'send_cancel' src/tmux/pane.rs
403:pub fn send_cancel(pane_id: &str) -> Result<()> {
```
```
$ grep -rn 'WatchHookGuard' src/daemon/executor/knowledge.rs
780:struct WatchHookGuard {
786:impl Drop for WatchHookGuard {
841:        let _guard = WatchHookGuard {
```

**Files changed:**
- `src/daemon/executor/mod.rs` — added `exclude_chat_pane` helper, updated `find_best_target_pane` fallback list, added 3 unit tests
- `src/daemon/executor/foreground.rs` — replaced cache lookup with `crate::tmux::pane_exists(tp)` in C3b guard, added `send_cancel` call on sudo `Cancelled`
- `src/tmux/pane.rs` — added `pub fn send_cancel`
- `src/daemon/executor/knowledge.rs` — added `WatchHookGuard` struct with `Drop`, moved guard into spawned task, removed manual uninstall

**Notes for review:**
- `PaneInfo` uses `String` for `current_cmd` and `summary` (not `Option<String>`), so test fixtures use `String::new()` — verified against `src/ipc.rs`.
- `hook_name` required a `.clone()` for the `set-hook` install call before the spawn block consumes the original for the guard.

### Review verdict — 2026-06-22

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** rexyMCP executor (Qwen/Qwen3.6-27B-FP8)
- **Scope deviations:** none — all four tasks implemented exactly as specced; out-of-scope items (local completion detection, exit-code surfacing, `target_hint`/AI-target cache reads, `FgHookGuard` generalization, remote sudo branch) left untouched.
- **Calibration:** none

Reviewer re-ran `cargo fmt --all --check`, `cargo build` (zero warnings), `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test` (743 lib + 27 integration, 0 failed) — all green. Verified by inspection: `exclude_chat_pane` helper + 3 real unit tests (mod.rs); C3b guard now calls `crate::tmux::pane_exists(tp)` (foreground.rs); `send_cancel` issues `C-c` with no `C-m` and fires only on `SudoFail::Cancelled` (pane.rs, foreground.rs); `WatchHookGuard` Drop moved into the spawned task with the manual `set-hook -u` removed (knowledge.rs). No new unwrap/expect/panic/unsafe/allow/TODO in production paths.

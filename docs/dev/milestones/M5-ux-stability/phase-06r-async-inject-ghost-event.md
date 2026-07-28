# Phase 06r: Make `inject_ghost_event` Async — the Last Mechanism-B Site

**Milestone:** M5 — UX & Stability
**Status:** in-progress
**Depends on:** phase-06q — `done`
**Estimated diff:** ~60 lines
**Tags:** language=rust, kind=refactor, size=m

## Goal

`inject_ghost_event` calls `notify_chat_panes` — **one `tmux display-message`
subprocess per active chat pane** — but the function is **synchronous**, so the
wrap cannot happen where the call sits. Make it `async`, `.await` it at its **12
call sites**, and wrap the `notify_chat_panes` call inside it.

This is the **last mechanism-B site in the daemon**.

**Finish condition: `inject_ghost_event` is `async`, all 12 call sites `.await`
it, `notify_chat_panes` is wrapped in `off_runtime`, and `cargo clippy
--all-targets --all-features -- -D warnings` passes.**

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
grep -c "off_runtime" src/webhook/process.rs                        # expect 2
grep -c "notify_chat_panes(" src/webhook/process.rs                 # expect 4
grep -cF "notify_chat_panes(sessions, one_liner);" src/webhook/process.rs  # expect 1
grep -cF "pub(crate) fn inject_ghost_event(sessions: &SessionStore, content: &str) {" src/webhook/process.rs  # expect 1
grep -c "inject_ghost_event(" src/webhook/process.rs                # expect 5
grep -c "inject_ghost_event(" src/daemon/scheduled.rs               # expect 5
grep -c "inject_ghost_event(" src/daemon/stream.rs                  # expect 2
grep -c "inject_ghost_event(" src/daemon/executor/knowledge/ghost.rs # expect 1
cargo test 2>&1 | grep "^test result" | head -3   # expect 916 lib, 0, 27 integration
```

**Every number above was produced by running that exact command against the tree
while drafting.** If one differs, **stop and report a blocker**.

`process.rs`'s **5** is the definition plus 4 call sites. Total call sites across
the four files: **4 + 5 + 2 + 1 = 12**.

## Current state

### ⭐ This cascade cannot strike out — the build stays green the whole way

`WORKFLOW.md` § "Prefer additive change shapes" warns that a multi-site mutation
can leave the tree non-compiling for many turns and trip the verifier's
consecutive-failure limit. **That does not happen here, and it was verified:**

Adding `async` to the definition and building produced **0 errors and 12
warnings**. Calling an `async fn` without `.await` is an *unused-`Future`*
warning, not a type error, so `cargo build` keeps succeeding at every
intermediate step. Only `clippy -D warnings` fails until all 12 are awaited.

**So the compiler hands you the checklist.** After step 1, `cargo build` prints
exactly these 12 locations — work them to zero:

```
--> src/webhook/process.rs:383:17
--> src/webhook/process.rs:415:29
--> src/webhook/process.rs:433:37
--> src/webhook/process.rs:444:37
--> src/daemon/scheduled.rs:46:13
--> src/daemon/scheduled.rs:93:25
--> src/daemon/scheduled.rs:106:25
--> src/daemon/scheduled.rs:123:33
--> src/daemon/scheduled.rs:134:33
--> src/daemon/stream.rs:1057:45
--> src/daemon/stream.rs:1072:45
--> src/daemon/executor/knowledge/ghost.rs:74:13
```

**Use the compiler's list, not this one, once you start** — line numbers shift as
you edit. Re-run `cargo build` after each file and let the remaining warnings
tell you what is left. **Do not hunt for call sites by re-reading files.**

### All 12 call sites are already in `async` contexts

Verified while drafting — every one sits in an `async fn` (or an `async move`
block inside one), so adding `.await` is legal at each with **no further
cascade**:

| File | Enclosing `async fn` | Sites |
|---|---|---|
| `webhook/process.rs` | `maybe_analyze_alert` (`:277`) | 4 |
| `daemon/scheduled.rs` | `run_scheduled_job` (`:27`) | 5 |
| `daemon/stream.rs` | `run_conversation_loop` (`:63`) | 2 |
| `executor/knowledge/ghost.rs` | `spawn_ghost` (`:8`) | 1 |

**None is inside a synchronous closure** — the species that has forced
restructures elsewhere in this milestone. The `stream.rs` and `process.rs` sites
sit inside `match … .await { Ok(()) => … }` arms, which are ordinary async
context.

### The helper being wrapped

```rust
// src/webhook/process.rs:164
pub(crate) fn notify_chat_panes(sessions: &SessionStore, msg: &str) {
    let panes: Vec<String> = with_sessions(sessions, |store| { … });
    // Unlocked phase: everything blocking happens out here.
    for pane in &panes {
        let _ = std::process::Command::new("tmux").args([…]).output();
    }
}
```

**One wrap moves N subprocesses — one per active chat pane.** Its two *other*
call sites are already wrapped; this phase closes the third and last.

## Spec

### 1. Make the definition `async` — `src/webhook/process.rs:182`

```rust
pub(crate) async fn inject_ghost_event(sessions: &SessionStore, content: &str) {
```

Then `cargo build`. **Expect 0 errors and 12 warnings.** That is the checklist.

### 2. Wrap `notify_chat_panes` inside it

Replace the bare call. Post-`fmt` form, from the checked run:

```rust
    inject_into_sessions(sessions, &msg);
    // One-liner for the tmux display-message overlay (strip newlines).
    let one_liner = content.lines().next().unwrap_or(content);
    let s_ncp = sessions.clone();
    let line = one_liner.to_string();
    let _ = crate::tmux::off_runtime("notify-chat-panes", move || {
        notify_chat_panes(&s_ncp, &line)
    })
    .await;
    // Always mirror ghost lifecycle events to events.jsonl for troubleshooting.
```

`one_liner` borrows from `content`, so it **must** be copied to an owned `String`
(`line`) before the `move` closure — `spawn_blocking` requires `F: 'static`.
`inject_into_sessions` and the `log_event` call below are **unchanged**.

### 3. `.await` all 12 call sites

Work the compiler's warning list to zero, one file at a time, `cargo build` after
each file. Every site becomes `…).await;`. Two shapes appear:

```rust
// single-line
inject_ghost_event(&sessions, &msg).await;

// multi-line — fmt puts `.await;` on its own line after the closing paren
                inject_ghost_event(
                    &state.sessions,
                    &format!(
                        "[Ghost Shell Skipped] Concurrency limit reached for alert: {}",
                        alert.alert_name
                    ),
                )
                .await;
```

**Do not restructure any call site** — only append `.await`.

### 4. Run `cargo fmt --all` before finishing

`fmt` moves `.await;` onto its own line at the multi-line sites. This project has
**no `format_fix` hook**; unformatted code fails the gate.

## Acceptance criteria

- [ ] `grep -cF "pub(crate) async fn inject_ghost_event(sessions: &SessionStore, content: &str) {" src/webhook/process.rs`
      returns **1**, and `grep -cF "pub(crate) fn inject_ghost_event(sessions: &SessionStore, content: &str) {" src/webhook/process.rs`
      returns **0**.
- [ ] `grep -c "off_runtime" src/webhook/process.rs` returns **≥ 3** (printed
      **2** before; 1 added).
- [ ] `grep -cF "notify_chat_panes(sessions, one_liner);" src/webhook/process.rs`
      returns **0** — the bare call is gone.
- [ ] `grep -c "notify_chat_panes(" src/webhook/process.rs` returns **4** —
      **unchanged**. The definition plus three call sites, all now wrapped.
      **Not 3 and not 5**: this phase wraps a call, it does not add or remove one.
- [ ] The four `inject_ghost_event(` counts are **unchanged**: **5**
      (`webhook/process.rs`), **5** (`daemon/scheduled.rs`), **2**
      (`daemon/stream.rs`), **1** (`executor/knowledge/ghost.rs`). A different
      number means a call site was added, dropped, or duplicated.
- [ ] `cargo build 2>&1 | grep -c "^warning"` returns **0** — **this is the
      criterion that proves all 12 sites are awaited.** An unawaited call is a
      warning, not an error, so the build alone cannot be trusted; the warning
      count is what closes the loop.
- [ ] `grep -c "block_on\|futures::executor\|spawn_blocking"` returns **0** in
      all four edited files.
- [ ] `git diff --name-only` lists exactly **four** `src/` files:
      `webhook/process.rs`, `daemon/scheduled.rs`, `daemon/stream.rs`,
      `daemon/executor/knowledge/ghost.rs`.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **916** lib-unit and **27** integration tests.

**Run every gate bare.**

## Test plan

`inject_ghost_event` needs a live tmux server and populated sessions; the ghost
lifecycle paths need a running webhook listener or scheduler. **None of the 12
sites has unit coverage.** Pre-existing gap, neither widened nor closed here.

**The whole change compiled and the full suite passed with no test edited** in
the checked run — so if any test needs editing, **stop and report a blocker**.

**Write no new tests.** Run the suite and report which commands you ran and
whether they passed. **Do not claim any test guards these sites.**

Three reasoning checks. **Quote the code — a claim without a quotation is not an
answer:**

1. **Why the build never broke.** State in one sentence why adding `async` to the
   definition produced warnings rather than errors, and what that meant for how
   you found the 12 sites.
2. **The owned copy.** Quote your `let line = …` line and state in one sentence
   why `one_liner` could not be moved into the closure directly.
3. **The count that proves completeness.** Paste the output of
   `cargo build 2>&1 | grep -c "^warning"` and state in one sentence why a green
   `cargo build` alone would not have proven every site was awaited.

## End-to-end verification

None required. 06a demonstrated the timeout arm fires; this phase adds no
machinery. **Do not repeat that demonstration.**

## Authorizations

- [x] May change `inject_ghost_event`'s signature to `async` — this is the one
      signature change the phase exists to make.
- [x] May edit `src/webhook/process.rs`, `src/daemon/scheduled.rs`,
      `src/daemon/stream.rs`, `src/daemon/executor/knowledge/ghost.rs` — **the
      definition, the `notify_chat_panes` wrap, and the 12 `.await`s only.**
- [x] May add owned bindings and `.clone()` calls inside `inject_ghost_event`.
- [ ] **No** change to `notify_chat_panes`'s body or signature.
- [ ] **No** change to `inject_into_sessions` or the `log_event` call.
- [ ] **No** restructuring of any call site beyond appending `.await`.
- [ ] **No** other signature made `async`.
- [ ] **No** `block_on`, `futures::executor`, or detached `tokio::spawn`.
- [ ] **No** edits to `src/tmux/` or any file outside the four named.
- [ ] **No** new tests, no new dependency, no `#[allow(...)]`.

## Out of scope

- **`inject_into_sessions`** — it does per-session **file writes**, already in the
  collect-then-act shape from an earlier phase. Not a tmux call; not this phase.
- **`src/cli/`** — no concurrency to protect; bounded by stage A instead.
- **`src/tmux/{pane,session,window,cache}.rs`** — the sync helper bodies. Stage A
  hardens them with their own timeouts. **Do not touch them.**

### ⚠ Traps

1. **Do not hunt for call sites by reading files.** `cargo build`'s warnings are
   the authoritative, always-current list. Re-reading files in a loop is how runs
   stall.
2. **`grep -c "^warning"` must reach 0**, not just `cargo build` succeeding — an
   unawaited async call compiles.
3. **`one_liner` borrows `content`** — copy it to an owned `String` before the
   `move` closure.
4. **`notify_chat_panes(` stays at 4.** Not 3, not 5.
5. **Only append `.await`** — do not reflow or restructure a call site by hand;
   let `cargo fmt --all` do it.
6. **Run `cargo fmt --all` before finishing** — no auto-format hook here.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-28 19:18 (started)

**Executor:** Claude (Anthropic)

Making `inject_ghost_event` async, wrapping `notify_chat_panes` in `off_runtime`, and `.await`-ing all 12 call sites across 4 files.

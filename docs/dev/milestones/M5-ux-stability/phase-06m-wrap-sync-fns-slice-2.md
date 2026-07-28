# Phase 06m: Wrap Blocking Sync Functions — Slice 2

**Milestone:** M5 — UX & Stability
**Status:** in-progress
**Depends on:** phase-06i — `done` (established wrap-the-caller)
**Estimated diff:** ~100 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Apply the wrap-the-caller pattern from phase 06i to **7 more call sites**,
covering four synchronous helpers that async daemon code calls directly:

| Sync helper | Blocking work inside | Call sites |
|---|---|---|
| `daemon::detect_session` | one `tmux display-message` | 1 |
| `daemon::install_session_hooks` | **5** `tmux set-hook` calls | 2 |
| `daemon::utils::host::get_pane_remote_host` | one `tmux display-message` | 2 |
| `webhook::process::notify_chat_panes` | a `tmux display-message` **per chat pane**, in a loop | 2 |

**Finish condition: all 7 call sites are inside an `off_runtime` closure, and
no helper is edited.**

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
grep -c "off_runtime" src/daemon/mod.rs                          # expect 10
grep -c "off_runtime" src/daemon/hook.rs                         # expect 1
grep -c "off_runtime" src/daemon/executor/file_ops/read.rs       # expect 3
grep -c "off_runtime" src/daemon/executor/foreground.rs          # expect 29
grep -c "off_runtime" src/webhook/process.rs                     # expect 0
cargo test 2>&1 | grep "^test result" | head -3   # expect 916 lib, 0, 27 integration
```

**Every number above was produced by running that exact command against the
tree while drafting.** If one differs, **stop and report a blocker**.

## Current state

### ⭐ The pattern, already proven in this tree

Phase 06i wrapped five call sites this way and **edited no helper and no test**.
Two of its conversions, quoted verbatim as the shape to copy:

```rust
// String-returning helper — src/daemon/background/run.rs:334
let p = pane_id.to_string();
let w = win_name.to_string();
let body = tmux::off_runtime("capture-and-archive", move || {
    capture_and_archive(&p, &w, pipe_log)
})
.await
.unwrap_or_default();

// ()-returning helper taking a SessionStore — src/daemon/mod.rs:768
let s = sessions_gc.clone();
let _ = crate::tmux::off_runtime("gc-bg-windows", move || {
    crate::daemon::background::gc_bg_windows(&s)
})
.await;
```

`spawn_blocking` requires `F: 'static`, so **every borrowed argument becomes
owned before the closure**: `&str` → `.to_string()`, and `SessionStore` →
`.clone()` (it is a `#[derive(Clone)]` newtype over `Arc<Mutex<…>>`,
`src/daemon/session.rs:122`, so the clone is an `Arc` bump).

**The helper is never edited.** The wrap at the call site is what moves its
whole body — every tmux subprocess it makes — onto the blocking pool.

### This phase's 7 call sites

Line numbers are current-as-of-drafting; re-derive before editing.

| File:line | Call | Helper returns | Collapse |
|---|---|---|---|
| `daemon/mod.rs:446` | `detect_session()` | `Option<String>` | `.flatten()` — Hazard 1 |
| `daemon/mod.rs:580` | `install_session_hooks(sn, &hook_exe_path)` | `()` | `let _ = …` |
| `daemon/hook.rs:167` | `install_session_hooks(&session_name, &hook_exe)` | `()` | `let _ = …` |
| `executor/foreground.rs:311` | `get_pane_remote_host(target_str).is_some()` | `Option<String>` | `.flatten()` — Hazard 2 |
| `executor/file_ops/read.rs:140` | `get_pane_remote_host(pane).is_none()` | `Option<String>` | `.flatten()` — Hazard 2 |
| `webhook/process.rs:130` | `notify_chat_panes(&state.sessions, &first_line)` | `()` | `let _ = …` |
| `webhook/process.rs:471` | `notify_chat_panes(&state.sessions, &first_line)` | `()` | `let _ = …` |

### ⚠ Hazard 1 — `detect_session` returns `Option<String>`, so the collapse is `.flatten()`

```rust
// src/daemon/mod.rs:155
pub fn detect_session() -> Option<String> {
```

`off_runtime` yields `Option<Option<String>>`. Use **`.flatten()`** — neither
`.and_then(|r| r.ok())` nor `.unwrap_or_default()` is right here.

```rust
let inside_session = crate::tmux::off_runtime("detect-session", detect_session)
    .await
    .flatten();
```

`detect_session` takes **no arguments**, so it can be passed as a function
item — no closure and no owned bindings needed. (A `move || detect_session()`
closure also compiles; either is acceptable.)

**A timeout yields `None`, which reads as "not launched inside tmux."** The
daemon then falls through to the adopt-or-create branch, which is already
guarded by its own converted `session_exists` / `new-session` calls, and those
bail with a clear error if tmux is genuinely wedged. That is the correct
direction — do not substitute a placeholder session name.

### ⚠ Hazard 2 — `get_pane_remote_host` is a tmux call the scan cannot see

```rust
// src/daemon/utils/host.rs:11
pub fn get_pane_remote_host(pane_id: &str) -> Option<String> {
    let out = std::process::Command::new("tmux")
        .args(["display-message", "-t", pane_id, "-p", "#{pane_current_command}"])
        .output()
        .ok()?;
```

It spawns tmux, but its name contains no `tmux::`, so **the span-matching scan
used by earlier phases never flagged its call sites.** `foreground.rs` reports
`UNWRAPPED: 2` (only its two `Drop` calls) and still had this blocking call at
`:311`. Both call sites use the result as a boolean:

```rust
// foreground.rs:311
let is_remote_pane = get_pane_remote_host(target_str).is_some();

// read.rs:140
let (content, is_remote) = if get_pane_remote_host(pane).is_none() { … } else { … };
```

Collapse with `.flatten()`, then keep the existing `.is_some()` / `.is_none()`:

```rust
let t = target_str.to_string();
let is_remote_pane = crate::tmux::off_runtime("pane-remote-host", move || {
    crate::daemon::utils::host::get_pane_remote_host(&t)
})
.await
.flatten()
.is_some();
```

**A timeout yields `None` → `is_some()` is `false` → the pane is treated as
local.** That matches what the helper already returns when the `tmux` call
fails (`.ok()?` → `None`), so it is behaviour-preserving. **Do not invert
either test** — `foreground.rs` asks `.is_some()`, `read.rs` asks `.is_none()`,
and they must stay that way.

### ⚠ Hazard 3 — `notify_chat_panes` loops one subprocess per chat pane

```rust
// src/webhook/process.rs:161
pub(crate) fn notify_chat_panes(sessions: &SessionStore, msg: &str) {
    let panes: Vec<String> = with_sessions(sessions, |store| { … });
    // Unlocked phase: everything blocking happens out here.
    for pane in &panes {
        let _ = std::process::Command::new("tmux").args([…]).output();
    }
}
```

This is the highest-value wrap in the phase: **N subprocesses, one per active
chat pane, currently all on a runtime worker.** One wrap moves the whole loop.

Both call sites take `&state.sessions` (a `SessionStore` field on
`WebhookState`, `src/webhook/server.rs:24`) and a local `first_line: String`:

```rust
let s = state.sessions.clone();
let line = first_line.clone();
let _ = crate::tmux::off_runtime("notify-chat-panes", move || {
    notify_chat_panes(&s, &line)
})
.await;
```

`first_line` is already an owned `String` at both sites. **Clone it** rather
than moving it if it is used afterwards — at `:130` check whether it is;
at `:471` it is not. Either way the code must compile without editing
surrounding lines.

**Leave the `with_sessions` closure inside the helper alone.** It is already
the collect-then-act shape; the wrap goes around the whole call, so nothing
about the locking changes.

### 🛑 Three call sites are deliberately NOT in this phase

| Site | Why |
|---|---|
| `webhook/process.rs:190` | its caller `inject_ghost_event` (`:179`) is **synchronous**, so the wrap has to move further up its call chain — a different edit |
| `hook.rs:107`, `mod.rs:740` — `entry.cleanup_bg_windows()` | `SessionEntry` is **not `Clone`** (`session.rs:22`), and neither is `BgWindowInfo`, so `&self` cannot cross `spawn_blocking`. Needs owned data extracted first — a restructure |

Both go to 06n with `notify_session` and `handlers.rs:186`. **Do not attempt
either here**; `cleanup_bg_windows` in particular will not compile.

## Spec

### 1. Wrap the 7 call sites

Use the shapes above. **Every helper keeps its current signature**, and no
helper body is edited.

### 2. Edit no helper, no test

`detect_session`, `install_session_hooks`, `get_pane_remote_host` and
`notify_chat_panes` are edited **nowhere**.

### 3. Build after every site

Not a suggestion. `cargo build` after each wrapped site.

## Acceptance criteria

- [ ] `grep -c "off_runtime" src/daemon/mod.rs` returns **≥ 12** (printed
      **10** before; 2 sites added).
- [ ] `grep -c "off_runtime" src/daemon/hook.rs` returns **≥ 2** (printed
      **1** before; 1 added).
- [ ] `grep -c "off_runtime" src/daemon/executor/foreground.rs` returns
      **≥ 30** (printed **29** before; 1 added).
- [ ] `grep -c "off_runtime" src/daemon/executor/file_ops/read.rs` returns
      **≥ 4** (printed **3** before; 1 added).
- [ ] `grep -c "off_runtime" src/webhook/process.rs` returns **≥ 2** (printed
      **0** before; 2 added).
- [ ] **The four helpers are untouched.** Quote the result of:

```bash
git diff --stat HEAD -- src/daemon/utils/host.rs
git diff --stat -- src/daemon/mod.rs | grep -c "detect_session\|install_session_hooks" || true
grep -cF "pub fn detect_session() -> Option<String> {"          src/daemon/mod.rs        # 1
grep -cF "pub fn install_session_hooks(session_name: &str, hook_exe: &str) {" src/daemon/mod.rs  # 1
grep -cF "pub fn get_pane_remote_host(pane_id: &str) -> Option<String> {"     src/daemon/utils/host.rs  # 1
grep -cF "pub(crate) fn notify_chat_panes(sessions: &SessionStore, msg: &str) {" src/webhook/process.rs  # 1
```

      All four signature greps return **1**, and `host.rs` shows **no diff at
      all** — it contains only the helper, so any change to it is a scope
      violation.

- [ ] `grep -c "notify_chat_panes(" src/webhook/process.rs` returns **4** — the
      definition, the two wrapped call sites, and `:190` **left unwrapped**.
      Verify by reading that `:190` is still a bare call.
- [ ] `grep -c "cleanup_bg_windows()" src/daemon/hook.rs` returns **1** and the
      same for `src/daemon/mod.rs` — both still bare, neither wrapped.
- [ ] `grep -c "block_on\|futures::executor\|spawn_blocking"` returns **0** in
      all five edited files.
- [ ] `git diff --name-only` lists exactly **five** `src/` files.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **916** lib-unit and **27** integration tests.

**Run every gate bare.**

## Test plan

All seven call sites are in code needing a live tmux server, and the webhook
pair additionally needs a running webhook listener. **None has unit coverage.**
Pre-existing gap, neither widened nor closed here.

**As in 06i, the wrap approach should change no test.** If any test needs
editing, **stop and report a blocker** — it means a signature changed, which
this phase forbids.

**Write no new tests.** Run the suite and report which commands you ran and
whether they passed. **Do not claim any test guards these sites.**

Three reasoning checks. **Quote the code — a claim without a quotation is not
an answer:**

1. **The invisible call.** Paste the converted `foreground.rs:311` and state in
   one sentence why the span-matching scan never flagged this site, and what
   `is_remote_pane` becomes on timeout.
2. **The loop wrap.** Paste one converted `notify_chat_panes` call and say how
   many tmux subprocesses one wrap moves off the runtime.
3. **The two you did not touch.** Quote `hook.rs`'s `entry.cleanup_bg_windows()`
   line as you left it, and state in one sentence why it cannot be wrapped.

## End-to-end verification

None required. 06a demonstrated the timeout arm fires; this phase adds no
machinery. **Do not repeat that demonstration.**

## Authorizations

- [x] May edit `src/daemon/mod.rs`, `src/daemon/hook.rs`,
      `src/daemon/executor/foreground.rs`,
      `src/daemon/executor/file_ops/read.rs`, `src/webhook/process.rs` — **the
      seven named call sites only.**
- [x] May add owned bindings and `.clone()` calls at those call sites.
- [ ] **No** edit to `src/daemon/utils/host.rs` — not one line.
- [ ] **No** edit to any helper body: `detect_session`,
      `install_session_hooks`, `notify_chat_panes`.
- [ ] **No** signature change to any function, anywhere.
- [ ] **No** wrapping of `process.rs:190`, `hook.rs:107`, or `mod.rs:740`.
- [ ] **No** change to any test.
- [ ] **No** `block_on`, `futures::executor`, or detached `tokio::spawn`.
- [ ] **No** edits to `src/tmux/` or any file outside the five named.
- [ ] **No** new tests, no new dependency, no `#[allow(...)]`.

## Out of scope

- **`close_bg_window` and `watch_pane`** (`executor/mod.rs:561`, `:388`) — both
  return a string the model sees, so their timeout text is a product decision,
  and `watch_pane` calls `tokio::spawn` internally, which needs checking against
  `spawn_blocking`. Their own phase.
- **`notify_session`, `handlers.rs:186`, `process.rs:190`,
  `cleanup_bg_windows` ×2** — all need a shape change first. 06n.
- **`src/cli/`** — no concurrency to protect; bounded by stage A instead.
- **`src/tmux/{pane,session,window,cache}.rs`** — the sync helper bodies. Stage
  A hardens them with their own timeouts. **Do not touch them.**

### ⚠ Traps

1. **Do not edit the helpers.** `host.rs` must show a completely empty diff.
2. **`detect_session` and `get_pane_remote_host` return `Option`** —
   `.flatten()`, not `.ok()` and not `.unwrap_or_default()`.
3. **Do not invert the boolean tests** — `.is_some()` in `foreground.rs`,
   `.is_none()` in `read.rs`.
4. **`cleanup_bg_windows` cannot be wrapped** — `SessionEntry` is not `Clone`.
   Attempting it will not compile.
5. **`process.rs:190` stays bare** — its caller is synchronous.
6. **`detect_session` takes no arguments** — no owned bindings needed.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-28 03:59 (started)

**Executor:** Claude (Sonnet 4.5)
**Action:** Started phase 06m — wrapping 7 call sites across 5 files.

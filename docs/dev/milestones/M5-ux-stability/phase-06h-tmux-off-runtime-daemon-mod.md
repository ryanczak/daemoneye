# Phase 06h: tmux Calls Off the Runtime — `daemon/mod.rs`

**Milestone:** M5 — UX & Stability
**Status:** review
**Depends on:** phase-06g — `done`
**Estimated diff:** ~130 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Apply `tmux::off_runtime` to the **9** convertible tmux calls in
`src/daemon/mod.rs` — all inside `pub async fn run_daemon` (`:327`), covering
daemon startup and clean shutdown.

**6 further hits are in *synchronous* functions and are NOT this phase's.**

**Finish condition: the scan reports `6` for `src/daemon/mod.rs` — and every
one is inside `detect_session` or `install_session_hooks`.**

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
grep -c "off_runtime"      src/daemon/mod.rs   # expect 0
grep -c "io::Error::other" src/daemon/mod.rs   # expect 0
grep -c "anyhow::bail!"    src/daemon/mod.rs   # expect 4
cargo test 2>&1 | grep "^test result" | head -3   # expect 916 lib, 0, 27 integration
```

**Every number above was produced by running that exact command against the
tree while drafting.** If one differs, **stop and report a blocker**.

## Current state

### 🛑 6 hits are in *synchronous* functions — do NOT convert them

`off_runtime` is `async`; you cannot `.await` in a sync fn (`error[E0728]`),
and the fix is not to add `async` — it changes the signature and every call
site.

| Lines | Enclosing fn | Why deferred |
|---|---|---|
| 159 | `pub fn detect_session` (`:155`) — **sync** | called at `mod.rs:446` |
| 192, 208, 224, 246, 270 | `pub fn install_session_hooks` (`:184`) — **sync** | called at `mod.rs:547` **and `hook.rs:157`** |

Making these `async` is a **restructure**, deferred to its own phase alongside
the `close_bg_window` / `watch_pane` work. **Leave all six exactly as they
are.**

### ⭐ Worked examples — four shapes already in the tree

```rust
// bool gate — a timeout must not read as "yes" — foreground.rs:1038
let pid = pane_id.to_string();
let pane_alive = tmux::off_runtime("pane-exists", move || crate::tmux::pane_exists(&pid))
    .await
    .unwrap_or(false);

// inline std::process::Command, discarded — foreground.rs:797
let th = target_str.to_string();
let _ = tmux::off_runtime("set-hook", move || {
    std::process::Command::new("tmux")
        .args(["set-hook", "-t", &th, &shn, &nh])
        .output()
})
.await;

// ()-returning helper — foreground.rs:906
let _ = tmux::off_runtime("unhighlight-pane", move || tmux::unhighlight_pane(&t, cp.as_deref()))
    .await;

// Result whose Err is USED — collapse Option<Result<T>> to Result<T> — scheduled.rs:216
let created = tmux::off_runtime("create-job-window", move || tmux::create_job_window(&s, &t))
    .await
    .unwrap_or_else(|| Err(anyhow::anyhow!("timed out creating window")));
let pane_id = match created { Ok(p) => p, Err(e) => { /* unchanged */ } };
```

`spawn_blocking` requires `F: 'static`, so **every borrowed argument becomes
owned before the closure**.

### This phase's 9 sites

Line numbers are current-as-of-drafting; re-derive with the Acceptance-criteria
script.

| Line | Call | Returns | Collapse |
|---|---|---|---|
| 454 | `session_exists` | `bool` | `.unwrap_or(false)` — Hazard 1 |
| 458 | inline `Command` — `new-session` | 3-arm `match` | add a `None` arm — Hazard 2 |
| 496 | inline `Command` — `set-hook -g pane-died` | `io::Result<Output>` | Hazard 3 |
| 509 | inline `Command` — `set-hook -g after-new-session` | `io::Result<Output>` | Hazard 3 |
| 525 | inline `Command` — `set-hook -g client-attached` | `io::Result<Output>` | Hazard 3 |
| 538 | inline `Command` — `set-hook -g client-detached` | `io::Result<Output>` | Hazard 3 |
| 563 | `client_dimensions` | `(u16, u16)` | `.unwrap_or((0, 0))` — Hazard 4 |
| 813 | inline `Command` — `set-hook -gu` **in a loop** | `io::Result<Output>` | Hazard 3 + Hazard 5 |
| 836 | `stop_pipe_pane` **in a loop** | `()` | `let _ = …` |

### ⚠ Hazard 1 — `:454`'s bool gate decides whether to *create* a session

```rust
if crate::tmux::session_exists(&name) {
    log::info!("Managed tmux session '{}' already exists — adopting.", name);
    (Some(name.clone()), Some(name))
} else {
    match std::process::Command::new("tmux").args(["new-session", "-d", "-s", &name]).output() { … }
}
```

Use **`.unwrap_or(false)`**, the same rule as every other bool gate in this
sweep. Think through both branches before changing it:

- `.unwrap_or(false)` → falls to the `else`, attempts `new-session`. That call
  is **also** converted (Hazard 2), so a wedged tmux produces a clear
  `anyhow::bail!` and the daemon **fails to start with a real reason**.
- `.unwrap_or(true)` → adopts a session whose existence was never confirmed,
  and the daemon runs against a phantom session for its whole lifetime.

**Failing loudly at startup is the correct outcome.** Write
`.unwrap_or(false)`.

### ⚠ Hazard 2 — `:458` is a three-arm `match` that needs a fourth arm

```rust
match std::process::Command::new("tmux")
    .args(["new-session", "-d", "-s", &name])
    .output()
{
    Ok(o) if o.status.success() => { … (Some(name.clone()), Some(name)) }
    Ok(o) => {
        let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
        anyhow::bail!("Failed to create tmux session '{}': {}", name, stderr);
    }
    Err(e) => {
        anyhow::bail!("tmux new-session failed for '{}': {}", name, e);
    }
}
```

Bind the `off_runtime` result and add a `None` arm. **All three existing arms
stay byte-identical**, including the guard on the first:

```rust
let n = name.clone();
let created = tmux::off_runtime("new-session", move || {
    std::process::Command::new("tmux")
        .args(["new-session", "-d", "-s", &n])
        .output()
})
.await;

match created {
    Some(Ok(o)) if o.status.success() => { … unchanged … }
    Some(Ok(o)) => { … unchanged … }
    Some(Err(e)) => { … unchanged … }
    None => {
        anyhow::bail!("timed out creating tmux session '{}'", name);
    }
}
```

`bail!` is right here because **both existing failure arms already bail** —
`run_daemon` returns `anyhow::Result<()>`, and a daemon with no session cannot
proceed.

### ⚠ Hazard 3 — the hook installs are `std::io::Error`, NOT `anyhow`

Five sites (`:496`, `:509`, `:525`, `:538`, `:813`) have this shape:

```rust
if let Err(e) = std::process::Command::new("tmux")
    .args(["set-hook", "-g", "pane-died", &global_notify_cmd])
    .output()
{
    log::error!("Failed to register global tmux pane-died hook: {}", e);
}
```

The `Err` is **used**, so this is the `.unwrap_or_else(|| Err(…))` collapse —
but `.output()` returns `std::io::Result<Output>`, so
`Err(anyhow::anyhow!(…))` **will not type-check**. Synthesise an
`std::io::Error` instead. **This exact shape was compile-checked while
drafting:**

```rust
let c = global_notify_cmd.clone();
let res = tmux::off_runtime("set-hook-pane-died", move || {
    std::process::Command::new("tmux")
        .args(["set-hook", "-g", "pane-died", &c])
        .output()
})
.await
.unwrap_or_else(|| Err(std::io::Error::other("timed out installing hook")));
if let Err(e) = res {
    log::error!("Failed to register global tmux pane-died hook: {}", e);
}
```

**Each `log::error!` / `log::warn!` message stays exactly as it is.** A timeout
now logs the same line with the synthetic error text — which is the point: a
hook that failed to install must be visible in `daemon.log` either way.

### ⚠ Hazard 4 — `client_dimensions` returns a plain tuple

```rust
// src/tmux/session.rs:260
pub fn client_dimensions(session_name: &str) -> (u16, u16) {
```

Not `Result`, not `Option`. So `off_runtime` yields `Option<(u16, u16)>` and
**neither `.and_then(|r| r.ok())` nor `.flatten()` compiles**. Collapse with
`.unwrap_or((0, 0))`:

```rust
let s = sn.to_string();
let (w, h) = tmux::off_runtime("client-dimensions", move || {
    crate::tmux::client_dimensions(&s)
})
.await
.unwrap_or((0, 0));
if w > 0 && h > 0 { … unchanged … }
```

`(0, 0)` is deliberate — the existing `if w > 0 && h > 0` guard then skips
seeding the viewport, exactly as it does when tmux reports no client today.

### ⚠ Hazard 5 — `:813` and `:836` are inside `for` loops

`:813` iterates `for hook in &["pane-died", "after-new-session", …]`, so `hook`
is a `&&str`. Clone it into the closure and keep using `hook` in the log line —
only the clone is moved:

```rust
for hook in &["pane-died", "after-new-session", "client-attached", "client-detached"] {
    let h = hook.to_string();
    let res = tmux::off_runtime("set-hook-unset", move || {
        std::process::Command::new("tmux").args(["set-hook", "-gu", &h]).output()
    })
    .await
    .unwrap_or_else(|| Err(std::io::Error::other("timed out uninstalling hook")));
    if let Err(e) = res {
        log::warn!("Failed to uninstall global tmux hook '{}': {}", hook, e);
    }
}
```

`:836` iterates `for pane_id in &pipe_panes` where `pipe_panes: Vec<String>`.
`stop_pipe_pane` returns `()` (`src/tmux/pane.rs:273`), so it is a plain
discard — **no `.ok()`, no `.flatten()`**:

```rust
for pane_id in &pipe_panes {
    let p = pane_id.clone();
    let _ = tmux::off_runtime("stop-pipe-pane", move || crate::tmux::stop_pipe_pane(&p)).await;
}
```

**Do not hoist either loop body out of its loop**, and do not batch the calls.

### ⚠ The `with_sessions` closure at `:829` stays synchronous

`:836`'s enclosing block first collects `pipe_panes` inside a
`with_sessions(&sessions, |store| …)` closure, then loops **outside** it. That
separation is deliberate — it is the collect-under-the-lock / act-outside shape
this milestone spent phase 05 establishing. **The `.await` you add must stay
outside the closure.** Putting it inside would not compile and would re-create a
defect that was already fixed once.

## Spec

### 1. Convert the 9 sites

Match each to its collapse from the table above. **Preserve every existing
match arm, guard, log message and failure default exactly.**

### 2. Convert nothing in the two sync functions

`detect_session` and `install_session_hooks` keep all 6 of their hits and their
`pub fn` signatures.

### 3. Build after every site

Not a suggestion. `cargo build` after each converted site.

## Acceptance criteria

- [ ] **Scan reports `6`, all inside the two sync fns:**

```bash
python3 - <<'PY'
import re, pathlib
f = "src/daemon/mod.rs"
src = pathlib.Path(f).read_text()
spans = []
for m in re.finditer(r'off_runtime\s*\(', src):
    i = m.end()-1; d = 0
    while i < len(src):
        if src[i] == '(': d += 1
        elif src[i] == ')':
            d -= 1
            if d == 0: break
        i += 1
    spans.append((m.start(), i))
inside = lambda p: any(a <= p <= b for a, b in spans)
PURE = {"off_runtime", "TMUX_TIMEOUT", "cache"}
bad = [(src[:m.start()].count("\n")+1, m.group(1))
       for m in re.finditer(r'\btmux::(\w+)', src)
       if m.group(1) not in PURE and not inside(m.start())]
bad += [(src[:m.start()].count("\n")+1, 'Command::new("tmux")')
        for m in re.finditer(r'Command::new\("tmux"\)', src) if not inside(m.start())]
print(f"{f}: {len(bad)}")
for l, n in sorted(bad): print(f"      {l}: {n}")
PY
#   src/daemon/mod.rs: 6
#   all six are Command::new("tmux"), all between `pub fn detect_session`
#   and the end of `pub fn install_session_hooks` (~155-290)
```

      **Read the line numbers and confirm all six fall inside those two
      functions.** Any hit after `pub async fn run_daemon` means a site was
      missed.

- [ ] `grep -c "off_runtime" src/daemon/mod.rs` returns **≥ 9**. The command
      printed **0** before this phase. A floor, not an identity — the scan
      proves the exact set.
- [ ] `grep -c "io::Error::other" src/daemon/mod.rs` returns **≥ 5** — the four
      hook installs plus the uninstall loop. Printed **0** before this phase.
- [ ] `grep -c "anyhow::bail!" src/daemon/mod.rs` returns **≥ 5** — it printed
      **4** before, and the `None` arm at `:458` adds one.
- [ ] `pub fn detect_session` and `pub fn install_session_hooks` still have
      **`pub fn`** signatures, not `pub async fn`. Quote both lines.
- [ ] The `with_sessions(&sessions, |store| …)` closure near `:829` contains
      **no `.await`**. Verify by reading and quote the closure.
- [ ] `grep -c "block_on\|futures::executor\|spawn_blocking" src/daemon/mod.rs`
      returns **0**.
- [ ] `git diff --name-only` lists exactly **one** `src/` file.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **916** lib-unit and **27** integration tests.

**Run every gate bare.**

## Test plan

`run_daemon` forks, binds a Unix socket, spawns a tmux session and runs until
shutdown; it has **no unit coverage and cannot practically get any**.
Pre-existing gap, neither widened nor closed here. `mod.rs`'s `mod tests`
(`:845`) covers only `supervise` — a generic restart wrapper this phase does not
touch.

**Write no new tests.** Run the suite and report which commands you ran and
whether they passed. **Do not claim any test guards these sites.**

Three reasoning checks. **Quote the code — a claim without a quotation is not
an answer:**

1. **The startup gate.** Paste the converted `:454` and say, in one sentence,
   what the daemon does when `session_exists` times out — following the path
   through to the `new-session` site.
2. **The error type.** Paste one converted hook install. State why
   `anyhow::anyhow!` could not be used there, and confirm the `log::error!`
   message is unchanged.
3. **The lock boundary.** Quote the `with_sessions` closure near `:829` and
   confirm the `stop_pipe_pane` `.await` is outside it.

## End-to-end verification

None required. 06a demonstrated the timeout arm fires; this phase adds no
machinery. **Do not repeat that demonstration.**

## Authorizations

- [x] May edit `src/daemon/mod.rs` — **the nine named sites and whatever their
      expressions require.**
- [x] May add owned bindings and `.clone()` calls at call sites.
- [x] May add a `None` arm to the `:458` match and `std::io::Error::other`
      values for the five hook sites.
- [ ] **No** change to any function's signature — in particular **do not make
      `detect_session` or `install_session_hooks` `async`.**
- [ ] **No** change to any existing match arm, guard, or log message.
- [ ] **No** `.await` inside the `with_sessions` closure.
- [ ] **No** hoisting or batching of the two loop bodies.
- [ ] **No** `block_on`, `futures::executor`, or detached `tokio::spawn`.
- [ ] **No** edits to `src/tmux/` or any file other than `src/daemon/mod.rs`.
- [ ] **No** new tests, no new dependency, no `#[allow(...)]`.

## Out of scope

- **`detect_session` / `install_session_hooks`** — 6 sync hits; the restructure
  phase owns them, along with `hook.rs:157`'s call site.
- **`background/` (11), `session.rs` (2), `ghost.rs` (2), `hook.rs` (1),
  `server/` (3), `webhook/` (1), `cli/` (~19)** — later phases, survey first.
- **`src/tmux/{pane,session,window,cache}.rs`** — those `Command::new("tmux")`
  calls **are** the sync helper bodies, not call sites. Stage A hardens them.
  **Do not touch them.**

### ⚠ Traps

1. **Six hits are in sync fns.** Converting one yields `E0728`; do not "fix" it
   by adding `async`.
2. **`io::Error::other`, not `anyhow::anyhow!`**, at the five hook sites —
   `.output()` returns `io::Result`.
3. **`client_dimensions` returns a bare tuple** — `.unwrap_or((0, 0))`; neither
   `.ok()` nor `.flatten()` compiles.
4. **`.unwrap_or(false)` at `:454`** even though it leads to a `bail!`. Failing
   loudly at startup beats running against a phantom session.
5. **Keep the `.await` outside the `with_sessions` closure** near `:829`.
6. **`stop_pipe_pane` returns `()`** — plain discard.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-27 22:15 (started)

**Executor:** rexyMCP executor

Converting 9 tmux calls in `run_daemon` to `off_runtime`. Sites 1–9 converted in order, building after each. All 6 sync-function hits left untouched.

### Update — ts=1785191055244 (complete, server-authored)

**Summary:** **Summary + Notes for review:**

Converted all 9 tmux calls in `pub async fn run_daemon` to `crate::tmux::off_runtime`, building after each site. The conversions cover: (1) `session_exists` with `.unwrap_or(false)`, (2) `new-session` with a 4-arm match including `None`, (3–6) four `set-hook -g` installs using `io::Error::other` for timeout fallback, (7) `client_dimensions` with `.unwrap_or((0, 0))`, (8) the `set-hook -gu` uninstall loop, and (9) the `stop_pipe_pane` loop. All 6 sync-function hits in `detect_session` and `install_session_hooks` are untouched with `pub fn` signatures preserved. The `with_sessions` closure contains no `.await`.

**Verification results:** Scan reports exactly 6 remaining hits (all in sync fns at lines 159, 192, 208, 224, 246, 270). `off_runtime` count = 9, `io::Error::other` count = 5, `anyhow::bail!` count = 5. `cargo build`, `cargo clippy`, `cargo fmt`, and `cargo test` (916 lib + 27 integration) all pass clean. Working tree is clean.

**Executor:** Qwen/Qwen3.6-27B-FP8

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.28s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
s::get_labeled_context_client_viewport_absent_when_zero ... ok
test tmux::cache::tests::get_labeled_context_client_viewport_shown_when_known ... ok
test tmux::cache::tests::get_labeled_context_copy_mode_annotated ... ok
test tmux::cache::tests::get_labeled_context_pane_classification ... ok
test tmux::cache::tests::get_labeled_context_synchronized_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_session_topology ... ok
test tmux::cache::tests::get_labeled_context_dead_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test tmux::cache::tests::get_labeled_context_chat_pane_excluded_from_background ... ok
test search::tests::search_events_returns_tail_not_head_when_segment_exceeds_cap ... ok
test search::tests::search_finds_match_in_runbooks ... ok
test search::tests::search_respects_kind_filter ... ok
test session_store::tests::artifacts_round_trip ... ok
test search::tests::search_returns_empty_for_no_match ... ok
test session_store::tests::backfill_idempotent ... ok
test session_store::tests::backfill_missing_artifact_returns_error_name ... ok
test memory::tests::memory_frontmatter_tags_parsed ... ok
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

test result: ok. 916 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.67s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 29 tests
test daemon_ping_status_loop ... ignored
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g5_depth_limit_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g5_child_inherits_depth_and_parent ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test g4_briefing_injection_block_format ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test event_log_append_read ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test ghost_config_parsing ... ok
test minimal_config_parsing ... ok
test schedule_store_persistence ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test config_pricing_round_trip ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test g4_briefing_read_and_clear ... ok
test g4_briefing_injects_on_next_run ... ok
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
- `docs/dev/milestones/M5-ux-stability/phase-06h-tmux-off-runtime-daemon-mod.md` — +7 -1
- `src/daemon/mod.rs` — +76 -30

**Commit:** ae61877e9ba4a75e7c514b18aa7fab694d245fb3

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

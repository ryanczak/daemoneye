# Phase 06i: Wrap Blocking Sync Functions at Their Async Call Sites

**Milestone:** M5 — UX & Stability
**Status:** done
**Depends on:** phase-06j — `done` (the daemon's direct call sites are finished)
**Estimated diff:** ~70 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Some tmux work lives inside **synchronous** helper functions that async daemon
code calls directly. `off_runtime` is `async`, so it cannot be applied *inside*
those helpers without changing their signatures. **Wrap them at the call site
instead** — one `off_runtime` per async caller moves the whole helper, and every
tmux call it makes, off the runtime.

This phase establishes that pattern on **5 call sites**:

| Sync helper | Blocking work inside | Async call sites |
|---|---|---|
| `background::helpers::capture_and_archive` | a tmux capture + file read/write | 4 |
| `background::gc_bg_windows` | `kill_job_window` per window, looped over every session | 1 |

**This is the first phase of a new sub-pattern, so it is deliberately small** —
the same reason the `off_runtime` adapter itself landed on one file first. Later
slices apply the established shape more widely.

**Finish condition: all 5 call sites are inside an `off_runtime` closure, and
no helper signature changed.**

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
grep -c "off_runtime" src/daemon/background/run.rs      # expect 23
grep -c "off_runtime" src/daemon/background/respawn.rs  # expect 15
grep -c "off_runtime" src/daemon/mod.rs                 # expect 9
grep -c "capture_and_archive(" src/daemon/background/helpers.rs   # expect 1 (the definition)
cargo test 2>&1 | grep "^test result" | head -3         # expect 916 lib, 0, 27 integration
```

**Every number above was produced by running that exact command against the
tree while drafting.** If one differs, **stop and report a blocker**.

## Current state

### The pattern: wrap the *caller*, not the callee

Previous phases converted individual `tmux::foo(...)` calls sitting directly in
`async fn` bodies. These five are different: the tmux call is inside a
**synchronous helper**, and the helper is called from async code.

Two ways to fix that. **This phase uses the second**, and the choice is
deliberate:

| Approach | Cost |
|---|---|
| Make the helper `async`, convert each tmux call inside it | signature change + every call site + any unit test that calls it synchronously |
| **Wrap the helper at each async call site** | one `off_runtime` per call site; **no signature change, no test change** |

The wrap moves the helper's *entire* body — tmux subprocesses, file I/O, all of
it — onto the blocking pool, and bounds it with `TMUX_TIMEOUT`. The helper stays
synchronous and stays directly unit-testable.

### ⭐ The shape

`spawn_blocking` requires `F: 'static`, so **every borrowed argument becomes
owned before the closure**. For `&str` that is `.to_string()`; for
`SessionStore` it is `.clone()` — it is a `#[derive(Clone)]` newtype over
`Arc<Mutex<…>>` (`src/daemon/session.rs:122`), so cloning is an `Arc` bump, not
a deep copy.

```rust
// before
let body = capture_and_archive(&pane_id, &win_name, pipe_log);

// after
let p = pane_id.to_string();
let w = win_name.to_string();
let body = tmux::off_runtime("capture-and-archive", move || {
    capture_and_archive(&p, &w, pipe_log)
})
.await
.unwrap_or_default();
```

`pipe_log` is `Option<std::path::PathBuf>` — **already owned**. Move it in
directly; do not clone or re-wrap it.

### This phase's 5 call sites

Line numbers are current-as-of-drafting; re-derive before editing.

| File:line | Helper | Returns | Collapse |
|---|---|---|---|
| `background/run.rs:334` | `capture_and_archive(&pane_id, &win_name, pipe_log)` | `String` | `.unwrap_or_default()` |
| `background/run.rs:457` | `capture_and_archive(&pane_id_bg, &win_name_bg, pipe_log)` | `String` | `.unwrap_or_default()` |
| `background/respawn.rs:197` | `capture_and_archive(pane_id, win_name, pipe_log)` | `String` | `.unwrap_or_default()` |
| `background/respawn.rs:319` | `capture_and_archive(&pane_id_bg, &win_name_bg, pipe_log)` | `String` | `.unwrap_or_default()` |
| `daemon/mod.rs:768` | `gc_bg_windows(&sessions_gc)` | `()` | `let _ = …` |

**Note `respawn.rs:197` passes `pane_id` and `win_name` without `&`** — they are
already references there. Own them the same way (`.to_string()`); do not add an
extra `&`.

### ⚠ Hazard 1 — `capture_and_archive` returns `String`, and the timeout default matters

The helper returns a plain `String` (not `Result`), so `off_runtime` yields
`Option<String>` and the collapse is **`.unwrap_or_default()`** — neither
`.and_then(|r| r.ok())` nor `.flatten()` compiles.

`body` feeds the job-completion notification. On timeout it becomes the empty
string, so the notification still fires with no captured output — the same
outcome the helper already produces when it can neither read the pipe log nor
capture the pane. **Do not invent a placeholder message**; an empty body is the
existing failure representation.

### ⚠ Hazard 2 — `gc_bg_windows` is inside a supervised `async move` loop

```rust
// src/daemon/mod.rs:762
move || {
    let sessions_gc = sessions_gc_sup.clone();
    async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            crate::daemon::background::gc_bg_windows(&sessions_gc);
        }
    }
}
```

The closure already clones `sessions_gc` once per supervisor restart, and the
`loop` borrows it every minute. `spawn_blocking` needs an owned value **per
iteration**, so clone inside the loop:

```rust
loop {
    tokio::time::sleep(Duration::from_secs(60)).await;
    let s = sessions_gc.clone();
    let _ = crate::tmux::off_runtime("gc-bg-windows", move || {
        crate::daemon::background::gc_bg_windows(&s)
    })
    .await;
}
```

**Do not move `sessions_gc` itself into the closure** — the loop needs it again
on the next tick, and moving it will not compile. **Do not hoist the clone above
the `loop`** for the same reason.

`gc_bg_windows` returns `()`, so this is a plain discard.

**A timeout here skips one GC pass.** That is correct and self-healing: the next
tick is 60 s away and GC is idempotent. Do not add a retry.

### ⚠ Hazard 3 — do NOT touch `notify_session`

`background::helpers::notify_session` sits right beside `capture_and_archive`
and is called from the same two files, so it looks like it belongs here. **It
does not, and it will not compile if you try:**

```rust
// src/daemon/background/helpers.rs:146
pub(super) fn notify_session(sessions: &SessionStore, session_id: &str, job: BgJobInfo<'_>) {
```

`BgJobInfo<'_>` carries a lifetime, so it is **not `'static`** and cannot cross
`spawn_blocking`. Wrapping it requires an owned variant of `BgJobInfo` first —
a separate change, deliberately out of scope. Leave both `notify_session` call
sites (`run.rs:471`, `respawn.rs:334`) exactly as they are.

## Spec

### 1. Wrap the 5 call sites

Use the shape above. **Every helper keeps its current signature**, and no
helper body is edited.

### 2. Change no helper, no test

`capture_and_archive` and `gc_bg_windows` are edited **nowhere**. Their internal
tmux calls stay exactly as they are — wrapping the caller is what moves them off
the runtime.

### 3. Build after every site

Not a suggestion. `cargo build` after each wrapped site.

## Acceptance criteria

- [ ] `grep -c "off_runtime" src/daemon/background/run.rs` returns **≥ 25**
      (printed **23** before this phase; 2 sites added).
- [ ] `grep -c "off_runtime" src/daemon/background/respawn.rs` returns **≥ 17**
      (printed **15** before; 2 added).
- [ ] `grep -c "off_runtime" src/daemon/mod.rs` returns **≥ 10** (printed **9**
      before; 1 added).
- [ ] **Every `capture_and_archive(` call is inside an `off_runtime` closure.**
      Verify by reading each of the four and quoting one; a bare call remaining
      in an `async fn` means a site was missed.
- [ ] `grep -c "capture_and_archive(" src/daemon/background/helpers.rs` still
      returns **1** — the definition, unedited.
- [ ] `git diff --stat src/daemon/background/helpers.rs` shows **no change**.
      Quote the (empty) result.
- [ ] `grep -cF "pub(super) fn capture_and_archive(" src/daemon/background/helpers.rs`
      returns **1** and
      `grep -cF "pub fn gc_bg_windows(" src/daemon/background/gc.rs` returns
      **1** — both still `fn`, not `async fn`.
- [ ] `grep -c "notify_session(" src/daemon/background/run.rs` returns **1** and
      the same for `respawn.rs` — both untouched, neither wrapped.
- [ ] `git diff --name-only` lists exactly **three** `src/` files.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **916** lib-unit and **27** integration tests.

**Run every gate bare.**

## Test plan

All five call sites are in code that needs a live tmux server and a running
daemon; **none has unit coverage.** Pre-existing gap, neither widened nor closed
here.

**The point of the wrap approach is that it changes no test.** `gc.rs`'s
`mod tests` covers `plan_gc_actions`, and `helpers.rs`'s covers
`trim_large_output` — both call synchronous helpers this phase leaves
synchronous. If any test needs editing, **stop and report a blocker**: it means
a signature changed, which this phase forbids.

**Write no new tests.** Run the suite and report which commands you ran and
whether they passed. **Do not claim any test guards these sites.**

Three reasoning checks. **Quote the code — a claim without a quotation is not
an answer:**

1. **The wrap, not the callee.** Paste one converted `capture_and_archive` call
   site and confirm `helpers.rs` is unchanged, naming what moved off the runtime
   as a result.
2. **The GC clone.** Paste the converted `gc_bg_windows` loop and say in one
   sentence why the clone is inside the `loop` rather than above it.
3. **`notify_session`.** Quote its signature and state in one sentence why it
   cannot be wrapped the same way.

## End-to-end verification

None required. 06a demonstrated the timeout arm fires; this phase adds no
machinery. **Do not repeat that demonstration.**

## Authorizations

- [x] May edit `src/daemon/background/run.rs`,
      `src/daemon/background/respawn.rs`, `src/daemon/mod.rs` — **the five named
      call sites only.**
- [x] May add owned bindings and `.clone()` calls at those call sites.
- [ ] **No** edit to `src/daemon/background/helpers.rs` or
      `src/daemon/background/gc.rs` — not one line.
- [ ] **No** signature change to any function, anywhere.
- [ ] **No** wrapping of `notify_session`.
- [ ] **No** change to any test.
- [ ] **No** `block_on`, `futures::executor`, or detached `tokio::spawn`.
- [ ] **No** edits to `src/tmux/` or any file outside the three named.
- [ ] **No** new tests, no new dependency, no `#[allow(...)]`.

## Out of scope

- **`notify_session`** — needs an owned `BgJobInfo` first. Hazard 3.
- **The other sync helpers** — `close_bg_window`, `watch_pane`'s prologue,
  `detect_session`, `install_session_hooks`, `cleanup_bg_windows`,
  `webhook/process.rs`, `utils/host.rs`, and `handlers.rs:186`'s `.filter()`
  closure. Later slices, once this phase has established the shape.
- **`src/cli/`** — no concurrency to protect; bounded by stage A's helper
  timeouts instead.
- **`src/tmux/{pane,session,window,cache}.rs`** — the sync helper bodies. Stage
  A hardens them with their own timeouts. **Do not touch them.**

### ⚠ Traps

1. **Do not edit the helpers.** The whole point is that the wrap goes at the
   call site. An edit to `helpers.rs` or `gc.rs` fails an acceptance criterion.
2. **`capture_and_archive` returns `String`** — `.unwrap_or_default()`, not
   `.ok()` and not `.flatten()`.
3. **Clone `sessions_gc` inside the `loop`**, not above it.
4. **`respawn.rs:197` already passes references** — own them, don't add a `&`.
5. **`pipe_log` is already owned** — move it, don't clone it.
6. **`notify_session` cannot be wrapped** — `BgJobInfo<'_>` is not `'static`.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-28 02:21 (started)

**Executor:** Claude (Sonnet 4.5)
**Work:** Wrapping 5 sync helper call sites with `off_runtime` in `run.rs`, `respawn.rs`, and `daemon/mod.rs`.

### Update — ts=1785205706495 (complete, server-authored)

**Summary:** All 5 call sites are wrapped with `off_runtime` across 3 files. **Summary + Notes for review:**

- **What was built:** Wrapped `capture_and_archive` at 4 async call sites (`run.rs:334`, `run.rs:457`, `respawn.rs:197`, `respawn.rs:319`) and `gc_bg_windows` at 1 call site (`daemon/mod.rs:768`) with `tmux::off_runtime`, moving all tmux subprocesses and file I/O onto the blocking pool. No helper signatures changed, no helpers edited, no tests touched.
- **Verification:** `off_runtime` counts are now 25/17/10 (up from 23/15/9). `helpers.rs` and `gc.rs` are untouched. `notify_session` remains unwrapped as required. `cargo build`, `cargo clippy`, `cargo fmt`, and `cargo test` (916 lib + 27 integration) all pass clean. Working tree is clean.

**Executor:** Qwen/Qwen3.6-27B-FP8

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
ient_viewport_absent_when_zero ... ok
test tmux::cache::tests::get_labeled_context_copy_mode_annotated ... ok
test tmux::cache::tests::get_labeled_context_dead_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_client_viewport_shown_when_known ... ok
test tmux::cache::tests::get_labeled_context_synchronized_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_session_topology ... ok
test tmux::cache::tests::get_labeled_context_pane_classification ... ok
test tmux::cache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test tmux::cache::tests::get_labeled_context_chat_pane_excluded_from_background ... ok
test search::tests::search_events_returns_tail_not_head_when_segment_exceeds_cap ... ok
test memory::tests::memory_without_frontmatter_has_empty_metadata ... ok
test search::tests::search_respects_kind_filter ... ok
test search::tests::search_returns_empty_for_no_match ... ok
test session_store::tests::artifacts_round_trip ... ok
test session_store::tests::backfill_idempotent ... ok
test session_store::tests::backfill_missing_artifact_returns_error_name ... ok
test memory::tests::migrate_namespace_skips_already_migrated ... ok
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

test result: ok. 916 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.18s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 29 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g4_briefing_injection_block_format ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_ask_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test event_log_append_read ... ok
test ipc_session_info_round_trip ... ok
test ghost_config_parsing ... ok
test schedule_store_persistence ... ok
test minimal_config_parsing ... ok
test session_jsonl_round_trip ... ok
test config_pricing_round_trip ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_namespace_field_persisted ... ok
test g6_agent_config_roundtrip ... ok
test event_log_entry_format ... ok
test session_index_persistence ... ok
test g4_briefing_masking_applied ... ok
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
- `docs/dev/milestones/M5-ux-stability/phase-06i-wrap-sync-fns-at-call-sites.md` — +6 -1
- `src/daemon/background/respawn.rs` — +14 -2
- `src/daemon/background/run.rs` — +14 -2
- `src/daemon/mod.rs` — +5 -1

**Commit:** 7e911975a6dbe9075e3b5f0dfd9de8b6cc940b44

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-07-28

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** Qwen/Qwen3.6-27B-FP8 (58 turns)
- **Scope deviations:** none
- **Calibration:** none

All four gates re-run bare and green (`cargo fmt --all --check`, `cargo build`
after `touch`ing all three edited files — zero warnings, `cargo clippy
--all-targets --all-features -- -D warnings`, `cargo test` at 916 lib + 27
integration, unchanged).

**The wrap-the-caller pattern is established, and its defining property holds:**

```
$ git diff --stat HEAD~2 HEAD -- src/daemon/background/helpers.rs src/daemon/background/gc.rs
$
```

Empty. **Neither helper was touched — not one line** — and both keep their
synchronous signatures (`pub(super) fn capture_and_archive(`,
`pub fn gc_bg_windows(`, one occurrence each). No test changed. That is the
whole point of the approach: the same blocking work moved off the runtime that
making these `async` would have achieved, but with zero signature churn and
`close_bg_window`-style direct unit tests left intact for the later slices.

`off_runtime` counts are 25 / 17 / 10 against a verified 23 / 15 / 9 — exactly
+2 / +2 / +1. Three `src/` files in the code commit.

Verified by reading, since counts cannot show these:

- **All four `capture_and_archive` calls are inside `off_runtime` closures.**
  The four remaining textual matches in `run.rs`/`respawn.rs` are the calls
  *within* the closure bodies, at `:337`, `:466`, `:200` and `:328` — there is
  no bare call left in an `async fn`. Each collapses with
  `.unwrap_or_default()`, correct for a `String`-returning helper, so a timeout
  yields the empty body the helper already produces when it can neither read the
  pipe log nor capture the pane.
- **`respawn.rs:197` was owned without an extra `&`.** It passed `pane_id` and
  `win_name` as existing references; the conversion took `.to_string()` on each
  and passes `&p`/`&w` inside — matching the other three sites' final shape
  without double-referencing.
- **`pipe_log` is moved, not cloned**, at all four sites — correct, since it is
  already an owned `Option<PathBuf>`.
- **The GC clone is inside the `loop`**, not hoisted above it, so `sessions_gc`
  survives for the next tick 60 s later. `gc_bg_windows` returns `()`, so the
  result is plainly discarded, and a timeout skips exactly one idempotent GC
  pass with no retry added.
- **`notify_session` was left alone** — one call site each in `run.rs` and
  `respawn.rs`, neither wrapped. It sits directly beside `capture_and_archive`
  and would not have compiled (`BgJobInfo<'_>` is not `'static`); naming it as a
  do-not-touch hazard rather than leaving it to be discovered was worth the
  paragraph.

Test plan honoured, and its stronger form was satisfied: the phase said *"if any
test needs editing, stop and report a blocker"*, and none did.

**Note for the later slices.** The wrap approach means the span-matching scan is
no longer the finish condition — the `Command::new("tmux")` calls stay inside the
helpers, so the scan output is unchanged by design. 06m and 06n should keep
using call-site-shaped criteria (`off_runtime` floors plus a `git diff --stat`
showing the helper untouched), as this phase did.

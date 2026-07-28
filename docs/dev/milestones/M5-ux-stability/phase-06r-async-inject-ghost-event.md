# Phase 06r: Make `inject_ghost_event` Async — the Last Mechanism-B Site

**Milestone:** M5 — UX & Stability
**Status:** done
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

### Update — ts=1785266807751 (complete, server-authored)

**Summary:** **Summary + Notes for review:**

Made `inject_ghost_event` async, wrapped its `notify_chat_panes` call in `off_runtime` (spawn_blocking), and `.await`-ed all 12 call sites across 4 files (`webhook/process.rs`, `daemon/scheduled.rs`, `daemon/stream.rs`, `daemon/executor/knowledge/ghost.rs`). No restructuring was needed — every call site was already in an async context. The build stayed green throughout because calling an async fn without `.await` produces an unused-Future warning, not an error, so the compiler enumerated all 12 sites. All acceptance criteria verified: `off_runtime` count is 3 (was 2), bare `notify_chat_panes(sessions, one_liner)` is gone, `notify_chat_panes(` stays at 4, all inject counts unchanged, 0 warnings, 916 lib + 27 integration tests pass, `clippy -D warnings` clean, `fmt` clean. Working tree is clean.

**Executor:** Qwen/Qwen3.6-27B-FP8

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.06s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
ne_noted ... ok
test tmux::cache::tests::get_labeled_context_pane_classification ... ok
test tmux::cache::tests::get_labeled_context_client_viewport_absent_when_zero ... ok
test tmux::cache::tests::get_labeled_context_synchronized_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_client_viewport_shown_when_known ... ok
test tmux::cache::tests::get_labeled_context_session_topology ... ok
test tmux::cache::tests::get_labeled_context_copy_mode_annotated ... ok
test tmux::cache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test tmux::cache::tests::get_labeled_context_chat_pane_excluded_from_background ... ok
test search::tests::search_events_returns_tail_not_head_when_segment_exceeds_cap ... ok
test search::tests::search_finds_match_in_runbooks ... ok
test memory::tests::memory_without_frontmatter_has_no_tags ... ok
test search::tests::search_returns_empty_for_no_match ... ok
test session_store::tests::artifacts_round_trip ... ok
test session_store::tests::backfill_idempotent ... ok
test session_store::tests::backfill_missing_artifact_returns_error_name ... ok
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

test result: ok. 916 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.32s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 29 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g4_briefing_injection_block_format ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test ipc_ask_round_trip ... ok
test event_log_entry_format ... ok
test minimal_config_parsing ... ok
test ghost_config_parsing ... ok
test schedule_store_persistence ... ok
test event_log_append_read ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g4_briefing_read_and_clear ... ok
test config_pricing_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
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
- `docs/dev/milestones/M5-ux-stability/phase-06r-async-inject-ghost-event.md` — +7 -1
- `src/daemon/executor/knowledge/ghost.rs` — +2 -1
- `src/daemon/scheduled.rs` — +10 -5
- `src/daemon/stream.rs` — +4 -2
- `src/webhook/process.rs` — +15 -6

**Commit:** d6cd5122ec50cc664ac788cc3c28252814727678

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-07-28

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** Qwen/Qwen3.6-27B-FP8 (65 turns)
- **Scope deviations:** none
- **Calibration:** none for the executor; one `WORKFLOW.md` refinement candidate
  (below)

All four gates re-run bare and green (`cargo fmt --all --check`, `cargo build`
after `touch`ing all four edited files, `cargo clippy --all-targets
--all-features -- -D warnings`, `cargo test` at 916 lib + 27 integration,
unchanged).

**The completeness criterion reads 0:**

```
$ touch <the four files> && cargo build 2>&1 | grep -c "^warning"
0
```

That is the one that matters here — a green build proves nothing when the
failure mode is a warning.

Every other criterion is exact: the `async` declaration **1** and the old sync
declaration **0**; `off_runtime` in `process.rs` **3** (2 before); the bare
`notify_chat_panes(sessions, one_liner);` **0**; `notify_chat_panes(` **4** —
unchanged, neither 3 nor 5; all four `inject_ghost_event(` counts unchanged at
**5 / 5 / 2 / 1**; `block_on`/`spawn_blocking` **0** in all four; four `src/`
files in the code commit.

Verified by reading:

- **Every one of the 12 sites is a pure `.await` append.** `);` → `)\n.await;`
  and nothing else — no reflow, no restructuring, no argument changes. The added
  diff contains **13** lines that are exactly `.await;`: the 12 call sites plus
  the one closing the new `off_runtime` wrap.
- **Exactly one function became `async`.** The only `+…async fn` line in the diff
  is `inject_ghost_event` itself; the cascade did not leak into any other
  signature.
- **`notify_chat_panes` is untouched** — no `[-+]` line matches its signature or
  its `Command::new` loop. Its `with_sessions` collect-then-act shape is intact,
  and the wrap goes around the whole call.
- **`one_liner` was correctly copied** to an owned `String` (`line`) before the
  `move` closure, since it borrows from `content` and `spawn_blocking` needs
  `F: 'static`. `inject_into_sessions` and the trailing `log_event` are unchanged.
- **No test was touched.**

### This closes the milestone's fourth exit criterion

Every tmux subprocess reachable from the daemon's async contexts is now either
off the runtime or wrapped at its call site — whether it sat directly in an
`async fn` (04x/06a–06j), inside a synchronous helper an async caller invokes
(06i/06m/06p/06n), under a lock guard (05x/06q), or behind a synchronous function
that had to become `async` (this phase).

### Calibration — a `WORKFLOW.md` refinement candidate, at one occurrence

`WORKFLOW.md` § "Prefer additive change shapes" treats every multi-site mutation
as break-the-world: it warns that the tree stops compiling the moment the
definition changes and prescribes a hand-authored leaf-first edit order with
build checkpoints.

**A Rust `fn` → `async fn` change does not behave that way.** Calling an
`async fn` without `.await` is an unused-`Future` *warning*, so the build stays
green at every intermediate step and the verifier's consecutive-failure limit has
nothing to fire on. This run confirms it end to end: 65 turns, zero bounces, no
stall, across what the README had classified as a 13-site cascade well past the
≤3-site blast radius the fold bounds mutations to.

That inverts the remedy. For a warn-the-world mutation the *compiler* is the
ordered site list — and a better one than the architect's, because it stays
current as line numbers shift. The spec said so explicitly and forbade hunting
call sites by re-reading files, which is the behaviour that stalls runs.

**One occurrence — noted, not folded.** If a second warn-the-world cascade
appears, the natural landing spot is a short carve-out in § "Prefer additive
change shapes" distinguishing *break*-the-world mutations (required fields,
derive graphs, signature/type changes) from *warn*-the-world ones
(`fn` → `async fn`), with "let the compiler enumerate the sites, and pin a
zero-warning criterion" as the latter's remedy. **No doc change made.**

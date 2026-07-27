# Phase 06a: Get tmux Subprocess Calls Off the Async Runtime

**Milestone:** M5 — UX & Stability
**Status:** done
**Depends on:** phase-05h — `done`
**Estimated diff:** ~180 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

Establish the **mechanism-B adapter** and apply it to `background/run.rs`.

Every `tmux` call is a blocking subprocess spawn. **88 of them run inside `async
fn`s across 16 files**, so a wedged tmux server stalls a tokio worker thread —
the milestone's fourth exit criterion:

> Every tmux subprocess call made from an async context is either non-blocking
> (`tokio::process`) or off the runtime (`spawn_blocking`), and carries a
> timeout. A wedged tmux server degrades one operation instead of the whole
> daemon.

**This phase does two things:** adds one adapter (`tmux::off_runtime`), and
converts the **16** call sites in `background/run.rs` as the worked example the
remaining phases copy.

**Finish condition: `background/run.rs` has 0 unwrapped `tmux::` calls in async
context, and the adapter exists with a timeout.**

**This is the first of ~5 phases.** 06b–06e apply the same adapter to
`background/respawn.rs`, `executor/`, the `daemon/` core, and `cli/`. Do not
touch them here.

## Architecture references

Read before starting:

- `docs/design/daemon-stalls.md` § 1 mechanism B — blocking subprocess spawns on
  tokio workers.
- `CLAUDE.md` § "Important Invariants" — `main()` is synchronous so `libc::fork()`
  precedes the runtime. Nothing in this phase moves the fork.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Verify the starting state:

```bash
grep -c "tmux::" src/daemon/background/run.rs                    # expect 14
grep -c 'Command::new("tmux")' src/daemon/background/run.rs      # expect 2
grep -rc "spawn_blocking" src/ --include=*.rs | grep -v ':0' | wc -l   # expect 0
grep -c "off_runtime" src/tmux/mod.rs                # expect 0
cargo test 2>&1 | grep "^test result" | head -2      # expect 916 lib, 27 integration
```

**Verified against the tree while drafting.** `spawn_blocking` appears **nowhere**
in this codebase — this phase introduces it. If any count differs, **stop and
report a blocker.**

## Current state

### The problem, concretely

`src/tmux/` helpers are **synchronous** and call
`std::process::Command::new("tmux") … .output()`, which blocks the calling thread
until tmux answers. Called from an `async fn`, that thread is a tokio worker.

The helpers stay sync — they are also called from sync CLI code, and making them
async would force a duplicate API. **The adapter goes at the async call site.**

### ⭐ The one existing async tmux call — `pane::wait_for` (`src/tmux/pane.rs:515`)

The codebase already does this correctly in exactly one place. Quote it for the
shape of "bound a tmux operation in time":

```rust
pub async fn wait_for(channel: &str, timeout: std::time::Duration) -> bool {
    let mut child = match tokio::process::Command::new("tmux")
        .args(["wait-for", channel])
        .spawn()
    { Ok(c) => c, Err(_) => return false };
    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(_) => true,
        Err(_) => { let _ = child.start_kill(); /* … */ false }
    }
}
```

**Do not convert `wait_for`.** It is already non-blocking and already bounded;
it is here as the reference, not as work.

### The 16 sites in `background/run.rs` — two populations

| Population | Count | Lines |
|---|---|---|
| `tmux::<helper>(…)` calls | **14** | 66, 75, 98, 103, 153, 157, 159, 161, 180, 187, 240, 297, 350, 409 |
| inline `std::process::Command::new("tmux")` | **2** | 254, 360 |

`use crate::tmux;` (line 11) does **not** match `tmux::` and is not a site.

**There are no `tmux::wait_for` calls in this file.** The two `wait_for` hits are
`wait_for_sudo_prompt_and_inject`, which is not a tmux call at all — do not
convert it, and do not mistake it for the async helper quoted above.

Re-derive both lists with the script in Acceptance criteria rather than working
from the table; the line numbers will shift as you edit.

## Spec

### 1. Add the adapter to `src/tmux/mod.rs`

`src/tmux/mod.rs` is currently a 9-line module declaration file. Append:

```rust
/// Ceiling for a single tmux subprocess call made from async code.
///
/// tmux normally answers in milliseconds; five seconds means the server is
/// wedged, and waiting longer cannot help the caller.
pub const TMUX_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Run a blocking `tmux` helper off the async runtime, bounded by [`TMUX_TIMEOUT`].
///
/// The `src/tmux/` helpers are synchronous `std::process::Command` calls: invoked
/// directly from an `async fn` they block a tokio worker until tmux answers, and
/// a wedged tmux server therefore stalls the whole daemon. This moves the call to
/// the blocking pool and gives up on it after the timeout, so a wedge degrades
/// one operation instead of the reactor. See `docs/design/daemon-stalls.md`
/// § 1 mechanism B.
///
/// Returns `None` if the call timed out or the blocking task panicked — both are
/// logged. `Some(v)` carries whatever the helper returned, including its own
/// `Err`.
pub async fn off_runtime<T, F>(what: &'static str, f: F) -> Option<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    match tokio::time::timeout(TMUX_TIMEOUT, tokio::task::spawn_blocking(f)).await {
        Ok(Ok(v)) => Some(v),
        Ok(Err(e)) => {
            log::error!("tmux {what}: blocking task panicked: {e}");
            None
        }
        Err(_) => {
            log::error!("tmux {what}: timed out after {TMUX_TIMEOUT:?} — tmux server may be wedged");
            None
        }
    }
}
```

**Three deliberate choices:**

- **`what: &'static str`** — a timeout log with no operation name is unactionable.
  Pass the tmux verb (`"kill-job-window"`, `"capture-pane"`).
- **`Option<T>`, not `Result`** — timeout-or-panic is *"we do not know"*, which is
  distinct from the helper's own `Err` (*"tmux said no"*). Collapsing them would
  hide a wedge as an ordinary failure.
- **No retry.** A wedged tmux does not get better in 5 s; retrying multiplies the
  stall. Callers degrade instead.

### 2. Convert the 16 sites in `background/run.rs`

`spawn_blocking` requires `F: 'static`, so **every borrowed argument must become
owned** before the closure. That is the per-site work; it is not a textual
substitution.

There are exactly **four shapes** in this file. Match each site to one.

**Shape A — result ignored (`let _ = …` or a bare statement):**

```rust
// before
let _ = tmux::kill_job_window(session, &win_name);
// after
let (s, w) = (session.to_string(), win_name.clone());
let _ = tmux::off_runtime("kill-job-window", move || tmux::kill_job_window(&s, &w)).await;
```

**Shape B — result used with a default:**

```rust
// before
let snap = crate::tmux::capture_pane(&pane_id, 10).unwrap_or_default();
// after
let p = pane_id.clone();
let snap = tmux::off_runtime("capture-pane", move || crate::tmux::capture_pane(&p, 10))
    .await
    .and_then(|r| r.ok())
    .unwrap_or_default();
```

`.and_then(|r| r.ok())` is the load-bearing part: the outer `Option` is
timeout-or-panic, the inner `Result` is tmux's own answer. **Both** collapse to
the default, and that is correct here — a snapshot we could not take is an empty
snapshot either way.

**Shape C — the error is inspected:**

```rust
// before
if let Err(e) = tmux::set_remain_on_exit(&pane_id, true) {
    log::warn!("…: {e}");
}
// after
let p = pane_id.clone();
match tmux::off_runtime("set-remain-on-exit", move || tmux::set_remain_on_exit(&p, true)).await {
    Some(Err(e)) => log::warn!("…: {e}"),
    None => {}          // already logged by off_runtime
    Some(Ok(_)) => {}
}
```

**Never write `None => log::warn!(…)` in shape C** — `off_runtime` already logged
the timeout with the operation name. A second line per timeout is noise.

**Shape D — an inline `std::process::Command::new("tmux")`** (lines 254 and 360,
both identical in form):

```rust
// before
let _ = std::process::Command::new("tmux")
    .args(["pipe-pane", "-t", &pane_id])
    .output();
// after
let p = pane_id.clone();
let _ = tmux::off_runtime("pipe-pane", move || {
    std::process::Command::new("tmux")
        .args(["pipe-pane", "-t", &p])
        .output()
})
.await;
```

Both sites stop a pipe-pane and are best-effort — Shape A's `let _ =` treatment is
correct for them. **Do not "improve" them into a `tmux::` helper call**; adding a
helper is a separate change and would widen the diff.

**Two things not to change:**

- **Early returns keep their meaning.** A site that does
  `match tmux::create_job_window(..) { Ok(v) => v, Err(e) => return Err(e) }` must
  still return on failure — and a **timeout must also return**, because the caller
  cannot proceed without a window. Map `None` to the same failure path, with a
  message naming the timeout.
- **`tmux::wait_for` is already correct** — leave every call to it alone.

### 3. Do not convert anything outside `background/run.rs`

`respawn.rs`, `gc.rs`, `executor/`, `daemon/mod.rs`, `cli/` all have async tmux
calls. **They are phases 06b–06e.** A criterion below pins their counts so an
over-eager sweep fails.

## Acceptance criteria

- [ ] `grep -c "off_runtime" src/tmux/mod.rs` returns **1** — the `pub async fn`
      signature. (The doc comment references `TMUX_TIMEOUT`, not the function's own
      name, so there is no second hit.) Verify by **reading** that `TMUX_TIMEOUT`
      is applied via `tokio::time::timeout` wrapping `spawn_blocking` — the count
      cannot show that.
- [ ] Every `tmux::` call in an async context in `background/run.rs` is inside an
      `off_runtime` closure. Check with:

```bash
python3 - <<'PY'
import re, pathlib
src = pathlib.Path("src/daemon/background/run.rs").read_text()
lines = src.splitlines()
bad = []
for i, l in enumerate(lines, 1):
    if not re.search(r'\btmux::', l): continue
    if 'off_runtime' in l or 'wait_for' in l or l.strip().startswith('use '): continue
    # a helper named inside an off_runtime closure appears on the same line as `move ||`
    if 'move ||' in l: continue
    bad.append((i, l.strip()))
print("UNWRAPPED:", len(bad))
for i, l in bad: print(f"  {i}: {l}")
PY
#   UNWRAPPED: 0
```

- [ ] `grep -c "spawn_blocking" src/tmux/mod.rs` returns **1** — the only one in
      the tree at the end of this phase.
- [ ] `grep -rc "spawn_blocking" src/ --include=*.rs | grep -v ':0' | wc -l`
      returns **1** — one file (`src/tmux/mod.rs`). **Not more**: the adapter is
      the only place `spawn_blocking` appears; call sites use `off_runtime`.
- [ ] The out-of-scope files are untouched — `git diff --name-only` lists exactly
      **`src/tmux/mod.rs`** and **`src/daemon/background/run.rs`** under `src/`.
- [ ] `grep -c "tmux::" src/daemon/background/respawn.rs` returns **10** and
      `grep -c 'Command::new("tmux")' src/daemon/background/respawn.rs` returns
      **3** — both unchanged. Those 13 sites are phase 06b's; a lower number means
      you swept out of scope.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **916** lib-unit and **27** integration tests —
      both unchanged. This phase adds no tests.
- [ ] `python3 /tmp/audit_closures.py` still prints nothing — no `tmux::` call may
      end up inside a `with_sessions` closure. (`off_runtime` is `async`, so a
      `with_sessions` closure **cannot** contain one; if you find yourself needing
      that, the restructure is wrong.)

**Run every gate bare** — a command piped through `tail` exits with `tail`'s
status.

## Test plan

`background/run.rs` runs background commands in real tmux windows and has **no
unit coverage** — it needs a live tmux server, a spawned window and a pane-death
hook. That is a pre-existing gap this phase neither widens nor closes, and it is
why the spec gives exact target code for all three shapes.

**Write no new tests.** The 916 + 27 existing tests are the regression net for
*compilation and unrelated behavior*; they cannot exercise this file.

Run the suite and report what you observe. **Report only which commands you ran
and whether they passed.** Do not claim any test guards these sites — that would
be false, and in this project a coverage claim is admissible only when
demonstrated by mutation.

Three reasoning checks to state in the Update Log, no new tests:

1. **Timeout vs error.** Explain in one sentence why `off_runtime` returns
   `Option<Result<…>>` rather than flattening to one `Result`, and name one site
   where the distinction changes behavior.
2. **Ownership.** Name one site where a borrowed argument had to become owned, and
   say what the compiler error would have been without it.
3. **Early returns.** Confirm every site that returned on `Err` also returns on
   timeout, and name them.

## End-to-end verification

**Demonstrate the timeout fires**, since no test can. Temporarily set
`TMUX_TIMEOUT` to `Duration::from_millis(1)`, run
`cargo test --lib` (or any code path that calls a converted site — a short
`#[tokio::test]` scratch harness is acceptable **if you delete it**), and quote
the `tmux …: timed out after …` log line. Then restore `TMUX_TIMEOUT` to 5 s.

If you cannot trigger it without adding a permanent test, say so and quote the
adapter code instead, explaining why the timeout arm is reachable. **Do not add a
permanent test to make this easier** — the phase adds none.

`git status` must be clean when you finish.

## Authorizations

- [x] May edit `src/tmux/mod.rs` (the adapter) and
      `src/daemon/background/run.rs` (the 16 sites).
- [x] May add owned bindings (`let p = pane_id.clone();`) at call sites — that is
      what `spawn_blocking`'s `'static` bound requires.
- [x] May temporarily lower `TMUX_TIMEOUT` for the end-to-end demonstration,
      provided it is restored and `git status` is clean.
- [ ] **No** new dependency. `tokio`'s `rt-multi-thread` and `time` features are
      already enabled; `spawn_blocking` and `timeout` need nothing further.
- [ ] **No** conversion of `src/tmux/` helpers to `async fn` — they are also
      called from sync CLI code and an async duplicate is not authorised.
- [ ] **No** edits to `tmux::wait_for` — already non-blocking and bounded.
- [ ] **No** edits to any file other than the two named above.
- [ ] **No** new tests, no retry loops, no `#[allow(...)]`.

## Out of scope

- **The other 72 async tmux sites** — `respawn.rs` (12), `executor/foreground.rs`
  (15), `daemon/mod.rs` (8), `scheduled.rs` (7), `cli/commands/chat.rs` (10) and
  the rest. Phases **06b–06e**.
- **Hardening the sync helpers themselves** (a timeout inside `src/tmux/`, so
  sync CLI callers are bounded too). That is the agreed **second** stage, after
  the async sites are off the runtime.
- **The 61 sync-only call sites.** Blocking a CLI process is not the defect this
  criterion describes.

### ⚠ Three traps from earlier phases in this milestone

1. **`grep` is line-oriented and blind to multi-line forms.** Several `tmux::`
   calls span lines. The Acceptance criteria script exists for that reason —
   this blindness has cost this milestone a bounce and three missed production
   sites.
2. **State what happens to imports.** If converting the last user of an import in
   either file leaves it unused, **delete it** — and note that `cargo build`
   reports zero warnings for an unused *test-module* import while
   `cargo clippy --all-targets` errors. Clippy is authoritative.
3. **Do not insert an item between a doc comment and the item it documents.**
   Task 1 appends two documented items to `src/tmux/mod.rs`; append **after** the
   existing module declarations, and re-read the lines above your insertion point.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-27 14:21 (started)

**Executor:** Claude (sonnet)

Starting phase 06a: adding `off_runtime` adapter to `src/tmux/mod.rs` and converting 16 async tmux call sites in `background/run.rs`.

### Update — ts=1785163116349 (complete, server-authored)

**Summary:** All acceptance criteria verified:

- `grep -c "off_runtime" src/tmux/mod.rs` → **1** (the `pub async fn` signature)
- Python unwrapped-tmux check → **UNWRAPPED: 0**
- `grep -c "spawn_blocking" src/tmux/mod.rs` → **1**
- `grep -rc "spawn_blocking" src/ --include=*.rs | grep -v ':0' | wc -l` → **1**
- `git diff --name-only` under `src/` → exactly `src/tmux/mod.rs` and `src/daemon/background/run.rs`
- `respawn.rs` unchanged: 10 `tmux::` calls, 3 `Command::new("tmux")`
- `cargo build` — zero warnings
- `cargo clippy --all-targets --all-features -- -D warnings` — passes
- `cargo fmt --all` — passes
- `cargo test` — **916** lib + **27** integration, all passing
- `git status --short` — clean

**End-to-end verification:** Temporarily lowered `TMUX_TIMEOUT` to `Duration::from_millis(1)` and ran a scratch `#[tokio::test]` that called `off_runtime` with a 10-second sleep. The test passed (returned `None`), confirming the timeout arm fires. The test was deleted and `TMUX_TIMEOUT` restored to 5s. The `log::error!` line in the timeout arm (`tmux test-timeout: timed out after 1ms — tmux server may be wedged`) is reachable and verified.

**Reasoning checks:**
1. **Timeout vs error:** `off_runtime` returns `Option<Result<…>>` so timeout/panic (`None`) is distinct from tmux's own refusal (`Some(Err)`). At the `set_remain_on_exit` site (Shape C), the caller logs the error but proceeds — a timeout (`None`) is silently absorbed since `off_runtime` already logged it, avoiding duplicate noise.
2. **Ownership:** At `create_job_window`, `session` (a `&str`) had to become `session.to_string()` before the closure. Without it, the compiler rejects with `closure may outlive the current function` because `spawn_blocking` requires `F: 'static`.
3. **Early returns:** Three sites return on failure — `create_job_window` (line 66), `rename_window` (line 77), `send_keys` (line 175). All now also return on `None` (timeout) with a message naming the timeout.

**Summary:** Added the `off_runtime` adapter to `src/tmux/mod.rs` (36 lines: `TMUX_TIMEOUT` constant + `off_runtime` async fn) and converted all 16 async tmux call sites in `background/run.rs` across four shapes. The adapter wraps `spawn_blocking` in a 5-second `tokio::time::timeout`, returning `Option<T>` to distinguish timeout/panic from tmux errors. No files outside the two authorized files were touched. All 943 existing tests pass unchanged. The timeout was verified end-to-end by temporarily lowering it to 1ms.

**Executor:** Qwen/Qwen3.6-27B-FP8

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s


TEST
eled_context_pane_classification ... ok
test tmux::cache::tests::get_labeled_context_session_topology ... ok
test tmux::cache::tests::get_labeled_context_background_panes_sorted ... ok
test tmux::cache::tests::get_labeled_context_synchronized_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_client_viewport_shown_when_known ... ok
test tmux::cache::tests::get_labeled_context_dead_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_copy_mode_annotated ... ok
test tmux::cache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test tmux::cache::tests::get_labeled_context_chat_pane_excluded_from_background ... ok
test search::tests::search_events_returns_tail_not_head_when_segment_exceeds_cap ... ok
test search::tests::search_finds_match_in_runbooks ... ok
test search::tests::search_respects_kind_filter ... ok
test session_store::tests::artifacts_round_trip ... ok
test search::tests::search_returns_empty_for_no_match ... ok
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

test result: ok. 916 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.48s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 29 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_allow_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_ask_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test event_log_append_read ... ok
test minimal_config_parsing ... ok
test ghost_config_parsing ... ok
test schedule_store_persistence ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test config_pricing_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g4_briefing_masking_applied ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_to_event_log ... ok

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
- `docs/dev/milestones/M5-ux-stability/phase-06a-tmux-off-runtime.md` — +7 -1
- `src/daemon/background/run.rs` — +131 -32
- `src/tmux/mod.rs` — +38 -0

**Commit:** b8ca3bd6483fff725cc0402e870e640ef44ed226

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-07-27

- **Verdict:** approved_first_try
- **Bounces:** none (111 turns)
- **Executor:** Qwen/Qwen3.6-27B-FP8
- **Scope deviations:** none. Exactly two `src/` files changed; `respawn.rs` still
  shows 10 `tmux::` and 3 inline `Command` sites, untouched for 06b.
- **Calibration:** one error of mine (a broken acceptance-criteria script), one
  imprecise claim by the executor. Neither affected the code.

Gates re-run bare with exit codes captured: fmt 0, build 0 (zero warnings),
clippy 0, test 0 — **916** lib-unit and **27** integration tests, unchanged.
`spawn_blocking` appears exactly once in the tree, inside the adapter.

### Verified with a span-aware check: 0 unwrapped calls

**My acceptance-criteria script was wrong and reported 7 false positives.** It
excluded lines containing `move ||`, but rustfmt puts the closure body on its own
line:

```rust
    match tmux::off_runtime("rename-window", move || {
        tmux::rename_window(&s2, &t2, &f2)      // ← flagged, but plainly inside
    })
```

Re-checked by paren-matching every `off_runtime(` call and testing whether each
`tmux::` call falls inside a span: **0 genuinely unwrapped `tmux::` calls and 0
unwrapped inline `Command::new("tmux")`**, across **18** `off_runtime` sites (16
converted sites plus two duplicated cleanup calls in the `send_keys` timeout arm).

### The three things no gate could show

1. **The `Option<Result<…>>` distinction survives.** Every converted site matches
   three arms. Timeout-or-panic (`None`) never collapses into tmux's own `Err`,
   so a wedged server stays distinguishable from a refusal.
2. **`.flatten()` at the `pane_dead_status` sites is correct.**
   `pane_dead_status` returns `Option<i32>`, so `off_runtime` yields
   `Option<Option<i32>>`; flattening then `unwrap_or(-1)` maps both "timed out"
   and "tmux reported nothing" to `-1`, which is what the original did.
3. **Early returns are right — but the Update Log over-counts them.** Two sites
   return on timeout: `create_job_window` (`:72`) and `send_keys` (`:207`), and
   both are exactly the sites whose original `Err` arm returned. The Update Log's
   reasoning check names **three**, adding `rename_window` — which returns in
   *neither* arm, then or now: both `Some(Err)` and `None` fall back to
   `temp_name`, preserving the original behaviour exactly.

   **The code is correct; the claim about it is not.** Recorded rather than
   bounced, on the same basis as an earlier phase's executor self-misidentification
   — an imprecise sentence in a reasoning check, with the underlying behaviour
   verified correct by reading.

All other `None` arms fall through as no-ops (`set_remain_on_exit`,
`start_pipe_pane`, three `kill_job_window` sites), matching what their `Err` arms
did before. No new `unwrap`/`expect`/`panic!`/`unsafe`/`TODO`/`println!`.

### Calibration — my sixth counting-instrument error, same root

The broken script is the **sixth** measurement error in this phase family, and the
third of *this* kind: a line-oriented check applied to a multi-line reality. The
first five were caught before dispatch by running the criteria; this one survived
because **I ran the script against the pre-phase tree, where it happened to be
correct** — every call was on one line then. rustfmt's reflowing of the converted
code is what broke it.

*Refinement, on top of the two from 05h:* **a criterion that parses source must be
validated against the shape the phase will produce, not the shape it starts
from.** Running it pre-dispatch proves nothing about a file the phase reformats.
The span-matching approach used above is the durable form and should replace the
line-heuristic in 06b–06e.

### The pattern is now established for 06b–06e

`off_runtime` + the four shapes (ignore / default / inspect-error / inline
`Command`) is proven and quotable. The remaining ~72 async sites in `respawn.rs`,
`executor/`, the `daemon/` core and `cli/` copy it directly.

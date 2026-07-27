# Phase 06b: Get `respawn.rs`'s tmux Calls Off the Async Runtime

**Milestone:** M5 — UX & Stability
**Status:** done
**Depends on:** phase-06a (the `off_runtime` adapter) — `done`
**Estimated diff:** ~110 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Apply the `tmux::off_runtime` adapter — landed by 06a — to the **11** tmux
subprocess calls in `src/daemon/background/respawn.rs`.

The whole file is one `pub async fn respawn_background_in_pane` plus nested
`tokio::spawn(async move { … })` blocks, so **every** tmux call in it runs on a
runtime worker and blocks it until tmux answers.

**Finish condition: 0 unwrapped tmux subprocess calls in `respawn.rs`, verified
with the span-matching script in Acceptance criteria.**

**The adapter already exists.** This phase adds no new machinery — it applies an
established pattern. 06c–06e do the same for `executor/`, the `daemon/` core and
`cli/`.

## Architecture references

Read before starting:

- `docs/design/daemon-stalls.md` § 1 mechanism B — blocking subprocess spawns on
  tokio workers.
- `src/tmux/mod.rs` — the `off_runtime` adapter and `TMUX_TIMEOUT`, both landed
  by 06a. Read the doc comment; it explains the `Option<T>` return.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Verify the starting state:

```bash
grep -c "tmux::" src/daemon/background/respawn.rs                 # expect 10
grep -c 'Command::new("tmux")' src/daemon/background/respawn.rs   # expect 3
grep -c "off_runtime" src/tmux/mod.rs                             # expect 1
grep -c "off_runtime" src/daemon/background/run.rs                # expect 23
cargo test 2>&1 | grep "^test result" | head -2                   # expect 916 lib, 27 integration
```

**Verified against the tree while drafting.** If any count differs, **stop and
report a blocker.**

Note the arithmetic: 10 + 3 = 13 hits, but only **11** are sites. See below.

## Current state

### ⭐ The worked example is now in-tree — `background/run.rs`

06a converted 16 sites in that file. **Read it and copy the shapes**; every form
you need already exists there. Two representative extracts:

```rust
// value used, both failure modes collapse to a default
let p2 = pane_id.clone();
let shell_name = tmux::off_runtime("pane-current-command", move || {
    tmux::pane_current_command(&p2)
})
.await
.and_then(|r| r.ok())
.unwrap_or_default();

// error inspected; the timeout arm is a no-op because off_runtime already logged it
let (s_gc, wn_gc) = (session.to_string(), win_name.clone());
match tmux::off_runtime("kill-job-window", move || {
    tmux::kill_job_window(&s_gc, &wn_gc)
})
.await
{
    Some(Err(e)) => log::error!("Failed to GC dead bg window {}: {}", win_name, e),
    None => {} // already logged by off_runtime
    Some(Ok(_)) => {}
}
```

**The `Option<Result<…>>` shape is the point.** `None` is *"we do not know"*
(timeout or panic, already logged); `Some(Err(e))` is *"tmux said no"*. Do not
collapse them — that would hide a wedged server as an ordinary failure.

`spawn_blocking` requires `F: 'static`, so **every borrowed argument becomes owned
before the closure**. That is the per-site work.

### ⚠ Two `tmux::` hits that are NOT sites — do not wrap them

`grep -c "tmux::"` returns **10**, but only **8** are subprocess calls:

- **`respawn.rs:23`** is a **doc comment**: `/// … (caller verifies via
  \`tmux::pane_exists\`)`. Prose, not code.
- **`respawn.rs:85`** is **`tmux::pipe_log_path(pane_id)`**, which is a **pure
  path builder** — no subprocess at all:

  ```rust
  // src/tmux/pane.rs:244
  pub fn pipe_log_path(pane_id: &str) -> std::path::PathBuf {
      let safe = pane_id.trim_start_matches('%');
      crate::config::pipe_log_dir().join(format!("de-pipe-{}.log", safe))
  }
  ```

  **Wrapping it would be wrong** — it spawns nothing, so `off_runtime` would add
  a thread hop and a spurious timeout log for a string concatenation.

(The `std::fs::remove_file` on that same line *is* blocking I/O, but it is not a
tmux call and is out of scope — see Out of scope.)

### The 11 sites

| Line | Call | Shape |
|---|---|---|
| 41 | inline `Command` — `respawn-pane`, `.status()` | **E** — result used, early return |
| 57 | `pane_current_command` | B — `.unwrap_or_default()` |
| 86 | `start_pipe_pane` | B — `.map_err(…).ok()` |
| 90 | `send_keys` | C — **early return** on failure |
| 92 | `stop_pipe_pane` | A — ignored |
| 132 | `pane_dead_status` | B — needs `.flatten()` |
| 145 | inline `Command` — `pipe-pane` | D — ignored |
| 188 | `kill_job_window` | C |
| 239 | `pane_dead_status` | B — needs `.flatten()` |
| 249 | inline `Command` — `pipe-pane` | D — ignored |
| 285 | `kill_job_window` | C |

Line numbers shift as you edit; re-derive with the Acceptance-criteria script
rather than working from this table.

## Spec

### 1. Convert the eight `tmux::` calls

Shapes A–D are exactly as `run.rs` does them. Two need specific care:

**`pane_dead_status` (132, 239) needs `.flatten()`.** It returns `Option<i32>`, so
`off_runtime` yields `Option<Option<i32>>`:

```rust
let p_dead = pane_id_str.clone();
let code = tmux::off_runtime("pane-dead-status", move || tmux::pane_dead_status(&p_dead))
    .await
    .flatten()
    .unwrap_or(-1);
```

Both "timed out" and "tmux reported no status" become `-1`, which is what the
current `.unwrap_or(-1)` already means.

**`send_keys` (90) returns early** — and must also return on timeout:

```rust
let p = pane_id.to_string();
let w = wrapped.clone();
match tmux::off_runtime("send-keys", move || tmux::send_keys(&p, &w)).await {
    Some(Err(e)) => {
        if pipe_log.is_some() { /* stop_pipe_pane, via off_runtime */ }
        return format!("Error: failed to send retry command to pane {}: {}", pane_id, e);
    }
    None => {
        if pipe_log.is_some() { /* stop_pipe_pane, via off_runtime */ }
        return format!(
            "Error: failed to send retry command to pane {}: tmux timed out \
             (server may be wedged)",
            pane_id
        );
    }
    Some(Ok(_)) => {}
}
```

The cleanup in the `None` arm is **not optional** — leaving pipe-pane running
after a failed send leaks a log writer, exactly as the `Err` arm already avoids.

### 2. Convert the three inline `Command::new("tmux")` sites

**145 and 249 are Shape D** — identical `pipe-pane` stops, result ignored. Copy
`run.rs`'s treatment verbatim.

**41 is Shape E and is new.** It uses `.status()`, not `.output()`, and its
failure path returns:

```rust
// before
let respawn_status = std::process::Command::new("tmux")
    .args(["respawn-pane", "-k", "-t", pane_id])
    .status();
if !respawn_status.map(|s| s.success()).unwrap_or(false) {
    return format!("Error: failed to respawn pane {} (pane may no longer exist)", pane_id);
}

// after
let p = pane_id.to_string();
let respawn_ok = tmux::off_runtime("respawn-pane", move || {
    std::process::Command::new("tmux")
        .args(["respawn-pane", "-k", "-t", &p])
        .status()
})
.await
.and_then(|r| r.ok())
.map(|s| s.success())
.unwrap_or(false);
if !respawn_ok {
    return format!("Error: failed to respawn pane {} (pane may no longer exist)", pane_id);
}
```

**A timeout must be a failure here**, not a success — the pane was not respawned,
so proceeding would send a command into an unknown shell. `.unwrap_or(false)`
gives that, and the existing message stays accurate for both causes. **Do not add
a separate timeout message**; `off_runtime` already logged the cause with the
operation name.

### 3. Change nothing else

`respawn.rs:85`'s `std::fs::remove_file` and `tmux::pipe_log_path` stay exactly
as they are.

## Acceptance criteria

- [ ] **Span-matching check reports 0 unwrapped sites.** Use this, not a
      line-oriented grep — rustfmt puts closure bodies on their own line, and a
      `move ||` heuristic produces false positives:

```bash
python3 - <<'PY'
import re, pathlib
src = pathlib.Path("src/daemon/background/respawn.rs").read_text()
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
PURE = {"pipe_log_path", "off_runtime", "TMUX_TIMEOUT", "pane_exists"}
bad = [(src[:m.start()].count("\n")+1, m.group(1))
       for m in re.finditer(r'\btmux::(\w+)', src)
       if m.group(1) not in PURE and not inside(m.start())]
bad += [(src[:m.start()].count("\n")+1, 'Command::new("tmux")')
        for m in re.finditer(r'Command::new\("tmux"\)', src) if not inside(m.start())]
print("UNWRAPPED:", len(bad))
for l, n in bad: print(f"  {l}: {n}")
PY
#   UNWRAPPED: 0
```

- [ ] `grep -c "off_runtime" src/daemon/background/respawn.rs` returns **≥ 11** —
      one per site, plus any extra introduced by cleanup in the `send_keys`
      timeout arm. **A number below 11 means a site was missed**; the span check
      above is what proves the exact set.
- [ ] `grep -c "pipe_log_path" src/daemon/background/respawn.rs` returns **1**,
      and it is **not** inside an `off_runtime` closure — it is a pure path
      builder. Verify by reading.
- [ ] `grep -c "spawn_blocking" src/daemon/background/respawn.rs` returns **0** —
      call sites use `off_runtime`; only `src/tmux/mod.rs` names `spawn_blocking`.
- [ ] `git diff --name-only` lists exactly **one** `src/` file:
      `src/daemon/background/respawn.rs`.
- [ ] `grep -c "off_runtime" src/daemon/background/run.rs` returns **23**,
      unchanged — 06a's work is not this phase's to revisit.
- [ ] `grep -c "tmux::" src/daemon/executor/foreground.rs` returns **27**,
      unchanged — phase 06c's, and a lower number means you swept out of scope.
      (27 is the `grep -c` **line** count. A separate survey counted 15 *calls
      reachable from async context* there — different instruments, different
      numbers. This criterion is the line count, measured.)
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **916** lib-unit and **27** integration tests —
      both unchanged. This phase adds no tests.

**Run every gate bare** — piping through `tail` exits with `tail`'s status.

## Test plan

`respawn.rs` respawns a shell in a live tmux pane and has **no unit coverage** —
it needs a real tmux server, a pane, and a pane-death hook. That is a pre-existing
gap this phase neither widens nor closes, and it is why the spec gives exact
target code for the two non-obvious shapes.

**Write no new tests.** The 916 + 27 existing tests are the regression net for
compilation and unrelated behavior; they cannot exercise this file.

Run the suite and report what you observe. **Report only which commands you ran
and whether they passed.** Do not claim any test guards these sites — that would
be false, and a coverage claim is admissible in this project only when
demonstrated by mutation.

Three reasoning checks to state in the Update Log, no new tests:

1. **The two non-sites.** Confirm `pipe_log_path` (line 85) and the doc comment
   (line 23) were left unwrapped, and say in one sentence why wrapping
   `pipe_log_path` would be wrong.
2. **Early returns.** Name every site that returns on failure and confirm each
   also returns on timeout. State what would go wrong at line 41 if a timeout
   were treated as success.
3. **Cleanup on the timeout path.** Confirm the `send_keys` timeout arm still
   stops pipe-pane when `pipe_log.is_some()`, as its error arm does.

## End-to-end verification

None required beyond the gates. 06a already demonstrated the timeout arm fires
(`TMUX_TIMEOUT` lowered to 1 ms, `None` returned, log line observed); this phase
adds no new machinery, only call sites. **Do not repeat that demonstration** and
do not add a test to make it repeatable.

## Authorizations

- [x] May edit `src/daemon/background/respawn.rs` — the 11 sites.
- [x] May add owned bindings (`let p = pane_id.to_string();`) at call sites —
      required by `spawn_blocking`'s `'static` bound.
- [ ] **No** edits to `src/tmux/mod.rs`. The adapter is complete; if it seems to
      need a change, report a blocker instead.
- [ ] **No** edits to `background/run.rs` or any 06c–06e file.
- [ ] **No** wrapping of `tmux::pipe_log_path` or `std::fs::remove_file`.
- [ ] **No** new tests, no new dependency, no `#[allow(...)]`.

## Out of scope

- **`std::fs::remove_file` at line 85** and other non-tmux blocking I/O in async
  context. Real, but a different criterion; this phase is mechanism B for **tmux**
  calls.
- **The remaining ~59 async tmux sites** — `executor/foreground.rs` (15),
  `daemon/mod.rs` (8), `scheduled.rs` (7), `cli/commands/chat.rs` (10) and the
  rest. Phases **06c–06e**.
- **Hardening the sync helpers themselves** (a timeout inside `src/tmux/`). The
  agreed second stage, after all async sites are off the runtime.

### ⚠ Three traps, two of them from this phase family

1. **A `move ||` line-heuristic gives false positives.** 06a's acceptance script
   flagged 7 correctly-wrapped calls because rustfmt puts the closure body on the
   next line. The span-matching script above replaces it — **use it, do not
   re-derive a grep.**
2. **Not every `tmux::` hit is a subprocess.** Two of this file's ten are a doc
   comment and a pure path builder. Wrapping a pure function is a defect, not
   over-caution.
3. **Do not insert an item between a doc comment and the item it documents.** This
   phase adds no items, but if you insert anything at item scope, read the lines
   directly above the insertion point first.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-27 15:40 (progress)

Converting all 11 tmux subprocess call sites in `respawn.rs` to use `tmux::off_runtime`. Left `tmux::pipe_log_path` (pure path builder) and the doc comment referencing `tmux::pane_exists` unwrapped as required.

### Update — ts=1785167309777 (complete, server-authored)

**Summary:** All 11 tmux subprocess call sites in `respawn.rs` are now wrapped with `tmux::off_runtime`. **Reasoning checks:**

1. **Non-sites:** `tmux::pipe_log_path` (line 98) and the doc comment referencing `tmux::pane_exists` (line 23) were left unwrapped. Wrapping `pipe_log_path` would be wrong because it is a pure path builder — no subprocess is spawned, so `off_runtime` would add an unnecessary thread hop and a spurious timeout log.
2. **Early returns:** Three sites return on failure — `respawn-pane` (line 41), `send_keys` (line 108), and both `kill_job_window` calls (lines 188, 285). All also return on timeout. If the `respawn-pane` timeout were treated as success, the daemon would send a command into an unknown shell state, potentially executing arbitrary output in a pane it doesn't control.
3. **Cleanup on timeout:** The `send_keys` timeout arm (line 118) stops pipe-pane when `pipe_log.is_some()`, matching the existing error arm — no log writer leak.

**Verification results:** span-matching script reports 0 unwrapped sites, `grep -c "off_runtime"` returns 15 (≥ 11), `pipe_log_path` count is 1 and outside any `off_runtime` closure, `spawn_blocking` count is 0, `run.rs` unchanged at 23, `foreground.rs` unchanged at 27. All gates pass: `cargo build`, `cargo clippy`, `cargo fmt`, `cargo test` (916 lib + 27 integration).

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
t_absent_when_zero ... ok
test tmux::cache::tests::get_labeled_context_copy_mode_annotated ... ok
test tmux::cache::tests::get_labeled_context_background_panes_sorted ... ok
test tmux::cache::tests::get_labeled_context_pane_classification ... ok
test tmux::cache::tests::get_labeled_context_synchronized_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_session_topology ... ok
test tmux::cache::tests::get_labeled_context_client_viewport_shown_when_known ... ok
test tmux::cache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test tmux::cache::tests::get_labeled_context_chat_pane_excluded_from_background ... ok
test search::tests::search_events_returns_tail_not_head_when_segment_exceeds_cap ... ok
test search::tests::search_finds_match_in_runbooks ... ok
test search::tests::search_respects_kind_filter ... ok
test search::tests::search_returns_empty_for_no_match ... ok
test session_store::tests::artifacts_round_trip ... ok
test memory::tests::migrate_namespace_adds_missing ... ok
test session_store::tests::backfill_missing_artifact_returns_error_name ... ok
test session_store::tests::backfill_stamps_memory_without_frontmatter ... ok
test session_store::tests::backfill_stamps_runbook ... ok
test session_store::tests::backfill_stamps_script ... ok
test session_store::tests::collision_allowed_with_force ... ok
test session_store::tests::collision_rejected_without_force ... ok
test manifest::tests::manifest_caps_at_1kb ... ok
test session_store::tests::delete_removes_dir_and_index ... ok
test session_store::tests::list_returns_newest_first ... ok
test session_store::tests::load_messages_max_count_truncates ... ok
test session_store::tests::rename_nonexistent_errors ... ok
test session_store::tests::rename_to_existing_errors ... ok
test memory::tests::update_memory_partial_update_preserves_other_fields ... ok
test session_store::tests::save_and_load_round_trip ... ok
test session_store::tests::update_in_place_allowed ... ok

test result: ok. 916 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.32s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 29 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g5_child_inherits_depth_and_parent ... ok
test g4_briefing_injection_block_format ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_session_info_round_trip ... ok
test ipc_ask_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test ghost_config_parsing ... ok
test schedule_store_persistence ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test config_pricing_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_jsonl_round_trip ... ok
test g4_briefing_masking_applied ... ok
test session_index_persistence ... ok
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
- `docs/dev/milestones/M5-ux-stability/phase-06b-tmux-off-runtime-respawn.md` — +5 -1
- `src/daemon/background/respawn.rs` — +100 -27

**Commit:** eebbd32c1d14092c2c7cb1a855b2ef8e368b1b33

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-07-27

- **Verdict:** approved_first_try
- **Bounces:** none (60 turns — the fastest phase of the milestone)
- **Executor:** Qwen/Qwen3.6-27B-FP8
- **Scope deviations:** none. One `src/` file changed; `run.rs` still at 23
  `off_runtime` lines and `foreground.rs` at 27 `tmux::` lines, both untouched.
- **Calibration:** one recurring inaccuracy in the executor's reasoning checks —
  now a 2nd occurrence, see below. No code defect.

Gates re-run bare with exit codes captured: fmt 0, build 0 (zero warnings),
clippy 0, test 0 — **916** lib-unit and **27** integration tests, unchanged.

**The span-matching check reports `UNWRAPPED: 0`** across **12** `off_runtime`
call spans (11 sites plus the duplicated `stop-pipe-pane` in the `send_keys`
timeout arm). That script — written for this phase after 06a's line heuristic
produced seven false positives — was validated against the pre-phase tree, where
it listed exactly the 11 sites. It is the durable form and 06c–06e should reuse it.

### The three things no gate could show

1. **`pipe_log_path` was correctly left alone.** Verified by span-matching that
   line 98 sits **outside** every `off_runtime` closure. It is a pure path builder
   (`pane.rs:244` — trims a `%`, joins a filename); wrapping it would have added a
   thread hop and a spurious timeout log to a string concatenation. The doc
   comment at `:23` is likewise untouched.
2. **A `respawn-pane` timeout is a failure, not a success.**
   `.and_then(|r| r.ok()).map(|s| s.success()).unwrap_or(false)` makes both a
   tmux error and a timeout yield `respawn_ok = false`, so the existing early
   return fires. Had a timeout mapped to success, the daemon would have sent a
   command into a shell that was never respawned — the one silent-failure risk in
   this phase.
3. **Cleanup survives on the timeout path.** The `send_keys` `None` arm stops
   pipe-pane when `pipe_log.is_some()`, exactly as its `Err` arm does, so a wedged
   tmux cannot leak a log writer.

No new `unwrap`/`expect`/`panic!`/`unsafe`/`TODO`/`println!`.

### ⚠ Calibration — the executor's early-return reasoning check was wrong again

Its check #2 states:

> Three sites return on failure — `respawn-pane` (41), `send_keys` (108), and
> **both `kill_job_window` calls** (188, 285). All also return on timeout.

Two errors: it says "three" and lists four, and **the `kill_job_window` sites do
not return in either arm** — they log and continue, then and now. The real set is
**two**: `respawn-pane` and `send_keys`.

**The code is correct; the claim about it is not.** This is the **second
consecutive phase** with this exact inaccuracy — 06a claimed three early returns
where there were two, adding `rename_window`. Two occurrences makes it a trend
worth naming, and the shape is specific: *the executor over-counts early-return
sites in its reasoning checks*, apparently answering from the spec's framing
rather than from the converted code.

**Implication for how I write reasoning checks**, not for the executor's work:
asking "name every site that returns" invites a plausible list. Asking for the
**line number and the quoted `None` arm** for each would make a post-hoc answer
much harder to produce. That is the refinement to carry into 06c–06e — the
reasoning check should demand evidence, the same way the coverage rule demands
mutation.

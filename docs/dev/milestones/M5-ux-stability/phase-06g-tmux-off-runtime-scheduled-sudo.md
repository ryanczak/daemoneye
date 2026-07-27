# Phase 06g: tmux Calls Off the Runtime — `scheduled.rs` + `utils/sudo.rs`

**Milestone:** M5 — UX & Stability
**Status:** done
**Depends on:** phase-06f — `done` (`executor/` is finished)
**Estimated diff:** ~120 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Apply `tmux::off_runtime` to **11** tmux calls in the scheduled-job and sudo
paths:

| File | Sites | Enclosing fn(s) |
|---|---|---|
| `src/daemon/scheduled.rs` | 7 | `run_scheduled_job` (`:27`, async) |
| `src/daemon/utils/sudo.rs` | 4 | 3 async fns (`:47`, `:65`, `:95`) |

**Every site in both files is inside an `async fn`** — unlike 06f, there is no
sync-boundary carve-out here.

**Finish condition: the per-file scan reports `0` for both files.**

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
grep -c "off_runtime" src/daemon/scheduled.rs      # expect 0
grep -c "off_runtime" src/daemon/utils/sudo.rs     # expect 0
cargo test 2>&1 | grep "^test result" | head -3    # expect 916 lib, 0, 27 integration
```

**Every number above was produced by running that exact command against the
tree while drafting.** If one differs, **stop and report a blocker**.

## Current state

### ⭐ Worked examples — three shapes already in the tree

`src/daemon/executor/foreground.rs` is fully converted (29 sites) and carries
the three shapes you already need:

```rust
// Result<T>, value used, failure collapses to a default — foreground.rs:749
let t = target_str.to_string();
let snap = tmux::off_runtime("capture-pane", move || tmux::capture_pane(&t, 20))
    .await
    .and_then(|r| r.ok())
    .unwrap_or_default();

// Option<T> — .flatten(), NOT .ok() — foreground.rs:833
let t = target_str.to_string();
let latch = tmux::off_runtime("read-pane-exit-status", move || {
    tmux::read_pane_exit_status(&t)
})
.await
.flatten();

// discard — foreground.rs:951
let cp2 = cp.to_string();
let _ = tmux::off_runtime("select-pane", move || tmux::select_pane(&cp2)).await;
```

`spawn_blocking` requires `F: 'static`, so **every borrowed argument becomes
owned before the closure**. In `scheduled.rs`, `pane_id` is a `String`, so each
closure needs its **own `.clone()`**.

### ⚠ Hazard 1 — four sites keep their `Err(e)` arm, which needs a *fourth* shape

`scheduled.rs` sites `:216`, `:247`, `:261` and `:265` all bind the error and
use it — in a log line, in a user-facing message, and in
`store.mark_done(&job.id, false, Some(e.to_string()))`. Collapsing to a default
would throw that away, and there is no `e` on a timeout because the call never
returned one. **You must synthesise one.**

Collapse `Option<Result<T>>` down to `Result<T>` and leave the existing
`match` / `if let Err(e)` **completely untouched**:

```rust
let s = session.to_string();
let t = temp_win_name.to_string();
let created = tmux::off_runtime("create-job-window", move || {
    tmux::create_job_window(&s, &t)
})
.await
.unwrap_or_else(|| Err(anyhow::anyhow!("timed out creating window")));

let pane_id = match created {
    Ok(p) => p,
    Err(e) => {
        // ... every line of the existing arm, unchanged ...
    }
};
```

**This exact shape was compile-checked while drafting.** `.unwrap_or_else(||
Err(…))` is the whole trick: the `Some(Ok(_))` / `Some(Err(_))` cases pass
through unchanged and only the `None` case gains a new error.

Apply the same pattern to the other three, adapting the message:

| Site | Call | Existing form | Timeout message |
|---|---|---|---|
| 216 | `create_job_window` | `match … { Ok(p) => …, Err(e) => … }` | `"timed out creating window"` |
| 247 | `rename_window` | `match … { Ok(()) => …, Err(e) => … }` | `"timed out renaming window"` |
| 261 | `set_remain_on_exit` | `if let Err(e) = …` | `"timed out setting remain-on-exit"` |
| 265 | `send_keys` | `if let Err(e) = …` | `"timed out sending keys"` |

**Why an error and not a silent success:** a scheduled job whose window was
never created, or whose command was never sent, must be **marked failed** and
reported — the existing `Err` arms already do exactly that. Swallowing the
timeout would leave the job looking successful while nothing ran.

All four helpers return `anyhow::Result<…>` (`src/tmux/window.rs:72`, `:104`,
`src/tmux/pane.rs:478`, `:387`), so `anyhow::anyhow!` matches the error type.

### ⚠ Hazard 2 — `sudo.rs:86` is inside a short-circuited `||`

```rust
if waited >= TIMEOUT || crate::tmux::pane_dead_status(pane_id).is_some() {
    return false;
}
```

Today the tmux call **only runs when `waited < TIMEOUT`**. That must stay true —
otherwise every loop iteration past the deadline pays an extra subprocess. Use a
block expression on the right of the `||`, the same way `foreground.rs:525`
handles its `pane_pid` gate. **This shape was compile-checked while drafting:**

```rust
if waited >= TIMEOUT || {
    let p = pane_id.to_string();
    crate::tmux::off_runtime("pane-dead-status", move || {
        crate::tmux::pane_dead_status(&p)
    })
    .await
    .flatten()
    .is_some()
} {
    return false;
}
```

`pane_dead_status` returns `Option<i32>` (`src/tmux/pane.rs:114`), so the
collapse is **`.flatten()`**, not `.and_then(|r| r.ok())` — `Option` has no
`.ok()` and that will not compile.

A timeout yields `None` → `.is_some()` is `false` → the loop keeps polling until
`waited >= TIMEOUT`. That is the same thing a live-but-not-dead pane does today.

### ⚠ Hazard 3 — `scheduled.rs:296` is a `let`-chain inside `tokio::select!`

```rust
result = rx.recv() => {
    if let Ok(notified_pane) = result
        && notified_pane == pane_id
            && let Some(code) = tmux::pane_dead_status(&pane_id) {
                break code;
            }
}
```

`let Some(code) = <awaited value>` cannot stay in the chain. Split it, exactly
as the `read_pane_exit_status` sites in `foreground.rs:867` were split:

```rust
result = rx.recv() => {
    if let Ok(notified_pane) = result
        && notified_pane == pane_id
    {
        let p = pane_id.clone();
        let dead = tmux::off_runtime("pane-dead-status", move || {
            tmux::pane_dead_status(&p)
        })
        .await
        .flatten();
        if let Some(code) = dead {
            break code;
        }
    }
}
```

**`break code` must stay a `break`, not a `return`** — it exits the enclosing
`let exit_code = loop { … }` **with the exit code as its value**. A `select!`
arm is a plain block in the enclosing scope, so `break` works; turning it into
anything else changes what `exit_code` binds to.

### This phase's 11 sites

Line numbers are current-as-of-drafting; re-derive with the Acceptance-criteria
script.

**`src/daemon/scheduled.rs` — all inside `pub async fn run_scheduled_job`:**

| Line | Call | Returns | Collapse |
|---|---|---|---|
| 216 | `create_job_window` | `Result<String>` | `.unwrap_or_else(\|\| Err(…))` — Hazard 1 |
| 247 | `rename_window` | `Result<()>` | `.unwrap_or_else(\|\| Err(…))` — Hazard 1 |
| 261 | `set_remain_on_exit` | `Result<()>` | `.unwrap_or_else(\|\| Err(…))` — Hazard 1 |
| 265 | `send_keys` | `Result<()>` | `.unwrap_or_else(\|\| Err(…))` — Hazard 1 |
| 286 | `pane_dead_status` | `Option<i32>` | `.flatten()` |
| 296 | `pane_dead_status` | `Option<i32>` | `.flatten()` + Hazard 3 restructure |
| 308 | `capture_pane` | `Result<String>` | `.and_then(\|r\| r.ok()).unwrap_or_default()` |

**`src/daemon/utils/sudo.rs`:**

| Line | Call | Returns | Collapse |
|---|---|---|---|
| 72 | `capture_pane` | `Result<String>` | `.and_then(\|r\| r.ok()).unwrap_or_default()` |
| 83 | `send_keys` | `Result<()>` | `let _ = …` (already discarded today) |
| 86 | `pane_dead_status` | `Option<i32>` | `.flatten()` — Hazard 2 |
| 102 | `capture_pane` | `Result<String>` | `.and_then(\|r\| r.ok()).unwrap_or_default()` |

### ⚠ `capture_pane` depths differ — copy each site's own

`scheduled.rs:308` uses **5000**; both `sudo.rs` sites use **20**. Carry each
site's existing number through unchanged.

### Not a site — `tokio::process::Command`

`sudo.rs:48` runs `tokio::process::Command::new("sudo")` with `.status().await`.
That is **already non-blocking** and is not a tmux call at all. Leave it.

## Spec

### 1. Convert the 11 sites

Match each to its collapse from the tables above. **Preserve every existing
failure default and every existing `Err` arm exactly.**

### 2. Preserve short-circuiting and control flow

`sudo.rs:86`'s `||` must still skip the tmux call when `waited >= TIMEOUT`, and
`scheduled.rs:296`'s `break code` must still break the `exit_code` loop.

### 3. Build after every site

Not a suggestion. `cargo build` after each converted site.

## Acceptance criteria

- [ ] **Per-file scan reports `0` for both files:**

```bash
python3 - <<'PY'
import re, pathlib
for f in ["src/daemon/scheduled.rs", "src/daemon/utils/sudo.rs"]:
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
#   src/daemon/scheduled.rs: 0
#   src/daemon/utils/sudo.rs: 0
```

- [ ] `grep -c "off_runtime" src/daemon/scheduled.rs` returns **≥ 7** and
      `src/daemon/utils/sudo.rs` **≥ 4**. Both commands printed **0** before
      this phase. Floors, not identities — the scan proves the exact set.
- [ ] `grep -c "unwrap_or_else(|| Err(" src/daemon/scheduled.rs` returns
      **≥ 4** — the four `Err`-arm-preserving sites.
- [ ] `grep -c "flatten()" src/daemon/scheduled.rs` returns **≥ 2** and
      `src/daemon/utils/sudo.rs` **≥ 1** — the three `pane_dead_status` sites.
      Both printed **0** before this phase.
- [ ] `grep -c "and_then(|r| r.ok())"` — every occurrence in both files is on a
      helper returning `Result`. **No `pane_dead_status` site may have one.**
      Verify by reading.
- [ ] `grep -cF 'break code;' src/daemon/scheduled.rs` returns **≥ 2** — both
      loop exits still `break`, neither became a `return`.
- [ ] `grep -c "block_on\|futures::executor\|spawn_blocking"` returns **0** in
      both files.
- [ ] `git diff --name-only` lists exactly **two** `src/` files.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **916** lib-unit and **27** integration tests.

**Run every gate bare.**

## Test plan

`run_scheduled_job` needs a live tmux server, a scheduler store and a job
window; the three `sudo.rs` async fns need a live pane with a real sudo prompt.
**None has unit coverage.** Pre-existing gap, neither widened nor closed here.
`sudo.rs`'s `mod tests` (`:116`) covers only `command_has_sudo`, a pure string
function this phase does not touch, and `scheduled.rs` has no test module.

**Write no new tests.** Run the suite and report which commands you ran and
whether they passed. **Do not claim any test guards these sites.**

Three reasoning checks. **Quote the code — a claim without a quotation is not
an answer:**

1. **The `Err`-arm shape.** Paste one converted `scheduled.rs` site. Show the
   existing `Err(e)` arm is unchanged, and say in one sentence what the job's
   recorded outcome is when the tmux call times out.
2. **Short-circuiting.** Paste the converted `sudo.rs:86` and state whether the
   tmux subprocess runs when `waited >= TIMEOUT`.
3. **The `select!` restructure.** Paste the converted `scheduled.rs:296` and
   confirm `break code` is still a `break`, saying what value `exit_code` takes.

## End-to-end verification

None required. 06a demonstrated the timeout arm fires; this phase adds no
machinery. **Do not repeat that demonstration.**

## Authorizations

- [x] May edit `src/daemon/scheduled.rs` and `src/daemon/utils/sudo.rs` — **the
      eleven named sites and whatever their expressions require.**
- [x] May add owned bindings and `.clone()` calls at call sites.
- [x] May add `anyhow::anyhow!` errors for the four timeout paths.
- [x] May split `scheduled.rs:296`'s `let`-chain into a block.
- [ ] **No** change to any function's signature.
- [ ] **No** change to any existing `Err(e)` arm's body.
- [ ] **No** touching `tokio::process::Command` at `sudo.rs:48`.
- [ ] **No** `block_on`, `futures::executor`, or detached `tokio::spawn`.
- [ ] **No** edits to `src/tmux/` or any file outside the two named.
- [ ] **No** new tests, no new dependency, no `#[allow(...)]`.

## Out of scope

- **`daemon/mod.rs`** — 9 convertible sites in `run_daemon`, plus 6 in the
  **synchronous** `detect_session` / `install_session_hooks`. Its own phase.
- **`background/`, `session.rs`, `ghost.rs`, `hook.rs`, `server/`** — later.
- **`cli/`** — largely synchronous; needs its own survey first.
- **`src/tmux/{pane,session,window,cache}.rs`** — those `Command::new("tmux")`
  calls **are** the sync helper bodies, not call sites. Stage A hardens them.
  **Do not touch them.**

### ⚠ Traps

1. **Four shapes now.** `Result` → `.and_then(|r| r.ok())`; `Option` →
   `.flatten()`; discard → `let _ =`; **`Result` whose `Err` is used** →
   `.unwrap_or_else(|| Err(anyhow::anyhow!(…)))`. Picking the wrong neighbour's
   form is the likeliest failure.
2. **`pane_dead_status` returns `Option<i32>`** — `.ok()` will not compile.
3. **Keep the `||` short-circuit** at `sudo.rs:86`.
4. **Keep `break code` a `break`** at `scheduled.rs:296`.
5. **`capture_pane` depths differ** — 5000 in `scheduled.rs`, 20 in `sudo.rs`.
6. **`pane_id` is a `String` in `scheduled.rs`** — clone per closure.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-27 21:33 (started)

**Executor:** Claude (Sonnet 4.5)

Converting 11 tmux calls in `scheduled.rs` (7) and `utils/sudo.rs` (4) to use `off_runtime`.

### Update — ts=1785188532187 (complete, server-authored)

**Summary:** **Summary + Notes for review**

Converted all 11 tmux calls in `scheduled.rs` (7) and `utils/sudo.rs` (4) to use `tmux::off_runtime`, preventing blocking of the tokio async runtime. All four shapes were applied correctly: `.unwrap_or_else(|| Err(…))` for the 4 `Result` sites that preserve `Err` arms, `.flatten()` for 3 `Option<i32>` `pane_dead_status` sites, `.and_then(|r| r.ok()).unwrap_or_default()` for 3 `capture_pane` sites, and `let _ = …` for 1 discarded `send_keys`. The `sudo.rs:86` short-circuit is preserved via a block expression on the right of `||`, and `scheduled.rs:296`'s `break code` remains a `break` after splitting the `let`-chain. All acceptance criteria pass: per-file scan reports 0 for both files, `cargo build`, `cargo clippy`, `cargo fmt`, and `cargo test` (916 lib + 27 integration) all pass clean.

**Executor:** Qwen/Qwen3.6-27B-FP8

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.99s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
xt_client_viewport_shown_when_known ... ok
test tmux::cache::tests::get_labeled_context_client_viewport_absent_when_zero ... ok
test tmux::cache::tests::get_labeled_context_pane_classification ... ok
test tmux::cache::tests::get_labeled_context_copy_mode_annotated ... ok
test tmux::cache::tests::get_labeled_context_synchronized_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_dead_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_session_topology ... ok
test tmux::cache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test tmux::cache::tests::get_labeled_context_chat_pane_excluded_from_background ... ok
test search::tests::search_events_returns_tail_not_head_when_segment_exceeds_cap ... ok
test search::tests::search_finds_match_in_runbooks ... ok
test search::tests::search_respects_kind_filter ... ok
test search::tests::search_returns_empty_for_no_match ... ok
test session_store::tests::artifacts_round_trip ... ok
test session_store::tests::backfill_idempotent ... ok
test memory::tests::memory_frontmatter_tags_parsed ... ok
test session_store::tests::backfill_stamps_memory_without_frontmatter ... ok
test session_store::tests::backfill_stamps_runbook ... ok
test session_store::tests::backfill_stamps_script ... ok
test session_store::tests::collision_allowed_with_force ... ok
test memory::tests::memory_without_frontmatter_has_empty_metadata ... ok
test session_store::tests::delete_nonexistent_errors ... ok
test session_store::tests::delete_removes_dir_and_index ... ok
test session_store::tests::list_returns_newest_first ... ok
test session_store::tests::load_messages_max_count_truncates ... ok
test session_store::tests::rename_nonexistent_errors ... ok
test session_store::tests::rename_to_existing_errors ... ok
test memory::tests::update_memory_partial_update_preserves_other_fields ... ok
test session_store::tests::save_and_load_round_trip ... ok
test session_store::tests::update_in_place_allowed ... ok

test result: ok. 916 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.18s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 29 tests
test daemon_ping_status_loop ... ignored
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g4_briefing_injection_block_format ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test window_switch_does_not_corrupt_chat ... ignored
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_ask_round_trip ... ok
test ghost_config_parsing ... ok
test minimal_config_parsing ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test schedule_store_persistence ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test config_pricing_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test g4_briefing_masking_applied ... ok
test session_index_persistence ... ok
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
- `docs/dev/milestones/M5-ux-stability/phase-06g-tmux-off-runtime-scheduled-sudo.md` — +7 -1
- `src/daemon/scheduled.rs` — +48 -9
- `src/daemon/utils/sudo.rs` — +25 -4

**Commit:** bddec15a28487ba65e79abf8e2295d16db8ba647

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-07-27

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** Qwen/Qwen3.6-27B-FP8 (62 turns)
- **Scope deviations:** none
- **Calibration:** none

All four gates re-run bare and green (`cargo fmt --all --check`, `cargo build`
after `touch`ing both edited files — zero warnings, `cargo clippy --all-targets
--all-features -- -D warnings`, `cargo test` at 916 lib + 27 integration,
unchanged).

Every criterion is exact: the per-file scan reports **0** for both files;
`off_runtime` 7 and 4 against a verified 0 / 0 before; `unwrap_or_else(|| Err(`
**4**; `flatten()` 2 and 1 (the three `pane_dead_status` sites);
`grep -cF 'break code;'` **2**; `block_on`/`futures::executor`/`spawn_blocking`
**0** in both; two `src/` files in the code commit. Every
`and_then(|r| r.ok())` in both files sits on `capture_pane` — none on a
`pane_dead_status` site.

**All four shapes were applied to the right sites**, which was this phase's
stated main risk:

- **The `Err`-preserving collapse is correct at all four sites**, and — the part
  that matters — **every `Err(e)` arm survived byte-for-byte**. Both
  `store.mark_done(&job.id, false, Some(e.to_string()))` calls are still there,
  as are the "failed to create window", "failed to send keys", "Failed to rename
  sched window" and "Failed to set remain-on-exit" messages. A tmux timeout now
  flows into those same arms, so a job whose window was never created is marked
  **failed** rather than silently appearing to succeed.
- **The `||` short-circuit at `sudo.rs:93` is preserved.** The block expression
  sits on the right of `waited >= TIMEOUT ||`, so once the deadline passes the
  tmux subprocess is never spawned — exactly as before.
- **`break code` stayed a `break` in both places**, so `exit_code` still binds
  the loop's value. The `select!` arm's `let`-chain was split into a block
  rather than dodged.

Also verified by reading:

- **`capture_pane` depths carried per-site**, not harmonised: 5000 in
  `scheduled.rs:344`, 20 at both `sudo.rs` sites.
- **`wrapped` is moved, not cloned**, into the `send_keys` closure — correct,
  because it is constructed at `:214` and never read after `:285`. The executor
  chose the minimal form rather than cloning defensively.
- **`final_win_name` *is* cloned** into the rename closure, because the `Ok(())`
  arm still returns the original. The distinction between these two cases was
  got right without being spelled out.
- **Loop ordering is unchanged** — dead-check, then deadline, then `select!`. A
  wedged tmux costs each iteration up to `TMUX_TIMEOUT`, but the deadline check
  immediately follows, so the loop still terminates at 300 s.

Test plan honoured: no new tests, no coverage claim — correct for a scheduled-job
path and three sudo helpers that all need a live tmux server. All three reasoning
checks were answered and hold against the tree.

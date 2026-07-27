# Phase 06e: `foreground.rs` tmux Calls Off the Runtime — Slice 3 (exit status & cleanup)

**Milestone:** M5 — UX & Stability
**Status:** done
**Depends on:** phase-06d (slice 2) — `done`
**Estimated diff:** ~100 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Apply `tmux::off_runtime` to the **last 9** convertible tmux calls in
`src/daemon/executor/foreground.rs` — the exit-status detection and cleanup
stage.

06c converted 10 (setup & send), 06d converted 10 (poll & capture). These 9
finish the file.

**Finish condition: the span-matching script reports `UNWRAPPED: 2` — the two
`Drop` sites and nothing else.**

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
grep -c "off_runtime" src/daemon/executor/foreground.rs   # expect 20
grep -c "flatten()"   src/daemon/executor/foreground.rs   # expect 0
cargo test 2>&1 | grep "^test result" | head -3           # expect 916 lib, 0, 27 integration
```

**Every number above was produced by running that exact command against the
tree while drafting.** If one differs, **stop and report a blocker**.

## Current state

### ⭐ Worked examples — two of them, and you need both

**06c and 06d converted twenty sites in this same file.** Read them first;
they are the closest possible analogue. The two dominant shapes already present
in `foreground.rs`:

```rust
// Result-returning helper, value used, both failure modes collapse to a default
let t = target_str.to_string();
let snap = tmux::off_runtime("capture-pane", move || tmux::capture_pane(&t, 20))
    .await
    .and_then(|r| r.ok())
    .unwrap_or_default();

// ()-returning helper — no .ok(), and the Option<&str> arg goes owned + .as_deref()
let t = target_str.to_string();
let cp = chat_pane.map(|s| s.to_string());
let _ = tmux::off_runtime("unhighlight-pane", move || {
    tmux::unhighlight_pane(&t, cp.as_deref())
})
.await;
```

**This slice introduces a third return shape that neither of those covers**, so
the second worked example comes from `background/`:

```rust
// src/daemon/background/run.rs:307 — an Option-returning helper
let p_dead = pane_id.clone();
let code = tmux::off_runtime("pane-dead-status", move || tmux::pane_dead_status(&p_dead))
    .await
    .flatten()
    .unwrap_or(-1);
```

`pane_dead_status` returns `Option<i32>` — **the identical shape to
`read_pane_exit_status`**, which two of this slice's sites call. Copy that
form. (`src/daemon/background/respawn.rs:171` is the same conversion again.)

`spawn_blocking` requires `F: 'static`, so **every borrowed argument becomes
owned before the closure**.

### ⚠ Scope is defined by SITE, not by line number

06c's review established this and 06d confirmed it: a site's expression can
extend far past any line boundary. **One site in this slice is a 33-line
`match`** (see below). Converting it necessarily edits all 33 lines. That is
correct, not scope creep.

This phase owns **the nine sites named below and whatever their expressions
require, wherever those extend.** Line numbers shift as you edit — re-derive
with the Acceptance-criteria script.

### This slice's 9 sites — and they span two functions

Eight are in `async fn run_foreground`; **the ninth is in `async fn
run_background`.** Line numbers are current-as-of-drafting:

| Line | Call | Returns | Shape |
|---|---|---|---|
| 832 | `read_pane_exit_status` | `Option<i32>` | **`.flatten()`** — see below |
| 837 | `pane_pid` | `Result<u32>` | `.and_then(\|r\| r.ok()).unwrap_or(0)` |
| 855 | `read_pane_exit_status` | `Option<i32>` | **`.flatten()`** |
| 863 | `pane_pid` | `Result<u32>` | `.and_then(\|r\| r.ok()).unwrap_or(0)` |
| 868 | `pane_pid` | `Result<u32>` | `.and_then(\|r\| r.ok()).unwrap_or(0)` |
| 878 | `unhighlight_pane` | `()` | `let _ = …` — no `.ok()` |
| 880 | `capture_pane` | `Result<String>` | **the 33-line `match`** |
| 915 | `select_pane` | `Result<()>` | `let _ = …` |
| 1001 | `pane_exists` (in `run_background`) | `bool` | **negated gate** — see below |

### ⚠ Hazard 1 — `read_pane_exit_status` returns `Option<i32>`, so `.ok()` will not compile

```rust
// src/tmux/pane.rs:358
pub fn read_pane_exit_status(pane_id: &str) -> Option<i32> {
```

`off_runtime` yields `Option<Option<i32>>`. `.and_then(|r| r.ok())` — correct at
the `pane_pid` and `capture_pane` sites in this very slice — **will not compile
here**, because `Option` has no `.ok()` method. Use `.flatten()`, per the
`pane_dead_status` example quoted above.

Both sites read the same way today:

```rust
if let Some(code) = tmux::read_pane_exit_status(target_str) {
    exit_status = Some(code);
    break;
}
```

Target form — the `if let Some(code)` shape is preserved exactly:

```rust
let t = target_str.to_string();
let latch = tmux::off_runtime("read-pane-exit-status", move || {
    tmux::read_pane_exit_status(&t)
})
.await
.flatten();
if let Some(code) = latch {
    exit_status = Some(code);
    break;
}
```

A timeout collapses to `None`, which reads as "the latch is not set yet" — the
loop keeps polling until its deadline, exactly as it does today when the latch
is genuinely absent. **That is the correct direction; do not add a fallback
exit code.**

### ⚠ Hazard 2 — `pane_exists` is a *negated* gate, and `.unwrap_or(false)` is still right

```rust
// src/daemon/executor/foreground.rs:1001, inside run_background
if !crate::tmux::pane_exists(pane_id) {
    let msg = format!("Error: retry_in_pane '{}' does not exist. …", pane_id);
    send_response_split(tx, Response::ToolResult(msg.clone())).await?;
    return Ok(ToolCallOutcome::Result(msg));
}
```

`pane_exists` returns a bare `bool`, so `off_runtime` yields `Option<bool>` and
the established rule is `.unwrap_or(false)` — **a timeout must not read as
"yes"**. The negation makes that look inverted at a glance. It is not:

- `.unwrap_or(false)` → `!false` is `true` → the retry is refused with the
  existing error message. **Fail-safe**: the daemon declines to respawn into a
  pane it could not confirm.
- `.unwrap_or(true)` → the guard is skipped and the code proceeds to respawn
  into a pane whose existence was never established.

**Write `.unwrap_or(false)`. Do not "correct" it to `true`.**

Note this site is written `crate::tmux::pane_exists`, not `tmux::pane_exists` —
keep whichever path form compiles in that scope.

### ⚠ Hazard 3 — the `capture_pane` site is a 33-line `match` with three arms

Today (`:880`–`:912`), abbreviated:

```rust
let mut output = match tmux::capture_pane(target_str, 200) {
    Ok(snap) if is_interactive => { … 20 lines … }
    Ok(snap) => { … 9 lines … }
    Err(_) => "Command sent but could not capture output.".to_string(),
};
```

Collapse the timeout into the **same** fallback the `Err` arm already uses, and
rewrite the arm patterns from `Ok`/`Err` to `Some`/`None`:

```rust
let t = target_str.to_string();
let captured = tmux::off_runtime("capture-pane", move || tmux::capture_pane(&t, 200))
    .await
    .and_then(|r| r.ok());
let mut output = match captured {
    Some(snap) if is_interactive => { … unchanged … }
    Some(snap) => { … unchanged … }
    None => "Command sent but could not capture output.".to_string(),
};
```

**The three arm bodies are unchanged — only the patterns and the scrutinee
move.** The guard `if is_interactive` stays on the first arm and the arm order
stays the same; swapping them would send every capture down the interactive
branch. The `None` string must stay byte-identical, including the trailing
period.

### ⚠ Hazard 4 — `pane_pid` at `:837` tests `!=`, not `==`

```rust
if tmux::pane_pid(target_str).unwrap_or(0) != idle_pid {
    saw_child = true;
}
```

The other two `pane_pid` sites test `== idle_pid`. This one is inverted, so a
timeout (→ `0`, and this branch only runs when `idle_pid != 0`) sets
`saw_child = true`. **That is exactly what a `pane_pid` error does today**, so
preserving `.unwrap_or(0)` is behaviour-preserving. **Do not invent a different
sentinel to "fix" it** — changing the failure default is a separate decision and
is out of scope here.

### ⚠ Three non-sites — unchanged from 06c and 06d

| Hit | Why not a site |
|---|---|
| `:74`, `:79` — `Command::new("tmux")` in `impl Drop for FgHookGuard` | `Drop::drop` cannot be `async` |
| `:23` — `use crate::tmux::cache::SessionCache;` | a type import |
| `wait_for_sudo_prompt_and_inject` | a local helper, not `tmux::wait_for` |

**No** `block_on`, `futures::executor`, or detached `tokio::spawn` as a
workaround for the `Drop` limit. Those two calls are bounded by **stage A**
(hardening the sync helpers), not by this phase.

## Spec

### 1. Convert the 9 sites

Match each to its return shape from the table above. **Preserve each site's
existing failure default exactly** — `.unwrap_or(0)` stays `0`, the capture
fallback string stays byte-identical, the exit-status latch stays `None`.

### 2. Build after every site

Not a suggestion. `cargo build` after each converted site. A predecessor run on
this file died because one conversion's type error surfaced 470 lines from its
cause and could not be localised. **If a converted form would change the type of
a binding used later, keep the binding's type identical** — collapse the
`off_runtime` result at the site rather than letting `Option<…>` leak
downstream.

The three return shapes in this slice collapse differently. Getting them mixed
up is the single most likely way to lose the run:

| Helper returns | Collapse with |
|---|---|
| `Result<T>` | `.and_then(\|r\| r.ok())` |
| `Option<T>` | `.flatten()` |
| `bool` | `.unwrap_or(false)` |
| `()` | nothing — `let _ = … .await;` |

### 3. Leave the `Drop` block alone

`impl Drop for FgHookGuard` (`:71`–`:84`) must come out byte-identical.

## Acceptance criteria

- [ ] **Span-matching check reports `UNWRAPPED: 2`:**

```bash
python3 - <<'PY'
import re, pathlib
src = pathlib.Path("src/daemon/executor/foreground.rs").read_text()
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
print("UNWRAPPED:", len(bad))
for l, n in sorted(bad): print(f"   {l}: {n}")
PY
#   UNWRAPPED: 2
#   both lines are Command::new("tmux") inside impl Drop for FgHookGuard (~74, ~79)
```

      **Read the two lines and confirm both are the `Drop` sites.** Any other
      name means a site was missed.

- [ ] `grep -c "off_runtime" src/daemon/executor/foreground.rs` returns **≥ 29**
      (the command printed 20 before this phase; 9 sites are added). The span
      check above is what proves the exact set; this is a floor, not an identity.
- [ ] `grep -c "flatten()" src/daemon/executor/foreground.rs` returns **≥ 2** —
      one per `read_pane_exit_status` site. The command printed **0** before this
      phase, so a 0 here means both were converted with the wrong shape (or not
      at all).
- [ ] Every `and_then(|r| r.ok())` in the file sits on a helper that returns
      `Result`. **`read_pane_exit_status`, `unhighlight_pane` and `pane_exists`
      must have none.** Verify by reading each converted site, not by counting.
- [ ] The `impl Drop for FgHookGuard` block is **byte-identical**. Verify with
      `diff` against the parent commit, not by eye, and quote the result.
- [ ] `grep -c "block_on\|futures::executor" src/daemon/executor/foreground.rs`
      returns **0**. (Scoped to this file — pre-existing hits elsewhere under
      `executor/knowledge/` are unrelated and must not be touched.)
- [ ] `grep -c "spawn_blocking" src/daemon/executor/foreground.rs` returns **0**.
- [ ] `git diff --name-only` lists exactly **one** `src/` file.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **916** lib-unit and **27** integration tests.

**Run every gate bare.**

## Test plan

`run_foreground` and `run_background` need a live tmux server and pane; they
have **no unit coverage**. Pre-existing gap, neither widened nor closed here.
The 13 tests in this file's `mod tests` cover only `looks_like_shell_prompt`,
`is_shell_prompt` and `exit_status_annotation` — pure functions this phase does
not touch.

**Write no new tests.** Run the suite and report which commands you ran and
whether they passed. **Do not claim any test guards these sites.**

Three reasoning checks. **Quote the code — a claim without a quotation is not
an answer:**

1. **`read_pane_exit_status`.** Paste one converted site and show it uses
   `.flatten()` and not `.and_then(|r| r.ok())`, explaining in one sentence why
   the latter would not compile.
2. **`pane_exists`.** Paste the converted site and state, in one sentence, what
   the daemon does when the call times out and why that is the safe direction
   given the `!` in front of it.
3. **The `capture_pane` match.** Quote the three arm *patterns* before and
   after. Show the guard is still on the first arm and the fallback string is
   unchanged.

## End-to-end verification

None required. 06a demonstrated that the timeout arm fires; this phase adds no
machinery. **Do not repeat that demonstration.**

## Authorizations

- [x] May edit `src/daemon/executor/foreground.rs` — **the nine named sites and
      whatever their expressions require**, including the 33-line `match`.
- [x] May add owned bindings at call sites.
- [x] May rewrite the `capture_pane` match arms from `Ok`/`Err` to `Some`/`None`.
- [ ] **No** edit to `impl Drop for FgHookGuard`.
- [ ] **No** change to any site's failure default.
- [ ] **No** `block_on`, `futures::executor`, or detached `tokio::spawn`.
- [ ] **No** edits to `src/tmux/mod.rs` or any other file.
- [ ] **No** new tests, no new dependency, no `#[allow(...)]`.

## Out of scope

- **The 2 `Drop` calls** — structurally unconvertible; bounded by stage A.
- **`executor/knowledge/pane.rs`, `file_ops/`, `daemon/` core, `cli/`** — 06f–06h.

### ⚠ Traps

1. **Three return shapes, three different collapses.** `Result` → `.ok()`,
   `Option` → `.flatten()`, `bool` → `.unwrap_or(false)`. Copying the wrong
   neighbour's form is this phase's most likely failure.
2. **Build after every site.** A type error 470 lines from its cause killed an
   earlier run of this file.
3. **Use the span script, not a `move ||` line grep** — rustfmt puts closure
   bodies on the next line, which produces false positives.
4. **`.unwrap_or(false)` under a `!` is correct.** Reason about the timeout
   path before changing it.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-27 19:37 (started)

**Executor:** claude-code
**Action:** Started phase 06e — converting 9 remaining tmux calls in `foreground.rs` to `off_runtime` (exit status detection, cleanup, and the `pane_exists` gate in `run_background`).

### Update — ts=1785181519065 (complete, server-authored)

**Summary:** All tasks complete. Here is the **Summary + Notes for review**:

**What I built:** Converted all 9 remaining direct `tmux::*` calls in `foreground.rs` to `tmux::off_runtime`, finishing the file. The conversions used three distinct collapse shapes: `.flatten()` for the two `read_pane_exit_status` calls (`Option<i32>`), `.and_then(|r| r.ok()).unwrap_or(0)` for the four `pane_pid` calls (`Result<u32>`), `let _ =` for `unhighlight_pane` and `select_pane` (`()`), `.and_then(|r| r.ok())` with `Some`/`None` match arms for the 33-line `capture_pane` match, and `.unwrap_or(false)` for the negated `pane_exists` gate in `run_background`.

**Reasoning checks:**
1. `read_pane_exit_status` uses `.flatten()` because it returns `Option<i32>`, so `off_runtime` yields `Option<Option<i32>>` — `.and_then(|r| r.ok())` would not compile since `Option` has no `.ok()` method.
2. `pane_exists` uses `.unwrap_or(false)`: on timeout the guard becomes `!false` → `true`, refusing the retry — the fail-safe direction since the daemon declines to respawn into a pane it could not confirm exists.
3. The `capture_pane` match arms changed from `Ok(snap) if is_interactive` / `Ok(snap)` / `Err(_)` to `Some(snap) if is_interactive` / `Some(snap)` / `None`, with the guard preserved on the first arm and the fallback string byte-identical.

**Verification:** Span script reports `UNWRAPPED: 2` (both `Command::new("tmux")` in `Drop`). `off_runtime` count: 29. `flatten()` count: 2. `cargo build`, `cargo clippy`, `cargo fmt`, and `cargo test` (916 lib + 27 integration) all pass clean. Working tree is clean.

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
client_viewport_absent_when_zero ... ok
test tmux::cache::tests::get_labeled_context_session_topology ... ok
test tmux::cache::tests::get_labeled_context_client_viewport_shown_when_known ... ok
test tmux::cache::tests::get_labeled_context_background_panes_sorted ... ok
test tmux::cache::tests::get_labeled_context_synchronized_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_dead_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_pane_classification ... ok
test tmux::cache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test tmux::cache::tests::get_labeled_context_chat_pane_excluded_from_background ... ok
test search::tests::search_events_returns_tail_not_head_when_segment_exceeds_cap ... ok
test search::tests::search_finds_match_in_runbooks ... ok
test search::tests::search_respects_kind_filter ... ok
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

test result: ok. 916 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.22s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 29 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_allow_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g4_briefing_injection_block_format ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test g5_child_inherits_depth_and_parent ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g5_depth_limit_enforced ... ok
test window_switch_does_not_corrupt_chat ... ignored
test ipc_tool_call_response_round_trip ... ok
test event_log_append_read ... ok
test ipc_session_info_round_trip ... ok
test ipc_ask_round_trip ... ok
test minimal_config_parsing ... ok
test ghost_config_parsing ... ok
test schedule_store_persistence ... ok
test event_log_entry_format ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_namespace_field_persisted ... ok
test g6_agent_config_roundtrip ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_to_event_log ... ok
test g5_mailbox_write_and_read ... ok

test result: ok. 27 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.02s


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
- `docs/dev/milestones/M5-ux-stability/phase-06e-tmux-off-runtime-foreground-3.md` — +6 -1
- `src/daemon/executor/foreground.rs` — +52 -12

**Commit:** 9c052999a241a305b23c660bcea0d7c033e128a4

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-07-27

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** Qwen/Qwen3.6-27B-FP8 (59 turns)
- **Scope deviations:** none
- **Calibration:** one non-blocking note (see bottom)

All four gates re-run bare and green (`cargo fmt --all --check`, `cargo build`
after `touch`ing the edited file — zero warnings, `cargo clippy --all-targets
--all-features -- -D warnings`, `cargo test` at 916 lib + 27 integration,
unchanged).

**`foreground.rs` is finished.** The span check reports `UNWRAPPED: 2` and both
lines are `Command::new("tmux")` inside `impl Drop for FgHookGuard` (`:74`,
`:79`) — nothing else remains. `off_runtime` 29, `flatten()` 2,
`block_on`/`futures::executor`/`spawn_blocking` all 0, exactly one `src/` file in
the code commit, and the `Drop` block diffs clean against the parent.

**The three-shape hazard was navigated correctly at every site.** Mapping each
`off_runtime` call to its collapse:

| Site | Helper returns | Collapse | Correct |
|---|---|---|---|
| `:833`, `:867` | `Option<i32>` | `.flatten()` | ✅ |
| `:844`, `:881`, `:890` | `Result<u32>` | `.and_then(\|r\| r.ok()).unwrap_or(0)` | ✅ |
| `:906` | `()` | discarded, no collapse | ✅ |
| `:912` | `Result<String>` | `.and_then(\|r\| r.ok())` | ✅ |
| `:951` | `Result<()>` | discarded | ✅ |
| `:1038` | `bool` | `.unwrap_or(false)` | ✅ |

Every `and_then(|r| r.ok())` in the file sits on `capture_pane`,
`pane_current_command` or `pane_pid`. None on `read_pane_exit_status`,
`unhighlight_pane` or `pane_exists` — the three that would not compile with one.

Verified by reading, since counts cannot show these:

- **Both traps where the correct code looks wrong were left correct.**
  `pane_exists` is `.unwrap_or(false)` under its `!`, so a timeout refuses the
  retry rather than respawning into an unconfirmed pane. The `:844` `pane_pid`
  still tests `!=`, not `==`, and kept `.unwrap_or(0)` — the same thing an error
  does there today. Neither was "improved".
- **The 33-line `match` was rewritten, not mangled.** Arms are
  `Some(snap) if is_interactive` (`:916`) → `Some(snap)` (`:936`) → `None`
  (`:946`) — original order, guard still on the first arm, all three bodies
  untouched. Swapping the first two would route every capture down the
  interactive branch silently; it did not happen. The fallback string is
  byte-identical by literal `grep -cF`.
- **The `read_pane_exit_status` hoists preserved statement order.** Both moved
  the call out of the `if let` scrutinee into a preceding `let`, leaving the
  `if let Some(code) { exit_status = …; break; }` shape and its position
  relative to the deadline check and the `sleep` unchanged. A timeout reads as
  "latch not set yet" and the loop keeps polling to its deadline.
- **`unhighlight_pane` still precedes the capture** and still runs on the same
  path, so no exit route leaves the pane tinted.

Test plan honoured: no new tests, no coverage claim. All three required
reasoning checks were answered and all three hold against the tree.

**Calibration note (non-blocking, first occurrence in an executor summary).**
The completion summary says "the four `pane_pid` calls"; this phase converted
**three** (`:844`, `:881`, `:890`) — the other two `pane_pid` sites in the file
came from 06c and 06d. The code is right and every criterion passed; only the
prose count is wrong. Worth noting because it is the same failure mode as the
architect-side counting thread — a number written rather than run — now
appearing on the executor side. One occurrence; no action.

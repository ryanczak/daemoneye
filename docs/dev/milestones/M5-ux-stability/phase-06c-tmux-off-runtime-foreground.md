# Phase 06c: `foreground.rs` tmux Calls Off the Runtime — Slice 1 (setup & send)

**Milestone:** M5 — UX & Stability
**Status:** review
**Depends on:** phase-06b — `done`
**Estimated diff:** ~90 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Apply `tmux::off_runtime` to the **10** tmux calls in the first region of
`src/daemon/executor/foreground.rs` — **lines ≤ 460**, the setup-and-send stage
of `run_foreground`.

**This phase was re-scoped after a `hard_fail`.** The first attempt tried all 29
sites in this 1228-line file at once, converted 5, hit a type error whose symptom
surfaced 470 lines from its cause, and stalled re-reading the file for 60
consecutive turns. The file is now split into three slices of ~10 sites each —
the size that succeeded in the two preceding phases.

**Finish condition: the span-matching script reports `UNWRAPPED: 21` — 19
convertible sites in later slices, plus the 2 `Drop` sites — and every remaining
one is at line > 460 or inside `impl Drop`.**

## Architecture references

Read before starting:

- `docs/design/daemon-stalls.md` § 1 mechanism B — blocking subprocess spawns on
  tokio workers.
- `src/tmux/mod.rs` — the `off_runtime` adapter and `TMUX_TIMEOUT`.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Verify the starting state:

```bash
grep -c "off_runtime" src/daemon/executor/foreground.rs   # expect 0
grep -c "off_runtime" src/daemon/background/respawn.rs    # expect 15
cargo test 2>&1 | grep "^test result" | head -2           # expect 916 lib, 27 integration
```

**Verified against the tree while drafting.** If any count differs, **stop and
report a blocker.**

## Current state

### ⭐ Two in-tree worked examples — read them, do not invent

`background/run.rs` (06a) and `background/respawn.rs` (06b). The canonical forms:

```rust
// value used, both failure modes collapse to a default
let p = pane_id.to_string();
let out = tmux::off_runtime("capture-pane", move || tmux::capture_pane(&p, 10))
    .await
    .and_then(|r| r.ok())
    .unwrap_or_default();

// error inspected; timeout arm is a no-op because off_runtime already logged it
let (s, w) = (session.to_string(), win_name.to_string());
match tmux::off_runtime("kill-job-window", move || tmux::kill_job_window(&s, &w)).await {
    Some(Err(e)) => log::error!("…: {e}"),
    None => {} // already logged by off_runtime
    Some(Ok(_)) => {}
}
```

`spawn_blocking` requires `F: 'static`, so **every borrowed argument becomes owned
before the closure**. That is the per-site work.

### ⚠ The type error that stalled the first attempt

The first run converted `send_keys` (`:374`) as
`let result = match tmux::off_runtime("send-keys", …)` and produced a type
mismatch that the compiler reported **470 lines later**, at `:860`, where
`result` was consumed.

**Convert one site, then run `cargo build`, before moving to the next.** Ten
small builds cost seconds each; one big build at the end reports an error whose
cause you then have to find in a 1228-line function. That is exactly what
consumed the previous run.

If a site's converted form changes the type of a binding used later, **that is a
signal to keep the binding's type identical** — collapse the `off_runtime` result
back to whatever the original expression produced, at the site, rather than
letting `Option<Result<…>>` leak downstream.

### ⚠ Three non-sites — do not wrap them

| Hit | Why not a site |
|---|---|
| `:74`, `:79` — `Command::new("tmux")` in `impl Drop for FgHookGuard` | **`Drop::drop` cannot be `async`.** No `.await` in a destructor. Structurally impossible. |
| `:23` — `use crate::tmux::cache::SessionCache;` | a type import; spawns nothing |
| `:12`, `:563` — `wait_for_sudo_prompt_and_inject` | a local async helper, not `tmux::wait_for` |

**Do not** work around the `Drop` limit with `block_on`, `futures::executor`, or a
detached `tokio::spawn` — all are worse than the blocking call. Bounding those two
belongs to the later sync-helper stage.

### This slice's 10 sites

| Line | Call |
|---|---|
| 200 | `pane_exists` |
| 303 | `pane_pid` |
| 362 | inline `Command::new("tmux")` |
| 372 | `clear_pane_exit_status` |
| 374 | `send_keys` ← the one that broke the first attempt |
| 376 | `highlight_pane` |
| 420 | `pane_current_command` |
| 427 | `capture_pane` |
| 452 | `pane_current_command` |
| 457 | `capture_pane` |

Line numbers shift as you edit; re-derive with the Acceptance-criteria script.

## Spec

### 1. Convert the 10 sites at lines ≤ 460

Ordinary A/B/C shapes from `run.rs`/`respawn.rs`, except:

**`pane_exists` (`:200`) gates behaviour and returns `bool`.** A timeout must not
read as "the pane exists":

```rust
let p = target.to_string();
let exists = tmux::off_runtime("pane-exists", move || tmux::pane_exists(&p))
    .await
    .unwrap_or(false);
```

Treating a wedged tmux as "pane present" would send keys into a pane that may be
gone.

**`send_keys` (`:374`) is where the first attempt broke.** Keep the surrounding
binding's type exactly as it is today. Convert it, then `cargo build`, before
touching anything else.

**`highlight_pane` (`:376`) has its `unhighlight_pane` partners in later slices**
(`:602`, `:773`). Convert the highlight as Shape A (`let _ = …`) so it still runs
on exactly the paths it runs on today. **Do not touch the unhighlight calls** —
they are 06d's and 06e's. The pair stays balanced because every conversion is
behaviour-preserving; an early return that skipped one would break it.

### 2. Build after every site

Not a suggestion. `cargo build` after each converted site, so a type error is
attributed to the site that caused it.

### 3. Change nothing at line > 460, and nothing in `impl Drop`

## Acceptance criteria

- [ ] **Span-matching check reports `UNWRAPPED: 21`, and every one is either at
      line > 460 or inside `impl Drop`:**

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
early = [(l, n) for l, n in sorted(bad) if l < 100 or l <= 460]
print("of which at line <= 460 (must be ONLY the 2 Drop sites):")
for l, n in early: print(f"   {l}: {n}")
PY
#   UNWRAPPED: 21
#   of which at line <= 460 (must be ONLY the 2 Drop sites):
#      <~74>: Command::new("tmux")
#      <~79>: Command::new("tmux")
```

- [ ] `grep -c "off_runtime" src/daemon/executor/foreground.rs` returns **≥ 10**.
- [ ] The `impl Drop for FgHookGuard` block is **byte-identical** to its current
      form. Quote it in the Update Log.
- [ ] `grep -c "spawn_blocking" src/daemon/executor/foreground.rs` returns **0**,
      and `grep -rn "block_on\|futures::executor" src/daemon/executor/` returns
      **nothing**.
- [ ] `git diff --name-only` lists exactly **one** `src/` file.
- [ ] `grep -c "off_runtime" src/daemon/background/respawn.rs` returns **15**,
      unchanged.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **916** lib-unit and **27** integration tests.

**Run every gate bare.**

## Test plan

`run_foreground` needs a live tmux server and pane; it has **no unit coverage**
and cannot have any here. That is a pre-existing gap this phase neither widens nor
closes.

**Write no new tests.** Run the suite and report which commands you ran and
whether they passed. Do not claim any test guards these sites.

Two reasoning checks. **Quote the code — a claim without a quotation is not an
answer:**

1. **`pane_exists` on timeout.** Quote the converted site with its
   `.unwrap_or(false)` and say what would go wrong if a timeout read as `true`.
2. **The `Drop` block.** Paste it as it stands after your changes and state why
   `off_runtime` cannot be used there.

## End-to-end verification

None required. 06a already demonstrated the timeout arm fires; this phase adds no
machinery. **Do not repeat that demonstration.**

## Authorizations

- [x] May edit `src/daemon/executor/foreground.rs` — **lines ≤ 460 only**.
- [x] May add owned bindings at call sites.
- [ ] **No** edit to `impl Drop for FgHookGuard`.
- [ ] **No** `block_on`, `futures::executor`, or detached `tokio::spawn`.
- [ ] **No** edits at line > 460 — those are 06d and 06e.
- [ ] **No** edits to `src/tmux/mod.rs` or any other file.
- [ ] **No** new tests, no new dependency, no `#[allow(...)]`.

## Out of scope

- **Lines 461–710 (10 sites)** — phase **06d**.
- **Lines > 710 (9 sites)** — phase **06e**.
- **The 2 `Drop` calls** — structurally unconvertible; the sync-helper stage.
- **`executor/knowledge/pane.rs`, `file_ops/`, `daemon/` core, `cli/`** — 06f–06h.

### ⚠ Traps

1. **Build after every site.** The previous attempt's type error surfaced 470
   lines from its cause and cost the run.
2. **A `move ||` line-heuristic gives false positives** — rustfmt puts closure
   bodies on the next line. Use the span script.
3. **Not every `tmux::` hit is a site** — see the three non-sites above.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-27 (escalation)

**Chosen lever:** re-split (not resume, not takeover)
**Rationale:** the first attempt's `hard_fail` was a scope problem, not a spec
gap — 29 sites in a 1228-line file, 5 converted, then 60 read-only turns chasing
a type error reported 470 lines from its cause; resuming into the same scope
would hit the same wall, and takeover would forfeit telemetry on a phase whose
only defect is size.

The partial work (5 sites, **non-compiling**) was stashed as
`stash@{0}` — "06c partial: 5/29 sites, broken build (type error at :860 from
:389)" — and the tree returned to a green baseline (916 tests) rather than
handing the next run a broken file to interpret. Those 5 sites are re-done as
part of this slice; redoing them mechanically is cheaper than resuming from a
broken state.

`foreground.rs` is now three phases of ~10 sites — the size that landed
`approved_first_try` in 06a (16) and 06b (11).

### Update — 2026-07-27 17:27 (started)

**Executor:** Claude (Sonnet 4.5)

### Update — ts=1785173747986 (complete, server-authored)

**Summary:** **Summary + Notes for review:**

Converted all 10 tmux call sites at lines ≤ 460 in `foreground.rs` to use `tmux::off_runtime`, following the patterns from phases 06a and 06b. Sites converted: `pane_exists` (line 200, `.unwrap_or(false)`), `pane_pid` (line 303), inline `Command::new("tmux")` set-hook (line 362), `clear_pane_exit_status` (line 372), `send_keys` (line 374, the one that broke the first attempt — kept the `result` binding type identical by collapsing `Option<Result<()>>` at the match site), `highlight_pane` (line 376, Shape A `let _ =`), `pane_current_command` (lines 420 and 452), and `capture_pane` (lines 427 and 457). Built after every single site conversion as required.

The `impl Drop for FgHookGuard` block is byte-identical to its pre-change form — `Drop::drop` cannot be async, so no conversion is possible there. The span-matching script confirms `UNWRAPPED: 21`, with only the 2 Drop `Command::new("tmux")` calls at lines 74 and 79 remaining below line 460. All 916 lib-unit and 27 integration tests pass, clippy is clean, and the working tree is clean.

**Executor:** Qwen/Qwen3.6-27B-FP8

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s


TEST
_yes_detected ... ok
test webhook::process::tests::severity_rank_ordering ... ok
test webhook::server::tests::auth_correct_token_allows ... ok
test session_store::tests::backfill_stamps_runbook ... ok
test webhook::server::tests::auth_empty_secret_always_allows ... ok
test webhook::server::tests::auth_missing_header_denies ... ok
test webhook::server::tests::auth_token_without_bearer_prefix_denies ... ok
test webhook::server::tests::auth_wrong_token_denies ... ok
test session_store::tests::backfill_stamps_script ... ok
test session_store::tests::collision_allowed_with_force ... ok
test session_store::tests::collision_rejected_without_force ... ok
test session_store::tests::delete_nonexistent_errors ... ok
test session_store::tests::delete_removes_dir_and_index ... ok
test tmux::cache::tests::get_labeled_context_background_panes_sorted ... ok
test tmux::cache::tests::get_labeled_context_pane_classification ... ok
test tmux::cache::tests::get_labeled_context_client_viewport_absent_when_zero ... ok
test tmux::cache::tests::get_labeled_context_client_viewport_shown_when_known ... ok
test tmux::cache::tests::get_labeled_context_session_topology ... ok
test tmux::cache::tests::get_labeled_context_dead_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_copy_mode_annotated ... ok
test tmux::cache::tests::get_labeled_context_synchronized_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test session_store::tests::list_returns_newest_first ... ok
test session_store::tests::load_messages_max_count_truncates ... ok
test session_store::tests::rename_nonexistent_errors ... ok
test session_store::tests::rename_to_existing_errors ... ok
test session_store::tests::rename_updates_dir_and_index ... ok
test session_store::tests::save_and_load_round_trip ... ok
test session_store::tests::update_in_place_allowed ... ok
test tmux::cache::tests::get_labeled_context_chat_pane_excluded_from_background ... ok

test result: ok. 916 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.58s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 29 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g5_depth_limit_enforced ... ok
test window_switch_does_not_corrupt_chat ... ignored
test ipc_tool_call_response_round_trip ... ok
test event_log_entry_format ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test schedule_store_persistence ... ok
test ipc_session_info_round_trip ... ok
test event_log_append_read ... ok
test minimal_config_parsing ... ok
test g4_briefing_read_and_clear ... ok
test ghost_config_parsing ... ok
test g6_agent_config_roundtrip ... ok
test config_pricing_round_trip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test g4_briefing_masking_applied ... ok
test g4_briefing_injects_on_next_run ... ok
test session_jsonl_round_trip ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test webhook_alert_to_event_log ... ok
test g5_mailbox_write_and_read ... ok

test result: ok. 27 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.02s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.08s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-02078a4e6496e4bf)
     Running unittests src/main.rs (target/debug/deps/daemoneye-a516fbd17ae98770)
     Running tests/integration.rs (target/debug/deps/integration-ce8aee7d32f385fa)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M5-ux-stability/README.md` — +1 -1
- `docs/dev/milestones/M5-ux-stability/phase-06c-tmux-off-runtime-foreground.md` — +5 -1
- `src/daemon/executor/foreground.rs` — +85 -14

**Commit:** c893c895e63d5481dcc91abc70b6acbc6ff1794b

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

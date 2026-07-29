# Phase 06v: `bounded_output` — Stage A Slice 3b, `src/tmux/pane.rs` (last 14)

**Milestone:** M5 — UX & Stability
**Status:** review
**Depends on:** phase-06u — `done`
**Estimated diff:** ~95 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

Convert the **remaining 14 `.output()` call sites in `src/tmux/pane.rs`** — every
one from `read_pane_exit_status` to the end of the file — to
`crate::tmux::bounded_output`.

This finishes `src/tmux/`. After it, **every synchronous tmux spawn in the
directory is timeout-bounded**: `window.rs` 6 + `session.rs` 9 + `pane.rs` 29 =
**44**.

**Finish condition: `src/tmux/pane.rs` has exactly 1 `.output()` call left and 29
`bounded_output(` calls.** The residue is `wait_for`'s `tokio::process` call —
see Hazard 1. **A residue of 0 means something was converted that must not be.**

## Architecture references

- `docs/design/daemon-stalls.md` § 1 mechanism B.
- `src/tmux/mod.rs` — `bounded_output`, `bounded_output_with`, `TMUX_TIMEOUT`.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Verify the starting state:

```bash
grep -c "\.output()"      src/tmux/pane.rs                       # expect 15
grep -c "bounded_output(" src/tmux/pane.rs                       # expect 15
grep -c 'std::process::Command::new("tmux")' src/tmux/pane.rs    # expect 2
grep -c "tokio::process::Command" src/tmux/pane.rs               # expect 2
grep -c "bounded_output(" src/tmux/session.rs                    # expect 9
grep -c "bounded_output(" src/tmux/window.rs                     # expect 6
cargo test 2>&1 | grep "^test result" | head -3   # expect 921 lib, 0, 27 integration
```

**Every number above was produced by running that exact command against the tree
while drafting.** If one differs, **stop and report a blocker**.

## Current state

### The conversion, unchanged from slices 1, 2 and 3a

```
<builder>.output()   →   crate::tmux::bounded_output(<builder>)
```

`bounded_output` returns the **same** `std::io::Result<std::process::Output>`, so
**every surrounding expression stays exactly as it is** — the `?`, the `.ok()?`,
the `let _ =`, the `.map(…).unwrap_or(false)`. **There is no collapse. Do not add
one.**

### This slice is uniform — every site is a bare `Command::new("tmux")`

Unlike slice 3a, **there are no fully-qualified sites here.** The file's two
`std::process::Command::new("tmux")` sites (`start_pipe_pane`, `stop_pipe_pane`)
are at `:250` and `:271` — **already converted by 06u.** Leave them alone; the
count must still read **2** at the end.

### ⚠ Hazard 1 — `wait_for` uses `tokio::process` and must NOT be touched

At the bottom of the file:

```rust
pub async fn wait_for(channel: &str, timeout: std::time::Duration) -> bool {
    let mut child = match tokio::process::Command::new("tmux")
        .args(["wait-for", channel])
        .spawn()
    { … };
    match tokio::time::timeout(timeout, child.wait()).await {
        …
            let _ = tokio::process::Command::new("tmux")
                .args(["wait-for", "-S", channel])
                .output()
                .await;
```

Two reasons it is not a target: it is **already async and already bounded** by
`tokio::time::timeout`, and `bounded_output` takes `&mut std::process::Command`,
so it would not compile anyway. **This is the file's 1-call residue.**

### The 14 sites

Line numbers are current-as-of-drafting; re-derive with
`grep -n "\.output()" src/tmux/pane.rs`.

| Fn | Site(s) | Surrounding shape |
|---|---|---|
| `read_pane_exit_status` | `:370` | `.ok()?` |
| `clear_pane_exit_status` | `:391` | `let _ = …;` |
| `send_keys` | `:398` | `?;` |
| `send_cancel` | `:412` | `?;` |
| `pane_window_id` | `:424` | `?;` |
| `list_panes_in_window` | `:434` | `?;` |
| `pane_exists` | `:447` | `.map(\|o\| o.status.success()).unwrap_or(false)` |
| `highlight_pane` | `:462`, `:466` | `let _ = …;` ×2 |
| `unhighlight_pane` | `:478`, `:482` | `let _ = …;` ×2 |
| `set_remain_on_exit` | `:490` | `?;` |
| `save_buffer` | `:502` | `?;` |
| `delete_buffer` | `:514` | `let _ = …;` |

**The four shapes above are listed only so you can confirm you disturbed
nothing.** The substitution is type-preserving, so all four stay byte-identical.
**Adding a collapse — `.ok()`, `.flatten()`, `.unwrap_or_default()` — is the
first trap.**

What a timeout produces at each site is already what that site produces when tmux
fails today, because `.output()` already returns `Err` on spawn failure:
`read_pane_exit_status` → `None`, `pane_exists` → `false`, the `?` sites →
propagated `Err`, the `let _ =` sites → nothing (best-effort by design). **The
change replaces "hang forever" with "fail the way this site already fails".**

### ⚠ `cargo fmt` reflows these call sites heavily

Converting changes the expression's nesting depth, so `fmt` re-wraps the
`.args([…])` arrays — sometimes exploding a one-line array to one element per
line, sometimes pulling a chain back to column 4. **Expected and correct.** Apply
the substitution, run `cargo fmt --all`, accept its output. **Do not hand-format.**

Two post-`fmt` forms from the checked run, showing the range:

```rust
// read_pane_exit_status — fmt pulls the whole thing onto a continuation line
    let output =
        crate::tmux::bounded_output(Command::new("tmux").args(["show-environment", &key])).ok()?;
```

```rust
// pane_exists — fmt explodes the array AND de-indents the trailing chain to col 4
    crate::tmux::bounded_output(Command::new("tmux").args([
        "display-message",
        "-t",
        pane_id,
        "-p",
        "#{pane_id}",
    ]))
    .map(|o| o.status.success())
    .unwrap_or(false)
```

The second is the one that reads wrong at a glance — the `.map`/`.unwrap_or`
lines drop back to the function's own indent level. **That is `rustfmt`'s output,
not a mistake. Leave it.**

## Spec

1. **Convert the 14 `.output()` sites** in `src/tmux/pane.rs` from
   `read_pane_exit_status` to the end of the file, per the substitution above.
2. **Do not touch `wait_for`** — both its `tokio::process::Command` calls stay.
3. **Do not touch anything above `read_pane_exit_status`** — slice 3a is done,
   including the two `std::process::`-prefixed sites.
4. **Run `cargo fmt --all`** — mandatory; this project has no `format_fix` hook.
5. `cargo build` after the file.

## Acceptance criteria

- [ ] `grep -c "\.output()" src/tmux/pane.rs` returns **1** (printed **15**
      before). **Not 0** — see Hazard 1.
- [ ] `grep -n "\.output()" src/tmux/pane.rs` reports a single line, and that
      line is inside `wait_for`, immediately above a `.await;`.
- [ ] `grep -c "bounded_output(" src/tmux/pane.rs` returns **29**.
- [ ] `grep -c "tokio::process::Command" src/tmux/pane.rs` returns **2** —
      `wait_for` untouched.
- [ ] `grep -c 'std::process::Command::new("tmux")' src/tmux/pane.rs` returns
      **2** — **unchanged**; those are 06u's sites.
- [ ] `grep -cF "std::process::crate::" src/tmux/pane.rs` returns **0**.
- [ ] `grep -rn "\.output()" src/tmux/` reports **exactly one line**, in
      `pane.rs` — the whole directory's residue. *(This is the criterion that
      closes stage A.)*
- [ ] `grep -c "bounded_output(" src/tmux/session.rs` returns **9** and
      `grep -c "bounded_output(" src/tmux/window.rs` returns **6** — both
      unchanged.
- [ ] `grep -cF "pub fn bounded_output_with(" src/tmux/mod.rs` returns **1** —
      the helper was not modified.
- [ ] `git diff --name-only -- src/` lists exactly **one** file:
      `src/tmux/pane.rs`.
- [ ] `git diff --name-only | grep -c Cargo` returns **0**.
- [ ] `grep -cE '#\[allow|unsafe' src/tmux/pane.rs` returns **0**.
- [ ] `git diff -U0 src/tmux/pane.rs | grep '^+' | grep -cE 'unwrap\(\)|expect\(|panic!'`
      returns **0** — no collapse smuggled in via a new unwrap.
- [ ] `git diff -U0 src/tmux/pane.rs | grep '^+' | grep -cE '\basync\b|\.await'`
      returns **0** — this phase adds no async.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **921** lib-unit and **27** integration tests —
      **unchanged**. This phase adds no tests.

**Run every gate bare** — a command piped through `tail` exits with `tail`'s
status. Every number above was produced by running that exact command against a
tree with this change applied.

## Test plan

`bounded_output` is covered by the five tests in `src/tmux/mod.rs`, including the
1 MiB pipe-buffer regression test. **This phase adds no tests**: it changes which
function these 14 sites call, not what they compute, and every one needs a live
tmux server.

**The suite must stay at 921 lib tests.** If any test needs editing, **stop and
report a blocker**.

Two reasoning checks. **Quote the code — a claim without a quotation is not an
answer:**

1. **The residue.** Quote the one surviving `.output()` call and state in one
   sentence why it is not a conversion target.
2. **No collapse.** Quote `pane_exists` as you left it and state in one sentence
   why `.map(|o| o.status.success()).unwrap_or(false)` needed no change.

## End-to-end verification

Not applicable — this phase ships no new runtime-loadable artifact. It redirects
14 existing call sites to a helper whose timeout behaviour is covered by its own
tests. **Do not attempt a live-tmux demonstration.**

## Authorizations

- [x] May edit `src/tmux/pane.rs` — **the 14 `.output()` sites from
      `read_pane_exit_status` to the end of the file only.**
- [x] May let `cargo fmt --all` reflow those call sites.
- [ ] **No** change to any surrounding `?`, `.ok()?`, `let _ =`, or
      `.map(…).unwrap_or(false)`.
- [ ] **No** edit above `/// Read the last exit status recorded by the shell
      hook` — that is 06u's slice, already done.
- [ ] **No** change to `wait_for` or either `tokio::process::Command` call.
- [ ] **No** change to `src/tmux/mod.rs`, `src/tmux/session.rs`, or
      `src/tmux/window.rs`.
- [ ] **No** new dependency, no new tests, no `#[allow(...)]`.
- [ ] **No** signature change to any function.

## Out of scope

- **`wait_for`** — already async and already bounded; never a target.
- **The two `Drop` impls (`FgHookGuard`, `WatchHookGuard`) and the raw tmux
  spawns in `src/cli/`.** They call `std::process::Command::new("tmux")`
  **directly** rather than through a `src/tmux/` helper, so bounding the helpers
  does not reach them. That is a separate phase — see the note below.

### ⚠ Traps

1. **Residue is 1, not 0.** `wait_for` is `tokio::process`, already bounded, and
   would not compile through `bounded_output` anyway.
2. **No collapse.** The return type is unchanged; `?` stays `?`, `.ok()?` stays
   `.ok()?`, `.map(…).unwrap_or(false)` stays as it is.
3. **`std::process::Command::new("tmux")` must stay at 2.** Those are 06u's
   converted sites, above the boundary. Do not re-touch them.
4. **Let `fmt` reflow** — including `pane_exists`, where the trailing chain
   de-indents to column 4. Run `cargo fmt --all`; do not hand-format.
5. **The suite stays at 921.**

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-29 18:03 (started)

**Executor:** Claude
**Action:** Started phase 06v — converting the remaining 14 `.output()` sites in `src/tmux/pane.rs` to `bounded_output`.

### Update — ts=1785348641855 (complete, server-authored)

**Summary:** **Summary + Notes for review:**

Converted all 14 remaining `.output()` call sites in `src/tmux/pane.rs` (from `read_pane_exit_status` through `delete_buffer`) to `crate::tmux::bounded_output()`. Every surrounding expression (`?`, `.ok()?`, `let _ =`, `.map(|o| o.status.success()).unwrap_or(false)`) was preserved byte-identically — no collapse was introduced. The single remaining `.output()` call is inside `wait_for` at line 560, which uses `tokio::process::Command` and is already bounded by `tokio::time::timeout`, so it correctly stays untouched. All acceptance criteria verified: `pane.rs` has 1 `.output()` and 29 `bounded_output(` calls; `src/tmux/` has exactly one `.output()` line total (the `wait_for` residue); `session.rs` at 9, `window.rs` at 6, both unchanged. `cargo build`, `clippy`, `fmt`, and `cargo test` all pass clean. The working tree is committed and clean.

**Executor:** Qwen/Qwen3.6-27B-FP8

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
::tests::auth_empty_secret_always_allows ... ok
test webhook::server::tests::auth_token_without_bearer_prefix_denies ... ok
test webhook::server::tests::auth_wrong_token_denies ... ok
test tmux::cache::tests::get_labeled_context_session_topology ... ok
test tmux::cache::tests::get_labeled_context_pane_classification ... ok
test tmux::cache::tests::get_labeled_context_synchronized_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test tmux::cache::tests::get_labeled_context_chat_pane_excluded_from_background ... ok
test tmux::bounded_output_tests::bounded_output_times_out_and_kills_the_child ... ok
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

test result: ok. 921 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.28s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 29 tests
test daemon_ping_status_loop ... ignored
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test window_switch_does_not_corrupt_chat ... ignored
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_ask_round_trip ... ok
test minimal_config_parsing ... ok
test event_log_append_read ... ok
test ghost_config_parsing ... ok
test schedule_store_persistence ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test g4_briefing_read_and_clear ... ok
test config_pricing_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test g6_agent_config_roundtrip ... ok
test g4_briefing_masking_applied ... ok
test g6_agent_namespace_field_persisted ... ok
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
- `docs/dev/milestones/M5-ux-stability/phase-06v-bounded-output-pane-2.md` — +6 -1
- `src/tmux/pane.rs` — +67 -45

**Commit:** 8dd0885c2e36fedf33753f56e770625acb355069

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

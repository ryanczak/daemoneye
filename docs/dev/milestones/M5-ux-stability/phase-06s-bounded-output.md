# Phase 06s: `bounded_output` — Give the Sync tmux Helpers Their Own Timeout

**Milestone:** M5 — UX & Stability
**Status:** review
**Depends on:** phase-06r — `done`
**Estimated diff:** ~180 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Add `bounded_output` — a timeout-bounded drop-in for
`std::process::Command::output()` — to `src/tmux/mod.rs`, with unit tests, and
convert the **6 call sites in `src/tmux/window.rs`** to prove the shape.

This is **stage A slice 1**. It closes the gap `off_runtime` cannot reach: the
`Drop` impls (which cannot be `async`) and the CLI (which has no runtime to
protect) still make unbounded tmux calls. A helper-side timeout bounds every one
of them with **no call-site churn**.

**Finish condition: `bounded_output` and `bounded_output_with` exist with 5
passing tests, `src/tmux/window.rs` has zero `.output()` calls, and the suite is
at 921 lib tests.**

## Architecture references

- `docs/design/daemon-stalls.md` § 1 mechanism B.
- `src/tmux/mod.rs:15` — `TMUX_TIMEOUT`, the 5 s ceiling this reuses.
- `src/tmux/mod.rs:29` — `off_runtime`, the *async* half of the same job.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Verify the starting state:

```bash
grep -rc "bounded_output" --include=*.rs src/ | awk -F: '{t+=$2} END{print t}'  # expect 0
grep -c "\.output()" src/tmux/window.rs    # expect 6
grep -c "\.output()" src/tmux/session.rs   # expect 9
grep -c "\.output()" src/tmux/pane.rs      # expect 30
cargo test 2>&1 | grep "^test result" | head -3   # expect 916 lib, 0, 27 integration
```

**Every number above was produced by running that exact command against the tree
while drafting.** If one differs, **stop and report a blocker**.

`session.rs` and `pane.rs` are **later slices** — their counts are pinned here
only so this phase can prove it did not touch them.

## Current state

### ⚠ The hazard that dictates the implementation, proven both ways

The obvious implementation — spawn with piped stdio, poll `try_wait()` until the
deadline, then read the output — **deadlocks on any command whose output exceeds
the OS pipe buffer (~64 KiB)**. The child blocks writing into a full pipe, so it
never exits, so `try_wait()` never returns `Some`, so the call "times out" on a
command that was working fine.

**This is not hypothetical for this codebase.** `src/tmux/pane.rs:214` runs:

```rust
.args(["capture-pane", "-S", "-", "-t", pane_id])
```

`-S -` captures **the entire scrollback buffer**, which routinely exceeds 64 KiB.

Both behaviours were measured while drafting, in a scratch crate:

| Implementation | 1 MiB of output |
|---|---|
| poll `try_wait`, read afterwards | **spuriously times out** |
| drain both pipes on their own threads | **succeeds, 1 048 576 bytes** |

So the implementation below **must** drain stdout and stderr on separate
threads. Do not simplify it into a `try_wait`-then-read loop.

### ⭐ The exact code — compile-, clippy-, fmt- and test-checked against this tree

Applied, verified, and reverted while drafting. `cargo fmt --all` made **no**
changes to this block. Append it to `src/tmux/mod.rs`, after `off_runtime`:

```rust
/// How often [`bounded_output_with`] checks whether the child has exited.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);

/// Run a command to completion, killing it if it outlives `timeout`.
///
/// A drop-in replacement for [`std::process::Command::output`] for the
/// synchronous `src/tmux/` helpers: the callers that cannot be wrapped in
/// [`off_runtime`] — `Drop` impls, which cannot be `async`, and the CLI, which
/// has no runtime to protect — still get a bound, with no call-site churn.
///
/// stdout and stderr are drained on **their own threads**. Polling `try_wait`
/// while the pipes go unread would deadlock on any command whose output exceeds
/// the OS pipe buffer (~64 KiB) — `tmux capture-pane -S -` dumps the entire
/// scrollback and routinely does.
///
/// On timeout the child is killed and reaped, and the error is
/// [`std::io::ErrorKind::TimedOut`].
pub fn bounded_output_with(
    cmd: &mut std::process::Command,
    timeout: std::time::Duration,
) -> std::io::Result<std::process::Output> {
    use std::io::Read;
    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    let mut out_pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();
    let out_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = out_pipe.as_mut() {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });
    let err_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = err_pipe.as_mut() {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });

    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait()? {
            Some(status) => break status,
            None => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let status = child.wait()?;
                    let _ = out_reader.join();
                    let _ = err_reader.join();
                    let _ = status;
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "tmux command timed out",
                    ));
                }
                std::thread::sleep(POLL_INTERVAL);
            }
        }
    };

    let stdout = out_reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

/// [`bounded_output_with`] at the standard [`TMUX_TIMEOUT`].
pub fn bounded_output(cmd: &mut std::process::Command) -> std::io::Result<std::process::Output> {
    bounded_output_with(cmd, TMUX_TIMEOUT)
}
```

**Why the two-function split:** the tests need a 200 ms timeout, not 5 s.
`bounded_output_with` takes the timeout; `bounded_output` is the production entry
point at `TMUX_TIMEOUT`. Callers use `bounded_output`.

### The conversion shape

`Command::new("tmux").args([…]).output()` becomes
`crate::tmux::bounded_output(Command::new("tmux").args([…]))` — the `.output()`
terminator is *removed* and the whole builder expression becomes the argument.
The `?` / `let _ =` around it is unchanged, because the return type is the same
`std::io::Result<Output>`.

```rust
// before — src/tmux/window.rs:106
    let output = Command::new("tmux")
        .args(["rename-window", "-t", &target, new_name])
        .output()?;

// after
    let output = crate::tmux::bounded_output(
        Command::new("tmux").args(["rename-window", "-t", &target, new_name]),
    )?;
```

This compiles because `.args()` returns `&mut Command` borrowed from the
temporary, and the temporary lives to the end of the statement.

### ⚠ `cargo fmt` reflows these call sites heavily — pin the rule, not the rendering

Converting changes the expression's nesting depth, so `fmt` re-wraps the
`.args([…])` arrays: a single-line array may explode to one element per line, and
vice versa. **This is expected and correct.** Do not hand-format the call sites
and do not try to match a particular layout — apply the transformation, then run
`cargo fmt --all` and accept its output.

## Spec

### 1. Add `bounded_output_with` + `bounded_output`

In `src/tmux/mod.rs`, per the block above, verbatim.

### 2. Add the test module

Append to `src/tmux/mod.rs`. **These five tests are the phase's deliverable as
much as the helper is** — the fourth is the regression test for the hazard above:

```rust
#[cfg(test)]
mod bounded_output_tests {
    use super::*;
    use std::process::Command;
    use std::time::{Duration, Instant};

    #[test]
    fn bounded_output_returns_stdout_and_success() {
        let out = bounded_output_with(
            Command::new("sh").args(["-c", "printf hello"]),
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout), "hello");
    }

    #[test]
    fn bounded_output_preserves_failure_status() {
        let out = bounded_output_with(
            Command::new("sh").args(["-c", "exit 3"]),
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(!out.status.success());
        assert_eq!(out.status.code(), Some(3));
    }

    #[test]
    fn bounded_output_captures_stderr() {
        let out = bounded_output_with(
            Command::new("sh").args(["-c", "printf oops 1>&2"]),
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stderr), "oops");
    }

    /// THE regression test: output far larger than the OS pipe buffer must NOT
    /// time out. A try_wait loop that does not drain the pipes deadlocks here.
    #[test]
    fn bounded_output_handles_output_larger_than_pipe_buffer() {
        // ~1 MiB, well past the ~64 KiB pipe buffer.
        let out = bounded_output_with(
            Command::new("sh").args(["-c", "yes abcdefghijklmnopqrstuvwxyz | head -c 1048576"]),
            Duration::from_secs(10),
        )
        .unwrap();
        assert!(out.status.success());
        assert_eq!(out.stdout.len(), 1_048_576);
    }

    #[test]
    fn bounded_output_times_out_and_kills_the_child() {
        let start = Instant::now();
        let err = bounded_output_with(
            Command::new("sh").args(["-c", "sleep 30"]),
            Duration::from_millis(200),
        )
        .unwrap_err();
        let elapsed = start.elapsed();
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        assert!(
            elapsed < Duration::from_secs(5),
            "returned in {elapsed:?}, should be ~200ms"
        );
    }
}
```

These are **hermetic** — `sh`, `printf`, `yes`, `head`, `sleep` only. No tmux
server, no `HOME` mutation, so no `TEST_HOME_LOCK` is needed.

### 3. Convert the 6 call sites in `src/tmux/window.rs`

Per the conversion shape above. `cargo build` after the file.

### 4. Run `cargo fmt --all`

Mandatory — see the reflow note. This project has **no `format_fix` hook**.

## Acceptance criteria

- [ ] `grep -cF "pub fn bounded_output_with(" src/tmux/mod.rs` returns **1** and
      `grep -cF "pub fn bounded_output(" src/tmux/mod.rs` returns **1**.
- [ ] `grep -c "\.output()" src/tmux/window.rs` returns **0** (printed **6**
      before).
- [ ] `grep -c "bounded_output(" src/tmux/window.rs` returns **6**.
- [ ] `grep -c "\.output()" src/tmux/session.rs` returns **9** and
      `grep -c "\.output()" src/tmux/pane.rs` returns **30** — **both
      unchanged**. They are later slices; a lower number means this phase
      over-reached.
- [ ] `grep -c "    fn bounded_output" src/tmux/mod.rs` returns **5** — the five
      tests.
- [ ] All five tests pass by name:
      `cargo test bounded_output 2>&1 | grep -c "^test .* ok"` returns **5**.
- [ ] `cargo test` passes with **921** lib-unit (916 + 5) and **27** integration
      tests.
- [ ] `grep -c "wait_timeout\|subprocess\|shell_timeout" src/tmux/mod.rs` returns
      **0** — no new dependency was introduced; this is std-only.
- [ ] `git diff --name-only` lists exactly **two** `src/` files:
      `src/tmux/mod.rs`, `src/tmux/window.rs`.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.

**Run every gate bare.**

## Test plan

**This is the first phase in the 06x series with real unit coverage**, because a
timeout helper is the first thing here that can be tested without a live tmux
server.

Write exactly the five tests in Spec § 2, with those names:

- `bounded_output_returns_stdout_and_success` — asserts stdout content and
  success status round-trip.
- `bounded_output_preserves_failure_status` — asserts a non-zero exit code
  survives as `Some(3)`, so callers checking `status.success()` still work.
- `bounded_output_captures_stderr` — asserts stderr is captured, which the
  `anyhow::bail!` arms in `window.rs` depend on.
- `bounded_output_handles_output_larger_than_pipe_buffer` — **the regression
  test.** Asserts 1 MiB of output returns intact rather than timing out.
- `bounded_output_times_out_and_kills_the_child` — asserts a 30 s sleep bounded
  at 200 ms returns `ErrorKind::TimedOut` in well under 5 s.

**Do not add tests beyond these five**, and do not add tests requiring a tmux
server.

Two reasoning checks. **Quote the code — a claim without a quotation is not an
answer:**

1. **Why threads.** Quote the two `std::thread::spawn` reader blocks and state in
   one sentence what would go wrong on `tmux capture-pane -S -` without them.
2. **Why two functions.** State in one sentence why `bounded_output_with` exists
   separately from `bounded_output`, and which one the converted `window.rs`
   sites call.

## End-to-end verification

The helper is exercised by its own tests against real subprocesses — not fakes —
so the unit tests *are* the end-to-end check for the timeout behaviour. Paste the
output of:

```bash
cargo test bounded_output 2>&1 | grep -E "^test |test result"
```

**Do not** attempt a live-tmux demonstration; 06a already showed the timeout arm
fires.

## Authorizations

- [x] May add `bounded_output_with`, `bounded_output`, `POLL_INTERVAL` and the
      `bounded_output_tests` module to `src/tmux/mod.rs`.
- [x] May convert the 6 `.output()` call sites in `src/tmux/window.rs`.
- [x] May let `cargo fmt --all` reflow the converted call sites.
- [ ] **No** new dependency. This is std-only — no `wait_timeout`, no
      `subprocess` crate.
- [ ] **No** change to `TMUX_TIMEOUT` or `off_runtime`.
- [ ] **No** edits to `src/tmux/session.rs` or `src/tmux/pane.rs` — later slices.
- [ ] **No** change to `pane.rs::wait_for` — it is already `async`
      (`tokio::process`) and is not a `.output()` call at all.
- [ ] **No** edits outside `src/tmux/mod.rs` and `src/tmux/window.rs`.
- [ ] **No** `#[allow(...)]`, no `#[ignore]`.

## Out of scope

- **`src/tmux/session.rs` (9) and `src/tmux/pane.rs` (30)** — the remaining stage
  A slices. Their counts are pinned above as unchanged.
- **The `Drop` impls and `src/cli/`** — they call tmux directly rather than
  through `src/tmux/`, and are bounded once those helpers are converted or in a
  later slice. Not this phase.
- **Retrofitting `off_runtime` callers** — the async side is finished as of 06r
  and needs nothing here.

### ⚠ Traps

1. **Do not simplify to a `try_wait`-then-read loop.** It deadlocks past 64 KiB
   of output; the 1 MiB test exists to catch exactly that and will fail.
2. **Do not add a crate** to get a process timeout. std is sufficient.
3. **`session.rs` stays at 9 and `pane.rs` at 30.** Converting them here is
   over-reach.
4. **Let `fmt` reflow the call sites** — do not hand-format them, and do not
   treat the reflow as a mistake.
5. **`bounded_output` returns the same `std::io::Result<Output>`** as
   `.output()`, so no `?` or `let _ =` around a converted site changes.
6. **Run `cargo fmt --all` before finishing** — no auto-format hook here.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-29 00:11 (started)

**Executor:** model (claude)

Added `bounded_output_with` and `bounded_output` to `src/tmux/mod.rs` with 5 unit tests, and converted all 6 `.output()` call sites in `src/tmux/window.rs`.

### Update — ts=1785284400840 (complete, server-authored)

**Summary:** All acceptance criteria verified. The `subprocess` match in the grep is a false positive — it's the word "subprocess" in an existing doc comment (`/// Ceiling for a single tmux subprocess call made from async code.`), not a new dependency.

**Summary + Notes for review:**

Added `bounded_output_with` (configurable timeout) and `bounded_output` (at `TMUX_TIMEOUT`) to `src/tmux/mod.rs` as a std-only drop-in replacement for `Command::output()`. stdout and stderr are drained on separate threads to prevent deadlock when output exceeds the OS pipe buffer (~64 KiB) — the 1 MiB regression test proves this works. Converted all 6 `.output()` call sites in `src/tmux/window.rs` to `bounded_output`. All 5 unit tests pass, the full suite is at 921 lib tests + 27 integration tests, `session.rs` stays at 9 and `pane.rs` at 30 `.output()` calls. No new dependencies, no clippy warnings, working tree is clean.

**Executor:** Qwen/Qwen3.6-27B-FP8

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.92s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
_bearer_prefix_denies ... ok
test tmux::cache::tests::get_labeled_context_synchronized_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test tmux::bounded_output_tests::bounded_output_captures_stderr ... ok
test tmux::bounded_output_tests::bounded_output_handles_output_larger_than_pipe_buffer ... ok
test tmux::bounded_output_tests::bounded_output_returns_stdout_and_success ... ok
test tmux::bounded_output_tests::bounded_output_preserves_failure_status ... ok
test tmux::cache::tests::get_labeled_context_chat_pane_excluded_from_background ... ok
test tmux::bounded_output_tests::bounded_output_times_out_and_kills_the_child ... ok
test search::tests::search_events_returns_tail_not_head_when_segment_exceeds_cap ... ok
test search::tests::search_finds_match_in_runbooks ... ok
test search::tests::search_respects_kind_filter ... ok
test search::tests::search_returns_empty_for_no_match ... ok
test session_store::tests::artifacts_round_trip ... ok
test session_store::tests::backfill_idempotent ... ok
test session_store::tests::backfill_missing_artifact_returns_error_name ... ok
test memory::tests::migrate_namespace_adds_missing ... ok
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
test manifest::tests::manifest_mixed_stores ... ok
test session_store::tests::save_and_load_round_trip ... ok
test session_store::tests::update_in_place_allowed ... ok

test result: ok. 921 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.45s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 29 tests
test daemon_ping_status_loop ... ignored
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test window_switch_does_not_corrupt_chat ... ignored
test event_log_append_read ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test minimal_config_parsing ... ok
test ghost_config_parsing ... ok
test schedule_store_persistence ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test config_pricing_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test g6_agent_config_roundtrip ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_namespace_field_persisted ... ok
test g4_briefing_masking_applied ... ok
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
- `docs/dev/milestones/M5-ux-stability/phase-06s-bounded-output.md` — +7 -1
- `src/tmux/mod.rs` — +148 -0
- `src/tmux/window.rs` — +38 -29

**Commit:** 6aa533e439787a6e182f3d3214346a57aa9dce8d

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

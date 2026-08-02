# Phase 01: Bound `read_key` in the tty tests

**Milestone:** M10 — Residual Hygiene
**Status:** review
**Depends on:** none (first phase of M10; M9 closed 2026-08-02)
**Estimated diff:** ~45 lines, all inside the `#[cfg(test)] mod tests` block of
`src/cli/input/tty.rs`. **No production code changes.**

## Goal

Make a regression that starves `read_key` **fail** the tty tests instead of
**hanging** them.

Today all ten tty tests call `read_key(&stdin).await` directly. If a future change
stops bytes reaching it, the test does not fail — it waits forever. In CI a hang
burns the whole job budget and reports nothing.

## Read this first — the fix does NOT go in `read_key`

`src/cli/input/tty.rs:161` reads its **first** byte with no timeout:

```rust
pub async fn read_key(stdin: &AsyncStdin) -> Option<Key> {
    use tokio::time::{Duration, timeout};

    let b = stdin.read_byte().await?;        // <-- line 164, unbounded, and CORRECT
    Some(match b {
        b'\r' | b'\n' => Key::Enter,
        // ...
        b'\x1b' => {
            match timeout(Duration::from_millis(30), stdin.read_byte()).await {
```

Every *subsequent* read is bounded at 30 ms so a lone Escape is distinguishable
from a CSI sequence. The first one is deliberately unbounded.

**Do NOT add a timeout to `read_key`, and do NOT touch line 164.** Production
awaits it inside a `tokio::select!` — `src/cli/commands/stream.rs:686`:

```rust
tokio::select! {
    key = read_key(stdin) => {
        if let Some(key) = key {
            match interrupt_state.feed(&key) { /* ... */ }
        }
        continue;
    }
    res = recv_line(rx, buf) => { /* daemon message */ }
    _ = to => { /* overall timeout */ }
    _ = tokio::time::sleep(tick_interval), if tick_interval != Duration::MAX => {
        return StreamOutcome::Tick;
    }
}
```

The unbounded wait for the first byte is exactly how the chat loop waits for the
user to type while racing daemon messages and ticks. A timeout inside `read_key`
would make it return spuriously — and since `None` already means EOF, the loop
could not distinguish "the user is thinking" from "the terminal closed."

**The bound belongs in the tests.** That is this entire phase.

## Current state

`src/cli/input/tty.rs` is 501 lines. `#[cfg(test)] mod tests` starts at line
**332**. Measured against the tree on 2026-08-02:

| Fact | Value |
|---|---|
| Bare `read_key(&stdin).await` call sites in the test module | **10** |
| `from_millis(30)` occurrences (all production) | **10** |
| Line 164 | `    let b = stdin.read_byte().await?;` |
| `cargo test --lib` | **1035** passed |
| `cargo test --lib cli::input::tty` | **10** passed |

The hang was verified by mutation before this phase was written: replacing the
`write_bytes(...)` call in `read_key_bare_cr_yields_enter` with nothing makes the
test hang; it was killed externally at 25 s.

The existing test helper the new one sits beside (`tty.rs:338`):

```rust
    /// Create a pipe and return an AsyncStdin reading from the read end.
    fn make_pipe_stdin() -> (AsyncStdin, std::fs::File) {
        // pipe2 with O_NONBLOCK on both ends
        let mut fds: [libc::c_int; 2] = [-1, -1];
        let ret = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_NONBLOCK) };
        assert_eq!(ret, 0, "pipe2 failed: {}", std::io::Error::last_os_error());
        // ...
        (stdin, write_file)
    }
```

And a representative test as it stands today (`tty.rs:383`):

```rust
    #[tokio::test]
    async fn read_key_bare_cr_yields_enter() {
        let (stdin, write_file) = make_pipe_stdin();
        // Write a bare CR
        write_bytes(&write_file, b"\r").await;
        let key = read_key(&stdin).await;
        assert_eq!(key, Some(Key::Enter), "bare CR should yield Enter");
    }
```

## Spec

All work is inside `#[cfg(test)] mod tests` in `src/cli/input/tty.rs`.

### Task 1 — add the bounded helpers

Add these next to `write_bytes` (after it, before the first `#[tokio::test]`):

```rust
    /// How long a test will wait for `read_key` before declaring the read starved.
    ///
    /// Generous on purpose: it is only ever paid when something is already broken,
    /// so a slow machine must not trip it.
    const KEY_READ_BOUND: std::time::Duration = std::time::Duration::from_secs(5);

    /// `read_key`, but a starved read panics instead of hanging the suite.
    ///
    /// `read_key`'s first `read_byte()` is deliberately unbounded — production
    /// awaits it in a `select!` while the user thinks. That is correct there and
    /// fatal here: a regression that stops bytes reaching it would hang CI rather
    /// than fail it.
    async fn read_key_bounded(stdin: &AsyncStdin) -> Option<Key> {
        read_key_within(stdin, KEY_READ_BOUND).await
    }

    /// `read_key_bounded` with an explicit bound, so the guard itself is testable.
    async fn read_key_within(stdin: &AsyncStdin, bound: std::time::Duration) -> Option<Key> {
        match tokio::time::timeout(bound, read_key(stdin)).await {
            Ok(key) => key,
            Err(_) => panic!("read_key did not return within {bound:?} — no byte reached it"),
        }
    }
```

### Task 2 — route every test through the helper

Replace **all 10** occurrences of `read_key(&stdin).await` in the test module
with `read_key_bounded(&stdin).await`. Nothing else about those tests changes —
same assertions, same names, same byte sequences.

Afterwards `grep -c 'read_key(&stdin).await' src/cli/input/tty.rs` must be **0**
(the helper calls `read_key(stdin)` without the `&`, so it does not match).

### Task 3 — prove the guard actually fires

Add this test. It uses a 50 ms bound so it costs 50 ms, not 5 s:

```rust
    #[tokio::test]
    #[should_panic(expected = "read_key did not return within")]
    async fn read_key_within_panics_when_no_byte_ever_arrives() {
        // `_write_file` MUST stay bound: holding the pipe's write end open is what
        // makes the read block. Dropping it closes the pipe and `read_key` returns
        // `None` at once (EOF), which would pass this test for the wrong reason.
        let (stdin, _write_file) = make_pipe_stdin();
        let _ = read_key_within(&stdin, std::time::Duration::from_millis(50)).await;
    }
```

**This is the pinned negative case, and it is measured, not guessed:**

| Write end | `timeout(50ms, read_key(&stdin))` |
|---|---|
| Held (`_write_file`) | `Err(Elapsed)` → the helper panics → test passes |
| Dropped (bare `_`) | `Ok(None)` → no panic → **`should_panic` test FAILS** |

Both rows were run against this tree. Do not "simplify" `_write_file` to `_`.

## Acceptance criteria

- [ ] `cargo test --lib` reports **1036** passed — exactly one more than the 1035
      baseline. **1037+ means scope creep**; 1035 means the guard test is missing.
- [ ] `cargo test --lib cli::input::tty` reports **11** passed.
- [ ] `grep -c 'read_key(&stdin).await' src/cli/input/tty.rs` is **0**.
- [ ] `grep -c 'read_key_bounded(&stdin).await' src/cli/input/tty.rs` is **10**.
- [ ] **Production is untouched**: `sed -n '164p' src/cli/input/tty.rs` still
      prints `    let b = stdin.read_byte().await?;`, and
      `grep -c 'from_millis(30)' src/cli/input/tty.rs` is still **10**.
- [ ] `git diff -- src/cli/input/tty.rs` contains **no** changed line above the
      `#[cfg(test)]` marker.
- [ ] Only `src/cli/input/tty.rs` changes (plus this phase doc).
- [ ] `cargo fmt --all --check`, `cargo build`, and `cargo clippy --all-targets
      --all-features -- -D warnings` all clean.

## Test plan

- `read_key_within_panics_when_no_byte_ever_arrives` — the new guard test (Task 3).
- The 10 existing tty tests must still pass **unchanged in behavior**; they only
  change which helper they call.

**Mutation-check your own work before reporting complete**, and state the result:

1. In `read_key_within`, replace the `Err(_) => panic!(...)` arm with
   `Err(_) => None`. Confirm `read_key_within_panics_when_no_byte_ever_arrives`
   now **FAILS** (it should report that the panic did not occur).
2. Revert. Confirm it passes again.

A guard that cannot be shown to fire is not a guard.

## End-to-end verification

Paste the transcript of this block into the Update Log:

```sh
# 1. the guard fires, and costs ~50ms not 5s
cargo test --lib read_key_within_panics_when_no_byte_ever_arrives -- --nocapture 2>&1 | tail -3

# 2. no bare call sites remain; all ten go through the helper
echo "bare:    $(grep -c 'read_key(&stdin).await' src/cli/input/tty.rs)   # must be 0"
echo "bounded: $(grep -c 'read_key_bounded(&stdin).await' src/cli/input/tty.rs)   # must be 10"

# 3. production untouched
echo "line164: [$(sed -n '164p' src/cli/input/tty.rs)]"
echo "30ms:    $(grep -c 'from_millis(30)' src/cli/input/tty.rs)   # must be 10"

# 4. counts
cargo test --lib cli::input::tty 2>&1 | grep 'test result'   # must be 11 passed
cargo test --lib 2>&1 | grep 'test result' | head -1          # must be 1036 passed

# 5. gates
cargo fmt --all --check && echo "fmt ok"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -2
```

## Authorizations

- Edit `src/cli/input/tty.rs` **below** the `#[cfg(test)]` marker at line 332.
- Add exactly one test.

## Out of scope

- **Any change to production code**, in this file or any other. Specifically: do
  not add a timeout to `read_key`, do not touch line 164, do not alter the 30 ms
  inter-byte timeouts, and do not modify `src/cli/commands/stream.rs`.
- The other three M10 items — the `src/ai/mod.rs:364` sleep, the
  `epochs.rs:618` hardcoded table, and the `reindex` documentation. They are
  phases 02 and 03.
- Renaming or restructuring the existing ten tests.
- `tests/isolation.rs` and the harness — untouched by this phase.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-08-02 17:11 (started)

**Executor:** Claude executor

Added `read_key_bounded` and `read_key_within` helpers, routed all 10 existing test call sites through `read_key_bounded`, and added `read_key_within_panics_when_no_byte_ever_arrives` guard test.

### Update — ts=1785691190835 (complete, server-authored)

**Summary:** All tasks complete. Here's the summary:

**What was built:** Added `read_key_bounded` and `read_key_within` test helpers that wrap `read_key` in a `tokio::time::timeout`, so a starved read panics instead of hanging CI. Routed all 10 existing tty test call sites through `read_key_bounded`. Added `read_key_within_panics_when_no_byte_ever_arrives` to prove the guard fires (~50ms).

**Verification results:**
- `cargo test --lib`: **1036 passed** (exactly one more than baseline)
- `cargo test --lib cli::input::tty`: **11 passed**
- `grep -c 'read_key(&stdin).await'`: **0** (no bare call sites remain)
- `grep -c 'read_key_bounded(&stdin).await'`: **10** (all routed through helper)
- Line 164 unchanged (`stdin.read_byte().await?`), `from_millis(30)` count still **10** — production untouched
- `cargo fmt --all --check` and `cargo clippy` both clean

**Mutation check confirmed:** Replacing the `panic!` arm with `None` caused `read_key_within_panics_when_no_byte_ever_arrives` to fail ("test did not panic as expected"). Reverted — test passes again.

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
eturns_newest_first ... ok
test session_store::tests::load_messages_max_count_truncates ... ok
test session_store::tests::rename_nonexistent_errors ... ok
test session_store::tests::rename_to_existing_errors ... ok
test session_store::tests::rename_updates_dir_and_index ... ok
test session_store::tests::save_and_load_round_trip ... ok
test session_store::tests::update_in_place_allowed ... ok
test test_home_guard_tests::guard_restores_home_on_drop ... ok

test result: ok. 1036 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.22s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_reads_bare_word ... ok
test header_status_strips_trailing_prose ... ok
test header_status_uses_first_occurrence_only ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 1 test
test docs_do_not_carry_retired_index_claims ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 32 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_allow_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test event_log_entry_format ... ok
test ipc_ask_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ghost_config_parsing ... ok
test minimal_config_parsing ... ok
test ipc_session_info_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test schedule_store_persistence ... ok
test event_log_append_read ... ok
test config_pricing_round_trip ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_jsonl_round_trip ... ok
test g4_briefing_masking_applied ... ok
test session_index_persistence ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_no_severity_passes_gate ... ok

test result: ok. 30 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.02s


running 10 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test config_contains_webhook_and_stub_url ... ok
test hooks_land_on_private_server ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.15s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.07s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-b60224cb24515ede)
     Running unittests src/main.rs (target/debug/deps/daemoneye-e700f2084319867a)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-9b22636ef5c08466)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-c00c74ef4ffe9c11)
     Running tests/integration.rs (target/debug/deps/integration-6230826c10f36795)
     Running tests/isolation.rs (target/debug/deps/isolation-66949bca409172a9)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M10-residual-hygiene/README.md` — +1 -1
- `docs/dev/milestones/M10-residual-hygiene/phase-01-read-key-test-bound.md` — +7 -1
- `src/cli/input/tty.rs` — +44 -10

**Commit:** 8bcf1c7305ceca1e377c94a79e21c83269266fad

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

# Phase 01: Bound `read_key` in the tty tests

**Milestone:** M10 — Residual Hygiene
**Status:** in-progress
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

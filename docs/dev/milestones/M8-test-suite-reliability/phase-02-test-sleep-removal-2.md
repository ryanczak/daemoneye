# Phase 02: Test Sleep Removal (2)

**Milestone:** M8 — Test Suite Reliability
**Status:** todo
**Depends on:** phase-01 (port-lifetime, done)
**Estimated diff:** ~30 lines — the same six-line helper in two files.

**Tags:** language=rust, kind=bugfix, size=s

## Goal

Four real-clock sleeps remain in non-`#[ignore]`d tests, which `STANDARDS.md`
§3.3 forbids. They are two copies of one helper. Remove them and finish M7's
single unticked exit criterion.

## Architecture references

- `src/cli/input/tty.rs:355-375` — the `write_bytes` test helper.
- `src/cli/commands/stream.rs:1251-1269` — a **byte-identical copy** of it.
- `docs/dev/milestones/M7-memory-search-and-maintenance/README.md` § exit
  criteria, item 8 — the criterion this closes, recorded there as partly-met.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any file.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

The same helper appears twice, byte for byte. `src/cli/input/tty.rs:355`:

```rust
/// Write bytes into the write file and wait a bit for them to be available.
async fn write_bytes(file: &std::fs::File, bytes: &[u8]) {
    let fd = file.as_raw_fd();
    let mut remaining = bytes;
    while !remaining.is_empty() {
        let n = unsafe {
            libc::write(fd, remaining.as_ptr() as *const libc::c_void, remaining.len())
        };
        if n > 0 {
            remaining = &remaining[n as usize..];
        } else {
            // EAGAIN is fine, just loop
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    // Give the async reader time to see the data
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
}
```

`src/cli/commands/stream.rs:1251` is the same function without the two comments.

Two distinct sleeps, and they need different treatment:

### The 10 ms wait is simply unnecessary — measured

`write()` returns once the bytes are in the pipe buffer, and the caller then
reads the same fd. There is nothing to wait for. Verified by deleting both and
running the affected modules:

```
cli::input::tty          failures: 0 / 30
cli::commands::stream    failures: 0 / 30
full lib suite:          1032 passed
```

**Delete it.** It is cargo-culted, not load-bearing.

### The 1 ms EAGAIN backoff needs replacing, not deleting

That branch runs when `write()` returns `<= 0`. Deleting the sleep outright turns
it into a busy-spin. Two problems to fix at once, both in six lines:

1. `std::thread::sleep` **blocks the tokio worker thread** inside an `async fn` —
   the wrong primitive even ignoring §3.3.
2. The comment says "EAGAIN is fine" but the code never checks. **Any** write
   error spins forever, so a real failure hangs the suite instead of reporting.

### Do NOT touch the production sleeps in `stream.rs`

`src/cli/commands/stream.rs` also contains `tokio::time::sleep` at **lines 681,
705 and 727**. Those are **production code** — the streaming loop's overall
timeout and its tick interval. They are correct, they are not tests, and
removing them would break streaming.

Only lines **1265** and **1268** are in the test helper. A blanket
"remove sleeps from stream.rs" is the way this phase goes wrong.

### Why this matters when the tests are already fast

The suite does not visibly slow down: the tty module runs in 0.02 s and the
stream module in 0.05 s today, because the 10 ms sleeps overlap across parallel
tests. **The argument is not speed — it is determinism.** A test that waits a
fixed 10 ms and hopes is the same class of defect phase 01 just removed from the
port allocator: fine on an idle laptop, intermittently wrong under CI load. It
survives because nobody notices a flake at low frequency.

## Spec

### 1. Replace the helper body — identically, in both files

In **both** `src/cli/input/tty.rs` and `src/cli/commands/stream.rs`, replace the
`else` branch and delete the trailing wait, so the loop becomes:

```rust
        if n > 0 {
            remaining = &remaining[n as usize..];
        } else {
            // A short write on this pipe can only mean EAGAIN; anything else is
            // a real bug and must fail loudly rather than spin forever.
            let err = std::io::Error::last_os_error();
            assert_eq!(
                err.kind(),
                std::io::ErrorKind::WouldBlock,
                "write to test pipe failed: {err}"
            );
            tokio::task::yield_now().await;
        }
    }
}
```

Three changes in that block, all required:

- `std::thread::sleep(1ms)` → `tokio::task::yield_now().await` — yields the
  worker instead of blocking it, and consumes no wall-clock time.
- An `assert_eq!` on `ErrorKind::WouldBlock` before yielding, so a genuine write
  error fails the test with the errno instead of hanging.
- The trailing `tokio::time::sleep(10ms)` and its
  `// Give the async reader time to see the data` comment are **deleted
  entirely**.

`ErrorKind::WouldBlock` is `EAGAIN`; no `libc::EAGAIN` comparison is needed.

Keep `write_bytes` `async` — `yield_now().await` requires it, and every caller
already `.await`s it.

Update the tty copy's doc comment, which currently promises a wait:

```rust
/// Write bytes into the write file. Returns once every byte is in the pipe
/// buffer; the reader sees them immediately, so no wait is needed.
```

### 2. No new tests

The existing tests **are** the coverage — ten in `cli::input::tty` and fourteen
in `cli::commands::stream`, all of which call `write_bytes`. If the replacement
were wrong they would fail.

**The test count must not change.** `cargo test` must report **1032** lib tests,
not 1033. A rising count means something was added that this phase did not ask
for.

## Acceptance criteria

- [ ] `grep -c "thread::sleep" src/cli/input/tty.rs` returns **0**, and the same
      for `src/cli/commands/stream.rs`.
- [ ] `grep -c "tokio::time::sleep" src/cli/input/tty.rs` returns **0** (it is
      currently 1, and tty.rs has no production `tokio::time::sleep`).
- [ ] **`src/cli/commands/stream.rs` still contains its three production
      `tokio::time::sleep` calls** — `grep -c "tokio::time::sleep"` returns
      exactly **3**, down from 4 (lines 681, 705, 727 survive; only the test
      helper's goes). Fewer than 3 means a production sleep was deleted.
- [ ] **Do not grep for `from_millis(10)`** as a proxy — `tty.rs` contains
      **five** production uses at lines 287-292, `timeout(Duration::from_millis(10),
      stdin.read_byte())` in the escape-sequence reader, which must survive.
      `grep -c "from_millis(10)" src/cli/input/tty.rs` must therefore end at
      **5**, not 0.
- [ ] Both helpers assert `ErrorKind::WouldBlock` before yielding.
- [ ] `cargo test --lib cli::input::tty` and `cargo test --lib
      cli::commands::stream` each pass **30 consecutive runs**, 0 failures.
- [ ] `cargo build` zero new warnings; `cargo clippy --all-targets
      --all-features -- -D warnings` exits 0; `cargo fmt --all` leaves the tree
      unchanged.
- [ ] `cargo test` green with the count **unchanged**: lib **1032**, integration
      **30** (2 ignored), isolation **9** (1 ignored), `bug_tracker` **6**,
      `doc_truth` **1**.
- [ ] Only `src/cli/input/tty.rs` and `src/cli/commands/stream.rs` change.

## Test plan

No new tests; see spec task 2. The verification is the **30-consecutive-run
loop** per module, for the same reason phase 01 used 200 runs: a timing change
that is wrong intermittently cannot be distinguished from a correct one by a
single green run.

**What would make this phase a false success:** deleting the `else` branch
entirely along with its sleep. The loop would then spin on `n <= 0` with no
backoff and no yield, which on a full pipe buffer inside a single-threaded
runtime **hangs forever**. Every test would pass on a laptop where the buffer
never fills. The `yield_now()` is what keeps the loop cooperative, and the
`assert_eq!` is what turns a real error into a failure instead of a hang.

A second: deleting `stream.rs`'s production sleeps at 681/705/727 while
"removing sleeps from stream.rs". The third acceptance criterion — exactly 3
remaining `tokio::time::sleep` in that file — is what catches it.

## End-to-end verification

Run this block verbatim and paste the resulting file into your Update Log.

**Two constraints carried from M7 phase-03's post-mortem:** **no heredocs**, and
every long-running command wrapped in `timeout`. An M7 E2E block nested a
`python3` heredoc that hung and orphaned two processes at 100% CPU for 70
minutes. Do not reintroduce either pattern.

```bash
cd /home/matt/src/daemoneye
cargo build --tests 2>&1 | tail -2
{
  echo "=== the four test sleeps are gone ==="
  timeout 30 grep -c "thread::sleep" src/cli/input/tty.rs
  echo "tty-thread-sleep-above-must-be-0"
  timeout 30 grep -c "thread::sleep" src/cli/commands/stream.rs
  echo "stream-thread-sleep-above-must-be-0"
  timeout 30 grep -c "tokio::time::sleep" src/cli/input/tty.rs
  echo "tty-tokio-sleep-above-must-be-0"

  echo "=== the PRODUCTION timeouts and sleeps survived ==="
  timeout 30 grep -c "from_millis(10)" src/cli/input/tty.rs
  echo "tty-from_millis10-above-must-be-exactly-5   # the escape-seq timeouts"
  timeout 30 grep -c "tokio::time::sleep" src/cli/commands/stream.rs
  echo "stream-tokio-sleep-above-must-be-exactly-3"

  echo "=== the errno assert is present in both ==="
  timeout 30 grep -c "ErrorKind::WouldBlock" src/cli/input/tty.rs
  timeout 30 grep -c "ErrorKind::WouldBlock" src/cli/commands/stream.rs

  echo "=== 30 consecutive runs per module ==="
  for m in cli::input::tty cli::commands::stream; do
    f=0
    for i in $(seq 1 30); do
      timeout 120 cargo test --lib "$m" > /tmp/sleep-run.txt 2>&1 || f=$((f+1))
    done
    echo "$m failures=$f   # 0 == PASS"
  done

  echo "=== full gate ==="
  timeout 900 cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -2
  echo "clippy-exit=$?"
  timeout 900 cargo test 2>&1 | grep -E "^test result"
} > /tmp/m8-phase02-e2e.txt 2>&1
cat /tmp/m8-phase02-e2e.txt
```

The lib line must read **1032**, not 1033 — this phase adds no tests.

Paste the captured file into an Update Log entry titled
`### Update — <date> (end-to-end verification)`. **The server-authored
`(complete)` entry does not satisfy this** — its "Command output tails" block is
the automatic gate capture every phase receives, and a single green run is
exactly what cannot validate a timing change.

**If any part of the capture block fails or hangs, stop and report it as a
blocker.** Do not re-run the surviving sections separately and paste the
result — a transcript assembled from more than one run fails `STANDARDS.md` §1
even when every claim in it is true.

## Authorizations

- [ ] May add dependencies: **none**. `tokio::task::yield_now` is already
      available.
- [ ] May touch `docs/architecture.md`: no.
- [ ] May touch `CLAUDE.md`: no.
- [ ] May create new files: no.

## Out of scope

- **The three production `tokio::time::sleep` calls in
  `src/cli/commands/stream.rs`** (lines 681, 705, 727). Correct as they are; an
  acceptance criterion pins their survival.
- **Deduplicating the two identical `write_bytes` helpers.** They live in two
  different `#[cfg(test)]` modules in different files; hoisting them into a
  shared test utility is a separate refactor with its own module-layout
  decision. Fix both copies identically here.
- **A gate that forbids real-clock sleeps in tests.** Attractive, and the
  obvious durable answer — but a correct scanner must distinguish production
  code from `#[cfg(test)]` regions and must exempt `#[ignore]`d tests, and the
  M7 close-out audit got that wrong twice before getting it right by hand. A
  naive grep gate would fire on `stream.rs:681` (production) and on the four
  legitimately-sleeping `#[ignore]`d tests in `tests/integration.rs` and
  `tests/isolation.rs`. Worth its own phase if the class recurs; a wrong gate is
  worse than none.
- **The sleeps inside `#[ignore]`d tests** — `tests/integration.rs:1746,1770,1778`
  and `tests/isolation.rs:591`. `STANDARDS.md` §3.3 permits them; all four were
  individually verified as `#[ignore]`d during the M7 close-out audit.
- **Any non-test code anywhere.** This phase touches two `#[cfg(test)]` modules.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

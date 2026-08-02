# Phase 01: Port Lifetime

**Milestone:** M8 — Test Suite Reliability
**Status:** todo
**Depends on:** none (first phase of M8; M7 closed 2026-08-02)
**Estimated diff:** ~110 lines — `tests/harness/mod.rs` (the allocator and two
hand-off sites) plus three one-word changes and one new test in
`tests/isolation.rs`.

**Tags:** language=rust, kind=bugfix, size=m

## Goal

`cargo test --test isolation` fails about **5% of the time**. Two
`IsolatedEnv`s can be handed the same TCP port, because the allocator releases
the port before its consumer binds it. Hold the listener until the consumer
takes over, so the collision becomes impossible rather than unlikely.

## Architecture references

- `tests/harness/mod.rs:325` `alloc_free_port()` — the defect.
- `tests/harness/mod.rs:88` — the stub bind that fails downstream of it.
- `tests/isolation.rs:176` `webhook_ports_differ_between_environments` — already
  a canary for this bug and one of its two observed failure sites. **Keep it.**

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any file.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

```rust
// tests/harness/mod.rs:325
fn alloc_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind to port 0");
    let port = listener.local_addr().expect("local addr").port();
    drop(listener);   // the port is free again from here
    port              // the caller binds it much later
}
```

Both ports are allocated up front in `IsolatedEnv::new()`
(`tests/harness/mod.rs:39-40`) and bound much later — the stub in `start_stub()`
(`:88`), the webhook by a **separate daemon process** spawned from
`start_daemon()` (`:196`). Between the two, the port belongs to nobody.

### This is measured, not inferred

A 100-run baseline of `cargo test --test isolation` on the current tree:

| Site | Count | What it is |
|---|---|---|
| `tests/harness/mod.rs:88` | 4 | `bind stub server: Os { code: 98, AddrInUse }` |
| `tests/isolation.rs:184` | 1 | `assert_ne!(env_a.stub_port(), env_b.stub_port())` |

**5 failures / 100 runs.** The second site is the proof: that assertion exists
solely to check two environments get different ports, and it failed — so two
`IsolatedEnv`s really were given the identical port. The `AddrInUse` failures are
the same collision seen one step later.

**Two hypotheses were disproven** before this; do not re-run them. The kernel
does *not* hand back a just-freed ephemeral port under tight sequential
allocation (0/200), and a simplified concurrent model of alloc-close-rebind did
not reproduce it either (0/480). The real suite runs nine tests in parallel while
spawning daemons that consume ephemeral ports of their own, and that is the
difference.

### The hand-off API works — verified by compiling and running it

```rust
let std_l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
let port = std_l.local_addr().unwrap().port();
std_l.set_nonblocking(true).unwrap();          // required, or from_std misbehaves
let tok = tokio::net::TcpListener::from_std(std_l).unwrap();
// PROBE handoff ok port=36003 local=127.0.0.1:36003
```

Measured too: eight concurrent allocations with the listeners **held** produced
**8/8 distinct ports**. Two live listeners cannot share a port, which is the
whole reason this fix works.

## Spec

### 1. Replace the allocator

```rust
/// Bind an ephemeral port and **keep the listener alive**, so no other caller
/// can be handed the same port. The listener is released only when its real
/// consumer takes over — see `start_stub` and `start_daemon`.
fn alloc_held_port() -> (std::net::TcpListener, u16) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind to port 0");
    let port = listener.local_addr().expect("local addr").port();
    (listener, port)
}
```

Delete `alloc_free_port`. Nothing else may call it.

### 2. Hold both listeners on `IsolatedEnv`

Add two fields; keep the two existing `u16` fields so `webhook_port()` and
`stub_port()` are unchanged for callers:

```rust
pub struct IsolatedEnv {
    root: TempDir,
    webhook_port: u16,
    webhook_listener: Option<std::net::TcpListener>,
    stub_handle: Option<tokio::task::JoinHandle<()>>,
    stub_port: u16,
    stub_listener: Option<std::net::TcpListener>,
    stub_response: Arc<std::sync::Mutex<String>>,
}
```

and in `new()`:

```rust
let (webhook_listener, webhook_port) = alloc_held_port();
let (stub_listener, stub_port) = alloc_held_port();
```

storing both listeners as `Some(..)`.

### 3. Hand the stub its listener — no rebind at all

In `start_stub()` (`tests/harness/mod.rs:80`), replace the
`tokio::net::TcpListener::bind(("127.0.0.1", port))` call at line 88 with a
hand-off of the already-bound listener:

```rust
let std_listener = self
    .stub_listener
    .take()
    .expect("stub listener already taken — start_stub called twice");
std_listener
    .set_nonblocking(true)
    .expect("set stub listener non-blocking");
let listener = tokio::net::TcpListener::from_std(std_listener)
    .expect("adopt stub listener");
```

The rest of `start_stub` is unchanged. **The port is never released**, so this
site can no longer produce `AddrInUse`.

Keep the existing comment's intent — the listener is bound before the task is
spawned, so no readiness sleep is needed. That is now even more true.

### 4. Release the webhook port immediately before spawning the daemon

The daemon is a **separate process** and binds the webhook port itself from
`config.toml`, so the test must let go first — the daemon's webhook bind is
eager and a failure there is fatal.

`start_daemon` currently takes `&self`. Change it to `&mut self` and drop the
held listener as late as possible — after `setup`, after `write_test_config()`,
immediately before the `daemoneye daemon` invocation:

```rust
pub fn start_daemon(&mut self, session: &str) -> std::process::Output {
    // … existing setup + write_test_config() …

    // Release the webhook port only now. Holding it through construction and
    // config-writing is what stops another IsolatedEnv being handed the same
    // port; the daemon binds it within milliseconds of this drop.
    drop(self.webhook_listener.take());

    let daemon_out = self
        .daemoneye(&["daemon", "--session", session])
        // … unchanged …
}
```

This does not close the window to zero for the webhook port — the daemon still
binds after the drop. It does guarantee that **no other `IsolatedEnv` in the
process was ever given that port**, which is the collision the baseline
measured.

### 5. Three call sites need `let mut`

Changing `start_daemon` to `&mut self` breaks three bindings in
`tests/isolation.rs` that are currently `let env = IsolatedEnv::new();` and then
call `env.start_daemon(...)` — at **lines 57, 105 and 150**. Change each to
`let mut env = IsolatedEnv::new();`.

The compiler names every one; do not hunt for them by eye. The other
`IsolatedEnv` bindings are already `let mut`.

### 6. Tests

- `held_port_cannot_be_rebound` — new, in `tests/isolation.rs`. Call
  `IsolatedEnv::new()`, then attempt
  `std::net::TcpListener::bind(("127.0.0.1", env.stub_port()))` and assert it
  **returns `Err`**. This is the direct proof that the listener is genuinely
  held; if the allocator ever reverts to dropping it, this fails immediately.

  ```rust
  #[test]
  fn held_port_cannot_be_rebound() {
      let env = IsolatedEnv::new();
      let err = std::net::TcpListener::bind(("127.0.0.1", env.stub_port()))
          .expect_err("stub port must still be held by the env");
      assert_eq!(
          err.kind(),
          std::io::ErrorKind::AddrInUse,
          "expected AddrInUse, got {err:?}"
      );
  }
  ```

- **Keep `webhook_ports_differ_between_environments` exactly as it is.** It is
  the canary that caught this bug and it must keep passing.

## Acceptance criteria

- [ ] `alloc_free_port` no longer exists; `grep -c "alloc_free_port"
      tests/harness/mod.rs` returns **0**.
- [ ] **`cargo test --test isolation` passes 100 consecutive runs, 0 failures.**
      The measured pre-fix baseline is 5/100, so this is the criterion that
      matters. Run it; do not assert it.
- [ ] `held_port_cannot_be_rebound` passes, and **fails if the listener is
      released** — demonstrate by making `alloc_held_port` drop its listener
      before returning, quoting the red run, then reverting.
- [ ] `webhook_ports_differ_between_environments` still exists and passes.
- [ ] `cargo build` zero new warnings; `cargo clippy --all-targets
      --all-features -- -D warnings` exits 0; `cargo fmt --all` leaves the tree
      unchanged.
- [ ] `cargo test` green: lib **1032** (unchanged), integration **30**
      (2 ignored), **isolation 9 (1 ignored)** — one more than the current 8,
      for the new test — `bug_tracker` **6**, `doc_truth` **1**.
- [ ] Only `tests/harness/mod.rs` and `tests/isolation.rs` change. **No `src/`
      file is touched.**

## Test plan

The load-bearing verification is **the 100-run loop**, not a unit test. A 5%
flake cannot be caught by running the suite once, which is exactly how it
survived two milestones. The E2E block below runs it 100 times and counts.

`held_port_cannot_be_rebound` is the regression guard: it makes the *invariant*
(the listener is held) testable in one second, so a future refactor that
reintroduces the drop fails immediately instead of 5% of the time.

**What would make this phase a false success:** running the suite once, seeing
green, and calling it fixed. At a 5% rate a single green run happens 95% of the
time on the **unfixed** code. The 100-run count is the only thing that
distinguishes a fix from luck.

A second one: deleting or weakening
`webhook_ports_differ_between_environments` because it is "flaky". It is not
flaky — it is the detector, and it was right.

## End-to-end verification

Run this block verbatim and paste the resulting file into your Update Log.

**Two constraints carried from M7 phase-03's post-mortem:** **no heredocs**, and
every long-running command wrapped in `timeout`. An M7 E2E block nested a
`python3` heredoc that hung and orphaned two processes at 100% CPU for 70
minutes. Do not reintroduce either pattern.

A warm isolation run takes about **0.33 s**, so 100 runs is roughly 35 seconds.

```bash
cd /home/matt/src/daemoneye
cargo build --tests 2>&1 | tail -2
{
  echo "=== the released-port allocator is gone ==="
  timeout 30 grep -c "alloc_free_port" tests/harness/mod.rs
  echo "count-above-must-be-0"

  echo "=== the canary and the new guard both exist ==="
  timeout 30 grep -c "webhook_ports_differ_between_environments" tests/isolation.rs
  timeout 30 grep -c "held_port_cannot_be_rebound" tests/isolation.rs

  echo "=== no src/ changes ==="
  timeout 30 git diff --name-only HEAD -- src/ | wc -l
  echo "src-files-changed-above-must-be-0"

  echo "=== 100 consecutive isolation runs (baseline before this phase: 5 failures) ==="
  fails=0
  for i in $(seq 1 100); do
    if ! timeout 120 cargo test --test isolation > /tmp/iso-run.txt 2>&1; then
      fails=$((fails+1))
      echo "  run $i FAILED:"
      grep -E "panicked at|assertion|^test .*FAILED" /tmp/iso-run.txt | head -3
    fi
  done
  echo "isolation-failures=$fails   # 0 == PASS"

  echo "=== full gate ==="
  timeout 900 cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -2
  echo "clippy-exit=$?"
  timeout 900 cargo test 2>&1 | grep -E "^test result"
} > /tmp/m8-phase01-e2e.txt 2>&1
cat /tmp/m8-phase01-e2e.txt
```

`isolation-failures=0` against a measured baseline of 5 is the proof. Paste the
captured file into an Update Log entry titled
`### Update — <date> (end-to-end verification)`, **and separately** the
released-listener red run the acceptance criteria require.

**The server-authored `(complete)` entry does not satisfy either** — its
"Command output tails" block is the automatic gate capture every phase receives,
and it shows that build/lint/test ran once, which is precisely the evidence that
cannot distinguish this fix from luck.

**If any part of the capture block fails or hangs, stop and report it as a
blocker.** Do not re-run the surviving sections separately and paste the
result — a transcript assembled from more than one run fails `STANDARDS.md` §1
even when every claim in it is true.

## Authorizations

- [ ] May add dependencies: **none**. `tokio` is already a dev-dependency and
      `TcpListener::from_std` needs no feature beyond what the harness uses.
- [ ] May touch `docs/architecture.md`: no.
- [ ] May touch `CLAUDE.md`: no.
- [ ] May create new files: no.

## Out of scope

- **Any file under `src/`.** An acceptance criterion pins this. The daemon's
  webhook binding is correct; the defect is entirely in the test harness.
- **`hooks_land_on_private_server`.** It binds no ports, it did not fail once in
  the 100-run baseline, and this phase does not claim to fix it. If it recurs
  after M8 it is a separate bug — do not try to harden it here.
- **The four real-clock sleeps** in `src/cli/input/tty.rs` and
  `src/cli/commands/stream.rs`. That is M8 phase 02.
- **Making the webhook window zero** by passing a file descriptor to the daemon
  process. It would need production changes to accept an inherited listener; the
  drop-immediately-before-spawn approach is what this phase buys, and the
  baseline says the collision — not the residual window — is what fails.
- **Reducing test parallelism** (`--test-threads=1`) to dodge the race. It would
  hide the bug and slow the suite; the collision would still exist for anyone
  running two cargo invocations at once.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

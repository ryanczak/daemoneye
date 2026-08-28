# Bug 1 on phase-01: the replacement `peer_euid` test trades a deterministic environment failure for a cross-thread fd-reuse race

**Severity:** major
**Status:** verified 2026-08-28 (commit `f008509`, round 2)
**Filed:** 2026-08-28

## What's wrong

`src/daemon/server/mod.rs:358-372` — the rewritten
`peer_euid_none_on_invalid_fd` obtains a file descriptor, lets the owning
`File` drop (closing the fd), and then asserts on the **closed fd number**:

```rust
let fd = std::os::fd::AsRawFd::as_raw_fd(&std::fs::File::open("/dev/null").unwrap()); // closes on drop
struct ClosedFd(std::os::fd::RawFd);
impl std::os::fd::AsRawFd for ClosedFd { fn as_raw_fd(&self) -> std::os::fd::RawFd { self.0 } }
assert_eq!(peer_euid(&ClosedFd(fd)), None);
```

The `File` temporary is dropped at the end of the `let` statement, so `fd` is
already dangling when `ClosedFd(fd)` is built. The test then depends on that
descriptor number **staying** unallocated until `peer_euid` runs.

It does not reliably stay unallocated. Linux hands out the lowest free
descriptor, so reuse is immediate — measured at review:

```
closed /dev/null fd = 3; next socket fd = 3  -> REUSED: True
```

`cargo test` runs the suite on multiple threads in **one process** with a
shared descriptor table, and the same module opens Unix sockets concurrently
(`peer_euid_matches_own_process`, `src/daemon/server/mod.rs:340-347`, binds a
`UnixListener` and connects a `UnixStream`). If any concurrent thread is
handed that descriptor number in the window between the drop and the
`getsockopt` call, `SO_PEERCRED` succeeds and `peer_euid` returns
`Some(uid)` — the assertion fails.

The window is small and the failure will be rare, which is what makes it
worse than the bug it replaced: a rare cross-thread race is harder to
diagnose than the deterministic environment dependency it was fixing.

## What should happen

The test must be hermetic **by construction**, not by the convention that no
other thread claims the descriptor — the same standard M17 phase-02 round 3
settled on for the alternate-screen guard ("correct by construction rather
than by convention", `docs/dev/NEXT.md`).

Two shapes satisfy it. Either is acceptable:

- Use a descriptor number that can never become valid (e.g. `-1`), so
  `getsockopt` returns `EBADF` unconditionally; or
- Keep a **live, non-socket** descriptor by binding the `File` to a named
  local that outlives the assertion, so `getsockopt` returns `ENOTSOCK`
  unconditionally. This also matches the test's own doc comment — "an fd that
  is no longer a socket" — more closely than a closed fd does.

The behaviour under test is unchanged: `peer_euid` must return `None` for a
descriptor that is not a live socket.

## Root cause

The executor was fixing a **real** pre-existing defect, and its diagnosis was
correct — confirmed independently at review. The original test asserted on
`std::io::stdin()`, so its result depended on what the harness handed the
process as stdin:

| stdin | `peer_euid(stdin)` | original test |
|---|---|---|
| `/dev/null` | `None` (ENOTSOCK) | passes |
| pipe | `None` (ENOTSOCK) | passes |
| **socketpair** | `Some(1000)` | **fails** |

The executor runs under an MCP stdio server, whose children inherit a socket
on fd 0 — so the gate genuinely blocked, every time, in that environment. The
fix direction (stop asserting on stdin) is right.

The defect is in the replacement's lifetime handling only: `as_raw_fd()` was
called on a temporary rather than on a binding, which silently converted
"a descriptor that is not a socket" into "a descriptor that is closed and
therefore reusable".

## Scope note (not a defect, recorded for the review verdict)

`src/daemon/server/mod.rs` is **not** in the phase doc's § Authorizations,
which lists four files, and § Authorizations also says to record a blocker
entry and stop rather than improvise. The executor edited it anyway.

Weighing this fairly: the instruction it arguably crossed reads *"If an
**acceptance criterion** cannot be satisfied honestly"* — what actually blocked
here was the `cargo test` **gate**, via a pre-existing test the phase never
touched. That case is not covered by the sentence, so the phase doc left a
real gap. The executor also diagnosed the cause correctly, changed no
production code, committed the fix separately as `test(ipc):`, logged the
investigation in two Update Log entries, and flagged the deviation for review
in its completion summary. That is the transparent, non-destructive form of
this behaviour, and a marked improvement on the M16 phase-01 precedent.

Treated as an **architect-side calibration item**, not an executor failure.
No action required in this bug beyond the fix above.

## Definition of done

Each command was run against the current tree at filing and produced the
"before" value shown.

- [ ] `grep -cE 'as_raw_fd\(&std::fs::File::open' src/daemon/server/mod.rs`
      prints `0` (**before: 1**) — no `as_raw_fd` on a temporary whose owner
      is dropped in the same statement. Both fix shapes above satisfy this.
- [ ] `grep -c "fn peer_euid_none_on_invalid_fd" src/daemon/server/mod.rs`
      prints `1` — the test is repaired, not deleted (**before: 1**, must not
      change).
- [ ] `peer_euid_none_on_invalid_fd` passes with a **socketpair** on stdin —
      the environment that broke the original. Run and paste the output:

      ```sh
      cargo build --tests 2>&1 | tail -1
      BIN=$(ls -t target/debug/deps/daemoneye-* | grep -v '\.d$' | head -1)
      python3 - "$BIN" <<'PY'
      import socket, subprocess, sys
      a, b = socket.socketpair()
      r = subprocess.run([sys.argv[1], "peer_euid_none_on_invalid_fd"], stdin=a.fileno(),
                         stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
      print([l for l in r.stdout.decode().splitlines() if l.startswith("test result:")][0])
      PY
      ```

      (**before: passes** — a regression guard, so it must still print
      `1 passed; 0 failed`.)
- [ ] `peer_euid_none_on_invalid_fd` also passes with `< /dev/null` and with a
      pipe on stdin, so the test is stdin-independent in all three shapes.
- [ ] All four gates green: `cargo fmt --all`, `cargo build`,
      `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- [ ] `cargo test --lib 2>&1 | grep -E "^test result:"` still reports
      `1395 passed; 0 failed`.

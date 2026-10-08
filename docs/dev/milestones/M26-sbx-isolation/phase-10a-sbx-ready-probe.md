# Phase 10a: sbx-ready-probe — a sandbox shell host must pass the readiness probe

**Milestone:** M26 — Agent isolation on Docker Sandboxes (sbx)
**Status:** in-progress
**Depends on:** phase-09c
**Estimated diff:** ~40 lines
**Tags:** language=rust, kind=bugfix, size=xs

## Goal

Found at the row-10 live close on pinky, 2026-10-08. A sandboxed ghost's
every command failed with:

```
Error: could not open a shell in sandbox de-ghost-…: shell s1 never became
ready at …/run/shells/s1.sock within 15s
```

while the host was healthy: its socket, lock and `agent@de-ghost-…` prompt
all appeared within 0.3 s of spawn, and the process stayed alive.

## Root cause

`crate::shell::launch::await_ready` (`src/shell/launch.rs`) sends
`ShellRequest::Status` and waits **200 ms** for the reply
(`tokio::time::timeout(Duration::from_millis(200), …)`). A plain PTY host
answers in microseconds. An sbx host's `SbxBackend::status`
(`src/shell/sbx_backend.rs`) answers only after running the in-VM FIND
script through `sbx exec`, measured at **0.24 s warm** against the live
host, and longer cold. So every probe times out and reconnects until the
15 s deadline, which `spawn_dedicated_for`, `shell_for_owner_in_sandbox`
and `start_background_in_sandbox_with` all treat as a dead host.

## Spec

Make the readiness probe tolerate a slow-but-healthy `Status` reply
without weakening the 15 s overall deadline or the "host never came up"
rollback. Choose the mechanism. Two that fit: a per-probe reply timeout
long enough for an sbx FIND (the probe already loops until the deadline),
or a cheap readiness answer that does not need the VM round trip. Do not
touch the vm-signal script or `SbxBackend`'s signal routing.

Add one test proving the fix: a `Status` reply that arrives after 500 ms
(a fake host, or a delayed responder) now passes `await_ready` within a
2 s deadline. Name it `await_ready_accepts_a_slow_status_reply`. Keep the
existing `await_ready` tests passing unchanged.

## Acceptance criteria

- [ ] `cargo test --lib await_ready_accepts_a_slow_status_reply` reports
      `1 passed`. Today it reports `0 passed`.
- [ ] `cargo test --lib shell::launch` reports `0 failed`, no existing test edited.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0;
      `cargo fmt --all --check` exits 0.
- [ ] `cargo test` reports `0 failed`, and exactly one more passing test
      than today's 2297: `passed=2298`.
- [ ] The Update Log entry contains the § End-to-end verification output,
      every line as printed, and a line `PASTE MATCH: yes`.

## End-to-end verification

```sh
cargo test --lib await_ready_accepts_a_slow_status_reply 2>&1 | grep -E '^test result'
cargo test --lib shell::launch 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
git diff --stat
```

Each line runs **once**. Expect **only** `test result:` summary lines from
the `cargo test` lines; rexyMCP's output filter removes the individual
`test … ok` lines.

## Authorizations

- [x] May edit `src/shell/launch.rs`, and `src/shell/sbx_backend.rs` only
      if the chosen mechanism needs it.

## Out of scope

- The live re-run: the architect repeats the row-10 ghost check after this
  lands.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-08 12:56 (started)

Starting phase-10a. Chosen mechanism: lengthen `await_ready`'s per-probe reply
timeout — a slow reply arrives inside the existing connect loop, so the 15 s
overall deadline and the rollback on failure stay untouched, and
`SbxBackend::status` needs no change (the FIND round trip is the reply, just a
slower one). Next: implement, add
`launch_await_ready_accepts_a_slow_status_reply`, verify. — Executor (phase-10a)

### Update — 2026-10-08 13:00 (end-to-end verification)

Ran the phase's End-to-end verification block; every line as printed, each run
once:

```text
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2243 filtered out; finished in 0.53s
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 2232 filtered out; finished in 1.02s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s
fmt_exit=0
passed=2298 failed=0
 docs/dev/milestones/M26-sbx-isolation/README.md    |  2 +-
 .../M26-sbx-isolation/phase-10a-sbx-ready-probe.md | 11 +++-
 src/shell/launch.rs                                | 58 ++++++++++++++++++++--
 3 files changed, 66 insertions(+), 5 deletions(-)
```

PASTE MATCH: yes

Notes: the test count moved 2297 → 2298 (the one new test, no existing test
edited); `shell::launch` is 12 passed / 0 failed. — Executor (phase-10a)

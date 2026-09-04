# Phase 05a: the shell-host wire protocol and socket server

**Milestone:** M20 — Shell Engine
**Status:** todo
**Depends on:** none in code. `src/shell/` exists from phases 02-04; this phase
adds two sibling modules and calls none of them.
**Estimated diff:** ~430 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Define the wire protocol a shell-host process speaks, and the socket server
that speaks it: newline-delimited JSON frames over
`~/.daemoneye/var/run/shells/<id>.sock`, peer-uid checked, dispatched to a
`ShellBackend` trait.

Hermetic: the tests drive a **fake** backend over a **real** Unix socket in a
temp directory. No PTY, no fork, no detached spawn, no config read — phase-05b
owns all of that and is the first implementor of the trait.

**Why this is split off.** Phase 05 as originally scoped bundled the protocol,
the server, PTY ownership, log and screen wiring, a detached spawn and a
readiness handshake. That is several sessions of work, and the parts needing a
real PTY or a `fork` cannot be executor-authored under STANDARDS § 1. This half
is the part that is hermetic and fully testable.

## Architecture references

Read before starting:

- `docs/design/daemoneye-2.0.md` § 2.1, the "Persistence across daemon
  restarts" paragraph — why a shell lives in its own process behind a socket.
- `docs/security.md` § 1 — IPC peer authentication is the primary boundary;
  this socket gets the same check as the daemon's, from the same function.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

- `src/shell/` holds `mod.rs`, `pty.rs`, `log.rs`, `screen.rs`. **`proto.rs`
  and `host.rs` do not exist.**
- `config::shells_dir()` (`src/config/load.rs:40`) resolves
  `~/.daemoneye/var/run/shells/`; phase-01 created it. **This phase does not
  write there** — every test uses `tempfile::tempdir()`.
- **The peer-uid check already exists and must not be duplicated.**
  `src/daemon/server/mod.rs` holds two private free functions, `peer_euid`
  and `check_peer_identity`:

  ```rust
  /// Reject connections whose peer euid differs from the daemon's euid.
  /// Returns `Err` (caller should drop the connection) when identity cannot be
  /// established or the peer is not our own user.
  fn check_peer_identity<S: std::os::fd::AsRawFd>(stream: &S) -> anyhow::Result<()> {
      let daemon_euid = unsafe { libc::geteuid() };
      match peer_euid(stream) {
  ```

  `daemon::server` is already `pub mod server` (`src/daemon/mod.rs:42`), so the
  only change needed is widening these two from private to `pub(crate)`.
  **Do that — do not move them, do not copy them, do not write a second
  implementation.** A security boundary with two implementations drifts.

  **On `unsafe`:** those functions contain `unsafe` blocks today. Widening a
  visibility keyword adds no `unsafe` and is expressly permitted here.
  STANDARDS § 1's prohibition is on *writing* new `unsafe`; you will write
  none. If you find yourself typing `unsafe`, stop — you have taken a wrong
  turn.

- `src/ipc.rs` is the framing analogue: `#[derive(Serialize, Deserialize)]`
  enums exchanged as newline-delimited JSON over a Unix socket. This phase
  follows the same shape for its own, separate protocol.
- `serde`, `serde_json`, `tokio` (with `net` and `io-util`), `anyhow` and
  `libc` are all already dependencies. `tempfile` is already a dev-dependency.
  **No new dependency is needed or authorized.**

## Measured facts — executed 2026-09-03, not reasoned about

### F1. The socket's privacy comes from the umask, and the default umask is not private

Binding a `tokio::net::UnixListener` and reading the resulting mode:

| condition | resulting socket mode |
|---|---|
| default umask | **`755`** |
| after `umask(0o077)` — what `main()` sets at `src/main.rs:280` | `700` |
| explicit `set_permissions(0o700)` after bind | `700` |

So the daemon's socket is private only because `main()` sets the umask before
anything binds. **This phase sets the mode explicitly after bind anyway**, so
the guarantee does not depend on a caller's umask — a shell-host started by
some future path with a different umask would otherwise expose the socket.

### F2. Binding over an existing socket path fails

A second `bind` to a path that already exists returns
`std::io::ErrorKind::AddrInUse`. A stale socket file must be removed first —
the daemon does exactly this at `src/daemon/mod.rs:964`. Removing a *live*
socket is what the instance lock exists to prevent, so this phase removes only
when the caller has said it owns the id; see Task 3.

## The wire format — pin this exactly

Newline-delimited JSON, one value per line, same as `src/ipc.rs`. Both enums
are `#[serde(tag = "type")]` so a frame is self-describing on the wire.

**Client → host** (`ShellRequest`):

```json
{"type":"Subscribe"}
{"type":"Input","bytes":[104,105,10]}
{"type":"Resize","rows":40,"cols":120}
{"type":"Signal","signal":"INT"}
{"type":"Status"}
```

**Host → client** (`ShellResponse`):

```json
{"type":"Ok"}
{"type":"Error","message":"no such shell"}
{"type":"Chunk","bytes":[104,105]}
{"type":"Status","state":"Running","rows":40,"cols":120,"pid":1234}
{"type":"Exited","code":0}
```

**Bytes travel as a JSON array of numbers** (`Vec<u8>`, serde's default for
that type). This is deliberate and has a cost: a 4 KiB output chunk becomes
roughly 16 KiB on the wire. The alternative — base64 — needs a dependency this
milestone has not authorized, and a JSON string cannot carry arbitrary bytes
because PTY output is not guaranteed valid UTF-8 (phase-02 measured a read
splitting a multi-byte character). **Losslessness wins over compactness here;
phase-05b may revisit it with measurements from real traffic.**

`signal` is one of the strings `"INT"`, `"TERM"`, `"STOP"`, `"CONT"` —
nothing else parses. `state` is one of `"Running"`, `"Exited"`.

## Spec

### Task 1 — Widen the peer check to `pub(crate)`

In `src/daemon/server/mod.rs`, change `fn peer_euid` and
`fn check_peer_identity` to `pub(crate) fn`. Change nothing else about them —
not the bodies, not the `unsafe` blocks, not the log messages. Existing callers
keep working unchanged.

### Task 2 — `src/shell/proto.rs`: the frame types

Create the module, declare it in `src/shell/mod.rs` beside the existing
`mod log; mod pty; mod screen;`, and re-export the public items.

Define `ShellRequest` and `ShellResponse` exactly as § "The wire format" pins
them, plus the two small enums they carry:

```rust
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum ShellSignal { Int, Term, Stop, Cont }

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum ShellState { Running, Exited }
```

Serialize both as the uppercase strings the wire format shows
(`"INT"`, `"Running"`, …) — use `#[serde(rename = "...")]` per variant rather
than a case-conversion attribute, so the wire strings are visible in the source
and cannot drift.

Add `pub fn encode(frame) -> String` helpers or rely on `serde_json::to_string`
directly; either is acceptable, but **every frame written to the wire ends with
exactly one `\n`** and contains none internally (serde_json never emits a bare
newline inside a compact value, so this holds automatically — do not
pretty-print).

### Task 3 — `src/shell/host.rs`: the server

```rust
/// What a shell-host must be able to do. Phase-05b implements this over a
/// real PTY; the tests implement it over a fake.
#[async_trait::async_trait]
pub trait ShellBackend: Send + Sync + 'static {
    async fn input(&self, bytes: &[u8]) -> anyhow::Result<()>;
    async fn resize(&self, rows: u16, cols: u16) -> anyhow::Result<()>;
    async fn signal(&self, sig: ShellSignal) -> anyhow::Result<()>;
    async fn status(&self) -> ShellResponse;
    /// A receiver of output chunks for one subscriber.
    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Vec<u8>>;
}
```

`async_trait` is **already a dependency** (`Cargo.toml`), and `src/ai/mod.rs`
uses it for `AiClient` — follow that shape.

- `pub async fn bind(path: &Path) -> anyhow::Result<UnixListener>` — remove a
  stale file at `path` if present (F2), bind, then
  `set_permissions(path, Permissions::from_mode(0o700))` (F1). Return the
  listener.
- `pub async fn serve<B: ShellBackend>(listener: UnixListener, backend: Arc<B>) -> anyhow::Result<()>`
  — accept in a loop; for each connection call
  `crate::daemon::server::check_peer_identity(&stream)` **before reading a
  single byte** and drop the connection on `Err`; then spawn a task that reads
  newline-delimited requests and writes responses.
- Frame handling: `Input`/`Resize`/`Signal` call the backend and answer `Ok`,
  or answer `Error` with the error's `to_string()` — **a backend error is an
  `Error` frame, never a dropped connection and never a panic.** `Status`
  returns whatever the backend gives. `Subscribe` switches that connection to
  streaming: every chunk from the broadcast receiver becomes a `Chunk` frame
  until the client disconnects.
- **A malformed line is answered with an `Error` frame and the connection
  stays open.** A client that sends garbage must not be able to kill the
  server, and must not be silently ignored either.

**Error handling:** propagate with `anyhow::Context`. No `.unwrap()`,
`.expect()` or `panic!()` anywhere outside `#[cfg(test)]`.

### Task 4 — Write the tests named in § Test plan

Hermetic: `tempfile::tempdir()` for the socket path, a fake backend recording
what it was asked to do, and a **real** `UnixStream` client. Tokio tests use
`#[tokio::test]`. No PTY, no fork, no `~/.daemoneye/` access.

### Task 5 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-05a.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this. Then run the PASTE MATCH self-check
in that same section and paste **the literal verdict line it prints** into the
same entry.

## Acceptance criteria

Every command below was run against the current tree while drafting and
returns the "before" value shown.

- [ ] `test -f src/shell/proto.rs && test -f src/shell/host.rs && echo yes` →
      **yes** (both absent now).
- [ ] `grep -c '^mod proto;' src/shell/mod.rs` → **1** (now `0`).
- [ ] `grep -c '^mod host;' src/shell/mod.rs` → **1** (now `0`).
- [ ] `grep -cE '^pub enum ShellRequest' src/shell/proto.rs` → **1**.
- [ ] `grep -cE '^pub enum ShellResponse' src/shell/proto.rs` → **1**.
- [ ] `grep -cE '^pub trait ShellBackend' src/shell/host.rs` → **1**.
- [ ] **The peer check is called, not reimplemented:**
      `grep -c 'check_peer_identity' src/shell/host.rs` → **at least 1**, and
      `grep -c 'SO_PEERCRED' src/shell/host.rs` → **0**.
- [ ] `grep -c 'pub(crate) fn check_peer_identity' src/daemon/server/mod.rs` →
      **1** (now `0`).
- [ ] **No new `unsafe` anywhere in this phase:**
      `grep -vE '^\s*(//|///|//!|\*)' src/shell/host.rs src/shell/proto.rs | grep -c 'unsafe'`
      → **0**.
- [ ] No `unwrap`/`expect`/`panic!` outside test code:
      `awk '/^#\[cfg\(test\)\]/{exit} {print}' src/shell/host.rs | grep -cE '\.(unwrap|expect)\(|panic!\('`
      → **0**. The `^` anchor is required, or a doc comment mentioning the test
      attribute stops awk early and the guard goes vacuous.
- [ ] `cargo test --lib shell::host::` reports **7 or more** passing and
      `0 failed` (now: `0 passed; 0 failed; … 1576 filtered out`).
- [ ] `cargo test --lib shell::pty::` → **13 passed**,
      `cargo test --lib shell::log::` → **12 passed**,
      `cargo test --lib shell::screen::` → **11 passed** — phases 02-04 are
      untouched.
- [ ] All four gates pass: `cargo fmt --all`, `cargo build`,
      `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.

## Test plan

Names pinned; placement is not — a `#[cfg(test)] mod tests` at the **bottom**
of each module, the repo convention. Every name in `host.rs` begins `host_`
and every name in `proto.rs` begins `proto_`.

**The round-trip over a real socket is the headline test**, for the same reason
it was in phase-03: a protocol is only correct if a client's frame reaches the
backend and the answer comes back.

- `host_round_trips_every_request_over_a_real_socket` — **the primary-use
  test.** Bind in a tempdir, serve a fake backend, connect a real
  `UnixStream`, and send `Input`, `Resize`, `Signal` and `Status` in sequence
  on **one** connection; assert each answer and assert the fake recorded each
  call with the right arguments. Sending them on one connection is the point —
  a per-frame connection would not catch state left behind between frames.
- `host_subscribe_streams_chunks_until_the_client_disconnects` — subscribe,
  push two chunks through the broadcast sender, assert both arrive as `Chunk`
  frames in order and with byte-exact payloads, including a chunk containing
  **non-UTF-8 bytes** (this is why the wire carries a byte array).
- `host_answers_a_backend_error_with_an_error_frame` — a fake whose `input`
  returns `Err` produces an `Error` frame carrying the message, and **the
  connection stays open**: a following `Status` on the same connection still
  answers.
- `host_answers_malformed_input_with_an_error_frame` — send `not json\n`;
  assert an `Error` frame comes back and the connection is still usable.
- `host_binds_the_socket_private` — after `bind`, the socket's mode is
  `0o700`. F1 is why this is asserted rather than assumed.
- `host_replaces_a_stale_socket_file` — a plain file already at the path does
  not prevent `bind` (F2 says a second bind fails, so the stale file must be
  removed first); binding twice in sequence to the same path succeeds both
  times.
- `host_rejects_a_peer_it_cannot_identify` — **the negative case for the
  security boundary.** Assert `crate::daemon::server::check_peer_identity`
  returns `Ok` for a socket pair created by this same process (our own uid),
  which is the only case reachable in a hermetic test. State in the assertion
  message that a differing-uid peer cannot be constructed in-process and is
  covered by the daemon's own boundary, so this test pins the *call site*, not
  the kernel's decision.
- `proto_request_frames_match_the_pinned_wire_format` — each `ShellRequest`
  variant serialises to exactly the JSON in § "The wire format", compared as
  parsed `serde_json::Value` so field order is not pinned.
- `proto_response_frames_match_the_pinned_wire_format` — the same for
  `ShellResponse`.
- `proto_signal_and_state_use_their_wire_strings` — `ShellSignal::Int`
  serialises as `"INT"` and the other three likewise; `ShellState::Running` as
  `"Running"`. **Negative case:** deserialising `"int"` or `"SIGINT"` fails.
- `proto_bytes_survive_a_non_utf8_round_trip` — a `Vec<u8>` payload containing
  `0xff` and `0x00` round-trips byte-exact through serialise/deserialise. This
  is the property the array encoding was chosen for.

## End-to-end verification

Run this block verbatim from the repo root. It writes `/tmp/e2e-05a.txt`.

```sh
{
echo "== A. build =="
cargo build 2>&1 | tail -2; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== B. named tests (each line is one pinned test) =="
cargo test --lib 2>&1 | grep -E "^test shell::(host|proto)::.* ok$" | sed 's/^test //' | sort
echo "cargo_exit=${PIPESTATUS[0]}"
echo "== C. shell::host:: and shell::proto:: totals =="
cargo test --lib shell::host:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
cargo test --lib shell::proto:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== D. phases 02-04 untouched (13, 12, 11) =="
cargo test --lib shell::pty:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
cargo test --lib shell::log:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
cargo test --lib shell::screen:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== E. lib suite totals =="
cargo test --lib 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== F. structural greps (each must print the stated number) =="
echo -n "proto.rs + host.rs exist (1): "; test -f src/shell/proto.rs && test -f src/shell/host.rs && echo 1 || echo 0
echo -n "mod proto declaration    (1): "; grep -c '^mod proto;' src/shell/mod.rs
echo -n "mod host declaration     (1): "; grep -c '^mod host;' src/shell/mod.rs
echo -n "pub enum ShellRequest    (1): "; grep -cE '^pub enum ShellRequest' src/shell/proto.rs
echo -n "pub enum ShellResponse   (1): "; grep -cE '^pub enum ShellResponse' src/shell/proto.rs
echo -n "pub trait ShellBackend   (1): "; grep -cE '^pub trait ShellBackend' src/shell/host.rs
echo -n "calls check_peer_identity(>=1): "; grep -c 'check_peer_identity' src/shell/host.rs
echo -n "does NOT reimpl peercred (0): "; grep -c 'SO_PEERCRED' src/shell/host.rs
echo -n "peer check widened       (1): "; grep -c 'pub(crate) fn check_peer_identity' src/daemon/server/mod.rs
echo -n "no new unsafe            (0): "; grep -vE '^\s*(//|///|//!|\*)' src/shell/host.rs src/shell/proto.rs | grep -c 'unsafe'
echo -n "unwrap/expect/panic pre-test (0): "
awk '/^#\[cfg\(test\)\]/{exit} {print}' src/shell/host.rs | grep -cE '\.(unwrap|expect)\(|panic!\('
} > /tmp/e2e-05a.txt 2>&1
cat /tmp/e2e-05a.txt
```

Paste the contents of `/tmp/e2e-05a.txt` into your Update Log entry as a fenced
block, then run the self-check and paste its verdict line into the same entry:

```sh
D=docs/dev/milestones/M20-shell-engine/phase-05a-shell-host-protocol.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-05a.txt
diff /tmp/pasted-05a.txt /tmp/e2e-05a.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

**Sections B through E can each report success with nothing having run.**
Measured on the current tree: `cargo test --lib shell::host::` prints
`test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1576 filtered out`
and exits `0`. A zero exit proves nothing — the pass conditions are the named
test lines in B, a count of seven or more in C, and exactly `13`, `12`, `11`
in D.

**Section F on an absent file errors rather than printing `0`.** Measured: a
`grep -c` against a missing path warns on stderr and exits `2`, printing no
count. The block redirects `2>&1`, so a warning there is itself proof the file
is missing.

The PASTE MATCH self-check was validated both ways while drafting a sibling
phase: a byte-exact paste printed `PASTE MATCH`, and the same paste with one
line retyped printed `PASTE MISMATCH` naming the divergent line.

## Authorizations

- Create `src/shell/proto.rs` and `src/shell/host.rs`; edit `src/shell/mod.rs`
  (the two `mod` lines and the `pub use`).
- Edit `src/daemon/server/mod.rs` **for Task 1 only** — widening two `fn` to
  `pub(crate) fn`. No other change to that file.
- Run `cargo fmt --all`, `cargo build`,
  `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- **No new dependencies.** `serde`, `serde_json`, `tokio`, `async-trait`,
  `anyhow`, `libc` and dev-`tempfile` are all present and are all this phase
  needs.
- May **not** touch `src/shell/pty.rs`, `src/shell/log.rs` or
  `src/shell/screen.rs`. Phases 02-04 are `done`; their counts stay 13, 12, 11.
- May **not** touch `docs/architecture.md`, `CLAUDE.md` or `README.md` —
  M20's documentation updates land in the milestone's closing phase.

## Out of scope

- **Everything needing a real PTY or a `fork`.** No `daemoneye shell-host`
  subcommand, no PTY ownership, no detached spawn, no readiness pipe, no
  adoption. That is phase-05b, and it is reserved for architect authorship
  because it needs `unsafe` or a measured safe substitute.
- **Wiring to `PtyShell`, `CastWriter` or `ShellScreen`.** The trait exists so
  05b can implement it; this phase's only implementor is the test fake. A
  module with tests and no production caller is the intended end state — do not
  add `#[allow(dead_code)]`, do not invent a caller.
- **Writing under `~/.daemoneye/`.** Tests use a temp directory. The real path
  from `config::shells_dir()` is used by 05b.
- **A registry, per-owner caps, or GC.** Phase-06.
- **base64 or any compact byte encoding.** The array form is the pinned
  decision for this phase; revisit in 05b with real traffic measurements.
- **Reconnect, retry or backpressure policy.** A dropped subscriber simply
  ends that connection; the broadcast channel's lag behaviour is 05b's problem
  when it has real volume to measure.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

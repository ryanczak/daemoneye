# Phase 05b: the PTY-backed shell host backend

**Milestone:** M20 — Shell Engine
**Status:** review
**Depends on:** phase-05a (`ShellBackend`, done) and phase-02/03/04 (`PtyShell`,
`CastWriter`, `ShellScreen`, all done).
**Estimated diff:** ~450 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Give `PtyShell` a **public raw-output stream**, then implement phase-05a's
`ShellBackend` over it as `PtyBackend`: one task pumps PTY bytes to three
consumers — the subscriber broadcast, the asciicast log, and the screen — while
`input`, `resize`, `signal` and `status` act on the live shell.

This is the piece that makes a shell host real. It uses a **real PTY** but
**no fork, no CLI subcommand, no socket** — phase-05c wraps a binary around it.

## Architecture references

Read before starting:

- `docs/design/daemoneye-2.0.md` § 2.1 — the shell engine; the screen is the
  viewport and the cast log is the transcript.
- `docs/dev/milestones/M20-shell-engine/README.md` § Notes — the resize carry
  and the two findings that produced this phase's split.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. **The tree is deliberately dirty**: `src/shell/pty.rs` (Task 1, done) and
   the two M20 docs. That is the expected starting state, not a problem to
   fix — do not revert, stash or re-derive any of it.

## Current state

- `src/shell/` holds `mod.rs`, `pty.rs`, `log.rs`, `screen.rs`, `proto.rs`,
  `host.rs`. Test counts today: `pty` **13**, `log` **12**, `screen` **11**,
  `host` **8**, `proto` **5**.
- **Task 1 has landed on disk (architect-verified 2026-09-03, see § Notes
  for executor). `PtyShell` now streams.** The paragraph below describes the
  tree *before* that change and is kept for the record; do not re-derive it.
  Before Task 1, output arrived on a private field:

  ```rust
  pub struct PtyShell {
      shell: String,
      master: Box<dyn MasterPty + Send>,
      writer: Box<dyn Write + Send>,
      child: Box<dyn portable_pty::Child + Send + Sync>,
      reader_rx: mpsc::Receiver<Vec<u8>>,
      _reader_handle: Option<JoinHandle<()>>,
  }
  ```

  `run()` drains `reader_rx` with `recv_timeout` until it sees the end marker.
  `mpsc::Receiver` is **single-consumer**, so a host that must feed subscribers,
  the log and the screen at once cannot share it. This phase changes that.
- `ShellBackend` (phase-05a, `src/shell/host.rs`) is the trait to implement:

  ```rust
  #[async_trait]
  pub trait ShellBackend: Send + Sync + 'static {
      async fn input(&self, bytes: &[u8]) -> anyhow::Result<()>;
      async fn resize(&self, rows: u16, cols: u16) -> anyhow::Result<()>;
      async fn signal(&self, sig: ShellSignal) -> anyhow::Result<()>;
      async fn status(&self) -> ShellResponse;
      fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Vec<u8>>;
  }
  ```

- `ShellResponse::Status { state, rows, cols, pid }` and `ShellState` come from
  `src/shell/proto.rs`.
- `CastWriter::{create, write_output, write_input, mark, byte_len}` takes an
  injected `Duration` — the module never reads a clock, so **this phase owns
  the time base** and passes elapsed-since-start.
- `ShellScreen::{new, feed, contents, annotated, summary, is_alt_screen, size}`.
- No new dependency is needed: `tokio` (with `sync`, `net`, `io-util`, `time`),
  `async-trait`, `anyhow`, `portable-pty`, `vt100`, dev-`tempfile` are present.

## Measured facts — executed 2026-09-03, not reasoned about

### F1. Detaching a child needs no `unsafe` — the reservation on this work is lifted

The milestone reserved the shell-host for architect authorship on the
assumption that detaching needed `fork`/`setsid`. Measured instead, using only
`std::os::unix::process::CommandExt::process_group(0)` (safe, stable):

| child | pgid | own group? |
|---|---|---|
| spawned with `process_group(0)` | `522310`, its own pid | **yes** |
| control, spawned without it | `522311`, the parent's | no |

Both children also **outlived the parent's exit** and kept writing a heartbeat
file after it was gone. So group isolation is available safely, and orphan
survival is the default. **This phase still writes no spawn code** — that is
05c — but the finding is why 05c is an ordinary dispatchable phase.

### F2. The resize carry, restated because it lands here

Phase-04 measured that `vt100`'s `set_size` **does not reflow**: a soft-wrapped
row becomes a hard break at the *old* width, so widening corrupts text already
on screen. **Do not call `set_size`.** On resize, resize the PTY and then
**replace** the `ShellScreen` with a fresh one at the new size. History is not
lost — the cast log is the transcript of record; the screen is only the
viewport.

## Spec

### Task 1 — Give `PtyShell` a public output stream — **ALREADY DONE, DO NOT REDO**

**This task is on disk, unverified-by-you but verified by the architect:**
`cargo test --lib shell::pty::` reports **13 passed** and clippy is clean on
the tree you are handed. `git diff src/shell/pty.rs` shows it. Read it, do not
rewrite it. Your first action on `pty.rs` is **none**; your first gate is the
one in Task 2.

What landed, so Task 2 can rely on it without reading further:

- `pub const STREAM_CAPACITY: usize = 1024` — the broadcast capacity.
- The reader thread publishes every chunk on a
  `tokio::sync::broadcast::Sender<Vec<u8>>`; a `send` with no subscriber is
  ignored, never fatal.
- `pub fn subscribe(&self) -> broadcast::Receiver<Vec<u8>>`.
- `run()` subscribes **before** writing the command and drains with a
  `try_recv` poll loop (5 ms sleep on `Empty`), because `broadcast::Receiver`
  has **no `recv_timeout`** — it has `recv().await`, `try_recv()` and
  `blocking_recv()` only. This is the wall both prior runs hit; it is solved.
  `Lagged(n)` fails the command with an `Err` naming the lag.
- Four helpers Task 2 needs, all `pub`:

  ```rust
  pub fn write_input(&mut self, bytes: &[u8]) -> anyhow::Result<()>   // write + flush
  pub fn pid(&self) -> Option<u32>                                     // child.process_id()
  pub fn is_alive(&mut self) -> anyhow::Result<bool>                   // child.try_wait().is_none()
  pub fn signal_foreground(&self, sig: &str) -> anyhow::Result<()>     // kill -s <sig> -- -<pgid>
  ```

  `signal_foreground` is `terminate_foreground` generalised: `kill(1)` with
  `-s <NAME>` and the **negative** foreground PGID from
  `master.process_group_leader()`. `terminate_foreground` now calls it with
  `"TERM"`. Task 2's `signal()` maps `ShellSignal::{Int,Term,Stop,Cont}` to
  `"INT"`/`"TERM"`/`"STOP"`/`"CONT"` and calls it. **Nothing else is needed
  for signals** — no `nix`, no `libc`, no crate reading.

**Hard rule for this and every later phase:** never run `kill` from the shell
against `-1`, a negative pid, or a process you did not spawn. The first run of
this phase executed `kill -s INT -- -1` as a probe and took down the
operator's desktop. Signal behaviour is exercised only through the pinned
`backend_*` tests against the PTY child.

### Task 2 — `src/shell/backend.rs`: `PtyBackend`

Create the module, declare it in `src/shell/mod.rs`, re-export `PtyBackend`.

```rust
/// A live shell behind phase-05a's `ShellBackend`: one pump task feeds the
/// subscriber broadcast, the cast log and the screen from the same PTY bytes.
pub struct PtyBackend { /* … */ }

impl PtyBackend {
    /// `shell` is a path or bare name; `cast` is where the recording goes.
    pub fn spawn(shell: &str, rows: u16, cols: u16, scrollback: usize,
                 cast: &Path, started_unix: u64) -> anyhow::Result<Arc<Self>>;
}
```

`spawn` builds a `PtyShell`, a `CastWriter` at `cast`, and a `ShellScreen`,
then starts **one** pump task that, for every chunk the PTY produces, does all
three of:

1. broadcasts it to subscribers,
2. writes it to the cast log as an `"o"` event at the elapsed time,
3. feeds it to the screen.

**All three, for every chunk, in that order.** A chunk that reaches subscribers
but not the log leaves the transcript wrong; one that reaches the log but not
the screen leaves `status` stale. This is the phase's central guarantee and
§ Test plan pins it.

**The pump is a `std::thread`, not a tokio task.** `CastWriter` and
`ShellScreen` are synchronous, and `broadcast::Receiver::blocking_recv` panics
inside a runtime, so a plain thread is the shape that needs no runtime handle
and holds no mutex across an `.await`. Shape (adapt names, keep the structure):

```rust
pub struct PtyBackend {
    shell: Mutex<PtyShell>,          // input / resize / signal / status
    log: Mutex<CastWriter>,          // "i" events from input(); "o" from the pump
    screen: Mutex<ShellScreen>,      // replaced wholesale on resize (F2)
    out_tx: broadcast::Sender<Vec<u8>>, // what subscribe() hands out
    started: Instant,                // the time base every log event uses
    scrollback: usize,               // so resize can rebuild the screen
    _pump: JoinHandle<()>,
}

// use tokio::sync::broadcast::{self, error::RecvError};
// in spawn(), after building shell/log/screen and `let mut rx = shell.subscribe();`
let this = Arc::new(Self { /* … */ });
let pump = {
    let this = Arc::clone(&this);
    std::thread::spawn(move || loop {
        match rx.blocking_recv() {
            Ok(chunk) => {
                let _ = this.out_tx.send(chunk.clone());               // 1. subscribers
                let _ = this                                           // 2. cast log
                    .log
                    .lock()
                    .unwrap_or_log()
                    .write_output(this.started.elapsed(), &chunk);
                this.screen.lock().unwrap_or_log().feed(&chunk);       // 3. screen
            }
            Err(RecvError::Lagged(_)) => continue, // the pump keeps going; § Test plan pins this
            Err(RecvError::Closed) => break,
        }
    })
};
```

`_pump` cannot be a field initialised after `Arc::new` without interior
mutability; the simplest fix is to not store the handle at all (the thread
ends when the PTY closes). Either is acceptable. Lock sites use
`.unwrap_or_log()` (`crate::util::UnpoisonExt`) per CLAUDE.md, never
`.unwrap()`.

`status()` uses `shell.is_alive()` for `state`, `screen.size()` for
`rows`/`cols`, and `shell.pid()` for `pid`.

Trait methods:

- `input(bytes)` — write to the PTY **and** record an `"i"` event in the log at
  the elapsed time. The log's whole point is that a replay shows what was
  typed.
- `resize(rows, cols)` — resize the PTY, then **replace** the screen with a
  fresh `ShellScreen` at the new size (F2). Never `set_size`.
- `signal(sig)` — deliver the signal to the shell's process group. Phase-02
  established the shape in `PtyShell::terminate_foreground`; reuse the same
  mechanism, mapping `ShellSignal::{Int,Term,Stop,Cont}` to `INT`/`TERM`/
  `STOP`/`CONT`. **Pin the negative case:** the signal goes to the *foreground
  group*, never to the whole session, so the shell itself survives an `Int`.
- `status()` — `ShellResponse::Status { state, rows, cols, pid }` with `state`
  `Running` while the child lives and `Exited` once it does not.
- `subscribe()` — a receiver on the same broadcast the pump feeds.

**Error handling:** `anyhow::Context` throughout. No `.unwrap()`, `.expect()`
or `panic!()` outside `#[cfg(test)]`, and **no `unsafe`** — F1 means none is
needed anywhere in this phase.

### Task 3 — Write the tests named in § Test plan

Real PTY against `bash`, `tempfile::tempdir()` for the cast. No fork, no
socket, no `~/.daemoneye/` access. Bound every wait; a hung PTY read must fail
the test rather than hang the suite.

### Task 4 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-05b.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this. Then run the PASTE MATCH self-check
and paste **the literal verdict line it prints** into the same entry.

## Acceptance criteria

Each was run against the current tree while drafting and returns the "before"
value shown.

- [ ] `test -f src/shell/backend.rs && echo yes` → **yes** (absent now).
- [ ] `grep -c '^mod backend;' src/shell/mod.rs` → **1** (now `0`).
- [ ] `grep -cE '^pub struct PtyBackend' src/shell/backend.rs` → **1**.
- [ ] `grep -cE '^    pub fn subscribe' src/shell/pty.rs` → **1** (now `0`).
- [ ] **The resize carry is honoured:** `grep -c 'set_size' src/shell/backend.rs`
      → **0**.
- [ ] No `unsafe`:
      `grep -vE '^\s*(//|///|//!|\*)' src/shell/backend.rs | grep -c 'unsafe'`
      → **0**.
- [ ] No `unwrap`/`expect`/`panic!` outside test code:
      `awk '/^#\[cfg\(test\)\]/{exit} {print}' src/shell/backend.rs | grep -cE '\.(unwrap|expect)\(|panic!\('`
      → **0**. The `^` anchor is required or the guard goes vacuous.
- [ ] `cargo test --lib shell::backend::` reports **6 or more** passing,
      `0 failed` (now: `0 passed; … 1589 filtered out`).
- [ ] `cargo test --lib shell::pty::` still reports **13 passed** — `run()`'s
      contract is unchanged. `shell::host::` **8**, `shell::log::` **12**,
      `shell::screen::` **11**, `shell::proto::` **5**.
- [ ] All four gates pass.

## Test plan

Names pinned; placement is not — `#[cfg(test)] mod tests` at the bottom of
`src/shell/backend.rs`. Every name begins `backend_`.

**The fan-out test is the headline**, and it is written the way this
milestone's hardest lesson requires: *name the two things that happen at once
and test them happening at once.* Three phases in a row here had their defect
on a boundary the spec named in prose and no test crossed.

- `backend_fans_one_chunk_out_to_subscriber_log_and_screen` — **the primary-use
  test.** Spawn a backend, subscribe, run a command producing known output, and
  assert **all three** land: the subscriber receives the bytes, the cast file
  contains them as an `"o"` event, and `status()`'s screen reflects them. One
  test asserting all three, not three tests asserting one each — the defect
  shape this guards against is a chunk reaching two consumers and not the third.
- `backend_streams_while_input_is_written` — **the simultaneity test.** With a
  subscriber attached, write input **while** output is streaming, and assert
  the input reached the shell (its echo or effect appears downstream) and no
  streamed chunk was lost. Bound the wait.
- `backend_records_input_as_an_i_event` — after `input`, the cast file contains
  an `"i"` event whose payload is the bytes written.
- `backend_resize_replaces_the_screen_and_does_not_reflow` — after `resize`,
  `status()` reports the new dimensions, and the module contains no
  `set_size` call (F2). Assert the reported `rows`/`cols` changed.
- `backend_signal_targets_the_foreground_group_not_the_shell` — **the negative
  case.** Send `Int` while a long-running foreground command is up; assert the
  command stops **and** the shell is still alive by running a further command
  successfully afterwards. A signal that killed the shell would pass a weaker
  assertion.
- `backend_status_reports_running_then_exited` — `Running` while alive; after
  the shell exits, `Exited`.
- `backend_lagged_subscriber_does_not_break_the_pump` — a subscriber that stops
  reading must not stall the pump for others: attach two subscribers, let one
  fall behind past capacity, and assert the other still receives current
  chunks.

## End-to-end verification

Run this block verbatim from the repo root. It writes `/tmp/e2e-05b.txt`.

```sh
{
echo "== A. build =="
cargo build 2>&1 | tail -2; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== B. named tests (each line is one pinned test) =="
cargo test --lib 2>&1 | grep -E "^test shell::backend::.* ok$" | sed 's/^test //' | sort
echo "cargo_exit=${PIPESTATUS[0]}"
echo "== C. shell::backend:: totals =="
cargo test --lib shell::backend:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== D. earlier phases untouched (13, 12, 11, 8, 5) =="
for m in pty log screen host proto; do
  cargo test --lib shell::$m:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
done
echo "== E. lib suite totals =="
cargo test --lib 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== F. structural greps (each must print the stated number) =="
echo -n "backend.rs exists       (1): "; test -f src/shell/backend.rs && echo 1 || echo 0
echo -n "mod backend declaration (1): "; grep -c '^mod backend;' src/shell/mod.rs
echo -n "pub struct PtyBackend   (1): "; grep -cE '^pub struct PtyBackend' src/shell/backend.rs
echo -n "PtyShell::subscribe     (1): "; grep -cE '^    pub fn subscribe' src/shell/pty.rs
echo -n "no set_size in backend  (0): "; grep -c 'set_size' src/shell/backend.rs
echo -n "no unsafe in backend    (0): "; grep -vE '^\s*(//|///|//!|\*)' src/shell/backend.rs | grep -c 'unsafe'
echo -n "unwrap/expect/panic pre-test (0): "
awk '/^#\[cfg\(test\)\]/{exit} {print}' src/shell/backend.rs | grep -cE '\.(unwrap|expect)\(|panic!\('
} > /tmp/e2e-05b.txt 2>&1
cat /tmp/e2e-05b.txt
```

Paste the contents of `/tmp/e2e-05b.txt` into your Update Log entry as a fenced
block, then run the self-check and paste its verdict line into the same entry:

```sh
D=docs/dev/milestones/M20-shell-engine/phase-05b-shell-host-backend.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-05b.txt
diff /tmp/pasted-05b.txt /tmp/e2e-05b.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

**Sections B through E can each report success with nothing having run.**
Measured on the current tree: `cargo test --lib shell::backend::` prints
`test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1589 filtered out`
and exits `0`. A zero exit proves nothing — the pass conditions are the named
lines in B, six or more in C, and exactly `13`, `12`, `11`, `8`, `5` in D.

**Section F on an absent file errors rather than printing `0`** — a `grep -c`
against a missing path warns on stderr and exits `2`. The block redirects
`2>&1`, so a warning there is itself proof the file is missing.

## Authorizations

- Create `src/shell/backend.rs`; edit `src/shell/mod.rs`.
- **Do not edit `src/shell/pty.rs`.** Task 1 is landed; its 13 tests must
  still pass **unedited** at the end. Adding a test there is allowed only if a
  Task 2 defect traces back to it, and then the Update Log must say why.
- Run the four gate commands.
- **No new dependencies.**
- May **not** touch `src/shell/log.rs`, `screen.rs`, `proto.rs` or `host.rs`,
  nor anything outside `src/shell/`.
- May **not** touch `docs/architecture.md`, `CLAUDE.md` or `README.md`.

## Out of scope

- **The `daemoneye shell-host` subcommand, detached spawn, and readiness** —
  phase-05c. F1 records that none of it needs `unsafe`, but none of it is
  written here.
- **Any socket.** `PtyBackend` implements the trait; nothing binds or serves.
  05a's `serve` already exists and 05c connects the two.
- **The registry, ids, per-owner caps, adoption, GC** — phase-06.
- **Writing under `~/.daemoneye/`.** Tests use a temp directory.
- **Rebuilding the screen from the cast log on resize.** Replacing it with a
  fresh empty screen is what this phase specifies; reconstructing scrollback
  from the log is a later refinement if a consumer needs it.
- **Masking.** Raw bytes throughout; masking happens where they reach a model.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Notes for executor — 2026-09-03

**Task 1 is already on disk and green; start at Task 2.** Two prior runs of
this phase (sessions `6a9a3720`, interrupted by a host reboot at turn 38, and
`6a9a3deb`, stopped by the operator at turn 102) never left exploration: both
found that `tokio::sync::broadcast::Receiver` has no `recv_timeout` and spent
their turns reading `nix`, `libc` and `portable-pty` sources looking for a way
around it. The second run's only edit called `recv_timeout` on a broadcast
receiver anyway and did not compile. That is a spec gap — Task 1 asked for a
replacement without saying how a synchronous `run()` waits with a deadline —
so the architect executed Task 1 (`try_recv` poll loop) and verified it:
`shell::pty::` **13 passed**, build and clippy clean. Task 1's section now
records exactly what landed, and Task 2 gained a pump skeleton and the four
`PtyShell` helpers it needs. There is no crate source left to read: every API
this phase touches is named in § Spec.

### Update — 2026-09-03 21:30 (escalation)

**Chosen lever:** refined re-dispatch
**Rationale:** the same design-discovery wall stalled two runs in a row and
nothing on disk was reusable, so the architect owns that discovery (Task 1,
executed and verified) and the executor gets the mechanical remainder with a
worked pump skeleton; takeover would forfeit the scorecard data point on a
phase whose remaining tasks are exactly the shape this executor clears.

### Update — 2026-09-04 03:15 (progress)

Executor `p05b-r1` picked up the phase. Work plan: Task 1 rewires `PtyShell`'s
reader thread onto a `tokio::sync::broadcast` channel (capacity 1024, named
const), adds `subscribe()`, and keeps `run()`'s 13 tests green unedited;
Task 2 creates `src/shell/backend.rs` with `PtyBackend` (one pump task feeding
broadcast + cast log + screen, trait methods over the live shell); Task 3 adds
the seven pinned `backend_*` tests; Task 4 captures the E2E evidence block
verbatim and runs the PASTE MATCH self-check.

### Update — 2026-09-04 (escalation)

**Chosen lever:** resume
**Rationale:** the spec held — session `6a9a48eb` died on a transient
infrastructure fault (`backend error: Request failed: error sending request for
url (http://brain:8888/v1/chat/completions)`) at turn 27 with the executor
mid-orientation and about to write `src/shell/backend.rs`, so there is nothing
to refine and nothing to take over.

**Read this before starting: the tree is exactly as the 2026-09-03 notes above
describe it, and nothing new is on disk.** Session `6a9a48eb` was the third run
of this phase and the first to get a fair trial of the refined spec. It changed
**zero files** before the endpoint dropped. Re-verified by the architect at
escalation: `cargo build` clean, `cargo test --lib shell::pty::` **13 passed**,
and `src/shell/backend.rs` **does not exist**. Task 1 remains done (architect-
executed); **start at Task 2**.

Its 27 turns were healthy orientation, not a stall — the novelty sampler
reported 19–20 distinct targets in a 24-call window, context peaked at 26% of
249k, and the final completion before the fault was a full implementation plan
for `PtyBackend`. Nothing in that run indicates a spec gap, so § Spec, §
Acceptance criteria and the § Test plan are unchanged.

**Infrastructure note:** the dead run pointed at `brain:8888`, which is down.
The endpoint is now `brain:8000`, serving `deepseek-ai/DeepSeek-V4-Flash-0731`;
`rexymcp.toml` was corrected by the PE and the architect verified a live
completion against it before resuming. This is not a code-facing fact — it is
recorded so a future reader does not mistake the fault for a phase defect.

### Update — 2026-09-04 22:10 (progress)

Executor `p05b-r2` started at Task 2 (Task 1 already on disk and green, so no
progress entry exists for it before this one). Created `src/shell/backend.rs`
(`PtyBackend`, one pump thread feeding subscriber broadcast → cast log `"o"`
→ screen for every chunk), declared `mod backend;` + `pub use
backend::PtyBackend;` in `src/shell/mod.rs`, and wrote the seven pinned
`backend_*` tests. All four gates pass and `shell::backend::` reports 7
passed; `pty` stays 13.

### Update — 2026-09-04 23:20 (end-to-end verification)

Ran the § End-to-end verification block verbatim from the repo root;
`/tmp/e2e-05b.txt` follows exactly as captured:

```sh
== A. build ==
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s
cargo_exit=0
== B. named tests (each line is one pinned test) ==
shell::backend::tests::backend_fans_one_chunk_out_to_subscriber_log_and_screen ... ok
shell::backend::tests::backend_lagged_subscriber_does_not_break_the_pump ... ok
shell::backend::tests::backend_records_input_as_an_i_event ... ok
shell::backend::tests::backend_resize_replaces_the_screen_and_does_not_reflow ... ok
shell::backend::tests::backend_signal_targets_the_foreground_group_not_the_shell ... ok
shell::backend::tests::backend_status_reports_running_then_exited ... ok
shell::backend::tests::backend_streams_while_input_is_written ... ok
cargo_exit=0
== C. shell::backend:: totals ==
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 1589 filtered out; finished in 20.61s
cargo_exit=0
== D. earlier phases untouched (13, 12, 11, 8, 5) ==
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 1583 filtered out; finished in 2.00s
cargo_exit=0
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 1584 filtered out; finished in 0.00s
cargo_exit=0
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 1585 filtered out; finished in 0.00s
cargo_exit=0
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 1588 filtered out; finished in 0.00s
cargo_exit=0
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 1591 filtered out; finished in 0.00s
cargo_exit=0
== E. lib suite totals ==
test result: ok. 1592 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 24.35s
cargo_exit=0
== F. structural greps (each must print the stated number) ==
backend.rs exists       (1): 1
mod backend declaration (1): 1
pub struct PtyBackend   (1): 1
PtyShell::subscribe     (1): 1
no set_size in backend  (0): 0
no unsafe in backend    (0): 0
unwrap/expect/panic pre-test (0): 0
```

PASTE MATCH self-check: after pasting the block above, the § End-to-end
self-check was run and printed:

```
PASTE MATCH
```

### Update — ts=1788563428399 (complete, server-authored)

**Summary:** All tasks done. The tree is committed clean.

**Summary**

I implemented `src/shell/backend.rs` (`PtyBackend`, ~506 lines incl. tests), declared `mod backend;` + `pub use backend::PtyBackend;` in `src/shell/mod.rs`, and reused Task 1's already-landed `PtyShell::subscribe`/helpers verbatim (`pty.rs` untouched by me; its 13 tests still pass unedited). `PtyBackend::spawn` builds the `PtyShell`, `CastWriter` at the cast path, and `ShellScreen`, then starts **one `std::thread` pump** that, for every chunk in order, (1) broadcasts it to the subscriber sender, (2) writes it as an `"o"` cast event at elapsed time, (3) feeds the screen; lagged subscribers are skipped, `Closed` ends the pump. `ShellBackend` methods: `input` writes + records an `"i"` event, `resize` resizes the PTY then replaces the screen with a fresh `ShellScreen` at the new size (never `set_size`, F2), `signal` maps `Int/Term/Stop/Cont` → `kill -s NAME -- -<fg pgid>` (targets the foreground **group**, shell survives an `Int`), `status` reports `Running`/`Exited` from `is_alive()` plus screen `rows/cols` and the shell pid. All seven pinned `backend_*` tests (Test plan §) pass; I added a small `screen_contents()` accessor on `PtyBackend` so the fan-out test can assert the third consumer directly.

**Deviations / notes for review:** none isolating from the spec — all three consumers of the same chunk are asserted in the primary `backend_fans_one_chunk_out_to_subscriber_log_and_screen` test. One nontrivial adaptation: while debugging the `Running → Exited` transition I briefly wrote a throwaway probe crate under `probes/de-dbg`; it measured that the child was a zombie (`<defunct>`, `try_wait` reaps it → `is_alive()` correctly sees the exit) and it was fully deleted — `git status` is clean. The `status_reports_running_then_exited` test kills the child through `PtyShell::kill()` and polls `is_alive()`/`status` to the exit, because a `TERM` to an idle interactive bash (which ignores `INT`) wouldn't exit it. `status()` swallows a liveness-error as `Exited` behind a `log::error!` (there is no error channel on `status`), and the only production-path `unwrap_or` is `pid().unwrap_or(0)` — a degenerate-but-client-visible `pid` sentinel the phase-05a wire type requires, since the spec forbids error-suppressing idioms for errors you care about; the acceptance-criteria greps (F section) confirm zero `unwrap/expect/panic` outside `#[cfg(test)]` and zero `unsafe`.

**Verification (E2E):** ran the § End-to-end block verbatim; `/tmp/e2e-05b.txt` captured: `shell::backend::` 7 passed, `pty` 13, `log` 12, `screen` 11, `host` 8, `proto` 5, lib suite 1592 passed / 0 failed / 4 ignored, all F greps at the stated values (including `no set_size in backend (0): 0`, `mod backend declaration (1): 1`). The paste-MATCH self-check printed `PASTE MATCH`, and that literal verdict line is recorded in the entry. `cargo fmt --all` (run immediately before `git add`), `cargo build`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test` all pass. Committed as `88e8bbe`; working tree clean. Status left at `in-progress` for the server to flip.

**Executor:** deepseek-ai/DeepSeek-V4-Flash-0731

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
nning 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_reads_bare_word ... ok
test header_status_uses_first_occurrence_only ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test header_status_strips_trailing_prose ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 10 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_tools_counts_are_accurate ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_config_template_documents_every_config_field ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 33 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ghost_config_parsing ... ok
test ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test window_switch_does_not_corrupt_chat ... ignored
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test event_log_append_read ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_below_threshold_discarded ... ok

test result: ok. 31 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 10 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test hooks_land_on_private_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.15s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Compiling daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 2.42s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-9810e08c3de1e001)
     Running unittests src/main.rs (target/debug/deps/daemoneye-54ad57cf3704bc40)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-d709c9b862b5691d)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-34cb94d3012a3d69)
     Running tests/integration.rs (target/debug/deps/integration-deb70358b520d0d5)
     Running tests/isolation.rs (target/debug/deps/isolation-45facff22bac60f5)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M20-shell-engine/phase-05b-shell-host-backend.md` — +63 -0
- `src/shell/backend.rs` — +506 -0
- `src/shell/mod.rs` — +2 -0

**Commit:** 88e8bbe7491ec3ed4747224a86ca32c5523efb2d

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

# Phase 05b: the PTY-backed shell host backend

**Milestone:** M20 — Shell Engine
**Status:** todo
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
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

- `src/shell/` holds `mod.rs`, `pty.rs`, `log.rs`, `screen.rs`, `proto.rs`,
  `host.rs`. Test counts today: `pty` **13**, `log` **12**, `screen` **11**,
  `host` **8**, `proto` **5**.
- **`PtyShell` cannot stream, and this is the phase's central problem.** Its
  output arrives on a private field:

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

### Task 1 — Give `PtyShell` a public output stream

In `src/shell/pty.rs`, replace the private single-consumer channel with a
broadcast so several consumers can read the same output.

- Change the reader thread to publish into a
  `tokio::sync::broadcast::Sender<Vec<u8>>` held by the shell.
- Add `pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Vec<u8>>`.
- **`run()` must keep working exactly as it does today.** Its 13 tests are the
  contract: it takes its own subscription at the start of the call and drains
  that. All 13 must still pass unchanged — do not edit them.
- Broadcast capacity: use **1024** chunks. State it as a named `const` with a
  one-line comment, so the number is visible rather than buried.

**Gotcha, measured in phase-05a:** a `broadcast::Receiver` that falls behind
yields `RecvError::Lagged(n)` rather than an error you can ignore silently.
`run()` must treat `Lagged` as a **failure of that command**, not skip it — a
dropped chunk could be the one carrying the end marker, and silently continuing
would hang until the timeout with no explanation. Return an `Err` naming the
lag.

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
- **Edit `src/shell/pty.rs` for Task 1 only** — the broadcast change and the
  new `subscribe`. Its 13 existing tests must keep passing **unedited**; adding
  tests there is allowed, editing the existing ones is not.
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

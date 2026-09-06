# Phase 07a: the shell-host client — running a command over the socket

**Milestone:** M20 — Shell Engine
**Status:** todo
**Depends on:** phase-05a (`bind`/`serve`, the frame set), phase-05b
(`PtyBackend`), phase-02 (the marker primitives) — all done.
**Estimated diff:** ~320 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Give the daemon a way to **run a command in a shell it does not own** — connect
to `var/run/shells/<id>.sock`, send a marker-wrapped command, accumulate the
output chunks, and return the real exit code.

This is the missing half of the socket protocol. Phase-05a built the server;
nothing in production speaks to it yet. Wiring this into
`run_terminal_command` is **phase-07b**.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.1 — the shell engine and its socket.
- `docs/dev/milestones/M20-shell-engine/README.md` § Notes — the flock/liveness
  and adoption facts from 06b.

## Pre-flight

- `git rev-parse --abbrev-ref HEAD` → `master`, tree clean.
- `cargo test --lib shell::` must report the counts in § Current state before
  you start. If any differs, stop and say so.

## Current state

Counts today: `registry` **17**, `launch` **8**, `pty` **13**, `log` **12**,
`screen` **11**, `host` **8**, `proto` **5**, `backend` **7**.

`src/shell/client.rs` **does not exist**; `cargo test --lib shell::client::`
matches **0** tests today.

**There is no client for this protocol in production code.** The only existing
connect is `launch::await_ready`, which does a `Status` round-trip and nothing
else. Everything else that connects lives in `#[cfg(test)]`.

The pieces you compose already exist and are **exported** — do not
re-implement any of them:

```rust
// src/shell/pty.rs — the marker protocol, all pure
pub struct Nonce(String);              // Nonce::new(), .as_str()
pub fn wrap_command(cmd: &str, nonce: &Nonce, shell_name: &str) -> String;
pub fn parse_outcome(buf: &[u8], nonce: &Nonce) -> Option<CommandOutcome>;
pub struct CommandOutcome { pub output: Vec<u8>, pub exit_code: i32 }

// src/shell/proto.rs — the wire frames
pub enum ShellRequest  { Subscribe, Input { bytes: Vec<u8> }, Resize {..}, Signal {..}, Status }
pub enum ShellResponse { Ok, Error { message }, Chunk { bytes: Vec<u8> }, Status {..}, Exited {..} }
pub fn encode(frame: impl Serialize) -> serde_json::Result<String>;   // appends '\n'
```

## Measured facts — executed 2026-09-06, not reasoned about

**F1 — the whole round trip works, and this is the phase's happy path.**
A probe compiled into the tree at drafting: start `PtyBackend` + `serve` on a
temp socket, connect once, send `Subscribe` then `Input` with a wrapped
command, accumulate `Chunk` bytes and call `parse_outcome` until it answers:

```
PROBE WIRE exit_code=5 output="WIRE_OK\r\n"
```

**Subscribe and Input on the *same* connection is the supported shape** — that
is what phase-05a's `bug-05a-1` fixed, and it is confirmed working. Do not open
two connections.

**F2 — `parse_outcome` is the loop's terminator, not a line count or a
timeout.** It returns `None` until both markers are present in the accumulated
buffer, then `Some(CommandOutcome)` with the framing stripped. The probe's
output came back as exactly `"WIRE_OK\r\n"` — no marker text, no echo.

**F3 — chunks split arbitrarily, so accumulate bytes and re-parse.** Phase-02
measured that a 4096-byte read splits multi-byte UTF-8 characters; both halves
of a 4942-byte stream failed `from_utf8` while the whole buffer was valid.
**Never** decode a chunk on its own — extend a `Vec<u8>` and hand the whole
buffer to `parse_outcome`.

**F4 — `wrap_command` needs the shell's name**, because the exit-code variable
differs (`$status` for fish, `$?` elsewhere — `pty::exit_var`). The socket does
**not** report which shell the host is running, and adding that to the protocol
is out of scope. The client therefore takes `shell_name` as a parameter and the
caller supplies it.

**F5 — the `.cast` and `.meta.json` are created with `File::create`**
(`src/shell/log.rs:34` and `:155`), so their mode depends on the process umask.
Measured at the M20 sign-off: they come out `0600` today, and stay `0600` even
under `umask 022`, because `main()` sets `umask(0o077)`. That is privacy by
accident rather than by construction — the same fragility `host::bind` removed
for the socket by setting the mode explicitly after bind.

## Spec

### Task 1 — `src/shell/client.rs`

```rust
/// Run `cmd` in the shell hosted at `sock` and return its real outcome.
pub async fn run_command(
    sock: &Path,
    cmd: &str,
    shell_name: &str,
    timeout: Duration,
) -> anyhow::Result<CommandOutcome>
```

The sequence, and the order matters:

1. `UnixStream::connect(sock)`, split into a read half (wrapped in
   `BufReader`) and a write half.
2. Send `Subscribe` **before** the command, so no byte of its output can
   precede the subscription. `PtyShell::run` documents the same reasoning at
   `src/shell/pty.rs`:
   ```rust
   // Subscribe before writing, so no byte of this command's output can
   // precede the subscription.
   let mut rx = self.subscribe();
   ```
3. `let nonce = Nonce::new();` then
   `let wrapped = wrap_command(cmd, &nonce, shell_name);` and send it as
   `ShellRequest::Input { bytes: wrapped.into_bytes() }`.
4. Read newline-delimited frames to a deadline. For each frame that
   deserialises to `ShellResponse::Chunk { bytes }`, extend an accumulating
   `Vec<u8>` and call `parse_outcome(&buf, &nonce)`; return on `Some`.
   **Ignore every other frame variant** — `Ok`, `Status`, `Exited` and
   `Error` may interleave and are not this command's output.
5. On deadline, return an error naming the command and the timeout. Do **not**
   return a partial outcome and do **not** invent an exit code.

Use `tokio::time::timeout_at` around the read so a silent shell cannot outlive
the deadline — `launch::await_ready` uses the same shape.

**Do not build marker strings in this module.** `wrap_command` and
`parse_outcome` own that; a second implementation of the marker grammar is a
second thing to keep in sync, and phase-02 spent five rounds getting it right.
A criterion pins `DE_` at **0** occurrences in `client.rs`.

### Task 2 — explicit modes on the log files (F5)

In `src/shell/log.rs`, replace both `File::create(path)` calls (the `.cast` at
:34 and the `.meta.json` at :155) with an explicit `0600` create:

```rust
use std::os::unix::fs::OpenOptionsExt;

let mut file = std::fs::OpenOptions::new()
    .write(true)
    .create(true)
    .truncate(true)
    .mode(0o600)
    .open(path)?;
```

This is the shape `host::bind` already uses for the socket (`chmod 0700` set
explicitly rather than inherited). Privacy must not depend on how the process
was started.

## Acceptance criteria

Each was run against the current tree while drafting and returns the "before"
value shown.

**Two are *preservation* criteria** — `DE_` → 0 and the grammar checks — which
pass trivially today because the file does not exist. They exist to catch a
re-implementation, not to prove work happened. The evidence that work happened
is the `run_command` count, the test totals, and the `log` count moving 12 → 13.

- [ ] `test -f src/shell/client.rs && echo yes` → **yes** (now: absent).
- [ ] `grep -c '^mod client;' src/shell/mod.rs` → **1** (now `0`).
- [ ] `grep -c '^pub async fn run_command' src/shell/client.rs` → **1**.
- [ ] **The marker protocol is reused, not reinvented:**
      `grep -c 'wrap_command' src/shell/client.rs` → **1 or more**,
      `grep -c 'parse_outcome' src/shell/client.rs` → **1 or more**, and
      `grep -c 'DE_' src/shell/client.rs` → **0**.
- [ ] **Chunks are accumulated, not decoded individually:**
      `grep -c 'from_utf8' src/shell/client.rs` → **0** (F3).
- [ ] `grep -c 'mode(0o600)' src/shell/log.rs` → **2** (now `0`) — the `.cast`
      and the `.meta.json`.
- [ ] No `unsafe` in non-test code:
      `awk '/^#\[cfg\(test\)\]/{exit} {print}' src/shell/client.rs | grep -vE '^\s*(//|///|//!|\*)' | grep -c 'unsafe'`
      → **0**.
- [ ] No `unwrap`/`expect`/`panic!` outside test code:
      `awk '/^#\[cfg\(test\)\]/{exit} {print}' src/shell/client.rs | grep -cE '\.(unwrap|expect)\(|panic!\('`
      → **0**.
- [ ] `cargo test --lib shell::client::` reports **6 or more** passing,
      `0 failed` (now: `0 passed`).
- [ ] `cargo test --lib shell::log::` reports **13** — **13, not 12**: Task 2
      adds exactly one test (the mode assertion).
- [ ] Unchanged: `shell::registry::` **17**, `shell::launch::` **8**,
      `shell::pty::` **13**, `shell::screen::` **11**, `shell::host::` **8**,
      `shell::proto::` **5**, `shell::backend::` **7**.
- [ ] All four gates pass.

## Test plan

Names pinned; placement is not. Client tests begin `client_`; the log test
begins `log_`.

**Build the server in-process** — `PtyBackend::spawn` + `host::bind` +
`tokio::spawn(host::serve(...))` against a `tempfile::tempdir()`. That is a
**real** PTY and a **real** socket, and it is exactly what F1's probe did. Do
**not** try to spawn `daemoneye shell-host`: phase-05c measured that
`spawn_detached` resolves `current_exe()` to the libtest binary under
`cargo test`, so it cannot work from a test.

**Any test that spawns a PTY must hold `crate::test_home_guard()` across the
spawn** — `bug-05c-1`: the guard only serialises tests that take it, and a
`PtyShell::spawn` resolving `bash` through `PATH` races any other thread's
`set_var`. Bind it to a named variable (`_home`, not bare `_`).

**The two things that happen at once here are output streaming and command
completion.** `client_run_command_returns_the_real_exit_code` must show both:
a command that *both* prints output *and* exits non-zero, asserting the exit
code **and** the exact output in one test. Two tests asserting one each would
not catch an implementation that returns the code but loses the bytes.

- `client_run_command_returns_the_real_exit_code` — **the primary-use test.**
  Run `printf 'CLIENT_OK\n'; sh -c 'exit 5'`; assert `exit_code == 5` **and**
  the output contains `CLIENT_OK`.
- `client_run_command_output_excludes_the_markers` — **the negative case.**
  The returned `output` must **not** contain `DE_BEG`, `DE_END`, or the
  nonce string. F2 measured that `parse_outcome` already strips them; this
  pins that the client does not hand back the raw buffer.
- `client_run_command_survives_a_chunk_split_mid_marker` — a command whose
  output is large enough to arrive in several chunks (e.g. `head -c 20000
  /dev/zero | tr '\0' 'x'` then a distinct tail) still resolves, proving the
  loop accumulates rather than parsing per chunk (F3).
- `client_run_command_times_out_on_a_silent_command` — `sleep 30` with a
  short timeout returns `Err`, and the error names the timeout. Bound the
  test's own wait.
- `client_run_command_ignores_interleaved_non_chunk_frames` — while the
  command runs, the shell also produces a `Status`-triggering interaction, or
  the test sends an extra `Status` request on the same connection; the reply
  frames must not corrupt the accumulation. Assert the outcome is still
  correct. *(This is the `bug-05a-1` shape from the client side.)*
- `client_run_command_errors_when_the_socket_is_absent` — a path with no
  socket returns `Err` promptly rather than hanging to the deadline.
- `log_cast_and_meta_are_created_mode_0600` — **the Task 2 test.** Create a
  `CastWriter` and write a meta index under a temp dir with a **permissive
  umask** applied in-test (`libc::umask` is not available to you — instead
  assert the mode directly, since `mode(0o600)` is explicit and does not
  depend on the umask). Assert `metadata().permissions().mode() & 0o777 ==
  0o600` for both files.

## End-to-end verification

Run this block verbatim from the repo root. It writes `/tmp/e2e-07a.txt`.

```sh
{
echo "== A. build =="
cargo build 2>&1 | tail -2; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== B. named tests (each line is one pinned test) =="
cargo test --lib 2>&1 | grep -E "^test shell::(client|log)::.* ok$" | sed 's/^test //' | sort
echo "cargo_exit=${PIPESTATUS[0]}"
echo "== C. shell::client:: totals =="
cargo test --lib shell::client:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== D. log is 13; the rest unchanged (17, 8, 13, 11, 8, 5, 7) =="
cargo test --lib shell::log:: 2>&1 | grep -E "^test result:"
for m in registry launch pty screen host proto backend; do
  cargo test --lib shell::$m:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
done
echo "== E. lib suite, three consecutive runs =="
for r in 1 2 3; do cargo test --lib 2>&1 | grep -E "^test result:" | head -1; done
echo "== F. structural greps (each must print the stated number) =="
echo -n "client.rs exists       (1): "; test -f src/shell/client.rs && echo 1 || echo 0
echo -n "mod client declaration (1): "; grep -c '^mod client;' src/shell/mod.rs
echo -n "pub async fn run_command (1): "; grep -c '^pub async fn run_command' src/shell/client.rs
echo -n "wrap_command reused   (>=1): "; grep -c 'wrap_command' src/shell/client.rs
echo -n "parse_outcome reused  (>=1): "; grep -c 'parse_outcome' src/shell/client.rs
echo -n "no marker re-impl      (0): "; grep -c 'DE_' src/shell/client.rs
echo -n "no per-chunk decode    (0): "; grep -c 'from_utf8' src/shell/client.rs
echo -n "explicit 0600 modes    (2): "; grep -c 'mode(0o600)' src/shell/log.rs
echo -n "no unsafe pre-test     (0): "
awk '/^#\[cfg\(test\)\]/{exit} {print}' src/shell/client.rs | grep -vE '^\s*(//|///|//!|\*)' | grep -c 'unsafe'
echo -n "unwrap/expect/panic pre-test (0): "
awk '/^#\[cfg\(test\)\]/{exit} {print}' src/shell/client.rs | grep -cE '\.(unwrap|expect)\(|panic!\('
echo "== G. no shell-host leaked by the test run =="
echo -n "stray daemoneye shell-host processes (0): "
ps -eo comm,args | awk '$1=="daemoneye" && /shell-host/ {n++} END{print n+0}'
} > /tmp/e2e-07a.txt 2>&1
cat /tmp/e2e-07a.txt
```

Paste the contents of `/tmp/e2e-07a.txt` into your Update Log entry as a fenced
block. **The entry's header must be exactly this line**, or the self-check
below cannot find it:

```
### Update — YYYY-MM-DD HH:MM (end-to-end verification)
```

Then run the self-check and paste its verdict line into the same entry:

```sh
D=docs/dev/milestones/M20-shell-engine/phase-07a-shell-client.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-07a.txt
diff /tmp/pasted-07a.txt /tmp/e2e-07a.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

**Sections B through E can each report success with nothing having run.**
`cargo test --lib shell::client::` exits `0` today and prints `0 passed` — a
zero exit proves nothing. The pass conditions are the named lines in B, **six
or more** in C, and exactly `13` for `log` plus `17`, `8`, `13`, `11`, `8`,
`5`, `7` for the rest in D.

**Section E runs three times on purpose** (`bug-05c-1`: a race here reproduced
at roughly one run in six). All three must report `0 failed`.

**Section F on an absent file errors rather than printing `0`** — `grep -c`
against a missing path warns on stderr and exits `2`; the block redirects
`2>&1`, so a warning there is itself proof the file is missing.

**Section G must print `0`.** Do **not** rewrite it as
`pgrep -fc 'daemoneye shell-host'` — measured while drafting phase-06a, that
form reports **1 on a clean tree** because `pgrep -f` matches its own invoking
shell.

## Authorizations

You may edit, and nothing else:

- **create** `src/shell/client.rs`
- `src/shell/mod.rs` — add `mod client;` and one `pub use` line
- `src/shell/log.rs` — **only** the two `File::create` calls and the `use` line
  they need (Task 2), plus the one new test
- this phase doc's Update Log

**Do not** change `host.rs`, `backend.rs`, `pty.rs`, `proto.rs`, `screen.rs`,
`launch.rs` or `registry.rs`. In particular, **do not add frames to the wire
protocol** — F1 proves the existing `Subscribe` + `Input` shape carries a full
command round trip.

**Do not run `pkill`, `killall`, `tmux kill-server`, `kill -- -1`, or any
signal to a process group or to PID -1.**

## Out of scope

- **Wiring this into `run_terminal_command`** — the executor branch on
  `[execution] backend`, get-or-create a shell for the session, the masked and
  annotated tool result, and the real exit code in the `events.jsonl` command
  record. All phase-07b.
- **Masking.** The client returns raw bytes on purpose; masking happens where
  the result is built for the model, which is 07b's job and 07b owns that exit
  criterion.
- Interactive-command routing, signals, pause/resume (phase-08).
- Anything touching the registry, adoption, or the host binary.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

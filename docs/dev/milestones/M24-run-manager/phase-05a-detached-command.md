# Phase 05a: the detached command runner and dedicated shells

**Milestone:** M24 — Run Manager
**Status:** todo
**Depends on:** M20 (the shell substrate: `run_command`, the marker protocol,
the registry). Nothing in M24 phases 01–04 — this phase is pure substrate and
touches no `src/daemon/run/` file.
**Estimated diff:** ~430 lines incl. tests
**Tags:** language=rust, kind=feature, size=m

## Goal

Give the substrate the two primitives `background=true` needs, and nothing
else:

1. **A command you start but do not await.** `run_command`
   (`src/shell/client.rs:111`) writes the command and then blocks until the end
   marker arrives. Background needs the same handshake with the waiting part
   detached: return as soon as the command is *running*, and deliver the real
   `CommandOutcome` later.
2. **A shell nobody else can be handed.** `shell_for_owner`
   (`src/shell/session.rs:17`) reuses an owner's lowest live shell. A
   background command must not share a shell with the owner's foreground
   commands — that is 08b's poisoning rule, the same reason `interactive`
   shells are excluded from reuse.

Phase-05b wires these into the executor, the `[Background Task Completed]`
event and the `Run` model. **This phase changes no user-visible behaviour** —
nothing calls the new functions yet.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.3 "Run manager" — background is "an
  un-awaited Run"; this phase is the "un-awaited" half of the substrate.
- `docs/design/daemoneye-2.0.md` § 2.5 — `run_terminal_command { …, wait?: bool
  (default true) }`; `wait: false` is what these primitives serve.
- `docs/dev/milestones/M20-shell-engine/README.md` § Retrospective — "every
  substrate shell is detached/daemon-owned, so foreground vs background is no
  longer a property of the *shell* but of the *call* — does the turn await the
  end marker or not."

## Pre-flight

Confirm on the current tree before writing code (all values are as of drafting,
2026-09-10, commit `38221c5`):

```sh
git rev-parse --abbrev-ref HEAD                        # master
git status --porcelain | wc -l                         # 0
cargo test --lib shell::client   2>&1 | grep '^test result'   # 9 passed
cargo test --lib shell::session  2>&1 | grep '^test result'   # 7 passed
cargo test --lib shell::registry 2>&1 | grep '^test result'   # 27 passed
cargo test --lib                 2>&1 | grep '^test result'   # 1716 passed
grep -c 'pub async fn spawn_command'    src/shell/client.rs   # 0
grep -c 'pub dedicated: bool'           src/shell/registry.rs # 0
grep -c 'pub async fn spawn_dedicated_for' src/shell/session.rs # 0
```

## Current state

**`run_command` is one function doing three jobs** (`src/shell/client.rs:111-190`):
connect + subscribe, write the wrapped command, then loop reading frames until
`parse_outcome` finds the end marker. Quoted, because the split below must
preserve every line of it:

```rust
pub async fn run_command(
    sock: &Path,
    cmd: &str,
    shell_name: &str,
    timeout: Duration,
) -> anyhow::Result<CommandOutcome> {
    let stream = UnixStream::connect(sock)
        .await
        .with_context(|| format!("connecting to shell socket at {}", sock.display()))?;
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    // Subscribe before writing, so no byte of this command's output can
    // precede the subscription.
    let wire = encode(ShellRequest::Subscribe).context("encoding Subscribe frame")?;
    writer.write_all(wire.as_bytes()).await.context("sending Subscribe frame")?;

    let nonce = Nonce::new();
    let wrapped = wrap_command(cmd, &nonce, shell_name);
    let wire = encode(ShellRequest::Input { bytes: wrapped.into_bytes() })
        .context("encoding Input frame")?;
    writer.write_all(wire.as_bytes()).await.context("sending Input frame")?;
    writer.flush().await.context("flushing Input frame")?;

    let deadline = tokio::time::Instant::now() + timeout;
    let mut line: Vec<u8> = Vec::new();
    let mut buf: Vec<u8> = Vec::new();
    loop {
        line.clear();
        let read = tokio::time::timeout_at(deadline, reader.read_until(b'\n', &mut line)).await;
        // … Ok(Ok(n)) / Ok(Err(e)) / Err(_) arms, n == 0 arm, malformed-frame arm …
        match frame {
            ShellResponse::Chunk { bytes } => {
                // Chunks split arbitrarily (a PTY read can land mid-sequence),
                // so never decode a chunk on its own — accumulate bytes and
                // hand the whole buffer to `parse_outcome`.
                buf.extend_from_slice(&bytes);
                if let Some(outcome) = parse_outcome(&buf, &nonce) {
                    return Ok(outcome);
                }
            }
            _ => { /* Ok, Status, Exited, Error may interleave */ }
        }
    }
}
```

**`shell_for_owner`'s reuse filter** (`src/shell/session.rs:38-42`) — the
exclusion this phase extends, with the comment that says why the precedent
exists:

```rust
    let reuse: Option<ShellId> = with_shells(reg, |store| {
        let mut matches: Vec<ShellId> = store
            .values()
            .filter(|record| {
                // A shell whose terminal an interactive command took passes
                // the liveness probe but eats every byte written to it
                // (08b F2); the owner gets a fresh one instead. The record
                // stays and the host stays alive: later work addresses the
                // session by id.
                !record.interactive && record.owner.key() == owner_key
            })
```

**`shell_for_owner`'s readiness rollback** (`src/shell/session.rs:66-84`) — the
shape `spawn_dedicated_for` must reproduce, because a record for a host that
never came up is worse than no record at all:

```rust
    let id = spawn_for(reg, owner, max_per_owner, spawn).await?;
    let socket = launch::socket_path(id.as_str());
    if let Err(e) = launch::await_ready(&socket, Instant::now() + ready_timeout).await {
        // (4) A record for a host that never came up would be handed to the
        // next caller as a live shell; `spawn_for` rolled back only a spawn
        // failure, so readiness is this function's own responsibility to
        // undo.
        with_shells(reg, |store| { store.remove(id.as_str()); });
        return Err(e).with_context(|| {
            format!("shell {} never became ready at {} within {ready_timeout:?}",
                id.as_str(), socket.display())
        });
    }
    Ok((id, socket))
```

**`spawn_for` sets every `ShellRecord` field at one production site**
(`src/shell/registry.rs:184-203`) — this is where the new field is threaded:

```rust
    let id = with_shells(reg, |store| -> anyhow::Result<ShellId> {
        check_owner_cap(store, &owner, max_per_owner)?;
        let occupied = store.values().map(|record| record.id.index()).collect();
        let id = ShellId::from_index(allocate_index(&occupied));
        let started_unix = /* … */;
        store.insert(
            id.as_str().to_string(),
            ShellRecord {
                id: id.clone(), owner: owner.clone(), started_unix,
                interactive: false, exited_unix: None,
            },
        );
        Ok(id)
    })?;
```

## Spec

Seven tasks. Tasks 1–3 are the dedicated shell; 4–6 the detached runner; 7 the
re-exports.

### Task 1 — `ShellRecord.dedicated`

Add to `ShellRecord` (`src/shell/registry.rs:74`), directly after
`interactive`, with a doc comment in the same voice as its neighbour:

```rust
    /// Set when this shell was created to serve exactly one command for its
    /// whole life — a background run. Unlike `interactive` this is decided at
    /// creation, not discovered later: the shell is never offered to another
    /// command, so a background job and the owner's foreground commands can
    /// never land in the same shell (08b's poisoning rule).
    pub dedicated: bool,
```

### Task 2 — thread it through `spawn_for`

`spawn_for` gains a `dedicated: bool` parameter, placed **after
`max_per_owner`** and before the `spawn` closure, and sets the field from it.
Do not add a second constructor: one construction site is what keeps the
default honest. Update the three call sites named in § Authorizations.

### Task 3 — exclude dedicated shells from reuse, and add `spawn_dedicated_for`

3a. Extend the `shell_for_owner` filter (`src/shell/session.rs:42`) to
`!record.interactive && !record.dedicated && record.owner.key() == owner_key`.

3b. Add to `src/shell/session.rs`:

```rust
/// A ready shell created for one command's exclusive use. Unlike
/// `shell_for_owner` this **never** reuses: a background command gets its own
/// shell or none.
pub async fn spawn_dedicated_for<F, Fut>(
    reg: &ShellRegistry,
    owner: Owner,
    max_per_owner: u32,
    ready_timeout: Duration,
    spawn: F,
) -> anyhow::Result<(ShellId, PathBuf)>
where
    F: FnOnce(ShellId) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
```

It calls `spawn_for(reg, owner, max_per_owner, true, spawn)`, then reproduces
`shell_for_owner`'s readiness wait **and its rollback** verbatim in shape (the
block quoted in § Current state). The owner cap still applies and its error
still propagates: a background command that would exceed `[shells]
max_per_owner` must fail loudly here, not silently borrow a foreground shell.

### Task 4 — split `run_command` into handshake + read loop

Extract two private helpers in `src/shell/client.rs`. The point is that
`run_command` and `spawn_command` **cannot drift**: there is one handshake and
one marker-parsing loop in the file, and both callers use them.

```rust
/// Connect, subscribe, and write the marker-wrapped command. Returns the
/// subscribed reader and the nonce its end marker will carry. Everything that
/// can fail *before the command is running* fails here.
async fn start_command(
    sock: &Path,
    cmd: &str,
    shell_name: &str,
) -> anyhow::Result<(BufReader<tokio::net::unix::OwnedReadHalf>, Nonce)>;

/// Read frames until `parse_outcome` finds this nonce's end marker, the
/// deadline passes, or the socket closes.
async fn read_to_marker(
    reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
    nonce: &Nonce,
    sock: &Path,
    cmd: &str,
    timeout: Duration,
) -> anyhow::Result<CommandOutcome>;
```

`run_command` becomes `start_command(...)` then `read_to_marker(...)`. **Every
existing error message and context string is preserved verbatim** — the nine
`shell::client` tests pin them and must not be edited.

The writer half is dropped when `start_command` returns; the command is already
written and flushed, and nothing later writes to this connection.

### Task 5 — `CommandHandle` and `spawn_command`

```rust
/// A command that was started in a shell but not awaited.
pub struct CommandHandle {
    /// Resolves once the end marker arrives, the timeout expires, or the
    /// socket closes. A dropped receiver simply discards the outcome; the
    /// reader task ends at its own deadline either way.
    pub outcome: tokio::sync::oneshot::Receiver<anyhow::Result<CommandOutcome>>,
    /// The nonce this command's end marker carries — diagnostic only.
    pub nonce: Nonce,
}

/// Start `cmd` in the shell at `sock` and return **as soon as it is running**.
///
/// The handshake — connect, subscribe, write, flush — completes before this
/// returns, so a missing socket, a dead host or an encode failure is an `Err`
/// from this call. After `Ok`, the command is running and its outcome arrives
/// on `CommandHandle::outcome`.
pub async fn spawn_command(
    sock: &Path,
    cmd: &str,
    shell_name: &str,
    timeout: Duration,
) -> anyhow::Result<CommandHandle>;
```

It calls `start_command`, creates a `oneshot::channel`, `tokio::spawn`s a task
that runs `read_to_marker` and sends the result, and returns the handle. The
task owns the reader; it must not borrow anything from the caller.

**The ordering is the contract, and it is what task 6's test guards:** a
failure to *start* is never delivered as a late outcome, because the caller
needs to tell the model "it is running" or "it did not start" synchronously.

### Task 6 — tests

Write the eleven tests named in § Test plan. `shell::client` tests use the real
PTY host helper already in the file (`src/shell/client.rs:205-221`), quoted
here because it carries a gotcha you must not drop:

```rust
    /// A real PTY-backed shell host on a temp socket — exactly the shape F1's
    /// probe used. Avoids `launch::run_host` because that owns the per-id
    /// instance-lock bookkeeping this client does not need.
    struct Host { path: PathBuf, _dir: tempfile::TempDir }

    /// Hold the home guard across the PTY spawn (bug-05c-1): no
    /// `PtyShell::spawn` may overlap another thread's `set_var`.
    async fn spawn_host() -> Host {
        let _home = crate::test_home_guard();
        let dir = tempfile::tempdir().unwrap();
        let cast = dir.path().join("e2e.cast");
        let backend = PtyBackend::spawn("bash", 24, 80, 100, &cast, 0).expect("bash available");
        let path = dir.path().join("shell.sock");
        let listener = host::bind(&path).await.expect("bind the shell socket");
        tokio::spawn(host::serve(listener, Arc::clone(&backend)));
        Host { path, _dir: dir }
    }
```

### Task 7 — re-exports

`src/shell/mod.rs`: add `CommandHandle` and `spawn_command` to the `client`
re-export line (`src/shell/mod.rs:19`) and `spawn_dedicated_for` to the
`session` line (`src/shell/mod.rs:33`). Add `mark_dedicated` **only if** you
need it — the design above sets the flag at creation and does not.

### Task 8 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-m24-05a.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this.

## Acceptance criteria

Each was run against the current tree and returns the "now" value shown.

- [ ] `grep -c 'pub dedicated: bool' src/shell/registry.rs` → **1** (now `0`).
- [ ] `grep -c '!record.dedicated' src/shell/session.rs` → **1** (now `0`).
- [ ] `grep -c 'pub async fn spawn_dedicated_for' src/shell/session.rs` → **1**
      (now `0`).
- [ ] `grep -c 'pub async fn spawn_command' src/shell/client.rs` → **1** (now
      `0`), and `grep -c 'pub struct CommandHandle' src/shell/client.rs` → **1**
      (now `0`).
- [ ] **One handshake, one read loop — the anti-drift property.**
      `grep -c 'async fn start_command' src/shell/client.rs` → **1** (now `0`);
      `grep -c 'async fn read_to_marker' src/shell/client.rs` → **1** (now `0`);
      and **`grep -c 'parse_outcome(&buf' src/shell/client.rs` → still exactly
      `1`** (now `1`), and **`grep -c 'ShellRequest::Subscribe'
      src/shell/client.rs` → still exactly `1`** (now `1`). *(The last two are
      preservation criteria: they fail if the read loop or the handshake was
      copied into `spawn_command` instead of shared.)*
- [ ] **`spawn_for` threads the flag, one construction site:**
      `grep -c 'dedicated: bool' src/shell/registry.rs` → **2 or more** (the
      field plus the parameter; now `0`), and
      `grep -c 'ShellRecord {' src/shell/registry.rs` → still **5** (now `5`) —
      no new construction site. *(5, not 3: the pattern also matches the struct
      definition at `:74` and the test helper's return type at `:394`. The three
      real constructions are `:194`, `:316`, `:395`.)*
- [ ] **The outcome type is a receiver, not a polled flag:**
      `grep -c 'oneshot::Receiver<anyhow::Result<CommandOutcome>>' src/shell/client.rs`
      → **1 or more** (now `0`).
- [ ] No `unsafe`/`unwrap`/`expect`/`panic!` outside test code in either changed
      file:
      `awk '/^#\[cfg\(test\)\]/{exit} {print}' src/shell/client.rs | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe'`
      → **0** (now `0`), same for `src/shell/session.rs` → **0** (now `0`) and
      `src/shell/registry.rs` → **0** (now `0`).
- [ ] **No real sleeps in the new tests.** A test may `sleep` *inside the shell
      command* (that is the subject under test); the Rust side must not:
      `grep -cE 'thread::sleep' src/shell/client.rs` → **0** (now `0`).
- [ ] `cargo test --lib shell::client` → **15 or more** passing, `0 failed`
      (now **9**). *Arithmetic: 9 existing + the 6 client tests in the Test
      plan.*
- [ ] `cargo test --lib shell::session` → **10 or more** passing, `0 failed`
      (now **7**). *7 existing + 3.*
- [ ] `cargo test --lib shell::registry` → **28 or more** passing, `0 failed`
      (now **27**). *27 existing + 1.*
- [ ] **Preservation, each must not move:** `daemon::run::` **50**,
      `daemon::cancel::` **6**, `daemon::executor` **226**,
      `config::path_audit` **24**. This phase touches no daemon file.
- [ ] **The real HOME's shell litter does not grow.**
      `ls ~/.daemoneye/var/run/shells/ 2>/dev/null | wc -l` is **the same before
      and after** a full suite run. It is **5 right now** (`s0.lock`, `s0.sock`,
      `s1.lock`, `s2.lock`, `s3.lock`) — pre-existing litter from M20's
      `shell::launch` tests, recorded as an open item in the milestone README.
      **Do not clean it up here** (that is its own phase) and **do not add to
      it**: every new test in this phase writes under a tempdir HOME. The
      criterion is the delta, not the absolute count.
- [ ] Full lib suite **1726 or more** passing, `0 failed`, on **three
      consecutive runs** (now **1716**).
- [ ] All four gates pass.

## Test plan

**`shell::client`** (6 new; all against the real `spawn_host()` bash PTY):

- `spawn_command_returns_before_the_command_finishes` — start
  `sleep 1; printf 'LATE\n'`. Assert `spawn_command` returned `Ok` **and** the
  receiver has not resolved yet (`try_recv()` is `Err(TryRecvError::Empty)`).
  Then await it and assert it does resolve. This is the whole point of the
  phase: the call returns while the command runs.
- `spawn_command_delivers_the_real_exit_code` — `sh -c 'exit 7'`; the awaited
  outcome's `exit_code` is **7**, not 0 and not 1.
- `spawn_command_delivers_the_output_between_markers` — `printf 'BG_OK\n'`; the
  outcome's `output`, lossy-decoded, contains `BG_OK` and **not** the marker
  literal.
- `spawn_command_errors_when_the_shell_socket_is_absent` — a path with no host
  bound. `spawn_command` itself returns `Err`; **no** handle is produced. The
  negative half is the assertion that matters: a start failure must never be
  delivered as a late outcome.
- `spawn_command_timeout_resolves_the_receiver_with_an_error` — `sleep 30` with
  a 500 ms timeout. `spawn_command` returns `Ok`; the receiver resolves to
  `Err`, and the message names the timeout the same way `run_command`'s does.
- `run_command_and_spawn_command_agree_on_one_command` — the same
  `printf 'PARITY\n'; sh -c 'exit 3'` through both paths yields the same
  `exit_code` and the same decoded output. This is the anti-drift test: it fails
  if the split changed either path's behaviour.

**`shell::registry`** (1 new):

- `spawn_for_sets_dedicated_from_its_argument` — `spawn_for(.., false, ..)`
  leaves the record `dedicated == false`; `spawn_for(.., true, ..)` sets it
  `true`. Both in one test, because the pair is the behaviour.

**`shell::session`** (3 new):

- `dedicated_shell_is_never_reused_by_shell_for_owner` — seed the owner a
  live-looking `dedicated: true` record, then call `shell_for_owner`; it must
  allocate a **different** id. Follow the seeding shape at
  `src/shell/session.rs:270-285`.
- `spawn_dedicated_for_creates_a_new_shell_even_when_one_is_reusable` — seed a
  perfectly reusable non-dedicated shell for the owner; `spawn_dedicated_for`
  must still allocate a new id and mark it `dedicated`.
- `spawn_dedicated_for_removes_the_record_when_readiness_never_arrives` — spawn
  closure succeeds but no host binds; with a short `ready_timeout` the call
  returns `Err` **and** the registry holds no record for the allocated id.

## Mutation checks

Three, each aimed at a property whose guard is otherwise in doubt. Use the
`patch` tool to break and to revert — **`sed -i`, `perl -i` and `> file` are
forbidden by the executor contract and `bash` refuses them.** Record the
red-then-green transcript for each in the Update Log.

Each names **the assertion message expected in the failure output**, not just
the test. A mutation that reddens a multi-assertion test proves only that
*some* assertion fired; quote the line that actually failed, so the check is
evidence about the property you meant to guard.

1. **The handshake must complete before the call returns.** Move the whole body
   of `start_command` — connect included — inside `spawn_command`'s spawned
   task, so the call returns `Ok` before anything has been attempted →
   `spawn_command_errors_when_the_shell_socket_is_absent` must go **red**, and
   the failure must name the "no handle is produced" assertion, not the timeout
   one.
2. **Dedicated shells are excluded from reuse.** Drop `!record.dedicated` from
   the `shell_for_owner` filter → `dedicated_shell_is_never_reused_by_shell_for_owner`
   must go **red** on the "must allocate a different id" assertion.
3. **The real exit code survives the detour.** In `spawn_command`'s task,
   replace the sent outcome's `exit_code` with `0` →
   `spawn_command_delivers_the_real_exit_code` must go **red** naming the
   expected `7`. (`run_command_and_spawn_command_agree_on_one_command` should
   redden too — note whether it did; that is the parity guard proving its worth.)

## End-to-end verification

The real artifact here is the PTY substrate itself: a real `bash` behind a real
PTY, a real socket, and the real marker protocol. The `shell::client` tests
exercise exactly that — there is no CLI door for this phase, and inventing one
would be theatre. Section B runs them with `--nocapture` so the real exit codes
appear in the transcript.

Run from the repo root, verbatim:

```sh
A=/tmp/e2e-m24-05a.txt
: > "$A"
SHELLS_BEFORE=$(ls ~/.daemoneye/var/run/shells/ 2>/dev/null | wc -l)
{
echo "== A. build =="
echo "real HOME shells BEFORE the suite: $SHELLS_BEFORE"
cargo build 2>&1 | tail -2; echo "cargo_exit=${PIPESTATUS[0]}"

echo "== B. the new detached-runner tests, against a real bash PTY =="
cargo test --lib shell::client -- --nocapture 2>&1 \
  | sed 's/\x1b\[[0-9;]*m//g' | grep -E '^test shell::client|^test result'
echo "cargo_exit=${PIPESTATUS[0]}"

echo "== C. module counts (client >=15, session >=10, registry >=28) =="
for m in shell::client shell::session shell::registry; do
  printf '%-18s ' "$m"
  cargo test --lib "$m" 2>&1 | grep '^test result'
done

echo "== D. preservation: run 50, cancel 6, executor 226, path_audit 24 =="
for m in daemon::run:: daemon::cancel:: daemon::executor config::path_audit; do
  printf '%-20s ' "$m"
  cargo test --lib "$m" 2>&1 | grep '^test result'
done

echo "== E. lib suite, three consecutive runs =="
for i in 1 2 3; do cargo test --lib 2>&1 | grep '^test result'; done

echo "== F. structural greps (each must print the stated number) =="
printf 'dedicated field            (1): %s\n' "$(grep -c 'pub dedicated: bool' src/shell/registry.rs)"
printf 'reuse excludes dedicated   (1): %s\n' "$(grep -c '!record.dedicated' src/shell/session.rs)"
printf 'spawn_dedicated_for        (1): %s\n' "$(grep -c 'pub async fn spawn_dedicated_for' src/shell/session.rs)"
printf 'spawn_command              (1): %s\n' "$(grep -c 'pub async fn spawn_command' src/shell/client.rs)"
printf 'CommandHandle              (1): %s\n' "$(grep -c 'pub struct CommandHandle' src/shell/client.rs)"
printf 'start_command helper       (1): %s\n' "$(grep -c 'async fn start_command' src/shell/client.rs)"
printf 'read_to_marker helper      (1): %s\n' "$(grep -c 'async fn read_to_marker' src/shell/client.rs)"
printf 'ONE parse_outcome site     (1): %s\n' "$(grep -c 'parse_outcome(&buf' src/shell/client.rs)"
printf 'ONE Subscribe site         (1): %s\n' "$(grep -c 'ShellRequest::Subscribe' src/shell/client.rs)"
printf 'dedicated threaded       (>=2): %s\n' "$(grep -c 'dedicated: bool' src/shell/registry.rs)"
printf 'ShellRecord sites          (5): %s\n' "$(grep -c 'ShellRecord {' src/shell/registry.rs)"
printf 'oneshot outcome          (>=1): %s\n' "$(grep -c 'oneshot::Receiver<anyhow::Result<CommandOutcome>>' src/shell/client.rs)"
printf 'no rust-side sleep         (0): %s\n' "$(grep -cE 'thread::sleep' src/shell/client.rs)"
for f in src/shell/client.rs src/shell/session.rs src/shell/registry.rs; do
  printf 'no unwrap/panic pre-test %-22s (0): %s\n' "$f" \
    "$(awk '/^#\[cfg\(test\)\]/{exit} {print}' "$f" | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe')"
done

echo "== G. no daemon file touched, no HOME litter =="
printf 'daemon/ diff lines         (0): %s\n' "$(git diff --name-only 38221c5 -- src/daemon/ | wc -l)"
printf 'real HOME shells AFTER (==BEFORE): %s\n' "$(ls ~/.daemoneye/var/run/shells/ 2>/dev/null | wc -l)"
} >> "$A" 2>&1
sed -i 's/\x1b\[[0-9;]*m//g' "$A" 2>/dev/null || true
cat "$A"
```

*(The trailing `sed -i` above is the **architect's** transcript-cleanup line and
is part of this block, not an edit to a source file; if `bash` refuses it, the
`|| true` lets the block finish and section B has already stripped colour.)*

Then, as the last task, the paste self-check:

```sh
D=docs/dev/milestones/M24-run-manager/phase-05a-detached-command.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m24-05a.txt
diff /tmp/pasted-m24-05a.txt /tmp/e2e-m24-05a.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

The executor may modify **only** these files:

- `src/shell/client.rs` — tasks 4, 5, 6.
- `src/shell/session.rs` — tasks 3, 6.
- `src/shell/registry.rs` — tasks 1, 2, 6.
- `src/shell/mod.rs` — task 7 (two re-export lines).
- `docs/dev/milestones/M24-run-manager/phase-05a-detached-command.md` — the
  Update Log only.

**Every `spawn_for` call site**, because task 2 changes its signature — all
three, none optional:

- `src/shell/session.rs:66` (production, inside `shell_for_owner` → pass
  `false`)
- `src/shell/registry.rs:897` (test)
- `src/shell/registry.rs:933` (test)

**Every `ShellRecord` construction site**, because task 1 adds a field — six,
all in the two files above:

- `src/shell/registry.rs:194` (production, `spawn_for`)
- `src/shell/registry.rs:316` (production, startup adoption → `false`: an
  adopted shell's original purpose is unknown, and `false` keeps it reusable,
  which is today's behaviour)
- `src/shell/registry.rs:395` (test helper)
- `src/shell/session.rs:276`, `:324`, `:334` (test helpers)

No other file may be touched. In particular **do not** edit anything under
`src/daemon/` — the wiring is phase-05b, and § G asserts a zero diff there.

## Out of scope

- **The executor arm.** `PendingCall::Background`, `foreground::run_background`
  and the `backend = "pty"` branch in `src/daemon/executor/mod.rs` are
  untouched. Background still runs through tmux `de-bg-*` after this phase.
- **The `[Background Task Completed]` event**, `notify_session`, and anything
  in `src/daemon/background/`.
- **`Run` wiring** — no `RunId`, no turn log, no cancel-registry registration.
  Phase-05a is substrate; it does not know what a `Run` is.
- **Removing the tmux background path.** That is 05b's exit criterion.
- **A `wait` parameter on `run_terminal_command`.** The tool schema is unchanged
  here; § 2.5's rename lands with the tool-surface work.
- **Cancelling a detached command.** Dropping the handle abandons the outcome;
  killing the underlying job is `send_signal`, already shipped, and wiring it to
  a run's cancel signal is 05b's.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

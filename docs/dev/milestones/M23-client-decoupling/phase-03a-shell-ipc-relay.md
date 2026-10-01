# Phase 03a: shell-ipc-relay — the client can list shells, type into one and resize it over IPC

**Milestone:** M23 — Client decoupling & shell UX
**Status:** in-progress
**Depends on:** M20 (shell host protocol), M22 (`list_shells`' row collector)
**Estimated diff:** ~150 lines of production code across 4 files (1 new); 5 new tests
**Tags:** language=rust, kind=feature, size=m

## Goal

Today the chat client has **no** way to see or touch the PTY shells the daemon
runs: no shell request exists in `src/ipc.rs`. After this phase, three
request/response IPC messages relay to the shells:

- `Request::ShellList` → `Response::ShellList { shells }` — every shell the
  daemon knows, with its host, owner, state and last command;
- `Request::ShellInput { id, bytes }` → `Response::Ok` — the bytes are typed
  into that shell;
- `Request::ShellResize { id, rows, cols }` → `Response::Ok` — the shell's
  terminal is resized.

An unknown or exited shell gets `Response::Error` naming it. There is no client
UI: rows 04 (`/shells`) and 07 (attached mode) consume these. **Live
streaming (`ShellTail`) is row 03b**, split out at drafting. The shell host
streams only from the moment of `Subscribe`, with no backlog, so a correct
starting screen is a design problem of its own.

The caller of these messages is **the user**, not the model, so `ShellList`
lists **all** shells, not only one owner's.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.6 — "IPC additions:
  `Request::{ShellList, ShellTail{id, follow}, ShellInput{id, bytes},
  ShellResize{id,w,h}, …}`". This phase does `ShellList`, `ShellInput` and
  `ShellResize`.
- `docs/dev/milestones/M23-client-decoupling/README.md` — rows 03a/03b.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on **2026-10-01** at commit `acc46944`.

Baseline: `cargo test --lib` → **2099 passed / 0 failed / 14 ignored**.

### 1. How a request is handled (`src/daemon/server/mod.rs`)

One request per connection. `handle_client` reads one JSON line, parses a
`Request`, splits the stream into `rx` / `tx`, and dispatches in a `match`.
Simple handlers live in `handlers.rs` and answer with `send_response_split`.
For example (`handlers.rs:19-25`):

```rust
pub(super) async fn handle_ping<W>(tx: &mut W) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    send_response_split(tx, Response::Ok).await?;
    Ok(())
}
```

and its dispatch arm (`mod.rs:172-174`):

```rust
        Request::Ping => {
            handle_ping(&mut tx).await?;
        }
```

`handlers.rs` has **no** test module. Put this phase's handlers in a **new
file** `src/daemon/server/shells.rs` (declare `mod shells;` in
`server/mod.rs`), with its own `#[cfg(test)] mod tests`.

### 2. The shell host client (`src/shell/client.rs`)

`send_input(sock, bytes, timeout)` (`:83-101`) exists, and so do
`send_signal` and `status`. **There is no resize function.** Add one with
exactly `send_input`'s shape, sending `ShellRequest::Resize { rows, cols }`:

```rust
pub async fn send_input(sock: &Path, bytes: &[u8], timeout: Duration) -> anyhow::Result<()> {
    let stream = UnixStream::connect(sock)
        .await
        .with_context(|| format!("connecting to shell socket at {}", sock.display()))?;
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let wire = encode(ShellRequest::Input {
        bytes: bytes.to_vec(),
    })
    .context("encoding Input frame")?;
    writer
        .write_all(wire.as_bytes())
        .await
        .context("sending Input frame")?;
    writer.flush().await.context("flushing Input frame")?;

    read_ack(sock, &mut reader, timeout).await
}
```

**Measured:** against an in-process host, a raw `Resize { rows: 30, cols: 100 }`
frame was acked (`Ok`), and `status()` then returned
`Status { state: Idle, rows: 30, cols: 100, … }`, where it had been `24, 80`.

A shell's socket is `crate::shell::socket_path(id.as_str())`.

### 3. The shell list already exists for the model (`src/daemon/executor/knowledge/shell.rs`)

`collect_row(record: &ShellRecord) -> ShellRow` (`:113-124`) copies what a
listing needs out of a registry record, under the registry lock:

```rust
pub(super) struct ShellRow {
    pub id: String,
    pub owner_key: String,
    pub host: crate::host::HostId,
    pub started_unix: u64,
    pub interactive: bool,
    pub dedicated: bool,
    pub exited_unix: Option<u64>,
}
```

Do **not** call into that module. `ShellRow` is `pub(super)`, and the IPC
answer is a wire type. Build the new list inside the same
`with_shells(reg, |store| …)` shape, mapping each `ShellRecord` to the new
`ShellInfo`.

**Last command.** A shell's recorded command index is at
`crate::shell::meta_path_for(&crate::shell::cast_path(id.as_str(), record.started_unix))`.
`crate::shell::MetaIndex::load(&path)` returns a `MetaIndex`, whose
`commands: Vec<CommandRecord>` each carry a `command: String`. The last
command is `commands.last().map(|c| c.command.clone())`. Load it **after**
releasing the registry lock (collect the ids and start times first). A
missing or unreadable index gives `None`, never an error. A live index file
looks like:

```json
{
  "shell_id": "s3",
  "cast": "s3-1788470000.cast",
  "started_unix": 1788470000,
  "commands": [
    { "index": 0, "command": "cmd 0", "started": 1.0, "ended": 1.5,
      "exit_code": 0, "first_byte": 355, "end_byte": 355 }
  ]
}
```

### 4. The wire-type idiom (`src/ipc.rs`)

List responses carry a `Vec` of a small `#[derive(Debug, Serialize,
Deserialize, Clone, PartialEq)]` struct, as with `ContainerInfo` and
`Response::ScheduleList { jobs: Vec<ScheduleListItem> }`. `Request` and
`Response` are plain (externally tagged) serde enums.

## Spec

### Task 1 — wire types (`src/ipc.rs`)

```rust
/// One PTY shell the daemon runs, as the client sees it (M23).
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ShellInfo {
    /// `s3`.
    pub id: String,
    /// `"local"` or the configured host name (`HostId::as_str()`).
    pub host: String,
    /// The owner's key (`Owner::key()`), e.g. `chat:<session>` or `ghost:<job>`.
    pub owner: String,
    pub started_unix: u64,
    pub interactive: bool,
    pub dedicated: bool,
    /// `Some` once the shell's host has been seen dead.
    pub exited_unix: Option<u64>,
    /// The most recent command recorded in the shell's index, if any.
    pub last_command: Option<String>,
}
```

Add to `Request`:
- `ShellList`;
- `ShellInput { id: String, bytes: Vec<u8> }`;
- `ShellResize { id: String, rows: u16, cols: u16 }`.

Add to `Response`: `ShellList { shells: Vec<ShellInfo> }`. Give each new
variant a one-line `///` doc comment like its neighbours.

### Task 2 — the resize client (`src/shell/client.rs`)

Add `pub async fn send_resize(sock: &Path, rows: u16, cols: u16, timeout: Duration) -> anyhow::Result<()>`,
shaped exactly like `send_input` (§ Current state 2), sending
`ShellRequest::Resize { rows, cols }`, with context strings that say "Resize".
Add `send_resize` to the existing `pub use client::{ …, send_input, … }` list
in `src/shell/mod.rs` (`:20-23`).

### Task 3 — the handlers (`src/daemon/server/shells.rs`, new)

Each takes the injected registry, so tests can use their own:

```rust
pub(super) async fn handle_shell_list<W>(tx: &mut W, reg: &crate::shell::ShellRegistry) -> anyhow::Result<()>
pub(super) async fn handle_shell_input<W>(tx: &mut W, reg: &crate::shell::ShellRegistry, id: &str, bytes: &[u8]) -> anyhow::Result<()>
pub(super) async fn handle_shell_resize<W>(tx: &mut W, reg: &crate::shell::ShellRegistry, id: &str, rows: u16, cols: u16) -> anyhow::Result<()>
```

(`where W: tokio::io::AsyncWriteExt + Unpin`; answer with
`crate::daemon::utils::send_response_split`, as `handle_ping` does.)

- **`handle_shell_list`:** every record in the registry, sorted by shell index
  (`ShellId::parse(id).map(|s| s.index())`, so `s2` lists before `s10`),
  mapped to `ShellInfo` per Task 1, with `last_command` per § Current state 3.
- **`handle_shell_input` / `handle_shell_resize`:** look the id up in the
  registry.
  - If `ShellId::parse` fails, the id is not in the registry, or the record
    has `exited_unix: Some(_)`, answer
    `Response::Error(format!("no live shell {id}"))`.
  - Otherwise call `crate::shell::client::send_input` / `send_resize` on
    `socket_path(id)` with a 5 s timeout. Answer `Response::Ok` on success,
    or `Response::Error(format!("shell {id}: {e:#}"))` on failure.

### Task 4 — dispatch (`src/daemon/server/mod.rs`)

Declare `mod shells;`, then add three arms to the `match request`, each
passing `crate::shell::registry()`:

```rust
        Request::ShellList => {
            shells::handle_shell_list(&mut tx, crate::shell::registry()).await?;
        }
        Request::ShellInput { id, bytes } => {
            shells::handle_shell_input(&mut tx, crate::shell::registry(), &id, &bytes).await?;
        }
        Request::ShellResize { id, rows, cols } => {
            shells::handle_shell_resize(&mut tx, crate::shell::registry(), &id, rows, cols).await?;
        }
```

If another `match` in the crate is over `Request` or `Response` **without** a
catch-all arm (the compiler tells you), add the new variants there the way
neighbouring variants are handled.

### Task 5 — CLAUDE.md

In the **Key files** table, extend the `src/daemon/server/` row's list with
`shells.rs` the shell IPC relay (`handle_shell_list`, `handle_shell_input`,
`handle_shell_resize`).

### Task 6 — tests

Five tests, names pinned.

In `src/ipc_tests.rs`:

1. `shell_ipc_messages_round_trip`: serialize, then deserialize:
   - `Request::ShellList`;
   - `Request::ShellInput { id: "s3", bytes: b"ls\n".to_vec() }`;
   - `Request::ShellResize { id: "s3", rows: 30, cols: 100 }`;
   - `Response::ShellList { shells: vec![<one ShellInfo with every field set>] }`.

   Assert each comes back with the same fields (the `ShellInfo` with
   `assert_eq!`).

In `src/daemon/server/shells.rs` tests. Each holds `crate::test_home_guard()`,
points `HOME` at a temp dir, calls `crate::config::Config::ensure_dirs()`,
creates `crate::config::shells_dir()`, and uses a **fresh**
`ShellRegistry::default()`. Writers are `Vec<u8>`. Parse the one response
line with `serde_json::from_slice::<Response>`.

2. `shell_list_reports_every_shell_with_its_last_command`:
   - register two shells with `crate::shell::registry::spawn_for(&reg, Owner::Chat("t".into()), 5, false, |_id| async { Ok(()) })`
     (a no-op spawn is enough to list);
   - read the first one's `started_unix` with `with_shells`, then write a meta
     index (§ Current state 3 shape, `commands` holding one entry with
     `"command": "echo hi"`) to `meta_path_for(&cast_path(id, started_unix))`,
     creating its parent directory.

   Assert `Response::ShellList` lists **both**, in index order, with
   `host == "local"`, `owner` equal to `Owner::Chat("t".into()).key()`, the
   first's `last_command == Some("echo hi")` and the second's `None`.
3. `shell_input_reaches_the_shell`: register a shell whose spawn is an
   in-process host. Copy the `spawn_in_process_host` fixture from
   `src/daemon/executor/file_ops/ops.rs` tests: a `PtyBackend::spawn("bash", …)`
   served on `socket_path(id)` with its per-id lock held. Send
   `handle_shell_input(…, b"printf in-ok > <tempfile>\n")`. Assert the
   response is `Response::Ok` and, polling up to 5 s, that `<tempfile>` exists
   and holds `in-ok`.
4. `shell_resize_changes_the_shells_size`: the same in-process shell.
   `handle_shell_resize(…, id, 30, 100)` answers `Response::Ok`, and
   `crate::shell::client::status(&socket_path(id), 5 s)` then reports
   `rows: 30, cols: 100`.
5. `shell_ipc_refuses_an_unknown_shell`:
   - `handle_shell_input(…, "s99", b"x")` answers a `Response::Error`
     containing `s99`;
   - `handle_shell_resize(…, "not-a-shell", 1, 1)` answers a `Response::Error`;
   - neither panics.

**Test count:** five new, so **2104 passed / 14 ignored**. **Close every
in-process shell you open** at the end of tests 3 and 4: a leaked host once
made unrelated PTY tests fail at random. The idiom, from
`src/daemon/executor/file_ops/ops.rs:1404-1410`, with your test's registry and
owner:

```rust
let _ = crate::daemon::executor::knowledge::close_shell_with(
    &reg,
    id.as_str(),
    &owner,
    std::time::Duration::from_millis(3000),
)
.await;
```

### Task 7 — capture the end-to-end evidence

Run the block in § End-to-end verification verbatim. Then create a new Update
Log entry titled exactly `### Update — <date> (end-to-end verification)`, and
**make its first fenced block the contents of `/tmp/e2e-m23-03a.txt`**:
the file's lines, copied by a tool. **Never paste the script itself**, and
never retype the lines (the real lines end `lib1=ok ` **with a trailing
space**). Then run the paste check once, and put the line it prints into the
entry **on its own line**.

## Acceptance criteria

Parenthesised values are what each line produces **against the current
tree**. Every non-preservation line fails today; each was run at drafting.

- [ ] `cargo test --lib` → **2104 passed / 0 failed / 14 ignored** on three
      consecutive runs (current: 2099/0/14). Both counts are exact.
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → other targets unmoved: bug_tracker 6, doc_truth 14,
      integration 32, isolation 11 (*preservation*).
- [ ] The five new tests pass:
      ``cargo test --lib -- shell_ipc_messages_round_trip shell_list_reports_every_shell_with_its_last_command shell_input_reaches_the_shell shell_resize_changes_the_shells_size shell_ipc_refuses_an_unknown_shell``
      → **5 passed** (current: `0 passed; … filtered out`).
- [ ] The relay is wired: ``grep -c 'shells::handle_shell_' src/daemon/server/mod.rs``
      → **3** (current: 0), and
      ``grep -c 'pub async fn send_resize' src/shell/client.rs`` → **1**
      (current: 0).
- [ ] Mutations A and B were applied, run, and their observed results recorded
      **in this doc**.
- [ ] The end-to-end entry exists:
      ``grep -c '^### Update — .* (end-to-end verification)$' <this doc>`` →
      **1** (current: 0), its first fenced block is the capture file's
      contents, and the paste-check line is in the doc on its own line:
      ``grep -cx 'PASTE MATCH' <this doc>`` → **1** (current: 0).

## Mutations

Apply each with the `patch` tool (the executor contract forbids `sed -i`,
`perl -i`, `> file` and `rm -rf`), run once, restore, and **record the observed
result in this doc's end-to-end entry**.

- **Mutation A:** make `handle_shell_list` set `last_command: None` for every
  shell. Run test 2. Expected **red**.
- **Mutation B:** make `handle_shell_input` answer `Response::Ok` **without**
  calling `send_input`. Run test 3. Expected **red** (the file never
  appears).

After each restore, confirm `git diff --stat` matches its pre-mutation state.

## Test plan

Five tests:
- the wire round trip;
- the list, with ordering and the last command;
- input landing in a real in-process shell;
- resize changing a real shell's size;
- refusal of unknown ids.

## End-to-end verification

```sh
cd /home/matt/src/daemoneye
{
  for i in 1 2 3; do
    out=$(cargo test --lib 2>&1)
    printf 'lib%d=%s %s\n' "$i" "$(printf '%s\n' "$out" | grep -E '^test result' | tail -1 | grep -oE 'ok|FAILED')" "$(printf '%s\n' "$out" | grep -E '^test .* FAILED$' | sed -E 's/^test (.*) \.\.\. FAILED$/\1/' | tr '\n' ' ')"
  done
  printf 'lib_counts=%s\n' "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1p-\2f-\3i/')"
  printf 'new_tests=%s\n' "$(cargo test --lib -- shell_ipc_messages_round_trip shell_list_reports_every_shell_with_its_last_command shell_input_reaches_the_shell shell_resize_changes_the_shells_size shell_ipc_refuses_an_unknown_shell 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
  printf 'isolation=%s\n' "$(cargo test --test isolation 2>&1 | grep -E '^test result' | tail -1 | grep -oE '[0-9]+ passed; [0-9]+ failed')"
  printf 'wired=%s/%s\n' "$(grep -c 'shells::handle_shell_' src/daemon/server/mod.rs)" "$(grep -c 'pub async fn send_resize' src/shell/client.rs)"
} > /tmp/e2e-m23-03a.txt
cat /tmp/e2e-m23-03a.txt
```

Expected: `lib1..3=ok` with **nothing after `ok`**, `lib_counts=2104p-0f-14i`,
`new_tests=5p-0f`, `isolation=11 passed; 0 failed`, `wired=3/1`.

**Re-run budget: three** (WORKFLOW.md § End-to-end verification). The known
pre-existing flake is `shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker`.
If it fails, name it and re-run the whole block. After three re-runs, stop,
paste the last capture as it is, and name every failing test. Any other
failing test is a finding: report it by name. **If one of your new tests
leaves a shell open, other PTY tests fail at random; close every shell you
open.**

**Run this block with nothing else running `cargo test`.** Then make the
entry's first fenced block the **contents of** `/tmp/e2e-m23-03a.txt` (copied,
not retyped; **never the script above**), and run:

```sh
D=docs/dev/milestones/M23-client-decoupling/phase-03a-shell-ipc-relay.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m23-03a.txt
diff /tmp/pasted-m23-03a.txt /tmp/e2e-m23-03a.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

That snippet compares the entry's **first** fenced block with the capture
file. If it prints `PASTE MISMATCH`, run
`diff /tmp/pasted-m23-03a.txt /tmp/e2e-m23-03a.txt` once and fix what it
shows. **Run the check at most three times.** Put the line it prints into the
entry on its own line, then record both mutations after it.

## Authorizations

You may edit these files, and nothing else:

- `src/ipc.rs`, `src/ipc_tests.rs`: Task 1 and test 1.
- `src/shell/client.rs` (and `src/shell/mod.rs`, only to re-export
  `send_resize`): Task 2.
- `src/daemon/server/shells.rs` (new) and `src/daemon/server/mod.rs`:
  Tasks 3–4 and tests 2–5.
- Any file holding a non-exhaustive-match compile error the new variants
  cause: add the arms only (Task 4's last paragraph).
- `CLAUDE.md`: the one Key files row (Task 5).
- This phase doc's Update Log.

## Out of scope

- **`ShellTail` / live streaming:** row 03b.
- **Any client code** (`src/cli/`): rows 04 onward.
- **Signals (pause, resume, cancel) over IPC:** row 08 owns them, with the
  run-level semantics.
- **Owner scoping of `ShellList`:** the caller is the user, who sees every
  shell.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-01 18:15 (progress)

Starting phase 03a. Status flipped `todo → in-progress`, milestone README row
updated. Work: `ShellInfo` + the three `Request` variants and `Response::ShellList`
(Task 1), `send_resize` on the shell client (Task 2), the three handlers in a new
`src/daemon/server/shells.rs` (Task 3), dispatch arms (Task 4), the CLAUDE.md
Key-files row (Task 5), the five tests (Task 6), and the end-to-end capture
(Task 7). Two exhaustive `Response` matches exist outside this doc's own files
(`src/cli/commands/ask.rs:~216`, `src/cli/commands/stream.rs:~760`) and are
pre-measured to need one ignore-arm line each under Task 4's last paragraph.

Executor: model run for the rexyMCP server.

### Update — 2026-10-01 18:47 (end-to-end verification)

The § End-to-end verification block ran verbatim with nothing else running `cargo test` (re-run budget unused, no flake). Its capture file, `/tmp/e2e-m23-03a.txt`, contents:

```
lib1=ok 
lib2=ok 
lib3=ok 
lib_counts=2104p-0f-14i
new_tests=5p-0f
isolation=11 passed; 0 failed
wired=3/1
```

Paste check (the line the snippet printed, on its own line):

```
PASTE MATCH
```

## Mutations — observed results

**Mutation A** — `handle_shell_list` answered `last_command: None` for every
shell (applied with the `patch` tool, run once, restored):

```
cargo test --lib -- shell_list_reports_every_shell_with_its_last_command
test daemon::server::shells::tests::shell_list_reports_every_shell_with_its_last_command ... FAILED
panicked at src/daemon/server/shells.rs:250:9: the first shell's recorded last
command is on the wire (left: None, right: Some("echo hi"))
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2117 filtered out
```

Restored; `git diff --stat` back to its pre-mutation state (10 files, +142/-4),
and the same filter run printed `1 passed; 0 failed`.

**Mutation B** — `handle_shell_input` answered `Response::Ok` without calling
`send_input` (applied with the `patch` tool, run once, restored):

```
cargo test --lib -- shell_input_reaches_the_shell
test daemon::server::shells::tests::shell_input_reaches_the_shell ... FAILED
(the <tempfile> never appeared; the 5 s existence poll expired)
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2117 filtered out
```

Restored; `git diff --stat` back to its pre-mutation state, and the same filter
run printed `1 passed; 0 failed`.

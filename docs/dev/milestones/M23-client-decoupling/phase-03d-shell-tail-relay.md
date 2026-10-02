# Phase 03d: shell-tail-relay — a client can watch a shell over the daemon socket

**Milestone:** M23 — Client decoupling & shell UX
**Status:** todo
**Depends on:** 03a, 03b, 03c (done)
**Estimated diff:** ~150 lines of production code across 6 files; 6 new tests (~330 lines)
**Tags:** language=rust, kind=feature, size=m

## Goal

After this phase, a client sends `Request::ShellTail { id, follow }` over the
daemon socket and receives:

1. `Response::ShellScreen { bytes }` first: escape codes that redraw the
   shell's current screen (from the host's `SubscribeScreen`). The bytes are
   **empty** when the host is too old to provide a screen.
2. With `follow: true`, every later output chunk as
   `Response::ShellChunk { bytes }`.
3. `Response::ShellExited` when the shell ends. No frame follows it.

The tail stops when the shell exits or the client disconnects. Without
`follow`, only the screen is sent. An unknown or exited id gets
`Response::Error` naming it.

It also clears the two stale comments carried from 03c.

**Lag decision (recorded here, not built):** the host drops `Lagged` chunks
silently, so a viewer that falls 1024 chunks behind desyncs. This phase does
not detect that. A viewer recovers by issuing a fresh `ShellTail`, which
starts from an exact screen. Row 05's `/tail` viewer owns any "resync" key.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.6 — `ShellTail{id, follow}`.
- `docs/dev/milestones/M23-client-decoupling/README.md` — row 03d.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.
4. **Never run `kill`, `pkill` or `killall`.** The end-to-end block stops its
   daemon with a `Shutdown` request and bounds every process with `timeout`.

## Current state

Measured on **2026-10-02** at commit `ff047879`. Baseline:
`cargo test --lib` → **2114 passed / 0 failed / 14 ignored**.

**The whole change below was dry-run in this tree, then reverted.** Results
on the dry run:
- the six tests passed on 6 of 6 runs;
- the full suite was lib **2120**/0/14 on three runs, with every other target
  unmoved;
- clippy `-D warnings` was clean once the type aliases below were added;
- Mutations A, B and C each went red on 6 of 6 runs;
- the end-to-end block's live part, through a **real daemon** that adopted a
  **real** `shell-host`, printed the expected lines.

### Facts the design rests on (all measured)

- A current host answers `SubscribeScreen` with `Screen`, then `Chunk`s, and
  **closes the connection when its shell exits**. The `shell-host` process
  exits too.
- A host from an older binary answers `SubscribeScreen` with
  `ShellResponse::Error { message: "malformed frame: unknown variant …" }`
  and keeps the connection open. It **never** closes the connection when its
  shell exits. Only a `Status` request reveals `state: Exited`. So the tail
  also polls `Status`.
- **The host ends a connection whose write side reaches EOF**
  (`src/shell/host.rs`, `if n == 0 { return Ok(()); }`). The daemon must
  keep its write half to the host alive for the whole tail, or the
  subscription stops.
- `tokio::io::Lines::next_line` is **cancel safe** (tokio 1.53.1,
  `src/io/util/lines.rs`: "This method is cancel safe."), and so is
  `tokio::time::Interval::tick`. That is why both readers below use
  `.lines()` inside `tokio::select!`. Do **not** use `read_line` there: it is
  not cancel safe and can drop part of a frame.
- In `handle_client` (`src/daemon/server/mod.rs:169-170`) the client's read
  half is `let mut rx = BufReader::new(rx_half);`, and the tail reads it to
  notice a hang-up.

### The dispatch arm to copy (`src/daemon/server/mod.rs:182-184`)

```rust
        Request::ShellResize { id, rows, cols } => {
            shells::handle_shell_resize(&mut tx, crate::shell::registry(), &id, rows, cols).await?;
        }
```

### Exhaustive `Response` matches

These sites list every variant, and each needs the three new ones:
- `src/ipc.rs`, `Response::kind()` (`:816`);
- `src/cli/commands/ask.rs:217`;
- `src/cli/commands/stream.rs:756`.

Each already has a `Response::ShellList { .. }` line. Add the new variants
next to it in the same style.

## Spec

### Task 1 — wire types (`src/ipc.rs`)

Add to `Request`, after `ShellResize`:

```rust
    /// Watch one PTY shell (M23): its current screen, then — with `follow` —
    /// its live output until the shell exits or the client disconnects.
    ShellTail {
        id: String,
        follow: bool,
    },
```

Add to `Response`, after `ShellList`:

```rust
    /// The first frame of a `ShellTail`: escape codes that redraw the shell's
    /// current screen. Empty when the shell's host is too old to provide one.
    ShellScreen { bytes: Vec<u8> },
    /// Live output from a followed shell, after its `ShellScreen`.
    ShellChunk { bytes: Vec<u8> },
    /// The followed shell has exited; no more frames follow.
    ShellExited,
```

In `Response::kind()`, add `Response::ShellScreen { .. } => "ShellScreen"`,
`Response::ShellChunk { .. } => "ShellChunk"` and
`Response::ShellExited => "ShellExited"`. In `ask.rs` and `stream.rs`, add
`| Response::ShellScreen { .. }`, `| Response::ShellChunk { .. }` and
`| Response::ShellExited` after the `| Response::ShellList { .. }` line.

### Task 2 — the handler (`src/daemon/server/shells.rs`)

Insert this block **above** the comment `/// The registry half of an
input/resize relay` (which is above `fn live_shell`). It reuses that file's
`live_shell`, `RELAY_TIMEOUT` and `send_response_split`. This is the dry-run
code, already formatted and clippy-clean:

```rust
/// The shell host's answers, one JSON frame per line.
type HostLines = tokio::io::Lines<tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>>;

/// How often a followed tail asks the host whether its shell has exited. A
/// current host closes the connection when its shell ends, but a host from
/// an older binary never does, so the tail also asks.
const TAIL_STATUS_POLL: std::time::Duration = std::time::Duration::from_millis(500);

/// Relay one shell to the client: its current screen as `ShellScreen`, then,
/// with `follow`, each output chunk as `ShellChunk` until the shell exits
/// (`ShellExited`) or the client goes away. `rx` is the client's half of the
/// connection; the tail ends when it reaches EOF.
pub(super) async fn handle_shell_tail<R, W>(
    rx: &mut R,
    tx: &mut W,
    reg: &ShellRegistry,
    id: &str,
    follow: bool,
) -> Result<()>
where
    R: tokio::io::AsyncBufRead + Unpin,
    W: tokio::io::AsyncWriteExt + Unpin,
{
    use tokio::io::AsyncBufReadExt;

    if let Err(msg) = live_shell(reg, id) {
        send_response_split(tx, Response::Error(msg)).await?;
        return Ok(());
    }
    let sock = crate::shell::socket_path(id);
    let stream = match tokio::net::UnixStream::connect(&sock).await {
        Ok(s) => s,
        Err(e) => {
            send_response_split(tx, Response::Error(format!("shell {id}: {e}"))).await?;
            return Ok(());
        }
    };
    // `host_w` must stay alive for the whole tail: the host ends a connection
    // whose write side reaches EOF, which would end the subscription.
    let (host_r, mut host_w) = stream.into_split();
    let mut host = tokio::io::BufReader::new(host_r).lines();

    let screen = match subscribe(&mut host_w, &mut host).await {
        Ok(bytes) => bytes,
        Err(e) => {
            send_response_split(tx, Response::Error(format!("shell {id}: {e:#}"))).await?;
            return Ok(());
        }
    };
    send_response_split(tx, Response::ShellScreen { bytes: screen }).await?;
    if !follow {
        return Ok(());
    }

    let mut client = rx.lines();
    let mut tick = tokio::time::interval(TAIL_STATUS_POLL);
    loop {
        tokio::select! {
            line = host.next_line() => match line {
                Ok(Some(line)) => {
                    if let Ok(crate::shell::ShellResponse::Chunk { bytes }) =
                        serde_json::from_str(&line)
                    {
                        send_response_split(tx, Response::ShellChunk { bytes }).await?;
                    }
                }
                // The host closed the connection: a current host does that
                // when its shell exits.
                Ok(None) | Err(_) => {
                    send_response_split(tx, Response::ShellExited).await?;
                    return Ok(());
                }
            },
            line = client.next_line() => match line {
                Ok(Some(_)) => {}
                // The client hung up; stop relaying.
                Ok(None) | Err(_) => return Ok(()),
            },
            _ = tick.tick() => {
                if shell_has_exited(&sock).await {
                    send_response_split(tx, Response::ShellExited).await?;
                    return Ok(());
                }
            }
        }
    }
}

/// Ask the host for its screen (`SubscribeScreen`). A host from before that
/// request answers `Error`; fall back to `Subscribe` and an empty screen.
async fn subscribe(
    host_w: &mut tokio::net::unix::OwnedWriteHalf,
    host: &mut HostLines,
) -> anyhow::Result<Vec<u8>> {
    use crate::shell::{ShellRequest, ShellResponse, encode};
    use anyhow::Context as _;
    use tokio::io::AsyncWriteExt;

    host_w
        .write_all(encode(ShellRequest::SubscribeScreen)?.as_bytes())
        .await
        .context("sending SubscribeScreen")?;
    match next_frame(host).await? {
        ShellResponse::Screen { bytes } => Ok(bytes),
        ShellResponse::Error { .. } => {
            host_w
                .write_all(encode(ShellRequest::Subscribe)?.as_bytes())
                .await
                .context("sending Subscribe")?;
            match next_frame(host).await? {
                ShellResponse::Ok => Ok(Vec::new()),
                other => anyhow::bail!("unexpected answer to Subscribe: {other:?}"),
            }
        }
        other => anyhow::bail!("unexpected answer to SubscribeScreen: {other:?}"),
    }
}

/// The host's next frame, within the relay timeout.
async fn next_frame(host: &mut HostLines) -> anyhow::Result<crate::shell::ShellResponse> {
    use anyhow::Context as _;
    let line = tokio::time::timeout(RELAY_TIMEOUT, host.next_line())
        .await
        .context("timed out waiting for the shell host")?
        .context("reading from the shell host")?
        .context("the shell host closed the connection")?;
    serde_json::from_str(&line).context("parsing the shell host's frame")
}

/// Whether the host says its shell has exited, or no longer answers at all.
async fn shell_has_exited(sock: &std::path::Path) -> bool {
    matches!(
        crate::shell::status(sock, RELAY_TIMEOUT).await,
        Ok(crate::shell::ShellResponse::Status {
            state: crate::shell::ShellState::Exited,
            ..
        }) | Err(_)
    )
}


    // ── ShellTail ─────────────────────────────────────────────────────────
```

### Task 3 — dispatch (`src/daemon/server/mod.rs`)

After the `Request::ShellResize` arm:

```rust
        Request::ShellTail { id, follow } => {
            shells::handle_shell_tail(&mut rx, &mut tx, crate::shell::registry(), &id, follow)
                .await?;
        }
```

### Task 4 — the two stale comments carried from 03c (`src/daemon/executor/knowledge/shell.rs`)

1. In `close_shell_with`, replace the five-line comment that begins
   `// The wire carries only Int | Term | Stop | Cont — there is no SIGHUP and`
   with:

   ```rust
    // `Close` hangs the shell up (SIGHUP to its pid), which also ends a job
    // running in it; a host too old to know `Close` gets TERM instead. Either
    // is sent exactly once. Whether the shell actually leaves is observed
    // afterwards, not assumed; a host that lingers still holds the lock, so
    // the tool says so instead of claiming success.
   ```

2. Replace the doc comment on `async fn closed_by_observed_end`, which begins
   `/// The observable outcome of one close: poll the lock up to 1.5s from now`
   and ends `/// must not claim a closure it did not see.`, with:

   ```rust
/// The observable outcome of one close: poll the lock up to 1.5s from now
/// and report what is actually seen. A host releases its lock when its shell
/// exits (`serve_until_exit`), so a hung-up shell normally reads as gone well
/// inside the window; one that has not, is reported as still finishing, and
/// the tool's sentence never claims a closure it did not see.
   ```

### Task 5 — tests

Six tests, names pinned.

**`src/ipc_tests.rs`, `shell_tail_messages_round_trip`:**
- `Request::ShellTail { id: "s3", follow: true }` round-trips with both
  fields;
- `Response::ShellScreen { bytes: vec![0x1b, b'[', b'H', 0xff] }` and
  `Response::ShellChunk { bytes: b"out".to_vec() }` round-trip with their
  bytes (use the file's `roundtrip_resp`);
- `Response::ShellExited` round-trips.

**`src/daemon/server/shells.rs` tests.** These use the file's existing
`shell_test_home()`, `spawn_in_process_host`, `parse_response` and
`ShellRegistry::default()`. Add these helpers at the end of the test module
first (dry-run code; the type alias is what keeps clippy's
`type_complexity` quiet):

```rust
// ── ShellTail ─────────────────────────────────────────────────────────

/// The client's end of a tail: the response lines it reads.
type ClientLines =
    tokio::io::Lines<tokio::io::BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>>;

/// Every response line `buf` holds, parsed.
fn parse_all(buf: &[u8]) -> Vec<Response> {
    String::from_utf8_lossy(buf)
        .lines()
        .map(|l| serde_json::from_str::<Response>(l).expect("a response line parses"))
        .collect()
}

/// The next response the tail wrote to the client, within 10 s.
async fn next_response(lines: &mut ClientLines) -> Response {
    let line = tokio::time::timeout(std::time::Duration::from_secs(10), lines.next_line())
        .await
        .expect("a response within 10 s")
        .expect("reading the client side")
        .expect("the tail is still writing");
    serde_json::from_str(&line).expect("a response line parses")
}

/// Render escape-code bytes on a fresh 24×80 terminal and return the text.
fn rendered(bytes: &[u8]) -> String {
    let mut p = vt100::Parser::new(24, 80, 0);
    p.process(bytes);
    p.screen().contents()
}

/// Run `handle_shell_tail` against `reg` in a task with a duplex standing
/// in for the client connection. Returns the client's line reader, its
/// write half (drop it to hang up), and the task.
fn spawn_tail(
    reg: ShellRegistry,
    id: String,
) -> (
    ClientLines,
    tokio::io::WriteHalf<tokio::io::DuplexStream>,
    tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    use tokio::io::AsyncBufReadExt;
    let (client, server) = tokio::io::duplex(1 << 20);
    let (server_r, mut server_w) = tokio::io::split(server);
    let task = tokio::spawn(async move {
        let mut rx = tokio::io::BufReader::new(server_r);
        handle_shell_tail(&mut rx, &mut server_w, &reg, &id, true).await
    });
    let (client_r, client_w) = tokio::io::split(client);
    (tokio::io::BufReader::new(client_r).lines(), client_w, task)
}
```

and this fake **old** host, a `spawn_for` spawn closure like
`spawn_in_process_host`:

```rust
/// A host that predates `SubscribeScreen`: it refuses that frame the way
/// an older binary does, accepts `Subscribe` and sends one chunk, and
/// answers `Status` as idle.
async fn spawn_old_host(id: crate::shell::ShellId) -> anyhow::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let listener = crate::shell::bind(&crate::shell::socket_path(id.as_str())).await?;
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (r, mut w) = stream.into_split();
                let mut lines = tokio::io::BufReader::new(r).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let reply: &[u8] = if line.contains("SubscribeScreen") {
                        b"{\"type\":\"Error\",\"message\":\"malformed frame: unknown variant `SubscribeScreen`\"}\n"
                    } else if line.contains("Subscribe") {
                        b"{\"type\":\"Ok\"}\n{\"type\":\"Chunk\",\"bytes\":[111,108,100]}\n"
                    } else {
                        b"{\"type\":\"Status\",\"state\":\"Idle\",\"rows\":24,\"cols\":80,\"pid\":0}\n"
                    };
                    if w.write_all(reply).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    Ok(())
}
```

Then the five tests. Each in-process shell is closed at the end with the
file's existing `close_shell_with` idiom, **except** in test 3, where the
shell has already exited.

1. **`shell_tail_without_follow_sends_only_the_screen`**:
   - spawn an in-process shell;
   - `crate::shell::send_input(&socket_path(id), b"printf 'marker-tail-a\\n'\n", RELAY_TIMEOUT)`;
   - poll (sleep 50 ms, up to 10 s): call
     `handle_shell_tail(&mut BufReader::new(tokio::io::empty()), &mut Vec::new(), &reg, id, false)`
     until `parse_all` gives exactly one `ShellScreen` whose `rendered(bytes)`
     contains `marker-tail-a`;
   - assert exactly one frame, and that it is that screen.
2. **`shell_tail_follow_streams_live_chunks_until_the_client_disconnects`**:
   - `spawn_tail(reg.clone(), id)`;
   - the first `next_response` is `ShellScreen`;
   - `send_input` `b"printf 'marker-tail-b\\n'\n"`;
   - read `ShellChunk`s, accumulating bytes, until they contain
     `marker-tail-b` (any other frame panics);
   - then `drop(client_w); drop(lines);` and the task finishes within 5 s
     with `Ok(Ok(Ok(())))`.
3. **`shell_tail_reports_exited_when_the_shell_ends`**:
   - `spawn_tail`; the first frame is `ShellScreen`;
   - `send_input` `b"exit\n"`;
   - read frames, skipping `ShellChunk`s, until `ShellExited` (any other
     frame panics);
   - the task finishes within 5 s with `Ok(Ok(Ok(())))`.

   The in-process host keeps its connection open after the shell exits, so
   this test exercises the `Status` poll path.
4. **`shell_tail_falls_back_to_subscribe_on_an_old_host`**:
   - register a shell whose spawn is `spawn_old_host`;
   - `spawn_tail`;
   - the first frame is `ShellScreen` with **empty** bytes;
   - the next is `ShellChunk { bytes: b"old" }`;
   - drop the client, and the task ends within 5 s.
5. **`shell_tail_refuses_an_unknown_shell`**:
   - `handle_shell_tail(…, "s99", true)` on an empty registry, with an empty
     `rx` and a `Vec<u8>` `tx`;
   - the one response is `Response::Error` containing `s99`.

**Test count:** six new, so **2120 passed / 14 ignored**.

### Task 6 — capture the end-to-end evidence

Run the block in § End-to-end verification verbatim. Then create a new Update
Log entry titled exactly `### Update — <date> (end-to-end verification)`.
**Its first fenced block is the contents of `/tmp/e2e-m23-03d.txt`**: the
file's lines, copied by a tool. **Never paste the script itself**, and never
retype the lines (the real lines end `lib1=ok ` **with a trailing space**).
Then run the paste check once, put the line it prints into the entry **on its
own line**, and record the three mutations after it.

## Acceptance criteria

Parenthesised values are what each line produces against the current tree.
Every non-preservation line fails today, and each was run at drafting.

- [ ] `cargo test --lib` → **2120 passed / 0 failed / 14 ignored** on three
      consecutive runs (current: 2114/0/14). Both counts are exact.
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → other targets unmoved: bug_tracker 6, doc_truth 14,
      integration 32, isolation 11 (*preservation*).
- [ ] The six new tests pass:
      ``cargo test --lib -- shell_tail`` → **6 passed** (current:
      `0 passed`).
- [ ] **Through a real daemon** (end-to-end block): `live_first=ShellScreen`
      (current: `Error`, since the daemon does not know the request),
      `live_chunk=True` (current: `False`), `live_end=ShellExited,EOF`
      (current: `EOF`). `live_list=s1:adopted` and `live_host_exit=0` are
      *preservation*: both hold today.
- [ ] The carried comments are gone:
      ``grep -c 'there is no SIGHUP' src/daemon/executor/knowledge/shell.rs``
      → **0** (current: 1), and
      ``grep -c 'genuinely produces' src/daemon/executor/knowledge/shell.rs``
      → **0** (current: 1).
- [ ] Mutations A, B and C were applied, run, and their observed results
      recorded **in this doc**.
- [ ] The end-to-end entry exists:
      ``grep -c '^### Update — .* (end-to-end verification)$' <this doc>`` →
      **1** (current: 0). Its first fenced block is the capture file's contents,
      and the paste-check line is on its own line:
      ``grep -cx 'PASTE MATCH' <this doc>`` → **1** (current: 0).

## Mutations

Apply each with the `patch` tool (the executor contract forbids `sed -i`,
`perl -i`, `> file` and `rm -rf`). Run once, restore, and **record the
observed result**. Each was measured red on 6 of 6 runs at drafting.

- **Mutation A:** in `handle_shell_tail`, send
  `Response::ShellScreen { bytes: Vec::new() }` instead of the host's
  screen. Run test 1. Expected **red**.
- **Mutation B:** in `subscribe`, make the `ShellResponse::Error { .. }`
  arm unreachable (add `if false` to it). Run test 4. Expected **red**.
- **Mutation C:** make `shell_has_exited` return `false` always. Run test 3.
  Expected **red** (no `ShellExited` within 10 s).

After each restore, confirm `git diff --stat` matches its pre-mutation state.

## Test plan

Six tests:
- the wire round trip;
- screen only, without follow;
- live chunks until the client hangs up;
- `ShellExited` when the shell ends;
- the old-host fallback;
- refusal of an unknown id.

## End-to-end verification

```sh
cd /home/matt/src/daemoneye
cargo build -q
{
  for i in 1 2 3; do
    out=$(cargo test --lib 2>&1)
    printf 'lib%d=%s %s\n' "$i" "$(printf '%s\n' "$out" | grep -E '^test result' | tail -1 | grep -oE 'ok|FAILED')" "$(printf '%s\n' "$out" | grep -E '^test .* FAILED$' | sed -E 's/^test (.*) \.\.\. FAILED$/\1/' | tr '\n' ' ')"
  done
  printf 'lib_counts=%s\n' "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1p-\2f-\3i/')"
  printf 'new_tests=%s\n' "$(cargo test --lib -- shell_tail 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
  printf 'isolation=%s\n' "$(cargo test --test isolation 2>&1 | grep -E '^test result' | tail -1 | grep -oE '[0-9]+ passed; [0-9]+ failed')"
  printf 'stale_comments=%s/%s\n' "$(grep -c 'there is no SIGHUP' src/daemon/executor/knowledge/shell.rs)" "$(grep -c 'genuinely produces' src/daemon/executor/knowledge/shell.rs)"
  H=$(mktemp -d /tmp/de3d.XXXX)
  ( HOME=$H timeout 30 target/debug/daemoneye shell-host --id s1 --shell bash >/dev/null 2>&1; echo "$?" > $H/host_rc ) &
  for i in $(seq 1 50); do [ -S $H/.daemoneye/var/run/shells/s1.sock ] && break; sleep 0.1; done
  ( env -u TMUX -u TMUX_PANE HOME=$H ANTHROPIC_API_KEY=dummy timeout 30 target/debug/daemoneye daemon --console --session "" >/dev/null 2>&1 ) &
  for i in $(seq 1 100); do [ -S $H/.daemoneye/var/run/daemoneye.sock ] && break; sleep 0.1; done
  python3 - $H/.daemoneye/var/run/daemoneye.sock <<'EOF'
import socket, sys, json, time
SOCK = sys.argv[1]
def one(req):
    s = socket.socket(socket.AF_UNIX); s.settimeout(5); s.connect(SOCK)
    s.sendall((json.dumps(req) + "\n").encode()); d = b""
    while not d.endswith(b"\n"):
        d += s.recv(65536)
    s.close(); return json.loads(d)
shells = one("ShellList").get("ShellList", {}).get("shells", [])
print("live_list=" + ",".join(x["id"] + ":" + x["owner"] for x in shells))
t = socket.socket(socket.AF_UNIX); t.settimeout(0.5); t.connect(SOCK)
t.sendall(b'{"ShellTail":{"id":"s1","follow":true}}\n')
buf = b""; kinds = []; out = b""
def pump(secs):
    global buf, out
    end = time.time() + secs
    while time.time() < end:
        try:
            d = t.recv(65536)
        except socket.timeout:
            continue
        if not d:
            kinds.append("EOF"); return
        buf += d
        while b"\n" in buf:
            line, buf = buf.split(b"\n", 1); r = json.loads(line)
            k = r if isinstance(r, str) else list(r)[0]; kinds.append(k)
            if k == "ShellChunk":
                out += bytes(r["ShellChunk"]["bytes"])
pump(1.5)
print("live_first=" + (kinds[0] if kinds else "none")); kinds.clear()
one({"ShellInput": {"id": "s1", "bytes": list(b"printf 'live-door\\n'\n")}})
pump(1.5)
print("live_chunk=" + str(b"live-door" in out)); kinds.clear()
one({"ShellInput": {"id": "s1", "bytes": list(b"exit\n")}})
pump(5)
print("live_end=" + ",".join(kinds[-2:]))
one("Shutdown")
EOF
  wait
  printf 'live_host_exit=%s\n' "$(cat $H/host_rc)"
  find "$H" -depth -delete
} > /tmp/e2e-m23-03d.txt
cat /tmp/e2e-m23-03d.txt
```

The live part runs the real binaries:
1. It starts a `shell-host` (`s1`) in a throwaway HOME, under `timeout 30`.
2. It starts a daemon there with no tmux (`TMUX` and `TMUX_PANE` unset,
   `--session ""`). The daemon adopts `s1` at startup. The API key is a dummy:
   no model is called.
3. It lists the shells, tails `s1` with `follow`, types
   `printf 'live-door\n'`, then types `exit`.
4. It stops the daemon with a `Shutdown` request; nothing is killed.

`live_host_exit` is the `shell-host`'s own exit code: `0` means it stopped by
itself. The `/tmp/de3d.XXXX` path is short on purpose, because Unix socket
paths over 108 bytes fail to bind.

Expected:
- `lib1..3=ok` with **nothing after `ok`**;
- `lib_counts=2120p-0f-14i`;
- `new_tests=6p-0f`;
- `isolation=11 passed; 0 failed`;
- `stale_comments=0/0`;
- `live_list=s1:adopted`;
- `live_first=ShellScreen`;
- `live_chunk=True`;
- `live_end=ShellExited,EOF`;
- `live_host_exit=0`.

**Re-run budget: three** (WORKFLOW.md § End-to-end verification). The known
pre-existing flake is `shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker`.
If it fails, name it and re-run the whole block. After three re-runs, stop,
paste the last capture as it is, and name every failing test. Any other
failing test is a finding: report it by name.

**Run this block with nothing else running `cargo test`.** A concurrent run
of the PTY tests makes them fail spuriously. Then make the entry's first
fenced block the **contents of** `/tmp/e2e-m23-03d.txt` (copied, not retyped;
**never the script above**), and run:

```sh
D=docs/dev/milestones/M23-client-decoupling/phase-03d-shell-tail-relay.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m23-03d.txt
diff /tmp/pasted-m23-03d.txt /tmp/e2e-m23-03d.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

If it prints `PASTE MISMATCH`, run
`diff /tmp/pasted-m23-03d.txt /tmp/e2e-m23-03d.txt` once and fix what it
shows. **Run the check at most three times.** Put the line it prints into the
entry on its own line.

## Authorizations

You may edit these files, and nothing else:

- `src/ipc.rs` and `src/ipc_tests.rs`: Task 1 and test 1.
- `src/cli/commands/ask.rs` and `src/cli/commands/stream.rs`: the three match
  arms only.
- `src/daemon/server/shells.rs` and `src/daemon/server/mod.rs`: Tasks 2–3
  and tests 2–6.
- `src/daemon/executor/knowledge/shell.rs`: the two comments in Task 4 only.
- This phase doc's Update Log.

## Out of scope

- **Any client UI**: `/shells` is row 04, `/tail` is row 05.
- **Lag recovery** (see Goal): a viewer re-issues `ShellTail`.
- **Input or resize over the same connection**: `ShellInput` and
  `ShellResize` (03a) are separate requests.

## Update Log

<!-- entries appended below this line -->

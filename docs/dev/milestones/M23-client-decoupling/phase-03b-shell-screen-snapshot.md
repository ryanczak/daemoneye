# Phase 03b: shell-screen-snapshot — a shell host can hand a new subscriber its current screen, then the live stream

**Milestone:** M23 — Client decoupling & shell UX
**Status:** in-progress
**Depends on:** 03a (done)
**Estimated diff:** ~60 lines of production code across 4 files; 5 new tests
**Tags:** language=rust, kind=feature, size=s

## Goal

Today a shell host streams output only from the moment a client sends
`Subscribe`. A client that starts watching a busy shell sees nothing until the
next byte, and then only a fragment. After this phase, a new host request,
`ShellRequest::SubscribeScreen`, is answered first with
`ShellResponse::Screen { bytes }`. Those bytes are escape codes that redraw
the host's current screen exactly. Every later output chunk follows as
`Chunk`, the same way `Subscribe` streams today.

`Screen` and `Chunk` are made **atomic**: no output byte can fall between
the snapshot and the subscription, and none can appear in both.

This phase is **host side only**. The daemon's `ShellTail` relay, which
row 05 `/tail` will use, is row 03c.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.6 — `ShellTail{id, follow}`. This phase
  provides the starting screen that request needs.
- `docs/dev/milestones/M23-client-decoupling/README.md` — rows 03b/03c.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on **2026-10-01** at commit `fadc8ebe`. Baseline:
`cargo test --lib` → **2105 passed / 0 failed / 14 ignored**.

**The whole change below was dry-run in this tree, then reverted.** Results
on the dry run:
- the five tests passed on 6 of 6 runs;
- the full lib suite gave 2110/0/14 on three runs;
- clippy `-D warnings` was clean;
- Mutations A, B and C each went red on 6 of 6 runs.

### 1. The host protocol (`src/shell/proto.rs:29-58`)

```rust
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum ShellRequest {
    Subscribe,
    Input { bytes: Vec<u8> },
    Resize { rows: u16, cols: u16 },
    Signal { signal: ShellSignal },
    Status,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum ShellResponse {
    Ok,
    Error {
        message: String,
    },
    Chunk {
        bytes: Vec<u8>,
    },
    Status { … },
    Exited { … },
}
```

A host that does not know a request type answers
`ShellResponse::Error { message: "malformed frame: …" }` and keeps the
connection open (`src/shell/host.rs:133-145`). An adopted host running an
older binary will therefore refuse `SubscribeScreen` cleanly. The 03c relay
will fall back to `Subscribe`.

### 2. The host's request handler (`src/shell/host.rs:149-152`) and its trait (`:20-27`)

```rust
        ShellRequest::Subscribe => {
            *chunks = Some(backend.subscribe());
            ShellResponse::Ok
        }
```

```rust
pub trait ShellBackend: Send + Sync + 'static {
    async fn input(&self, bytes: &[u8]) -> anyhow::Result<()>;
    async fn resize(&self, rows: u16, cols: u16) -> anyhow::Result<()>;
    async fn signal(&self, sig: ShellSignal) -> anyhow::Result<()>;
    async fn status(&self) -> ShellResponse;
    /// A receiver of output chunks for one subscriber.
    fn subscribe(&self) -> broadcast::Receiver<Vec<u8>>;
}
```

`handle_line` writes the arm's response **before** the connection loop
starts draining `chunks`. So a `Screen` frame returned from the new arm
always reaches the client before any `Chunk` from the same receiver. That
ordering comes for free.

There are **five** `ShellBackend` impls: `PtyBackend`, plus four test fakes
(`host.rs:239`, `client.rs:980`, `:1050`, `:1174`). Give the new method a
**default body** in the trait so none of the fakes changes.

### 3. Why the pump must change (`src/shell/backend.rs:117-139`)

The pump thread sends each chunk to subscribers, then writes the log, then
feeds the screen. The three steps are not under one lock:

```rust
                    Ok(chunk) => {
                        if looks_like_secret_prompt(&String::from_utf8_lossy(&chunk)) {
                            pump_this.awaiting_secret.store(true, Ordering::SeqCst);
                        }
                        let _ = pump_this.out_tx.send(chunk.clone());
                        let log_res = pump_this
                            .log
                            .lock()
                            .unwrap_or_log()
                            .write_output(pump_this.started.elapsed(), &chunk);
                        if let Err(e) = log_res {
                            log::error!("shell host: writing cast log: {e:#}");
                        }
                        pump_this.screen.lock().unwrap_or_log().feed(&chunk);
                    }
```

Suppose a snapshot is taken under the screen lock and the subscription made
in the same critical section. A chunk already **sent** (before the
subscription) but not yet **fed** (blocked on our lock) appears in neither,
so it is lost. The fix is to **feed and send under the screen lock**, in
that order. A snapshot taken under the same lock then sees every chunk the
subscription will not.

Keep the other two orderings exactly as they are:
- the secret-prompt arm stays **before** the send (see the field comment at
  `backend.rs:76-79`);
- the cast-log write stays **after** the send.

**Measured:** this race is **not** observable by any test here. A pump
reverted to the old order passed the property test on 12 of 12 runs. The
ordering is therefore checked at review by reading the code, and is **not**
a pinned mutation.

### 4. The screen (`src/shell/screen.rs:40-66`)

`ShellScreen` wraps a `vt100::Parser` (`vt100 = "0.16"`, resolved 0.16.2).
vt100 0.16.2's own doc for the method this phase uses:

```rust
    /// Return escape codes sufficient to reproduce the entire contents of the
    /// current terminal state. This is a convenience wrapper around
    /// [`contents_formatted`](Self::contents_formatted) and
    /// [`input_mode_formatted`](Self::input_mode_formatted).
    #[must_use]
    pub fn state_formatted(&self) -> Vec<u8> {
```

**Measured with vt100 0.16.2:** `state_formatted()` fed into a fresh 24×80
parser reproduces `contents_formatted()` and `cursor_position()` exactly:
- for a coloured prompt;
- for 60 scrolled lines;
- for a hidden cursor;
- and a live chunk applied to both afterwards keeps them equal.

**Gotcha, measured:** for an alt-screen app (vim, less) the visible content
is reproduced, but the receiving parser is **not** in alt-screen mode.
Prefixing `b"\x1b[?1049h"` when `screen().alternate_screen()` is true fixes
that. Known residual: when the app later exits the alt screen, the client has
no primary screen to return to and shows blank. Accepted, and documented in
the doc comment.

## Spec

### Task 1 — protocol (`src/shell/proto.rs`)

- Add `SubscribeScreen` to `ShellRequest`, right after `Subscribe`.
- Add `Screen { bytes: Vec<u8> }` to `ShellResponse`, right after `Chunk`.
- Give each a one-line `///` doc comment.

### Task 2 — the snapshot (`src/shell/screen.rs`)

Add to `impl ShellScreen`:

```rust
    /// Escape codes that redraw this screen exactly on a fresh terminal of
    /// the same size: contents, colours, cursor and input modes
    /// (`vt100::Screen::state_formatted`). An alt-screen app is re-entered
    /// first, since `state_formatted` does not carry that mode; the primary
    /// screen underneath it is not sent, so a client sees a blank screen if
    /// the app later exits.
    pub fn snapshot(&self) -> Vec<u8> {
        let mut out = Vec::new();
        if self.parser.screen().alternate_screen() {
            out.extend_from_slice(b"\x1b[?1049h");
        }
        out.extend(self.parser.screen().state_formatted());
        out
    }
```

### Task 3 — the backend (`src/shell/backend.rs` and `src/shell/host.rs`)

1. In the `ShellBackend` trait (`host.rs:20-27`), add a method with a default
   body:

   ```rust
       /// The current screen as `ShellScreen::snapshot` bytes, plus a receiver
       /// of every chunk after it. A backend with no screen returns empty
       /// bytes.
       fn subscribe_with_screen(&self) -> (Vec<u8>, broadcast::Receiver<Vec<u8>>) {
           (Vec::new(), self.subscribe())
       }
   ```

2. In `impl ShellBackend for PtyBackend`, override it. **The snapshot and the
   subscribe happen under one screen lock:**

   ```rust
       fn subscribe_with_screen(&self) -> (Vec<u8>, broadcast::Receiver<Vec<u8>>) {
           let screen = self.screen.lock().unwrap_or_log();
           (screen.snapshot(), self.out_tx.subscribe())
       }
   ```

3. In the pump (`backend.rs:117-139`), replace the separate `out_tx.send` and
   the trailing `screen…feed` with this block, placed where the send is now
   (after the secret-prompt arm, before the log write):

   ```rust
                        {
                            let mut screen = pump_this.screen.lock().unwrap_or_log();
                            screen.feed(&chunk);
                            let _ = pump_this.out_tx.send(chunk.clone());
                        }
   ```

   Then **delete** the old `pump_this.screen.lock().unwrap_or_log().feed(&chunk);`
   line at the end. Each chunk must be fed **exactly once**.

4. In `handle_line` (`host.rs:149`), add an arm after `Subscribe`:

   ```rust
        ShellRequest::SubscribeScreen => {
            let (bytes, rx) = backend.subscribe_with_screen();
            *chunks = Some(rx);
            ShellResponse::Screen { bytes }
        }
   ```

If the compiler names any other exhaustive `match` on `ShellRequest` or
`ShellResponse`, add the new variant there the way its neighbours are
handled. The dry run needed none.

### Task 4 — tests

Five tests, names pinned.

1. `src/shell/screen.rs` — **`snapshot_reproduces_the_screen`**:
   - feed `b"user@host:~$ ls\r\n\x1b[1;34mdir\x1b[0m  file.txt\r\nuser@host:~$ "`
     into `ShellScreen::new(24, 80, 100)`;
   - process `snapshot()` into a fresh `vt100::Parser::new(24, 80, 100)`;
   - assert its `screen().contents_formatted()` and `screen().cursor_position()`
     equal the source's. The test module can read the private `parser` field.
2. `src/shell/screen.rs` — **`snapshot_keeps_an_alt_screen_app_in_alt_mode`**:
   - feed `b"before\r\n\x1b[?1049h\x1b[H\x1b[2J\x1b[7m top bar \x1b[0m\x1b[5;10Hin vim"`;
   - process the snapshot into a fresh parser;
   - assert `p.screen().alternate_screen()` is true and `p.screen().contents()`
     equals the source's `contents()`.
3. `src/shell/proto.rs` — **`proto_screen_frames_round_trip`**:
   - `encode(ShellRequest::SubscribeScreen)` is exactly
     `"{\"type\":\"SubscribeScreen\"}\n"`;
   - `ShellResponse::Screen { bytes: vec![0x1b, b'[', b'H', 0xff] }` survives
     `encode` and then `serde_json::from_str` unchanged.
4. `src/shell/host.rs` — **`host_answers_subscribe_screen_with_the_screen_first`**.
   Write it beside `host_subscribe_streams_chunks_until_the_client_disconnects`
   (`host.rs:361`), with the same fixtures:

   ```rust
       let server = start_server().await;
       let client = UnixStream::connect(&server.path).await.unwrap();
       let (reader, mut writer) = client.into_split();
       let mut reader = BufReader::new(reader);
       write_request(&mut writer, &ShellRequest::SubscribeScreen).await;
   ```

   - the first response is `ShellResponse::Screen { bytes: vec![] }`
     (`FakeBackend` uses the trait's default);
   - after `server.backend.push(b"live".to_vec())`, the next response is
     `ShellResponse::Chunk { bytes: b"live".to_vec() }`.
5. `src/shell/backend.rs` — **`subscribe_with_screen_then_live_chunks_rebuild_the_screen`**.
   This is a `#[test]` in the style of `backend_resize_replaces_the_screen_and_does_not_reflow`:
   `let backend = spawn_backend(&dir);` and a `tokio::runtime::Runtime` for
   the async calls. Steps:
   1. Send `backend.input(b"printf 'marker-one\\nmarker-two\\n'\n")`. Poll
      (sleep 20 ms, up to 10 s) until `backend.screen_contents()` contains
      `"marker-two\n"`.
   2. `let (snap, mut rx) = backend.subscribe_with_screen();`. Process `snap`
      into a fresh `vt100::Parser::new(24, 80, 0)`.
   3. Send `backend.input(b"printf 'marker-three\\n'\n")`.
   4. Loop, up to 10 s: drain `rx.try_recv()` into the parser. Break when
      `backend.screen_contents()` contains `"marker-three\n"` **and**
      `p.screen().contents() == backend.screen_contents()`. On timeout,
      panic, printing both screens.
   5. End by sending `backend.signal(ShellSignal::Term)`, ignoring the result.
      The shell must not leak.

**Test count:** five new, so **2110 passed / 14 ignored**.

### Task 5 — capture the end-to-end evidence

Run the block in § End-to-end verification verbatim. Then create a new Update
Log entry titled exactly `### Update — <date> (end-to-end verification)`.
**Its first fenced block is the contents of `/tmp/e2e-m23-03b.txt`**: the
file's lines, copied by a tool. **Never paste the script itself**, and never
retype the lines (the real lines end `lib1=ok ` **with a trailing space**).
Then run the paste check once, put the line it prints into the entry **on its
own line**, and record the three mutations after it.

## Acceptance criteria

Parenthesised values are what each line produces against the current tree.
Every non-preservation line fails today, and each was run at drafting.

- [ ] `cargo test --lib` → **2110 passed / 0 failed / 14 ignored** on three
      consecutive runs (current: 2105/0/14). Both counts are exact.
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → other targets unmoved: bug_tracker 6, doc_truth 14,
      integration 32, isolation 11 (*preservation*).
- [ ] The five new tests pass:
      ``cargo test --lib -- snapshot_reproduces_the_screen snapshot_keeps_an_alt_screen_app_in_alt_mode proto_screen_frames_round_trip host_answers_subscribe_screen_with_the_screen_first subscribe_with_screen_then_live_chunks_rebuild_the_screen``
      → **5 passed** (current: `0 passed; … filtered out`).
- [ ] Each chunk is fed to the screen exactly once:
      ``grep -c 'feed(&chunk)' src/shell/backend.rs`` → **1** (current: 1,
      *preservation* — a second feed would corrupt the screen).
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

- **Mutation A:** in `ShellScreen::snapshot`, delete the
  `out.extend_from_slice(b"\x1b[?1049h");` line. Run test 2. Expected **red**.
- **Mutation B:** make `PtyBackend::subscribe_with_screen` return
  `(Vec::new(), self.out_tx.subscribe())`. Run test 5. Expected **red** (the
  rebuilt screen lacks `marker-one`).
- **Mutation C:** make the `SubscribeScreen` arm answer `ShellResponse::Ok`.
  Run test 4. Expected **red**.

After each restore, confirm `git diff --stat` matches its pre-mutation state.

## Test plan

Five tests:
- the snapshot round-trips a coloured screen and its cursor;
- an alt-screen app stays in alt mode;
- the new frames round-trip;
- the host sends `Screen` before any `Chunk`;
- a real bash shell's snapshot plus its live chunks rebuild the host's screen
  exactly.

## End-to-end verification

```sh
cd /home/matt/src/daemoneye
{
  for i in 1 2 3; do
    out=$(cargo test --lib 2>&1)
    printf 'lib%d=%s %s\n' "$i" "$(printf '%s\n' "$out" | grep -E '^test result' | tail -1 | grep -oE 'ok|FAILED')" "$(printf '%s\n' "$out" | grep -E '^test .* FAILED$' | sed -E 's/^test (.*) \.\.\. FAILED$/\1/' | tr '\n' ' ')"
  done
  printf 'lib_counts=%s\n' "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1p-\2f-\3i/')"
  printf 'new_tests=%s\n' "$(cargo test --lib -- snapshot_reproduces_the_screen snapshot_keeps_an_alt_screen_app_in_alt_mode proto_screen_frames_round_trip host_answers_subscribe_screen_with_the_screen_first subscribe_with_screen_then_live_chunks_rebuild_the_screen 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
  printf 'isolation=%s\n' "$(cargo test --test isolation 2>&1 | grep -E '^test result' | tail -1 | grep -oE '[0-9]+ passed; [0-9]+ failed')"
  printf 'fed_once=%s\n' "$(grep -c 'feed(&chunk)' src/shell/backend.rs)"
} > /tmp/e2e-m23-03b.txt
cat /tmp/e2e-m23-03b.txt
```

Expected:
- `lib1..3=ok` with **nothing after `ok`**;
- `lib_counts=2110p-0f-14i`;
- `new_tests=5p-0f`;
- `isolation=11 passed; 0 failed`;
- `fed_once=1`.

**Re-run budget: three** (WORKFLOW.md § End-to-end verification). The known
pre-existing flake is `shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker`.
If it fails, name it and re-run the whole block. After three re-runs, stop,
paste the last capture as it is, and name every failing test. Any other
failing test is a finding: report it by name.

**Run this block with nothing else running `cargo test`.** A concurrent run
of the PTY tests makes them fail spuriously; that was measured at the last
review. Then make the entry's first fenced block the **contents of**
`/tmp/e2e-m23-03b.txt` (copied, not retyped; **never the script above**), and
run:

```sh
D=docs/dev/milestones/M23-client-decoupling/phase-03b-shell-screen-snapshot.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m23-03b.txt
diff /tmp/pasted-m23-03b.txt /tmp/e2e-m23-03b.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

If it prints `PASTE MISMATCH`, run
`diff /tmp/pasted-m23-03b.txt /tmp/e2e-m23-03b.txt` once and fix what it
shows. **Run the check at most three times.** Put the line it prints into the
entry on its own line.

## Authorizations

You may edit these files, and nothing else:

- `src/shell/proto.rs`: Task 1 and test 3.
- `src/shell/screen.rs`: Task 2 and tests 1–2.
- `src/shell/host.rs`: Task 3.1 and 3.4, and test 4.
- `src/shell/backend.rs`: Task 3.2 and 3.3, and test 5.
- Any file the compiler names for a non-exhaustive match on the new variants:
  add the arms only.
- This phase doc's Update Log.

## Out of scope

- **The daemon relay** (`Request::ShellTail`, fallback to `Subscribe` on an
  old host's `malformed frame` error, client-disconnect handling): row 03c.
- **Lagged subscribers.** The host's connection loop drops `Lagged` chunks
  silently (`host.rs`, `RecvError::Lagged(_) => continue`), so a slow viewer
  desyncs. 03c decides whether to re-snapshot on lag.
- **The primary screen under an alt-screen app** (§ Current state 4).
- **Any client code.**

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-01 22:12 (started)

Executing phase 03b (shell-screen-snapshot). Executor: Codex (gpt-5-codex via rexyMCP). Flipping status `todo` → `in-progress`; beginning Task 1 (protocol), then Task 2 (snapshot), Task 3 (backend + host arm), Task 4 (five pinned tests).

### Update — 2026-10-01 22:12 (end-to-end verification)

```text
lib1=ok 
lib2=ok 
lib3=ok 
lib_counts=2110p-0f-14i
new_tests=5p-0f
isolation=11 passed; 0 failed
fed_once=1
```

PASTE MATCH

**total_re_runs=0**

Mutations (each applied with the `patch` tool, run once, restored; `git diff
--stat` matched its pre-mutation state after each restore):

- **Mutation A** — deleted `out.extend_from_slice(b"\x1b[?1049h");` from
  `ShellScreen::snapshot`, ran test 2
  (`snapshot_keeps_an_alt_screen_app_in_alt_mode`) → **red**
  (`FAILED. 0 passed; 1 failed`). Restored, test 2 → 1 passed.
- **Mutation B** — made `PtyBackend::subscribe_with_screen` return
  `(Vec::new(), self.out_tx.subscribe())`, ran test 5
  (`subscribe_with_screen_then_live_chunks_rebuild_the_screen`) → **red**
  (`FAILED. 0 passed; 1 failed`). Restored, test 5 → 1 passed.
- **Mutation C** — made the `SubscribeScreen` arm answer `ShellResponse::Ok`,
  ran test 4 (`host_answers_subscribe_screen_with_the_screen_first`) → **red**
  (`FAILED. 0 passed; 1 failed`). Restored, test 4 → 1 passed.

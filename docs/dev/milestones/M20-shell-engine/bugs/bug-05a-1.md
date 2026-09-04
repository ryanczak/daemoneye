# Bug 1 on phase-05a: a subscribed connection loses request frames to a cancelled read

**Severity:** major
**Status:** resolved (round 2, 2026-09-03, commit `396482c`)
**Filed:** 2026-09-03

## What's wrong

Once a connection has subscribed, an incoming request frame is **silently
discarded** if an output chunk arrives while that frame is only partly read.
The client gets a `malformed frame` error for a request it sent correctly.

Measured at review through the crate's public API — subscribe, then send a
well-formed `Status` frame in two writes with a chunk pushed between them:

```
subscribe ack : {"type":"Ok"}
request split : "{\"type\":\"" then "Status\"}\n"
(chunk pushed while the request frame is half-read)
  reply 1: {"type":"Chunk","bytes":[79,85,84,80,85,84,45,67,72,85,78,75]}
  reply 2: {"type":"Error","message":"malformed frame: expected value at line 1 column 1"}

VERDICT: request answered correctly = false
         request corrupted (malformed) = true
```

The chunk is delivered fine. The `Status` request is not: its first half is
lost, the second half arrives alone, and it fails to parse.

## What should happen

Streaming output and accepting requests on the same connection is the whole
point of `Subscribe` — the 2.0 design's attached mode forwards keystrokes while
rendering live output (`docs/design/daemoneye-2.0.md` § 2.6). A request must be
answered correctly no matter how many chunks arrive while it is in flight, and
regardless of how the client's write is split across reads.

Phase-05a § Spec, Task 3 states it plainly: `Status` "returns whatever the
backend gives", and a malformed-frame error is reserved for a client that
"sends garbage". A well-formed frame must never produce one.

## Root cause

`src/shell/host.rs`, in `handle_connection`:

```rust
    loop {
        line.clear();
        let read_fut = reader.read_until(b'\n', &mut line);
        let n = match chunks.as_mut() {
            None => read_fut.await,
            Some(rx) => tokio::select! {
                res = read_fut => res,
                res = rx.recv() => {
                    match res {
                        Ok(bytes) => {
                            write_frame(&mut writer, &ShellResponse::Chunk { bytes }).await?;
                            continue;
                        }
```

`tokio::io::AsyncBufReadExt::read_until` is **not cancellation safe**. Tokio's
own documentation says that if it is used as a branch in `tokio::select!` and
another branch completes first, data may have been partially read — and that
partial data has already been appended to `line`.

The chunk branch then `continue`s, the loop head runs `line.clear()`, and those
bytes are gone. The rest of the frame arrives on the next read and is parsed on
its own, which is why the error message points at column 1.

Both halves are needed for the bug: the cancellation-unsafe read puts partial
data in the buffer, and the unconditional `clear()` throws it away.

## Definition of done

- [ ] A named test `host_answers_a_request_split_around_a_chunk` exists and
      passes: on a subscribed connection, send a well-formed request in two
      writes with a chunk pushed between them, and assert the client receives
      the chunk **and** the correct answer to the request — not an `Error`.
      Confirmed failing now — no such test exists
      (`cargo test --lib host_answers_a_request_split` reports `0 passed`), and
      the behaviour it pins is currently wrong.
- [ ] `host_subscribe_streams_chunks_until_the_client_disconnects` and the
      other six `host_*` tests keep passing — the streaming path must not
      regress while the read path is fixed.
- [ ] `cargo test --lib shell::host::` reports **8 or more** passing with
      `0 failed` (7 today).
- [ ] `cargo test --lib shell::proto::` still reports `5 passed`, and
      `shell::pty::` / `shell::log::` / `shell::screen::` still report
      `13` / `12` / `11`.
- [ ] All four gates green.

**Constraint on the solution, not a prescription.** Whatever shape you choose,
a partially-read request frame must survive a chunk being written to the same
connection. Note that simply moving `line.clear()` is **not** sufficient on its
own: the read is cancelled at an arbitrary point, so correctness has to come
from either a cancellation-safe read or from not racing the read against the
chunk stream at all. Separating the connection into a reader half and a writer
half that share the write side is one shape that satisfies this; there are
others. Do not fix it by refusing to accept requests while subscribed — that
removes the capability the attached mode needs.

## Resolution — 2026-09-03 (round 2, commit `396482c`)

The read loop no longer clears its buffer wholesale. It drains complete
newline-delimited frames and keeps any trailing partial for the next read, so a
frame that was half-read when a chunk won the race survives.

Verified independently at review on the **original** failing case — a small
`Status` frame split around a chunk, which is not what the new test uses — ten
trials, ten correct answers, where round 1 returned `malformed frame`.

**The guard is probabilistic, and that is recorded rather than hidden.**
Restoring the old wholesale clear and running the new test twelve times, it
failed 8 of 12, matching the executor's self-reported 9 of 15. A deterministic
version would need a real-clock sleep, which this project deliberately removed
from its suite, or a test seam in the backend. Carried to phase-05b, which
builds the real backend and is the natural home for such a seam.

**Also cleared at review, not a defect:** the drain loop slices `&line[..end]`
rather than `&line[offset..end]`, which would concatenate frames if the buffer
held two. Pipelined requests were tested and dispatch correctly, because
`read_until` stops at the first newline so two complete frames never coexist in
the buffer. Fragile under an unstated invariant; worth tightening when the file
is next touched.

# Phase 10 — shell-host-framing

**Milestone:** M39 — Critical fixes
**Status:** in-progress
**Drafted:** 2026-11-24
**Depends on:** none
**Estimated diff:** ~120 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

This phase fixes two defects in the shell host, `src/shell/host.rs`. The
architect re-checked both on 2026-11-24.

- **S4b: a pipelined frame is parsed wrongly.** `handle_connection` loops
  over every complete frame in `line`, but it passes `&line[..end]` to
  `handle_line`. It should pass `&line[offset..end]`. When one read
  brings in two frames, the second call parses both together.
  `serde_json` rejects the trailing data, so the client gets
  `Error { "malformed frame: …" }` and the second request never runs.
  Today's clients write one frame and wait, so nothing triggers it yet.
  Any client that pipelines will.
- **Q12: `host_sends_lagged_when_subscriber_falls_behind` is flaky.** The
  architect ran it 300 times, 24 at a time, and it failed **5** times.
  - It runs on a `multi_thread` runtime, so the host task can drain the
    broadcast queue while the test pushes its burst. The overflow the
    test depends on may then never happen.
  - It reads exactly three frames and expects the `Lagged` among them.
  - Its comment says the channel has capacity 1, but `FakeBackend` uses
    16.

## Statement of work

1. **S4b:** each frame in the loop is parsed from its own offset. A
   client that writes two complete requests in one `write_all` gets two
   correct responses, in order.
2. **Q12:** make the test deterministic.
   - The overflow must be **guaranteed**: no task can drain the queue
     during the burst. One way is a current-thread runtime with a burst
     that never awaits. Another is a burst larger than the capacity in
     use.
   - The read loop must not depend on an exact frame count. Read until a
     `Lagged` and then a later `Chunk` arrive, with a bound on frames read
     and an overall timeout.
   - Make the comment match the capacity actually used.

**Constraints:**

- The test still proves the same three things: a `Lagged` frame arrives,
  it names a non-zero count, and streaming continues with a normal chunk
  after it.
- No other `host.rs` behaviour changes.
- No `unsafe`, `.unwrap()`, `.expect()` or `panic!` in production code.

**Test level.** Use the `host.rs` test module: `FakeBackend`, `bind` and
`serve`, `UnixStream::connect`, and the `write_request` / `read_response`
helpers. `host_sends_lagged_when_subscriber_falls_behind` is the
analogue. No new harness is needed.

**Closed file list:** `src/shell/host.rs`. You need no other file. Read
at most **3** files before your first edit.

**Budget.** You have **1200 turns**, so use them efficiently. Your
**first tool call** after reading writes the first red test.

## First red test

`host_parses_each_pipelined_frame_from_its_own_offset`. Send two
complete requests in a single `write_all`, for example two `Status`
frames, or an `Input` followed by a `Status`. Assert that both get their
correct response and that no `Error` frame arrives. It is red today: the
second response is `Error { "malformed frame: …" }`.

## Acceptance criteria

- [ ] **Pipelining:** `host_parses_each_pipelined_frame_from_its_own_offset`
      passes. Red run pasted.
- [ ] **No flake:** a stress loop of
      `host_sends_lagged_when_subscriber_falls_behind` passes **300 of 300**
      runs, 24 in parallel. Run the lib test binary directly with
      `--exact`. Paste the loop and its failure count, which must be 0.
      Today it fails about 5 times in 300.
- [ ] **Same proof:** the lagged test still asserts a `Lagged` with a
      non-zero count, then a later `Chunk`.
- [ ] **Mutation check,** with the output pasted: put `&line[..end]`
      back, run the pipelining test and see it fail, then restore it.
- [ ] **Gates:** all four pass. The lib suite reports **at least 3104**
      passed (3103 plus one).

## Spec

1. **Break down:** write the first red test and run it red. Then write a
   numbered task list into an Update Log entry, one line per task, each
   naming the acceptance item it closes. Done when that entry is written.
2. **Implement:** fix the slice, then make the lagged test deterministic.
   Done when `cargo build --tests` is green.
3. **Meet the acceptance criteria:** check off every item, including the
   stress loop and the mutation check. Done when every item is checked
   with its evidence named.
4. **Capture the evidence:** run the four gates. Paste each `test result:`
   line, the red run, the stress-loop count and the mutation check into a
   new Update Log entry, without the per-test `… ok` lines. Commit as one
   `fix:` commit.

## Authorizations

- You may touch `src/shell/host.rs` only.

## Out of scope

- The client side (`src/shell/client.rs` and `client_read.rs`).
- Making any client pipeline its requests.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-24 19:15 (started)

Phase 10 (shell-host-framing) started by executor. Plan: red test first
(`host_parses_each_pipelined_frame_from_its_own_offset`), then the slice fix and
the deterministic lagged test, then the stress loop, mutation check, and gates.


### Update — 2026-11-24 (escalation)

**Chosen lever:** resume.
**Rationale:** half the work is done and correct, and the only obstacle was
mechanical. The run hit `IdenticalToolCallRepetition` after 46 turns. It
was re-running `cargo test` one run at a time to build the stress loop. The
architect ran the rewritten lagged test (current-thread runtime, a 64-push
burst, up to 8 reads) 300 times, 24 in parallel, on the partial tree:
**0 failures**. What remains is the S4b fix, the pipelining test (never
written), the mutation check and the gates. The resume guidance gives the
exact stress command.

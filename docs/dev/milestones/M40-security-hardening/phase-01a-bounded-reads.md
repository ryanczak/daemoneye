# Phase 01a — bounded-reads

**Milestone:** M40 — Security hardening
**Status:** todo
**Drafted:** 2026-11-25
**Depends on:** none
**Estimated diff:** ~300 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

Audit row **S4a**, first half. The two sockets the daemon serves read a
whole line into memory before checking its size, and their accept loops
spawn a task for every connection with no limit. The architect
**reproduced both reads** on 2026-11-25 with scratch probes, since
reverted. Each probe wrote 64 MiB with no newline:

- **Daemon socket** (`handle_client`, `src/daemon/server/mod.rs`): all
  67108864 bytes were accepted and buffered. Only then did it answer
  `Request too large (67108864 bytes; limit 1048576 bytes)`. The 1 MiB
  check runs after `read_line` has read everything.
- **Shell host** (`handle_connection`, `src/shell/host.rs`): all
  67108864 bytes were accepted. `read_until` keeps growing `line` and
  never refuses.
- **Accept loops** (`run_daemon` in `src/daemon/startup.rs`, `serve` in
  `src/shell/host.rs`) accept and spawn without bound.

Phase 01b does the other half: the reads inside an open connection
(approval, credential and prompt replies, the presence, notices and tail
loops). This phase builds the two tools 01b will reuse.

## Statement of work

1. **New module `src/ipc_bounded.rs`**, a child of `src/ipc.rs`
   declared the way `ipc_types.rs` is, with its items re-exported from
   `crate::ipc`. It holds:
   - **`read_bounded_line(reader, buf: &mut Vec<u8>, cap: usize) ->
     std::io::Result<usize>`**, generic over `AsyncBufRead + Unpin`. It
     uses `fill_buf` / `consume` and appends up to and including the first
     `\n`.
     - It returns `Ok(n)`, the bytes appended. `Ok(0)` means EOF with
       nothing appended. A partial line at EOF is returned as it is, like
       `read_until`.
     - **The cap bounds `buf` as a whole**, including bytes left in it by
       an earlier call. A line whose newline would land beyond `cap` is an
       `Err` of kind `InvalidData` whose message contains `exceeds`. The
       error is returned as soon as `buf` would pass `cap`. The function
       never consumes more than one `fill_buf` slice past that point.
     - **Cancel safety matches `read_until`.** Every consumed byte is
       already in `buf`, so a future dropped mid-line loses nothing. The
       shell host depends on this (see the comment above its
       `read_until`).
   - **`accept_bounded(listener: &UnixListener, permits:
     &Arc<Semaphore>) -> std::io::Result<(UnixStream,
     OwnedSemaphorePermit)>`**. It acquires a permit **before** calling
     `accept`, so when every permit is held, nothing more is accepted and
     new connections wait in the kernel backlog. Both futures it awaits
     are cancel-safe, so dropping a waiting call leaks no permit.
   - The caps, as named constants: the request cap is **1 MiB** (today's
     `MAX_IPC_MESSAGE_BYTES`, moved here). The shell-host frame cap is
     **16 MiB**: an `Input` frame encodes its bytes as a JSON number
     array, about 4 bytes per input byte, so 16 MiB is about 4 MiB of
     input. Allow **128** daemon connections and **32** per shell host.
2. **Daemon request read.** `handle_client` reads its first line with
   `read_bounded_line` at the request cap.
   - Over the cap, it sends `Response::Error`. The message contains
     `too large` and names the limit. Then the connection closes.
   - Other read errors propagate as they do today.
   - Parsing and dispatch are unchanged.
3. **Shell-host frame read.** `handle_connection` reads frames with
   `read_bounded_line` at the frame cap, in the same `select!` as today.
   - Over the cap, it writes one `ShellResponse::Error` that names the
     limit, then ends the connection.
   - Other connections to the same host are unaffected.
4. **Accept loops.** Both loops call `accept_bounded`. The spawned task
   holds its permit until the connection's handler returns.
   - In `run_daemon`, the accept stays one branch of the existing
     `select!`, so SIGTERM and SIGINT still shut the daemon down while
     every permit is held.
   - `serve` keeps its signature.

**Constraints:**

- No `unsafe`, `.unwrap()`, `.expect()` or `panic!` in new production
  code.
- Valid requests and frames under the caps behave exactly as today. These
  existing tests keep passing unchanged:
  - `host_answers_a_request_split_around_a_chunk` (cancel safety);
  - `host_parses_each_pipelined_frame_from_its_own_offset`;
  - `host_sends_lagged_when_subscriber_falls_behind`.
- Every await in the new socket tests runs under a timeout. A writer stops
  at its first write error and reports the bytes the peer accepted before
  it. The tests must not wait for the writer to finish 64 MiB.

**Closed file list:**

| Source | Tests |
|---|---|
| `src/ipc_bounded.rs` (new) | its own `mod tests` |
| `src/ipc.rs` (module declaration and re-export only) | none |
| `src/daemon/server/mod.rs` | its own `mod tests` |
| `src/shell/host.rs` | its own `mod tests` |
| `src/daemon/startup.rs` | none |
| `CLAUDE.md` (one Key files row for the new module) | — |

**Edit order:** item 1 with its tests, then 2, 3 and 4. Read at most **3**
files before your first edit.

**Budget.** You have **1200 turns**, so use them efficiently. Your
**first tool call** after reading writes the first red test.

**Test seams.** `DaemonState::new_for_test(Config::default())` and
`tokio::net::UnixStream::pair()` drive `handle_client` without disk
access. The `host.rs` tests already have `FakeBackend`, `bind`, `serve`,
`write_request` and `read_response`.

## First red test

`daemon_refuses_an_oversized_request_within_budget`, in
`src/daemon/server/mod.rs` tests.
- Run `handle_client` on one end of a `UnixStream::pair()`.
- From the other end, write 64 MiB of `a` with no newline, in 64 KiB
  chunks, counting the bytes accepted until the first write error.
- Read the reply. It must be a `Response::Error` containing `too large`.
- **At most 4 MiB** were accepted.

It compiles today and is red: 67108864 bytes are accepted. Paste the red
run.

## Acceptance criteria

- [ ] **Daemon budget:** `daemon_refuses_an_oversized_request_within_budget`
      passes. Red run pasted.
- [ ] **Host budget:** `host_refuses_an_oversized_frame_within_budget`
      passes. Red run pasted (today 67108864 bytes are accepted).
      - It writes 64 MiB with no newline to a `serve`d host.
      - The writer had **at most 20 MiB** accepted.
      - The reply is a `ShellResponse::Error`.
      - A **new** connection to the same host then gets a `Status`
        answer.
- [ ] **Line reader:** these tests in `src/ipc_bounded.rs` pass. Each one
      calls `read_bounded_line`:
      - `bounded_line_reads_one_line_and_leaves_the_rest`: two lines
        arrive in one write. The first call returns only the first line,
        and the second call returns the second.
      - `bounded_line_accepts_a_line_of_exactly_the_cap`: a line whose
        length, newline included, equals `cap`.
      - `bounded_line_refuses_one_byte_over_the_cap`: the error kind is
        `InvalidData`.
      - `bounded_line_returns_a_partial_line_at_eof`: also, empty input
        returns `Ok(0)`.
      - `bounded_line_cap_counts_bytes_already_in_the_buffer`: `buf`
        already holds `cap - 2` bytes, and `abc\n` is refused.
      - `bounded_line_refusal_reads_within_budget`: the source counts the
        bytes it hands out and would yield 64 MiB of `a`. The call is
        refused after the source handed out **at most `cap + 64 KiB`**.
      - `bounded_line_keeps_bytes_across_a_cancelled_call`: on a
        `tokio::io::duplex`, write half a line. A call under a 50 ms
        timeout is dropped. Write the rest. The next call completes the
        whole line in `buf`.
- [ ] **Accept cap:** these tests in `src/ipc_bounded.rs` pass. Each one
      calls `accept_bounded` on a 1-permit semaphore:
      - `accept_bounded_waits_for_a_free_permit`: hold the first
        connection's permit, then connect a second client. The second
        `accept_bounded` has not returned after 200 ms. It returns once
        the first permit is dropped.
      - `accept_bounded_waiting_call_can_be_dropped`: drop a waiting call
        while the permit is held, then release the permit. A fresh call
        accepts, and `available_permits()` is back to 1 after its permit
        drops.
- [ ] **Both loops use it:** run once. It prints a count of at least 1
      for each file (each prints 0 today):
      `grep -c 'accept_bounded' src/daemon/startup.rs src/shell/host.rs`
- [ ] **Mutation check,** run once with the output pasted:
      1. make `read_bounded_line` skip its cap check;
      2. run
         `cargo test --lib bounded_line_refusal_reads_within_budget`
         once and see it fail;
      3. restore the check.
- [ ] **Gates:** all four pass, run once each. The lib suite reports **at
      least 3125** passed (3114 plus eleven). `tests/hygiene.rs` passes.

## Spec

1. **Break down:** write the first red test and run it red. Then write a
   numbered task list into an Update Log entry, one line per task, each
   naming the acceptance item it closes. Done when that entry is written.
2. **Implement:** items 1 to 4 in the edit order, writing each red test
   before its fix. Done when `cargo build --tests` is green.
3. **Meet the acceptance criteria:** check off every item. Run each
   command **once**. Done when every item is checked with its evidence
   named.
4. **Capture the evidence:** run the four gates, once each. Paste into a
   new Update Log entry:
   - each `test result:` line, without the per-test `… ok` lines;
   - the two red runs;
   - the grep;
   - the mutation check.

   Commit as one `fix:` commit.

## Authorizations

- You may touch the files in the closed file list, and no others.

## Out of scope

- Reads inside an open connection: `approval_reply`, the prompt and
  credential replies, `chat_presence`, `notices` and the shell tail relay.
  Phase 01b covers them.
- A timeout on the daemon's first request read.
- Keeping a refused connection open: after an oversized line, the stream
  is not resynchronised.
- The daemon-side shell client (`src/shell/client.rs`,
  `client_read.rs`) and the CLI's reads of daemon responses.

## Update Log

<!-- entries appended below this line -->

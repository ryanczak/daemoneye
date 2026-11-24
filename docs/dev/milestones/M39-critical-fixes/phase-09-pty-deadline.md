# Phase 09 — pty-deadline

**Milestone:** M39 — Critical fixes
**Status:** todo
**Drafted:** 2026-11-24
**Depends on:** none
**Estimated diff:** ~200 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

A command that never stops printing never times out. The audit calls
this C3; the architect re-verified it on the current tree on 2026-11-24.

`read_to_marker` (`src/shell/client_read.rs`) reads frames in slices of
at most `EXIT_POLL_INTERVAL`. The deadline, the exit-status poll, the
paused-time extension and the Ctrl-C interrupt all live in
`handle_read_timeout`, and that runs only when a read **times out**. A
command that prints at least once every 500 ms (`yes`, `tail -f`,
`ping`, a chatty build) never lets a read time out. So:

- `timeout_secs` is never enforced and no Ctrl-C is typed. The call
  blocks until the command ends on its own, which may be never.
- `buf` collects every byte the command prints, with no bound.

After this phase the deadline holds whatever the output rate, and the
buffer has a fixed upper size.

## Statement of work

1. **Deadline checks don't depend on a quiet stream.** When a slice ends,
   `read_to_marker` runs the same checks whether the read timed out or
   data kept arriving past the slice's end. Those checks are the
   exit-status poll, the paused extension, the deadline and the Ctrl-C,
   then the interrupt grace. A command that ignores the Ctrl-C and keeps
   printing ends with `CommandTimedOut { interrupted: None }` once the
   grace has passed, as a silent one does today.
2. **The output buffer is capped.** Add a named constant for the cap in
   `src/shell/client.rs`, next to `OFFER_WINDOW`, at no more than 16 MiB.
   Once `buf` would pass it, drop bytes from the **middle**:
   - Keep the head through the begin marker. `parse_outcome` needs the
     begin marker.
   - Keep the most recent tail.
   - Write a one-line note where the bytes were cut, of the form
     `[daemoneye: N bytes of output dropped here]`, as the `Lagged` arm
     does.
   - Keep the end-marker scan and `offered_at` correct after the cut.
     `offered_at` must never point past `buf.len()`.

**Constraints:**

- A command that finishes before its deadline behaves exactly as before,
  with the same output bytes and exit code. That includes output above
  the cap's size being cut only when it passes the cap.
- The paused extension and the prompt-wait extension still add their time
  back to the deadline.
- No `unsafe`, `.unwrap()`, `.expect()` or `panic!` in production code.

**Test level.** Every test drives `run_command` (or
`run_command_with_prompts`) against `spawn_scripted()` in
`src/shell/client_tests.rs`. `timeout_types_ctrl_c_and_returns_the_interrupted_outcome`
is the analogue. It uses `host.backend.tx` to push chunks,
`await_input` / `nonce_of` to get the nonce, and `test_begin_marker` /
`test_end_marker` to build the markers. Write each new test the same way.
No new harness is needed.

**Closed file list:**
- `src/shell/client_read.rs`
- `src/shell/client.rs` (the constant only)
- `src/shell/client_tests.rs`

You need no other file. Read at most **5** files before your first edit,
and only from this list.

**Budget.** You have **1200 turns**, so use them efficiently. Your
**first tool call** after reading writes the first red test.

## First red test

`continuous_output_still_times_out`:
1. Start `run_command` with a 1 s timeout.
2. Send the begin marker, then send a chunk every 100 ms from a spawned
   task, and never send the end marker.
3. Wrap the call in a 15 s `tokio::time::timeout`, so a hang fails the
   test rather than the suite.
4. Assert the call returns `CommandTimedOut` with `interrupted: None` well
   inside 15 s.
5. Assert a `0x03` write appears in `host.backend.writes()`.

It is red today: the 15 s wrapper fires.

## Acceptance criteria

- [ ] **Deadline:** `continuous_output_still_times_out` passes. Red run
      pasted.
- [ ] **Interrupted:** `continuous_output_answered_ctrl_c_returns_the_outcome`
      passes. Output keeps flowing past the deadline. After the `0x03`
      write, the script sends the end marker with code 130. The error is
      `CommandTimedOut` with `interrupted` set, and that outcome's exit
      code is 130. Red today.
- [ ] **Cap:** `output_past_the_cap_keeps_head_tail_and_a_note` passes. A
      command prints more than the cap, then its end marker with code 0.
      The outcome's exit code is 0. Its output is no longer than the cap,
      contains the `bytes of output dropped here` note, and ends with the
      last bytes sent. Red today.
- [ ] **Preservation:** every existing test in `src/shell/client_tests.rs`
      passes unchanged.
- [ ] **Mutation check,** with the output pasted: disable the slice-end
      check on the data path, run `continuous_output_still_times_out`, see
      it fail, then restore it.
- [ ] **Gates:** all four pass. The lib suite reports **at least 3101**
      passed (3098 plus three).

## Spec

1. **Break down:** write the first red test and run it red. Then write a
   numbered task list into an Update Log entry, one line per task, each
   naming the acceptance item it closes. Done when that entry is written.
2. **Implement:** work through the list, each test red before its code.
   Done when `cargo build --tests` is green.
3. **Meet the acceptance criteria:** check off every item, including the
   mutation check. Done when every item is checked with its evidence named.
4. **Capture the evidence:** run the four gates. Paste each `test result:`
   line, your red runs and the mutation check into a new Update Log entry,
   without the per-test `… ok` lines. Commit as one `fix:` commit.

## Authorizations

- You may touch the three files in the closed list.

## Out of scope

- How much of the output the AI is shown. Output is already truncated
  later, in the executor.
- `spawn_command` callers and the background watcher. They share
  `read_to_marker`, so they get the fix without changes.
- User-facing docs. The architect writes them at review.

## Update Log

<!-- entries appended below this line -->

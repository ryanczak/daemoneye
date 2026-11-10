# Bug 4 on phase-21a: any keystroke into a running program drops its working record

**Severity:** major
**Status:** open
**Filed:** 2026-11-10

## What's wrong

To fix bug-21a-2, the executor used an input-line epoch. That was the right
call. As bug-21a-2 was written, the rule was racy: a report printed by the
shell's builtin `printf` gets stamped with whichever process group wins the
race against the next fork. Any rule that has to be decided at the moment
of that fork has the same problem.

The epoch, though, advances on **every** `PtyBackend::input` call
(`src/shell/backend.rs`, the `fetch_add` at the top of `input`). That means
a keystroke, a pasted answer or a `send_input` into a program that is
**still running** ends its `working` or `blocked` record. Here is the case
this feature exists for:

1. Claude Code runs in a daemon shell and reports `working`.
2. The user types into its prompt (`/shell`), or the AI sends it keys with
   `send_input`.
3. The record is gone while Claude Code is still working, and it stays gone
   until the program reports again.

The spec says the opposite: "No heartbeat; a terminal MUST NOT require
re-sending to keep a record". It drops `working` and `blocked` only on
process exit or a new shell prompt. Input to the program is neither.

A second, smaller problem: the comment above `screen.expire_stamps` in
`PtyBackend::status` still says each report "stamped the group" and that
the read "compares those groups". The code compares input epochs.

## What should happen

A new epoch begins only when **a command line is entered at the shell's own
prompt**. That means input carrying a line end (`\r` or `\n`) while no job
holds the terminal. This is the daemon's nearest stand-in for the spec's
"new shell prompt" signal. Input that goes to a running program never
advances it. The rest of the rule stays as it is:

- A read with no job in front drops `working` and `blocked` records.
- A read with a job in front drops records from an earlier epoch.

The comment says what the code does.

## Root cause

"An input line was entered" was implemented as "an `input()` call
happened", and nothing checks whether the shell or a program is reading
that input.

## Constraints

- `input()` already holds the `shell` lock to write. Deciding "a job is in
  front" there takes no other lock, so the phase's lock order is unchanged.
- `backend_status_carries_a_working_report_while_its_program_runs`,
  `backend_status_keeps_a_done_report_after_its_program_exits` and
  `backend_status_drops_a_working_report_once_another_program_holds_the_terminal`
  keep passing.

## Definition of done

- [ ] `backend_status_keeps_a_working_report_while_input_goes_to_its_program`
      passes. Use real bash: one command line prints a `working` report,
      then reads a line from stdin, then sleeps. While it waits, the test
      sends a line with `input()`. A `status` during the sleep still
      carries the `working` record. **This test doesn't exist yet. Written
      against the current tree, it fails.**
- [ ] The three tests named under Constraints still pass.

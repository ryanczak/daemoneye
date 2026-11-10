# Bug 5 on phase-21a: record lifetime now depends on the last character on the screen

**Severity:** major
**Status:** open
**Filed:** 2026-11-10

## What's wrong

The bug-21a-4 fix advances the input-line epoch whenever the screen's last
non-empty line ends in `$`, `%`, `#` or `>`. That is `looks_like_prompt` in
`src/shell/backend.rs`, checked by the pump after every chunk. It also
removed the rule that a read with no job in front drops `working` and
`blocked` records. Record lifetime now hangs on screen text, and it fails
both ways:

- **Dropped too early.** A program that reports `working` and then prints a
  progress line ending in `%` (`50%`) advances the epoch. Its own record
  goes on the next read while it is still running. The same happens with a
  line ending in `>`, `#` or `$`: an HTML tail, a REPL or a TUI input box.
- **Never dropped.** At a prompt that ends in none of the four characters
  (`PS1='❯ '`, which is starship's and fish's default), the epoch never
  advances, and nothing else expires a record. A `working` report from a
  command that has exited stays for good. The spec says "MUST drop
  `working` and `blocked` records on a new shell prompt".

It also renders the whole screen to text (`contents()`) under the screen
lock for every output chunk.

## Why the executor got here (the architect's error)

bug-21a-4's Definition of Done told the executor to test with
`printf …; read -r line; sleep 30`. `read` is a bash **builtin**: it runs in
the shell's own process group, so "input while no job holds the terminal"
can't tell it from input at the prompt. The test the bug asked for
contradicted the rule the bug asked for. The executor's adaptation
satisfied the test and broke the rule. This bug corrects the test shape.

## What should happen

This is the rule bug-21a-4 described, now with a test that can hold it:

1. **The epoch advances** in `PtyBackend::input` when the bytes carry a
   line end (`\r` or `\n`) **and** no job holds the terminal. "No job" uses
   the same check `status()` uses: no foreground job and no live resumed
   group.
2. **A read with no job in front** drops every `working` and `blocked`
   record.
3. **A read with a job in front** drops the `working` and `blocked` records
   from an older epoch.
4. **No screen content takes part in record lifetime.** Remove
   `looks_like_prompt` and the pump's per-chunk check.

Known and accepted: input to a bash builtin that reads the terminal
(`read`) can't be told from input at the prompt, so it starts a new epoch.
Programs that report OSC 7501 are external processes, so this is not the
case the feature serves. Say so in a doc comment.

## Definition of done

- [ ] `backend_status_keeps_a_working_report_while_input_goes_to_its_program`
      uses an **external** reader in place of the `read` builtin, for
      example `printf …; head -n 1 >/dev/null; sleep 30`. It still sends
      keystrokes, a paste and a full line, and still passes.
- [ ] `backend_status_keeps_a_working_report_while_its_program_prints_a_percent`
      passes: real bash runs `printf <working report>; echo 'progress 50%'; sleep 30`,
      and a `status` during the sleep still carries the record. **This test
      doesn't exist yet. Written against the current tree, it fails.**
- [ ] `backend_status_drops_a_working_report_at_a_prompt_without_a_glyph`
      passes: with `PS1='❯ '`, a command line prints a `working` report and
      exits, and a `status` once the shell is idle again has no record.
      **This test doesn't exist yet. Written against the current tree, it
      fails.**
- [ ] `grep -c looks_like_prompt src/shell/backend.rs` prints 0.
- [ ] The carry, done-survives and drops tests from the earlier bugs still
      pass.

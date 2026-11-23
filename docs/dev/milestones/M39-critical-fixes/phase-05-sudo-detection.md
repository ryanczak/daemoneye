# Phase 05 — sudo-detection

**Milestone:** M39 — Critical fixes
**Status:** todo
**Drafted:** 2026-11-23
**Depends on:** none
**Estimated diff:** ~300 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

`command_has_sudo` (`src/daemon/utils/sudo.rs`) decides whether a command
elevates. Its callers act on the answer:
- `GhostPolicy::is_safe` lets a non-elevating ghost command run freely.
- The chat client never auto-approves an elevating command.
- The foreground executor picks its credential-prompt arm.
- `sbx::script_invocation` declines to treat it as a plain script.

The detector is easy to dodge. Every command below returns `false` on the
current tree; the architect re-verified this at drafting on 2026-11-23,
18 of 18:

```
s\udo id      s'u'do id      "su""do" id      sh -c 's\udo id'
</dev/null sudo id      2>/dev/null sudo id      >out sudo id
/usr/bin/sud? id      /usr/bin/s*do id      $S id      ${X}udo id
su -c id      doas id      pkexec id      run0 id      sudoedit /etc/x
strace sudo id      flock /tmp/l sudo id
```

After this phase, every one of them returns `true`, and every existing
must-not-match case still returns `false`.

## Statement of work

**What "fixed" means:**

1. **Spelling.** A shell word is read the way the shell reads it: across
   quotes and backslash escapes, with the quote characters and the
   backslashes removed before the word is compared. `s\udo`, `s'u'do`
   and `"su""do"` are all the word `sudo`. Quoted text is **still scanned
   as a command** as well, which is the existing `sh -c '…'` behaviour.
   Keep it.
2. **Leading redirections.** A redirection and its target never change
   command position. This covers `<`, `>`, `>>`, `<<<`, `&>`, and an fd
   number written straight before the operator (`2>`, `2>>`, `1<`). So
   `</dev/null sudo id` and `2>/dev/null sudo id` see `sudo` as the
   command word.
3. **Other elevators.** In command position, the basenames `sudo`,
   `sudoedit`, `su`, `doas`, `pkexec` and `run0` all count as elevation.
   That includes an absolute path or a leading-backslash spelling
   (`/bin/su`, `\doas`).
4. **More wrappers.** Commands that run their remaining words as a command
   join `WRAPPER_COMMANDS`. At least `strace`, `ltrace`, `flock`, `chroot`,
   `unshare`, `nsenter`, `setpriv`, `chrt`, `taskset` and `systemd-run`.
   The set fails closed: every later word of the simple command is a
   command position, as today.
5. **Fail closed.** A command-position word the detector cannot resolve
   counts as elevation. That means a word that, after dequoting, still
   contains an unquoted parameter expansion (`$S`, `${X}udo`, `"$CMD"`) or
   an unquoted glob character (`*`, `?`, `[`). The same characters in
   **argument** position change nothing.

**Must stay `false` (preservation).** Every command in the existing
`sudo_detect_argument_is_not_sudo`, `command_has_sudo_false_positive_guard`
and `command_has_sudo_no_sudo` tests. Also each of these:

```
ls *.txt      rm -f /tmp/*.log      ls [ab].txt      echo $HOME
FOO=$HOME ls      echo su      echo doas      cat /usr/bin/pkexec
sudoku      ls /usr/bin/su*      git status
```

`FOO=$HOME ls` stays false because a leading assignment is not the
command word. The word after it, `ls`, is.

**Constraints:**

- No new dependencies and no shell-parser crate. This is a word scanner,
  not a shell.
- Keep the function's name and signature; six callers use it. Update its
  doc comment to say it detects elevation, fail-closed.
- `tests/hygiene.rs` caps a function at 150 lines, and `command_has_sudo`
  is already about 95. Give the word reading and dequoting their own
  function rather than growing the loop.
- No `unsafe`, `.unwrap()`, `.expect()` or `panic!` in production code.

**Gotcha.** The executor's bash classifier may refuse a command line that
contains the word `sudo`. If it refuses `cargo test --lib sudo_detect`,
filter by a part of the name without that word instead:
`cargo test --lib -- detect_`.

**Scoping hints.** Start here, and widen only if the work cannot be done
without it. If you do widen, log why first.

- `src/daemon/utils/sudo.rs`: `command_has_sudo`, `sudo_command_word`,
  `WRAPPER_COMMANDS`, `COMMAND_KEYWORDS`, `is_assignment`, and the test
  module's `sudo_detect_*` tests.

**Do not read:** `tests/doc_truth.rs`, `tests/hygiene.rs`, `README.md`,
`CLAUDE.md`, `docs/dev/WORKFLOW.md`, or the callers. Their behaviour comes
from the answer, so they need no change.

**Budget.** You have **1200 turns**, so use them efficiently. Make your
first edit within your first **8** tool calls.

## First red test

**Write this before any production change.** Add
`sudo_detect_quote_and_backslash_spelling` to `src/daemon/utils/sudo.rs`'s
test module. It asserts `true` for `s\udo id`, `s'u'do id`, `"su""do" id`
and `sh -c 's\udo id'`. Run it red.

## Acceptance criteria

Each item is a named test that fails on the current tree, unless marked
preservation. Paste each red run into the Update Log before its fix.

- [ ] **Spelling:** `sudo_detect_quote_and_backslash_spelling` passes.
- [ ] **Redirections:** `sudo_detect_after_leading_redirections` passes for
      `</dev/null sudo id`, `2>/dev/null sudo id`, `>out sudo id`,
      `>>log sudo id` and `&>/dev/null sudo id`.
- [ ] **Elevators:** `sudo_detect_other_elevators` passes for `su -c id`,
      `doas id`, `pkexec id`, `run0 id`, `sudoedit /etc/x`, `/bin/su` and
      `\doas id`.
- [ ] **Wrappers:** `sudo_detect_more_wrappers` passes for `strace sudo id`,
      `flock /tmp/l sudo id` and one more command from each wrapper you add.
- [ ] **Fail closed:** `sudo_detect_unresolvable_command_word_fails_closed`
      passes for `/usr/bin/sud? id`, `/usr/bin/s*do id`, `$S id`,
      `${X}udo id` and `"$CMD" id`.
- [ ] **Preservation:** `sudo_detect_globs_and_expansions_in_arguments_stay_clear`
      passes for the must-stay-false list above. The three existing negative
      tests pass **unchanged**, so their bodies stay out of the diff.
- [ ] **Preservation:** every existing `sudo_detect_*` / `command_has_sudo_*`
      test passes unchanged.
- [ ] **Mutation check,** with the output pasted: stop removing quotes
      during word reading, run the spelling test and see it fail, then
      restore it.
- [ ] **Gates:** all four pass, and the lib suite reports **at least 3092**
      passed (3086 plus at least six new tests). If a test in another
      module changes its expectation because a command it uses now counts
      as elevating, name it and give the reason in the Update Log.

## Spec

1. **Break down** — write the first red test and run it red. Then write a
   numbered task list into an Update Log entry, one line per task, each
   naming the acceptance item it closes. Ends when that entry is written.
2. **Implement** — work through the list, each test red before its code.
   Ends at a green `cargo build --tests`.
3. **Meet the acceptance criteria** — check off every item, including the
   mutation check and the preservation tests. Ends when every item is
   checked with its evidence named.
4. **Capture the evidence** — run the four gates. Paste each `test result:`
   line, your red runs and the mutation check into a new Update Log entry,
   but no per-test `… ok` lines. Commit as one `fix:` commit.

## Authorizations

- May touch `src/daemon/utils/sudo.rs` and its test module. Nothing else.
- If a test elsewhere must change its expectation (see Gates), record the
  file, the test and the reason in the Update Log before editing it.

## Out of scope

- `GhostPolicy::is_safe`'s structural check and `wrap_remote`. Phase 06
  covers `wrap_remote`.
- Prompting for `su`/`doas`/`pkexec` passwords. Routing them to the
  credential arm is enough; the arm's sentinel stays sudo-only.
- A real shell parser: here-docs, aliases, functions, `eval` bodies built at
  runtime. `eval` should be treated as a wrapper only if that is a one-line
  addition.
- User-facing docs. The architect writes them at review.

## Update Log

<!-- entries appended below this line -->

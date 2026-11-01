# Phase 08 — heredoc-wrap

**Status:** todo
**Milestone:** M35 — Findings closeout
**Drafted:** 2026-10-31
**Depends on:** phase 07 (done)
**Tags:** language=rust, kind=fix, size=s

## Goal

GitHub issue #1, bug 1. `wrap_command` (`src/shell/pty.rs`) appends
`; printf …DE_END… $?` to the command's **last line**. When that line is a
heredoc terminator the shell sees `EOF; printf …`, which is not a
terminator: the heredoc never closes, the END marker is never printed, the
call can only end by timeout, and the shell sits at its `>` prompt with
every later command typed into the heredoc body. Seen live on pinky
(2026-10-31, shell `s10`): a `python3 - <<'EOF' … EOF` ping sweep hung the
shell, and the next three commands sent to `s10` timed out one after
another. The same appending breaks a command whose last line ends in a
`# comment`, which swallows the END printf.

After this phase the command runs inside a brace group whose closing brace
is on its own line, so the last line of the command is always followed by a
newline before any protocol text:

```
printf '\x1fDE_''BEG <nonce>\x1f\n'; { <cmd>
}; printf '\n\x1fDE_''END <nonce> %s\x1f\n' $?
```

`$?` after `}` is the group's status, which is the last command's; `cd` and
variables persist because a brace group runs in the current shell.

The suite goes from **2632 to exactly 2634**.

## Constraints

- **Not the newline form.** Putting the END printf on its own line without
  the braces (`<cmd>\nprintf …END…`) is wrong, and it is the obvious thing to
  try: the shell reads that line only after the command has run, so its
  echo (`$ printf '\n\x1fDE_''END …' $?`) lands **between the command's
  output and the END marker** and becomes part of every command's captured
  output. Measured on a real PTY at drafting; the brace group has no such
  echo because the whole compound command is read before it runs.
- **Only the `$?` shells get the group.** A shell whose exit variable is
  `$status` (fish, csh, tcsh — see `exit_var`) keeps today's single-line
  form: fish has no brace groups and no heredocs, and csh is not a user of
  this path. Decide by `exit_var`'s answer, not by a second name list.
- The two `parse_outcome` fixtures that spell the single-line echoed form
  (`matt@scrappy:~$ printf …; echo hi; printf …`) are PTY byte captures,
  not wrapped commands; they stay as they are and must keep passing.
- The marker protocol itself does not change: `begin_marker`, `end_marker`,
  `parse_outcome`, `strip_markers` and `log.rs`'s `marked_inner_range` are
  untouched. The split-quote `DE_''BEG` / `DE_''END` spelling stays, for the
  reason the existing doc comment gives.
- **Use `patch` with whole blocks,** never `patch_lines`.

## Spec

1. **Group the command** — `wrap_command` in `src/shell/pty.rs` emits the
   brace-group form above for a `$?` shell and today's form for a `$status`
   shell. Rewrite the doc comment: keep the split-quote paragraph, add why
   the group (the heredoc terminator and the trailing comment) and why not
   the bare newline (the echo). Add
   `wrap_command_groups_the_command_for_a_heredoc_terminator` beside
   `wrap_command_splits_the_marker_word`: for `cat <<'EOF'\nx\nEOF` on
   `bash` the wrapped text contains `\nEOF\n}; printf` and still ends with
   `$?\n`; for `fish` the wrapped text contains no `{ ` and ends with
   `$status\n`.
2. **Prove it on a shell** — `a_heredoc_command_completes_with_its_exit_code`
   beside the other `PtyShell::spawn("bash", …)` tests: run
   `cat <<'EOF'\nde-heredoc-ok\nEOF` with a 5 s timeout, assert exit 0 and
   that the output contains `de-heredoc-ok`; then run `echo after` in the
   **same** shell and assert exit 0 with `after` in the output (the shell
   is not stuck). Then run `sh -c 'exit 3' <<'EOF'\nEOF` and assert exit 3
   (the group passes the real status through).
   **Red run required:** run this test against the pre-phase tree (stash
   step 1, or write the test first) and paste the failing output — the
   timeout error naming the command — fenced, in an
   `### Update — <date> (red run)` entry.
3. **Findings** — one `### Findings — <date>` entry from `git diff --stat`
   alone, two or three lines, or `none`.
4. **End-to-end** — run the block once before committing; paste its output
   fenced in an `### Update — <date> (end-to-end verification)` entry. This
   step is required.
5. **Commit** as `fix: a command ending in a heredoc terminator completes (#1)`.

## Acceptance criteria

- [ ] The E2E block's first line prints `group_form=1 new_tests=2`
      (today `group_form=0 new_tests=0`).
- [ ] The red run is pasted and shows `a_heredoc_command_completes_with_its_exit_code`
      failing on the pre-phase tree with a timeout.
- [ ] hygiene, doc_truth and bug_tracker pass; `build_warnings=0`; clippy
      clean; `fmt_exit=0`.
- [ ] `cargo test` reports exactly `passed=2634 failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
f=src/shell/pty.rs
echo "group_form=$(sed -n '/^#\[cfg(test)\]/q;p' $f | grep -c '}}; printf') new_tests=$(grep -c 'fn wrap_command_groups_the_command_for_a_heredoc_terminator\|fn a_heredoc_command_completes_with_its_exit_code' $f)"
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

`group_form` counts the format string's `}}; printf` (a literal `}` is
doubled inside `format!`) in the production part of the file only — the
test module is cut off first, so the new test's own `}; printf` assertion
does not inflate it.

## Authorizations

- Edit `src/shell/pty.rs`.

## Out of scope

- Sending `^C` to a shell whose command timed out (the socket client's
  timeout in `src/shell/client.rs` bails without signalling; a candidate,
  not scoped).
- The sudo wrapper (`with_pty_prompt_sentinel`), `ask.rs`, the interactive
  path.

## Update Log

<!-- entries appended below this line -->

# Phase 12 — Client prompts

**Status:** todo
**Milestone:** M32 — Findings fixes
**Drafted:** 2026-10-30
**Depends on:** phase-11b
**Estimated diff:** ~145 insertions, ~160 deletions
**Tags:** language=rust, kind=refactor, size=s
**Findings:** M31 `FINDINGS.md` #66, #67, #68 (#27, #28, #30, #55 and
#56 move to phase 12b)

## Goal

The chat client's approval prompts repeat themselves. After this phase:

- **#66:** `src/cli/commands/stream_prompts.rs` has one approval-input
  state machine, `read_approval_input_with`, taking the redraw as a
  closure. `read_approval_input` and `read_approval_input_panel` stay,
  as three-line wrappers, so no caller changes.
- **#67:** the dead `let _path_approved = …` in
  `prompt_edit_file_ratatui` is gone.
- **#68:** the redirect and skipped tail that `prompt_tool_call_ratatui`
  and `prompt_edit_file_ratatui` both end with becomes
  `commit_declined`. In `src/cli/commands/stream_ask.rs`, the nine
  identical prompt status literals in `ask_with_session_ratatui` become
  calls to `prompt_status`, which fixes in one place what a prompt's
  status bar shows: no session id, the daemon down, no tool count.

This is a pure tidy (M32 decision 2). No key, text or frame changes. The
suite stays at **exactly 2609**; a different count means something else
changed.

## Architecture references

- `src/cli/commands/stream_prompts.rs` § the approval primitives.
- M32 README, decision 2.

## Spec

1. **Run this, once, as your first command after the started entry:**

   ```sh
   python3 docs/dev/milestones/M32-findings-fixes/scripts/phase-12-client-prompts.py && cargo fmt --all
   ```

   **Do not open the script, and do not read any source file before
   running it.** It was run on a scratch worktree of `09f7129a`: fmt and
   clippy clean, hygiene passing, the full suite at
   `passed=2609 failed=0`. It asserts its own anchors and prints two
   lines, the last `nine status literals use prompt_status`. If an assert
   fires, stop and report it in the Update Log. Do not hand-edit anything
   it touches, and do not edit the script.
2. **Log it at once.** Append an `### Update — <date> (cuts done)` entry
   quoting the script's output, with an edit tool.
3. **Findings** go in their own `### Findings — <date>` entry. Run
   exactly `git diff --stat`, write what it shows in one or two lines, or
   write `none`. Open no file.
4. **Run the End-to-end block** and paste its output into an
   `### Update — <date> (end-to-end verification)` entry, as a fenced
   block of the output itself, not a description of it.
5. **Commit** as `refactor:`.

The script is
`docs/dev/milestones/M32-findings-fixes/scripts/phase-12-client-prompts.py`
(sha256 begins `8b8a71757b406b20`), kept in the repo as the record of the change.

## Acceptance criteria

- [ ] The End-to-end block prints
      `state_machines=1 dead=0 ladders=1 status_literals=3` (today:
      `state_machines=2 dead=1 ladders=2 status_literals=12`). The three
      status literals left belong to another function and are out of
      scope.
- [ ] `cargo test` reports **exactly** `passed=2609 failed=0`, the same
      count as before the phase.
- [ ] Preservation: `tests/hygiene.rs` and `cargo test --test bug_tracker`
      pass; `tests/hygiene_allowlist.txt` is unchanged. fmt, build and
      clippy are clean.
- [ ] `grep -c '^### Findings — '` on this doc prints `1`, and the
      end-to-end entry holds the fenced output.

## End-to-end verification

```sh
bash <<'E2E'
echo "state_machines=$(grep -c 'let first = read_byte_by(deadline, stdin.read_byte()).await?;' src/cli/commands/stream_prompts.rs) dead=$(grep -c '_path_approved' src/cli/commands/stream_prompts.rs) ladders=$(grep -c 'redirecting agent with your message' src/cli/commands/stream_prompts.rs) status_literals=$(grep -c 'approval_hint: &approval.hint(),' src/cli/commands/stream_ask.rs)"
git diff --quiet HEAD -- tests/hygiene_allowlist.txt && echo allowlist=unchanged
cargo test --test hygiene 2>&1 | grep -E '^test result'
cargo test --test bug_tracker 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

Expected: `state_machines=1 dead=0 ladders=1 status_literals=3`,
`allowlist=unchanged`, hygiene `ok. 16 passed`, bug tracker
`ok. 6 passed`, clippy clean, `fmt_exit=0`, `passed=2609 failed=0`. If
the suite stops early on a failure, paste that output, run the block once
more and paste both.

## Authorizations

- Run `scripts/phase-12-client-prompts.py`; do not edit it.
- Edit the two files it names, through the script only:
  `src/cli/commands/stream_prompts.rs`,
  `src/cli/commands/stream_ask.rs`.

## Out of scope

- The three status literals in `stream_ask.rs`'s other function, which
  carry a session id and no cost.
- #27, #28, #30, #55, #56: phase 12b.
- The `ask_with_session_ratatui` allowlist entry. The function shrinks
  but stays far above the cap, and the gate allows a shrinking entry.

## Update Log

<!-- entries appended below this line -->

# Phase 16 — Sentinels and wrap

**Status:** todo
**Milestone:** M32 — Findings fixes
**Drafted:** 2026-10-30
**Depends on:** phase-15
**Estimated diff:** ~93 insertions, ~25 deletions
**Tags:** language=rust, kind=bugfix, size=s
**Findings:** M32 `FINDINGS.md` #10, #7

## Goal

Two defects, added by the PE on 2026-10-30 after the M32 findings review.
After this phase:

- **M32 #10 (bug):** the remote `edit`, the sudo `edit` and the remote
  `create` read their result through `remote_write_outcome`
  (`src/daemon/executor/file_ops/mod.rs`), as `delete` and `copy` already
  do. They scanned the output for any line *containing* `DE_OK:`, checked
  before `DE_ERROR:`. A failure line that names a path holding `DE_OK:`
  therefore read as success, so a create onto an existing
  `DE_OK:exists.txt` replied `Created …` and wrote nothing.
  `remote_write_outcome` counts only lines before `__DE_DONE__` that
  *start* with a sentinel. Each site keeps its own replies, and the sudo
  edit keeps its loud "did not run" error when neither sentinel appears.
- **M32 #7 (bug):** `wrap_words` (`src/cli/viewer_layout.rs`) budgets
  the first row by subtracting the leading whitespace's **byte** length
  from a width counted in chars. A full-width space (U+3000, 3 bytes)
  wrapped the first row two chars early. The byte length stays for
  `split_at`, and the budget counts chars.

The suite goes from **2612** to **exactly 2615**: three tests added, none
removed.

## Architecture references

- `src/daemon/executor/file_ops/mod.rs` § `remote_write_outcome` and its
  tests: the sentinel contract.
- M32 README, decisions 1 and 2.

## Spec

1. **Run this, once, as your first command after the started entry:**

   ```sh
   python3 docs/dev/milestones/M32-findings-fixes/scripts/phase-16-sentinels-and-wrap.py && cargo fmt --all
   ```

   **Do not open the script, and do not read any source file before
   running it.** It was run on a scratch worktree of the commit that
   drafted this phase: fmt and clippy clean, hygiene passing, the full
   suite at `passed=2615 failed=0`. It asserts its own anchors and prints
   three lines, the last `wrap_words counts leading whitespace in chars`.
   If an assert fires, stop and report it in the Update Log. Do not
   hand-edit anything it touches, and do not edit the script.
2. **Log it at once.** Append an `### Update — <date> (cuts done)` entry
   quoting the script's output, with an edit tool.
3. **Findings** go in their own `### Findings — <date>` entry. Run
   exactly `git diff --stat`, write what it shows in one or two lines, or
   write `none`. Open no file.
4. **Run the End-to-end block** and paste its output into an
   `### Update — <date> (end-to-end verification)` entry, as a fenced
   block of the output itself, not a description of it.
5. **Commit** as `fix:`.

The script is
`docs/dev/milestones/M32-findings-fixes/scripts/phase-16-sentinels-and-wrap.py`
(sha256 begins `a4bc15c28f5ebad1`), kept in the repo as the record of the
change.

### The red run is done — do not repeat it

The three new tests were run against the pre-phase code at drafting. Each
failed on the defect it names:

```
a_failed_create_on_a_sentinel_path_is_an_error ... FAILED
  the failed create is reported as a failure: "Created /tmp/.tmpng9nMM/DE_OK:exists.txt on host web01."
a_failed_edit_on_a_sentinel_path_is_an_error ... FAILED
  the failed edit is reported as a failure: "Edited /tmp/.tmpxCe1ZA/DE_OK:probe.txt on host web01."
wrap_words_counts_leading_whitespace_in_chars ... FAILED
  left: ["\u{3000}abcd", "efgh"]
 right: ["\u{3000}abcd efgh"]
```

The sudo edit has no red test of its own: its swap is the same parser,
and the existing root-edit tests, which cover the refused-password path
the loud error serves, pass with it.

## Acceptance criteria

- [ ] The End-to-end block prints
      `ok_scans=0 outcome_calls=3 sentinel_tests=2 wrap_chars=1 wrap_test=1`
      (today: `ok_scans=3 outcome_calls=0 sentinel_tests=0 wrap_chars=0
      wrap_test=0`).
- [ ] `cargo test` reports **exactly** `passed=2615 failed=0` (today
      2612).
- [ ] Preservation: `tests/hygiene.rs` and `cargo test --test bug_tracker`
      pass; `tests/hygiene_allowlist.txt` is unchanged. fmt, build and
      clippy are clean.
- [ ] `grep -c '^### Findings — '` on this doc prints `1`, and the
      end-to-end entry holds the fenced output.

## End-to-end verification

```sh
bash <<'E2E'
echo "ok_scans=$(grep -c 'line.contains("DE_OK:")' src/daemon/executor/file_ops/ops.rs) outcome_calls=$(grep -c 'super::remote_write_outcome(&' src/daemon/executor/file_ops/ops.rs) sentinel_tests=$(grep -cE 'fn a_failed_(create|edit)_on_a_sentinel_path_is_an_error' src/daemon/executor/file_ops/ops.rs) wrap_chars=$(grep -c 'leading.chars().count()' src/cli/viewer_layout.rs) wrap_test=$(grep -c 'fn wrap_words_counts_leading_whitespace_in_chars' src/cli/viewer.rs)"
git diff --quiet HEAD -- tests/hygiene_allowlist.txt && echo allowlist=unchanged
cargo test --test hygiene 2>&1 | grep -E '^test result'
cargo test --test bug_tracker 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

Expected: the shape line above, `allowlist=unchanged`, hygiene
`ok. 16 passed`, bug tracker `ok. 6 passed`, clippy clean, `fmt_exit=0`,
`passed=2615 failed=0`. If the suite stops early on a failure, paste that
output, run the block once more and paste both.

## Authorizations

- Run `scripts/phase-16-sentinels-and-wrap.py`; do not edit it.
- Edit the three files it names, through the script only:
  `src/daemon/executor/file_ops/ops.rs`, `src/cli/viewer_layout.rs`,
  `src/cli/viewer.rs`.

## Out of scope

- Splitting `run_edit_with_spawn` or `run_create_with_spawn`, both
  allowlisted: the function milestone.
- The perl branch's `DE_ERROR` lines, which do not name the path.

## Update Log

<!-- entries appended below this line -->

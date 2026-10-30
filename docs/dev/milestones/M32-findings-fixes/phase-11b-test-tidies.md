# Phase 11b — Test tidies

**Status:** in-progress
**Milestone:** M32 — Findings fixes
**Drafted:** 2026-10-30
**Depends on:** phase-11
**Estimated diff:** ~140 insertions, ~560 deletions
**Tags:** language=rust, kind=refactor, size=s
**Findings:** M31 `FINDINGS.md` #51, #53, #54 (#5 and #6 are rejected
below)

## Goal

The two render test files build the same renderer and the same status
bar by hand. After this phase:

- **#54:** one `test_renderer(cols, rows, viewport, depth)` in
  `src/cli/render_ratatui_tests.rs` builds every test renderer. The 17
  inline `RatatuiRenderer { … }` builds become one-line calls, and
  `make_test_renderer()` calls it too.
- **#53:** `default_status()` and `approval_test_status()` move from
  the layout test file into `render_ratatui_tests.rs`, shared by both
  files. The 15 inline `StatusBarState` literals that copy one of them
  become calls. The one literal with different values stays inline.
- **#51:** the two bare `assert_eq!`s in
  `src/memory/index/incremental_tests.rs` that a failure would leave
  unexplained get messages.

This is a pure tidy (M32 decision 2). The suite stays at **exactly
2609**; a different count means something else changed.

**Rejected:**
- **#5, #6** (the PTY tests' poll loops and bare sleeps). They were
  filed as a test-speed cost, and the cost was measured at drafting. The
  four PTY test groups take 5.1 s, 3.4 s, 0.7 s and 20.0 s. The 20 s
  group (`daemon::server::shells`) is slow because its 25 tests each
  hold the HOME lock and start a real bash, so they run one after
  another, about 0.8 s each. The 10–50 ms poll intervals cost almost
  nothing, and the loops already wait on a condition with a deadline.
  The client test's 3 s sleep is the behaviour under test: a paused
  command must outlast its deadline. Rewriting about 50 timing-sensitive
  waits would buy no speed and would risk new flakes.
- **#51's other nine:** the `Corpus::from_table` table checks, such as
  `assert_eq!(Corpus::from_table("turns_map"), None)`. The expression
  names the case, and `assert_eq!` prints both sides on failure.

## Architecture references

- `src/cli/render_ratatui.rs` § `RatatuiRenderer`: the struct the
  builder fills.
- M32 README, decision 2.

## Spec

1. **Run this, once, as your first command after the started entry:**

   ```sh
   python3 docs/dev/milestones/M32-findings-fixes/scripts/phase-11b-test-tidies.py && cargo fmt --all
   ```

   **Do not open the script, and do not read any source file before
   running it.** It was run on a scratch worktree of `4bbf45ba`: fmt and
   clippy clean, hygiene passing, the full suite at
   `passed=2609 failed=0`. It asserts its own anchors and prints two
   lines, the last `two assert_eq! messages added`. If an assert fires,
   stop and report it in the Update Log. Do not hand-edit anything it
   touches, and do not edit the script.
2. **Log it at once.** Append an `### Update — <date> (cuts done)` entry
   quoting the script's output, with an edit tool.
3. **Findings** go in their own `### Findings — <date>` entry. Run
   exactly `git diff --stat`, write what it shows in one or two lines, or
   write `none`. Open no file.
4. **Run the End-to-end block** and paste its output into an
   `### Update — <date> (end-to-end verification)` entry, as a fenced
   block of the output itself, not a description of it.
5. **Commit** as `test:`.

The script is
`docs/dev/milestones/M32-findings-fixes/scripts/phase-11b-test-tidies.py`
(sha256 begins `140156f63ba0a0e0`), kept in the repo as the record of the change.

## Acceptance criteria

- [ ] The End-to-end block prints
      `renderer_literals=1 status_literals=3 builder=1 messages=2`
      (today: `renderer_literals=18 status_literals=18 builder=0
      messages=0`).
- [ ] `cargo test` reports **exactly** `passed=2609 failed=0`, the same
      count as before the phase.
- [ ] Preservation: `tests/hygiene.rs` and `cargo test --test bug_tracker`
      pass. fmt, build and clippy are clean.
- [ ] `grep -c '^### Findings — '` on this doc prints `1`, and the
      end-to-end entry holds the fenced output.

## End-to-end verification

```sh
bash <<'E2E'
F="src/cli/render_ratatui_tests.rs src/cli/render_ratatui_layout_tests.rs"
echo "renderer_literals=$(cat $F | grep -c 'RatatuiRenderer {') status_literals=$(cat $F | grep -c 'StatusBarState {') builder=$(grep -c 'pub(super) fn test_renderer(' src/cli/render_ratatui_tests.rs) messages=$(grep -cE 'the seeded memory is indexed before the reconcile probe|the one hit is the incident memory' src/memory/index/incremental_tests.rs)"
cargo test --test hygiene 2>&1 | grep -E '^test result'
cargo test --test bug_tracker 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

Expected: `renderer_literals=1 status_literals=3 builder=1 messages=2`,
hygiene `ok. 16 passed`, bug tracker `ok. 6 passed`, clippy clean,
`fmt_exit=0`, `passed=2609 failed=0`. If the suite stops early on a
failure, paste that output, run the block once more and paste both.

## Authorizations

- Run `scripts/phase-11b-test-tidies.py`; do not edit it.
- Edit the three files it names, through the script only:
  `src/cli/render_ratatui_tests.rs`,
  `src/cli/render_ratatui_layout_tests.rs`,
  `src/memory/index/incremental_tests.rs`.

## Out of scope

- #5, #6, and #51's nine table checks (rejected above).
- The unique `StatusBarState` literal (`session_id: "abcdef12"`,
  `context_window: 0`), which copies neither fixture.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-30 07:02 (started) — executor-claude

Phase 11b start: status flipped to in-progress. Task 1 next: run
`phase-11b-test-tidies.py` as the first command, before opening any source file.

### Update — 2026-10-30 07:02 (cuts done) — executor-claude

Ran, once, as the first command after the started entry (with `cargo fmt --all`
after it, per the spec):

```
$ python3 docs/dev/milestones/M32-findings-fixes/scripts/phase-11b-test-tidies.py && cargo fmt --all
one test_renderer builder; 17 inline renderers and 15 status literals replaced
two assert_eq! messages added
```

Exit 0, no assert fired. Per the spec I have not opened the script or any source
file; task 3's `git diff --stat` is next.

### Findings — 2026-10-30 — executor-claude

`git diff --stat`: the script's three files changed as specified —
`src/cli/render_ratatui_layout_tests.rs` (392), `src/cli/render_ratatui_tests.rs`
(299), `src/memory/index/incremental_tests.rs` (10) — 160 insertions, 565
deletions overall, the remainder being this phase doc and the milestone README's
status flip.

### Update — 2026-10-30 07:02 (end-to-end verification) — executor-claude

Ran the spec's End-to-end block once, verbatim, and pasted its output below
unmodified:

````
renderer_literals=1 status_literals=3 builder=1 messages=2
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.18s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 10.93s
fmt_exit=0
passed=2609 failed=0
````

Matches the spec's expected line-for-line: `renderer_literals=1
status_literals=3 builder=1 messages=2`, hygiene `ok. 16 passed`, bug tracker
`ok. 6 passed`, clippy clean, `fmt_exit=0`, and the suite at exactly
`passed=2609 failed=0`.


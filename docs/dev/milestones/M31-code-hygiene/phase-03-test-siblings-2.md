# Phase 03 — Test modules to sibling files, batch 2

**Status:** done
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-25

## Goal

Seven more files whose production code already fits the cap but whose
inline test module pushes them past 1000 lines keep their tests in a sibling file,
the way `ipc.rs` → `ipc_tests.rs` and `tmux/cache.rs` → `cache_tests.rs`
already do. No test changes, no production change, same test count.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md` — decisions 5 (moves
  are shell range cuts), 6 (no behaviour change) and 7 (the Findings
  entry).
- `src/ipc.rs` and `src/ipc_tests.rs` — the convention: the parent ends in
  `#[cfg(test)] #[path = "ipc_tests.rs"] mod tests;` and the sibling holds
  the module's former body at column 0, with its `use super::*;` intact
  (`super` still names the parent, because `#[path]` does not change the
  module tree).

## Pre-flight

- The four gates are green at HEAD; `cargo test --test hygiene` passes.
- In each file below, the test module is the **last** item: one
  `#[cfg(test)]` line followed by `mod <name> {`, closing with the file's
  final `}`. The module is named `tests` everywhere except
  `daemon/utils/mod.rs`, whose is `sweep_tests`: keep each module's name.
  A test-only item higher in a file (`search.rs` has a test-only helper
  near the top) is not part of the move and stays where it is.

## Spec

The files, with the sibling each gets:

| File | Sibling |
|---|---|
| `src/shell/registry.rs` | `src/shell/registry_tests.rs` |
| `src/shell/log.rs` | `src/shell/log_tests.rs` |
| `src/daemon/executor/file_ops/read.rs` | `src/daemon/executor/file_ops/read_tests.rs` |
| `src/daemon/utils/mod.rs` | `src/daemon/utils/sweep_tests.rs` |
| `src/search.rs` | `src/search_tests.rs` |
| `src/daemon/utils/event_log.rs` | `src/daemon/utils/event_log_tests.rs` |
| `src/daemon/scheduled.rs` | `src/daemon/scheduled_tests.rs` |

1. **Before any edit**, record the test inventory:
   `cargo test -- --list 2>/dev/null | grep ': test$' | sort > /tmp/m31-03-before.txt`.
2. **For each file, one move, with the shell.** Let `T` be the line of the
   trailing `#[cfg(test)]` (`grep -n '^#\[cfg(test)\]' FILE | tail -1`),
   so `T+1` is `mod <name> {` and the last line of the file is its `}`.
   - The sibling is lines `T+2` through the second-to-last line:
     `sed -n "$((T+2)),$(( $(wc -l < FILE) - 1 ))p" FILE > SIBLING`.
   - The parent keeps lines `1` through `T-1`, then the three-line
     declaration: `#[cfg(test)]`, `#[path = "<sibling file name>"]`,
     `mod <name>;`, with `<name>` the module's existing name. Write it with `head -n $((T-1))` into a temp file,
     append the three lines, and `mv` it over FILE.
   - Do **not** retype, re-indent or edit the module body. `cargo fmt
     --all` afterwards brings the sibling to column 0.
3. After all seven: `cargo fmt --all`, then the gates, then
   `cargo test -- --list 2>/dev/null | grep ': test$' | sort > /tmp/m31-03-after.txt`
   and `diff /tmp/m31-03-before.txt /tmp/m31-03-after.txt` must print
   nothing. The seven files and the seven siblings are the only changes
   under `src/`.
4. **Findings** (README decision 7): while the only code read is the test
   bodies as they scroll past, record anything seen: a test that asserts
   nothing, a duplicated fixture that wants a shared helper, a sleep-based
   wait. `none` is a valid entry.

**Gotchas.**
- `#[path]` is relative to the directory of the file that declares the
  module, so it is the sibling's **file name**, not a path.
- Every sibling here is `<stem>_tests.rs` beside its parent, except
  `daemon/utils/mod.rs`, whose module is `sweep_tests` and whose sibling
  is `daemon/utils/sweep_tests.rs`. The hygiene gate classes both shapes
  as test files.
- The hygiene gate measures test siblings against the 2000-line cap; the
  largest body here is about 870 lines, so none needs splitting.

## Acceptance criteria

- [ ] All seven siblings exist, and for each of the seven parents
      `grep -cE '^mod [a-z_]*tests \{' FILE` is 0 while
      `grep -cE '^mod [a-z_]*tests;' FILE` is 1.
- [ ] `diff /tmp/m31-03-before.txt /tmp/m31-03-after.txt` is empty: the
      same tests, by full path, before and after.
- [ ] Preservation: `cargo test` passes with 2586 passing tests, none
      ignored that was not ignored before; `cargo test --test hygiene`
      passes; `git diff --stat HEAD -- src | tail -1` names 14 files.
- [ ] `git diff HEAD -- src` contains no `+` line that is not either one of
      the three declaration lines or a line present in the moved body:
      `git diff -U0 HEAD -- <each parent> | grep '^+' | grep -vE '^\+\+\+|^\+#\[cfg\(test\)\]|^\+#\[path = |^\+mod [a-z_]*tests;'` prints nothing.
- [ ] The Update Log has a `### Findings — <date>` entry.

## Test plan

No new tests: this phase moves tests. The proof is the identical
`--list` inventory and the unchanged pass count.

## End-to-end verification

Run with `bash`. Paste the output byte-exact into the Update Log.

```sh
A=/tmp/e2e-m31-03.txt; : > "$A"
for f in src/shell/registry.rs src/shell/log.rs src/daemon/executor/file_ops/read.rs src/daemon/utils/mod.rs src/search.rs src/daemon/utils/event_log.rs src/daemon/scheduled.rs; do echo "$f inline=$(grep -cE '^mod [a-z_]*tests \{' $f) decl=$(grep -cE '^mod [a-z_]*tests;' $f)" >> "$A"; done
ls src/shell/registry_tests.rs src/shell/log_tests.rs src/daemon/executor/file_ops/read_tests.rs src/daemon/utils/sweep_tests.rs src/search_tests.rs src/daemon/utils/event_log_tests.rs src/daemon/scheduled_tests.rs 2>&1 | wc -l >> "$A"
echo "list_diff_lines=$(diff /tmp/m31-03-before.txt /tmp/m31-03-after.txt | wc -l)" >> "$A"
echo "files_changed=$(git diff --stat HEAD -- src | tail -1)" >> "$A"
for f in src/shell/registry.rs src/shell/log.rs src/daemon/executor/file_ops/read.rs src/daemon/utils/mod.rs src/search.rs src/daemon/utils/event_log.rs src/daemon/scheduled.rs; do git diff -U0 HEAD -- $f | grep '^+' | grep -vE '^\+\+\+|^\+#\[cfg\(test\)\]|^\+#\[path = |^\+mod [a-z_]*tests;'; done | wc -l | sed 's/^/foreign_added_lines=/' >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Expected: seven `inline=0 decl=1` lines, `7`, `list_diff_lines=0`,
`files_changed=` naming 14 files, `foreign_added_lines=0`, hygiene `ok.`,
clippy clean, `fmt_exit=0`, `passed=2586 failed=0`. (If the run is before
a commit, `HEAD` is the pre-phase tree, which is what the diff checks
want; if the moves were committed first, use the pre-phase commit hash in
place of `HEAD`.)

## Authorizations

- Edit the seven files in the table; create the seven siblings. Nothing
  else under `src/`.

## Out of scope

- Any edit inside a test body, any rename, any new test.
- Files whose production code is itself over the cap (`context/epochs.rs`,
  `cli/tail.rs`, `cli/viewer.rs` and the rest): their tests move with their
  split, later. The four files whose test bodies exceed 2000 lines need
  splitting by subject, a later phase.
- CLAUDE.md's "Key files" table: test siblings are not modules it lists.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-26 00:43 (in progress, executor)

**Executor:** nvidia/GLM-5.3-Flash-FP4-Local

Executed M31 phase-03 (test-siblings-2): recorded the pre-edit test inventory
(2599 entries into `/tmp/m31-03-before.txt`), then moved the trailing inline
test module of each of the seven listed files to its specified sibling by
shell range cut (`registry_tests.rs`, `log_tests.rs`, `read_tests.rs`,
`sweep_tests.rs`, `search_tests.rs`, `event_log_tests.rs`,
`scheduled_tests.rs`), each parent ending in the three-line
`#[cfg(test)] #[path = "<sibling>"] mod <name>;` declaration with the
module's existing name (`sweep_tests` for `daemon/utils/mod.rs`). Gates run:
`cargo fmt --all` (pass), `cargo build` (no warnings), `cargo clippy
--all-targets --all-features -- -D warnings` (clean), `cargo test`
(passed=2586 failed=0, inventory diff empty — same 2599 tests by full path),
`cargo test --test hygiene` (ok. 16 passed). End-to-end transcript appended
below. Findings entry appended separately.

```
src/shell/registry.rs inline=0 decl=1
src/shell/log.rs inline=0 decl=1
src/daemon/executor/file_ops/read.rs inline=0 decl=1
src/daemon/utils/mod.rs inline=0 decl=1
src/search.rs inline=0 decl=1
src/daemon/utils/event_log.rs inline=0 decl=1
src/daemon/scheduled.rs inline=0 decl=1
7
list_diff_lines=0
files_changed= 7 files changed, 14 insertions(+), 5322 deletions(-)
foreign_added_lines=0
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2586 failed=0
```

### Findings — 2026-10-26

Read nothing but test bodies as they moved (5,315 sibling lines, ~240
`#[test]`/`#[tokio::test]` items). `none` was the expected entry; instead:

- **`search_tests.rs` opens with a `static COUNTER:
  std::sync::atomic::AtomicU64`** used for unique temp dir names — the same
  unique-temp-dir need phase-02 flagged in other siblings; a crate-level
  helper (`crate::unique_temp_dir()`) would retire it too. Recorded here,
  not fixed (out of scope).
- **`scheduled_tests.rs` `presence_count` re-derivation.** Two tests recompute
  expected presence counts by hand in the assertion (duplicated fixture
  logic); a shared `expected_presence()` helper would shrink them. Minor.
- **No test asserts nothing; no sleep-based waits in these seven siblings**
  (checked each for `std::thread::sleep`/`tokio::time::sleep` outside
  deadline loops: none). No duplicated fixture rose to the level of the
  temp-HOME copies flagged in phase-02.

### Review verdict — 2026-10-26

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** nvidia/GLM-5.3-Flash-FP4-Local (55 turns)
- **Scope deviations:** none. rustfmt dropped seven trailing commas when it
  rejoined wrapped calls after the dedent (`sweep_tests.rs` 2,
  `search_tests.rs` 3, `scheduled_tests.rs` 2); every other token matches
  the pre-phase body, compared with whitespace removed.
- **Calibration:** the review re-ran the four gates (fmt 0, build and
  clippy clean, 2586 passed / 0 failed / 13 ignored) and the criteria
  against `552c4ea5`: one hunk per parent, the right declaration (module
  name `sweep_tests` kept), no foreign line, `search.rs`'s top test-only
  helper left in place, 14 files. **Architect error:** this doc was derived
  from phase 02's after phase 02 was flipped to `done`, so it was drafted
  and dispatched with `Status: done` (the README row stayed `todo`); the
  dispatch did not check the line. Harmless here, since the executor
  worked from the Spec, but a derived doc must reset its status. Executor
  inaccuracies, minor: its summary says 3 tests are ignored (13 are), and
  its reading of the `--stat` line is wrong, though the transcript itself
  is right.

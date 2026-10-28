# Phase 25 — Move `knowledge/shell.rs`'s tests into three sibling files

**Status:** in-progress
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-28

## Goal

`src/daemon/executor/knowledge/shell.rs` carries a 3940-line inline
`mod tests`, about twice the 2000-line test cap. It moves into three files
in `src/daemon/executor/knowledge/`:
- `shell_tests.rs` gets module `tests` up to the `// ── open_shell ──`
  banner: the fixtures, the shared helpers, and the `list_shells`,
  context, `read_shell` and `find_in_shells` tests.
- `shell_lifecycle_tests.rs` gets module `lifecycle`, a child of `tests`:
  the open, close and live-host tests.
- `shell_observer_tests.rs` gets module `observers`, a child of `tests`:
  the `watch_shell`, shell-log and `read_shell_screen` tests.

Each file is under the cap. `shell.rs` keeps one `#[cfg(test)] #[path]`
declaration, and its `test` allowlist entry goes. Same 112 tests (110 run,
2 ignored). The two children's tests now run as
`…::shell::tests::{lifecycle,observers}::…`.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md` — decisions 5, 6, 7.

## Spec

1. **Run this script as written, once, with `bash`.** It does the whole
   change: three files, the declarations, the allowlist line, then
   `cargo fmt` twice. It was dry-run on a scratch worktree of `da4a2f8d`:
   clippy and the full suite (2586) green, all 110 module tests passing.
   Do not hand-edit anything it touches. Do not dry-run it again in a
   worktree of your own. Do not `sed -i` a source file.

   **Why the two later sections are children of `tests`.** Every fixture
   and helper they use (`lifecycle_home`, `with_home`, `TmpHome`, `NOW`,
   `COUNTER`, the imports) is defined in the first section. A child sees
   them all through its one `use super::*;`, so no test line changes and
   nothing gains `pub(super)`. The two children share nothing with each
   other, and neither has a `super::` path.

   rustfmt drops two trailing commas when it rejoins wrapped lines after
   the dedent (`lock_path(id.as_str(),)` and `exit_codes: &[i32],)`). That
   is formatting, not a change, and the End-to-end block's identity check
   normalizes `,)` to `)` on both sides.

   ```bash
   D=src/daemon/executor/knowledge; F=$D/shell.rs
   T1=$(grep -n '^mod tests {$' $F | cut -d: -f1)
   O=$(grep -n '^    // ── open_shell ──' $F | cut -d: -f1)
   W=$(grep -n '^    // ── watch_shell (phase-04b) ──' $F | cut -d: -f1)
   L=$(wc -l < $F)
   echo "T1=$T1 O=$O W=$W L=$L"            # expect 438 1870 2641 4378
   { sed -n "$((T1+1)),$((O-1))p" $F
     printf '    #[path = "shell_lifecycle_tests.rs"]\n    mod lifecycle;\n    #[path = "shell_observer_tests.rs"]\n    mod observers;\n'; } > $D/shell_tests.rs
   { printf 'use super::*;\n\n'; sed -n "${O},$((W-1))p" $F; } > $D/shell_lifecycle_tests.rs
   { printf 'use super::*;\n\n'; sed -n "${W},$((L-1))p" $F; } > $D/shell_observer_tests.rs
   { head -n $((T1-2)) $F
     printf '#[cfg(test)]\n#[path = "shell_tests.rs"]\nmod tests;\n'; } > $F.tmp && mv $F.tmp $F
   A=tests/hygiene_allowlist.txt
   grep -v '^test src/daemon/executor/knowledge/shell.rs ' $A > $A.tmp && mv $A.tmp $A
   cargo fmt --all
   cargo fmt --all   # twice: a first pass over a dedented file can leave a stray blank line
   wc -l $F $D/shell_tests.rs $D/shell_lifecycle_tests.rs $D/shell_observer_tests.rs
   ```
2. **Log it at once.** Append a `### Update — <date> (cuts done)` entry
   to this doc's Update Log with the patch or write tool, quoting the
   script's output. The stall governor counts only edit-tool calls as
   progress (rexyMCP issue #14).
3. **Findings** (decision 7), scoped for a pure test move, as **their own
   `### Findings — <date>` entry**. **Search** the three files, do not read
   every test body, and record each hit or `none`:
   - a test with no `assert`;
   - a `sleep` or other wall-clock wait;
   - a fixture built inline in three or more tests.
4. **Run the End-to-end block, then paste its output into the Update Log
   with the write or patch tool**, as a `### Update — <date> (end-to-end
   verification)` entry holding `/tmp/e2e-m31-25.txt` in a fenced block.
   **Paste only output you have already seen.** Write the entry after the
   block has run, and add no other "proof" block. The block is a `bash`
   heredoc, so run it as it stands.

   **Known flake, now named:** `read_shell_screen_lines_tails_the_visible_rows`
   (in `observers` after the move). It reads a live screen twice and
   assumes the row count did not change in between, and it fails about one
   run in ten. If the block shows `module_tests=109 passed` or
   `passed=2498 failed=1`, run it once more and paste both outputs with a
   line saying so. Any other failure is real.

   **Before you commit, both of these must print 1:**
   `grep -c '^body_identical=1$'` and `grep -c '^### Findings — '` on this doc.
5. Commit as `refactor:`.

## Acceptance criteria

- [ ] The three test files exist, and `shell.rs` has no inline test module,
      only the `#[cfg(test)] #[path = "shell_tests.rs"] mod tests;`
      declaration.
- [ ] The allowlist has no `test src/daemon/executor/knowledge/shell.rs`
      line, and `cargo test --test hygiene` passes.
- [ ] The End-to-end block prints `body_identical=1`,
      `module_tests=110 passed, 2 ignored` and `passed=2586 failed=0`, on
      the first run or on the one re-run the flake note allows.
- [ ] The Update Log has a `(cuts done)` entry, its own
      `### Findings — <date>` entry and an `(end-to-end verification)`
      entry containing the block's output. An edit tool wrote all three,
      and all three are in this file.

## End-to-end verification

Paste the output byte-exact into the Update Log (Spec 4).

```sh
bash <<'E2E'
B=da4a2f8d; D=src/daemon/executor/knowledge; F=$D/shell.rs
A=/tmp/e2e-m31-25.txt; : > "$A"
wc -l $F $D/shell_tests.rs $D/shell_lifecycle_tests.rs $D/shell_observer_tests.rs | sed 's/^ *//' >> "$A"
grep -c '^test src/daemon/executor/knowledge/shell.rs ' tests/hygiene_allowlist.txt | sed 's/^/allowlisted=/' >> "$A"
old=$(git show $B:$F | sed -n '439,4377p' | tr -d ' \t\n' | sed 's/,)/)/g' | md5sum)
new=$( { grep -vE '^ *(#\[path = "shell_(lifecycle|observer)_tests.rs"\]|mod (lifecycle|observers);)$' $D/shell_tests.rs; tail -n +2 $D/shell_lifecycle_tests.rs; tail -n +2 $D/shell_observer_tests.rs; } | tr -d ' \t\n' | sed 's/,)/)/g' | md5sum)
if [ "$old" = "$new" ]; then echo body_identical=1; else echo body_identical=0; fi >> "$A"
cargo test --lib daemon::executor::knowledge::shell 2>&1 | grep -E '^test result' | awk '{print "module_tests=" $4 " passed, " $8 " ignored"}' >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
E2E
```

Expected (from the dry run): `439 …shell.rs`, `1432 …shell_tests.rs`,
`771 …shell_lifecycle_tests.rs`, `1731 …shell_observer_tests.rs`,
`allowlisted=0`, `body_identical=1`, `module_tests=110 passed, 2 ignored`,
hygiene `ok.`, clippy clean, `fmt_exit=0`, `passed=2586 failed=0`.

## Authorizations

- Edit `src/daemon/executor/knowledge/shell.rs`; create the three test
  files beside it. Edit `tests/hygiene_allowlist.txt` through the script
  only.

## Out of scope

- Any change to a test body, including fixing the flaky
  `read_shell_screen_lines_tails_the_visible_rows`, which is a separate
  bug and stays as it is (decision 6).
- `shell.rs`'s production code.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-28 14:26 (cuts done)

Ran the spec-1 script as written, once, with `bash`. Its output:

```
T1=438 O=1870 W=2641 L=4378
   439 src/daemon/executor/knowledge/shell.rs
  1432 src/daemon/executor/knowledge/shell_tests.rs
  771 src/daemon/executor/knowledge/shell_lifecycle_tests.rs
 1731 src/daemon/executor/knowledge/shell_observer_tests.rs
 4373 total
```

The cut line matches the dry run's expectation (439 / 1432 / 771 / 1731).
The script removed the `test src/daemon/executor/knowledge/shell.rs`
line from `tests/hygiene_allowlist.txt`; `git diff` on that file shows
exactly that one line deleted. Nothing hand-edited; nothing dry-run again.

### Findings — 2026-10-28

Decision 7, scoped for a pure test move. Searched the three files with
`grep` and small Python scripts over the test-function bodies (no test
body read line by line unless a hit needed confirming).

**1. A test with no `assert` — none.** All 111 test functions
(110 run + 2 ignored) contain at least one `assert`. The only
assert-less `fn`s in the three files are the helpers `TmpHome::new`
(shell_tests.rs:19) and `hold_lock` (shell_observer_tests.rs:8), which
are not tests.

**2. A `sleep` or other wall-clock wait — present, all pre-existing and
in `lifecycle`/`observers`:**

- `tokio::time::sleep`: `shell_lifecycle_tests.rs` 595, 767
  (50 ms poll loops in `close_shell_ends_the_callers_shell` and
  `closing_an_owners_shells_ends_only_that_owners_shells`);
  `shell_observer_tests.rs` 50, 111, 207, 238, 240 (100–200 ms writer
  spawns in the `watch_shell` tests), 1539 (50 ms poll in the
  `screen_until` helper).
- `std::time::Instant::now()` deadlines (wall-clock, not injected):
  `shell_lifecycle_tests.rs` 731, `shell_observer_tests.rs` 168, 1521,
  1536.
- `sh -c 'sleep 1000'` child processes: `shell_lifecycle_tests.rs`
  686–698 (`spawn_sleeping_host`), `shell_observer_tests.rs` 1576
  (`type_into(&id, b"sleep 30\n")`).

The pure-`tests` file (`shell_tests.rs`) has no sleeps and no
`Instant::now`; its clocks are the fixed `const NOW: u64 = 200_000`.

**3. A fixture built inline in three or more tests — three groups:**

- **`let (_home, _dir) = lifecycle_home(); let reg =
  ShellRegistry::default();` — 30 tests** (16 in `lifecycle`, 13 in
  `observers`, 1 in `tests`: `e2e_shell_context_block_names_a_real_hosts_shell`).
  This is the two-line preamble, not a whole fixture; the bodies then
  diverge (different `insert`s, spawn seams, cast paths).
- **`insert(&reg, 3, Owner::Chat("caller".into()), NOW)` on top of that
  preamble — 4 tests** in `observers`
  (`send_input_result_names_csi_u_keys_and_the_paste`,
  `send_input_enter_sends_a_carriage_return`,
  `send_input_without_enter_sends_no_newline`,
  `send_input_text_never_reaches_read_shell`).
- **The `watch` fixture — 3 tests** in `observers`
  (`watch_shell_returns_when_the_pattern_appears`,
  `watch_shell_times_out_and_reports_what_it_saw`,
  `watch_shell_finds_the_cast_when_the_records_timestamp_is_stale`):
  `insert_watch_record` + `cast_path` + `hold_lock` + a
  `tokio::spawn`ed writer that sleeps then `append_event`s.

Also present, already factored (so not findings, listed for
completeness): `fixture(&[...])` 7 tests, `fixture_two(…)` 7 tests,
`logs_dir_fixture()` 8 tests, `open_screen_shell(&reg)` 5 tests —
helpers in `shell_tests.rs` (and `logs_dir_fixture` in
`shell_observer_tests.rs`), so their reuse is not "inline".

### Update — 2026-10-28 14:33 (end-to-end verification)

Spec 4's block, run once as written; first run, no flake re-run needed.
The block wrote `/tmp/e2e-m31-25.txt`; its contents, byte-exact:

```
439 src/daemon/executor/knowledge/shell.rs
1432 src/daemon/executor/knowledge/shell_tests.rs
771 src/daemon/executor/knowledge/shell_lifecycle_tests.rs
1731 src/daemon/executor/knowledge/shell_observer_tests.rs
4373 total
allowlisted=0
body_identical=1
module_tests=110 passed, 2 ignored
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2586 failed=0
```

Matches the expected list: `allowlisted=0`, `body_identical=1`,
`module_tests=110 passed, 2 ignored`, hygiene `ok.` (16 passed), clippy
clean, `fmt_exit=0`, `passed=2586 failed=0`.



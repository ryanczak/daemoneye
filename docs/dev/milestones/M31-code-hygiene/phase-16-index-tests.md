# Phase 16 — Move the memory index's tests into two sibling files

**Status:** in-progress
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-27

## Goal

`src/memory/index/mod.rs` carries a 2543-line inline `mod tests`, over the
2000-line test cap. It moves into two files beside it in
`src/memory/index/`:
- `tests.rs` gets the module from its start up to the `// ── Phase 03a
  tests` banner: schema, FTS5 search and schema v2.
- `incremental_tests.rs` gets the rest: the incremental index hooks and
  the per-corpus reconcile.

Each file is under the cap. `mod.rs` keeps two `#[cfg(test)] mod …;` lines
in the module's place. Same 64 tests, same pass count. The 24 tests in the
second file now run as `memory::index::incremental_tests::…`.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md` — decisions 5, 6, 7.

## Spec

1. **Run this script as written, once, with `bash`.** It does the whole
   change: both test files, the two declarations in `mod.rs`, the
   allowlist line, then `cargo fmt` (twice, see the comment in the script),
   which brings both files to column 0.
   It was dry-run on a scratch worktree of `ea12993b`: clippy and the full
   suite (2586) green, all 64 `memory::index::` tests passing. Do not
   hand-edit anything it touches. Do not dry-run it again in a worktree of
   your own. Do not `sed -i` a source file.

   One line changes inside the moved tests. The module's
   `use std::io::{BufRead, Seek};` becomes `use std::io::BufRead;` in
   `tests.rs`, because that half reaches `Seek` only through its own inner
   `use std::io::Seek;` lines, and clippy rejects the unused import.
   `incremental_tests.rs` opens with `use super::*;` and the original
   `std::io` import, both of which it needs. rustfmt rejoins three
   wrapped lines after the dedent. That is formatting, not a change, and
   the End-to-end block compares the bodies with whitespace ignored.

   ```bash
   D=src/memory/index; F=$D/mod.rs
   T=$(grep -n '^#\[cfg(test)\]' $F | tail -1 | cut -d: -f1)
   P=$(grep -n '^    // ── Phase 03a tests ' $F | cut -d: -f1)
   L=$(wc -l < $F)
   echo "T=$T P=$P L=$L"                  # expect 500 2086 3042
   sed -n "$((T+2)),$((P-1))p" $F | sed 's/^    use std::io::{BufRead, Seek};$/    use std::io::BufRead;/' > $D/tests.rs
   { printf 'use super::*;\nuse std::io::{BufRead, Seek};\n\n'; sed -n "${P},$((L-1))p" $F; } > $D/incremental_tests.rs
   { head -n $((T-1)) $F
     printf '#[cfg(test)]\nmod tests;\n#[cfg(test)]\nmod incremental_tests;\n'; } > $F.tmp && mv $F.tmp $F
   A=tests/hygiene_allowlist.txt
   grep -v '^test src/memory/index/mod.rs ' $A > $A.tmp && mv $A.tmp $A
   cargo fmt --all
   cargo fmt --all   # twice: the first pass leaves a blank line at the top of tests.rs
   wc -l $F $D/tests.rs $D/incremental_tests.rs
   ```
2. **Log it at once.** Append a `### Update — <date> (cuts done)` entry
   to this doc's Update Log with the patch or write tool, quoting the
   script's output (`T=… P=… L=…` and the `wc -l` counts). The stall
   governor counts only edit-tool calls as progress (rexyMCP issue #14).
3. **Findings** (decision 7): skim the two test files once and record
   what you see (a test that asserts nothing, a duplicated fixture, a
   sleep), or `none`.
4. **Run the End-to-end block and paste its output into the Update Log
   with the write or patch tool**, as a `### Update — <date> (end-to-end
   verification)` entry holding `/tmp/e2e-m31-16.txt` in a fenced block.
   The block is a `bash` heredoc, so run it as it stands. Check with
   `grep -c '^body_identical=1$'` on this doc before you commit; it must
   print 1.
5. Commit as `refactor:`.

## Acceptance criteria

- [ ] `src/memory/index/tests.rs` and `src/memory/index/incremental_tests.rs`
      exist. `mod.rs` has no inline test module, only the two
      `#[cfg(test)] mod …;` declarations.
- [ ] The allowlist has no `test src/memory/index/mod.rs` line, and
      `cargo test --test hygiene` passes (each new file is under 2000
      lines).
- [ ] The End-to-end block prints `body_identical=1`,
      `index_tests=64 passed` and `passed=2586 failed=0`.
- [ ] The Update Log has a `(cuts done)` entry, a `### Findings — <date>`
      entry and an `(end-to-end verification)` entry containing the
      block's output. An edit tool wrote all three, and all three are in
      this file.

## End-to-end verification

Paste the output byte-exact into the Update Log (Spec 4).

```sh
bash <<'E2E'
B=ea12993b; D=src/memory/index; F=$D/mod.rs
A=/tmp/e2e-m31-16.txt; : > "$A"
wc -l $F $D/tests.rs $D/incremental_tests.rs | sed 's/^ *//' >> "$A"
grep -c '^test src/memory/index/mod.rs ' tests/hygiene_allowlist.txt | sed 's/^/allowlisted=/' >> "$A"
old=$(git show $B:$F | sed -n 502,3041p | grep -vE '^    use (super::\*|std::io::\{BufRead, Seek\});$' | tr -d ' \t\n' | md5sum)
new=$(cat $D/tests.rs $D/incremental_tests.rs | grep -vE '^use (super::\*|std::io::BufRead|std::io::\{BufRead, Seek\});$' | tr -d ' \t\n' | md5sum)
if [ "$old" = "$new" ]; then echo body_identical=1; else echo body_identical=0; fi >> "$A"
cargo test --lib memory::index:: 2>&1 | grep -E '^test result' | awk '{print "index_tests=" $4 " passed"}' >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
E2E
```

Expected (from the dry run): `503 …mod.rs`, `1581 …tests.rs`,
`958 …incremental_tests.rs`, `allowlisted=0`, `body_identical=1`,
`index_tests=64 passed`, hygiene `ok.`, clippy clean, `fmt_exit=0`,
`passed=2586 failed=0`.

## Authorizations

- Edit `src/memory/index/mod.rs`; create `tests.rs` and
  `incremental_tests.rs` beside it. Edit `tests/hygiene_allowlist.txt`
  through the script only.

## Out of scope

- Any change to a test body, beyond the one narrowed import.
- `mod.rs`'s production code, which the previous phase settled.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-27 20:45 (started)

**Executor:** this run. Phase 16 is in progress. The phase doc's Status is
`in-progress` and the milestone README's phase-table row matches. No source
file touched yet; Spec 1's script runs next.

### Update — 2026-10-27 20:47 (cuts done)

**Spec 1 script run once, as written, with `bash`.** Output, byte-exact:

```text
T=500 P=2086 L=3042
   503 src/memory/index/mod.rs
  1581 src/memory/index/tests.rs
  958 src/memory/index/incremental_tests.rs
  3042 total
```

All three counts match the expected values (`T=500 P=2086 L=3042`; 503 /
1581 / 958). The one-line change inside the moved tests landed: `tests.rs`
line 2 reads `use std::io::BufRead;` (narrowed from
`use std::io::{BufRead, Seek};`), while `incremental_tests.rs` keeps the
original `use std::io::{BufRead, Seek};`. `mod.rs` ends with the two
`#[cfg(test)] mod …;` declarations and no inline test module.
`tests/hygiene_allowlist.txt` has zero remaining
`test src/memory/index/mod.rs ` lines. `cargo fmt --all` ran twice per the
script; `mod.rs`'s two declarations came out as `mod incremental_tests;`
then `mod tests;`.

### Findings — 2026-10-27

Skimmed all 64 tests across the two files. Findings, per README decision 7
(each names file and symbol, kind, what was seen, why it matters, severity;
none acted on):

1. `src/memory/index/tests.rs` + `src/memory/index/incremental_tests.rs` ·
   57 test bodies — **smell, minor**. The three-line prologue
   `let _guard = crate::test_home_guard(); let tmp =
   tempfile::tempdir().unwrap(); unsafe { std::env::set_var("HOME",
   tmp.path()) };` is copy-pasted 57 times. A `#[fixture]`-style helper or a
   plain `fn test_home() -> (TestHomeGuard, TempDir)` would delete ~170
   lines and centralise the unsafe `set_var` dance in one place.
2. `src/memory/index/incremental_tests.rs` · `make_test_message` —
   **smell, nit**. This private helper duplicates
   `mod.rs::make_test_message_for_index` (which exists for
   `daemon::utils::sweep_tests`). One of the two should go; the sweep tests
   could import the same helper.
3. `src/memory/index/tests.rs` · `reconcile_indexes_archive_turns`,
   `turns_map_offsets_point_at_the_right_line` — **smell, nit**. Both build
   the identical three-line JSON transcript fixture
   (`{"role":"user","content":"hello","turn":0}` …) and write it to the
   archive; a shared fixture would halve the setup.
4. `src/memory/index/incremental_tests.rs` · 16 `assert_eq!` calls —
   **smell, nit**. Sixteen assertions carry no failure message (e.g. in
   `incremental_and_reconcile_agree`'s count comparisons) where the file's
   other asserts use per-assertion messages; STANDARDS §3.3 prefers one.
5. `src/memory/index/incremental_tests.rs` ·
   `incremental_and_reconcile_agree` (98 lines) — **smell, nit**. The
   longest test in either file; a candidate for the same fixture-helper
   tidy as #1. Not over any cap (150), so no action needed this milestone.

No test asserts nothing (a mechanical scan found every test body carries at
least one `assert`); no `sleep`, no real wall-clock, no unseeded RNG. The
`unsafe { std::env::set_var("HOME", …) }` pattern appears in all 57 tests
but predates this phase and is the established pattern across the repo's
test files (`src/lib.rs::test_home_guard` guards it).

### Update — 2026-10-27 21:00 (end-to-end verification)

The End-to-end block from this phase doc ran once, as a `bash` heredoc,
exactly as written; its full output (`/tmp/e2e-m31-16.txt`) is below,
byte-exact:

```text
503 src/memory/index/mod.rs
1581 src/memory/index/tests.rs
958 src/memory/index/incremental_tests.rs
3042 total
allowlisted=0
body_identical=1
index_tests=64 passed
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2586 failed=0
```

Read against the acceptance criteria: line counts match the dry run
(503 / 1581 / 958, each new file under the 2000-line test cap);
`allowlisted=0` (the allowlist's `test src/memory/index/mod.rs ` line is
gone); `body_identical=1` (the moved test bodies are whitespace-identical
to `ea12993b`'s inline module modulo the two import lines the split
accounts for); `index_tests=64 passed`; the hygiene test (16 tests)
passes; clippy is clean (`Finished … target(s)`, no warnings under `-D
warnings`); `fmt_exit=0`; the full suite is `passed=2586 failed=0`.



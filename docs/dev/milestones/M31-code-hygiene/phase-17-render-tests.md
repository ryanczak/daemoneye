# Phase 17 — Move `render_ratatui.rs`'s tests into two sibling files

**Status:** todo
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-27

## Goal

`src/cli/render_ratatui.rs` carries a 2055-line inline `mod tests`, over
the 2000-line test cap. It moves into two siblings in `src/cli/`:
- `render_ratatui_tests.rs` gets the tests up to the
  `// ── Helpers for spinner-row tests` banner (the parser, renderer and
  picker tests).
- `render_ratatui_layout_tests.rs` gets the rest, as module `layout_tests`:
  the spinner row, cursor placement, repaint plans, and the approval and
  credential panels.

Each file is under the cap. The parent keeps two `#[cfg(test)] #[path]`
declarations. Same 59 tests, same pass count. The tests that move to the
second file now run as `cli::render_ratatui::layout_tests::…`.
The file's 1225 production lines are a later phase.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md` — decisions 5, 6, 7.

## Spec

1. **Run this script as written, once, with `bash`.** It does the whole
   change: both test files, the two declarations, the allowlist line, then
   `cargo fmt` twice (see the comment in the script). It was dry-run on a
   scratch worktree of `39541871`: clippy and the full suite (2586) green, all
   59 `cli::render_ratatui::` tests passing. Do not hand-edit anything it
   touches. Do not dry-run it again in a worktree of your own. Do not
   `sed -i` a source file.

   One line changes inside the moved tests. The shared fixture
   `make_test_renderer`, defined in the first half and used 11 times in
   the second, gains `pub(super)`, and the second file imports it as
   `use super::tests::make_test_renderer;`. The second file opens with
   that line, `use super::*;` and `use ratatui::backend::TestBackend;`,
   because it needs all three.

   ```bash
   D=src/cli; F=$D/render_ratatui.rs
   T=$(grep -n '^#\[cfg(test)\]' $F | tail -1 | cut -d: -f1)
   H=$(grep -n '^    // ── Helpers for spinner-row tests' $F | cut -d: -f1)
   L=$(wc -l < $F)
   echo "T=$T H=$H L=$L"                  # expect 1226 2185 3281
   sed -n "$((T+2)),$((H-1))p" $F | sed 's/^    fn make_test_renderer(/    pub(super) fn make_test_renderer(/' > $D/render_ratatui_tests.rs
   { printf 'use super::tests::make_test_renderer;\nuse super::*;\nuse ratatui::backend::TestBackend;\n\n'; sed -n "${H},$((L-1))p" $F; } > $D/render_ratatui_layout_tests.rs
   { head -n $((T-1)) $F
     printf '#[cfg(test)]\n#[path = "render_ratatui_tests.rs"]\nmod tests;\n#[cfg(test)]\n#[path = "render_ratatui_layout_tests.rs"]\nmod layout_tests;\n'; } > $F.tmp && mv $F.tmp $F
   A=tests/hygiene_allowlist.txt
   grep -v '^test src/cli/render_ratatui.rs ' $A > $A.tmp && mv $A.tmp $A
   cargo fmt --all
   cargo fmt --all   # twice: a first pass over a dedented file can leave a stray blank line
   wc -l $F $D/render_ratatui_tests.rs $D/render_ratatui_layout_tests.rs
   ```
2. **Log it at once.** Append a `### Update — <date> (cuts done)` entry
   to this doc's Update Log with the patch or write tool, quoting the
   script's output. The stall governor counts only edit-tool calls as
   progress (rexyMCP issue #14).
3. **Findings** (decision 7), scoped for a pure test move. Do **not** read
   every test body line by line; the last test move spent most of its run
   doing that. Search the two files for these and record each hit, or
   `none`:
   - a test with no `assert`;
   - a `sleep` or other wall-clock wait;
   - a fixture built inline in three or more tests.
4. **Run the End-to-end block and paste its output into the Update Log
   with the write or patch tool**, as a `### Update — <date> (end-to-end
   verification)` entry holding `/tmp/e2e-m31-17.txt` in a fenced block.
   The block is a `bash` heredoc, so run it as it stands. Check with
   `grep -c '^body_identical=1$'` on this doc before you commit; it must
   print 1.
5. Commit as `refactor:`.

## Acceptance criteria

- [ ] `src/cli/render_ratatui_tests.rs` and
      `src/cli/render_ratatui_layout_tests.rs` exist, and
      `render_ratatui.rs` has no inline test module.
- [ ] The allowlist has no `test src/cli/render_ratatui.rs` line. Its
      `file src/cli/render_ratatui.rs 1225` line stays.
      `cargo test --test hygiene` passes.
- [ ] The End-to-end block prints `body_identical=1`,
      `module_tests=59 passed` and `passed=2586 failed=0`.
- [ ] The Update Log has a `(cuts done)` entry, a `### Findings — <date>`
      entry and an `(end-to-end verification)` entry containing the
      block's output. An edit tool wrote all three, and all three are in
      this file.

## End-to-end verification

Paste the output byte-exact into the Update Log (Spec 4).

```sh
bash <<'E2E'
B=39541871; D=src/cli; F=$D/render_ratatui.rs
A=/tmp/e2e-m31-17.txt; : > "$A"
wc -l $F $D/render_ratatui_tests.rs $D/render_ratatui_layout_tests.rs | sed 's/^ *//' >> "$A"
grep -c '^test src/cli/render_ratatui.rs ' tests/hygiene_allowlist.txt | sed 's/^/allowlisted=/' >> "$A"
old=$(git show $B:$F | sed -n 1228,3280p | sed 's/^    fn make_test_renderer(/    pub(super) fn make_test_renderer(/' | grep -vE '^    use (super::\*|ratatui::backend::TestBackend);$' | tr -d ' \t\n' | md5sum)
new=$(cat $D/render_ratatui_tests.rs $D/render_ratatui_layout_tests.rs | grep -vE '^use (super::\*|super::tests::make_test_renderer|ratatui::backend::TestBackend);$' | tr -d ' \t\n' | md5sum)
if [ "$old" = "$new" ]; then echo body_identical=1; else echo body_identical=0; fi >> "$A"
cargo test --lib cli::render_ratatui:: 2>&1 | grep -E '^test result' | awk '{print "module_tests=" $4 " passed"}' >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
E2E
```

Expected (from the dry run): `1231 …render_ratatui.rs`,
`955 …render_ratatui_tests.rs`, `1100 …render_ratatui_layout_tests.rs`,
`allowlisted=0`, `body_identical=1`, `module_tests=59 passed`, hygiene
`ok.`, clippy clean, `fmt_exit=0`, `passed=2586 failed=0`.

## Authorizations

- Edit `src/cli/render_ratatui.rs`; create the two test files beside it.
  Edit `tests/hygiene_allowlist.txt` through the script only.

## Out of scope

- `render_ratatui.rs`'s production code (still over 800; a later phase).
- Any change to a test body beyond `make_test_renderer`'s `pub(super)`.

## Update Log

<!-- entries appended below this line -->

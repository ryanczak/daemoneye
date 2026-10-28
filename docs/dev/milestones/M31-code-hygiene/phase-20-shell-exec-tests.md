# Phase 20 — Move `shell_exec.rs`'s tests into three sibling files

**Status:** todo
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-28

## Goal

`src/daemon/executor/shell_exec.rs` carries 3091 lines of inline tests in
two modules, over the 2000-line test cap. They move into three files in
`src/daemon/executor/`:
- `shell_exec_tests.rs` gets module `tests` up to the
  `// ── run_terminal_command { host? }` banner: the fixtures, helpers and
  local-shell tests.
- `shell_exec_hosted_tests.rs` gets the rest of `tests` (the `host?` and
  shell-exit tests), as module `hosted`. It is a **child of `tests`**,
  declared inside `shell_exec_tests.rs`.
- `shell_exec_connect_tests.rs` gets module `connect_tests`, whole.

Each file is under the cap. Same 54 tests (52 run, 2 ignored), same pass
count. The second-half tests now run as `…::shell_exec::tests::hosted::…`.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md` — decisions 5, 6, 7.

## Spec

1. **Run this script as written, once, with `bash`.** It does the whole
   change: the three test files, the two declarations in `shell_exec.rs`,
   the `mod hosted;` declaration at the end of `shell_exec_tests.rs`, the
   allowlist line, then `cargo fmt` twice. It was dry-run on a scratch
   worktree of `20e97ecd`: clippy and the full suite (2586) green, all 54
   `daemon::executor::shell_exec::` tests present. Do not hand-edit
   anything it touches. Do not dry-run it again in a worktree of your own.
   Do not `sed -i` a source file.

   **Why `hosted` is a child of `tests`, not a sibling.** The second half
   uses a lot of the first half's privates: six helpers (`test_home`,
   `run_in_shell_for_test`, `run_in_shell_for_test_on_host`,
   `answered_transport`, `run_with_transport`, `spawn_with_cast`), the
   consts `ROWS`, `COLS`, `PROMPTING`, `SILENT` and `CREDENTIAL`, and the
   `RecordingWriter` fixture. A child module sees all of them, and every
   import of `tests`, through its single `use super::*;`. So no test line
   changes and nothing gains `pub(super)`. `connect_tests` already imports
   what it needs from `super::tests` and moves unchanged.

   ```bash
   D=src/daemon/executor; F=$D/shell_exec.rs
   T1=$(grep -n '^mod tests {$' $F | cut -d: -f1)
   H=$(grep -n '^    // ── run_terminal_command { host? }' $F | cut -d: -f1)
   T2=$(grep -n '^mod connect_tests {$' $F | cut -d: -f1)
   L=$(wc -l < $F)
   echo "T1=$T1 H=$H T2=$T2 L=$L"          # expect 778 2166 3400 3868
   { sed -n "$((T1+1)),$((H-1))p" $F
     printf '    #[path = "shell_exec_hosted_tests.rs"]\n    mod hosted;\n'; } > $D/shell_exec_tests.rs
   { printf 'use super::*;\n\n'; sed -n "${H},$((T2-4))p" $F; } > $D/shell_exec_hosted_tests.rs
   sed -n "$((T2+1)),$((L-1))p" $F > $D/shell_exec_connect_tests.rs
   { head -n $((T1-2)) $F
     printf '#[cfg(test)]\n#[path = "shell_exec_tests.rs"]\nmod tests;\n#[cfg(test)]\n#[path = "shell_exec_connect_tests.rs"]\nmod connect_tests;\n'; } > $F.tmp && mv $F.tmp $F
   A=tests/hygiene_allowlist.txt
   grep -v '^test src/daemon/executor/shell_exec.rs ' $A > $A.tmp && mv $A.tmp $A
   cargo fmt --all
   cargo fmt --all   # twice: a first pass over a dedented file can leave a stray blank line
   wc -l $F $D/shell_exec_tests.rs $D/shell_exec_hosted_tests.rs $D/shell_exec_connect_tests.rs
   ```
2. **Log it at once.** Append a `### Update — <date> (cuts done)` entry
   to this doc's Update Log with the patch or write tool, quoting the
   script's output. The stall governor counts only edit-tool calls as
   progress (rexyMCP issue #14).
3. **Findings** (decision 7), scoped for a pure test move: **search** the
   three files, do not read every test body, and record each hit or
   `none`:
   - a test with no `assert`;
   - a `sleep` or other wall-clock wait;
   - a fixture built inline in three or more tests.
4. **Run the End-to-end block and paste its output into the Update Log
   with the write or patch tool**, as a `### Update — <date> (end-to-end
   verification)` entry holding `/tmp/e2e-m31-20.txt` in a fenced block.
   The block is a `bash` heredoc, so run it as it stands. Check with
   `grep -c '^body_identical=1$'` on this doc before you commit; it must
   print 1. **Known flake:** the full suite occasionally reports
   `passed=2498 failed=1`. A re-run is clean. If the block shows exactly
   that, run it once more and paste both outputs with a line saying so.
5. Commit as `refactor:`.

## Acceptance criteria

- [ ] The three test files exist. `shell_exec.rs` has no inline test
      module, only the two `#[cfg(test)] #[path]` declarations.
- [ ] The allowlist has no `test src/daemon/executor/shell_exec.rs` line,
      and `cargo test --test hygiene` passes (each file under 2000 lines).
- [ ] The End-to-end block prints `body_identical=1`,
      `module_tests=52 passed, 2 ignored` and `passed=2586 failed=0`.
- [ ] The Update Log has a `(cuts done)` entry, a `### Findings — <date>`
      entry and an `(end-to-end verification)` entry containing the
      block's output. An edit tool wrote all three, and all three are in
      this file.

## End-to-end verification

Paste the output byte-exact into the Update Log (Spec 4).

```sh
bash <<'E2E'
B=20e97ecd; D=src/daemon/executor; F=$D/shell_exec.rs
A=/tmp/e2e-m31-20.txt; : > "$A"
wc -l $F $D/shell_exec_tests.rs $D/shell_exec_hosted_tests.rs $D/shell_exec_connect_tests.rs | sed 's/^ *//' >> "$A"
grep -c '^test src/daemon/executor/shell_exec.rs ' tests/hygiene_allowlist.txt | sed 's/^/allowlisted=/' >> "$A"
old=$(git show $B:$F | sed -n '779,3396p;3401,3867p' | tr -d ' \t\n' | md5sum)
new=$( { cat $D/shell_exec_tests.rs | grep -vE '^ *(#\[path = "shell_exec_hosted_tests.rs"\]|mod hosted;)$'; tail -n +2 $D/shell_exec_hosted_tests.rs; cat $D/shell_exec_connect_tests.rs; } | tr -d ' \t\n' | md5sum)
if [ "$old" = "$new" ]; then echo body_identical=1; else echo body_identical=0; fi >> "$A"
cargo test --lib daemon::executor::shell_exec:: 2>&1 | grep -E '^test result' | awk '{print "module_tests=" $4 " passed, " $8 " ignored"}' >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
E2E
```

Expected (from the dry run): `782 …shell_exec.rs`,
`1386 …shell_exec_tests.rs`, `1232 …shell_exec_hosted_tests.rs`,
`467 …shell_exec_connect_tests.rs`, `allowlisted=0`, `body_identical=1`,
`module_tests=52 passed, 2 ignored`, hygiene `ok.`, clippy clean,
`fmt_exit=0`, `passed=2586 failed=0`.

## Authorizations

- Edit `src/daemon/executor/shell_exec.rs`; create the three test files
  beside it. Edit `tests/hygiene_allowlist.txt` through the script only.

## Out of scope

- Any change to a test body, a helper's visibility, or `shell_exec.rs`'s
  production code.

## Update Log

<!-- entries appended below this line -->

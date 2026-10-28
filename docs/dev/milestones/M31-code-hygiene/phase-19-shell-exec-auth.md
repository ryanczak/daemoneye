# Phase 19 — Move credentials and connecting out of `shell_exec.rs`

**Status:** todo
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-28

## Goal

`src/daemon/executor/shell_exec.rs` (1129 production lines) drops under the
800-line cap by moving one contiguous section into a new
`src/daemon/executor/shell_exec_auth.rs`, declared as a child module of
`shell_exec`. The section runs from `SudoAttempts` through
`run_sudo_command`: the sudo retry bookkeeping, the credential prompt round
trip, the cold-host connect step (`drive_connect`, `ensure_connected`) and
the sudo run. `shell_exec` re-exports `ensure_connected`, so the three
executor files that call `shell_exec::ensure_connected` are unchanged. The
file's two inline test modules (3091 lines) are a later phase.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md` — decisions 5, 6, 7.

## Spec

1. **Run this script as written, once, with `bash`.** It does the whole
   change: the cut, the wiring, the allowlist line and the CLAUDE.md row,
   then `cargo fmt`. It was dry-run on a scratch worktree of `b808a710`:
   clippy, fmt and the full suite (2586) green, hygiene included. Do not
   hand-edit anything it touches. Do not dry-run it again in a worktree of
   your own. Do not `sed -i` a source file.

   Two lines change inside the moved code, and the move requires both:
   - `ensure_connected` becomes `pub(in super::super)`: the executor
     module, its exact old reach. A child's `pub(super)` reaches only
     `shell_exec`, too narrow for the re-export.
   - `run_sudo_command` becomes `pub(super)`, because the parent's
     `run_in_shell_core` calls it.

   The wiring has two parts that look odd but are each forced.
   - **`use auth::*;`, not a named or `#[cfg(test)]` import.** It brings
     in `run_sudo_command` and, for the tests, `drive_connect`. A separate
     `#[cfg(test)] use auth::drive_connect;` counts as test lines and grows
     the file's allowlisted `test … 3091` entry, which the hygiene gate
     forbids. The first dry run hit exactly that.
   - **`use super::USER_PROMPT_TIMEOUT;`.** The moved code reaches that
     executor-level const as `super::USER_PROMPT_TIMEOUT`. From the child,
     `super` is `shell_exec`, so the parent imports it and the path resolves
     unchanged. **Do not rewrite it to `super::super::…`.**

   ```bash
   D=src/daemon/executor; F=$D/shell_exec.rs; N=$D/shell_exec_auth.rs
   S=$(grep -n '^/// Prompt round-trip bookkeeping for one sudo run' $F | cut -d: -f1)
   E=$(( $(grep -n '^/// Record one completed command in the shell' $F | cut -d: -f1) - 2 ))
   echo "S=$S E=$E"                      # expect 655 1013
   { printf '//! Credentials and connecting: the sudo retry bookkeeping, the credential\n//! prompt round trip, the cold-host connect step (`drive_connect`,\n//! `ensure_connected`) and `run_sudo_command`.\n\nuse super::*;\n\n'
     sed -n "${S},${E}p" $F | sed -e 's/^pub(super) async fn ensure_connected</pub(in super::super) async fn ensure_connected</' \
                                  -e 's/^async fn run_sudo_command</pub(super) async fn run_sudo_command</'; } > $N
   { head -n $((S-1)) $F
     printf '#[path = "shell_exec_auth.rs"]\nmod auth;\n// The glob brings in `run_sudo_command` and, for the tests, `drive_connect`.\nuse auth::*;\npub(super) use auth::ensure_connected;\n// `auth` reaches this as `super::USER_PROMPT_TIMEOUT`, through this import.\nuse super::USER_PROMPT_TIMEOUT;\n\n'
     tail -n +$((E+2)) $F; } > $F.tmp && mv $F.tmp $F
   A=tests/hygiene_allowlist.txt
   grep -v '^file src/daemon/executor/shell_exec.rs ' $A > $A.tmp && mv $A.tmp $A
   awk '{print} /^\| `src\/daemon\/executor\/shell_exec.rs` \|/ {print "| `src/daemon/executor/shell_exec_auth.rs` | Credentials and connecting for `run_terminal_command`: the sudo retry bookkeeping, the credential prompt round trip, the cold-host connect step (`drive_connect`, `ensure_connected`) and `run_sudo_command`; a child module of `src/daemon/executor/shell_exec.rs`, which re-exports `ensure_connected` |"}' CLAUDE.md > CLAUDE.md.tmp && mv CLAUDE.md.tmp CLAUDE.md
   cargo fmt --all
   wc -l $F $N
   ```
2. **Log it at once.** Append a `### Update — <date> (cuts done)` entry
   to this doc's Update Log with the patch or write tool, quoting the
   script's output. The stall governor counts only edit-tool calls as
   progress (rexyMCP issue #14).
3. **Findings** (decision 7): read `shell_exec_auth.rs` once and record
   what you see, or `none`.
4. **Run the End-to-end block and paste its output into the Update Log
   with the write or patch tool**, as a `### Update — <date> (end-to-end
   verification)` entry holding `/tmp/e2e-m31-19.txt` in a fenced block.
   The block is a `bash` heredoc, so run it as it stands. Check with
   `grep -c '^moved_identical=1$'` on this doc before you commit; it must
   print 1. **Known flake:** the full suite occasionally reports
   `passed=2498 failed=1`, once in phase 05 and once in this phase's
   validation run. A re-run is clean. If the block shows exactly that, run
   it once more, and paste both outputs with a line saying so. Any other
   failure is real.
5. Commit as `refactor:`.

## Acceptance criteria

- [ ] `shell_exec_auth.rs` defines `drive_connect`, `ensure_connected` and
      `run_sudo_command`. `shell_exec.rs` defines none of them and has at
      most 800 production lines (`cargo test --test hygiene` measures it).
- [ ] The allowlist has no `file src/daemon/executor/shell_exec.rs` line.
      Its `test src/daemon/executor/shell_exec.rs 3091` line stays
      unchanged.
- [ ] The End-to-end block prints `lost_other=0`, `added_other=0`,
      `moved_identical=1` and `passed=2586 failed=0`.
- [ ] The Update Log has a `(cuts done)` entry, a `### Findings — <date>`
      entry and an `(end-to-end verification)` entry containing the
      block's output. An edit tool wrote all three, and all three are in
      this file.

## End-to-end verification

Paste the output byte-exact into the Update Log (Spec 4).

```sh
bash <<'E2E'
export LC_ALL=C   # sort and comm disagree under the host locale without it
B=b808a710; D=src/daemon/executor; F=$D/shell_exec.rs; N=$D/shell_exec_auth.rs
A=/tmp/e2e-m31-19.txt; : > "$A"
wc -l $F $N | sed 's/^ *//' >> "$A"
grep -c '^file src/daemon/executor/shell_exec.rs ' tests/hygiene_allowlist.txt | sed 's/^/allowlisted=/' >> "$A"
grep -c '^test src/daemon/executor/shell_exec.rs 3091$' tests/hygiene_allowlist.txt | sed 's/^/test_entry_kept=/' >> "$A"
norm() { tr -d ' \t' | grep -v '^$' | sort; }
git show $B:$F | norm > /tmp/m31-19-old.txt; cat $F $N | norm > /tmp/m31-19-new.txt
echo "lost_other=$(comm -23 /tmp/m31-19-old.txt /tmp/m31-19-new.txt | grep -vcE "^(asyncfnrun_sudo_command<|pub\(super\)asyncfnensure_connected<)")" >> "$A"
echo "added_other=$(comm -13 /tmp/m31-19-old.txt /tmp/m31-19-new.txt | grep -vcE '^(#\[path=|//|modauth;$|useauth::\*;$|pub\(super\)useauth::ensure_connected;$|usesuper::\*;$|usesuper::USER_PROMPT_TIMEOUT;$|pub\(insuper::super\)asyncfnensure_connected<|pub\(super\)asyncfnrun_sudo_command<)')" >> "$A"
if diff -q <(git show $B:$F | sed -n 655,1013p | sed -e 's/^pub(super) async fn ensure_connected</pub(in super::super) async fn ensure_connected</' -e 's/^async fn run_sudo_command</pub(super) async fn run_sudo_command</') <(tail -n +7 $N) >/dev/null; then echo moved_identical=1; else echo moved_identical=0; fi >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
E2E
```

Expected (from the dry run): `3868 …shell_exec.rs`,
`365 …shell_exec_auth.rs`, `allowlisted=0`, `test_entry_kept=1`,
`lost_other=0`, `added_other=0`, `moved_identical=1`, hygiene `ok.`,
clippy clean, `fmt_exit=0`, `passed=2586 failed=0`.

## Authorizations

- Edit `src/daemon/executor/shell_exec.rs`; create `shell_exec_auth.rs`
  beside it. Edit `tests/hygiene_allowlist.txt` and `CLAUDE.md` through the
  script only.

## Out of scope

- Both inline test modules (`tests`, `connect_tests`) and
  `run_in_shell_core` (250 lines, a later function split).
- Any change inside the moved code beyond the two visibility lines,
  including rewriting its `super::` path.

## Update Log

<!-- entries appended below this line -->

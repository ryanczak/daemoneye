# Phase 23 — Move the shell readers and watchers out of `knowledge/shell.rs`

**Status:** todo
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-28

## Goal

`src/daemon/executor/knowledge/shell.rs` holds nine tool families in
2627 production lines. This is the first of two phases that bring it under
the 800-line cap. Four contiguous families move into two child modules of
`shell`, glob re-exported so every `shell::…` path `knowledge/mod.rs` uses
still resolves:
- `read_shell` and the shell-log readers (`list_shell_logs`,
  `read_shell_log`) go to `shell_read.rs`;
- `find_in_shells` and `watch_shell` go to `shell_watch.rs`.

`shell.rs` drops to 1499 production lines, and the allowlist's file entry
ratchets down to that. The next phase moves `send_input`, the screen
reader and the open/close lifecycle. Pure move.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md` — decisions 5, 6, 7.

## Spec

1. **Run this script as written, once, with `bash`.** It does the whole
   change: both cuts, the wiring, both allowlist edits and the CLAUDE.md
   row, then `cargo fmt`. It was dry-run on a scratch worktree of `90400f06`:
   clippy, fmt and the full suite (2586) green, hygiene included. Do not
   hand-edit anything it touches. Do not dry-run it again in a worktree of
   your own. Do not `sed -i` a source file.

   Ten lines change inside the moved code, all `pub(super)`. The parent,
   the sibling child or the tests reach these items:
   - the fns `read_recorded_output`, `read_shell_in`, `find_in_shells_in`,
     `live_shell_ids` and `unknown_shell_error`;
   - the const `READ_SHELL_MAX_LINES`;
   - the struct `ShellOutput` and its three fields, which `find_in_shells`
     reads.

   The items already `pub(super)` (`watch_shell_with`, `read_log_in`, the
   consts) need no change. Only `shell.rs` and its tests use them, and a
   child's `pub(super)` still reaches `shell`.

   **The allowlist ratchets, it is not deleted.** `file …/shell.rs 2627`
   becomes `1499`, the gate's own measurement after the cut, and
   `watch_shell_with 159` moves with its function to `shell_watch.rs`. The
   `test …/shell.rs 4143` line stays.

   ```bash
   D=src/daemon/executor/knowledge; F=$D/shell.rs
   R1=$(( $(grep -n '^// read_shell (M22 phase-03b)$' $F | cut -d: -f1) - 1 ))
   W1=$(( $(grep -n '^// Find in shells (M22 phase-03c)$' $F | cut -d: -f1) - 1 ))
   S1=$(( $(grep -n '^// send_input (M22 phase-05)$' $F | cut -d: -f1) - 1 ))
   echo "R1=$R1 W1=$W1 S1=$S1"            # expect 420 1049 1555
   vis() { sed -e 's/^\(async \)\?fn \(read_recorded_output\|read_shell_in\|find_in_shells_in\|live_shell_ids\|unknown_shell_error\)(/pub(super) \1fn \2(/' \
               -e 's/^const READ_SHELL_MAX_LINES:/pub(super) const READ_SHELL_MAX_LINES:/' \
               -e '/^struct ShellOutput {$/,/^}$/s/^    \([a-z_]*\): /    pub(super) \1: /' \
               -e 's/^struct ShellOutput {$/pub(super) struct ShellOutput {/'; }
   hdr() { printf '//! %s\n\nuse super::*;\n\n' "$1"; }
   { hdr 'The recorded-output readers: `read_shell` and the shell-log readers `list_shell_logs` / `read_shell_log`.'
     sed -n "${R1},$((W1-1))p" $F | vis; } > $D/shell_read.rs
   { hdr 'Searching and waiting on shell output: `find_in_shells` and the blocking observer `watch_shell`.'
     sed -n "${W1},$((S1-1))p" $F | vis; } > $D/shell_watch.rs
   { head -n $((R1-1)) $F
     printf '#[path = "shell_read.rs"]\nmod readers;\npub use readers::*;\n#[path = "shell_watch.rs"]\nmod watchers;\npub use watchers::*;\n\n'
     tail -n +$S1 $F; } > $F.tmp && mv $F.tmp $F
   A=tests/hygiene_allowlist.txt
   sed -e 's|^file src/daemon/executor/knowledge/shell.rs 2627$|file src/daemon/executor/knowledge/shell.rs 1499|' \
       -e 's|^fn src/daemon/executor/knowledge/shell.rs watch_shell_with |fn src/daemon/executor/knowledge/shell_watch.rs watch_shell_with |' $A > $A.tmp && mv $A.tmp $A
   awk '{print} /^\| `src\/daemon\/executor\/shell_exec_auth.rs` \|/ {print "| `src/daemon/executor/knowledge/shell_read.rs`, `shell_watch.rs` | Shell tool families split out of `knowledge/shell.rs` (child modules, glob re-exported): `shell_read.rs` the recorded-output readers (`read_shell`, `list_shell_logs`, `read_shell_log`), `shell_watch.rs` `find_in_shells` and the blocking observer `watch_shell` |"}' CLAUDE.md > CLAUDE.md.tmp && mv CLAUDE.md.tmp CLAUDE.md
   cargo fmt --all
   wc -l $F $D/shell_read.rs $D/shell_watch.rs
   ```
2. **Log it at once.** Append a `### Update — <date> (cuts done)` entry
   to this doc's Update Log with the patch or write tool, quoting the
   script's output. The stall governor counts only edit-tool calls as
   progress (rexyMCP issue #14).
3. **Findings** (decision 7): read the two new files once. For each,
   check these and record every hit, or `none`:
   - a `lock().expect` or `lock().unwrap()` (CLAUDE.md requires
     `.unwrap_or_log()`);
   - any other `expect`, `unwrap` or `panic!` in production code;
   - an error that is swallowed (`let _ =`, `.ok()`, an empty `Err(_) =>`);
   - an obvious duplicate of code elsewhere in `shell`.

   For `watch_shell_with` (159 lines), also name its natural seams.
4. **Run the End-to-end block, then paste its output into the Update Log
   with the write or patch tool**, as a `### Update — <date> (end-to-end
   verification)` entry holding `/tmp/e2e-m31-23.txt` in a fenced block.
   **Paste only output you have already seen.** Write the entry after the
   block has run, and add no other "proof" block. The block is a `bash`
   heredoc, so run it as it stands. Check with `grep -c '^lost_other=0$'`
   on this doc before you commit; it must print 1. **Known flake:**
   `passed=2498 failed=1` on the full suite. If the block shows exactly
   that, run it once more and paste both outputs with a line saying so.
5. Commit as `refactor:`.

## Acceptance criteria

- [ ] `shell_read.rs` defines `read_shell`, `list_shell_logs` and
      `read_shell_log`, and `shell_watch.rs` defines `find_in_shells`,
      `watch_shell` and `watch_shell_with`. `shell.rs` defines none of
      them. `cargo test --test hygiene` passes.
- [ ] The allowlist reads `file src/daemon/executor/knowledge/shell.rs 1499`
      and `fn src/daemon/executor/knowledge/shell_watch.rs watch_shell_with 159`.
      The `test …/shell.rs 4143` line is unchanged.
- [ ] The End-to-end block prints `lost_other=0`, `added_other=0`,
      `read_identical=1`, `watch_identical=1` and `passed=2586 failed=0`.
- [ ] The Update Log has a `(cuts done)` entry, a `### Findings — <date>`
      entry and an `(end-to-end verification)` entry containing the
      block's output. An edit tool wrote all three, and all three are in
      this file.

## End-to-end verification

Paste the output byte-exact into the Update Log (Spec 4).

```sh
bash <<'E2E'
export LC_ALL=C   # sort and comm disagree under the host locale without it
B=90400f06; D=src/daemon/executor/knowledge; F=$D/shell.rs
A=/tmp/e2e-m31-23.txt; : > "$A"
wc -l $F $D/shell_read.rs $D/shell_watch.rs | sed 's/^ *//' >> "$A"
grep -cE '^(file src/daemon/executor/knowledge/shell.rs 1499|fn src/daemon/executor/knowledge/shell_watch.rs watch_shell_with 159|test src/daemon/executor/knowledge/shell.rs 4143)$' tests/hygiene_allowlist.txt | sed 's/^/allowlist_lines=/' >> "$A"
vis() { sed -e 's/^\(async \)\?fn \(read_recorded_output\|read_shell_in\|find_in_shells_in\|live_shell_ids\|unknown_shell_error\)(/pub(super) \1fn \2(/' \
            -e 's/^const READ_SHELL_MAX_LINES:/pub(super) const READ_SHELL_MAX_LINES:/' \
            -e '/^struct ShellOutput {$/,/^}$/s/^    \([a-z_]*\): /    pub(super) \1: /' \
            -e 's/^struct ShellOutput {$/pub(super) struct ShellOutput {/'; }
norm() { tr -d ' \t' | grep -v '^$' | sort; }
git show $B:$F | norm > /tmp/m31-23-old.txt; cat $F $D/shell_read.rs $D/shell_watch.rs | norm > /tmp/m31-23-new.txt
V='(fn(read_recorded_output|read_shell_in|find_in_shells_in|live_shell_ids|unknown_shell_error)\(|constREAD_SHELL_MAX_LINES:|structShellOutput\{|(owner_key|command_count|text):)'
echo "lost_other=$(comm -23 /tmp/m31-23-old.txt /tmp/m31-23-new.txt | grep -vcE "^$V")" >> "$A"
echo "added_other=$(comm -13 /tmp/m31-23-old.txt /tmp/m31-23-new.txt | grep -vcE "^(#\[path=|//|mod(readers|watchers);$|pubuse(readers|watchers)::\*;$|usesuper::\*;$|pub\(super\)$V)")" >> "$A"
if diff -q <(git show $B:$F | sed -n 420,1047p | vis) <(tail -n +5 $D/shell_read.rs) >/dev/null; then echo read_identical=1; else echo read_identical=0; fi >> "$A"
if diff -q <(git show $B:$F | sed -n 1049,1554p | vis) <(tail -n +5 $D/shell_watch.rs) >/dev/null; then echo watch_identical=1; else echo watch_identical=0; fi >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
E2E
```

Expected (from the dry run): `5642 …shell.rs`, `632 …shell_read.rs`,
`510 …shell_watch.rs`, `allowlist_lines=3`, `lost_other=0`,
`added_other=0`, `read_identical=1`, `watch_identical=1`, hygiene `ok.`,
clippy clean, `fmt_exit=0`, `passed=2586 failed=0`.

## Authorizations

- Edit `src/daemon/executor/knowledge/shell.rs`; create `shell_read.rs`
  and `shell_watch.rs` beside it. Edit `tests/hygiene_allowlist.txt` and
  `CLAUDE.md` through the script only.

## Out of scope

- `list_shells`, the context blocks, `send_input`, the screen reader, the
  open/close lifecycle, and `shell.rs`'s 4143 test lines (later phases).
- Any change inside the moved code beyond the ten `pub(super)` lines.

## Update Log

<!-- entries appended below this line -->

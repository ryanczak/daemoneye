# Phase 12 — Move the `delete` and `copy` operations out of `ops.rs`

**Status:** todo
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-27

## Goal

`src/daemon/executor/file_ops/ops.rs` (1018 production lines) drops under
the 800-line cap. Its `operation = "delete"` section moves into a new child
module `ops_delete.rs` and its `operation = "copy"` section into
`ops_copy.rs`, both declared from `ops.rs`. `ops` re-exports `run_delete`
and `run_copy`, so `write.rs`'s `super::ops::run_delete` / `run_copy` calls
still resolve. `edit` and `create`, the shared spawn plumbing and
`ops.rs`'s `mod tests` stay where they are.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md` — decisions 5, 6, 7.

## Spec

1. **Run this script as written, once, with `bash`.** It does the whole
   code change: both cuts, the wiring, the allowlist and the CLAUDE.md
   row. It was dry-run on a scratch worktree of `50f23de5`: fmt, clippy and
   the full suite (2586) green. Do not hand-edit anything it touches. Do
   not dry-run it again in a worktree of your own. Do not `sed -i` a
   source file: the script rewrites through temp files and `mv`, and its
   `sed` calls are filters in pipelines.

   Only two kinds of line change inside the moved code, and the move
   requires both:
   - **Paths.** The moved code reaches `file_ops`-level items as
     `super::write::…`, `super::sq_escape` and `super::run_in_host_shell`.
     One module deeper, those become `super::super::…`.
   - **Visibility.** `run_delete` and `run_copy` keep their exact reach,
     the `file_ops` module, by becoming `pub(in super::super)`. A child's
     `pub(super)` reaches only `ops`, and `ops` cannot re-export it any
     wider. The `_with_spawn` variants keep `pub(super)`. Only `ops.rs`'s
     tests call them, so `ops` imports them under `#[cfg(test)]`.

   ```bash
   D=src/daemon/executor/file_ops; F=$D/ops.rs
   D0=$(grep -n '^// operation = "delete"$' $F | cut -d: -f1)
   C0=$(grep -n '^// operation = "copy"$' $F | cut -d: -f1)
   U0=$(grep -n '^// Unit tests$' $F | cut -d: -f1)
   DS=$((D0-1)); CS=$((C0-1)); CE=$((U0-3))
   echo "DS=$DS CS=$CS CE=$CE"                    # expect 606 777 1013
   hdr() { printf '//! File operation `%s`: approval prompt, then the local or remote run.\n\nuse super::*;\n\n' "$1"; }
   fix() { sed -e 's/^pub(super) async fn run_\(copy\|delete\)</pub(in super::super) async fn run_\1</' \
               -e 's/\bsuper::\(write::\|sq_escape\|run_in_host_shell\)/super::super::\1/g'; }
   { hdr delete; sed -n "${DS},$((CS-2))p" $F | fix; } > $D/ops_delete.rs
   { hdr copy;   sed -n "${CS},${CE}p" $F     | fix; } > $D/ops_copy.rs
   { head -n $((DS-1)) $F
     printf '#[path = "ops_delete.rs"]\nmod delete;\n#[path = "ops_copy.rs"]\nmod copy;\npub(super) use copy::run_copy;\n#[cfg(test)]\nuse copy::run_copy_with_spawn;\npub(super) use delete::run_delete;\n#[cfg(test)]\nuse delete::run_delete_with_spawn;\n\n'
     tail -n +$((CE+2)) $F; } > $F.tmp && mv $F.tmp $F
   A=tests/hygiene_allowlist.txt
   grep -v '^file src/daemon/executor/file_ops/ops.rs ' $A \
     | sed 's|^fn src/daemon/executor/file_ops/ops.rs run_copy_with_spawn |fn src/daemon/executor/file_ops/ops_copy.rs run_copy_with_spawn |' > $A.tmp && mv $A.tmp $A
   awk '{print} /^\| `src\/daemon\/executor\/shell_exec.rs` \|/ {print "| `src/daemon/executor/file_ops/` | `edit_file` and `read_file`: `write.rs` dispatches by operation, `ops.rs` runs `edit` and `create`, its child modules `ops_delete.rs` and `ops_copy.rs` run `delete` and `copy` (re-exported from `ops`), `read.rs` the paginated read |"}' CLAUDE.md > CLAUDE.md.tmp && mv CLAUDE.md.tmp CLAUDE.md
   cargo fmt --all
   wc -l $F $D/ops_delete.rs $D/ops_copy.rs
   ```
2. **Log it at once.** Append a `### Update — <date> (cuts done)` entry
   to this doc's Update Log with the patch or write tool, quoting the
   script's two output lines (`DS=… CS=… CE=…` and the `wc -l` counts).
   The stall governor counts only edit-tool calls as progress (rexyMCP
   issue #14).
3. **Findings** (decision 7): read the two new files once and record what
   you see, or `none`. `run_copy_with_spawn` is 213 lines, a later split:
   note its natural seams.
4. **Run the End-to-end block and paste its output into the Update Log
   with the write or patch tool**, as a `### Update — <date> (end-to-end
   verification)` entry holding `/tmp/e2e-m31-12.txt` in a fenced block.
   Check with `grep -c '^lost_other=0$'` on this doc before you commit; it
   must print 1.
5. Commit the code as `refactor:` (a move is not a `feat:`).

## Acceptance criteria

- [ ] `ops_delete.rs` defines `run_delete` and `run_delete_with_spawn`, and
      `ops_copy.rs` defines `run_copy` and `run_copy_with_spawn`. `ops.rs`
      defines none of the four and has at most 800 production lines
      (`cargo test --test hygiene` measures it).
- [ ] The allowlist has no `file src/daemon/executor/file_ops/ops.rs` line,
      and its `run_copy_with_spawn 213` line names `ops_copy.rs`.
- [ ] The End-to-end block prints `lost_other=0`, `added_other=0`,
      `rewrites=11/11` and `passed=2586 failed=0`.
- [ ] The Update Log has a `(cuts done)` entry, a `### Findings — <date>`
      entry and an `(end-to-end verification)` entry containing the
      block's output. An edit tool wrote all three, and all three are in
      this file.

## End-to-end verification

Run with `bash`. Paste the output byte-exact into the Update Log (Spec 4).

```sh
export LC_ALL=C   # sort and comm disagree under the host locale without it
B=50f23de5; D=src/daemon/executor/file_ops; F=$D/ops.rs
A=/tmp/e2e-m31-12.txt; : > "$A"
wc -l $F $D/ops_delete.rs $D/ops_copy.rs | sed 's/^ *//' >> "$A"
grep -c '^file src/daemon/executor/file_ops/ops.rs ' tests/hygiene_allowlist.txt | sed 's/^/allowlisted=/' >> "$A"
grep -c '^fn src/daemon/executor/file_ops/ops_copy.rs run_copy_with_spawn 213$' tests/hygiene_allowlist.txt | sed 's/^/fn_moved=/' >> "$A"
norm() { tr -d ' \t' | grep -v '^$' | sort; }
git show $B:$F | norm > /tmp/m31-12-old.txt; cat $F $D/ops_delete.rs $D/ops_copy.rs | norm > /tmp/m31-12-new.txt
comm -23 /tmp/m31-12-old.txt /tmp/m31-12-new.txt > /tmp/m31-12-lost.txt
comm -13 /tmp/m31-12-old.txt /tmp/m31-12-new.txt > /tmp/m31-12-added.txt
R='super::(write::|sq_escape|run_in_host_shell)|^pub\(super\)asyncfnrun_(copy|delete)<'
echo "lost_other=$(grep -vcE "$R" /tmp/m31-12-lost.txt)" >> "$A"
echo "added_other=$(grep -vcE 'super::super::|^pub\(insuper::super\)asyncfnrun_(copy|delete)<|^#\[cfg\(test\)\]$|^#\[path=|^//!|^mod(copy|delete);$|^usesuper::\*;$|^(pub\(super\))?use(copy|delete)::run_(copy|delete)(_with_spawn)?;$' /tmp/m31-12-added.txt)" >> "$A"
echo "rewrites=$(grep -cE "$R" /tmp/m31-12-lost.txt)/$(grep -cE 'super::super::|^pub\(insuper::super\)' /tmp/m31-12-added.txt)" >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Expected (from the dry run): `2270 …ops.rs`, `174 …ops_delete.rs`,
`241 …ops_copy.rs`, `allowlisted=0`, `fn_moved=1`, `lost_other=0`,
`added_other=0`, `rewrites=11/11`, hygiene `ok.`, clippy clean,
`fmt_exit=0`, `passed=2586 failed=0`.

## Authorizations

- Edit `src/daemon/executor/file_ops/ops.rs`; create `ops_delete.rs` and
  `ops_copy.rs` beside it. Edit `tests/hygiene_allowlist.txt` and
  `CLAUDE.md`, through the script only.

## Out of scope

- `edit`, `create`, the spawn helpers and `ops.rs`'s `mod tests` (1250
  lines, under the 2000 cap).
- Any change inside the moved functions beyond the paths and visibility
  above, including splitting `run_copy_with_spawn`.

## Update Log

<!-- entries appended below this line -->

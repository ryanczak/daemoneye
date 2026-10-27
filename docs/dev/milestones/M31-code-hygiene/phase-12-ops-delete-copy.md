# Phase 12 — Move the `delete` and `copy` operations out of `ops.rs`

**Status:** in-progress
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

### Update — 2026-10-26 20:36 (cuts done)

Executor run (session 2026-10-27). Spec 1 ran as written, once, with
`bash`: the block computed `DS=606 CS=777 CE=1013` (matching the expected
606 777 1013), cut lines 606–775 of `src/daemon/executor/file_ops/ops.rs`
into a new `ops_delete.rs` and lines 777–1013 into a new `ops_copy.rs`,
each with the module doc header and `use super::*;` and both rewrites
through the `fix` path/visibility filter; the deletions in `ops.rs` went
through a temp file + `mv`, never `sed -i` on a source file. The
allowlist edit and the CLAUDE.md row landed through the script's own
grep/awk rewrites. No hand edits to anything the script touches. Output:

```
DS=606 CS=777 CE=1013
  2270 src/daemon/executor/file_ops/ops.rs
   174 src/daemon/executor/file_ops/ops_delete.rs
   241 src/daemon/executor/file_ops/ops_copy.rs
  2685 total
```

Line counts match the dry run (2270/174/241). The phase doc's Status
flipped to `in-progress` and the README phase-table row to match before
the run; this entry is written with the patch tool, per Spec 2.

### Findings — 2026-10-26 20:40

Read of the two new files in full (`ops_delete.rs` 174 lines,
`ops_copy.rs` 241 lines), decision 7. The moved code is the same code
that lived in `ops.rs`; findings are about what it does, not the move:

1. `ops_copy.rs` · `run_copy_with_spawn` — smell, minor. 213 lines doing
   four jobs in one body: argument validation, the remote-host branch
   (shell string built *before* the approval prompt is sent), the local
   pre-flight (canonicalize, config-dir block, dest-exists check, read)
   and the local commit (mkdir, tmp write, rename). Natural seams for a
   later split: a `run_copy_remote(...)` and `run_copy_local(...)` pair
   split on `host`, each taking the already-approved prompt outcome —
   the remote branch's shell-string construction and the local branch's
   pre-flight are each independently testable units. The `delete`
   sibling is the same shape at 137 lines and would take the same two
   seams if the cap ever reaches it.
2. `ops_copy.rs` · `run_copy_with_spawn` remote branch — smell, minor.
   The remote shell string is built (lines 61–69) *before* the approval
   prompt (74–88), so an unapproved copy already constructs the command;
   harmless today (nothing runs until `await_edit_file_response`
   returns), but building after approval would make the ordering
   intent explicit.
3. `ops_copy.rs` · dest validation vs. remote branch — bug, minor. The
   `dest.contains("..")` / absolute-path check (52–57) runs for both
   hosts, and the *remote* branch then trusts the validated string, but
   the *local* branch re-derives `dest_std` from `dest` and checks
   `dest_std.exists()` — an existing destination is rejected locally
   (94–98) while the remote branch rejects it inside the shell
   (`elif [ -e ... ]`). Same rule, two enforcements, two message
   formats; the local one surfaces before approval, the remote one
   after. Not a defect — both reject — but a divergence worth knowing
   about if the messages ever get asserted on.
4. `ops_delete.rs` · remote branch sentinel scan — smell, nit. The
   result scan walks `snap.lines().rev()` looking for `DE_OK:` /
   `DE_ERROR:`, so a file whose *name* contains `DE_ERROR:` (path under
   a directory so named) would be misread as a failure after a
   successful delete. `__DE_DONE__` is echoed but never checked. The
   `copy` variant shares the pattern with the same nit.
5. `ops_delete.rs` / `ops_copy.rs` · local branch line counts — smell,
   nit. `existing.lines().count()` / `source_content.lines().count()`
   read the whole file into memory solely to print "N line(s)" in the
   result; for the copy case the content is needed anyway, but delete
   reads it only for the count and the prompt. Fine at these sizes.
6. `ops_copy.rs` · `tmp_path = dest_std.with_extension("de_tmp")` —
   smell, nit. `with_extension` replaces an existing extension, so
   `a.txt` → `a.de_tmp` while `a` → `a.de_tmp` too — collisions between
   two concurrent copies of differently-named files are unlikely but
   the tmp name is not unique per invocation (no pid/nonce). A failed
   rename cleans up, so no residue today.

None acted on; the phase stays a move.

### Update — 2026-10-26 20:40 (end-to-end verification)

Ran the phase doc's End-to-end block with `bash` (LC_ALL=C, base
commit `50f23de5`); output captured mechanically to
`/tmp/e2e-m31-12.txt` and pasted below byte-exact:

```text
2270 src/daemon/executor/file_ops/ops.rs
174 src/daemon/executor/file_ops/ops_delete.rs
241 src/daemon/executor/file_ops/ops_copy.rs
2685 total
allowlisted=0
fn_moved=1
lost_other=0
added_other=0
rewrites=11/11
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2586 failed=0
```

Every expected line matches the dry run: `2270/174/241` line counts,
`allowlisted=0` (the `file src/daemon/executor/file_ops/ops.rs 1018`
allowlist line is deleted), `fn_moved=1` (the
`run_copy_with_spawn 213` line now names `ops_copy.rs`), `lost_other=0`
(nothing from the old file is gone except the path/visibility rewrites
the block's grep names), `added_other=0` (nothing new beyond the module
headers, `use super::*;` and the re-export stanzas the grep excludes),
`rewrites=11/11` (all eleven `super::` reach-ins and both visibility
lines rewrote), hygiene `ok. 16 passed`, clippy clean, `fmt_exit=0`,
and the full suite at `passed=2586 failed=0`.

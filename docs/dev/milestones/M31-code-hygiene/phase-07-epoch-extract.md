# Phase 07 — Move memory extraction out of `context/epochs.rs`

**Status:** todo
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-26

## Goal

`src/daemon/context/epochs.rs` (840 production lines) drops under the
800-line cap by moving its last production section, `extract_memories_from_epoch`,
`apply_extraction` and `ExtractedFact`, into a new
`src/daemon/context/epoch_extract.rs`, declared as a child module of
`epochs`. Pure move: `epochs::extract_memories_from_epoch` keeps its path
through a re-export, so its caller in `context/background.rs` is untouched.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md` — decisions 5, 6, 7.

## Spec

1. **Cut with the shell — run this as written.** Dry-run on a scratch
   worktree of `e1a648e5`: fmt, clippy and the full suite (2586) green.
   **No `sed -i` on source files** (the executor refuses in-place edits):
   the rewrite goes through a temp file and `mv`.

   ```bash
   F=src/daemon/context/epochs.rs; N=src/daemon/context/epoch_extract.rs
   S=$(awk '{l[NR]=$0} /^pub async fn extract_memories_from_epoch\(/ {s=NR-1; while (s>0 && (l[s] ~ /^\/\/\// || l[s] ~ /^#\[/)) s--; print s+1; exit}' $F)
   T=$(grep -n '^#\[cfg(test)\]' $F | tail -1 | cut -d: -f1)
   echo "S=$S T=$T"                      # expect 747 841
   { printf '//! Memory extraction from a finished epoch: `extract_memories_from_epoch`.\n\nuse super::*;\n\n'
     sed -n "${S},$((T-2))p" $F; } > $N
   { head -n $((S-1)) $F
     printf '#[path = "epoch_extract.rs"]\nmod extract;\npub use extract::extract_memories_from_epoch;\n#[cfg(test)]\nuse extract::apply_extraction;\n\n'
     tail -n +$T $F; } > $F.tmp && mv $F.tmp $F
   ```

   The five lines it writes into `epochs.rs` declare the child module, keep
   the public path, and give `epochs.rs`'s own `mod tests` the
   `apply_extraction` its four tests call.
2. **Log the cut at once.** Append a `### Update — <date> (cuts done)`
   entry to this doc's Update Log **with the patch or write tool**. The
   stall governor counts only edit-tool calls as progress (rexyMCP issue
   #14).
3. **One edit, with the patch tool:** in `epoch_extract.rs`,
   `fn apply_extraction(` becomes `pub(super) fn apply_extraction(`.
4. `cargo fmt --all`. No import pruning is needed: the new file uses only
   `use super::*;`, and `epochs.rs` keeps every import it had.
5. **Allowlist:** delete `file src/daemon/context/epochs.rs 840`.
6. **CLAUDE.md** "Key files": add a row for
   `src/daemon/context/epoch_extract.rs`: `extract_memories_from_epoch()`,
   best-effort memory extraction from a finished epoch, a child module of
   `context/epochs.rs` re-exported from it.
7. **Findings** (decision 7): read the moved section once and record what
   you see, or `none`.

**Gotcha.** `#[path]` on a `mod` declared inside `epochs.rs` resolves
relative to `epochs.rs`'s directory, so the bare file name is right. Do not
move the file into an `epochs/` directory.

## Acceptance criteria

- [ ] `src/daemon/context/epoch_extract.rs` holds
      `pub async fn extract_memories_from_epoch(`, `fn apply_extraction(` and
      `struct ExtractedFact`; `epochs.rs` defines none of them.
- [ ] `grep -c '^file src/daemon/context/epochs.rs ' tests/hygiene_allowlist.txt`
      is 0, and `cargo test --test hygiene` passes.
- [ ] The End-to-end block prints `lost_other=0` and `added_other=0`.
- [ ] Preservation: `cargo test` passes with exactly 2586 passing tests.
- [ ] The Update Log has a `(cuts done)` entry written by an edit tool and a
      `### Findings — <date>` entry.

## End-to-end verification

Run with `bash`. Paste the output byte-exact into the Update Log.

```sh
export LC_ALL=C   # sort and comm disagree under the host locale without it
B=e1a648e5; F=src/daemon/context/epochs.rs; N=src/daemon/context/epoch_extract.rs
A=/tmp/e2e-m31-07.txt; : > "$A"
wc -l $F $N | sed 's/^ *//' >> "$A"
echo "allowlisted=$(grep -c '^file src/daemon/context/epochs.rs ' tests/hygiene_allowlist.txt)" >> "$A"
norm() { tr -d ' \t' | grep -v '^$' | sort; }
git show $B:$F | norm > /tmp/m31-07-old.txt; cat $F $N | norm > /tmp/m31-07-new.txt
echo "lost_other=$(comm -23 /tmp/m31-07-old.txt /tmp/m31-07-new.txt | grep -vE '^fnapply_extraction\(' | wc -l)" >> "$A"
echo "added_other=$(comm -13 /tmp/m31-07-old.txt /tmp/m31-07-new.txt | grep -vE '^(use|pubuse|//!|pub\(super\)|#\[cfg\(test\)\]|#\[path=|modextract;)' | wc -l)" >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Expected (from the dry run): `1799 …epochs.rs`, `97 …epoch_extract.rs`,
`allowlisted=0`, `lost_other=0`, `added_other=0`, hygiene `ok.`, clippy
clean, `fmt_exit=0`, `passed=2586 failed=0`.

## Authorizations

- Edit `src/daemon/context/epochs.rs`; create
  `src/daemon/context/epoch_extract.rs`. Edit `tests/hygiene_allowlist.txt`
  (the one deletion) and `CLAUDE.md` (one row).

## Out of scope

- The rest of `epochs.rs`, and `epochs.rs`'s `mod tests` (1046 lines, under
  the 2000-line test cap).
- Any change inside the moved functions, even one a finding names.

## Update Log

<!-- entries appended below this line -->

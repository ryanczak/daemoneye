# Phase 15 — Split the memory index's search and rebuild out of `mod.rs`

**Status:** todo
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-27

## Goal

`src/memory/index/mod.rs` (1135 production lines) drops under the 800-line
cap. Two of its sections move into sibling files in `src/memory/index/`:
- the queries (`fts5_search_in_category` through `search_epochs`, with
  their `*Hit` structs) to `search.rs`;
- the rebuild from disk (`Corpus` through `reconcile_index`) to
  `reconcile.rs`.

Both are child modules of `index` and glob re-exported from it, so every
`crate::memory::index::…` path still resolves. `mod.rs` keeps the schema,
`build_match_expr`, the write path (`index_*`, `remove_*`) and its inline
`mod tests`. Pure move.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md` — decisions 5, 6, 7.

## Spec

1. **Run this script as written, once, with `bash`.** It does the whole
   code change: both cuts, the wiring, the allowlist line and the
   CLAUDE.md row, then `cargo fmt`. It was dry-run on a scratch worktree
   of `eea2bb93`: clippy and the full suite (2586) green. Do not hand-edit
   anything it touches. Do not dry-run it again in a worktree of your own.
   Do not `sed -i` a source file: the script rewrites through temp files
   and `mv`.

   Two lines change inside the moved code. `Corpus::from_table` and
   `Corpus::table_name` gain `pub(super)`, because the parent's
   `open_and_reconcile_if_empty` and its tests call them. The rebuild code
   reaches `memory` items as `super::memory_dir_for_namespace` and
   `super::parse_memory_frontmatter`. From the child, `super` is `index`,
   so the script imports those two items into `mod.rs`, and the paths
   resolve unchanged. **Do not rewrite them to `super::super::…`.**

   ```bash
   D=src/memory/index; F=$D/mod.rs
   S1=$(grep -n '^/// As \[`fts5_search`\], but restricted' $F | cut -d: -f1)
   E1=$(( $(grep -n '^/// Scan one archive file and insert' $F | cut -d: -f1) - 2 ))
   S2=$(grep -n '^/// Which corpus a table belongs to\.' $F | cut -d: -f1)
   E2=$(( $(grep -n '^#\[cfg(test)\]' $F | tail -1 | cut -d: -f1) - 2 ))
   echo "S1=$S1 E1=$E1 S2=$S2 E2=$E2"     # expect 212 560 843 1196
   { printf '//! Querying the index: BM25-ranked search over memories, turns, artifacts,\n//! events and epochs.\n\nuse super::*;\n\n'
     sed -n "${S1},${E1}p" $F; } > $D/search.rs
   { printf '//! Rebuilding the index from the files on disk: `reconcile_corpus` for one\n//! corpus, `reconcile_index` for all of them.\n\nuse super::*;\n\n'
     sed -n "${S2},${E2}p" $F | sed -e 's/^    fn from_table(/    pub(super) fn from_table(/' -e 's/^    fn table_name(/    pub(super) fn table_name(/'; } > $D/reconcile.rs
   { head -n $((S1-1)) $F
     printf 'mod reconcile;\nmod search;\npub use reconcile::*;\npub use search::*;\n// `reconcile` reaches these memory items as `super::…`, through this import.\nuse super::{memory_dir_for_namespace, parse_memory_frontmatter};\n\n'
     sed -n "$((E1+2)),$((S2-1))p" $F
     tail -n +$((E2+2)) $F; } > $F.tmp && mv $F.tmp $F
   A=tests/hygiene_allowlist.txt
   grep -v '^file src/memory/index/mod.rs ' $A > $A.tmp && mv $A.tmp $A
   awk '{print} /^\| `src\/memory\/index.rs` \|/ {print "| `src/memory/index/search.rs`, `reconcile.rs` | The index split by concern, both child modules of `src/memory/index/mod.rs` (the schema and the write path) and glob re-exported from it: `search.rs` the BM25 queries (`fts5_search`, `fts5_search_in_category`, `search_turns`, `search_artifacts`, `search_events`, `search_epochs`), `reconcile.rs` the rebuild from disk (`Corpus`, `reconcile_corpus`, `reconcile_index`) |"}' CLAUDE.md > CLAUDE.md.tmp && mv CLAUDE.md.tmp CLAUDE.md
   cargo fmt --all
   wc -l $F $D/search.rs $D/reconcile.rs
   ```
2. **Log it at once.** Append a `### Update — <date> (cuts done)` entry
   to this doc's Update Log with the patch or write tool, quoting the
   script's output (`S1=… E2=…` and the `wc -l` counts). The stall
   governor counts only edit-tool calls as progress (rexyMCP issue #14).
3. **Findings** (decision 7): read `search.rs` and `reconcile.rs` once and
   record what you see, or `none`.
4. **Run the End-to-end block and paste its output into the Update Log
   with the write or patch tool**, as a `### Update — <date> (end-to-end
   verification)` entry holding `/tmp/e2e-m31-15.txt` in a fenced block.
   The block is a `bash` heredoc, so run it as it stands. Check with
   `grep -c '^lost_other=0$'` on this doc before you commit; it must
   print 1.
5. Commit the code as `refactor:`.

## Acceptance criteria

- [ ] `search.rs` defines `fts5_search` and `search_turns`, and
      `reconcile.rs` defines `Corpus`, `reconcile_corpus` and
      `reconcile_index`. `mod.rs` defines none of them and has at most 800
      production lines (`cargo test --test hygiene` measures it).
- [ ] The allowlist has no `file src/memory/index/mod.rs` line. Its
      `test src/memory/index/mod.rs 2605` line stays, because the tests
      do not move.
- [ ] The End-to-end block prints `lost_other=0`, `added_other=0`,
      `search_identical=1`, `reconcile_identical=1` and
      `passed=2586 failed=0`.
- [ ] The Update Log has a `(cuts done)` entry, a `### Findings — <date>`
      entry and an `(end-to-end verification)` entry containing the
      block's output. An edit tool wrote all three, and all three are in
      this file.

## End-to-end verification

Paste the output byte-exact into the Update Log (Spec 4).

```sh
bash <<'E2E'
export LC_ALL=C   # sort and comm disagree under the host locale without it
B=eea2bb93; D=src/memory/index; F=$D/mod.rs
A=/tmp/e2e-m31-15.txt; : > "$A"
wc -l $F $D/search.rs $D/reconcile.rs | sed 's/^ *//' >> "$A"
grep -c '^file src/memory/index/mod.rs ' tests/hygiene_allowlist.txt | sed 's/^/allowlisted=/' >> "$A"
norm() { tr -d ' \t' | grep -v '^$' | sort; }
git show $B:$F | norm > /tmp/m31-15-old.txt; cat $F $D/search.rs $D/reconcile.rs | norm > /tmp/m31-15-new.txt
echo "lost_other=$(comm -23 /tmp/m31-15-old.txt /tmp/m31-15-new.txt | grep -vcE '^fn(from_table|table_name)\(')" >> "$A"
echo "added_other=$(comm -13 /tmp/m31-15-old.txt /tmp/m31-15-new.txt | grep -vcE '^pub\(super\)fn(from_table|table_name)\(|^//|^mod(reconcile|search);$|^pubuse(reconcile|search)::\*;$|^usesuper::\*;$|^usesuper::\{memory_dir_for_namespace,parse_memory_frontmatter\};$')" >> "$A"
if diff -q <(git show $B:$F | sed -n 212,560p) <(tail -n +6 $D/search.rs) >/dev/null; then echo search_identical=1; else echo search_identical=0; fi >> "$A"
if diff -q <(git show $B:$F | sed -n 843,1196p | sed -e 's/^    fn from_table(/    pub(super) fn from_table(/' -e 's/^    fn table_name(/    pub(super) fn table_name(/') <(tail -n +6 $D/reconcile.rs) >/dev/null; then echo reconcile_identical=1; else echo reconcile_identical=0; fi >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
E2E
```

Expected (from the dry run): `3042 …mod.rs`, `354 …search.rs`,
`359 …reconcile.rs`, `allowlisted=0`, `lost_other=0`, `added_other=0`,
`search_identical=1`, `reconcile_identical=1`, hygiene `ok.`, clippy
clean, `fmt_exit=0`, `passed=2586 failed=0`.

## Authorizations

- Edit `src/memory/index/mod.rs`; create `search.rs` and `reconcile.rs`
  beside it. Edit `tests/hygiene_allowlist.txt` and `CLAUDE.md`, through
  the script only.

## Out of scope

- `mod.rs`'s inline `mod tests` (still over the 2000 test cap; a later
  phase moves and splits it), the schema and the write path.
- Any change inside the moved code beyond the two `pub(super)` lines,
  including rewriting its `super::` paths.

## Update Log

<!-- entries appended below this line -->

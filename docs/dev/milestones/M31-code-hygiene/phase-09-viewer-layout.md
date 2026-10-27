# Phase 09 — Move the transcript layout out of `viewer.rs`

**Status:** todo
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-26

## Goal

`src/cli/viewer.rs` (907 production lines) drops under the 800-line cap by
moving its layout section — `layout_blocks` through `row_at`: block layout,
collapsed layout, word wrap, scroll clamping and mouse hit-testing — into a
new `src/cli/viewer_layout.rs`, declared as a child module of `viewer` and
re-exported, so every `crate::cli::viewer::…` path still resolves. Pure
move.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md` — decisions 5, 6, 7.

## Spec

1. **Cut with the shell — run this as written.** Dry-run on a scratch
   worktree of `e57ece85`: fmt, clippy and the full suite (2586) green.
   **No `sed -i` on source files** (the executor refuses in-place edits):
   the rewrite goes through a temp file and `mv`. The one change inside the
   moved code, `wrap_words` gaining `pub(super)` (the inline tests call it),
   is made by the pipeline below.

   ```bash
   F=src/cli/viewer.rs; N=src/cli/viewer_layout.rs
   S=$(grep -n '^/// Lay the transcript out at ' $F | cut -d: -f1)
   E=$(( $(grep -n '^/// What the search feature needs' $F | cut -d: -f1) - 2 ))
   echo "S=$S E=$E"                      # expect 203 456
   { printf '//! Transcript layout: blocks to screen rows, word wrap, scroll clamping and\n//! mouse hit-testing.\n\nuse super::*;\n\n'
     sed -n "${S},${E}p" $F | sed 's/^fn wrap_words(/pub(super) fn wrap_words(/'; } > $N
   { head -n $((S-1)) $F
     printf '#[path = "viewer_layout.rs"]\nmod layout;\n#[cfg(test)]\nuse layout::wrap_words;\npub use layout::{clamp_scroll, layout_blocks, layout_blocks_with, row_at};\n\n'
     tail -n +$((E+2)) $F; } > $F.tmp && mv $F.tmp $F
   ```
2. **Log the cut at once.** Append a `### Update — <date> (cuts done)`
   entry to this doc's Update Log **with the patch or write tool**. The
   stall governor counts only edit-tool calls as progress (rexyMCP issue
   #14).
3. `cargo fmt --all` (a no-op in the dry run). No import pruning: the build
   has no warnings.
4. **Allowlist:** delete the line `file src/cli/viewer.rs 907`. Leave
   `fn src/cli/viewer.rs viewer_loop 234` alone — it did not move.
5. **CLAUDE.md** "Key files": add a row for `src/cli/viewer_layout.rs`:
   transcript layout for the viewer (`layout_blocks`, `layout_blocks_with`,
   `wrap_words`, `clamp_scroll`, `row_at`), a child module of
   `cli/viewer.rs`, re-exported from it.
6. **Findings** (decision 7): read the moved section once and record what
   you see, or `none`.
7. **Run the End-to-end block and paste its output into the Update Log
   with the write or patch tool**, as a `### Update — <date> (end-to-end
   verification)` entry holding the contents of `/tmp/e2e-m31-09.txt` in a
   fenced block. Writing the file to `/tmp` is not enough, and saying so in
   your summary is not enough: phases 07 and 08 both claimed the paste and
   did not make it, and 08 was bounced for it. Check with
   `grep -c '^lost_other=0$'` on this doc before you commit — it must
   print 1.

## Acceptance criteria

- [ ] `src/cli/viewer_layout.rs` defines `layout_blocks`,
      `layout_blocks_with`, `wrap_words`, `clamp_scroll` and `row_at`;
      `viewer.rs` defines none of them and has at most 800 production lines
      (`cargo test --test hygiene` measures it).
- [ ] `grep -c '^file src/cli/viewer.rs ' tests/hygiene_allowlist.txt` is 0,
      `grep -c '^fn src/cli/viewer.rs viewer_loop 234$'` is 1, and
      `cargo test --test hygiene` passes.
- [ ] The End-to-end block prints `lost_other=0` and `added_other=0`.
- [ ] Preservation: `cargo test` passes with exactly 2586 passing tests.
- [ ] The Update Log has a `(cuts done)` entry, a `### Findings — <date>`
      entry and an `(end-to-end verification)` entry containing the block's
      output — all three written by an edit tool, all three in this file.

## End-to-end verification

Run with `bash`. Paste the output byte-exact into the Update Log (Spec 7).

```sh
export LC_ALL=C   # sort and comm disagree under the host locale without it
B=e57ece85; F=src/cli/viewer.rs; N=src/cli/viewer_layout.rs
A=/tmp/e2e-m31-09.txt; : > "$A"
wc -l $F $N | sed 's/^ *//' >> "$A"
echo "allowlisted=$(grep -c '^file src/cli/viewer.rs ' tests/hygiene_allowlist.txt) fn_kept=$(grep -c '^fn src/cli/viewer.rs viewer_loop 234$' tests/hygiene_allowlist.txt)" >> "$A"
norm() { tr -d ' \t' | grep -v '^$' | sort; }
git show $B:$F | norm > /tmp/m31-09-old.txt; cat $F $N | norm > /tmp/m31-09-new.txt
echo "lost_other=$(comm -23 /tmp/m31-09-old.txt /tmp/m31-09-new.txt | grep -vE '^fnwrap_words\(' | wc -l)" >> "$A"
echo "added_other=$(comm -13 /tmp/m31-09-old.txt /tmp/m31-09-new.txt | grep -vE '^(use|pubuse|//!|pub\(super\)fnwrap_words\(|#\[path=|#\[cfg\(test\)\]$|modlayout;)' | wc -l)" >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Expected (from the dry run): `1503 …viewer.rs`, `259 …viewer_layout.rs`,
`allowlisted=0 fn_kept=1`, `lost_other=0`, `added_other=0`, hygiene `ok.`,
clippy clean, `fmt_exit=0`, `passed=2586 failed=0`.

## Authorizations

- Edit `src/cli/viewer.rs`; create `src/cli/viewer_layout.rs`. Edit
  `tests/hygiene_allowlist.txt` (one line) and `CLAUDE.md` (one row).

## Out of scope

- `viewer.rs`'s inline `mod tests` (stays inline, under the 2000-line cap)
  and `viewer_loop` (234 lines, split in group 3).
- Any change inside the moved functions beyond `wrap_words`'s `pub(super)`.

## Update Log

<!-- entries appended below this line -->

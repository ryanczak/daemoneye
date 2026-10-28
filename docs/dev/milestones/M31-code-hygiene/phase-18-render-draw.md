# Phase 18 — Move `RatatuiRenderer`'s drawing methods out of `render_ratatui.rs`

**Status:** todo
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-27

## Goal

`src/cli/render_ratatui.rs` (1225 production lines) drops under the
800-line cap. Its generic `impl<B: Backend> RatatuiRenderer<B>` block moves
into a new `src/cli/render_ratatui_draw.rs`, declared as a child module of
`render_ratatui`. The block holds `draw` and its panel variants, `commit`,
`commit_panel` and the repaint history. A child module may implement its
parent's type and read the type's private fields, so nothing is
re-exported and no caller changes. The struct, the Stdout-backend impl, the
ANSI parser and the region renderers stay. Pure move: no line inside the
block changes.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md` — decisions 5, 6, 7.

## Spec

1. **Run this script as written, once, with `bash`.** It does the whole
   change: the cut, the module declaration, the allowlist line, the
   CLAUDE.md row, then `cargo fmt`. It was dry-run on a scratch worktree of
   `d3bc6ad3`: clippy and the full suite (2586) green, the moved block
   byte-identical. Do not hand-edit anything it touches. Do not dry-run it
   again in a worktree of your own. Do not `sed -i` a source file.

   Every method that crosses the boundary is already `pub`. The one private
   method, `record_committed_rows`, is called only inside the block.

   ```bash
   D=src/cli; F=$D/render_ratatui.rs; N=$D/render_ratatui_draw.rs
   S=$(grep -n '^impl<B: Backend> RatatuiRenderer<B> {$' $F | cut -d: -f1)
   E=$(( $(grep -n '^/// Render the live (non-committed) region' $F | cut -d: -f1) - 2 ))
   echo "S=$S E=$E"                      # expect 401 939
   { printf '//! `RatatuiRenderer`'"'"'s drawing and committing methods, for any backend:\n//! `draw` and its panel variants, `commit` and `commit_panel`, and the repaint\n//! history they keep.\n\nuse super::*;\n\n'
     sed -n "${S},${E}p" $F; } > $N
   { head -n $((S-1)) $F
     printf '#[path = "render_ratatui_draw.rs"]\nmod draw;\n\n'
     tail -n +$((E+2)) $F; } > $F.tmp && mv $F.tmp $F
   A=tests/hygiene_allowlist.txt
   grep -v '^file src/cli/render_ratatui.rs ' $A > $A.tmp && mv $A.tmp $A
   awk '{print} /^\| `src\/cli\/viewer_layout.rs` \|/ {print "| `src/cli/render_ratatui_draw.rs` | `RatatuiRenderer<B>`'"'"'s drawing and committing methods (`draw`, `draw_spinner`, `draw_prompt`, the approval / credential / picker panels, `commit`, `commit_panel`, the repaint history); a child module of `src/cli/render_ratatui.rs` |"}' CLAUDE.md > CLAUDE.md.tmp && mv CLAUDE.md.tmp CLAUDE.md
   cargo fmt --all
   wc -l $F $N
   ```
2. **Log it at once.** Append a `### Update — <date> (cuts done)` entry
   to this doc's Update Log with the patch or write tool, quoting the
   script's output. The stall governor counts only edit-tool calls as
   progress (rexyMCP issue #14).
3. **Findings** (decision 7): read `render_ratatui_draw.rs` once and record
   what you see, or `none`. It was flagged at scoping as a 700-line block;
   note where `draw`'s panel variants share code.
4. **Run the End-to-end block and paste its output into the Update Log
   with the write or patch tool**, as a `### Update — <date> (end-to-end
   verification)` entry holding `/tmp/e2e-m31-18.txt` in a fenced block.
   The block is a `bash` heredoc, so run it as it stands. Check with
   `grep -c '^moved_identical=1$'` on this doc before you commit; it must
   print 1.
5. Commit as `refactor:`.

## Acceptance criteria

- [ ] `render_ratatui_draw.rs` holds `impl<B: Backend> RatatuiRenderer<B>`,
      and `render_ratatui.rs` no longer does. `render_ratatui.rs` has at
      most 800 production lines (`cargo test --test hygiene` measures it).
- [ ] The allowlist has no `file src/cli/render_ratatui.rs` line.
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
B=d3bc6ad3; D=src/cli; F=$D/render_ratatui.rs; N=$D/render_ratatui_draw.rs
A=/tmp/e2e-m31-18.txt; : > "$A"
wc -l $F $N | sed 's/^ *//' >> "$A"
grep -c '^file src/cli/render_ratatui.rs ' tests/hygiene_allowlist.txt | sed 's/^/allowlisted=/' >> "$A"
norm() { tr -d ' \t' | grep -v '^$' | sort; }
git show $B:$F | norm > /tmp/m31-18-old.txt; cat $F $N | norm > /tmp/m31-18-new.txt
echo "lost_other=$(comm -23 /tmp/m31-18-old.txt /tmp/m31-18-new.txt | wc -l)" >> "$A"
echo "added_other=$(comm -13 /tmp/m31-18-old.txt /tmp/m31-18-new.txt | grep -vcE '^(#\[path=|//!|moddraw;$|usesuper::\*;$)')" >> "$A"
if diff -q <(git show $B:$F | sed -n 401,939p) <(tail -n +7 $N) >/dev/null; then echo moved_identical=1; else echo moved_identical=0; fi >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
E2E
```

Expected (from the dry run): `694 …render_ratatui.rs`,
`545 …render_ratatui_draw.rs`, `allowlisted=0`, `lost_other=0`,
`added_other=0`, `moved_identical=1`, hygiene `ok.`, clippy clean,
`fmt_exit=0`, `passed=2586 failed=0`.

## Authorizations

- Edit `src/cli/render_ratatui.rs`; create `render_ratatui_draw.rs` beside
  it. Edit `tests/hygiene_allowlist.txt` and `CLAUDE.md` through the script
  only.

## Out of scope

- The struct, the Stdout-backend impl, the ANSI parser, the region
  renderers and both test files.
- Any change inside the moved block.

## Update Log

<!-- entries appended below this line -->

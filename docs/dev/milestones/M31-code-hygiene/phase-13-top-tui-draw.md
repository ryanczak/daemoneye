# Phase 13 — Move `top`'s drawing out of `top_tui.rs`

**Status:** in-progress
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-27

## Goal

`src/cli/commands/top_tui.rs` (1050 production lines) drops under the
800-line cap by moving its drawing section, `draw` through `draw_panel`,
into a new `src/cli/commands/top_tui_draw.rs`. The new file is declared
as a child module of `top_tui`. The section covers one frame of header,
panels or the open detail, and footer, plus the panel windowing that
keeps the rows that matter on screen. `TopModel`, the refresh planning,
the layout helpers and `run_tui` stay in `top_tui.rs`. Pure move.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md` — decisions 5, 6, 7.

## Spec

1. **Run this script as written, once, with `bash`.** It does the whole
   code change: the cut, the wiring, the allowlist line and the CLAUDE.md
   row, then `cargo fmt`. It was dry-run on a scratch worktree of `6c4feb2c`:
   clippy and the full suite (2586) green, the moved block byte-identical
   apart from the line below. Do not hand-edit anything it touches. Do
   not dry-run it again in a worktree of your own. Do not `sed -i` a
   source file: the script rewrites through temp files and `mv`.

   The only line it changes inside the moved code is `inner_height`, which
   gains `pub(super)`. The layout helpers that stay in `top_tui.rs` call
   it. `draw` is re-exported for `run_tui`. `draw_at` and `visible_range`
   are imported under `#[cfg(test)]`, because only the inline tests use
   them from the parent, and a plain `pub use` fails the build as an
   unused import.

   ```bash
   D=src/cli/commands; F=$D/top_tui.rs; N=$D/top_tui_draw.rs
   S=$(grep -n '^/// Draw one frame at the wall clock' $F | cut -d: -f1)
   E=$(( $(grep -n '^/// The live loop\. ' $F | cut -d: -f1) - 2 ))
   echo "S=$S E=$E"                      # expect 615 920
   { printf '//! Drawing `top`: one frame of header, panels or detail, and footer, and the\n//! windowing that keeps the rows that matter on screen.\n\nuse super::*;\n\n'
     sed -n "${S},${E}p" $F | sed 's/^fn inner_height(/pub(super) fn inner_height(/'; } > $N
   { head -n $((S-1)) $F
     printf '#[path = "top_tui_draw.rs"]\nmod draw;\nuse draw::inner_height;\npub use draw::draw;\n#[cfg(test)]\nuse draw::{draw_at, visible_range};\n\n'
     tail -n +$((E+2)) $F; } > $F.tmp && mv $F.tmp $F
   A=tests/hygiene_allowlist.txt
   grep -v '^file src/cli/commands/top_tui.rs ' $A > $A.tmp && mv $A.tmp $A
   awk '{print} /^\| `src\/cli\/commands\/slash_session.rs` \|/ {print "| `src/cli/commands/top_tui_draw.rs` | Drawing `daemoneye top`: `draw` / `draw_at` (header, four panels or the open detail, footer), the panel windowing (`visible_range`, `runs_window`) and `draw_panel`; a child module of `src/cli/commands/top_tui.rs`, re-exported from it |"}' CLAUDE.md > CLAUDE.md.tmp && mv CLAUDE.md.tmp CLAUDE.md
   cargo fmt --all
   wc -l $F $N
   ```
2. **Log it at once.** Append a `### Update — <date> (cuts done)` entry
   to this doc's Update Log with the patch or write tool, quoting the
   script's output (`S=… E=…` and the `wc -l` counts). The stall governor
   counts only edit-tool calls as progress (rexyMCP issue #14).
3. **Findings** (decision 7): read `top_tui_draw.rs` once and record what
   you see, or `none`.
4. **Run the End-to-end block and paste its output into the Update Log
   with the write or patch tool**, as a `### Update — <date> (end-to-end
   verification)` entry holding `/tmp/e2e-m31-13.txt` in a fenced block.
   Check with `grep -c '^lost_other=0$'` on this doc before you commit; it
   must print 1.
5. Commit the code as `refactor:`.

## Acceptance criteria

- [ ] `top_tui_draw.rs` defines `draw`, `draw_at`, `visible_range`,
      `runs_window` and `draw_panel`. `top_tui.rs` defines none of them and
      has at most 800 production lines (`cargo test --test hygiene`
      measures it).
- [ ] The allowlist has no `file src/cli/commands/top_tui.rs` line.
- [ ] The End-to-end block prints `lost_other=0`, `added_other=0`,
      `moved_identical=1` and `passed=2586 failed=0`.
- [ ] The Update Log has a `(cuts done)` entry, a `### Findings — <date>`
      entry and an `(end-to-end verification)` entry containing the
      block's output. An edit tool wrote all three, and all three are in
      this file.

## End-to-end verification

Run with `bash`. Paste the output byte-exact into the Update Log (Spec 4).

```sh
export LC_ALL=C   # sort and comm disagree under the host locale without it
B=6c4feb2c; F=src/cli/commands/top_tui.rs; N=src/cli/commands/top_tui_draw.rs
A=/tmp/e2e-m31-13.txt; : > "$A"
wc -l $F $N | sed 's/^ *//' >> "$A"
grep -c '^file src/cli/commands/top_tui.rs ' tests/hygiene_allowlist.txt | sed 's/^/allowlisted=/' >> "$A"
norm() { tr -d ' \t' | grep -v '^$' | sort; }
git show $B:$F | norm > /tmp/m31-13-old.txt; cat $F $N | norm > /tmp/m31-13-new.txt
echo "lost_other=$(comm -23 /tmp/m31-13-old.txt /tmp/m31-13-new.txt | grep -vcE '^fninner_height\(')" >> "$A"
echo "added_other=$(comm -13 /tmp/m31-13-old.txt /tmp/m31-13-new.txt | grep -vcE '^(usesuper::\*;|//!|#\[path=|#\[cfg\(test\)\]$|moddraw;|usedraw::|pubusedraw::draw;|pub\(super\)fninner_height\()')" >> "$A"
if diff -q <(git show $B:$F | sed -n 615,920p | sed 's/^fn inner_height(/pub(super) fn inner_height(/') <(tail -n +6 $N) >/dev/null; then echo moved_identical=1; else echo moved_identical=0; fi >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Expected (from the dry run): `2417 …top_tui.rs`, `311 …top_tui_draw.rs`,
`allowlisted=0`, `lost_other=0`, `added_other=0`, `moved_identical=1`,
hygiene `ok.`, clippy clean, `fmt_exit=0`, `passed=2586 failed=0`.

## Authorizations

- Edit `src/cli/commands/top_tui.rs`; create `top_tui_draw.rs` beside it.
  Edit `tests/hygiene_allowlist.txt` and `CLAUDE.md`, through the script
  only.

## Out of scope

- `top_tui.rs`'s inline `mod tests` (1666 lines, under the 2000 cap),
  `TopModel`, the layout helpers and `run_tui`.
- Any change inside the moved functions beyond `inner_height`'s
  `pub(super)`.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-27 14:20 (started)

Executor dispatch (Claude) begins phase 13. Flipped Status to `in-progress` and
the README phase-table row to match. Next: run the Spec 1 script once with
`bash`, then log its output per Spec 2.

### Update — 2026-10-27 14:22 (cuts done)

Ran the Spec 1 script once with `bash`, as written (heredoc body pasted into a
single `bash` invocation; the write tools cannot author outside the project
root, so the script text lived only in the command). Output:

```
S=615 E=920
  2417 src/cli/commands/top_tui.rs
   311 src/cli/commands/top_tui_draw.rs
  2728 total
```

`S`/`E` match the expected 615/920 and the line counts match the dry run
(2417 + 311). `cargo fmt --all` ran inside the script. Nothing hand-edited in
the touched files. Next: findings (Spec 3), then end-to-end (Spec 4).

### Findings — 2026-10-27

Read `top_tui_draw.rs` (311 lines) once, top to bottom. `use super::*;` pulls
`TopModel`, the `top` items (`Panel`, `plan_layout` via `layout_at`,
`header_text`, the text builders, `run_fades`, `BAR_CHROME`) and ratatui's
widgets/styles from the parent — the glob keeps the moved code byte-identical
apart from `inner_height`. Observations, all pre-existing behaviour carried
over, none introduced by the move:

- `visible_range` clamps with `min(total.saturating_sub(inner_h))`, so a `top`
  past the last window lands on the last full window; zero height yields an
  empty range. Sound, and the inline tests in the parent cover all four edges.
- `runs_window` keeps line 0 (the column header) and a `rows_visible` slice
  that slides to keep the selected row on screen; `kept_lines` records the
  original indices so `draw_at` can map faded runs through it instead of
  redoing the window arithmetic. The `kept` vector for the under-height case
  is `(0..line_count).collect()` — fine, but it allocates a full index vec
  every frame for the common non-overflowing case. Not worth touching under
  "pure move"; noting only.
- `window_text` collects `Vec<String>` and joins with `\n`, so every frame
  reallocates the whole panel body text. Same pre-existing shape as before the
  move; out of scope.
- `activity_window_text` is a one-line alias of `window_text` — carried over
  verbatim per the pure-move rule.
- `draw_panel` takes `fades: &[usize]` and does `fades.contains(&i)` per line —
  linear per row, but panel row counts are small. Pre-existing.
- `inner_width` is `fn` (private to the child); only `draw_at`'s budget bar
  uses it, while `inner_height` is `pub(super)` for the parent's layout
  helpers. That asymmetry is exactly what the spec's one-line change intends.

No blockers; nothing in the moved code looks wrong or surprising beyond the
above, which are performance-flavoured and out of scope for a pure move.

### Update — 2026-10-27 14:35 (end-to-end verification)

Ran the phase doc's End-to-end block with `bash` (as a file, `.e2e-m31-13.sh`,
then deleted: the tool shell is `dash`, whose non-interactive mode rejects the
block's `<(...)` process substitution and `norm()` function definitions —
`Syntax error: "(" unexpected` at exit 2 before any line was produced; under
`bash` the identical bytes ran green, once). Output below is `/tmp/e2e-m31-13.txt`
byte-exact:

```
2417 src/cli/commands/top_tui.rs
311 src/cli/commands/top_tui_draw.rs
2728 total
allowlisted=0
lost_other=0
added_other=0
moved_identical=1
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2586 failed=0
```

Matches the dry run's expectations on every line: 2417 + 311, `allowlisted=0`,
`lost_other=0`, `added_other=0`, `moved_identical=1`, hygiene `ok.`,
clippy `Finished` clean, `fmt_exit=0`, `passed=2586 failed=0`.


# Phase 18 — Move `RatatuiRenderer`'s drawing methods out of `render_ratatui.rs`

**Status:** in-progress
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

### Update — 2026-10-28 00:08 (started)

**Executor:** headless run.
Phase doc flipped to in-progress, README phase table updated. Beginning the cut script (Spec 1).

### Update — 2026-10-28 00:09 (cuts done)

Ran the Spec 1 script once with `bash`, exactly as written. Output:

```
S=401 E=939
  694 src/cli/render_ratatui.rs
  545 src/cli/render_ratatui_draw.rs
 1239 total
```

`cargo fmt --all` inside the script reported nothing to change. The moved
block went into `src/cli/render_ratatui_draw.rs` under the `use super::*;`
header, `#[path = "render_ratatui_draw.rs"] mod draw;` landed where the impl
block was, the allowlist lost its `file src/cli/render_ratatui.rs 1225`
line, and the CLAUDE.md row was inserted after the `viewer_layout.rs` row.

### Findings — 2026-10-28 00:12 (decision 7, scoped per Spec 3)

Method: read `render_ratatui_draw.rs` (545 lines) once, end to end, plus the
parent-file helpers it calls (`render_ratatui.rs`: `render_live_region`,
`render_spinner_region`, `render_prompt_region`, `approval_options_line`,
`credential_hint_line`, `picker_window`, `truncate_with_ellipsis`,
`short_session`, `fmt_uptime`, `MAX_REPAINT_HISTORY`) to see what the block
shares. No line changed; findings recorded, not fixed.

**1. (`smell`, minor) Three panel variants duplicate the status-bar tail.**
`draw_approval_panel` (lines ~247–263), `draw_credential_panel`
(~332–348) and `draw_picker_panel` (~419–435) each re-declare the identical
status-bar block: the same `Layout::default()` with
`[Constraint::Min(1), Constraint::Length(1)]`, the same `fmt_uptime(start_time.elapsed())`,
the same `session:{short_session} · {model} · up {uptime}` format string, and
the same dim `DarkGray` status paragraph rendered into `chunks[1]`. The
closure-boundary (each needs `&mut self` inside `terminal.draw`) makes the
obvious extraction mildly awkward, but a free fn taking `(frame, area,
session_id, model, start_time)` would drop ~17 lines per panel. Also a
third copy of the status text lives in `render_live_region` /
`render_spinner_region` / `render_prompt_region` in the parent, so the
format string `" session:{} · {} · up {} "` exists 6+ times across the two
files.

**2. (`smell`, minor) Two panel variants duplicate the panel shell.**
`draw_approval_panel` and `draw_credential_panel` build the same
`Block::default().borders(Borders::ALL).border_type(Rounded).border_style(red)`
+ bold-yellow title span, then differ only in the content paragraph. The
picker variant omits the red border (default style) — possibly intentional,
possibly drift; worth a look when these are next touched.

**3. (`smell`, minor) `commit_panel` / `commit_panel_labeled` render into a
manually-built `Vec<Line>` instead of a `Paragraph` inside the panel Block
the draw variants use.** The committed shape (top/bottom border strings
with manual `─.repeat(fill)` arithmetic) is a separate visual dialect from
the `Block`-based panels; the manual `inner.saturating_sub(title_len + 4)`
fill math silently mis-frames when the title contains wide (double-width)
characters, since `chars().count()` is not display width. Severity is
bounded by the fact that titles here are ASCII tool names today.

**4. (`perf`, nit) `commit` builds the history `Vec<Line>` even when
`insert_before` fails.** `record_committed_rows` is called unconditionally
after `self.terminal.insert_before(...)`, so on a backend error the rows are
still pushed into the repaint history and `inserted_rows` was already
incremented — committed-count drifts from reality on a failed commit. Not
user-visible in the current flow (the error propagates and the caller tears
down), but the accounting is optimistic.

**5. (`smell`, nit) `record_committed_rows` is the only private method in the
block** — correct per spec (called only by `commit`, `commit_styled`,
`commit_panel_labeled` inside the child module), and `commit_panel`'s
`use ratatui::widgets::Clear;` is a fn-local import while everything else
comes from `use super::*;`; harmless but inconsistent with the file's other
imports.

**6. (`smell`, nit) The module doc comment's first line has a stray
backtick** — `` //! `RatatuiRenderer's drawing … `` opens a backtick before
`RatatuiRenderer` that is never closed (the apostrophe consumed the closing
one in the spec's printf). Cosmetic; rustdoc renders it literally.

### Update — 2026-10-28 00:20 (end-to-end verification)

Ran the Spec 4 End-to-end block (bash heredoc) as written; `/tmp/e2e-m31-18.txt`
contents, byte-exact:

```
694 src/cli/render_ratatui.rs
545 src/cli/render_ratatui_draw.rs
1239 total
allowlisted=0
lost_other=0
added_other=0
moved_identical=1
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2586 failed=0
```

Every line matches the dry-run expectation: parent 694 / child 545 (both under
the 800 cap), no allowlist entry, nothing lost or added beyond the four
expected new lines, the moved block byte-identical to `d3bc6ad3` lines
401–939, hygiene `ok.`, clippy clean, `fmt_exit=0`, full suite 2586/0.

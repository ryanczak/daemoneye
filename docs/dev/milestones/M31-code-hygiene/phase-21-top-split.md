# Phase 21 — Split `top.rs` into loaders, text frames and run detail

**Status:** todo
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-28

## Goal

`src/cli/commands/top.rs` (1394 production lines) drops under the 800-line
cap. Three of its banner-delimited sections move into child modules,
each glob re-exported:
- `// ── Loaders ──` and the fingerprint go to `top_load.rs`;
- `// ── Text frame ──` (panel texts, budget bar, header, `plan_layout`)
  goes to `top_text.rs`;
- `// ── Run detail ──` goes to `top_detail.rs`.

`top.rs` keeps the snapshot types, the Activity rows (with
`activity_row_in`), `run_top` and its tests, at about 620 production
lines. Every `crate::cli::commands::top::…` path `top_tui.rs` uses still
resolves. Pure move.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md` — decisions 5, 6, 7.

## Spec

1. **Run this script as written, once, with `bash`.** It does the whole
   change: three cuts, the wiring, the allowlist line, the CLAUDE.md row,
   then `cargo fmt`. It was dry-run on a scratch worktree of `06ae5753`:
   clippy, fmt and the full suite (2586) green, hygiene included. Do not
   hand-edit anything it touches. Do not dry-run it again in a worktree of
   your own. Do not `sed -i` a source file.

   Four lines change inside the moved code. `run_subject`, `state_word`,
   `sort_rows` and `format_age` gain `pub(super)`, because the parent, a
   sibling child or the tests call them. The glob re-exports
   (`pub use load::*;` and so on) re-export the public items and quietly
   limit the `pub(super)` ones to `top`. The moved code reaches the sibling
   commands `ipc_client` and `scorecard` as `super::…`, so the script
   imports both into `top.rs`. **Do not rewrite those paths to
   `super::super::…`.**

   **Why `activity_row_in` stays in `top.rs`.** It was the first choice to
   move, but its signature is 99 columns long. `pub(super)` (the tests call
   it) makes rustfmt wrap it, which grows the function past its
   allowlisted 163 lines, and the hygiene gate fails. The first dry run hit
   exactly that.

   ```bash
   D=src/cli/commands; F=$D/top.rs
   L1=$(grep -n '^// ── Loaders ──' $F | cut -d: -f1)
   X1=$(grep -n '^// ── Text frame ──' $F | cut -d: -f1)
   X2=$(( $(grep -n '^/// `daemoneye top \[--once\]' $F | cut -d: -f1) - 2 ))
   R1=$(grep -n '^// ── Run detail ──' $F | cut -d: -f1)
   T=$(grep -n '^#\[cfg(test)\]' $F | tail -1 | cut -d: -f1)
   echo "L1=$L1 X1=$X1 X2=$X2 R1=$R1 T=$T"     # expect 553 784 1105 1172 1395
   vis() { sed -e 's/^fn \(run_subject\|state_word\|sort_rows\|format_age\)(/pub(super) fn \1(/'; }
   hdr() { printf '//! %s\n\nuse super::*;\n\n' "$1"; }
   { hdr '`top`'"'"'s loaders and change fingerprint: the Runs, Budget, Shells and header reads, `take_snapshot`, `fingerprint`.'
     sed -n "${L1},$((X1-2))p" $F | vis; } > $D/top_load.rs
   { hdr '`top`'"'"'s text frames and layout: the panel texts, the budget bar, the header and `plan_layout`.'
     sed -n "${X1},${X2}p" $F | vis; } > $D/top_text.rs
   { hdr 'The run detail view: `load_run_detail` and `run_detail_text`.'
     sed -n "${R1},$((T-2))p" $F | vis; } > $D/top_detail.rs
   { head -n $((L1-1)) $F
     printf '#[path = "top_load.rs"]\nmod load;\npub use load::*;\n#[path = "top_text.rs"]\nmod text_frame;\npub use text_frame::*;\n// The children reach these sibling commands as `super::…`, through this import.\nuse super::{ipc_client, scorecard};\n\n'
     sed -n "$((X2+2)),$((R1-1))p" $F
     printf '#[path = "top_detail.rs"]\nmod detail;\npub use detail::*;\n\n'
     tail -n +$T $F; } > $F.tmp && mv $F.tmp $F
   A=tests/hygiene_allowlist.txt
   grep -v '^file src/cli/commands/top.rs ' $A > $A.tmp && mv $A.tmp $A
   awk '{print} /^\| `src\/cli\/commands\/top_tui_draw.rs` \|/ {print "| `src/cli/commands/top_load.rs`, `top_text.rs`, `top_detail.rs` | `daemoneye top` split by concern, child modules of `src/cli/commands/top.rs` (the snapshot types, the Activity rows and `run_top`) and glob re-exported from it: `top_load.rs` the loaders and `fingerprint`, `top_text.rs` the panel texts and `plan_layout`, `top_detail.rs` the run detail view |"}' CLAUDE.md > CLAUDE.md.tmp && mv CLAUDE.md.tmp CLAUDE.md
   cargo fmt --all
   wc -l $F $D/top_load.rs $D/top_text.rs $D/top_detail.rs
   ```
2. **Log it at once.** Append a `### Update — <date> (cuts done)` entry
   to this doc's Update Log with the patch or write tool, quoting the
   script's output. The stall governor counts only edit-tool calls as
   progress (rexyMCP issue #14).
3. **Findings** (decision 7): read the three new files once. For each,
   check these and record every hit, or `none`:
   - a `lock().expect` or `lock().unwrap()` (CLAUDE.md requires
     `.unwrap_or_log()`);
   - any other `expect`, `unwrap` or `panic!` in production code;
   - an error that is swallowed (`let _ =`, `.ok()`, an empty `Err(_) =>`);
   - an obvious duplicate of code elsewhere in `top`.
4. **Run the End-to-end block, then paste its output into the Update Log
   with the write or patch tool**, as a `### Update — <date> (end-to-end
   verification)` entry holding `/tmp/e2e-m31-21.txt` in a fenced block.
   **Paste only output you have already seen.** Write the entry after the
   block has run, never before. Do not add any other "proof" block; the
   End-to-end output is the evidence. The block is a `bash` heredoc, so
   run it as it stands. Check with `grep -c '^lost_other=0$'` on this doc
   before you commit; it must print 1. **Known flake:** the full suite
   occasionally reports `passed=2498 failed=1`. If the block shows exactly
   that, run it once more and paste both outputs with a line saying so.
5. Commit as `refactor:`.

## Acceptance criteria

- [ ] `top_load.rs` defines `take_snapshot` and `fingerprint`, `top_text.rs`
      defines `budget_text` and `plan_layout`, and `top_detail.rs` defines
      `load_run_detail` and `run_detail_text`. `top.rs` defines none of
      them, still defines `activity_row_in` and `run_top`, and has at most
      800 production lines (`cargo test --test hygiene` measures it).
- [ ] The allowlist has no `file src/cli/commands/top.rs` line. Its
      `fn src/cli/commands/top.rs activity_row_in 163` line stays.
- [ ] The End-to-end block prints `lost_other=0`, `added_other=0`,
      `load_identical=1`, `text_identical=1`, `detail_identical=1` and
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
B=06ae5753; D=src/cli/commands; F=$D/top.rs
A=/tmp/e2e-m31-21.txt; : > "$A"
wc -l $F $D/top_load.rs $D/top_text.rs $D/top_detail.rs | sed 's/^ *//' >> "$A"
grep -c '^file src/cli/commands/top.rs ' tests/hygiene_allowlist.txt | sed 's/^/allowlisted=/' >> "$A"
grep -c '^fn src/cli/commands/top.rs activity_row_in 163$' tests/hygiene_allowlist.txt | sed 's/^/fn_kept=/' >> "$A"
vis() { sed -e 's/^fn \(run_subject\|state_word\|sort_rows\|format_age\)(/pub(super) fn \1(/'; }
norm() { tr -d ' \t' | grep -v '^$' | sort; }
git show $B:$F | norm > /tmp/m31-21-old.txt; cat $F $D/top_load.rs $D/top_text.rs $D/top_detail.rs | norm > /tmp/m31-21-new.txt
echo "lost_other=$(comm -23 /tmp/m31-21-old.txt /tmp/m31-21-new.txt | grep -vcE '^fn(format_age|run_subject|sort_rows|state_word)\(')" >> "$A"
echo "added_other=$(comm -13 /tmp/m31-21-old.txt /tmp/m31-21-new.txt | grep -vcE '^(#\[path=|//|mod(load|text_frame|detail);$|pubuse(load|text_frame|detail)::\*;$|usesuper::\*;$|usesuper::\{ipc_client,scorecard\};$|pub\(super\)fn(format_age|run_subject|sort_rows|state_word)\()')" >> "$A"
for p in "load 553 782" "text 784 1105" "detail 1172 1393"; do set -- $p
  if diff -q <(git show $B:$F | sed -n "$2,$3p" | vis) <(tail -n +5 $D/top_$1.rs) >/dev/null; then echo "$1_identical=1"; else echo "$1_identical=0"; fi >> "$A"; done
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
E2E
```

Expected (from the dry run): `1877 …top.rs`, `234 …top_load.rs`,
`326 …top_text.rs`, `226 …top_detail.rs`, `allowlisted=0`, `fn_kept=1`,
`lost_other=0`, `added_other=0`, `load_identical=1`, `text_identical=1`,
`detail_identical=1`, hygiene `ok.`, clippy clean, `fmt_exit=0`,
`passed=2586 failed=0`.

## Authorizations

- Edit `src/cli/commands/top.rs`; create `top_load.rs`, `top_text.rs` and
  `top_detail.rs` beside it. Edit `tests/hygiene_allowlist.txt` and
  `CLAUDE.md` through the script only.

## Out of scope

- `activity_row_in` (163 lines, a later function split), the snapshot
  types, `run_top`, the inline tests.
- Any change inside the moved code beyond the four `pub(super)` lines,
  including rewriting its `super::` paths.

## Update Log

<!-- entries appended below this line -->

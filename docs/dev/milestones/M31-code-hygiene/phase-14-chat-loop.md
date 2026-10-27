# Phase 14 — Move the ratatui chat loop out of `chat.rs`

**Status:** todo
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-27

## Goal

`src/cli/commands/chat.rs` (1147 production lines) drops under the 800-line
cap by moving its ratatui chat loop into a new
`src/cli/commands/chat_loop.rs`, declared as a child module of `chat`. The
moved block runs from the `// ── Ratatui chat loop` banner through
`read_input_line_inner_ratatui`. That covers `run_chat_ratatui`,
`RatatuiInputCtx` and the line reader. Both of the file's allowlisted giant
functions move with it, unchanged. `run_chat_inner`, the context structs,
the slash-command matchers, the banner and the label helpers stay in
`chat.rs`. Pure move.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md` — decisions 5, 6, 7.

## Spec

1. **Run this script as written, once, with `bash`.** It does the whole
   code change: the cut, the wiring, both allowlist edits and the CLAUDE.md
   row, then `cargo fmt`. It was dry-run on a scratch worktree of `e6450f12`:
   clippy and the full suite (2586) green. Do not hand-edit anything it
   touches. Do not dry-run it again in a worktree of your own. Do not
   `sed -i` a source file: the script rewrites through temp files and `mv`.

   One line changes inside the moved code: `run_chat_ratatui` gains
   `pub(super)`, because `run_chat_inner` calls it from the parent. The
   moved code reaches four sibling command modules as `super::…`
   (`chat_presence`, `ipc_client`, `agents`, `resume_hint`). From the child,
   `super` is `chat`, so the script adds `use super::{agents, ipc_client,
   resume_hint};` to `chat.rs`, which already imports `chat_presence`, and
   those paths resolve unchanged. **Do not rewrite them to
   `super::super::…`.** A longer path makes rustfmt wrap a line, which grows
   `run_chat_ratatui` past its allowlisted 500 lines, and the hygiene gate
   fails. The first dry run hit exactly that.

   ```bash
   D=src/cli/commands; F=$D/chat.rs; N=$D/chat_loop.rs
   S=$(grep -n '^// ── Ratatui chat loop ' $F | cut -d: -f1)
   E=$(( $(grep -n '^fn banner_lines(' $F | cut -d: -f1) - 2 ))
   echo "S=$S E=$E"                      # expect 276 983
   { printf '//! The ratatui chat loop and its line reader: `run_chat_ratatui` drives a\n//! chat session turn by turn, `read_input_line_inner_ratatui` reads one line.\n\nuse super::*;\n\n'
     sed -n "${S},${E}p" $F | sed 's/^async fn run_chat_ratatui(/pub(super) async fn run_chat_ratatui(/'; } > $N
   { head -n $((S-1)) $F
     printf '#[path = "chat_loop.rs"]\nmod chat_loop;\nuse chat_loop::run_chat_ratatui;\n// The loop reaches these sibling modules as `super::…`, through this import.\nuse super::{agents, ipc_client, resume_hint};\n\n'
     tail -n +$((E+2)) $F; } > $F.tmp && mv $F.tmp $F
   A=tests/hygiene_allowlist.txt
   grep -v '^file src/cli/commands/chat.rs ' $A \
     | sed -e 's|^fn src/cli/commands/chat.rs run_chat_ratatui |fn src/cli/commands/chat_loop.rs run_chat_ratatui |' \
           -e 's|^fn src/cli/commands/chat.rs read_input_line_inner_ratatui |fn src/cli/commands/chat_loop.rs read_input_line_inner_ratatui |' > $A.tmp && mv $A.tmp $A
   awk '{print} /^\| `src\/cli\/commands\/slash_session.rs` \|/ {print "| `src/cli/commands/chat_loop.rs` | The ratatui chat loop: `run_chat_ratatui` (one chat session, turn by turn: input, slash commands, the ask stream, the viewers) and `read_input_line_inner_ratatui` (one line of input); a child module of `src/cli/commands/chat.rs` |"}' CLAUDE.md > CLAUDE.md.tmp && mv CLAUDE.md.tmp CLAUDE.md
   cargo fmt --all
   wc -l $F $N
   ```
2. **Log it at once.** Append a `### Update — <date> (cuts done)` entry
   to this doc's Update Log with the patch or write tool, quoting the
   script's output (`S=… E=…` and the `wc -l` counts). The stall governor
   counts only edit-tool calls as progress (rexyMCP issue #14).
3. **Findings** (decision 7): read `chat_loop.rs` once and record what you
   see, or `none`. `run_chat_ratatui` (500 lines) and
   `read_input_line_inner_ratatui` (183) will be split later, so note
   their natural seams.
4. **Run the End-to-end block and paste its output into the Update Log
   with the write or patch tool**, as a `### Update — <date> (end-to-end
   verification)` entry holding `/tmp/e2e-m31-14.txt` in a fenced block.
   The block is a `bash` heredoc, so run it as it stands. Your tool shell
   is `dash`, and the heredoc carries the bash-only syntax past it. Check
   with `grep -c '^lost_other=0$'` on this doc before you commit; it must
   print 1.
5. Commit the code as `refactor:`.

## Acceptance criteria

- [ ] `chat_loop.rs` defines `run_chat_ratatui`, `RatatuiInputCtx` and
      `read_input_line_inner_ratatui`. `chat.rs` defines none of them and
      has at most 800 production lines (`cargo test --test hygiene`
      measures it).
- [ ] The allowlist has no `file src/cli/commands/chat.rs` line, and its
      `run_chat_ratatui 500` and `read_input_line_inner_ratatui 183` lines
      name `chat_loop.rs`.
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
B=e6450f12; D=src/cli/commands; F=$D/chat.rs; N=$D/chat_loop.rs
A=/tmp/e2e-m31-14.txt; : > "$A"
wc -l $F $N | sed 's/^ *//' >> "$A"
grep -c '^file src/cli/commands/chat.rs ' tests/hygiene_allowlist.txt | sed 's/^/allowlisted=/' >> "$A"
grep -cE '^fn src/cli/commands/chat_loop.rs (run_chat_ratatui 500|read_input_line_inner_ratatui 183)$' tests/hygiene_allowlist.txt | sed 's/^/fn_moved=/' >> "$A"
norm() { tr -d ' \t' | grep -v '^$' | sort; }
git show $B:$F | norm > /tmp/m31-14-old.txt; cat $F $N | norm > /tmp/m31-14-new.txt
echo "lost_other=$(comm -23 /tmp/m31-14-old.txt /tmp/m31-14-new.txt | grep -vcE '^asyncfnrun_chat_ratatui\(')" >> "$A"
echo "added_other=$(comm -13 /tmp/m31-14-old.txt /tmp/m31-14-new.txt | grep -vcE '^pub\(super\)asyncfnrun_chat_ratatui\(|^#\[path=|^//|^modchat_loop;$|^usesuper::\*;$|^usechat_loop::run_chat_ratatui;$|^usesuper::\{agents,ipc_client,resume_hint\};$')" >> "$A"
if diff -q <(git show $B:$F | sed -n 276,983p | sed 's/^async fn run_chat_ratatui(/pub(super) async fn run_chat_ratatui(/') <(tail -n +6 $N) >/dev/null; then echo moved_identical=1; else echo moved_identical=0; fi >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
E2E
```

Expected (from the dry run): `645 …chat.rs`, `713 …chat_loop.rs`,
`allowlisted=0`, `fn_moved=2`, `lost_other=0`, `added_other=0`,
`moved_identical=1`, hygiene `ok.`, clippy clean, `fmt_exit=0`,
`passed=2586 failed=0`.

## Authorizations

- Edit `src/cli/commands/chat.rs`; create `chat_loop.rs` beside it. Edit
  `tests/hygiene_allowlist.txt` and `CLAUDE.md`, through the script only.

## Out of scope

- `chat.rs`'s inline `mod tests`, `run_chat_inner` and everything else
  outside the moved block.
- Any change inside the moved functions beyond `run_chat_ratatui`'s
  `pub(super)`, including splitting it or rewriting its `super::` paths.

## Update Log

<!-- entries appended below this line -->

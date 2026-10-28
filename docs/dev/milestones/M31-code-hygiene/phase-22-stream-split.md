# Phase 22 — Split the chat turn and its prompts out of `cli/commands/stream.rs`

**Status:** todo
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-28

## Goal

`src/cli/commands/stream.rs` (1520 production lines) drops under the
800-line cap. Two blocks move into child modules of `stream`:
- `ask_with_session_ratatui`, the 673-line chat-turn function, goes to
  `stream_ask.rs`, re-exported so `chat.rs`, `chat_loop.rs` and `ask.rs`
  still import it from `super::stream`.
- The `// ── Ratatui interactive approval primitives` section and the
  `// ── Ratatui prompt functions` section after it go to
  `stream_prompts.rs`.

`stream.rs` keeps the four context structs, the stream helpers
(`recv_line`, `select_stream`, `send_cancel`, the silence budget) and its
three inline test modules, at about 234 production lines. Pure move.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md` — decisions 5, 6, 7.

## Spec

1. **Run this script as written, once, with `bash`.** It does the whole
   change: both cuts, the wiring, both allowlist edits and the CLAUDE.md
   row, then `cargo fmt`. It was dry-run on a scratch worktree of `b8c6a006`:
   clippy, fmt and the full suite (2586) green, hygiene included. Do not
   hand-edit anything it touches. Do not dry-run it again in a worktree of
   your own. Do not `sed -i` a source file.

   Seven lines change inside the moved code, and the move requires all of
   them:
   - `ask_with_session_ratatui` becomes `pub(in super::super)`: the
     `commands` module, its exact old reach, so `stream` can re-export it.
     Its parameters are already one per line, so the longer signature
     does not wrap, and the function stays at its allowlisted 673 lines.
   - Six private helpers in the prompts block gain `pub(super)`, because
     the parent's `select_stream` or the tests call them:
     `key_outcome`, `tool_runtime_label`, `build_approval_prompt`,
     `parse_approval_response`, `edit_file_auto_approved` and
     `edit_file_grants_session`.

   **The structs stay in `stream.rs` on purpose.** Their 13 fields are all
   `pub(super)`, and moving them would mean widening every one. The moved
   function reads them fine from inside `stream`. The function also reaches
   `super::approval::…`, so the script adds `use super::approval;` to
   `stream.rs`. **Do not rewrite that path to `super::super::…`.**

   ```bash
   D=src/cli/commands; F=$D/stream.rs
   A1=$(grep -n '^/// Stream the AI response through the ratatui renderer\.$' $F | cut -d: -f1)
   A2=$(( $(grep -n '^/// Read one newline-delimited `Response`, accumulating' $F | cut -d: -f1) - 2 ))
   B1=$(grep -n '^// ── Ratatui interactive approval primitives ──' $F | cut -d: -f1)
   T=$(grep -n '^#\[cfg(test)\]' $F | head -1 | cut -d: -f1)
   echo "A1=$A1 A2=$A2 B1=$B1 T=$T"       # expect 112 789 903 1517
   { printf '//! `ask_with_session_ratatui`: one chat turn streamed through the ratatui\n//! renderer, from the request to the final token.\n\nuse super::*;\n\n'
     sed -n "${A1},${A2}p" $F | sed 's/^pub(super) async fn ask_with_session_ratatui(/pub(in super::super) async fn ask_with_session_ratatui(/'; } > $D/stream_ask.rs
   { printf '//! The ratatui approval primitives and prompt functions: reading an\n//! approval, and the tool-call, credential, pane, yes/no, write and\n//! edit-file prompts.\n\nuse super::*;\n\n'
     sed -n "${B1},$((T-2))p" $F | sed -e 's/^fn \(key_outcome\|tool_runtime_label\|build_approval_prompt\|parse_approval_response\|edit_file_auto_approved\|edit_file_grants_session\)(/pub(super) fn \1(/'; } > $D/stream_prompts.rs
   { head -n $((A1-1)) $F
     printf '#[path = "stream_ask.rs"]\nmod ask;\npub(super) use ask::ask_with_session_ratatui;\n// `ask` reaches `super::approval::…` through this import.\nuse super::approval;\n\n'
     sed -n "$((A2+2)),$((B1-1))p" $F
     printf '#[path = "stream_prompts.rs"]\nmod prompts;\nuse prompts::*;\n\n'
     tail -n +$T $F; } > $F.tmp && mv $F.tmp $F
   A=tests/hygiene_allowlist.txt
   grep -v '^file src/cli/commands/stream.rs ' $A \
     | sed 's|^fn src/cli/commands/stream.rs ask_with_session_ratatui |fn src/cli/commands/stream_ask.rs ask_with_session_ratatui |' > $A.tmp && mv $A.tmp $A
   awk '{print} /^\| `src\/cli\/commands\/chat_loop.rs` \|/ {print "| `src/cli/commands/stream_ask.rs`, `stream_prompts.rs` | The chat turn'"'"'s client side, child modules of `src/cli/commands/stream.rs` (the context structs and the stream helpers): `stream_ask.rs` `ask_with_session_ratatui` (re-exported), `stream_prompts.rs` the approval primitives and the ratatui prompt functions |"}' CLAUDE.md > CLAUDE.md.tmp && mv CLAUDE.md.tmp CLAUDE.md
   cargo fmt --all
   wc -l $F $D/stream_ask.rs $D/stream_prompts.rs
   ```
2. **Log it at once.** Append a `### Update — <date> (cuts done)` entry
   to this doc's Update Log with the patch or write tool, quoting the
   script's output. The stall governor counts only edit-tool calls as
   progress (rexyMCP issue #14).
3. **Findings** (decision 7): read the two new files once. For each,
   check these and record every hit, or `none`:
   - a `lock().expect` or `lock().unwrap()` (CLAUDE.md requires
     `.unwrap_or_log()`);
   - any other `expect`, `unwrap` or `panic!` in production code;
   - an error that is swallowed (`let _ =`, `.ok()`, an empty `Err(_) =>`);
   - an obvious duplicate of code elsewhere in the file.

   For `ask_with_session_ratatui` (673 lines, a later split), also name its
   natural seams.
4. **Run the End-to-end block, then paste its output into the Update Log
   with the write or patch tool**, as a `### Update — <date> (end-to-end
   verification)` entry holding `/tmp/e2e-m31-22.txt` in a fenced block.
   **Paste only output you have already seen.** Write the entry after the
   block has run, and add no other "proof" block. The block is a `bash`
   heredoc, so run it as it stands. Check with `grep -c '^lost_other=0$'`
   on this doc before you commit; it must print 1. **Known flake:**
   `passed=2498 failed=1` on the full suite. If the block shows exactly
   that, run it once more and paste both outputs with a line saying so.
5. Commit as `refactor:`.

## Acceptance criteria

- [ ] `stream_ask.rs` defines `ask_with_session_ratatui`, and
      `stream_prompts.rs` defines `prompt_tool_call_ratatui`,
      `prompt_edit_file_ratatui` and `read_approval_input`. `stream.rs`
      defines none of them, still defines `QueryArgs` and `recv_line`, and
      has at most 800 production lines (`cargo test --test hygiene`
      measures it).
- [ ] The allowlist has no `file src/cli/commands/stream.rs` line, and its
      `ask_with_session_ratatui 673` line names `stream_ask.rs`.
- [ ] The End-to-end block prints `lost_other=0`, `added_other=0`,
      `ask_identical=1`, `prompts_identical=1` and `passed=2586 failed=0`.
- [ ] The Update Log has a `(cuts done)` entry, a `### Findings — <date>`
      entry and an `(end-to-end verification)` entry containing the
      block's output. An edit tool wrote all three, and all three are in
      this file.

## End-to-end verification

Paste the output byte-exact into the Update Log (Spec 4).

```sh
bash <<'E2E'
export LC_ALL=C   # sort and comm disagree under the host locale without it
B=b8c6a006; D=src/cli/commands; F=$D/stream.rs
A=/tmp/e2e-m31-22.txt; : > "$A"
wc -l $F $D/stream_ask.rs $D/stream_prompts.rs | sed 's/^ *//' >> "$A"
grep -c '^file src/cli/commands/stream.rs ' tests/hygiene_allowlist.txt | sed 's/^/allowlisted=/' >> "$A"
grep -c '^fn src/cli/commands/stream_ask.rs ask_with_session_ratatui 673$' tests/hygiene_allowlist.txt | sed 's/^/fn_moved=/' >> "$A"
P='key_outcome|tool_runtime_label|build_approval_prompt|parse_approval_response|edit_file_auto_approved|edit_file_grants_session'
norm() { tr -d ' \t' | grep -v '^$' | sort; }
git show $B:$F | norm > /tmp/m31-22-old.txt; cat $F $D/stream_ask.rs $D/stream_prompts.rs | norm > /tmp/m31-22-new.txt
echo "lost_other=$(comm -23 /tmp/m31-22-old.txt /tmp/m31-22-new.txt | grep -vcE "^(fn($P)\(|pub\(super\)asyncfnask_with_session_ratatui\()")" >> "$A"
echo "added_other=$(comm -13 /tmp/m31-22-old.txt /tmp/m31-22-new.txt | grep -vcE "^(#\[path=|//|mod(ask|prompts);$|pub\(super\)useask::ask_with_session_ratatui;$|useprompts::\*;$|usesuper::\*;$|usesuper::approval;$|pub\(insuper::super\)asyncfnask_with_session_ratatui\(|pub\(super\)fn($P)\()")" >> "$A"
if diff -q <(git show $B:$F | sed -n 112,789p | sed 's/^pub(super) async fn ask_with_session_ratatui(/pub(in super::super) async fn ask_with_session_ratatui(/') <(tail -n +6 $D/stream_ask.rs) >/dev/null; then echo ask_identical=1; else echo ask_identical=0; fi >> "$A"
if diff -q <(git show $B:$F | sed -n 903,1515p | sed -E "s/^fn ($P)\(/pub(super) fn \1(/") <(tail -n +7 $D/stream_prompts.rs) >/dev/null; then echo prompts_identical=1; else echo prompts_identical=0; fi >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
E2E
```

Expected (from the dry run): `841 …stream.rs`, `683 …stream_ask.rs`,
`619 …stream_prompts.rs`, `allowlisted=0`, `fn_moved=1`, `lost_other=0`,
`added_other=0`, `ask_identical=1`, `prompts_identical=1`, hygiene `ok.`,
clippy clean, `fmt_exit=0`, `passed=2586 failed=0`.

## Authorizations

- Edit `src/cli/commands/stream.rs`; create `stream_ask.rs` and
  `stream_prompts.rs` beside it. Edit `tests/hygiene_allowlist.txt` and
  `CLAUDE.md` through the script only.

## Out of scope

- The context structs, the stream helpers and the three inline test
  modules.
- Any change inside the moved code beyond the seven visibility lines,
  including splitting `ask_with_session_ratatui` or rewriting its
  `super::` path.

## Update Log

<!-- entries appended below this line -->

# Phase 05 — Move the named-session handlers out of `server/handlers.rs`

**Status:** todo
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-26

## Goal

`src/daemon/server/handlers.rs` (877 production lines) drops under the
800-line cap by moving its "Named session CRUD handlers" section, five
`handle_*_session` functions, into a new sibling `src/daemon/server/sessions.rs`.
Pure move: no signature, body or doc comment changes.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md` — decisions 5 (shell
  range cuts), 6 (no behaviour change), 7 (the Findings entry).
- `src/daemon/server/mod.rs` — declares `mod handlers;` and imports it with
  `use handlers::*;`; that is how the dispatch reaches every handler.

## Spec

1. **Cut the block with the shell — run this as written.** The block runs
   from the `// ── Named session CRUD handlers` banner to the line before
   the blank line that precedes the `// ── Run resume handler` banner. At
   `4b752515` that is lines 543–769.

   ```bash
   F=src/daemon/server/handlers.rs
   S=$(grep -n '^// ── Named session CRUD' $F | cut -d: -f1)
   E=$(( $(grep -n '^// ── Run resume' $F | cut -d: -f1) - 2 ))
   echo "S=$S E=$E"                      # expect 543 769
   { echo '//! Named-session IPC handlers: save, load, list, delete, rename.'; echo
     sed -n '1,16p' $F; echo; sed -n "${S},${E}p" $F; } > src/daemon/server/sessions.rs
   sed -i "${S},$((E+1))d" $F            # the block and the blank line after it
   ```

   The new file starts with a copy of `handlers.rs`'s whole `use` block
   (lines 1–16); step 4 prunes it.
2. **Log the cut at once.** Right after step 1, append a short
   `### Update — <date> (cuts done)` entry to this doc's Update Log **with
   the patch or write tool**, not with bash. The executor's stall governor
   counts only edit-tool calls as progress (rexyMCP issue #14), so a
   shell-only run is hard-failed as stalled however much it cuts.
3. **Wire the module.** In `src/daemon/server/mod.rs`, add `mod sessions;`
   beside `mod handlers;` and `use sessions::*;` beside `use handlers::*;`.
4. **Prune imports.** `cargo build`, then delete from each of the two files
   every `use` line clippy reports as unused, until
   `cargo clippy --all-targets --all-features -- -D warnings` is clean. Change
   no other line.
5. **Allowlist.** Delete `file src/daemon/server/handlers.rs 877` from
   `tests/hygiene_allowlist.txt`. Keep the `fn … handle_status 200` entry:
   `handle_status` stays in `handlers.rs`, unchanged.
6. **CLAUDE.md**, the `src/daemon/server/` row of "Key files": add
   `sessions.rs` the named-session handlers (`handle_save_session`,
   `handle_load_session`, `handle_list_saved_sessions`,
   `handle_delete_saved_session`, `handle_rename_saved_session`).
7. **Findings** (README decision 7): read the five moved handlers once and
   record what you see: a bug, a perf issue, a smell, or `none`.

## Acceptance criteria

- [ ] `src/daemon/server/sessions.rs` exists; `grep -c 'fn handle_.*session'`
      is 5 in it and 0 in `handlers.rs`; `handlers.rs` is at most 800 lines.
- [ ] `grep -c '^file src/daemon/server/handlers.rs ' tests/hygiene_allowlist.txt`
      is 0, and `cargo test --test hygiene` passes.
- [ ] The moved lines are unchanged: the End-to-end block's `lost_lines`
      is 0 and `added_other` is 0.
- [ ] Preservation: `cargo test` passes with exactly 2586 passing tests.
- [ ] The Update Log has a `(cuts done)` entry written by an edit tool and a
      `### Findings — <date>` entry.

## End-to-end verification

Run with `bash`. Paste the output byte-exact into the Update Log.

```sh
export LC_ALL=C   # sort and comm disagree under the host locale without it
B=4b752515; A=/tmp/e2e-m31-05.txt; : > "$A"
wc -l src/daemon/server/handlers.rs src/daemon/server/sessions.rs | sed 's/^ *//' >> "$A"
echo "allowlisted=$(grep -c '^file src/daemon/server/handlers.rs ' tests/hygiene_allowlist.txt)" >> "$A"
norm() { tr -d ' \t' | grep -v '^$' | sort; }
git show $B:src/daemon/server/handlers.rs | norm > /tmp/m31-05-old.txt
cat src/daemon/server/handlers.rs src/daemon/server/sessions.rs | norm > /tmp/m31-05-new.txt
echo "lost_lines=$(comm -23 /tmp/m31-05-old.txt /tmp/m31-05-new.txt | wc -l)" >> "$A"
echo "added_other=$(comm -13 /tmp/m31-05-old.txt /tmp/m31-05-new.txt | grep -vE '^(use|//!)' | wc -l)" >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Expected: `handlers.rs` at most 800 lines, `allowlisted=0`,
`lost_lines=0`, `added_other=0`, hygiene `ok.`, clippy clean, `fmt_exit=0`,
`passed=2586 failed=0`. (`lost_lines` stays 0 even after pruning, because
the pruned `use` lines are still present in `handlers.rs`'s own block; if a
`use` line is pruned from **both** files it shows as lost — list it.)

## Authorizations

- Edit `src/daemon/server/handlers.rs` and `src/daemon/server/mod.rs`;
  create `src/daemon/server/sessions.rs`. Edit `tests/hygiene_allowlist.txt`
  (the one deletion) and `CLAUDE.md` (the one row).

## Out of scope

- The other handlers in `handlers.rs`, including `handle_status` (200
  lines, allowlisted, a later function-split phase).
- Any change to a handler's code, even one a finding names.

## Update Log

<!-- entries appended below this line -->

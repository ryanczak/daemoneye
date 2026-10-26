# Phase 08 — Move the `/session` and `/runs` commands out of `slash.rs`

**Status:** todo
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-26

## Goal

`src/cli/commands/slash.rs` (958 production lines) drops under the
800-line cap by moving its last two command sections, `/session`
(`cmd_session`) and `/runs, /run, /stop` (`cmd_runs`, `cmd_run`,
`cmd_stop`), into a new `src/cli/commands/slash_session.rs`, declared as a
child module of `slash`. Pure move: `handle_slash` still calls the same
four functions, through a `use`.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md` — decisions 5, 6, 7.

## Spec

1. **Cut with the shell — run this as written.** Dry-run on a scratch
   worktree of `cb5f71f8`: fmt, clippy and the full suite (2586) green.
   **No `sed -i` on source files** (the executor refuses in-place edits):
   the rewrite goes through a temp file and `mv`.

   ```bash
   F=src/cli/commands/slash.rs; N=src/cli/commands/slash_session.rs
   S=$(grep -n '^// ── /session ' $F | cut -d: -f1)
   T=$(grep -n '^#\[cfg(test)\]' $F | tail -1 | cut -d: -f1)
   echo "S=$S T=$T"                      # expect 768 959
   { printf '//! The `/session`, `/runs`, `/run` and `/stop` slash commands.\n\nuse super::*;\n\n'
     sed -n "${S},$((T-2))p" $F; } > $N
   { head -n $((S-1)) $F
     printf '#[path = "slash_session.rs"]\nmod session;\nuse session::{cmd_run, cmd_runs, cmd_session, cmd_stop};\n\n'
     tail -n +$T $F; } > $F.tmp && mv $F.tmp $F
   ```
2. **Log the cut at once.** Append a `### Update — <date> (cuts done)`
   entry to this doc's Update Log **with the patch or write tool**. The
   stall governor counts only edit-tool calls as progress (rexyMCP issue
   #14).
3. **Four edits, with the patch tool**, in `slash_session.rs`: each of the
   four moved functions gains `pub(super)`:
   `pub(super) async fn cmd_session(`, `pub(super) fn cmd_runs(`,
   `pub(super) fn cmd_run(`, `pub(super) fn cmd_stop(`.
4. `cargo fmt --all`. No import pruning is needed: the build has no
   warnings, since the new file uses only `use super::*;` and `slash.rs`
   still uses every import it had.
5. **Allowlist:** delete `file src/cli/commands/slash.rs 958`, and change
   `fn src/cli/commands/slash.rs cmd_session 155` to
   `fn src/cli/commands/slash_session.rs cmd_session 155` (moved, unchanged).
6. **CLAUDE.md** "Key files": add a row for
   `src/cli/commands/slash_session.rs`: the `/session`, `/runs`, `/run` and
   `/stop` slash commands (`cmd_session`, `cmd_runs`, `cmd_run`,
   `cmd_stop`), a child module of `cli/commands/slash.rs`.
7. **Findings** (decision 7): read the moved sections once and record what
   you see, or `none`. `cmd_session` is 155 lines and will be split later:
   note its natural seams.

## Acceptance criteria

- [ ] `src/cli/commands/slash_session.rs` defines `cmd_session`, `cmd_runs`,
      `cmd_run` and `cmd_stop`; `slash.rs` defines none of them and has at
      most 800 production lines (`cargo test --test hygiene` measures it).
- [ ] `grep -c '^file src/cli/commands/slash.rs ' tests/hygiene_allowlist.txt`
      is 0, `grep -c '^fn src/cli/commands/slash_session.rs cmd_session 155$'`
      is 1, and `cargo test --test hygiene` passes.
- [ ] The End-to-end block prints `lost_other=0` and `added_other=0`.
- [ ] Preservation: `cargo test` passes with exactly 2586 passing tests.
- [ ] The Update Log has a `(cuts done)` entry written by an edit tool, a
      `### Findings — <date>` entry, and **the End-to-end block's output
      pasted into it** (not only written to `/tmp`).

## End-to-end verification

Run with `bash`. Paste the output byte-exact into the Update Log.

```sh
export LC_ALL=C   # sort and comm disagree under the host locale without it
B=cb5f71f8; F=src/cli/commands/slash.rs; N=src/cli/commands/slash_session.rs
A=/tmp/e2e-m31-08.txt; : > "$A"
wc -l $F $N | sed 's/^ *//' >> "$A"
echo "allowlisted=$(grep -c '^file src/cli/commands/slash.rs ' tests/hygiene_allowlist.txt) fn_moved=$(grep -c '^fn src/cli/commands/slash_session.rs cmd_session 155$' tests/hygiene_allowlist.txt)" >> "$A"
norm() { tr -d ' \t' | grep -v '^$' | sort; }
git show $B:$F | norm > /tmp/m31-08-old.txt; cat $F $N | norm > /tmp/m31-08-new.txt
echo "lost_other=$(comm -23 /tmp/m31-08-old.txt /tmp/m31-08-new.txt | grep -vE '^(async)?fncmd_(session|runs|run|stop)\(' | wc -l)" >> "$A"
echo "added_other=$(comm -13 /tmp/m31-08-old.txt /tmp/m31-08-new.txt | grep -vE '^(use|//!|pub\(super\)|#\[path=|modsession;)' | wc -l)" >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Expected (from the dry run): `1281 …slash.rs`, `194 …slash_session.rs`,
`allowlisted=0 fn_moved=1`, `lost_other=0`, `added_other=0`, hygiene `ok.`,
clippy clean, `fmt_exit=0`, `passed=2586 failed=0`.

## Authorizations

- Edit `src/cli/commands/slash.rs`; create
  `src/cli/commands/slash_session.rs`. Edit `tests/hygiene_allowlist.txt`
  (the two lines) and `CLAUDE.md` (one row).

## Out of scope

- The other slash commands, and `slash.rs`'s `mod tests`.
- Any change inside the moved functions, including splitting `cmd_session`.

## Update Log

<!-- entries appended below this line -->

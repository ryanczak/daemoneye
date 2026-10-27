# Phase 10 — Move the tail and log viewers out of `tail.rs`

**Status:** todo
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-27

## Goal

`src/cli/tail.rs` (988 production lines) drops under the 800-line cap by
moving its viewer section, `open_tail` through `color`, into a new
`src/cli/tail_view.rs` declared as a child module of `tail`. The section
covers opening a model from the first frame, `run_log_viewer`,
`run_tail_viewer`, `tail_loop`, the two status-line builders and
`render_tail`. The child is re-exported, so `crate::cli::tail::open_tail`,
`render_tail`, `run_log_viewer` and `run_tail_viewer` (used by
`cli/attach.rs` and `cli/commands/chat.rs`) still resolve. Pure move:
`TailModel`, its impls and the key-action functions stay in `tail.rs`.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md` — decisions 5, 6, 7.

## Spec

1. **Cut with the shell — run this as written.** Dry-run on a scratch
   worktree of `b39b5aff`: fmt (a no-op), clippy and the full suite (2586)
   green. **No `sed -i` on source files** (the executor refuses in-place
   edits): the rewrite goes through a temp file and `mv`. Nothing inside
   the moved code changes; every function that crosses the boundary is
   already `pub`.

   ```bash
   F=src/cli/tail.rs; N=src/cli/tail_view.rs
   S=$(grep -n '^/// The model a tail starts from, given ' $F | cut -d: -f1)
   T=$(grep -n '^#\[cfg(test)\]' $F | tail -1 | cut -d: -f1)
   echo "S=$S T=$T"                      # expect 607 989
   { printf '//! The tail and log viewers: opening a model from the first frame, the\n//! viewer loops, and drawing the screen and its status line.\n\nuse super::*;\n\n'
     sed -n "${S},$((T-2))p" $F; } > $N
   { head -n $((S-1)) $F
     printf '#[path = "tail_view.rs"]\nmod view;\npub use view::{open_log, open_tail, render_tail, run_log_viewer, run_tail_viewer, tail_loop};\n\n'
     tail -n +$T $F; } > $F.tmp && mv $F.tmp $F
   ```
2. **Log the cut at once.** Append a `### Update — <date> (cuts done)`
   entry to this doc's Update Log **with the patch or write tool**. The
   stall governor counts only edit-tool calls as progress (rexyMCP issue
   #14).
3. `cargo fmt --all`. No import pruning: the build has no warnings.
4. **Allowlist:** delete the line `file src/cli/tail.rs 988`. No `fn`
   entry names `tail.rs`.
5. **CLAUDE.md** "Key files": add a row for `src/cli/tail_view.rs`: the
   tail and log viewers (`open_tail`, `open_log`, `run_tail_viewer`,
   `run_log_viewer`, `tail_loop`, `render_tail`), a child module of
   `cli/tail.rs`, re-exported from it.
6. **Findings** (decision 7): read the moved section once and record what
   you see, or `none`.
7. **Run the End-to-end block and paste its output into the Update Log
   with the write or patch tool**, as a `### Update — <date> (end-to-end
   verification)` entry holding the contents of `/tmp/e2e-m31-10.txt` in
   a fenced block. Check with `grep -c '^lost_other=0$'` on this doc
   before you commit; it must print 1.

**Work in this checkout.** The spec is already dry-run, so do not repeat
it in a scratch worktree of your own. Phase 09 did, and it cost turns and
added no evidence.

## Acceptance criteria

- [ ] `src/cli/tail_view.rs` defines `open_tail`, `open_log`,
      `run_log_viewer`, `run_tail_viewer`, `tail_loop` and
      `render_tail`. `tail.rs` defines none of them and has at most 800
      production lines (`cargo test --test hygiene` measures it).
- [ ] `grep -c '^file src/cli/tail.rs ' tests/hygiene_allowlist.txt` is 0
      and `cargo test --test hygiene` passes.
- [ ] The End-to-end block prints `lost_other=0` and `added_other=0`.
- [ ] Preservation: `cargo test` passes with exactly 2586 passing tests.
- [ ] The Update Log has a `(cuts done)` entry, a `### Findings — <date>`
      entry and an `(end-to-end verification)` entry containing the
      block's output. An edit tool wrote all three, and all three are in
      this file.

## End-to-end verification

Run with `bash`. Paste the output byte-exact into the Update Log (Spec 7).

```sh
export LC_ALL=C   # sort and comm disagree under the host locale without it
B=b39b5aff; F=src/cli/tail.rs; N=src/cli/tail_view.rs
A=/tmp/e2e-m31-10.txt; : > "$A"
wc -l $F $N | sed 's/^ *//' >> "$A"
echo "allowlisted=$(grep -c '^file src/cli/tail.rs ' tests/hygiene_allowlist.txt)" >> "$A"
norm() { tr -d ' \t' | grep -v '^$' | sort; }
git show $B:$F | norm > /tmp/m31-10-old.txt; cat $F $N | norm > /tmp/m31-10-new.txt
echo "lost_other=$(comm -23 /tmp/m31-10-old.txt /tmp/m31-10-new.txt | wc -l)" >> "$A"
echo "added_other=$(comm -13 /tmp/m31-10-old.txt /tmp/m31-10-new.txt | grep -vE '^(usesuper::\*;|pubuseview::|//!|#\[path=|modview;)' | wc -l)" >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Expected (from the dry run): `1442 …tail.rs`, `386 …tail_view.rs`,
`allowlisted=0`, `lost_other=0`, `added_other=0`, hygiene `ok.`, clippy
clean, `fmt_exit=0`, `passed=2586 failed=0`.

## Authorizations

- Edit `src/cli/tail.rs`; create `src/cli/tail_view.rs`. Edit
  `tests/hygiene_allowlist.txt` (one line) and `CLAUDE.md` (one row).

## Out of scope

- `tail.rs`'s inline `mod tests` (stays inline, under the 2000-line cap),
  `TailModel` and the key-action functions.
- Any change inside the moved functions.

## Update Log

<!-- entries appended below this line -->

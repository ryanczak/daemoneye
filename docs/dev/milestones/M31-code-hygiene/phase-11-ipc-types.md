# Phase 11 — Move the IPC payload structs out of `ipc.rs`

**Status:** todo
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-27

## Goal

`src/ipc.rs` (966 production lines) drops under the 800-line cap by
moving the payload structs that `Request` and `Response` carry
(`ContainerInfo` through `SessionSummary`) into a new
`src/ipc_types.rs`. The new file is declared as a child module of `ipc`
and glob re-exported, so every `crate::ipc::<Type>` path still resolves.
`ipc.rs` keeps `APPROVAL_TIMEOUT`, the two enums, `classify` and
`Response`'s impl, and stays "the full wire protocol". This settles the
README's open question on `ipc.rs`: it needs a split, not an exemption.
Pure move.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md` — decisions 2, 5, 6, 7.

## Spec

1. **Cut with the shell — run this as written.** Dry-run on a scratch
   worktree of `6b9ee86c`: fmt (a no-op), clippy and the full suite (2586)
   green, and the moved block is byte-identical to its source range.
   **No `sed -i` on source files** (the executor refuses in-place edits):
   the rewrite goes through a temp file and `mv`. Nothing inside the moved
   code changes. Every moved struct is already `pub`. The child's
   `use super::*;` and the parent's `pub use types::*;` form a circular
   glob, which Rust resolves; leave it.

   ```bash
   F=src/ipc.rs; N=src/ipc_types.rs
   S=$(grep -n '^/// One sandboxed container this daemon owns' $F | cut -d: -f1)
   E=$(( $(grep -n '^/// Messages sent from the CLI client to the daemon' $F | cut -d: -f1) - 2 ))
   echo "S=$S E=$E"                      # expect 11 256
   { printf '//! The payload structs `Request` and `Response` carry: container, pane,\n//! schedule, script, shell, log, ghost, limits, runbook and session summaries.\n\nuse super::*;\n\n'
     sed -n "${S},${E}p" $F; } > $N
   { head -n $((S-1)) $F
     printf '#[path = "ipc_types.rs"]\nmod types;\npub use types::*;\n\n'
     tail -n +$((E+2)) $F; } > $F.tmp && mv $F.tmp $F
   ```
2. **Log the cut at once.** Append a `### Update — <date> (cuts done)`
   entry to this doc's Update Log **with the patch or write tool**. The
   stall governor counts only edit-tool calls as progress (rexyMCP issue
   #14).
3. `cargo fmt --all`. No import pruning: the build has no warnings.
4. **Allowlist:** delete the line `file src/ipc.rs 966`. No `fn` entry
   names `ipc.rs`.
5. **CLAUDE.md** "Key files": add a row for `src/ipc_types.rs` directly
   after the `src/ipc.rs` row: the payload structs the wire enums carry
   (`ContainerInfo`, `PaneInfo`, `ShellInfo`, `GhostConfig`,
   `SessionSummary`, …), a child module of `src/ipc.rs`, glob re-exported
   from it.
6. **Findings** (decision 7): read the moved section once and record what
   you see, or `none`.
7. **Run the End-to-end block and paste its output into the Update Log
   with the write or patch tool**, as a `### Update — <date> (end-to-end
   verification)` entry holding the contents of `/tmp/e2e-m31-11.txt` in
   a fenced block. Check with `grep -c '^lost_other=0$'` on this doc
   before you commit; it must print 1.

**Work in this checkout.** The spec has already been dry-run; do not
repeat it in a scratch worktree.

## Acceptance criteria

- [ ] `src/ipc_types.rs` defines `ContainerInfo`, `PaneInfo`, `ShellInfo`,
      `GhostConfig` and `SessionSummary`. `ipc.rs` defines none of them,
      still defines `Request` and `Response`, and has at most 800
      production lines (`cargo test --test hygiene` measures it).
- [ ] `grep -c '^file src/ipc.rs ' tests/hygiene_allowlist.txt` is 0 and
      `cargo test --test hygiene` passes.
- [ ] The End-to-end block prints `lost_other=0`, `added_other=0` and
      `moved_identical=1`.
- [ ] Preservation: `cargo test` passes with exactly 2586 passing tests.
- [ ] The Update Log has a `(cuts done)` entry, a `### Findings — <date>`
      entry and an `(end-to-end verification)` entry containing the
      block's output. An edit tool wrote all three, and all three are in
      this file.

## End-to-end verification

Run with `bash`. Paste the output byte-exact into the Update Log (Spec 7).

```sh
export LC_ALL=C   # sort and comm disagree under the host locale without it
B=6b9ee86c; F=src/ipc.rs; N=src/ipc_types.rs
A=/tmp/e2e-m31-11.txt; : > "$A"
wc -l $F $N | sed 's/^ *//' >> "$A"
echo "allowlisted=$(grep -c '^file src/ipc.rs ' tests/hygiene_allowlist.txt)" >> "$A"
norm() { tr -d ' \t' | grep -v '^$' | sort; }
git show $B:$F | norm > /tmp/m31-11-old.txt; cat $F $N | norm > /tmp/m31-11-new.txt
echo "lost_other=$(comm -23 /tmp/m31-11-old.txt /tmp/m31-11-new.txt | wc -l)" >> "$A"
echo "added_other=$(comm -13 /tmp/m31-11-old.txt /tmp/m31-11-new.txt | grep -vE '^(usesuper::\*;|pubusetypes::\*;|//!|#\[path=|modtypes;)' | wc -l)" >> "$A"
if diff -q <(git show $B:$F | sed -n 11,256p) <(tail -n +6 $N) >/dev/null; then echo moved_identical=1; else echo moved_identical=0; fi >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Expected (from the dry run): `726 …ipc.rs`, `251 …ipc_types.rs`,
`allowlisted=0`, `lost_other=0`, `added_other=0`, `moved_identical=1`,
hygiene `ok.`, clippy clean, `fmt_exit=0`, `passed=2586 failed=0`.

## Authorizations

- Edit `src/ipc.rs`; create `src/ipc_types.rs`. Edit
  `tests/hygiene_allowlist.txt` (one line) and `CLAUDE.md` (one row).

## Out of scope

- `Request`, `Response`, `RecentCommand`, `Classification` and
  `classify`, which stay in `ipc.rs`; `src/ipc_tests.rs`.
- Any change inside the moved structs, including their serde attributes
  (they are the wire format).

## Update Log

<!-- entries appended below this line -->

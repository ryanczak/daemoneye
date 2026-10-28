# Phase 34 — Exempt `defs.rs` from the hygiene caps

**Status:** todo
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-28

## Goal

`src/ai/tools/defs.rs` (1398 production lines) is the last file entry in
`tests/hygiene_allowlist.txt`. It is the flat `TOOLS` table, one
`ToolDef` per AI tool, and `tests/doc_truth.rs` already holds it to the
README. The PE decided on 2026-10-28 to exempt it under decision 2,
beside `pending.rs` and `args.rs` (milestone README, Notes). Splitting it
would scatter one table across files that nothing else distinguishes.

The phase changes three things:
- an `EXEMPT` entry in `tests/hygiene.rs`;
- deleting `file src/ai/tools/defs.rs 1398` from the allowlist;
- naming the third exempt file in `CLAUDE.md`'s Build & Test paragraph.

`defs.rs` has no function entries, so nothing else changes. After this
phase the allowlist holds no file entries, and every remaining line is a
function.

No code changes. The suite count stays at 2587; a rising count would
mean scope creep.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md`: decision 2 and the
  Notes entry "`defs.rs` exempted (PE decision 2026-10-28)".

## Spec

1. **Run the script below as written, once.** It makes the three edits
   and prints `defs.rs exempt; allowlist file entries left: 0`. It was
   dry-run on a scratch worktree of `74466777`: hygiene, clippy and fmt were
   clean, and the suite passed 2587. Do not hand-edit what it touches, and
   do not dry-run it again. It is a Python heredoc inside a shell script,
   so copy it **without adding indentation**.
2. **Log it at once**, as an `### Update — <date> (cuts done)` entry
   quoting the script's output, written with an edit tool.
3. **Findings** (decision 7) go in their own `### Findings — <date>`
   entry. The phase changes no production code, so the entry is the
   single line `none — the phase changes no production code`. Do not
   open `defs.rs`.
4. **Run the End-to-end block, then paste its output** into an
   `### Update — <date> (end-to-end verification)` entry, byte-exact, in
   a fenced block. **Known flake:**
   `read_shell_screen_lines_tails_the_visible_rows` (`FINDINGS.md` #65).
   If the block shows `passed=2586 failed=1`, run it once more and paste
   both outputs. Any other failure is real.
5. **Commit** as `chore:`.

### The script

```bash
python3 - <<'PY'
H = 'tests/hygiene.rs'
h = open(H).read()
old = '''    (
        "src/ai/tools/args.rs",
        "one argument struct per AI tool: flat data",
    ),
];'''
assert h.count(old) == 1
h = h.replace(old, '''    (
        "src/ai/tools/args.rs",
        "one argument struct per AI tool: flat data",
    ),
    (
        "src/ai/tools/defs.rs",
        "the `TOOLS` table, one `ToolDef` per AI tool: flat data",
    ),
];''')
open(H, 'w').write(h)
A = 'tests/hygiene_allowlist.txt'
al = open(A).read()
old = 'file src/ai/tools/defs.rs 1398\n'
assert al.count(old) == 1
open(A, 'w').write(al.replace(old, ''))
C = 'CLAUDE.md'
cm = open(C).read()
old = '`src/ai/types/pending.rs` and `src/ai/tools/args.rs` are exempt from both.'
assert cm.count(old) == 1, 'claude'
open(C, 'w').write(cm.replace(old, '`src/ai/types/pending.rs`, `src/ai/tools/args.rs` and `src/ai/tools/defs.rs` are exempt from both.'))
print('defs.rs exempt; allowlist file entries left:', sum(1 for l in open(A) if l.startswith('file ')))
PY
```

## Acceptance criteria

- [ ] `tests/hygiene.rs`'s `EXEMPT` names `src/ai/tools/defs.rs`, and
      `tests/hygiene_allowlist.txt` has no `file` entry.
- [ ] `cargo test --test hygiene` passes, and the End-to-end block prints
      `file_entries=0`, `defs_exempt=1`, `claude_md=1` and
      `passed=2587 failed=0`.
- [ ] The Update Log has a `(cuts done)` entry, a
      `### Findings — <date>` entry and an `(end-to-end verification)`
      entry, each under its own heading.

## End-to-end verification

```sh
bash <<'E2E'
A=/tmp/e2e-m31-34.txt; : > "$A"
grep -c '^file ' tests/hygiene_allowlist.txt | sed 's/^/file_entries=/' >> "$A"
grep -c '"src/ai/tools/defs.rs",' tests/hygiene.rs | sed 's/^/defs_exempt=/' >> "$A"
grep -c 'src/ai/tools/defs.rs` are exempt from both' CLAUDE.md | sed 's/^/claude_md=/' >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
E2E
```

Expected (from the dry run): `file_entries=0`, `defs_exempt=1`,
`claude_md=1`, hygiene `ok.`, clippy clean, `fmt_exit=0`,
`passed=2587 failed=0`.

## Authorizations

- Edit `tests/hygiene.rs`, `tests/hygiene_allowlist.txt` and `CLAUDE.md`,
  through the script only.

## Out of scope

- Any edit to `src/ai/tools/defs.rs`.
- The function entries: the next phases.

## Update Log

<!-- entries appended below this line -->

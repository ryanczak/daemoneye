# Phase 01 — cli-small-a

**Status:** todo
**Milestone:** M34 — Function caps
**Drafted:** 2026-10-31
**Depends on:** M33 (v2.6.0)
**Tags:** language=rust, kind=refactor, size=s

## Goal

Four functions, each just over the 150-line cap, are split until each is
at most 150 lines, and their four `fn` lines leave
`tests/hygiene_allowlist.txt`:

| Function | File | Lines today |
|---|---|---|
| `activity_row_in` | `src/cli/commands/top.rs` | 163 |
| `aggregate_costs` | `src/cli/commands/costs.rs` | 159 |
| `cmd_session` | `src/cli/commands/slash_session.rs` | 155 |
| `search_memory_fts` | `src/search.rs` | 154 |

Behaviour does not change. The suite stays at **exactly 2618** tests.

## Constraints

- **Lines are counted signature to closing brace** (`tests/hygiene.rs`);
  doc comments above the signature do not count. Run
  `cargo test --test hygiene` to see the gate's own numbers.
- **Extract, don't compress.** Move a coherent block (one match arm, one
  parsing step, one loop body) into a private helper in the same file, named
  for what it does. Joining lines, deleting comments, or squeezing the
  formatting to get under 150 is not a split, and `cargo fmt` would undo it
  anyway.
- **`src/search.rs` has the least room:** it is at most 784 lines in total,
  and the file cap is 800 production lines. Keep its net growth under 15
  lines. The other three files have over 150 lines of headroom.
- **Use `patch` with the whole block as `old_str`.** Do not use `patch_lines`
  on these files.
- **No test changes.** No test is added, edited or moved; a split that needs
  a test change is the wrong split.

## Spec

1. **Split the four functions** — for each, extract one or more helpers
   until `cargo test --test hygiene` no longer names it. Keep the public
   signatures (`aggregate_costs` is `pub`; `cmd_session` is `pub(super)`).
2. **Delete the four lines** from `tests/hygiene_allowlist.txt`, one per
   function, by its `fn <path> <name> <lines>` text. Touch no other line.
3. **Gates** — `cargo build`, `cargo clippy --all-targets --all-features --
   -D warnings`, `cargo fmt --all`, then `cargo test`. Done when the E2E block
   below prints what the criteria say.
4. **Findings** — one `### Findings — <date>` entry, from `git diff --stat`
   alone: one line per function naming the helpers you extracted, or `none`
   beyond that. Open no file for this step.
5. **End-to-end** — run the block once, paste its output fenced in an
   `### Update — <date> (end-to-end verification)` entry.
6. **Commit** as `refactor: split four CLI functions under the 150-line cap`.

## Acceptance criteria

- [ ] The E2E block's first line prints `allowlisted=0 allowlist_fn=36`
      (today `allowlisted=4 allowlist_fn=40`).
- [ ] hygiene and doc_truth pass; `build_warnings=0`; clippy clean;
      `fmt_exit=0`.
- [ ] `cargo test` reports exactly `passed=2618 failed=0`.
- [ ] `tests_changed=0`: no `#[test]` line added or removed in the diff.

## End-to-end verification

```sh
bash <<'E2E'
echo "allowlisted=$(grep -cE '^fn .* (activity_row_in|aggregate_costs|cmd_session|search_memory_fts) ' tests/hygiene_allowlist.txt) allowlist_fn=$(grep -c '^fn ' tests/hygiene_allowlist.txt)"
echo "tests_changed=$(git diff HEAD -- src | grep -cE '^[-+][[:space:]]*#\[(tokio::)?test')"
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
cargo test --test hygiene 2>&1 | grep -E '^test result'
cargo test --test doc_truth 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

Run it before committing: `tests_changed` compares the working tree with
`HEAD`.

## Authorizations

- Edit the four source files named in the Goal and
  `tests/hygiene_allowlist.txt` (deletions only).

## Out of scope

- Any other allowlisted function, and any behaviour change.

## Update Log

<!-- entries appended below this line -->

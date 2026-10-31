# Phase 02 — cli-small-b

**Status:** cancelled
**Milestone:** M34 — Function caps
**Drafted:** 2026-10-31
**Depends on:** phase 01 (done)
**Tags:** language=rust, kind=refactor, size=s

## Goal

Three CLI entry functions are split until each is at most 150 lines, and
their three `fn` lines leave `tests/hygiene_allowlist.txt`:

| Function | File | Lines today |
|---|---|---|
| `run_setup` | `src/cli/commands/setup.rs` | 256 |
| `async_main` | `src/main.rs` | 173 |
| `run_ask_raw` | `src/cli/commands/ask.rs` | 169 |

Behaviour does not change. The suite stays at **exactly 2618** tests.

## Constraints

- **Lines are counted signature to closing brace** (`tests/hygiene.rs`);
  doc comments above the signature do not count. `cargo test --test
  hygiene` prints the gate's own numbers.
- **Extract, don't compress.** Move a coherent block (one subcommand's arm,
  one setup step, one stream-handling loop) into a private helper in the same
  file, named for what it does. Joining lines or deleting comments is not a
  split. `run_setup` is the largest (256): it needs more than one helper.
- **`src/main.rs` has the least room:** 754 production lines of the 800
  cap. Keep its net growth under 40 lines. `setup.rs` and `ask.rs` are under
  300 lines.
- **`main()` stays synchronous.** The daemon forks before the tokio runtime
  starts (CLAUDE.md § Important Invariants). Split `async_main` only; do not
  move anything into or out of `main()`.
- **`run_setup` keeps its `pub` signature;** its callers are elsewhere.
- **Use `patch` with the whole block as `old_str`,** never `patch_lines`.
- **No test changes.** No test is added, edited or moved.

## Spec

1. **Split the three functions** — for each, extract helpers until
   `cargo test --test hygiene` no longer names it.
2. **Delete the three lines** from `tests/hygiene_allowlist.txt`, by their
   `fn <path> <name> <lines>` text. Touch no other line.
3. **Gates** — `cargo build`, `cargo clippy --all-targets --all-features --
   -D warnings`, `cargo fmt --all`, `cargo test`. Done when the E2E block
   prints what the criteria say.
4. **Findings** — one `### Findings — <date>` entry, from `git diff --stat`
   alone: one line per function naming its helpers and its new length, or
   `none` beyond that. Open no file for this step.
5. **End-to-end** — run the block once, before committing, and paste its
   output fenced in an `### Update — <date> (end-to-end verification)` entry.
6. **Commit** as `refactor: split three CLI entry functions under the 150-line cap`.

## Acceptance criteria

- [ ] The E2E block's first line prints `allowlisted=0 allowlist_fn=33`
      (today `allowlisted=3 allowlist_fn=36`).
- [ ] `tests_changed=0`.
- [ ] hygiene and doc_truth pass; `build_warnings=0`; clippy clean;
      `fmt_exit=0`.
- [ ] `cargo test` reports exactly `passed=2618 failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
echo "allowlisted=$(grep -cE '^fn .* (run_setup|async_main|run_ask_raw) ' tests/hygiene_allowlist.txt) allowlist_fn=$(grep -c '^fn ' tests/hygiene_allowlist.txt)"
echo "tests_changed=$(git diff HEAD -- src | grep -cE '^[-+][[:space:]]*#\[(tokio::)?test')"
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
cargo test --test hygiene 2>&1 | grep -E '^test result'
cargo test --test doc_truth 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- Edit the three source files named in the Goal and
  `tests/hygiene_allowlist.txt` (deletions only).

## Out of scope

- Any other allowlisted function, and any behaviour change.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-31 (cancelled)

Run 4e78800b stopped by `stop_phase` on the PE's instruction at turn 16,
stage `awaiting_model`, no file changed. The milestone was withdrawn; see
the README's Notes.

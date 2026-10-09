# Phase 01: helpers-out — move the generic helpers out of `src/tmux/`

**Milestone:** M27 — tmux becomes an optional observer; 2.0 close
**Status:** todo
**Depends on:** M26 (done)
**Tags:** language=rust, kind=refactor, size=s

## Goal

Several helpers live under `src/tmux/` but have nothing to do with tmux, and
non-tmux code depends on them. Later M27 phases shrink `src/tmux/` to a
read-only observer, so these must move first. This is a **pure move**: no
behaviour changes and no test changes beyond their paths.

The helpers to move:
- **`bounded_output`, `bounded_output_with`** (`src/tmux/mod.rs`): run a
  `Command` with a timeout and kill its process group on expiry. Used across
  the daemon, CLI and sudo code.
- **`annotate_output`** (`src/tmux/mod.rs`): used by
  `src/daemon/executor/shell_exec.rs`.
- **`src/tmux/status.rs`** (`PaneStatus`, `classify`, `is_shell_prompt`,
  `format_age`, `summarize`): `src/shell/screen.rs` and
  `src/cli/commands/slash.rs` use it, as well as the tmux cache.

## Spec

- Move `bounded_output`, `bounded_output_with` and `annotate_output`, with
  their tests, to a new module **`src/process.rs`**, declared in the crate
  root next to `util`.
- Move `src/tmux/status.rs`, with its tests, to **`src/shell/status.rs`**.
- Update every call site, **inside and outside** `src/tmux/`, to the new
  paths. **No re-exports from `crate::tmux`** for the moved items: a call
  site that still compiles through `tmux::` is the thing this phase removes.
- Update the `CLAUDE.md` Key files table: the `src/tmux/status.rs` row
  becomes `src/shell/status.rs`, and add a `src/process.rs` row. Keep each
  row's description.

Constraint: no behaviour change. Keep the functions' signatures and their
doc comments. Don't edit any test's body; only its path and `use` lines.

## Acceptance criteria

- [ ] `grep -rnE 'tmux::(bounded_output|annotate_output|status)' src/ tests/`
      prints nothing.
- [ ] `test -e src/tmux/status.rs` fails, and `src/process.rs` and
      `src/shell/status.rs` exist.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0;
      `cargo fmt --all --check` exits 0.
- [ ] *(Preservation)* `cargo test` reports exactly `passed=2300 failed=0`,
      the same as before the phase (2300 is the architect-measured baseline;
      the moved tests move, none are added or removed).
- [ ] *(Preservation)* `cargo test --test doc_truth` passes.
- [ ] The Update Log entry contains the § End-to-end verification output,
      every line as printed, and a line `PASTE MATCH: yes`.

## End-to-end verification

```sh
grep -rnE 'tmux::(bounded_output|annotate_output|status)' src/ tests/ ; echo "grep_done"
test -e src/tmux/status.rs; echo "old_status_exists=$?"
ls src/process.rs src/shell/status.rs
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test --test doc_truth 2>&1 | grep -E '^test result'
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
git diff --stat
```

Each line runs **once**. Expect **only** `test result:` summary lines from
the `cargo test` lines. rexyMCP's output filter removes the individual
`test … ok` lines. `old_status_exists=1` is the pass value.

## Authorizations

- [x] May edit any file that imports a moved item, and the `CLAUDE.md` Key
      files table.

## Out of scope

Anything else in `src/tmux/`: the cache, panes, windows, sessions and ANSI
handling stay where they are for now.

## Update Log

<!-- entries appended below this line -->

### Notes for executor — 2026-10-09 (refined re-dispatch)

The previous run edited nothing. It spent its turns measuring a "before"
test count, grepping the full `cargo test` output for `FAILED`, then looped.
**Do not measure a baseline.** It is `passed=2300 failed=0`, measured by the
architect on this tree. Only the `test result:` summary lines matter; do not
grep the full test output.

Start with the move itself: create `src/process.rs` and `src/shell/status.rs`,
move the items, then fix the call sites by following `cargo build` errors.

### Update — 2026-10-09 (escalation)

**Chosen lever:** refined re-dispatch
**Rationale:** the stall came from the spec's "same count as before"
criterion, which sent the executor to derive a baseline instead of doing the
move. Stating the baseline removes it, and there is no partial work to resume.

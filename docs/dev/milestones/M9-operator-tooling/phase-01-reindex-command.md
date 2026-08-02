# Phase 01: Reindex Command

**Milestone:** M9 — Operator Tooling
**Status:** todo
**Depends on:** none (first phase of M9; M8 closed 2026-08-02)
**Estimated diff:** ~130 lines — one new file `src/cli/commands/reindex.rs`,
plus four wiring lines across `src/main.rs` and `src/cli/commands/mod.rs`.

**Tags:** language=rust, kind=feature, size=m

## Goal

`reconcile_index()` can rebuild the memory index, and nothing an operator can
run calls it. Add `daemoneye reindex`.

## Architecture references

- `src/memory/index.rs:231` `reconcile_index() -> anyhow::Result<ReconcileReport>`
  — the function being exposed. **Do not modify it.**
- `src/cli/commands/audit_prompts.rs:173` `run_audit_prompts()` — the operator
  subcommand shape this copies.
- `src/main.rs:126-131` and `:484-486` — the variant declaration and dispatch arm
  to copy.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any file.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

`reconcile_index()` has exactly one caller — the reconcile-on-empty branch of
`fts5_search()`, which fires only when the index has **zero** rows. A *stale*
index (rows present but wrong) is therefore unreachable by any existing code
path, and unfixable by an operator short of deleting `var/index/memory.db` by
hand.

The function's contract, already built and tested:

```rust
pub struct ReconcileReport {
    /// Rows present in the index at the start of the pass.
    pub rows_before: usize,
    /// Rows present after the rebuild.
    pub rows_after: usize,
}

pub fn reconcile_index() -> anyhow::Result<ReconcileReport>
```

### Measured before this spec was written

`reconcile_index()` was called directly against three trees:

| Tree | Result |
|---|---|
| Bare `$HOME`, no `~/.daemoneye/` | `before=0 after=0`, returns `Ok` |
| Freshly `daemoneye setup` `$HOME` | `before=0 after=9` |
| Same tree, immediately again | `before=9 after=9` |

So it needs no daemon, tolerates a completely empty tree without erroring, and
is idempotent. The command inherits all three; it adds no logic of its own.

**The rebuild is atomic** — `DELETE FROM memories` and every re-insert share one
transaction (`src/memory/index.rs:254`-`:311`). A running daemon serving a search
sees the old index or the new one, never a half-empty one. **The command is
therefore safe to run without stopping the daemon**, and the help text should
say so.

### The report cannot prove content equality

`ReconcileReport` carries counts. Replacing nine stale rows with nine correct
ones reads `9 → 9`. The rebuild still repaired the index; the numbers just
cannot show it. **Word the output so it never claims the index was already
correct** — see spec task 2.

## Spec

### 1. The subcommand — copy the `AuditPrompts` shape exactly

In `src/main.rs`, add a variant to `enum Commands` immediately after
`AuditPrompts` (`:131`). The existing one, for shape:

```rust
    /// Audit installed prompt and knowledge memory files for stale path references.
    ///
    /// Reads the files directly from ~/.daemoneye/ and checks every path literal
    /// against the known inventory. Exits non-zero if any path is superseded or
    /// unknown. Never writes or modifies any file.
    AuditPrompts,
```

Add:

```rust
    /// Rebuild the memory search index from the memory files on disk.
    ///
    /// The index at ~/.daemoneye/var/index/memory.db is a derived cache; this
    /// discards it and rebuilds from every memory file, reporting the row count
    /// before and after. Safe to run while the daemon is up — the rebuild is a
    /// single transaction, so a concurrent search sees the old index or the new
    /// one, never a partial one. Needs no daemon and never modifies a memory file.
    Reindex,
```

Clap derives the kebab-case name, so this is invoked as `daemoneye reindex`.

And the dispatch arm, next to the existing one at `:484`:

```rust
        Commands::Reindex => {
            cli::run_reindex();
        }
```

### 2. `src/cli/commands/reindex.rs`

Two items — the CLI entry, and a **pure** formatter so the wording is testable
without running a rebuild.

```rust
//! `daemoneye reindex` — rebuild the derived memory index from disk.

use crate::memory::index::{ReconcileReport, reconcile_index};
use std::io::{self, Write};
use std::process;

/// Render the operator-facing report. Pure, so the wording is unit-testable.
fn format_report(report: &ReconcileReport) -> String
```

`format_report` returns, for the three cases:

| Condition | Line |
|---|---|
| `rows_after > rows_before` | `Index rebuilt: 3 → 9 rows (6 added).` |
| `rows_after < rows_before` | `Index rebuilt: 9 → 3 rows (6 removed).` |
| `rows_after == rows_before` | `Index rebuilt: 9 rows (count unchanged — the rebuild still replaced every row).` |

**That third wording is load-bearing.** It must not say "already up to date" or
"no changes needed", because the count matching does not mean the content did.
An operator who ran this to fix a suspected problem needs to know the rebuild
happened.

`run_reindex()`:

```rust
pub fn run_reindex() {
    match reconcile_index() {
        Ok(report) => {
            println!("{}", format_report(&report));
            let _ = io::stdout().flush();
        }
        Err(e) => {
            eprintln!("\x1b[31mReindex failed: {e:#}\x1b[0m");
            let _ = io::stderr().flush();
            process::exit(1);
        }
    }
}
```

**Exit 0 on a successful rebuild regardless of whether the count changed.** A
rebuild is the requested action, not a finding — this differs deliberately from
`run_audit_prompts`, which exits 1 when it *finds* something. Only a genuine
error (unwritable index, unreadable memory dir) is non-zero.

`{e:#}` prints the full `anyhow` context chain; the codebase uses that form
throughout (e.g. the index hooks in `src/memory.rs`).

### 3. Wire the module

In `src/cli/commands/mod.rs`, add to the `mod` block (alphabetical, after
`pane`):

```rust
mod reindex;
```

and to the `pub use` block:

```rust
pub use reindex::run_reindex;
```

`src/cli/mod.rs:13` is `pub use commands::*;`, so `cli::run_reindex` resolves
with no further change.

### 4. Tests

Add a `#[cfg(test)] mod tests` to `src/cli/commands/reindex.rs`. All three drive
`format_report` directly — pure, no `$HOME`, no rebuild:

- `report_wording_when_rows_grow` — `{rows_before: 3, rows_after: 9}` produces a
  string containing `3 → 9` and `6 added`.
- `report_wording_when_rows_shrink` — `{rows_before: 9, rows_after: 3}` contains
  `9 → 3` and `6 removed`.
- `report_wording_when_count_is_unchanged_does_not_claim_no_op` — with
  `{rows_before: 9, rows_after: 9}`, assert the output contains `9 rows` **and**
  that it does **not** contain `up to date` or `no changes`. This is the negative
  case that protects the wording decision in task 2 from being "simplified" later.

**Do not add tests that call `reconcile_index()`.** Its behaviour is already
covered — `reconcile_rebuilds_from_disk`,
`reconcile_after_incremental_writes_is_a_no_op`, and
`fresh_index_is_reconciled_on_first_search` in `src/memory/index.rs`. Duplicating
them here would add runtime and cover nothing new. The CLI glue is proved by the
end-to-end block below, which runs the real binary.

## Acceptance criteria

- [ ] `daemoneye reindex` exists and appears in `daemoneye --help`.
- [ ] On a freshly `setup` `$HOME` it reports **`0 → 9`** and exits **0**.
- [ ] Run immediately again on the same tree it reports **9 rows, count
      unchanged**, and exits **0** — and the wording does **not** contain
      `up to date` or `no changes`.
- [ ] On a bare `$HOME` with no `~/.daemoneye/` it reports **0 rows** and exits
      **0** — an empty tree is not an error.
- [ ] It runs with **no daemon started** in every case above.
- [ ] `reconcile_index()` in `src/memory/index.rs` is **unchanged** —
      `git diff --name-only` does not list that file.
- [ ] The three tests named in spec task 4 pass.
- [ ] `cargo build` zero new warnings; `cargo clippy --all-targets
      --all-features -- -D warnings` exits 0; `cargo fmt --all` leaves the tree
      unchanged.
- [ ] `cargo test` passes. Lib count rises by 3 (**1035**); integration **30**
      (2 ignored), isolation **9** (1 ignored), `bug_tracker` **6**,
      `doc_truth` **1**.
- [ ] Only `src/main.rs`, `src/cli/commands/mod.rs` and the new
      `src/cli/commands/reindex.rs` change.

## Test plan

Spec task 4 covers the wording. The **end-to-end block is the real proof**,
because it runs the shipped binary: a CLI subcommand that is declared but not
dispatched, or dispatched to the wrong function, passes every unit test and
fails the moment anyone types the command.

**What would make this phase a false success:** unit tests on `format_report`
alone, with the subcommand declared but never wired into the dispatch `match`.
Rust will not warn — the arm is simply absent and clap reports an unrecognised
subcommand at runtime. The E2E block invoking `./target/debug/daemoneye reindex`
three times is what catches it.

A second: softening the unchanged-count wording to "already up to date". That
reads better and is **false** — the rebuild replaced every row, and the count
matching proves nothing about content.
`report_wording_when_count_is_unchanged_does_not_claim_no_op` pins it.

## End-to-end verification

Run this block verbatim and paste the resulting file into your Update Log.

**Two constraints carried from an earlier post-mortem:** **no heredocs**, and
every long-running command wrapped in `timeout`. An earlier E2E block nested a
`python3` heredoc that hung and orphaned two processes at 100% CPU for 70
minutes. Do not reintroduce either pattern.

```bash
cd /home/matt/src/daemoneye
cargo build 2>&1 | tail -2
H=$(mktemp -d)
B=$(mktemp -d)
{
  echo "=== the subcommand is registered ==="
  timeout 30 ./target/debug/daemoneye --help 2>&1 | grep -c "reindex"
  echo "help-mentions-reindex-above-must-be-at-least-1"

  echo "=== bare HOME, no ~/.daemoneye at all, no daemon ==="
  HOME="$B" timeout 60 ./target/debug/daemoneye reindex
  echo "bare-exit=$?   # 0 == PASS"

  echo "=== seeded HOME: first run should report 0 -> 9 ==="
  HOME="$H" timeout 120 ./target/debug/daemoneye setup 2>&1 | tail -1
  HOME="$H" timeout 60 ./target/debug/daemoneye reindex
  echo "first-exit=$?   # 0 == PASS"

  echo "=== second run: unchanged count, and must NOT claim a no-op ==="
  HOME="$H" timeout 60 ./target/debug/daemoneye reindex > /tmp/reindex2.txt 2>&1
  echo "second-exit=$?   # 0 == PASS"
  cat /tmp/reindex2.txt
  timeout 30 grep -ci "up to date\|no changes" /tmp/reindex2.txt
  echo "forbidden-wording-count-above-must-be-0"

  echo "=== reconcile_index was not modified ==="
  timeout 30 git diff --name-only HEAD -- src/memory/index.rs | wc -l
  echo "index-rs-changed-above-must-be-0"

  echo "=== full gate ==="
  timeout 900 cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -2
  echo "clippy-exit=$?"
  timeout 900 cargo test 2>&1 | grep -E "^test result"
} > /tmp/m9-phase01-e2e.txt 2>&1
rm -rf "$H" "$B"
cat /tmp/m9-phase01-e2e.txt
```

The lib line must read **1035**.

Paste the captured file into an Update Log entry titled
`### Update — <date> (end-to-end verification)`. **The server-authored
`(complete)` entry does not satisfy this** — its "Command output tails" block is
the automatic gate capture every phase receives, and it never invokes the new
subcommand at all.

**If any part of the capture block fails or hangs, stop and report it as a
blocker.** Do not re-run the surviving sections separately and paste the
result — a transcript assembled from more than one run fails `STANDARDS.md` §1
even when every claim in it is true.

## Authorizations

- [ ] May add dependencies: **none**.
- [ ] May touch `docs/architecture.md`: no.
- [ ] May touch `CLAUDE.md`: no. (The new subcommand is worth a line there
      eventually, but this phase does not touch it — see Out of scope.)
- [ ] May create new files: **yes, exactly one** —
      `src/cli/commands/reindex.rs`.

## Out of scope

- **Modifying `reconcile_index()` or anything else in `src/memory/index.rs`.**
  An acceptance criterion pins the file as unchanged. The function already does
  what is needed; this phase exposes it.
- **A startup hook that reconciles on every daemon boot.** It would walk every
  memory file on every start — the cost two earlier phases deferred. The command
  is the cheap answer; if a boot-time refresh is wanted later it is its own
  decision.
- **A `--check` / dry-run mode.** Telling an operator whether the index is stale
  *without* rebuilding needs a comparison the current API cannot do, since
  `ReconcileReport` counts rows rather than diffing content. Worth its own phase
  if asked for.
- **`--json` or `--quiet` output.** One human-readable report.
- **Documenting the command in `CLAUDE.md` or `docs/architecture.md`.** Both are
  worth a line, and both are cheap to add in a later docs pass. Keeping this
  phase to code plus its own tests keeps the diff reviewable and avoids
  colliding with the `doc_truth` tripwire.
- **`daemoneye status` reporting index row counts.** A reasonable operator
  affordance and a different feature.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

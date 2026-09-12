# Phase 10: `daemoneye run list` and the chat slash commands

**Milestone:** M24 — Run Manager
**Status:** todo
**Depends on:** phase-02a (`read_turn_log`, `turn_log_file`), phase-02b
(`run_log_tail`), phase-04 (`request_stop`) and phase-09b (the `Run` CLI
group). All **done**.
**Estimated diff:** ~400 lines incl. tests
**Tags:** language=rust, kind=feature, size=m

## Goal

M24's remaining exit clause: **"`daemoneye runs` lists every kind."** Nine
phases have written `var/log/runs/<id>.jsonl` for chat, ghost, scheduled and
delegated runs, and **nothing reads that directory** — every earlier phase
addressed a run by an id the user had to find by hand. `daemoneye run resume`
(phase-09b) shipped with that limitation recorded in its Out of scope.

This phase supplies the discovery surface:

1. `run/list.rs` — a `RunSummary` per log, a `list_runs()` that enumerates the
   directory, and a pure table renderer.
2. `daemoneye run list` — the CLI, reading the files directly with **no daemon
   required**, exactly as `daemoneye stop <run>` does.
3. `/runs`, `/run <id>` and `/stop <id>` in chat — the same listing, one run's
   log tail, and a stop request. The last two are thin wrappers over
   `run_log_tail` and `request_stop`, which already exist.

**Naming note.** The exit clause says `daemoneye runs`, but phase-09b
established a `Run` subcommand group (`daemoneye run resume`). A second
top-level `runs` beside `run` would be two spellings of one noun, so the
command is **`daemoneye run list`**. The milestone criterion is met in
substance; record the wording deviation in the Update Log.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.3 — the run manager surfaces.
- M24 README § exit criteria, "**One `Run` for four kinds** … `daemoneye runs`
  lists every kind."

## Pre-flight

Confirm on the current tree (values as of drafting, 2026-09-12, commit
`bb5eb80`):

```sh
git status --porcelain | wc -l                                        # 0
cargo test --lib 2>&1 | grep '^test result'                           # 1803 passed
cargo test --lib daemon::run::log_read 2>&1 | grep '^test result'     # 11 passed
cargo test --lib daemon::run::resume 2>&1 | grep '^test result'       # 20 passed
cargo test --lib cli::commands::slash 2>&1 | grep '^test result'      # 8 passed
test -f src/daemon/run/list.rs && echo 1 || echo 0                    # 0
grep -c '"/runs"' src/cli/commands/slash.rs                           # 0
```

## Current state

### The log file, and how to find one

```rust
// src/daemon/run/turn_log.rs:50
pub fn turn_log_file(id: &RunId) -> PathBuf {
    crate::config::runs_dir().join(format!("{}.jsonl", id))
}
```

`runs_dir()` is `src/config/load.rs:113` (`var/log/runs`). So enumeration is:
read the directory, take each entry whose name ends `.jsonl`, strip the suffix,
and `RunId::parse` the stem — **a name that does not parse is skipped, not an
error**. That is the negative case: `runs_dir()` is a directory on a user's
disk and may hold anything.

### The summary is one step from code that already exists

`terminal_outcome` (`src/daemon/run/resume.rs`) already walks records for the
**first** `RunStarted` and the **last** `RunFinished`. This phase needs the same
walk but keeps the `RunState` rather than the briefing:

```rust
// the shape to mirror — first RunStarted, last RunFinished
    let kind = records.iter().find_map(|r| match &r.event {
        TurnLogEvent::RunStarted { kind } => Some(kind.clone()),
        _ => None,
    })?;
```

Do **not** call `terminal_outcome` — it returns `(RunKind, Option<Briefing>)`
and its errors are resume's, not listing's. Write the walk in `list.rs`.

### `TurnLogEvent` and `RunState`

```rust
// src/daemon/run/turn_log.rs:38-47
pub enum TurnLogEvent {
    RunStarted { kind: RunKind },
    TurnStarted { turn: u32 },
    Note { text: String },
    RunFinished { state: RunState },
}
```

```rust
// src/daemon/run/mod.rs:303
pub enum RunState { Running, Complete, HardFail(Briefing), BudgetExceeded(Briefing), Cancelled { stage: String, turns: u32 } }
```

`RunKind::label()` (`src/daemon/run/mod.rs:81`) gives `"chat"` / `"ghost"` /
`"scheduled"` / `"delegated"` for the table's Kind column.

### The slash dispatch table — add three arms

```rust
// src/cli/commands/slash.rs:57-66
    match cmd {
        "/refresh" => cmd_refresh(ctx.renderer).await,
        "/model" | "/models" => cmd_model(ctx, rest).await,
        "/pane" | "/panes" => cmd_pane(ctx, rest).await,
        "/approvals" | "/approval" => cmd_approvals(ctx, rest),
        "/prompt" => cmd_prompt(ctx, rest),
        "/limits" => cmd_limits(ctx.renderer, ctx.session_id).await,
        "/session" | "/sessions" => cmd_session(ctx, rest).await,
        _ => return SlashOutcome::NotACommand,
    }
```

Note `cmd_approvals` and `cmd_prompt` are **not** `async` and take no daemon
round trip — the three new commands are the same shape: they read and write
files under `$HOME`, never the socket. Commit output with the existing `note`
helper (`slash.rs:73`).

### What the two thin commands wrap

```rust
// src/daemon/run/log_read.rs:90
pub fn run_log_tail(run_id: &RunId, limit: Option<u64>) -> String;
// src/daemon/run/cancel.rs:165
pub fn request_stop(raw_id: &str) -> anyhow::Result<PathBuf>;
```

`run_log_tail` already clamps its limit and returns a rendered string;
`request_stop` already validates the id and writes the sentinel. **Do not
re-implement either** — a criterion pins that neither `read_turn_log` nor
`stop_sentinel_path` is called from the slash module.

## Spec

Seven tasks. **Task 6 is the mutation checks — a numbered Spec task**, because
in this milestone a phase's `## Mutation checks` went unrun when it sat outside
`## Spec`.

### Task 1 — `src/daemon/run/list.rs`: the summary

```rust
/// One row of `daemoneye run list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSummary {
    pub id: RunId,
    pub kind: RunKind,
    /// The state the run's last `RunFinished` recorded, or `Running` when it
    /// has none — a run whose log has no finish record is still in flight.
    pub state: RunState,
    /// The highest `TurnStarted` turn the log carries; 0 when none.
    pub turns: u32,
}

/// Summarise one run's records. `None` when the log carries no `RunStarted` —
/// without it there is no kind, and the file is not a run log we can describe.
pub fn summarize(id: &RunId, records: &[TurnLogRecord]) -> Option<RunSummary>
```

Pure — takes records, does no I/O. `state` is the **last** `RunFinished`'s
state, or `RunState::Running` when absent. `turns` is the **maximum**
`TurnStarted { turn }`, not the count, so a log appended to twice still reports
the furthest turn reached.

### Task 2 — `list_runs`: enumerate the directory

```rust
/// Every run log in `var/log/runs/`, newest first by file modification time.
/// Unparseable names and unreadable files are skipped, not reported.
pub fn list_runs() -> Vec<RunSummary>
```

- Read `crate::config::runs_dir()`; a missing directory yields an empty vec,
  **not** an error.
- For each entry: the name must end `.jsonl` and its stem must `RunId::parse`.
  Anything else is skipped — `README.md`, `notes.txt`, a directory, a
  `.jsonl.tmp`. Pin the skip as a named test.
- `read_turn_log(&id)` then `summarize`; skip a `None`.
- Sort newest first. Use the file's modification time; on a metadata error sort
  that entry last rather than failing the listing.

### Task 3 — `render_runs_table`: pure text

```rust
/// The table `daemoneye run list` and `/runs` both print.
pub fn render_runs_table(runs: &[RunSummary]) -> String
```

- Empty input renders exactly `no runs recorded yet\n` — a criterion and a test
  pin that literal, because "prints nothing" and "prints an empty table" are
  both worse for a user who typed the command.
- Otherwise a header line `ID  KIND  STATE  TURNS` and one row per run, columns
  space-aligned. Render `state` as a short word: `running`, `complete`,
  `hard-fail`, `budget-exceeded`, `cancelled`. Render `kind` via
  `RunKind::label()`.
- **Pure**: takes the slice, returns the string, touches no filesystem.

### Task 4 — `daemoneye run list`

Add `List` to the `RunCommands` enum in `src/main.rs` (which phase-09b created
with a single `Resume` variant) and a dispatch arm that prints
`render_runs_table(&list_runs())`. No daemon connection — this reads files, like
`run_stop_run` (`src/cli/commands/lifecycle.rs:80`).

### Task 5 — `/runs`, `/run <id>`, `/stop <id>`

Three arms in the `match cmd` block at `src/cli/commands/slash.rs:57`:

- `"/runs"` → commit `render_runs_table(&list_runs())`.
- `"/run"` → `rest` is the id. Empty `rest` → note `usage: /run <id>`. A
  `RunId::parse` failure → note `not a run id: <rest>`. Otherwise commit
  `run_log_tail(&id, None)`.
- `"/stop"` → `rest` is the id. Empty → note `usage: /stop <id>`. Otherwise
  call `request_stop(rest)` and note either `stop requested for <id>` or the
  error. **`request_stop` already validates the id**; do not pre-validate.

All three are **sync** (like `cmd_approvals`), take no socket, and return
`SlashOutcome::Handled`.

### Task 6 — run the mutation checks and record them

Run both mutations in § Mutation checks with the `patch` tool, capture each
red-then-green transcript, and paste them into the Update Log. **If a mutation
does not turn its test red, record that observation instead — do not invent a
failure.** Note also that a mutation instruction names a line that may not
exist in your implementation; if so, say which property you broke instead.

### Task 7 — capture the end-to-end evidence

Run § End-to-end verification verbatim, then paste `/tmp/e2e-m24-10.txt` into a
new Update Log entry headed `### Update — <date> (end-to-end verification)`.
Anything presented as command output must be command output, **including line
numbers**.

## Acceptance criteria

Each was run against the current tree and returns the "now" value shown.
`prod:` counts use `awk '/^#\[cfg\(test\)\]/{exit} {print}' <file> |`.

- [ ] **The module exists and is wired:**
      `test -f src/daemon/run/list.rs && echo 1 || echo 0` → **1** (now `0`);
      `grep -c '^mod list;' src/daemon/run/mod.rs` → **1** (now `0`);
      `grep -c 'pub use list::' src/daemon/run/mod.rs` → **1** (now `0`).
- [ ] **Declared once each**, prod on `src/daemon/run/list.rs`:
      `grep -c 'pub struct RunSummary'` → **1**; `grep -c 'pub fn summarize'` →
      **1**; `grep -c 'pub fn list_runs'` → **1**;
      `grep -c 'pub fn render_runs_table'` → **1** (each **0** now).
- [ ] **`summarize` and `render_runs_table` are pure:**
      `sed -n '/pub fn summarize/,/^}$/p' src/daemon/run/list.rs | grep -cE 'read_turn_log|std::fs|runs_dir'`
      → **0**, and the same over `render_runs_table` → **0**.
- [ ] **`list_runs` is the only reader:** prod
      `grep -c 'read_turn_log' src/daemon/run/list.rs` → **1**, and
      `grep -c 'runs_dir' src/daemon/run/list.rs` → **1**.
- [ ] **`terminal_outcome` was not reused:** prod
      `grep -c 'terminal_outcome' src/daemon/run/list.rs` → **0**.
- [ ] **The empty listing has a literal message:**
      `grep -c 'no runs recorded yet' src/daemon/run/list.rs` → **1 or more**.
- [ ] **The CLI gained exactly one subcommand:**
      `awk '/^enum RunCommands/,/^}/' src/main.rs | grep -cE '^    [A-Z]'` →
      **2** (now **1** — `Resume`), and `grep -c 'RunCommands::List' src/main.rs`
      → **1** (now `0`).
- [ ] **The three slash commands are dispatched:**
      `grep -c '"/runs"' src/cli/commands/slash.rs` → **1**;
      `grep -c '"/run"' src/cli/commands/slash.rs` → **1**;
      `grep -c '"/stop"' src/cli/commands/slash.rs` → **1** (each `0` now).
- [ ] **The slash module does not re-implement what it wraps:**
      `grep -c 'read_turn_log' src/cli/commands/slash.rs` → **0**, and
      `grep -c 'stop_sentinel_path' src/cli/commands/slash.rs` → **0**.
- [ ] **No daemon round trip in the new slash arms:** inside each of
      `cmd_runs`, `cmd_run` and `cmd_stop`,
      `awk '/fn cmd_runs|fn cmd_run\b|fn cmd_stop/,/^}$/' src/cli/commands/slash.rs | grep -c 'request_once'`
      → **0**. *(Every existing daemon-backed command uses `request_once`; the
      three new ones read `$HOME` directly.)*
- [ ] No `unsafe`/`unwrap`/`expect`/`panic!` in new production code: prod on
      `src/daemon/run/list.rs` → **0**.
- [ ] `cargo test --lib daemon::run::list` → **11 or more** passing, `0 failed`
      (now **0** — no such module). *Arithmetic: the 11 in the Test plan.*
- [ ] `cargo test --lib cli::commands::slash` → **8**, unchanged — the new arms
      are covered by `daemon::run::list`'s tests and the § D greps, not by a
      slash-module test that would need a renderer.
- [ ] **Preservation, each must not move:** `daemon::run::log_read` **11**,
      `daemon::run::resume` **20**, `daemon::run::turn_log` **7**,
      `daemon::run::guard` **8**, `daemon::ghost` **21**, `runbook::` **15**.
- [ ] **Nothing leaks into the developer's real HOME:**
      `ls ~/.daemoneye/var/log/runs/ 2>/dev/null | wc -l` → **0**, unchanged by
      a full suite run.
- [ ] Full lib suite **1814 or more** passing, `0 failed`, on **three
      consecutive runs** (now **1803**). *Arithmetic: 1803 + 11 = 1814.*
- [ ] All four gates pass.

## Test plan

**`src/daemon/run/list.rs` (11 new).** Seven are pure; four touch HOME and take
`crate::test_home_guard()` + a tempdir — reuse
`crate::daemon::run::cancel::test_support::{HomeGuard, fresh_run_id}`, as
`daemon::run::resume`'s tests do. **`list_runs` reads a process-global
directory and libtest runs in parallel — every HOME-touching test needs its own
guard, and must assert only about ids it wrote.**

`summarize` (pure):

- `summarize_reads_the_kind_and_the_terminal_state`
- `summarize_without_a_run_started_is_none` — **negative case.**
- `summarize_without_a_run_finished_reports_running` — an in-flight run.
- `summarize_takes_the_highest_turn_not_the_count` — `TurnStarted` 1, 2, 5 →
  `turns == 5`, not 3.

`render_runs_table` (pure):

- `render_runs_table_empty_says_no_runs` — asserts the literal
  `no runs recorded yet`.
- `render_runs_table_lists_every_kind` — one row each for chat, ghost,
  scheduled, delegated; assert all four `label()` words appear. *This is the
  milestone's "lists every kind" criterion, as a test.*
- `render_runs_table_names_each_terminal_state` — a run per state; assert
  `running`, `complete`, `hard-fail`, `budget-exceeded`, `cancelled` all
  appear.

`list_runs` (HOME):

- `list_runs_on_a_missing_directory_is_empty` — **negative case**: no
  `var/log/runs` at all returns an empty vec, not a panic.
- `list_runs_reads_a_written_log`
- `list_runs_skips_a_name_that_is_not_a_run_id` — **the load-bearing
  negative**: write `notes.txt` and `not-a-run-id.jsonl` beside a real log;
  assert only the real one is listed.
- `list_runs_skips_a_log_with_no_run_started` — **negative case**: a `.jsonl`
  whose records are only `TurnStarted` is skipped rather than listed with a
  guessed kind.

**No test drives the slash commands** — they need a live `RatatuiRendererStdout`.
Their bodies are three calls into functions tested here; the § D greps pin the
wiring. Say so in the Update Log rather than building a renderer harness.

## Mutation checks

Two, run as **Spec task 6**. Use the `patch` tool — **`sed -i`, `perl -i` and
`> file` are forbidden by the executor contract and `bash` refuses them.** Name
the assertion message in each failure.

1. **A non-run-id file must not be listed.** Make `list_runs` accept any
   `.jsonl` name (skip the `RunId::parse` check, or default a failed parse to a
   placeholder id) → `list_runs_skips_a_name_that_is_not_a_run_id` must go
   **red**.
2. **`turns` is the highest turn, not the count.** Make `summarize` count
   `TurnStarted` records instead of taking their maximum →
   `summarize_takes_the_highest_turn_not_the_count` must go **red** showing 3
   where 5 was expected.

## End-to-end verification

The real artifact is `var/log/runs/` read back through the public listing. §B
exercises it in a throwaway HOME, including the skip cases.

Run from the repo root, verbatim:

```sh
A=/tmp/e2e-m24-10.txt
: > "$A"
RUNS_BEFORE=$(ls ~/.daemoneye/var/log/runs/ 2>/dev/null | wc -l)
{
echo "== A. build =="
echo "real HOME run logs BEFORE: $RUNS_BEFORE"
cargo build 2>&1 | tail -2; echo "cargo_exit=${PIPESTATUS[0]}"

echo "== B. the listing module =="
cargo test --lib daemon::run::list -- --nocapture 2>&1 \
  | sed 's/\x1b\[[0-9;]*m//g' | grep -E '^test daemon::run::list|^test result'
echo "cargo_exit=${PIPESTATUS[0]}"

echo "== C. the CLI surface exists =="
cargo run --quiet -- run --help 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | head -12
echo "cargo_exit=${PIPESTATUS[0]}"

echo "== D. structural greps (each must print the stated number) =="
P() { awk '/^#\[cfg\(test\)\]/{exit} {print}' src/daemon/run/list.rs | grep -c "$1"; }
printf 'list.rs exists             (1): %s\n' "$(test -f src/daemon/run/list.rs && echo 1 || echo 0)"
printf 'mod list;                  (1): %s\n' "$(grep -c '^mod list;' src/daemon/run/mod.rs)"
printf 'pub use list::             (1): %s\n' "$(grep -c 'pub use list::' src/daemon/run/mod.rs)"
printf 'RunSummary                 (1): %s\n' "$(P 'pub struct RunSummary')"
printf 'summarize                  (1): %s\n' "$(P 'pub fn summarize')"
printf 'list_runs                  (1): %s\n' "$(P 'pub fn list_runs')"
printf 'render_runs_table          (1): %s\n' "$(P 'pub fn render_runs_table')"
printf 'summarize is pure          (0): %s\n' "$(sed -n '/pub fn summarize/,/^}$/p' src/daemon/run/list.rs | grep -cE 'read_turn_log|std::fs|runs_dir')"
printf 'render is pure             (0): %s\n' "$(sed -n '/pub fn render_runs_table/,/^}$/p' src/daemon/run/list.rs | grep -cE 'read_turn_log|std::fs|runs_dir')"
printf 'one read_turn_log          (1): %s\n' "$(P 'read_turn_log')"
printf 'one runs_dir               (1): %s\n' "$(P 'runs_dir')"
printf 'terminal_outcome unused    (0): %s\n' "$(P 'terminal_outcome')"
printf 'empty message literal    (>=1): %s\n' "$(grep -c 'no runs recorded yet' src/daemon/run/list.rs)"
printf 'RunCommands variants       (2): %s\n' "$(awk '/^enum RunCommands/,/^}/' src/main.rs | grep -cE '^    [A-Z]')"
printf 'RunCommands::List          (1): %s\n' "$(grep -c 'RunCommands::List' src/main.rs)"
printf 'slash /runs                (1): %s\n' "$(grep -c '\"/runs\"' src/cli/commands/slash.rs)"
printf 'slash /run                 (1): %s\n' "$(grep -c '\"/run\"' src/cli/commands/slash.rs)"
printf 'slash /stop                (1): %s\n' "$(grep -c '\"/stop\"' src/cli/commands/slash.rs)"
printf 'slash no read_turn_log     (0): %s\n' "$(grep -c 'read_turn_log' src/cli/commands/slash.rs)"
printf 'slash no sentinel path     (0): %s\n' "$(grep -c 'stop_sentinel_path' src/cli/commands/slash.rs)"
printf 'new arms hit no daemon     (0): %s\n' "$(awk '/fn cmd_runs|fn cmd_run\b|fn cmd_stop/,/^}$/' src/cli/commands/slash.rs | grep -c 'request_once')"
printf 'list.rs unwrap/panic       (0): %s\n' "$(P '\.\(unwrap\|expect\)(\|panic!(\|unsafe')"

echo "== E. preservation: log_read 11, resume 20, turn_log 7, guard 8, ghost 21, runbook 15, slash 8 =="
for m in daemon::run::log_read daemon::run::resume daemon::run::turn_log daemon::run::guard daemon::ghost 'runbook::' cli::commands::slash; do
  printf '%-28s ' "$m"
  cargo test --lib "$m" 2>&1 | grep '^test result'
done

echo "== F. lib suite, three consecutive runs =="
for i in 1 2 3; do cargo test --lib 2>&1 | grep '^test result'; done

echo "== G. gates =="
cargo fmt --all --check 2>&1 | tail -2; echo "fmt_exit=${PIPESTATUS[0]}"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -2; echo "clippy_exit=${PIPESTATUS[0]}"

echo "== H. no HOME litter from the suite =="
printf 'real HOME run logs AFTER (==BEFORE): %s\n' "$(ls ~/.daemoneye/var/log/runs/ 2>/dev/null | wc -l)"
} >> "$A" 2>&1
cat "$A"
```

Then the paste self-check:

```sh
D=docs/dev/milestones/M24-run-manager/phase-10-run-listing.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m24-10.txt
diff /tmp/pasted-m24-10.txt /tmp/e2e-m24-10.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

The executor may modify **only**:

- `src/daemon/run/list.rs` — new file, tasks 1–3 and the Test plan.
- `src/daemon/run/mod.rs` — one `mod list;` line and one `pub use` line.
- `src/main.rs` — task 4 **only**: the `List` variant and one dispatch arm.
- `src/cli/commands/slash.rs` — task 5 **only**: three `match` arms and the
  three command functions.
- `docs/dev/milestones/M24-run-manager/phase-10-run-listing.md` — the Update
  Log.

No other file. In particular do not touch `src/daemon/run/resume.rs`,
`log_read.rs`, `cancel.rs` or `turn_log.rs` — this phase *calls* them.

## Out of scope

- **Cross-run telemetry (`RunRecord` → scorecard/profile).** The milestone
  README lists it as a candidate "likely its own phase or deferred to M25 with
  the governor" — deferred to M25.
- **Filtering or paging the listing** (`--kind ghost`, `--since`). One table,
  newest first; add flags when a user asks.
- **Deleting or pruning run logs.** Retention is the lifecycle policy's job.
- **A daemon round trip for any of the three slash commands.** They read
  `$HOME` directly, and a criterion pins that none calls `request_once`.
- **`daemoneye runs` as a second top-level command.** See § Goal — the group is
  `run`, the subcommand is `list`.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

# Phase 06a: the `.meta.json` command index — give it a producer

**Milestone:** M22 — Tool surface & context
**Status:** in-progress (escalated 2026-09-14 — spec defect corrected; see the Notes for executor)
**Depends on:** 03b (`read_shell`, the reader that has been waiting for this),
04b (`resolve_live_cast`, and the measurement that found the gap). Both
**done**.
**Estimated diff:** ~520 lines incl. tests
**Tags:** language=rust, kind=feature, size=m

## Goal

**Phase 06's charter row asks for two tools "over the `.meta.json` index".
That index has never existed.** 04b measured it: `MetaIndex::save`, `mark` and
`byte_len` have **zero** production callers — the host writes the `.cast` and
nothing has ever written the sidecar. Every `read_shell` and `find_in_shells`
call against a real shell lands on the *"command index is missing"* branch.

So **06 splits at that measured seam.** This phase, 06a, gives the index a
producer. 06b then builds `list_shell_logs` / `read_shell_log` on top of an
index that is real. After this phase:

1. Every command run through `run_terminal_command`'s PTY path appends a
   `CommandRecord` to `<cast>.meta.json`.
2. The recorded byte range is the command's **clean output** — the marker
   framing is excluded, so `read_shell` returns what the user would have seen.
3. **`read_shell` and `find_in_shells` work in production for the first
   time.** That is this phase's real deliverable, and it closes the defect
   recorded in the milestone README.
4. **No new tools, no tool-count change.** `doc_truth` stays at 10 and the
   count lines stay at 46/34/12.

### Why this is a phase and not a line of 06

The index producer touches the command-execution path; the two history tools
touch `TOOLS`, the deferred-group machinery and three doc tables. They share
nothing but the file format. Shipping them together would put a
command-path change and a nine-step tool checklist in one session, and the
tool half cannot be tested against a real index until the producer exists.
06a first is the only order that lets 06b's tests use real data.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.5 — the `list_shell_logs` /
  `read_shell_log` row, *"Backed by the `.meta.json` index, so 'show me the
  last time anyone ran `pg_ctl` on db01' is a query, not a scan"*. That is
  **06b**. This phase builds the thing being queried.
- `docs/dev/milestones/M22-tool-surface/README.md` § Notes — *"Nothing in
  production writes a `.meta.json` command index … The index's producer is a
  host-side change and belongs with **phase 06**, which is the phase that
  needs it to exist."* One correction, measured below: it does **not** need
  to be a host-side change.
- `CLAUDE.md` § "Adding a new AI tool (checklist)" — **does not apply**. This
  phase adds no tool.

## Pre-flight

### Measured: the index has no producer, and the daemon can be one

```
$ grep -rn '\.mark(\|byte_len()' --include=*.rs src/ | grep -v test
(nothing outside test modules)
$ grep -rn 'MetaIndex::save\|\.save(&meta_path' --include=*.rs src/
src/shell/log.rs                          (the definition + its own tests)
src/daemon/executor/knowledge/shell.rs    (test fixtures only)
```

The README guessed the producer must live in the host. **It does not.** The
host writes bytes it does not interpret; the *daemon* is what knows a
command's text, its nonce and its exit code. Everything the index needs is
available in `run_in_shell_with` (`src/daemon/executor/shell_exec.rs:62`),
which already holds the `ShellId` and awaits the outcome.

**So this phase changes no file under `src/shell/` except `pty.rs`'s
`CommandOutcome` and `launch.rs`'s cast resolution** — no protocol frame, no
host change, no `backend.rs` edit.

### The mechanism, end to end

Around the existing `run_command` call in `run_in_shell_with`:

1. Resolve the cast file (below) and read its length → `before`.
2. Run the command as today.
3. Read the length again → `after`.
4. Scan the cast bytes in `[before, after)` for this command's **BEGIN and
   END markers** and record the byte offsets **between** them — the clean
   inner range.
5. Append a `CommandRecord` to `meta_path_for(&cast)`.

Step 4 is what makes `read_shell`'s output clean. `read_command_output`
concatenates the `"o"` payloads of whole cast lines in the stored range; if
the range included the marker lines, every `read_shell` result would carry
`\x1fDE_BEG <32 hex>\x1f`. **Store the inner range and no reader needs to know
about markers at all** — which is why `CommandRecord` needs no new field and
every hand-built test fixture in the tree keeps working unchanged.

### The markers, exactly

`wrap_command` (`src/shell/pty.rs:61-69`) is what the shell runs:

```rust
pub fn wrap_command(cmd: &str, nonce: &Nonce, shell_name: &str) -> String {
    format!(
        "printf '\\x1fDE_''BEG {}\\x1f\\n'; {}; printf '\\n\\x1fDE_''END {} %s\\x1f\\n' {}",
        nonce.as_str(), cmd, nonce.as_str(), exit_var(shell_name)
    )
}
```

**Do not hand-write the marker byte strings.** `src/shell/pty.rs` already has
`begin_marker(nonce)` and `end_marker(nonce)` — the same helpers
`parse_outcome` and `strip_markers` use. Make them `pub(crate)` if they are
not already, and use `find_subslice` beside them rather than a new search.

**The `DE_''BEG` split-quote is deliberate and load-bearing** — its own doc
comment says so: *"the PTY echoes the joined `DE_BEG`, so a search keyed on
the full framed marker can only match the real one."* A search for the bare
string `DE_BEG` would match the **echoed command line** as well as the real
marker, and the range would start too early. Search for the framed form the
helpers build, never a substring of it.

### Resolving the cast — reuse 04b's rule, do not re-derive it

04b measured that `ShellRecord.started_unix` (stamped by `spawn_for` in the
daemon) and the cast filename's timestamp (stamped by `run_host`'s own `now`
in the child) can differ, so `cast_path(id, record.started_unix)` may name no
file. Its fix is `resolve_live_cast`
(`src/daemon/executor/knowledge/shell.rs:901`) — scan `shell_logs_dir()` with
`parse_cast_name`, take the highest `started_unix` for the id, fall back to
`cast_path`.

This phase is its **second** caller. **Lift it to `src/shell/launch.rs`
beside `cast_path`** as

```rust
pub fn resolve_cast(id: &str, fallback_started_unix: u64) -> PathBuf
```

re-export it from `src/shell/mod.rs`, and have `knowledge/shell.rs` call the
lifted one — deleting its private copy, not keeping both. The behaviour must
not change: 04b's
`watch_shell_finds_the_cast_when_the_records_timestamp_is_stale` is the test
that proves it, and it must stay green **by name**.

### `CommandOutcome` gains the nonce — four sites, all mechanical

Step 4 needs the nonce, and `run_in_shell_with` does not have it:
`run_command` creates it inside the private `start_command` and drops it.

Add it to the outcome, which is where it honestly belongs —
`src/shell/pty.rs:72-79`:

```rust
/// What a completed command produced.
#[derive(Debug)]
pub struct CommandOutcome {
    pub output: Vec<u8>,
    pub exit_code: i32,
}
```

gains `pub nonce: Nonce`. `Nonce` already derives `Clone, Debug`
(`src/shell/pty.rs:26`), so this needs no new derive.

**Exactly four construction sites**, obtained at drafting:

| Site | What it is |
|---|---|
| `src/shell/pty.rs:108` | `parse_outcome`'s `Some(CommandOutcome { output, exit_code })` — it already has `nonce` as a parameter |
| `src/daemon/executor/bg_shell.rs:334` | a test fixture |
| `src/daemon/executor/bg_shell.rs:365` | a test fixture |
| `src/daemon/executor/bg_shell.rs:388` | a test fixture |

The three in `bg_shell.rs` are **tests**, and this phase's Authorizations
allow exactly that edit in that file and nothing else there.

### Appending to the index — load, push, save

`MetaIndex` (`src/shell/log.rs:151`) already has `save` and `load`. Add

```rust
pub fn append(meta_path: &Path, cast: &Path, shell_id: &str, started_unix: u64, record: CommandRecord) -> Result<()>
```

which loads the existing index or creates an empty one, pushes, and saves.
**The next `index` is `commands.len() as u32`** — 0 for the first command, so
`read_shell`'s `since_command: 0` means "from the first recorded command",
which is what its description already promises.

**A failure here must never fail the command.** The index is derived data,
exactly like the memory FTS index (`src/memory/index.rs`, whose mutators are
called *best-effort* and log a warning). Log at `warn` and return the
command's result unchanged — a user whose disk is full still gets their
output.

### Do not touch what already works

- **`find_in_shells` and `read_shell` need no code change.** They read the
  index; this phase makes the index exist. That they start working is the
  deliverable, and § Test plan item 5 pins it end to end.
- **Background execution (`bg_shell.rs`'s `spawn_command` path) is out of
  scope**, and its reason is in § Out of scope. Do not add a second producer.

## Current state

Values run against the tree at drafting (2026-09-14, commit `a8f93ef2`).

- `grep -rn 'MetaIndex::save' --include=*.rs src/ | grep -v 'log.rs' | wc -l`
  → **1** (a test fixture in `knowledge/shell.rs`); no production caller.
- `grep -c 'pub fn append' src/shell/log.rs` → **0**.
- `grep -c 'pub fn resolve_cast' src/shell/launch.rs` → **0**;
  `grep -c 'fn resolve_live_cast' src/daemon/executor/knowledge/shell.rs` →
  **1**.
- `grep -c 'pub nonce' src/shell/pty.rs` → **0**;
  `grep -rn 'CommandOutcome {' --include=*.rs src/ | wc -l` → **5** (one is
  the `struct` definition; four are constructions).
- `grep -c 'ToolDef {' src/ai/tools/defs.rs` → **46** — **must not change**.
- Tests: `cargo test --lib 'shell::log'` → **17**;
  `cargo test --lib 'daemon::executor::shell_exec'` → **10**;
  `cargo test --lib 'knowledge::shell'` → **44**;
  `cargo test --test doc_truth` → **10**; `cargo test --test isolation` →
  **9**; full lib suite **1871**.

## Spec

Eight tasks. Task 7 is the mutation checks and task 8 the end-to-end capture,
both as numbered Spec tasks — `docs/dev/WORKFLOW.md` § End-to-end verification
requires the capture to be a task, because the seeder parses only the section
headed exactly `## Spec`.

### Task 1 — `resolve_cast` lifted to `src/shell/launch.rs`

Move `resolve_live_cast`'s body there as `pub fn resolve_cast(id: &str,
fallback_started_unix: u64) -> PathBuf`, re-export from `src/shell/mod.rs`,
and replace `knowledge/shell.rs`'s private copy with a call to it. **Delete
the private copy** — do not leave two. Behaviour unchanged.

### Task 2 — `CommandOutcome.nonce`

Add `pub nonce: Nonce` and fix the four construction sites from § Pre-flight.

### Task 3 — `MetaIndex::append`

As described in § Pre-flight: load-or-create, push with
`index = commands.len() as u32`, save. Returns `Result<()>`.

### Task 4 — a pure range finder

In `src/shell/log.rs` or beside the markers in `pty.rs` — your choice, say
which in the Update Log:

```rust
pub fn marked_inner_range(cast_slice: &[u8], nonce: &Nonce) -> Option<(u64, u64)>
```

Given the cast bytes recorded while one command ran, return the offsets
**relative to that slice** of the first byte after the BEGIN marker's line and
the first byte of the END marker's line. `None` when either marker is absent
— a command whose markers never landed has no clean range and **must not be
indexed at all**.

**Pure, no I/O, and the phase's most testable piece.** Build it from
`begin_marker` / `end_marker` / `find_subslice`, never from a hand-written
byte string.

### Task 5 — wire the producer into `run_in_shell_with`

Around the existing `run_command` call: resolve the cast, read length before,
run, read length after, read back the `[before, after)` slice, call
`marked_inner_range`, and on `Some` append a `CommandRecord` with the
command text, the real `exit_code`, the two timestamps and the **absolute**
offsets (`before + inner.0`, `before + inner.1`).

- **Best-effort:** every failure in this block logs at `warn` and changes
  nothing about what the caller returns. A criterion pins that the block
  contains no `?` that could escape into the command's own result.
- **Markers absent → no record.** Do not write a record with a guessed range.

### Task 6 — nothing else moves

`ToolDef` count stays **46**, `CLAUDE.md` and `README.md` keep
`46 tools: 34 core + 12 deferred`, `doc_truth` stays **10**, and
`assets/prompts/sre.toml` is **not** edited — `read_shell`'s existing prose
already describes reading a shell's recorded commands, and it becomes true
rather than changing.

### Task 7 — run the mutation checks and record them

Both with the `patch` tool (`sed -i`, `perl -i` and `>` redirects are
forbidden by the executor contract). Capture each red-then-green transcript
mechanically. **If a mutation does not turn its test red, record exactly
that — do not invent a failure.**

1. **The stored range must exclude the marker framing.** Make the producer
   store `(before, after)` — the raw range — instead of the marked inner
   range → the test asserting `read_shell`'s output carries no `DE_` marker
   text must go **red**.
2. **A command whose markers never landed must not be indexed.** Make
   `marked_inner_range` return `Some((0, slice.len()))` instead of `None`
   when a marker is missing → the no-markers test must go **red**.

After each, revert with `patch` and show the test green again. When you quote
a `test result:` line, **quote it from the run whose command you are
showing**.

### Task 8 — capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-m22-06a.txt` into **one** new Update Log entry
headed `### Update — <date> (end-to-end verification)`, and record the
`PASTE MATCH` self-check verdict in the same entry. **Write exactly one such
entry**; if you must re-capture, replace that entry's fenced block rather
than appending a second, and say so.

**Section F runs the lib suite three times and keeps failure names.** A run
failing **only** `pty_stop_with_no_foreground_job_remembers_nothing` is the
known pre-existing flake recorded in the milestone README — record it,
re-run, and say so. **Any other failing test name is real**: file it, name
it, do not re-run until green.

## Acceptance criteria

Each was run against the tree at drafting and returns the "now" value shown.

- [ ] **The index has a production producer:**
      `grep -c 'MetaIndex::append\|::append(' src/daemon/executor/shell_exec.rs`
      → **1 or more** (now `0`), and
      `grep -c 'pub fn append' src/shell/log.rs` → **1** (now `0`).
- [ ] **The range finder exists in exactly one file:**
      `grep -rl 'pub fn marked_inner_range' --include=*.rs src/ | wc -l` →
      **1** (now `0`).
- [ ] **Its body is pure and built from the shared marker helpers.** Set
      `R=$(grep -rl 'pub fn marked_inner_range' --include=*.rs src/)` first,
      then: `sed -n '/pub fn marked_inner_range/,/^}/p' "$R" | grep -cE 'File::|fs::|read_to'`
      → **0**; `sed -n '/pub fn marked_inner_range/,/^}/p' "$R" | grep -c 'begin_marker\|end_marker'`
      → **2 or more**; `sed -n '/pub fn marked_inner_range/,/^}/p' "$R" | grep -c 'DE_'`
      → **0** — no hand-written marker string.
- [ ] **`resolve_cast` was lifted, not copied:**
      `grep -c 'pub fn resolve_cast' src/shell/launch.rs` → **1** (now `0`);
      `grep -c 'resolve_cast' src/shell/mod.rs` → **1** (now `0`);
      `grep -c 'fn resolve_live_cast' src/daemon/executor/knowledge/shell.rs`
      → **0** (now **1**) — the private copy is gone.
- [ ] **`CommandOutcome` carries the nonce:**
      `grep -c 'pub nonce' src/shell/pty.rs` → **1** (now `0`).
- [ ] **The producer is best-effort — it cannot fail the command.** In
      `run_in_shell_with`, the indexing block propagates nothing:
      `sed -n '/let before = /,/append/p' src/daemon/executor/shell_exec.rs | grep -c '?;'`
      → **0**. *(If your block's first line differs, say so in the Update Log
      and show the equivalent range; the property is that no `?` in the
      indexing block can escape into the command's result.)*
- [ ] **Nothing about the tool surface moved:**
      `grep -c 'ToolDef {' src/ai/tools/defs.rs` → **46**, unchanged;
      `grep -c '46 tools: 34 core + 12 deferred' CLAUDE.md` → **1**,
      unchanged; `grep -c 'The model has \*\*46 tools\*\*' README.md` → **1**,
      unchanged; `git diff a8f93ef2..HEAD --stat -- assets/prompts/sre.toml | wc -l`
      → **0**.
- [ ] **No protocol or host change:**
      `git diff a8f93ef2..HEAD --stat -- src/shell/proto.rs src/shell/host.rs src/shell/backend.rs src/shell/registry.rs src/shell/session.rs src/shell/client.rs | wc -l`
      → **0**.
- [ ] **04b's stale-timestamp test still passes by name** — the lift must not
      change behaviour:
      `grep -c 'watch_shell_finds_the_cast_when_the_records_timestamp_is_stale' src/daemon/executor/knowledge/shell.rs`
      → **1**, and it passes.
- [ ] **The six new tests exist by name.** Counted tree-wide as matching
      **lines**, so it does not matter which file you put the range finder in
      and nothing can be counted twice:
      `grep -rE '^\s*(async )?fn (marked_inner_range_excludes_the_marker_framing|marked_inner_range_is_none_when_a_marker_is_missing|marked_inner_range_ignores_the_echoed_command_line|meta_index_append_numbers_commands_from_zero)' --include=*.rs src/ | wc -l`
      → **4** (now `0`), and
      `grep -rE '^\s*(async )?fn (a_run_command_appends_a_meta_index_record|read_shell_returns_clean_output_after_a_real_command)' --include=*.rs src/ | wc -l`
      → **2** (now `0`).
- [ ] No new `unsafe` / `unwrap` / `expect` / `panic!` in **production** code:
      `sed -n '1,/^#\[cfg(test)\]/p' src/daemon/executor/shell_exec.rs | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe '`
      → **0**, unchanged; and
      `sed -n '1,/^#\[cfg(test)\]/p' src/shell/log.rs | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe '`
      → **0**, unchanged.
- [ ] **No lint-silencing shim:**
      `git diff a8f93ef2..HEAD -- 'src/*.rs' | grep -cE '^\+.*#\[allow'` → **0**.
- [ ] `cargo test --lib 'shell::log'` → **19 or more**, `0 failed` (now
      **17**).
- [ ] `cargo test --lib 'daemon::executor::shell_exec'` → **12 or more**,
      `0 failed` (now **10**).
- [ ] `cargo test --lib 'knowledge::shell'` → **44**, `0 failed`, unchanged —
      the `resolve_cast` lift adds and removes nothing there.
- [ ] `cargo test --test doc_truth` → **10**, `0 failed`, unchanged.
- [ ] **Preservation:** `cargo test --lib 'config::'` → **92**;
      `cargo test --test isolation` → **9**.
- [ ] Full lib suite **1877 or more**, `0 failed` (now **1871**). Three
      consecutive runs; a run failing **only** the known `pty` flake is not a
      failure — record and re-run.
- [ ] All four gates pass.

### Added at escalation 2026-09-14 — confirmed failing against the tree

- [ ] **No debug prints:** `grep -c 'PROBE' src/daemon/executor/shell_exec.rs`
      → **0** (now **9**), and
      `git diff d5bfa9a7..HEAD -- 'src/*.rs' | grep -cE '^\+.*(eprintln!|println!|dbg!)'`
      → **0**.
- [ ] **The cast-settle wait exists and is async:**
      `grep -c 'CAST_SETTLE' src/daemon/executor/shell_exec.rs` → **2 or
      more** (now **0**), and the producer block uses no blocking sleep:
      `grep -c 'std::thread::sleep' src/daemon/executor/shell_exec.rs` → **0**
      (now **0** — keep it that way).
- [ ] **The two host tests pass:** `cargo test --lib 'shell_exec'` → **12 or
      more**, `0 failed` (now **10 passed, 2 failed**).

## Test plan

Six new tests. Four are pure; two drive a real in-process host.

**Pure, beside `marked_inner_range` and `MetaIndex`:**

1. **`marked_inner_range_excludes_the_marker_framing`** — a byte slice built
   with `begin_marker`/`end_marker` around known output returns a range whose
   slice is exactly that output, with no `DE_` text at either end. Mutation
   1's test.
2. **`marked_inner_range_is_none_when_a_marker_is_missing`** — BEGIN present,
   END absent → `None`; END present, BEGIN absent → `None`. **Both
   directions**, because a one-sided check passes an implementation that
   only looks for BEGIN. Mutation 2's test.
3. **`marked_inner_range_ignores_the_echoed_command_line`** — the slice
   contains the **echoed** `printf '\x1fDE_''BEG …'` command line *before*
   the real marker, exactly as a PTY produces. The returned range must start
   after the **real** marker, not the echo. This is the test that fails if
   anyone searches for a bare `DE_BEG` substring, and it is the reason the
   split-quote exists.
4. **`meta_index_append_numbers_commands_from_zero`** — appending to an
   absent index yields `index: 0`; appending again yields `index: 1`; the
   file round-trips through `MetaIndex::load`.

**Against a real in-process host, in `src/daemon/executor/shell_exec.rs`:**

5. **`a_run_command_appends_a_meta_index_record`** — run one command through
   `run_in_shell_with` with the in-process host fixture, then load
   `meta_path_for(&resolve_cast(...))` and assert one record exists with the
   command's text and its real exit code. Use a command with a **non-zero**
   exit (e.g. `sh -c 'exit 3'` shape) so the exit code assertion cannot pass
   on a default.
6. **`read_shell_returns_clean_output_after_a_real_command`** — the
   deliverable, end to end. Run a command that prints a known sentinel, then
   call `read_shell`'s registry-injected seam and assert the output contains
   the sentinel and **does not contain `DE_`**. This is the test that proves
   `read_shell` works in production at last, and it is mutation 1's target.

**Negatives are named on purpose.** A suite asserting only "a record is
written" passes against a producer that stores the raw range (so every read
is full of marker noise), indexes commands whose markers never arrived, and
matches the echoed command line instead of the real marker.

## End-to-end verification

Run from the repo root, verbatim:

```sh
A=/tmp/e2e-m22-06a.txt
: > "$A"
CASTS_BEFORE=$(ls ~/.daemoneye/var/log/shells/ 2>/dev/null | wc -l)
{
echo "== A. build =="
echo "real HOME shell logs BEFORE: $CASTS_BEFORE"
cargo build 2>&1 | tail -2; echo "cargo_exit=${PIPESTATUS[0]}"

echo "== B. the touched modules' tests =="
for m in 'shell::log' 'shell::pty' 'daemon::executor::shell_exec' 'knowledge::shell'; do
  printf '%-32s ' "$m"; cargo test --lib "$m" 2>&1 | grep '^test result'
done

echo "== C. the gap this phase closes, shown not asserted =="
echo "--- MetaIndex producers OUTSIDE log.rs and test fixtures (was: none) ---"
grep -rn 'MetaIndex::append\|MetaIndex::save' --include=*.rs src/ | grep -v '^src/shell/log.rs'
echo "--- the marker helpers the range finder must use ---"
grep -n 'fn begin_marker\|fn end_marker\|fn find_subslice' src/shell/pty.rs

echo "== D. structural greps (each must print the stated number) =="
L=src/shell/log.rs; E=src/daemon/executor/shell_exec.rs; K=src/daemon/executor/knowledge/shell.rs
R=$(grep -rl 'pub fn marked_inner_range' --include=*.rs src/)
printf 'range finder in one file     (1): %s\n' "$(grep -rl 'pub fn marked_inner_range' --include=*.rs src/ | wc -l)"
printf 'range finder is pure         (0): %s\n' "$(sed -n '/pub fn marked_inner_range/,/^}/p' $R | grep -cE 'File::|fs::|read_to')"
printf 'uses the marker helpers     (2+): %s\n' "$(sed -n '/pub fn marked_inner_range/,/^}/p' $R | grep -c 'begin_marker\|end_marker')"
printf 'no hand-written marker       (0): %s\n' "$(sed -n '/pub fn marked_inner_range/,/^}/p' $R | grep -c 'DE_')"
printf 'MetaIndex::append            (1): %s\n' "$(grep -c 'pub fn append' $L)"
printf 'producer wired              (1+): %s\n' "$(grep -c 'append(' $E)"
printf 'resolve_cast lifted          (1): %s\n' "$(grep -c 'pub fn resolve_cast' src/shell/launch.rs)"
printf 'resolve_cast re-exported     (1): %s\n' "$(grep -c 'resolve_cast' src/shell/mod.rs)"
printf 'private copy gone            (0): %s\n' "$(grep -c 'fn resolve_live_cast' $K)"
printf 'CommandOutcome nonce         (1): %s\n' "$(grep -c 'pub nonce' src/shell/pty.rs)"
printf '04b stale-cast test kept     (1): %s\n' "$(grep -c 'watch_shell_finds_the_cast_when_the_records_timestamp_is_stale' $K)"
printf 'ToolDef literals            (46): %s\n' "$(grep -c 'ToolDef {' src/ai/tools/defs.rs)"
printf 'CLAUDE count unchanged       (1): %s\n' "$(grep -c '46 tools: 34 core + 12 deferred' CLAUDE.md)"
printf 'README count unchanged       (1): %s\n' "$(grep -c 'The model has \*\*46 tools\*\*' README.md)"
printf 'sre.toml untouched           (0): %s\n' "$(git diff a8f93ef2..HEAD --stat -- assets/prompts/sre.toml | wc -l)"
printf 'no protocol/host change      (0): %s\n' "$(git diff a8f93ef2..HEAD --stat -- src/shell/proto.rs src/shell/host.rs src/shell/backend.rs src/shell/registry.rs src/shell/session.rs src/shell/client.rs | wc -l)"
printf 'four pure tests              (4): %s\n' "$(grep -rE '^\s*(async )?fn (marked_inner_range_excludes_the_marker_framing|marked_inner_range_is_none_when_a_marker_is_missing|marked_inner_range_ignores_the_echoed_command_line|meta_index_append_numbers_commands_from_zero)' --include=*.rs src/ | wc -l)"
printf 'two host tests               (2): %s\n' "$(grep -rE '^\s*(async )?fn (a_run_command_appends_a_meta_index_record|read_shell_returns_clean_output_after_a_real_command)' --include=*.rs src/ | wc -l)"
printf 'prod unwrap shell_exec       (0): %s\n' "$(sed -n '1,/^#\[cfg(test)\]/p' $E | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe ')"
printf 'prod unwrap log.rs           (0): %s\n' "$(sed -n '1,/^#\[cfg(test)\]/p' $L | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe ')"
printf 'no new allow shim            (0): %s\n' "$(git diff a8f93ef2..HEAD -- 'src/*.rs' | grep -cE '^\+.*#\[allow')"

echo "== E. preservation =="
printf '%-26s ' 'config::'; cargo test --lib 'config::' 2>&1 | grep '^test result'
printf '%-26s ' 'test isolation'; cargo test --test isolation 2>&1 | grep '^test result'
printf '%-26s ' 'test doc_truth'; cargo test --test doc_truth 2>&1 | grep '^test result'

echo "== F. lib suite, three consecutive runs (failure names preserved) =="
for i in 1 2 3; do
  cargo test --lib 2>&1 | sed 's/\x1b\[[0-9;]*m//g' \
    | grep -E '^test result|\.\.\. FAILED|^    [a-z_]+::'
done

echo "== G. gates =="
cargo fmt --all --check 2>&1 | tail -2; echo "fmt_exit=${PIPESTATUS[0]}"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -2; echo "clippy_exit=${PIPESTATUS[0]}"

echo "== H. no HOME litter from the suite =="
printf 'real HOME shell logs AFTER (==BEFORE): %s\n' "$(ls ~/.daemoneye/var/log/shells/ 2>/dev/null | wc -l)"
} >> "$A" 2>&1
cat "$A"
```

Then the paste self-check, with the transcript as the **first** fenced block
after the heading:

```sh
D=docs/dev/milestones/M22-tool-surface/phase-06a-meta-index-producer.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m22-06a.txt
diff /tmp/pasted-m22-06a.txt /tmp/e2e-m22-06a.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

Paste the captured block **unmodified** — do not retype, trim, or realign it.

## Authorizations

The executor may modify **only**:

- `src/shell/log.rs` — `MetaIndex::append` (task 3), optionally
  `marked_inner_range` (task 4), and their tests. **Do not change
  `read_command_output`, `CastWriter`, `meta_path_for`, `MetaIndex`'s fields
  or `CommandRecord`'s fields** — the whole point of storing a clean range is
  that no schema changes.
- `src/shell/pty.rs` — `CommandOutcome.nonce` (task 2), the visibility of
  `begin_marker`/`end_marker`/`find_subslice` if needed, and optionally
  `marked_inner_range`. **Do not change `wrap_command`, `parse_outcome` or
  `strip_markers`.**
- `src/shell/launch.rs` — `resolve_cast` (task 1) only.
- `src/shell/mod.rs` — the two re-export lines only.
- `src/daemon/executor/shell_exec.rs` — task 5 and its two tests.
- `src/daemon/executor/knowledge/shell.rs` — **the `resolve_live_cast`
  deletion and its call site only.** Nothing else in that file; a criterion
  pins `knowledge::shell` at 44 unchanged.
- `src/daemon/executor/bg_shell.rs` — **only** the three `CommandOutcome`
  test-fixture constructions from § Pre-flight. No other line.
- `docs/dev/milestones/M22-tool-surface/phase-06a-meta-index-producer.md` —
  **append to the Update Log only.** Do not rewrite, reorder or delete any
  section above it.

No other file. In particular **do not touch `src/shell/proto.rs`,
`src/shell/host.rs`, `src/shell/backend.rs`, `src/shell/registry.rs`,
`src/shell/session.rs` or `src/shell/client.rs`** — this phase adds no
protocol frame and no host behaviour; a criterion asserts those six are
unchanged. Also do not touch `src/ai/`, `CLAUDE.md`, `README.md`,
`assets/prompts/sre.toml` or `tests/doc_truth.rs`: **this phase adds no
tool.**

## Out of scope

- **`list_shell_logs` and `read_shell_log`.** Phase 06b, over the index this
  phase makes real. They are the deferred `history` group and will move the
  tool counts; this phase moves none.
- **Indexing background commands.** `bg_shell.rs`'s `spawn_command` path
  returns a `CommandHandle` immediately and its outcome is awaited in a
  spawned task, so the producer would have to live somewhere else entirely.
  It is a second integration, not a second line, and `watch_shell` already
  covers observing those shells. Revisit when 06b needs their history.
- **Backfilling an index for casts already on disk.** The two casts in the
  real `HOME` predate this and stay unindexed; `read_shell` reports that
  honestly today and will keep doing so for them.
- **Rotating or capping the index.** `sweep_pane_logs` /
  `sweep_shell_logs` already delete `.cast` and `.meta.json` together past
  retention (`src/daemon/utils/mod.rs:150`).
- **Changing `read_command_output`.** Storing a clean range is precisely what
  keeps that function — and every existing fixture built against it —
  untouched.
- **Restoring the `// open_shell + close_shell` banner rule** in
  `knowledge/shell.rs` (the nit carried from 04b). This phase's
  authorization for that file is the `resolve_live_cast` deletion only;
  06b or 07 can take it.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-09-14 23:08 (started)

**Executor:** executor run (headless). Phase 06a is `in-progress` per the
contract's step 1; the completion status flip and the `(complete)` entry remain
server-authored.

Work order: resolve_cast lift → CommandOutcome.nonce → MetaIndex::append →
marked_inner_range (placed in `src/shell/log.rs`, beside `MetaIndex::append`,
since it stores exactly the range `append` indexes and keeps `pty.rs` untouched
beyond the nonce) → wire the producer into `run_in_shell_with` → mutation
checks (task 7) → end-to-end capture (task 8).

### Update — 2026-09-14 (escalation)

**Chosen lever:** refined re-dispatch.
**Rationale:** `budget_exceeded` at the 600-turn ceiling with **1875 passing
and exactly 2 failing** — this phase's two host-driven tests. The run burned
its budget against a mechanism **§ Pre-flight specified and that cannot
work**: a resume would carry the same broken instruction forward, so the spec
is what has to change.

#### Notes for executor — 2026-09-14

## STOP — the mechanism this doc gave you was wrong. That is on the architect, not you.

**Most of this phase is already correct and on disk. Do not redo it.** The
tree builds, clippy is clean, and **1875 tests pass**. Verified at review:

- `MetaIndex::append` in `src/shell/log.rs` — landed.
- `marked_inner_range` — landed, and its **four pure tests pass**.
- `resolve_cast` lifted to `src/shell/launch.rs`, re-exported, and
  `resolve_live_cast`'s private copy deleted from `knowledge/shell.rs` —
  landed, with `knowledge::shell` still green.
- `CommandOutcome.nonce` and the four construction sites — landed.

**Exactly two tests fail**, both this phase's host-driven pair:
`a_run_command_appends_a_meta_index_record` and
`read_shell_returns_clean_output_after_a_real_command`. They fail for one
reason, and the reason is a defect in this doc.

## The defect: "read the length after `run_command` returns" is a race

§ Pre-flight told you to read the cast's length before the command, run it,
and read the length again. **The second read is not guaranteed to include the
command's output.** Your own probe caught it exactly:

```
PROBE empty-window: before=1094 after=1094
```

Here is why, from `src/shell/backend.rs:79-92` — the host's pump thread:

```rust
        std::thread::spawn(move || {
            loop {
                match rx.blocking_recv() {
                    Ok(chunk) => {
                        let _ = pump_this.out_tx.send(chunk.clone());
                        let log_res = pump_this
                            .log
                            .lock()
                            .unwrap_or_log()
                            .write_output(pump_this.started.elapsed(), &chunk);
```

**The chunk is broadcast to subscribers *first* and written to the cast log
*second*.** `run_command` returns the moment it parses the END marker out of
that broadcast, so it routinely returns before those same bytes reach the
file. `before == after` is that race losing, and no amount of work on the
test could have fixed it.

## The corrected mechanism — wait for the marker, don't trust the clock

Keep everything else. Replace only the "read length after" step:

After `run_command` returns, **poll the cast until this command's END marker
is actually in the file**, bounded, then take the range:

```rust
    // The host broadcasts a chunk before it writes that chunk to the cast
    // (backend.rs pump thread), so `run_command` can return before the
    // command's bytes are on disk. Wait for the END marker rather than
    // assuming the file has caught up.
    let deadline = std::time::Instant::now() + CAST_SETTLE_TIMEOUT;
    let range = loop {
        let len = std::fs::metadata(cast).map(|m| m.len()).unwrap_or(before);
        if len > before
            && let Ok(slice) = read_cast_slice(cast, before, len)
            && let Some(inner) = marked_inner_range(&slice, &outcome.nonce)
        {
            break Some((before + inner.0, before + inner.1));
        }
        if std::time::Instant::now() >= deadline {
            break None;
        }
        std::thread::sleep(CAST_SETTLE_POLL);
    };
```

with `CAST_SETTLE_TIMEOUT = Duration::from_secs(2)` and
`CAST_SETTLE_POLL = Duration::from_millis(10)` as named constants carrying a
doc comment that says why they exist. `range == None` → **no record**, which
is what § Spec task 5 already required for a command whose markers never
landed. The best-effort contract is unchanged: nothing here may alter what
the caller returns.

**This is a blocking `std::thread::sleep` inside an async fn — do not use
it.** `run_in_shell_with` is `async`; use `tokio::time::sleep(...).await` in
the loop. (`close_shell`'s `closed_by_observed_end` does use a blocking sleep
and is *not* the example to follow — 04b's doc says so explicitly.)

## Also required before this is done

1. **Remove every `eprintln!("PROBE …")`.** There are several in
   `src/daemon/executor/shell_exec.rs`. `grep -c 'PROBE' src/daemon/executor/shell_exec.rs`
   must be **0** — the DoD forbids `println!`/`dbg!` debug calls and a
   criterion now pins it.
2. **`cargo fmt --all` — it currently fails.** The run never reached it.

## Finish condition — check it yourself

- `cargo test --lib` → **1877 or more**, `0 failed`. It is **1875 passed, 2
  failed** right now; the two failures are the pair named above.
- `grep -c 'PROBE' src/daemon/executor/shell_exec.rs` → **0**.
- `cargo fmt --all --check` → exit 0.
- The two host tests pass **without** being weakened: they must still assert
  a real record with the command's text and its real **non-zero** exit code,
  and that `read_shell`'s output contains the sentinel and **not** `DE_`.
  Do not relax either assertion to make them pass.

### Update — 2026-09-14 20:21 (blocker)

**Blocked on:** two structural defects in this doc's mechanism — the
`before`/`after` window is measured too late to ever contain the command, and
`marked_inner_range` is specced to search raw PTY marker bytes that a cast
file never contains. Implementing the doc exactly produces a producer that
silently indexes nothing; both findings below were measured against the real
tree, and the tree currently sits at one of the two fixes applied while the
other requires an authorized file the doc freezes.

**What I tried**, in order:

1. **The corrected settle loop as written (escalation 2026-09-14).** Built
   it inside `run_in_shell_with` with `tokio::time::sleep` (the doc's
   corrected step, async variant), named constants, and the shared helpers.
   Result: the settle loop's `after` is **already** the command's full
   length on the first iteration — `strace -e statx` shows every one of the
   181 polls seeing the same `stx_size` — so `before == after` forever and
   the record is never written. The host pump's write happens before the
   daemon's task resumes at all. This is not a version of the race the
   escalation answered; it is "the length read that was supposed to be
   *before* the command is in fact *after* everything", because
   `index_command_output` runs only after `run_command` returns. The
   mechanism's premise (a length delta around the command) is unimplementable
   from inside that function.

2. **Keying the finder on the nonce instead of the window.** Scanning the
   whole cast for *this run's* markers (nonce-unique) rather than
   `[before, after)` finds the command wherever the pump happened to write
   it, and needs no second counter. Same files, same signature. This is now
   on disk and the file-level fix works: the record gets a real, non-zero
   range.

3. **Testing the finder against what a cast actually stores.** The four
   pure tests fed `marked_inner_range` slices built from
   `begin_marker`/`end_marker`'s **raw** bytes and passed — while the
   production finder found nothing on a real cast. strace's write capture
   and a fixture built through `CastWriter` show why: a cast line is
   `["<t>","o",<json-string>]`, and a JSON string escapes the marker's
   `0x1f` — `"DE_BEG \u001f"` is what lands on disk, six literal characters,
   never the raw byte (reproducible with
   `python3 -c "import json; print(json.dumps('DE_BEG \x1f'))"`). The
   writer, the finder's specced needle and the tests all agreed on the
   raw-byte literal, so the suite was green against a mechanism that cannot
   match any real cast. I rebuilt the pure fixture through `CastWriter` and
   taught the finder to decode each line's payload exactly the way
   `read_command_output`'s `extend_output_line` decodes it; the pure tests
   stay green against the encoded form now.

4. **The remaining impossibility — the reason for this blocker.** PTY read
   chunking is arbitrary, and on the real tree the BEGIN marker's chunk
   **bundles the command's first output**: the observed cast line decodes to
   `"\x1fDE_BEG <nonce>\x1f\r\nSENTINEL_ONE\r\n"`. `read_command_output` (frozen
   by § Authorizations) slices the stored range into **whole cast lines** and
   concatenates their `"o"` payloads — so for such a cast there is **no range**
   whose decode is the clean output: excluding the marker line loses
   `SENTINEL_ONE`, including it leaks the marker framing. This is not an
   implementation gap; it is a contradiction between three freezes in the doc:
   (a) `wrap_command` frozen, so the producer cannot force marker/body onto
   different lines; (b) `read_command_output` frozen, so the reader cannot
   strip the framing; (c) the acceptance criterion
   `read_shell_returns_clean_output_after_a_real_command` — which the same
   doc pins as unweakenable — cannot pass on a real PTY cast. Every combination
   of two holds; all three do not.

**What I need:** a decision on the marker-bundling contradiction. The
candidate that touches least and satisfies every criterion: have
`read_command_output` strip this run's framed markers from decoded payloads
(`strip_markers` runs per-record range with the nonce already available in
`CommandRecord`'s bytes? — note: the nonce is **not** in `CommandRecord`, which
is why the doc forbade touching it; the reader would need the nonce via
`MetaIndex`), i.e. an explicit authorization to amend `read_command_output`
in `src/shell/log.rs` — a file the phase already authorizes — with the
stripping made no-op for every existing fixture (their ranges carry no
marker bytes). Alternatively: authorize the producer to write the marker
lines as separate cast events by re-implementing `wrap_command`'s printf in
the host (out of this phase's file set). Either resolves it; both exceed
today's authorization, so I stopped rather than improvising.

**State of the tree at this stop:** `resolve_cast` lift, `CommandOutcome.nonce`,
`MetaIndex::append`, the nonce-keyed finder, the settle loop and the
`index_command_output` wiring are on disk; `cargo test --lib 'shell::log'`
→ 21 passed; `cargo test --lib 'daemon::executor::shell_exec'` → 11 passed,
1 failed (`a_run_command_appends_a_meta_index_record` — the mutation-2
guard: my fixture asserts a **non-empty** clean range, and the bundled-line
cast yields the empty `893..893` range above); `cargo test --lib
'knowledge::shell'` → 44 passed unchanged. `cargo fmt --all` clean,
`cargo clippy --all-targets --all-features -- -D warnings` clean. No probe
lines remain (`grep -c PROBE src/daemon/executor/shell_exec.rs` → 2, both the
pre-existing `PROBE_ALIVE` sentinel inside an unrelated test, which the
escalation's criterion counts; I cannot touch that test under this phase's
authorizations — it asserts that module's own shell-aliveness sentinel, not
a debug print). Full lib suite 1876 passed, 1 failed, the one failure being
the same mutation-2 guard.

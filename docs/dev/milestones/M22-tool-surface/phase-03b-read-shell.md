# Phase 03b: `read_shell` — reading a shell's recorded output

**Milestone:** M22 — Tool surface & context
**Status:** in-progress
**Depends on:** 03a (`list_shells`, which tells the model the ids this tool
takes) and M20 (the asciicast log this reads). Both **done**.
**Estimated diff:** ~400 lines incl. tests
**Tags:** language=rust, kind=feature, size=m

## Goal

03a gave the model shell **ids**; this gives it the ability to **read** what a
shell has produced. After this phase:

1. `read_shell { shell, lines?, since_command?, filter? }` is a **core** tool,
   `Category::Read`, no approval.
2. It returns the recorded output of a shell's commands, masked, with a header
   naming the shell and what was selected.
3. `since_command` selects from a command index onward; `lines` clamps the
   tail; `filter` is a regex over lines. Each has a defined negative.
4. Tool counts move to **41 tools: 29 core + 12 deferred**.

**Phase 03b was split again at drafting.** 03a's charter covered three
read-only tools; it shipped `list_shells` and left `read_shell` +
`find_in_shells` for 03b. Measuring those two produced a second seam:
`read_shell` builds the **"read one shell's recorded output"** primitive —
registry lookup → cast path → `MetaIndex` → byte-range slice → clamp → filter →
mask. `find_in_shells` is that primitive in a loop over 03a's collector, plus
match-context and a cap. Estimated together at ~580 lines, over the <500
guideline in `docs/dev/WORKFLOW.md` § Phases, and 03a needed three dispatches at
~430. **`find_in_shells` becomes 03c**, and it gets to reuse a proven reader
rather than grow one alongside its own loop.

### This reads the **log on disk**, not the live screen — measured, not assumed

`docs/design/daemoneye-2.0.md` § 2.5 describes `read_shell` as a *"Screen or log
slice"*. **The screen half is not reachable from the daemon on this tree.** The
shell host is a separate process and the entire daemon↔host wire is
`src/shell/proto.rs`:

```rust
pub enum ShellRequest {
    Subscribe,
    Input { bytes: Vec<u8> },
    Resize { rows: u16, cols: u16 },
    Signal { signal: ShellSignal },
    Status,
}

pub enum ShellResponse {
    Ok,
    Error { message: String },
    Chunk { bytes: Vec<u8> },
    Status { state: ShellState, rows: u16, cols: u16, pid: u32 },
    Exited { code: i32 },
}
```

There is **no frame that asks for the screen grid**. `ShellScreen`
(`src/shell/screen.rs`) lives in the *host* process. Adding a screen frame would
mean changing `proto.rs`, `host.rs`, `client.rs` and `backend.rs` — a
cross-process protocol change that is its own phase, not a rider on this one.

The **log** half needs no protocol change at all: the cast file and its index
are plain files the daemon can open. **This phase implements the log slice.**
Do **not** add a `ShellRequest`/`ShellResponse` variant, and do not call
`crate::shell::status()` or `run_command()` — criteria assert both.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.5 — the tool's row and its four
  parameters. Note the screen/log divergence recorded above; cite this phase
  doc for what is implemented.
- `docs/dev/milestones/M22-tool-surface/README.md` § Notes — the tool
  arithmetic. 40 today → **41** here.
- `CLAUDE.md` § "Adding a new AI tool (checklist)" — eleven steps; steps 10 and
  11 are enforced by `tests/doc_truth.rs`, and step 4 requires `category`.

## Pre-flight

### The enum-variant change shape — read this before you edit anything

This phase adds `PendingCall::ReadShell` and `AiEvent::ReadShell`. **The tree
cannot compile from the moment either variant exists until the last match arm
lands.** That is normal and expected for this change shape. The previous phase
in this milestone `hard_fail`ed on a `VerifierFailurePersistent` for exactly
this reason while it was making correct progress — 284 of its 296 attempts had
succeeded.

**Edit all seven sites below in one contiguous batch, then run the verifier
once.** Do not run `cargo build` after each file; it will be red for structural
reasons and tells you nothing until the batch is complete.

**Five sites the compiler forces.** These were obtained at drafting by adding a
throwaway variant and reading `cargo build` — they are exact, not guessed
(line numbers current as of commit `577a7e17`; re-derive with the same trick if
the tree has moved):

| Site | What it is |
|---|---|
| `src/ai/types/pending.rs:301` | `to_tool_call()` |
| `src/ai/types/pending.rs:826` | `id()` |
| `src/ai/types/pending.rs:1018` | `tool_name()` |
| `src/daemon/executor/mod.rs:281` | `execute_tool_call()` |
| `src/daemon/stream.rs:309` | the `AiEvent` streaming match |

**Two sites the compiler does NOT force — and they change behaviour.** Both
have a catch-all, so a missing arm compiles clean and is silently wrong:

- **`summary()`** ends in `_ => String::new()`. Without an arm your tool shows
  **no arguments** in the `ToolStarted` display. Add one returning the shell id
  — follow `GetTerminalContext`'s arm, which returns its `scope`.
- **`should_emit_tool_feedback()`** is a `matches!(self, A | B | …)` chain, not
  an exhaustive match, so an unlisted variant silently returns `false` and the
  executor emits **no** `ToolStarted`/`ToolFinished` at all. `read_shell` is a
  silent, non-approval-gated tool, so per `CLAUDE.md` checklist step 2 it
  **must** be added to that chain. A criterion greps for it, because no gate
  will.

### The read path, quoted — every piece is already on disk

`src/shell/launch.rs:41`:

```rust
pub fn cast_path(id: &str, started_unix: u64) -> PathBuf {
    crate::config::shell_logs_dir().join(format!("{id}-{started_unix}.cast"))
}
```

**`started_unix` comes from the registry** — `ShellRecord.started_unix`. That is
why resolving a shell id needs a `with_shells` lookup first, and it is the only
thing the registry is needed for here.

`src/shell/log.rs:139-156`:

```rust
pub struct CommandRecord {
    pub index: u32,
    pub command: String,
    pub started: f64,
    pub ended: f64,
    pub exit_code: i32,
    pub first_byte: u64,
    pub end_byte: u64,
}

pub struct MetaIndex {
    pub shell_id: String,
    pub cast: String,
    pub started_unix: u64,
    pub commands: Vec<CommandRecord>,
}
```

`meta_path_for(cast)` gives the `.meta.json` beside the cast; `MetaIndex::load`
reads it; and `src/shell/log.rs:199`:

```rust
/// Concatenated `"o"` payload bytes of the command at `index`, sliced from
/// the cast file by byte range. Skips `"i"` and `"m"` lines; a malformed line
/// inside the range is skipped rather than fatal.
pub fn read_command_output(cast: &Path, meta: &MetaIndex, index: u32) -> Result<Vec<u8>>
```

All four are already re-exported from `crate::shell` (`src/shell/mod.rs`), so
this phase adds no exports there.

**No flush gotcha.** `CastWriter` is documented as "flushed per record", so a
live shell's output is readable the moment it is written. Do not add a sync,
a sleep, or a retry.

### The formatting analogue — `read_pane`

`read_pane` (`src/daemon/executor/knowledge/pane.rs:94`) is the same shape:
clamp a depth, optionally regex-filter, handle the empty case with a sentence
rather than an empty string, mask, and emit header + body.

```rust
const READ_PANE_DEFAULT_LINES: usize = 200;
const READ_PANE_MAX_LINES: usize = 2000;

fn read_pane_depth(requested: Option<u64>, history_size: usize) -> usize {
    let requested = match requested {
        Some(n) if n > 0 => (n as usize).min(READ_PANE_MAX_LINES),
        _ => READ_PANE_DEFAULT_LINES,
    };
    if history_size > 0 { requested.min(history_size) } else { requested }
}
```

and its empty/filtered-empty branch, which is the behaviour to mirror:

```rust
    if filtered.is_empty() {
        return match grep {
            Some(p) => format!(
                "{} (window '{}'{} status:{}): no lines matched /{}/ in the last {} lines.",
                pane_id, window_name, sess_part, status, p, depth
            ),
            None => format!("{} (window '{}'{} status:{}): pane is empty.", …),
        };
    }
    let body = mask_sensitive(filtered.join("\n").trim_end());
```

`mask_sensitive` is `crate::ai::filter::mask_sensitive`; `normalize_output` is
`crate::daemon::utils::normalize_output` (trims trailing whitespace per line and
strips leading/trailing blank lines). **Use the same two, in that order:
normalize, then mask.**

### What 03a already built that you should reuse

`src/daemon/executor/knowledge/shell.rs` (332 lines) holds `ShellRow`,
`list_shells`, `list_shells_in`, `collect_row`, `format_shell_rows`,
`format_age`, `now_unix`. **`ShellRow` already carries `id` and
`started_unix`** — the two fields the cast path needs. Put `read_shell` in this
same module; do not create a second one.

## Current state

Values run against the tree at drafting (2026-09-12, commit `577a7e17`).

- `grep -c 'ToolDef {' src/ai/tools/defs.rs` → **40**.
- `read_shell` appears nowhere in `src/`:
  `grep -rc 'read_shell' --include=*.rs src/ | grep -v ':0' | wc -l` → **0**.
- `grep -c '40 tools: 28 core + 12 deferred' CLAUDE.md` → **1**;
  `grep -c 'The model has \*\*40 tools\*\*' README.md` → **1**.
- `src/daemon/executor/knowledge/shell.rs` is **332** lines;
  `src/daemon/executor/knowledge/mod.rs` re-exports `shell::list_shells` only.
- `cargo test --lib 'knowledge::shell'` → **8 passed**;
  `cargo test --lib 'ai::tools'` → **22**; `cargo test --test doc_truth` → **10**;
  lib suite **1828**; `daemon::executor::` **243**; `config::` **92**;
  `cargo test --test isolation` → **9**.
- `grep -c 'ShellRequest' src/shell/proto.rs` → **1** (the enum declaration);
  it must stay **1**.

## Spec

Seven tasks. Task 6 is the mutation checks and task 7 the end-to-end capture,
both as numbered Spec tasks — `docs/dev/WORKFLOW.md` § End-to-end verification
requires the capture to be a task, because the seeder parses only the section
headed exactly `## Spec`.

### Task 1 — the reader primitive

In `src/daemon/executor/knowledge/shell.rs`, add a function that resolves a
shell id to its recorded output. Keep the same split 03a used and that the
reviewer will check: **registry access in one place, pure logic everywhere
else.**

- Resolve the id through `with_shells` to get `started_unix`. Collect what you
  need and return it; do **not** hold the guard while reading files.
- `cast_path(id, started_unix)` → `meta_path_for(...)` → `MetaIndex::load`.
- Select command indices (task 2), concatenate their
  `read_command_output` bytes, lossily decode to a `String`.
- `normalize_output`, then apply `filter`, then `mask_sensitive`.

**The selection logic must be a pure function** taking `&MetaIndex` (or a slice
of `CommandRecord`) and the two optional params, returning the indices to read.
Name it `select_command_indices`; a criterion greps for it and asserts it names
no filesystem type.

### Task 2 — parameter semantics, each with its negative

- **`since_command: Option<u32>`** — include commands whose `index >=` the
  value. Absent: include **all** recorded commands.
  *Negative:* a value greater than every recorded index returns a sentence
  saying so and naming the highest index — **not** an empty body and **not**
  a silent fall back to "all".
- **`lines: Option<u64>`** — clamp the result to the **last** N lines, with the
  same defaults `read_pane` uses (**default 200, max 2000**, `0` or absent →
  default). Reuse those two constants' values; you may lift them to module
  constants in `shell.rs` rather than importing `pane.rs`'s private ones.
  *Negative:* a shell whose output is shorter than `lines` returns everything
  it has without padding or complaint.
- **`filter: Option<String>`** — a regex over lines; only matching lines are
  returned. An **invalid** regex returns an error naming the pattern and the
  parse error — it must **not** be treated as a literal string and must not be
  ignored.
  *Negative:* a valid regex that matches nothing returns "no lines matched
  /…/" — a sentence, not an empty body.
- **Unknown shell id** — an error naming the id. Because the model may be
  working from a stale listing, the message must tell it what to do next;
  saying which ids *are* live is the useful form, and `list_shells`'s collector
  is already in this module.
  *Negative:* the error must **not** be an `Err` that propagates — every tool
  result is a value the model reads. Return the string.
- **A live shell with no commands yet**, and **a shell whose cast or meta file
  is missing** — both return an explanatory sentence, not a panic and not an
  empty string.

### Task 3 — the tool definition

`ToolDef`: `name: "read_shell"`, `deferred_group: None`, `category:
Category::Read`, four params (`shell` **required**, `lines`, `since_command`,
`filter` optional). `shell` and `filter` are `ParamTy::Str`; `lines` and
`since_command` are `ParamTy::Int`.

**Describe only what exists on this tree.** This reads a shell's **recorded
output** (its command log), not a live screen buffer — say that, because the
distinction determines whether the model expects to see a half-finished
interactive program's display. Do **not** claim it can drive, interrupt or
write to the shell; nothing can yet.

**Do not add `read_shell` to `APPROVAL_GATED_TOOLS`** — it prompts for nothing,
and that list also exempts a tool from every budget cap in `daemon::stream`.

**Do not add any of its param names to `enum_values`** in
`src/ai/tools/schema.rs`. That function is keyed by **param name** globally, so
a `"filter"` or `"lines"` entry would apply to every tool sharing the name. A
criterion asserts `enum_values` is unchanged.

### Task 4 — wire the tool

All seven sites from § Pre-flight, **in one batch**, plus the pieces the
compiler cannot see:

- `src/ai/types/pending.rs`: the variant + `to_tool_call` + `id` + `tool_name`
  + **`summary`** + **`should_emit_tool_feedback`**.
- `src/ai/types/events.rs`: the `AiEvent::ReadShell` variant.
- `src/ai/tools/args.rs`: `ReadShellArgs` + its `ToolArgs` impl.
- `src/ai/tools/dispatch.rs`: the dispatch arm **and** a row in the
  conformant-args table (~line 241) — `dispatch_roundtrip_all_tools` fails
  without it.
- `src/daemon/stream.rs` and `src/daemon/executor/mod.rs`: the two match arms.
- `src/daemon/executor/knowledge/mod.rs`: extend the existing
  `pub(super) use shell::{…}` re-export.

### Task 5 — the two doc tables and the prompt asset

- `CLAUDE.md` § "Current AI tools": count line to **`41 tools: 29 core + 12
  deferred.`** plus a `read_shell` row, `Loaded` = `core`.
- `README.md` § "AI tools": a Core-table row and the three numbers at the
  "The model has **40 tools**" line → **41**, **29**, 12 unchanged. No `**⚠**`.
- `assets/prompts/sre.toml`: prose for the tool. **Same restrictions as 03a's
  bounce established** — name no tool that does not exist on this tree, cite
  **no internal milestone or phase identifier**, and claim **no behaviour that
  is not implemented**. In particular do not say the model can send input to a
  shell; `send_input` is a later phase.

### Task 6 — run the mutation checks and record them

Both mutations with the `patch` tool (`sed -i`, `perl -i` and `>` redirects are
forbidden by the executor contract). Capture each red-then-green transcript
mechanically. Each names the **property**; find the line yourself. **If a
mutation does not turn its test red, record exactly that — do not invent a
failure, and do not report an outcome the applied patch cannot produce.**

1. **An out-of-range `since_command` must not silently return everything.**
   Make the selector ignore `since_command` when it exceeds the highest index →
   the out-of-range test must go **red**.
2. **An invalid `filter` regex must not be treated as a literal.** Make the
   filter fall back to a substring match when the regex fails to compile → the
   invalid-regex test must go **red**.

After each, revert with `patch` and show the test green again.

When you quote a `test result:` line, **quote it from the run whose command you
are showing** — counts differ by filter.

### Task 7 — capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-m22-03b.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`, and record the `PASTE MATCH`
self-check verdict in the same entry. The server-authored `(complete)` entry
does not satisfy this, and neither does a summary asserting the block was
pasted.

Capture **fresh**; do not paste a file from an earlier attempt.

## Acceptance criteria

Each was run against the tree at drafting and returns the "now" value shown.
`<TD>` abbreviates
`sed -n '/name: "read_shell"/,/^    },/p' src/ai/tools/defs.rs |`.

- [ ] **The tool exists, core + Read:** `grep -c 'name: "read_shell"' src/ai/tools/defs.rs`
      → **1** (now `0`); `<TD> grep -c 'category: Category::Read'` → **1**;
      `<TD> grep -c 'deferred_group: None'` → **1**.
- [ ] **Four params, `shell` required:** `<TD> grep -c 'name: "shell"'` → **1**;
      `<TD> grep -cE 'name: "(lines|since_command|filter)"'` → **3**;
      `<TD> grep -c 'required: true'` → **1** (only `shell`).
- [ ] **Counts moved together:** `grep -c 'ToolDef {' src/ai/tools/defs.rs` →
      **41** (now `40`); `grep -c '41 tools: 29 core + 12 deferred' CLAUDE.md`
      → **1** (now `0`); `grep -c 'The model has \*\*41 tools\*\*' README.md` →
      **1** (now `0`); and the old lines are gone —
      `grep -c '40 tools: 28 core + 12 deferred' CLAUDE.md` → **0** (now `1`).
- [ ] **Not approval-gated:**
      `sed -n '/APPROVAL_GATED_TOOLS: &\[&str\] = &\[/,/^\];/p' src/ai/tools/defs.rs | grep -c 'read_shell'`
      → **0**.
- [ ] **`enum_values` untouched:**
      `sed -n '/pub(super) fn enum_values/,/^}/p' src/ai/tools/schema.rs | grep -cE '"filter"|"lines"|"since_command"|"shell"'`
      → **0**, and the same range `grep -c '"session", "knowledge", "incident"'`
      → **1**, unchanged.
- [ ] **No protocol change:** `grep -c 'pub enum ShellRequest' src/shell/proto.rs`
      → **1** and `grep -c 'pub enum ShellResponse' src/shell/proto.rs` → **1**,
      both unchanged; `git diff --stat -- src/shell/ | wc -l` → **0**.
- [ ] **The reader does not use the live-shell client:**
      `grep -cE 'crate::shell::status|run_command|spawn_command' src/daemon/executor/knowledge/shell.rs`
      → **0**.
- [ ] **The selector is pure:** `grep -c 'fn select_command_indices' src/daemon/executor/knowledge/shell.rs`
      → **1** (now `0`), and
      `sed -n '/fn select_command_indices/,/^}/p' src/daemon/executor/knowledge/shell.rs | grep -cE 'Path|File|with_shells|ShellRegistry'`
      → **0**.
- [ ] **The two compiler-invisible sites were done:**
      `sed -n '/fn should_emit_tool_feedback/,/^    }/p' src/ai/types/pending.rs | grep -c 'ReadShell'`
      → **1** (now `0`), and
      `sed -n '/fn summary(&self)/,/^    }/p' src/ai/types/pending.rs | grep -c 'ReadShell'`
      → **1** (now `0`). *(Neither is enforced by any gate — that is why they
      are pinned here.)*
- [ ] **The rest of the wiring:** `grep -c 'ReadShell' src/ai/types/pending.rs`
      → **5 or more**; `grep -c 'ReadShell' src/ai/types/events.rs` → **1**;
      `grep -c 'ReadShell' src/daemon/stream.rs` → **2 or more**;
      `grep -c 'ReadShell' src/daemon/executor/mod.rs` → **1 or more**;
      `grep -c 'read_shell' src/ai/tools/dispatch.rs` → **2 or more**;
      `grep -c 'ReadShellArgs' src/ai/tools/args.rs` → **2 or more**;
      `grep -c 'read_shell' assets/prompts/sre.toml` → **1 or more**. All now
      `0`.
- [ ] **The prompt asset stays honest** (03a's bounce):
      ``sed -n '/^### `read_shell`/,/^### /p' assets/prompts/sre.toml | grep -cE 'send_input|03b|03c|M22'``
      → **0**.
- [ ] **The seven tests exist by name:**
      `grep -cE 'fn (read_shell_returns_recorded_output|read_shell_since_command_selects_from_that_index|read_shell_since_command_past_the_end_says_so|read_shell_filter_selects_matching_lines|read_shell_invalid_filter_regex_is_an_error|read_shell_unknown_id_is_an_error_naming_live_shells|read_shell_lines_clamps_to_the_tail)' src/daemon/executor/knowledge/shell.rs`
      → **7** (now `0`).
- [ ] No new `unsafe` / `unwrap` / `expect` / `panic!` in production code: prod
      on `src/daemon/executor/knowledge/shell.rs`
      (`sed -n '1,/^#\[cfg(test)\]/p' … | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe '`)
      → **0**, unchanged.
- [ ] `cargo test --lib 'knowledge::shell'` → **15 or more**, `0 failed` (now
      **8**). *Arithmetic: 8 existing + 7 named above.*
- [ ] `cargo test --lib 'ai::tools'` → **22**, unchanged, `0 failed`.
- [ ] `cargo test --test doc_truth` → **10**, `0 failed`, unchanged.
- [ ] **Preservation:** `cargo test --lib 'config::'` → **92**;
      `cargo test --test isolation` → **9**; `cargo test --lib 'daemon::executor::'`
      → **250 or more** (now **243**; it grows by this phase's tests).
- [ ] Full lib suite **1835 or more**, `0 failed`, on **three consecutive runs**
      (now **1828**).
- [ ] All four gates pass.

## Test plan

Seven tests, co-located in `src/daemon/executor/knowledge/shell.rs`. Build a
cast file and a `MetaIndex` on a `tempfile::TempDir` — writing two or three
asciicast `"o"` records and a matching `MetaIndex` by hand is enough, and it
keeps the tests off the process-global registry the way 03a's do.

1. **`read_shell_returns_recorded_output`** — a shell with two recorded
   commands returns both commands' output.
2. **`read_shell_since_command_selects_from_that_index`** — `since_command: 2`
   returns command 2's output and **not** command 1's. Asserts both halves.
3. **`read_shell_since_command_past_the_end_says_so`** — an index above the
   highest returns the explanatory sentence and **no** command output. The
   load-bearing negative for mutation 1: an implementation that ignores the
   parameter returns everything and passes a message-only assertion.
4. **`read_shell_filter_selects_matching_lines`** — a regex returns only
   matching lines, and a regex matching nothing returns the "no lines matched"
   sentence rather than an empty body.
5. **`read_shell_invalid_filter_regex_is_an_error`** — `"["` (an unterminated
   character class) produces an error naming the pattern, and the output
   contains **no** shell content. Mutation 2's test: a substring fallback would
   silently return lines containing a literal `[`.
6. **`read_shell_unknown_id_is_an_error_naming_live_shells`** — an id absent
   from the registry returns an error mentioning the id, and the function
   returns a `String` (it must not be a `Result` the caller unwraps).
7. **`read_shell_lines_clamps_to_the_tail`** — output longer than `lines`
   returns the **last** N lines, not the first; and output shorter than `lines`
   returns everything without padding.

**Negatives are named on purpose.** A suite asserting only the positives passes
against a `read_shell` that ignores every optional parameter.

## End-to-end verification

The artifact is the tool as the model receives it plus the reader's behaviour,
so section B runs the module's tests and section C greps the structure.

Run from the repo root, verbatim:

```sh
A=/tmp/e2e-m22-03b.txt
: > "$A"
RUNS_BEFORE=$(ls ~/.daemoneye/var/log/runs/ 2>/dev/null | wc -l)
{
echo "== A. build =="
echo "real HOME run logs BEFORE: $RUNS_BEFORE"
cargo build 2>&1 | tail -2; echo "cargo_exit=${PIPESTATUS[0]}"

echo "== B. the module's tests =="
cargo test --lib 'knowledge::shell' -- --nocapture 2>&1 \
  | sed 's/\x1b\[[0-9;]*m//g' | grep -E '^test daemon::executor::knowledge::shell|^test result'
echo "cargo_exit=${PIPESTATUS[0]}"
cargo test --lib 'ai::tools' 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | grep -E '^test result'

echo "== C. structural greps (each must print the stated number) =="
D=src/ai/tools/defs.rs; S=src/ai/tools/schema.rs; K=src/daemon/executor/knowledge/shell.rs; P=src/ai/types/pending.rs
TD() { sed -n '/name: "read_shell"/,/^    },/p' $D; }
printf 'tool defined                 (1): %s\n' "$(grep -c 'name: "read_shell"' $D)"
printf 'category Read                (1): %s\n' "$(TD | grep -c 'category: Category::Read')"
printf 'deferred_group None          (1): %s\n' "$(TD | grep -c 'deferred_group: None')"
printf 'shell param                  (1): %s\n' "$(TD | grep -c 'name: "shell"')"
printf 'three optional params        (3): %s\n' "$(TD | grep -cE 'name: "(lines|since_command|filter)"')"
printf 'exactly one required         (1): %s\n' "$(TD | grep -c 'required: true')"
printf 'ToolDef literals            (41): %s\n' "$(grep -c 'ToolDef {' $D)"
printf 'categories assigned         (41): %s\n' "$(grep -c 'category: Category::' $D)"
printf 'not approval-gated           (0): %s\n' "$(sed -n '/APPROVAL_GATED_TOOLS: &\[&str\] = &\[/,/^\];/p' $D | grep -c 'read_shell')"
printf 'enum_values untouched        (0): %s\n' "$(sed -n '/pub(super) fn enum_values/,/^}/p' $S | grep -cE '"filter"|"lines"|"since_command"|"shell"')"
printf 'memory param set intact      (1): %s\n' "$(sed -n '/pub(super) fn enum_values/,/^}/p' $S | grep -c '"session", "knowledge", "incident"')"
printf 'ShellRequest unchanged       (1): %s\n' "$(grep -c 'pub enum ShellRequest' src/shell/proto.rs)"
printf 'src/shell untouched          (0): %s\n' "$(git diff --stat -- src/shell/ | wc -l)"
printf 'no live-client calls         (0): %s\n' "$(grep -cE 'crate::shell::status|run_command|spawn_command' $K)"
printf 'selector exists              (1): %s\n' "$(grep -c 'fn select_command_indices' $K)"
printf 'selector is pure             (0): %s\n' "$(sed -n '/fn select_command_indices/,/^}/p' $K | grep -cE 'Path|File|with_shells|ShellRegistry')"
printf 'should_emit arm              (1): %s\n' "$(sed -n '/fn should_emit_tool_feedback/,/^    }/p' $P | grep -c 'ReadShell')"
printf 'summary arm                  (1): %s\n' "$(sed -n '/fn summary(&self)/,/^    }/p' $P | grep -c 'ReadShell')"
printf 'pending wiring              (5+): %s\n' "$(grep -c 'ReadShell' $P)"
printf 'events wiring               (1+): %s\n' "$(grep -c 'ReadShell' src/ai/types/events.rs)"
printf 'stream wiring               (2+): %s\n' "$(grep -c 'ReadShell' src/daemon/stream.rs)"
printf 'executor wiring             (1+): %s\n' "$(grep -c 'ReadShell' src/daemon/executor/mod.rs)"
printf 'dispatch wiring             (2+): %s\n' "$(grep -c 'read_shell' src/ai/tools/dispatch.rs)"
printf 'args wiring                 (2+): %s\n' "$(grep -c 'ReadShellArgs' src/ai/tools/args.rs)"
printf 'sre.toml documents it       (1+): %s\n' "$(grep -c 'read_shell' assets/prompts/sre.toml)"
printf 'sre.toml stays honest        (0): %s\n' "$(sed -n '/^### `read_shell`/,/^### /p' assets/prompts/sre.toml | grep -cE 'send_input|03b|03c|M22')"
printf 'CLAUDE.md new count          (1): %s\n' "$(grep -c '41 tools: 29 core + 12 deferred' CLAUDE.md)"
printf 'CLAUDE.md old count gone     (0): %s\n' "$(grep -c '40 tools: 28 core + 12 deferred' CLAUDE.md)"
printf 'README new count             (1): %s\n' "$(grep -c 'The model has \*\*41 tools\*\*' README.md)"
printf 'seven tests                  (7): %s\n' "$(grep -cE 'fn (read_shell_returns_recorded_output|read_shell_since_command_selects_from_that_index|read_shell_since_command_past_the_end_says_so|read_shell_filter_selects_matching_lines|read_shell_invalid_filter_regex_is_an_error|read_shell_unknown_id_is_an_error_naming_live_shells|read_shell_lines_clamps_to_the_tail)' $K)"
printf 'prod unwrap/panic in module  (0): %s\n' "$(sed -n '1,/^#\[cfg(test)\]/p' $K | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe ')"

echo "== D. preservation: config 92, isolation 9, doc_truth 10 =="
for m in 'config::' 'daemon::executor::'; do
  printf '%-26s ' "$m"; cargo test --lib "$m" 2>&1 | grep '^test result'
done
printf '%-26s ' 'test isolation'; cargo test --test isolation 2>&1 | grep '^test result'
printf '%-26s ' 'test doc_truth'; cargo test --test doc_truth 2>&1 | grep '^test result'

echo "== E. lib suite, three consecutive runs =="
for i in 1 2 3; do cargo test --lib 2>&1 | grep '^test result'; done

echo "== F. gates =="
cargo fmt --all --check 2>&1 | tail -2; echo "fmt_exit=${PIPESTATUS[0]}"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -2; echo "clippy_exit=${PIPESTATUS[0]}"

echo "== G. no HOME litter from the suite =="
printf 'real HOME run logs AFTER (==BEFORE): %s\n' "$(ls ~/.daemoneye/var/log/runs/ 2>/dev/null | wc -l)"
} >> "$A" 2>&1
cat "$A"
```

Then the paste self-check. **Anchor on the transcript fence specifically** —
the entry will contain the script block too, so a naive "first fence after the
heading" lands on the script and reports a structural mismatch:

```sh
D=docs/dev/milestones/M22-tool-surface/phase-03b-read-shell.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==3{print} c==4{exit}' > /tmp/pasted-m22-03b.txt
diff /tmp/pasted-m22-03b.txt /tmp/e2e-m22-03b.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

*(`c==3` selects the third fence: the entry holds the `sh` script first, then
the transcript. If you lay the entry out differently, adjust the index and say
so in the entry.)*

Paste the captured block **unmodified** — do not retype, trim, or realign it.

## Authorizations

The executor may modify **only**:

- `src/daemon/executor/knowledge/shell.rs` — tasks 1–2. **Do not change
  `list_shells`, `list_shells_in`, `format_shell_rows` or the eight existing
  tests**; a criterion pins `knowledge::shell` at 15+ and the existing 8 must
  still pass.
- `src/daemon/executor/knowledge/mod.rs` — the re-export line only.
- `src/ai/types/pending.rs`, `src/ai/types/events.rs`, `src/ai/tools/args.rs`,
  `src/ai/tools/dispatch.rs`, `src/ai/tools/defs.rs`, `src/daemon/stream.rs`,
  `src/daemon/executor/mod.rs` — task 4, the `read_shell` wiring only. Do not
  alter any other tool's arms.
- `CLAUDE.md` (§ "Current AI tools" table + count line only), `README.md`
  (§ "AI tools" tables + the tool-count prose only), `assets/prompts/sre.toml` —
  task 5.
- `docs/dev/milestones/M22-tool-surface/phase-03b-read-shell.md` — **append to
  the Update Log only.** Do not rewrite, reorder or delete any section above it;
  the reviewer checks that every `## ` heading survives.

No other file. In particular **do not touch `src/shell/`** — `proto.rs`,
`log.rs`, `launch.rs` and the registry are read exactly as they are, and a
criterion asserts that directory is unchanged. Also do not touch
`tests/doc_truth.rs`, `src/daemon/executor/knowledge/pane.rs`, or
`src/config/`.

## Out of scope

- **Reading the live screen grid.** There is no protocol frame for it; see the
  Goal. A screen read needs `proto.rs` + `host.rs` + `client.rs` + `backend.rs`
  and is its own phase if the design still wants it after 03c.
- **`find_in_shells`.** Phase 03c — it reuses this phase's reader in a loop over
  03a's collector, plus match context and a cap.
- **`list_shell_logs` / `read_shell_log`** — the deferred `history` group over
  *past* shells, phase 06. This phase reads shells that are **in the
  registry**; an id that has aged out is an unknown id here.
- **`send_input` or any write path.** Phase 05, and approval-gated.
- **Removing `read_pane`.** The six pane tools go in phase 09; both tools
  coexist, which is why the count goes 40 → 41.
- **Retention or rotation of cast files.** Already handled by the M6 lifecycle
  policy; this phase only reads.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-09-13 04:33 (started)

**Executor:** executor (headless run, phase-03b `read_shell`)
**Status:** in-progress — flipped from `todo`; milestone README row updated to match.

Working through the Spec in order: the reader primitive and its parameter
semantics in the existing `knowledge/shell.rs` module (tasks 1–2), the tool
definition (task 3), the seven-site wiring batch (task 4), the two doc tables
and the prompt asset (task 5), then the two mutation checks and the
end-to-end capture (tasks 6–7). No deviations from the Spec yet.

### Update — 2026-09-13 (mutation checks)

Two red-then-green transcripts, each applied with `patch` and reverted with
`patch`. The properties broken are exactly the two the Spec names. The tree
was green and `knowledge::shell` at 15 passed before mutation 1; after each
revert `cargo test --lib 'knowledge::shell'` returned to **15 passed**, so the
mutant left no residue.

**Mutation 1 — an out-of-range `since_command` must not silently return
everything.** The selector was mutated to fall back to "all indices" whenever
`since_command` exceeds the highest recorded index, exactly the silent
out-of-range behaviour the property forbids. **First red run caught the mutant
not yet in the tree** (the patch landed after that run started; both runs' full
transcripts are kept, so the reader can see the ordering): the mutant's red run
is the second, and it fails by returning output instead of the sentence.

```
== MUTATION 1: out-of-range since_command silently returns everything (property: it must not) ==
-- red run (mutant freshly applied; the first green line is the pre-patch tree, second is the mutant):
test daemon::executor::knowledge::shell::tests::read_shell_since_command_past_the_end_says_so ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1838 filtered out; finished in 0.00s
cargo_exit=0
-- (mutant applied: a `Some(_) => all indices` arm added to select_command_indices)
test daemon::executor::knowledge::shell::tests::read_shell_since_command_past_the_end_says_so ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1838 filtered out; finished in 0.01s
cargo_exit=101
-- revert applied (arm removed, original match restored)
-- green run:
test daemon::executor::knowledge::shell::tests::read_shell_since_command_past_the_end_says_so ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1838 filtered out; finished in 0.00s
cargo_exit=0
-- green (whole module, reverted):
test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 1824 filtered out; finished in 0.01s
cargo_exit=0
```

**Mutation 2 — an invalid `filter` regex must not be treated as a literal.**
The filter's compile-failure arm was mutated to ignore the parse error and
compile an escaped literal instead — the substring fallback the property
forbids. Same ordering as mutation 1 (the mutant's red line is the second
run).

```
== MUTATION 2: invalid filter regex falls back to a literal (property: it must be an error) ==
-- red run (mutant freshly applied; first line is the pre-patch tree, second is the mutant):
test daemon::executor::knowledge::shell::tests::read_shell_invalid_filter_regex_is_an_error ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1838 filtered out; finished in 0.00s
cargo_exit=0
-- (mutant applied: the Err arm compiled regex::escape(p) instead of returning the error string)
test daemon::executor::knowledge::shell::tests::read_shell_invalid_filter_regex_is_an_error ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1838 filtered out; finished in 0.01s
cargo_exit=101
-- revert applied (Err arm restored to the error path)
-- green run:
test daemon::executor::knowledge::shell::tests::read_shell_invalid_filter_regex_is_an_error ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1838 filtered out; finished in 0.00s
cargo_exit=0
-- green (whole module, reverted):
test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 1824 filtered out; finished in 0.01s
cargo_exit=0
```

The `1838 filtered out` count above is the single-test filter's own arithmetic
(one test selected from a suite that by then held 1839), distinct from the
module filter's `1824 filtered out`. **When a `test result:` line is quoted
here it is from the same invocation whose command is shown beside it** — the
test counts differ by filter, and each line was captured from its own run; the
whole-module green runs quote the module filter's count.


### Update — 2026-09-13 (end-to-end verification)

Ran the phase doc's § End-to-end verification block **verbatim and unmodified**
from the repo root. Fresh capture for this run — no reuse of a prior
`/tmp/e2e-m22-03b.txt`. The transcript below is this run's capture file,
pasted byte-for-byte:

```sh
A=/tmp/e2e-m22-03b.txt
: > "$A"
RUNS_BEFORE=$(ls ~/.daemoneye/var/log/runs/ 2>/dev/null | wc -l)
{
echo "== A. build =="
echo "real HOME run logs BEFORE: $RUNS_BEFORE"
cargo build 2>&1 | tail -2; echo "cargo_exit=${PIPESTATUS[0]}"

echo "== B. the module's tests =="
cargo test --lib 'knowledge::shell' -- --nocapture 2>&1 \
  | sed 's/\x1b\[[0-9;]*m//g' | grep -E '^test daemon::executor::knowledge::shell|^test result'
echo "cargo_exit=${PIPESTATUS[0]}"
cargo test --lib 'ai::tools' 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | grep -E '^test result'

echo "== C. structural greps (each must print the stated number) =="
D=src/ai/tools/defs.rs; S=src/ai/tools/schema.rs; K=src/daemon/executor/knowledge/shell.rs; P=src/ai/types/pending.rs
TD() { sed -n '/name: "read_shell"/,/^    },/p' $D; }
printf 'tool defined                 (1): %s\n' "$(grep -c 'name: "read_shell"' $D)"
printf 'category Read                (1): %s\n' "$(TD | grep -c 'category: Category::Read')"
printf 'deferred_group None          (1): %s\n' "$(TD | grep -c 'deferred_group: None')"
printf 'shell param                  (1): %s\n' "$(TD | grep -c 'name: "shell"')"
printf 'three optional params        (3): %s\n' "$(TD | grep -cE 'name: "(lines|since_command|filter)"')"
printf 'exactly one required         (1): %s\n' "$(TD | grep -c 'required: true')"
printf 'ToolDef literals            (41): %s\n' "$(grep -c 'ToolDef {' $D)"
printf 'categories assigned         (41): %s\n' "$(grep -c 'category: Category::' $D)"
printf 'not approval-gated           (0): %s\n' "$(sed -n '/APPROVAL_GATED_TOOLS: &\[&str\] = &\[/,/^\];/p' $D | grep -c 'read_shell')"
printf 'enum_values untouched        (0): %s\n' "$(sed -n '/pub(super) fn enum_values/,/^}/p' $S | grep -cE '"filter"|"lines"|"since_command"|"shell"')"
printf 'memory param set intact      (1): %s\n' "$(sed -n '/pub(super) fn enum_values/,/^}/p' $S | grep -c '"session", "knowledge", "incident"')"
printf 'ShellRequest unchanged       (1): %s\n' "$(grep -c 'pub enum ShellRequest' src/shell/proto.rs)"
printf 'src/shell untouched          (0): %s\n' "$(git diff --stat -- src/shell/ | wc -l)"
printf 'no live-client calls         (0): %s\n' "$(grep -cE 'crate::shell::status|run_command|spawn_command' $K)"
printf 'selector exists              (1): %s\n' "$(grep -c 'fn select_command_indices' $K)"
printf 'selector is pure             (0): %s\n' "$(sed -n '/fn select_command_indices/,/^}/p' $K | grep -cE 'Path|File|with_shells|ShellRegistry')"
printf 'should_emit arm              (1): %s\n' "$(sed -n '/fn should_emit_tool_feedback/,/^    }/p' $P | grep -c 'ReadShell')"
printf 'summary arm                  (1): %s\n' "$(sed -n '/fn summary(&self)/,/^    }/p' $P | grep -c 'ReadShell')"
printf 'pending wiring              (5+): %s\n' "$(grep -c 'ReadShell' $P)"
printf 'events wiring               (1+): %s\n' "$(grep -c 'ReadShell' src/ai/types/events.rs)"
printf 'stream wiring               (2+): %s\n' "$(grep -c 'ReadShell' src/daemon/stream.rs)"
printf 'executor wiring             (1+): %s\n' "$(grep -c 'ReadShell' src/daemon/executor/mod.rs)"
printf 'dispatch wiring             (2+): %s\n' "$(grep -c 'read_shell' src/ai/tools/dispatch.rs)"
printf 'args wiring                 (2+): %s\n' "$(grep -c 'ReadShellArgs' src/ai/tools/args.rs)"
printf 'sre.toml documents it       (1+): %s\n' "$(grep -c 'read_shell' assets/prompts/sre.toml)"
printf 'sre.toml stays honest        (0): %s\n' "$(sed -n '/^### `read_shell`/,/^### /p' assets/prompts/sre.toml | grep -cE 'send_input|03b|03c|M22')"
printf 'CLAUDE.md new count          (1): %s\n' "$(grep -c '41 tools: 29 core + 12 deferred' CLAUDE.md)"
printf 'CLAUDE.md old count gone     (0): %s\n' "$(grep -c '40 tools: 28 core + 12 deferred' CLAUDE.md)"
printf 'README new count             (1): %s\n' "$(grep -c 'The model has \*\*41 tools\*\*' README.md)"
printf 'seven tests                  (7): %s\n' "$(grep -cE 'fn (read_shell_returns_recorded_output|read_shell_since_command_selects_from_that_index|read_shell_since_command_past_the_end_says_so|read_shell_filter_selects_matching_lines|read_shell_invalid_filter_regex_is_an_error|read_shell_unknown_id_is_an_error_naming_live_shells|read_shell_lines_clamps_to_the_tail)' $K)"
printf 'prod unwrap/panic in module  (0): %s\n' "$(sed -n '1,/^#\[cfg(test)\]/p' $K | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe ')"

echo "== D. preservation: config 92, isolation 9, doc_truth 10 =="
for m in 'config::' 'daemon::executor::'; do
  printf '%-26s ' "$m"; cargo test --lib "$m" 2>&1 | grep '^test result'
done
printf '%-26s ' 'test isolation'; cargo test --test isolation 2>&1 | grep '^test result'
printf '%-26s ' 'test doc_truth'; cargo test --test doc_truth 2>&1 | grep '^test result'

echo "== E. lib suite, three consecutive runs =="
for i in 1 2 3; do cargo test --lib 2>&1 | grep '^test result'; done

echo "== F. gates =="
cargo fmt --all --check 2>&1 | tail -2; echo "fmt_exit=${PIPESTATUS[0]}"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -2; echo "clippy_exit=${PIPESTATUS[0]}"

echo "== G. no HOME litter from the suite =="
printf 'real HOME run logs AFTER (==BEFORE): %s\n' "$(ls ~/.daemoneye/var/log/runs/ 2>/dev/null | wc -l)"
} >> "$A" 2>&1
cat "$A"
```

The block's own output, arrived at by running exactly the script above — this
is the transcript the block produced and it is what was appended inside the
`{ … } >> "$A"` above:

```
== A. build ==
real HOME run logs BEFORE: 0
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.06s
cargo_exit=0
== B. the module's tests ==
test daemon::executor::knowledge::shell::tests::formatter_reports_a_single_shell_without_plural_splitting ... ok
test daemon::executor::knowledge::shell::tests::list_shells_rejects_an_unknown_scope ... ok
test daemon::executor::knowledge::shell::tests::list_shells_scope_match_is_case_sensitive ... ok
test daemon::executor::knowledge::shell::tests::list_shells_empty_registry_says_so ... ok
test daemon::executor::knowledge::shell::tests::list_shells_defaults_to_the_callers_own_shells ... ok
test daemon::executor::knowledge::shell::tests::formatter_marks_flags_and_exits_without_the_registry ... ok
test daemon::executor::knowledge::shell::tests::list_shells_orders_numerically_not_lexicographically ... ok
test daemon::executor::knowledge::shell::tests::list_shells_all_scope_includes_other_owners ... ok
test daemon::executor::knowledge::shell::tests::read_shell_invalid_filter_regex_is_an_error ... ok
test daemon::executor::knowledge::shell::tests::read_shell_since_command_past_the_end_says_so ... ok
test daemon::executor::knowledge::shell::tests::read_shell_unknown_id_is_an_error_naming_live_shells ... ok
test daemon::executor::knowledge::shell::tests::read_shell_filter_selects_matching_lines ... ok
test daemon::executor::knowledge::shell::tests::read_shell_returns_recorded_output ... ok
test daemon::executor::knowledge::shell::tests::read_shell_since_command_selects_from_that_index ... ok
test daemon::executor::knowledge::shell::tests::read_shell_lines_clamps_to_the_tail ... ok
test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 1824 filtered out; finished in 0.01s
cargo_exit=0
test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 1817 filtered out; finished in 0.00s
== C. structural greps (each must print the stated number) ==
tool defined                 (1): 1
category Read                (1): 1
deferred_group None          (1): 1
shell param                  (1): 1
three optional params        (3): 3
exactly one required         (1): 1
ToolDef literals            (41): 41
categories assigned         (41): 41
not approval-gated           (0): 0
enum_values untouched        (0): 0
memory param set intact      (1): 1
ShellRequest unchanged       (1): 1
src/shell untouched          (0): 0
no live-client calls         (0): 0
selector exists              (1): 1
selector is pure             (0): 0
should_emit arm              (1): 1
summary arm                  (1): 1
pending wiring              (5+): 6
events wiring               (1+): 1
stream wiring               (2+): 2
executor wiring             (1+): 1
dispatch wiring             (2+): 2
args wiring                 (2+): 2
sre.toml documents it       (1+): 2
sre.toml stays honest        (0): 0
CLAUDE.md new count          (1): 1
CLAUDE.md old count gone     (0): 0
README new count             (1): 1
seven tests                  (7): 7
prod unwrap/panic in module  (0): 0
== D. preservation: config 92, isolation 9, doc_truth 10 ==
config::                   test result: ok. 92 passed; 0 failed; 0 ignored; 0 measured; 1747 filtered out; finished in 0.00s
daemon::executor::         test result: ok. 250 passed; 0 failed; 4 ignored; 0 measured; 1585 filtered out; finished in 1.08s
test isolation             test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.16s
test doc_truth             test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
== E. lib suite, three consecutive runs ==
test result: ok. 1835 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 8.16s
test result: ok. 1835 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 8.43s
test result: ok. 1835 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 8.15s
== F. gates ==
fmt_exit=0
    Checking daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 6.04s
clippy_exit=0
== G. no HOME litter from the suite ==
real HOME run logs AFTER (==BEFORE): 0
```

Read: every structural grep returns its stated number, `knowledge::shell` is
15 passed including the seven named tests, `ai::tools` 22 / 0, doc_truth 10 /
0, config 92, isolation 9, `daemon::executor::` 250 (243 + 7 new), and the
lib suite is **1835 passed / 0 failed** on three consecutive runs. Fmt and
clippy exit 0. `src/shell/` has a 0-line diff (the screen half was not
touched) and no HOME litter (0 run logs before == after).

Paste self-check, run after appending this entry:

```sh
D=docs/dev/milestones/M22-tool-surface/phase-03b-read-shell.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==3{print} c==4{exit}' > /tmp/pasted-m22-03b.txt
diff /tmp/pasted-m22-03b.txt /tmp/e2e-m22-03b.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

Verdict recorded after running it: `PASTE MATCH`. *(The `c==3` index selects
this entry's transcript fence — the entry holds the `sh` script first and the
transcript second, exactly as the phase doc's note anticipates.)*

### Update — 2026-09-13 (grep proof for the spec-pinned literals)

Per the executor contract's grep requirement for pinned byte sequences — the
tool-name string in the `ToolDef`, and the seven specced test names:

```sh
$ grep -c 'name: "read_shell"' src/ai/tools/defs.rs
1
$ sed -n '/name: "read_shell"/,/^    },/p' src/ai/tools/defs.rs | grep -c 'category: Category::Read'
1
$ grep -cE 'fn (read_shell_returns_recorded_output|read_shell_since_command_selects_from_that_index|read_shell_since_command_past_the_end_says_so|read_shell_filter_selects_matching_lines|read_shell_invalid_filter_regex_is_an_error|read_shell_unknown_id_is_an_error_naming_live_shells|read_shell_lines_clamps_to_the_tail)' src/daemon/executor/knowledge/shell.rs
7
```

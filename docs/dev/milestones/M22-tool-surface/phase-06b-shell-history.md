# Phase 06b: `list_shell_logs` / `read_shell_log` — the deferred `history` group

**Milestone:** M22 — Tool surface & context
**Status:** done
**Depends on:** 03b (`read_shell`'s reader shape and error sentences) and
**06a**, which gave the `.meta.json` index a producer. Both **done**. Without
06a this phase would query a file that is never written.
**Estimated diff:** ~520 lines incl. tests
**Tags:** language=rust, kind=feature, size=m

## Goal

The shell tools so far all address a **live** shell in the registry. This adds
the two that address the **recording on disk**, so a past session is a query
rather than a scan. After this phase:

1. `list_shell_logs { since?, limit? }` enumerates the recordings in
   `~/.daemoneye/var/log/shells/`, live and finished alike, newest first.
2. `read_shell_log { log, command_index?, lines?, filter? }` reads one
   recording by its **log id** — the filename stem, not the bare shell id.
3. Both are the deferred **`history`** group: omitted from the default render,
   pulled in by `load_tools`.
4. Tool counts move to **48 tools: 34 core + 14 deferred**.
5. A retention defect that **06a made live** is fixed: `.meta.json` sidecars
   are currently never swept.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.5, the row:
  `list_shell_logs { host?, owner?, since?, limit? }` /
  `read_shell_log { id, command_index?, offset?, lines?, filter? }` |
  **history** (deferred) | — | *"Review past shell sessions … Backed by the
  `.meta.json` index, so 'show me the last time anyone ran `pg_ctl` on db01'
  is a query, not a scan."* **Three of those parameters do not exist on this
  tree** — § Pre-flight measures which and why.
- `docs/dev/milestones/M22-tool-surface/README.md` § Notes — the tool
  arithmetic (46 → **48**) and the `log.rs` raw-bytes-versus-decoded-payload
  hazard, which this phase stays clear of by reading only through
  `MetaIndex` and `read_command_output`.
- `CLAUDE.md` § "Adding a new AI tool (checklist)" — eleven steps, twice.
  Step 12 (`APPROVAL_GATED_TOOLS`) does **not** apply: both tools are
  read-only.

## Pre-flight

### The deferred group needs no machinery — this is free

`history` becomes a real group the moment a `ToolDef` carries
`deferred_group: Some("history")`. Everything downstream derives from `TOOLS`:

```rust
pub fn tools_in_group(group: &str) -> Vec<&'static str> {
    TOOLS
        .iter()
        .filter(|t| t.deferred_group == Some(group))
        .map(|t| t.name)
        .collect()
}
```

`select_tools` (`src/ai/tools/schema.rs:220`) filters on
`deferred_group.is_none()`, `deferred_catalog_text` builds the catalogue, and
the `LoadTools` executor arm (`src/daemon/executor/mod.rs:606`) resolves the
name through `tools_in_group` and reports "Unknown group(s)" when it comes
back empty. **Add no validation list and no match arm** — there is no
hard-coded set of group names anywhere, and adding one would be a second
source of truth.

### Measured: three of the design's parameters cannot be implemented

Same discipline as `open_shell { host?, label? }` in the 04a row. What a
recording on disk actually consists of:

```
~/.daemoneye/var/log/shells/
  s0-1788636479.cast            <- a cast with no index (pre-06a, or no command yet)
  s3-1788470000.cast
  s3-1788470000.meta.json       <- the index 06a now writes
```

and the whole of what the index carries:

```rust
pub struct MetaIndex {
    pub shell_id: String,
    pub cast: String,
    pub started_unix: u64,
    pub commands: Vec<CommandRecord>,
}
pub struct CommandRecord {
    pub index: u32,
    pub command: String,
    pub started: f64,
    pub ended: f64,
    pub exit_code: i32,
    pub first_byte: u64,
    pub end_byte: u64,
}
```

- **`host?` — no host concept exists.** Nothing on disk or in `ShellRecord`
  records one; hosts arrive with M21/M23. Omit the parameter.
- **`owner?` — not recoverable for a past shell.** `Owner` lives only in
  `ShellRecord`, which is the **live** registry; a finished shell's owner is
  gone with the process, and neither the filename nor `MetaIndex` carries it.
  Omit it. *(A future phase that writes the owner into the index can add it.)*
- **`offset?` — redundant and leaky.** `command_index` addresses a command in
  O(1) and `lines` tails it; a byte offset would expose cast-file internals to
  the model, and § Goal's point is that this is a **query**. Omit it.

**Ship `list_shell_logs { since?, limit? }` and
`read_shell_log { log, command_index?, lines?, filter? }`.** Do not add
`host`, `owner`, `offset`, or substitutes.

### `read_shell_log` collides with an existing private function — rename it first

`src/daemon/executor/knowledge/shell.rs:234` already has:

```rust
fn read_shell_log(
    reg: &ShellRegistry,
    id: &str,
    since_command: Option<u32>,
) -> Result<ShellOutput, String> {
```

That is 03b's **live-shell** reader (registry → cast → index → bytes), called
from `read_shell_in` and `find_in_shells_in`. The new *tool* needs the name.

**Rename the private one to `read_recorded_output`** — which is what it does —
and update its **two** call sites (`:349`, `:532`) plus the three doc comments
that name it (`:637`, `:926`, and its own). Mechanical, no behaviour change;
`knowledge::shell` must stay at **44** tests, green, by name.

### The log id is the filename stem, not the shell id

A shell id is reused: `s0` after a daemon restart is a different shell from
today's `s0`. The recordings are distinguished by `<id>-<started_unix>`, which
is exactly what `parse_cast_name` (`src/shell/registry.rs:274`) parses:

```rust
pub fn parse_cast_name(name: &str) -> Option<(ShellId, u64)> {
    let body = name.strip_suffix(".cast")?;
    let (id_part, ts_part) = body.rsplit_once('-')?;
    let id = ShellId::parse(id_part).ok()?;
    let started_unix = ts_part.parse::<u64>().ok()?;
    Some((id, started_unix))
}
```

**`read_shell_log`'s `log` parameter is that stem** — `"s3-1788470000"` — and
`list_shell_logs` reports ids in exactly that form. A bare `"s3"` is a
**refusal**, not a guess: § Test plan pins it, because silently picking "the
newest s3" is how a post-mortem reads the wrong incident. Say so in the
refusal text and name a valid id from the listing.

### The retention defect 06a made live — fix it here

`meta_path_for` writes `<stem>.meta.json`:

```rust
    if let Some(stem) = lossy.strip_suffix(".cast") {
        cast.with_file_name(format!("{stem}.meta.json"))
```

so `s3-1788470000.cast` → `s3-1788470000.meta.json`, which is what the real
log directory shows. But the sweep (`src/daemon/utils/mod.rs:148-152`) only
recognises a different spelling:

```rust
        let (extension, stem) = if let Some(stem) = name.strip_suffix(".cast") {
            (".cast", stem)
        } else if let Some(stem) = name.strip_suffix(".cast.meta.json") {
            (".meta.json", stem)
```

`s3-1788470000.meta.json` does not end in `.cast.meta.json`, so **no sidecar
is ever swept.** Its test passes because the fixture builds the sweep's
spelling rather than the producer's:

```rust
        let sidecar = logs_dir.join("s7-1788699323-shell.cast.meta.json");
```

Before 06a nothing wrote sidecars, so nothing leaked. **06a made it live** —
every indexed shell now leaves a file retention cannot reach, and
`list_shell_logs` would report those stale entries forever. Fix it here: the
sweep accepts the producer's spelling, and its fixture is built with
`meta_path_for` so writer and test cannot disagree again. *(This is the fourth
instance in this file family of a test agreeing with the wrong shape — see the
milestone README's `log.rs` hazard note.)*

### Reading a past log — use the index and the existing reader, nothing new

`read_shell_log` is `MetaIndex::load` + `read_command_output`, both of which
06a's producer now feeds. **Do not re-implement slicing, and do not touch
`windowed_payload` or `marked_inner_range`** — the milestone README records
that seam as a design hazard and this phase has no business in it.

`read_shell`'s own shape is the model to copy for the rendering, clamps and
masking: default 200 lines, cap 2000, `mask_sensitive` on the body, an invalid
`filter` regex is an error rather than a literal match. Build the regex the
way every other reader in the module does:
`regex::RegexBuilder::new(p).size_limit(1 << 20).build()`.

### Both tools are read-only

`category: Category::Read`, **not** in `APPROVAL_GATED_TOOLS`, and **both join
the `should_emit_tool_feedback` chain** (they are silent, non-gated tools —
`CLAUDE.md` checklist step 2). `summary()` gets an arm for each: the `since`
or the `log`. This is the same polarity as 03a–04b and the opposite of
`send_input`.

## Current state

Values run against the tree at drafting (2026-09-15, commit `1d413b78`).

- `grep -c 'ToolDef {' src/ai/tools/defs.rs` → **46**;
  `grep -c 'deferred_group: Some' src/ai/tools/defs.rs` → **12**.
- Existing groups: `agents` 4, `runs` 3, `runbooks` 2, `scripts` 2,
  `memory` 1. No `history`.
- `grep -c '46 tools: 34 core + 12 deferred' CLAUDE.md` → **1**;
  `grep -c 'The model has \*\*46 tools\*\*' README.md` → **1**.
- `grep -c 'fn read_shell_log' src/daemon/executor/knowledge/shell.rs` → **1**
  (the private live-shell reader);
  `grep -c 'fn read_recorded_output' …` → **0**.
- `grep -c 'cast.meta.json' src/daemon/utils/mod.rs` → **2** (the sweep arm and
  its test fixture).
- Tests: `knowledge::shell` **44**, `daemon::utils` (see § E) , `ai::tools`
  **22**, `doc_truth` **10**, `isolation` **9**, full lib **1878**.

## Spec

Eight tasks. Task 7 is the mutation checks and task 8 the end-to-end capture,
both numbered Spec tasks — `docs/dev/WORKFLOW.md` § End-to-end verification
requires the capture to be a task, because the seeder parses only `## Spec`.

### Task 1 — fix the sidecar sweep

In `src/daemon/utils/mod.rs`, make the sweep recognise the spelling
`meta_path_for` actually produces (`<stem>.meta.json`) as well as the one it
already handles. **Rebuild the existing test's fixture with
`crate::shell::meta_path_for`** rather than a hand-written name, so the test
and the producer cannot drift apart again, and add a second case asserting a
`.meta.json` sidecar **younger** than retention survives. Change no other
sweep behaviour.

### Task 2 — rename the private reader

`read_shell_log` → `read_recorded_output` in
`src/daemon/executor/knowledge/shell.rs`, with its two call sites and the doc
comments that name it. No behaviour change; `knowledge::shell` stays **44**.

### Task 3 — `list_shell_logs`

In `src/daemon/executor/knowledge/shell.rs`, a public
`list_shell_logs(since: Option<u64>, limit: Option<u64>) -> String` and a
directory-injected seam `list_shell_logs_in(dir, since, limit) -> String`
mirroring the `list_shells` / `list_shells_in` split.

- Scan the directory with `parse_cast_name`; ignore anything that does not
  parse (the sidecars, and any stray file).
- For each, load `MetaIndex` **best-effort**: a cast with no index is still
  **listed**, with its command count shown as unknown rather than 0 — a
  pre-06a recording and a shell that has run nothing are different facts and
  the listing must not equate them.
- Newest first by `started_unix`. `since` keeps entries at or after that unix
  timestamp; `limit` caps the rows (default 50, cap 500) and the output says
  when it clamped.
- Each row: the **log id** (`s3-1788470000`), an age, the command count, and
  the first command's text as a hint of what the session was.
- An empty directory says so plainly and names the directory.

### Task 4 — `read_shell_log`

A public `read_shell_log(log, command_index, lines, filter) -> String` plus a
directory-injected seam.

- **Refuse a bare shell id** (`"s3"`) naming the id, explaining that a log id
  carries the start timestamp, and pointing at `list_shell_logs`. Refuse an
  unknown log id the same way, listing what is available.
- `command_index` selects one command by its `CommandRecord.index`; absent
  reads every recorded command in order. An index past the highest recorded is
  an error naming the highest — the shape `read_shell`'s `since_command`
  already uses.
- `lines` tails (default 200, cap 2000); `filter` is a regex over the lines,
  invalid = error.
- Body masked with `mask_sensitive`. The header names the log id, the command
  count, and for a single command its **exit code** — the thing a post-mortem
  is looking for.
- A log with a cast but **no index** is not an error: say the recording
  predates the command index or recorded nothing, and that `read_shell` can
  read it live if the shell is still running.

### Task 5 — the two tool definitions

Both `deferred_group: Some("history")`, both `category: Category::Read`,
neither approval-gated.

- `list_shell_logs`: `since` (`ParamTy::Int`, optional, "unix timestamp"),
  `limit` (`ParamTy::Int`, optional, "default 50, capped at 500").
- `read_shell_log`: `log` (`ParamTy::Str`, **required**), `command_index`
  (`ParamTy::Int`, optional), `lines` (`ParamTy::Int`, optional), `filter`
  (`ParamTy::Str`, optional).
- Descriptions must say the `log` id is the one `list_shell_logs` prints, that
  these read **finished as well as live** recordings, and must **name no
  parameter that does not exist** — no `host`, no `owner`, no `offset`. Cite
  no milestone or phase identifier.

**Do not add `log`, `since` or `command_index` to `enum_values`** in
`src/ai/tools/schema.rs`; that map is keyed by param name globally.

### Task 6 — wire both tools, and the docs

The five compiler-forced sites **in one contiguous batch, then run the
verifier once** — `to_tool_call()`, `id()`, `tool_name()`, the `AiEvent` match
in `stream.rs`, and the `execute_tool_call` arms — plus `args.rs`,
`dispatch.rs` (both arms **and** both conformant-args rows), the two `AiEvent`
variants, the `knowledge/mod.rs` re-export, and the two compiler-invisible
arms (`summary`, and **both** tools joining `should_emit_tool_feedback`).

Docs: `CLAUDE.md` count line → **`48 tools: 34 core + 14 deferred.`** plus two
rows with `Loaded` = **history**; `README.md` § "AI tools" — a new
`| `history` | `list_shell_logs`, `read_shell_log` |` row in the **Deferred**
table and the numbers **46 → 48**, 34 unchanged, **12 → 14**; and a
`### `list_shell_logs` / `read_shell_log`` section in
`assets/prompts/sre.toml` that says the group must be loaded with
`load_tools` first.

### Task 7 — run the mutation checks and record them

Both with the `patch` tool (`sed -i`, `perl -i` and `>` redirects are
forbidden by the executor contract). **These mutations target code that does
not exist yet, so unlike a bug doc's they are NOT pre-measured — measuring
them is your job.** Apply each and run its test **six times**: a valid
mutation reddens **6 of 6**. If one comes back green in any run, the
*mutation* is wrong rather than the test — say so plainly and propose one that
always moves an observable. (This milestone already shipped a criterion
demanding a mutant go red that never could: chunk bundling made the mutant
byte-identical to correct code, and two runs burned budget on it.) Never
invent a failure.

1. **A bare shell id must be refused, not resolved.** Make `read_shell_log`
   accept a bare `"s3"` by picking the newest matching recording → the
   bare-id refusal test must go **red**.
2. **A cast with no index must still be listed.** Make `list_shell_logs_in`
   skip entries whose `MetaIndex` fails to load → the
   listing-includes-unindexed test must go **red**.

After each, revert with `patch` and show the test green again. Quote each
`test result:` line from the run whose command you are showing.

### Task 8 — capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, paste
the resulting `/tmp/e2e-m22-06b.txt` into **one** new Update Log entry headed
`### Update — <date> (end-to-end verification)`, and record the `PASTE MATCH`
verdict in the same entry. If you must re-capture, replace that entry's fenced
block rather than appending a second entry, and say so.

**Section F runs the lib suite three times.** A run failing **only**
`pty_stop_with_no_foreground_job_remembers_nothing` is the pre-existing flake
recorded in the milestone README — record it, re-run, say so. **Any other
failing name is real**: file it, do not re-run until green.

## Acceptance criteria

Each was run against the tree at drafting and returns the "now" value shown.

- [ ] **Both tools exist, deferred `history`, read-only:**
      `grep -c 'name: "list_shell_logs"' src/ai/tools/defs.rs` → **1** (now
      `0`), same for `read_shell_log` → **1** (now `0`);
      `grep -c 'deferred_group: Some("history")' src/ai/tools/defs.rs` → **2**
      (now `0`);
      `sed -n '/APPROVAL_GATED_TOOLS: &\[&str\] = &\[/,/^\];/p' src/ai/tools/defs.rs | grep -cE 'list_shell_logs|read_shell_log'`
      → **0**.
- [ ] **The group resolves with no new machinery:**
      `grep -rc '"history"' --include=*.rs src/ai/tools/schema.rs src/daemon/executor/mod.rs | grep -v ':0' | wc -l`
      → **0** — no hard-coded group list was added anywhere.
- [ ] **No phantom parameters:**
      `sed -n '/name: "list_shell_logs"/,/^    },/p' src/ai/tools/defs.rs | grep -cE 'name: "(host|owner|offset)"'`
      → **0**, and the same for `read_shell_log` → **0**.
- [ ] **Counts moved together:** `grep -c 'ToolDef {' src/ai/tools/defs.rs` →
      **48** (now `46`); `grep -c 'deferred_group: Some' src/ai/tools/defs.rs`
      → **14** (now `12`);
      `grep -c '48 tools: 34 core + 14 deferred' CLAUDE.md` → **1** (now `0`);
      `grep -c '46 tools: 34 core + 12 deferred' CLAUDE.md` → **0** (now `1`);
      `grep -c 'The model has \*\*48 tools\*\*' README.md` → **1** (now `0`);
      `grep -c '| `history` |' README.md` → **1** (now `0`).
- [ ] **The private reader was renamed, not duplicated:**
      `grep -c 'fn read_recorded_output' src/daemon/executor/knowledge/shell.rs`
      → **1** (now `0`), and
      `grep -c '^fn read_shell_log(' src/daemon/executor/knowledge/shell.rs` →
      **0** (now `1`). *(Corrected at review: the original dropped the `^…(`
      anchor and demanded **0** for the bare substring — impossible, because
      this very phase adds the public `fn read_shell_log(` tool and four test
      names embedding the string, so the literal reads **5**. The executor
      flagged it, held the intent, and did not contort the code; the intent —
      the old private reader renamed, not duplicated — is pinned by
      `fn read_recorded_output` → 1 and exactly one column-0 definition of
      `read_shell_log`. Architect error, no defect.)*
- [ ] **The sidecar sweep matches the producer's spelling:**
      `sed -n '/fn sweep_shell_logs/,/^}/p' src/daemon/utils/mod.rs | grep -c 'meta_path_for\|"\.meta\.json"'`
      → **1 or more**, and the sweep's test builds its fixture from the
      producer: `grep -c 'meta_path_for' src/daemon/utils/mod.rs` → **1 or
      more** (now **0**).
- [ ] **Both compiler-invisible arms, in the silent direction:**
      `sed -n '/pub fn should_emit_tool_feedback/,/^    }/p' src/ai/types/pending.rs | grep -cE 'ListShellLogs|ReadShellLog'`
      → **2** (now `0`), and
      `sed -n '/pub fn summary(&self)/,/^    }/p' src/ai/types/pending.rs | grep -cE 'ListShellLogs|ReadShellLog'`
      → **2** (now `0`).
- [ ] **The rest of the wiring:**
      `grep -cE 'ListShellLogs|ReadShellLog' src/ai/types/pending.rs` → **8 or
      more**; `… src/ai/types/events.rs` → **2**; `… src/daemon/stream.rs` →
      **4 or more**; `… src/daemon/executor/mod.rs` → **2 or more**;
      `grep -cE 'list_shell_logs|read_shell_log' src/ai/tools/dispatch.rs` →
      **4 or more**;
      `grep -cE 'ListShellLogsArgs|ReadShellLogArgs' src/ai/tools/args.rs` →
      **4 or more**;
      `grep -cE 'list_shell_logs|read_shell_log' assets/prompts/sre.toml` →
      **2 or more**. All now `0`.
- [ ] **`enum_values` untouched:**
      `sed -n '/pub(super) fn enum_values/,/^}/p' src/ai/tools/schema.rs | grep -cE '"log"|"since"|"command_index"'`
      → **0**, and the same range `grep -c '"session", "knowledge", "incident"'`
      → **1**, unchanged.
- [ ] **The `log.rs` hazard was not touched:**
      `git diff 1d413b78..HEAD --stat -- src/shell/log.rs src/shell/pty.rs | wc -l`
      → **0**.
- [ ] **The seven new tests exist by name** (tree-wide matching lines, so file
      placement does not matter):
      `grep -rE '^\s*(async )?fn (list_shell_logs_is_newest_first|list_shell_logs_includes_a_cast_with_no_index|list_shell_logs_since_and_limit_filter|read_shell_log_refuses_a_bare_shell_id|read_shell_log_reads_one_command_with_its_exit_code|read_shell_log_unknown_log_is_an_error_naming_what_exists|read_shell_log_invalid_filter_is_an_error)' --include=*.rs src/ | wc -l`
      → **7** (now `0`).
- [ ] No new `unsafe` / `unwrap` / `expect` / `panic!` in **production** code:
      `sed -n '1,/^#\[cfg(test)\]/p' src/daemon/executor/knowledge/shell.rs | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe '`
      → **1**, unchanged (the marker-carrying lazy-regex `expect`).
- [ ] **No lint-silencing shim:**
      `git diff 1d413b78..HEAD -- 'src/*.rs' | grep -cE '^\+.*#\[allow'` → **0**.
- [ ] `cargo test --lib 'knowledge::shell'` → **51 or more**, `0 failed` (now
      **44**). *Arithmetic: 44 + 7 named above.*
- [ ] `cargo test --lib 'ai::tools'` → **22**, unchanged;
      `cargo test --test doc_truth` → **10**, unchanged;
      `cargo test --lib 'daemon::utils'` → **+1 or more** over its value in
      § E of the capture (the sweep gains a case).
- [ ] **Preservation:** `cargo test --lib 'config::'` → **92**;
      `cargo test --test isolation` → **9**;
      `cargo test --lib 'daemon::executor::shell_exec'` → **13**, unchanged.
- [ ] Full lib suite **1886 or more**, `0 failed`, three consecutive runs (now
      **1878**).
- [ ] All four gates pass.

## Test plan

Seven new tests. All are directory-driven and need **no PTY host**: build a
temp log dir, write `.cast` + `.meta.json` pairs with `CastWriter` and
`MetaIndex::save`, and drive the `_in` seams. The module's existing
`fixture()` helpers (`knowledge/shell.rs`) already do exactly this — copy that
shape.

1. **`list_shell_logs_is_newest_first`** — three recordings with different
   `started_unix`; the rows come back newest first and each row carries its
   full log id (`s3-1788470000`), not a bare `s3`.
2. **`list_shell_logs_includes_a_cast_with_no_index`** — a cast with **no**
   sidecar is listed, and its row distinguishes "unknown" from a recording
   whose index exists with zero commands. Mutation 2's test. **Assert both
   halves**: a suite that only checks the row exists passes an implementation
   that prints `0 commands` for an unindexed cast, which is the wrong fact.
3. **`list_shell_logs_since_and_limit_filter`** — `since` excludes an older
   recording and includes one exactly at the boundary; `limit` caps and the
   output says it clamped.
4. **`read_shell_log_refuses_a_bare_shell_id`** — `"s3"` is refused, the
   message names `"s3"` and points at `list_shell_logs`, **and no recording is
   read** even though exactly one `s3-*` exists. Mutation 1's test: picking
   the newest match would satisfy a weaker assertion.
5. **`read_shell_log_reads_one_command_with_its_exit_code`** — a two-command
   index; `command_index: 1` returns the second command's output, **not** the
   first's, and the header carries that command's real **non-zero** exit code
   so the assertion cannot pass on a default.
6. **`read_shell_log_unknown_log_is_an_error_naming_what_exists`** — an absent
   log id returns text naming it and listing the available ids, and the
   function returns a `String`, not a `Result`.
7. **`read_shell_log_invalid_filter_is_an_error`** — `"[unclosed"` is an error
   naming the pattern and saying it is applied as a regex, returned **before**
   any file is read.

**Negatives are named on purpose.** A suite asserting only that logs list and
read passes against an implementation that silently resolves a bare id to the
wrong session, hides every pre-06a recording, and reports a missing index as
zero commands.

## End-to-end verification

Run from the repo root, verbatim:

```sh
A=/tmp/e2e-m22-06b.txt
: > "$A"
CASTS_BEFORE=$(ls ~/.daemoneye/var/log/shells/ 2>/dev/null | wc -l)
{
echo "== A. build =="
echo "real HOME shell logs BEFORE: $CASTS_BEFORE"
cargo build 2>&1 | tail -2; echo "cargo_exit=${PIPESTATUS[0]}"

echo "== B. the touched modules' tests =="
for m in 'knowledge::shell' 'daemon::utils' 'ai::tools' 'daemon::executor::shell_exec'; do
  printf '%-32s ' "$m"; cargo test --lib "$m" 2>&1 | grep '^test result'
done

echo "== C. the group resolves from TOOLS alone, shown not asserted =="
echo "--- the only thing that defines a group ---"
sed -n '/pub fn tools_in_group/,/^}/p' src/ai/tools/schema.rs
echo "--- history members ---"
grep -B12 'deferred_group: Some("history")' src/ai/tools/defs.rs | grep 'name: "'

echo "== D. structural greps (each must print the stated number) =="
D=src/ai/tools/defs.rs; S=src/ai/tools/schema.rs; K=src/daemon/executor/knowledge/shell.rs
P=src/ai/types/pending.rs; U=src/daemon/utils/mod.rs
printf 'list_shell_logs defined      (1): %s\n' "$(grep -c 'name: "list_shell_logs"' $D)"
printf 'read_shell_log defined       (1): %s\n' "$(grep -c 'name: "read_shell_log"' $D)"
printf 'history group members        (2): %s\n' "$(grep -c 'deferred_group: Some("history")' $D)"
printf 'neither approval-gated       (0): %s\n' "$(sed -n '/APPROVAL_GATED_TOOLS: &\[&str\] = &\[/,/^\];/p' $D | grep -cE 'list_shell_logs|read_shell_log')"
printf 'no hard-coded group list     (0): %s\n' "$(grep -rc '"history"' --include=*.rs $S src/daemon/executor/mod.rs | grep -v ':0' | wc -l)"
printf 'list has no phantom params   (0): %s\n' "$(sed -n '/name: "list_shell_logs"/,/^    },/p' $D | grep -cE 'name: "(host|owner|offset)"')"
printf 'read has no phantom params   (0): %s\n' "$(sed -n '/name: "read_shell_log"/,/^    },/p' $D | grep -cE 'name: "(host|owner|offset)"')"
printf 'ToolDef literals            (48): %s\n' "$(grep -c 'ToolDef {' $D)"
printf 'deferred total              (14): %s\n' "$(grep -c 'deferred_group: Some' $D)"
printf 'private reader renamed       (1): %s\n' "$(grep -c 'fn read_recorded_output' $K)"
printf 'old private name gone        (0): %s\n' "$(grep -c 'fn read_shell_log' $K)"
printf 'sweep uses producer spelling(1+): %s\n' "$(grep -c 'meta_path_for' $U)"
printf 'should_emit arms             (2): %s\n' "$(sed -n '/pub fn should_emit_tool_feedback/,/^    }/p' $P | grep -cE 'ListShellLogs|ReadShellLog')"
printf 'summary arms                 (2): %s\n' "$(sed -n '/pub fn summary(&self)/,/^    }/p' $P | grep -cE 'ListShellLogs|ReadShellLog')"
printf 'pending wiring              (8+): %s\n' "$(grep -cE 'ListShellLogs|ReadShellLog' $P)"
printf 'events wiring                (2): %s\n' "$(grep -cE 'ListShellLogs|ReadShellLog' src/ai/types/events.rs)"
printf 'stream wiring               (4+): %s\n' "$(grep -cE 'ListShellLogs|ReadShellLog' src/daemon/stream.rs)"
printf 'executor wiring             (2+): %s\n' "$(grep -cE 'ListShellLogs|ReadShellLog' src/daemon/executor/mod.rs)"
printf 'dispatch wiring             (4+): %s\n' "$(grep -cE 'list_shell_logs|read_shell_log' src/ai/tools/dispatch.rs)"
printf 'args wiring                 (4+): %s\n' "$(grep -cE 'ListShellLogsArgs|ReadShellLogArgs' src/ai/tools/args.rs)"
printf 'enum_values untouched        (0): %s\n' "$(sed -n '/pub(super) fn enum_values/,/^}/p' $S | grep -cE '"log"|"since"|"command_index"')"
printf 'memory param set intact      (1): %s\n' "$(sed -n '/pub(super) fn enum_values/,/^}/p' $S | grep -c '"session", "knowledge", "incident"')"
printf 'log.rs/pty.rs untouched      (0): %s\n' "$(git diff 1d413b78..HEAD --stat -- src/shell/log.rs src/shell/pty.rs | wc -l)"
printf 'CLAUDE new count             (1): %s\n' "$(grep -c '48 tools: 34 core + 14 deferred' CLAUDE.md)"
printf 'CLAUDE old count gone        (0): %s\n' "$(grep -c '46 tools: 34 core + 12 deferred' CLAUDE.md)"
printf 'README new count             (1): %s\n' "$(grep -c 'The model has \*\*48 tools\*\*' README.md)"
printf 'README history row           (1): %s\n' "$(grep -c '| `history` |' README.md)"
printf 'sre.toml documents both     (2+): %s\n' "$(grep -cE 'list_shell_logs|read_shell_log' assets/prompts/sre.toml)"
printf 'seven new tests              (7): %s\n' "$(grep -rE '^\s*(async )?fn (list_shell_logs_is_newest_first|list_shell_logs_includes_a_cast_with_no_index|list_shell_logs_since_and_limit_filter|read_shell_log_refuses_a_bare_shell_id|read_shell_log_reads_one_command_with_its_exit_code|read_shell_log_unknown_log_is_an_error_naming_what_exists|read_shell_log_invalid_filter_is_an_error)' --include=*.rs src/ | wc -l)"
printf 'prod unwrap knowledge        (1): %s\n' "$(sed -n '1,/^#\[cfg(test)\]/p' $K | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe ')"
printf 'no new allow shim            (0): %s\n' "$(git diff 1d413b78..HEAD -- 'src/*.rs' | grep -cE '^\+.*#\[allow')"

echo "== E. preservation =="
for m in 'config::' 'daemon::executor::shell_exec'; do
  printf '%-30s ' "$m"; cargo test --lib "$m" 2>&1 | grep '^test result'
done
printf '%-30s ' 'test isolation'; cargo test --test isolation 2>&1 | grep '^test result'
printf '%-30s ' 'test doc_truth'; cargo test --test doc_truth 2>&1 | grep '^test result'

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

Then the paste self-check, transcript as the **first** fenced block after the
heading:

```sh
D=docs/dev/milestones/M22-tool-surface/phase-06b-shell-history.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m22-06b.txt
diff /tmp/pasted-m22-06b.txt /tmp/e2e-m22-06b.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

Paste the captured block **unmodified** — do not retype, trim, or realign it.

## Authorizations

The executor may modify **only**:

- `src/daemon/utils/mod.rs` — task 1: the sidecar suffix arm and the sweep's
  test. **Change no other sweep behaviour** and do not touch the other
  `sweep_*` functions.
- `src/daemon/executor/knowledge/shell.rs` — tasks 2–4. **Do not change
  `list_shells`, `read_shell`, `find_in_shells`, `open_shell`, `close_shell`,
  `watch_shell`, `send_input` or any of the 44 existing tests** beyond the
  mechanical rename of `read_shell_log` → `read_recorded_output`.
- `src/daemon/executor/knowledge/mod.rs` — the re-export line only.
- `src/ai/types/pending.rs`, `src/ai/types/events.rs`, `src/ai/tools/args.rs`,
  `src/ai/tools/dispatch.rs`, `src/ai/tools/defs.rs`, `src/daemon/stream.rs`,
  `src/daemon/executor/mod.rs` — task 6's wiring only.
- `CLAUDE.md` (§ "Current AI tools" table + count line), `README.md`
  (§ "AI tools" tables + the three numbers), `assets/prompts/sre.toml` —
  task 6's docs.
- `docs/dev/milestones/M22-tool-surface/phase-06b-shell-history.md` —
  **append to the Update Log only.** Do not rewrite, reorder or delete any
  section above it.

No other file. In particular **do not touch `src/shell/log.rs` or
`src/shell/pty.rs`** — the milestone README records that file's
raw-bytes-versus-decoded-payload seam as a design hazard that produced four
defects in 06a, and this phase reads only through `MetaIndex` and
`read_command_output`; a criterion asserts both files are unchanged. Also do
not touch `src/shell/registry.rs`, `src/shell/launch.rs`,
`src/daemon/executor/shell_exec.rs`, `tests/doc_truth.rs` or `src/config/`.

## Out of scope

- **`host?` and `owner?` on `list_shell_logs`.** Neither exists on this tree:
  there is no host concept, and a finished shell's owner is gone with the
  process that held the registry. They arrive when M21/M23 give the registry
  host support and something writes the owner into the index.
- **`offset?` on `read_shell_log`.** `command_index` + `lines` address the
  same need without exposing cast byte offsets to the model.
- **Backfilling an index for pre-06a casts.** Those recordings list with an
  unknown command count and read with an explanatory message; rebuilding
  their index would mean re-deriving marker ranges for casts whose nonces are
  gone.
- **Indexing background commands** (`bg_shell.rs`'s `spawn_command` path).
  Still out of scope, as in 06a; `list_shell_logs` will show those shells'
  casts with no index, which is accurate.
- **The `extend_output_line` accumulate-and-rewindow hazard** recorded in the
  milestone README after 06a. `read_shell_log` reads whole command records
  through `read_command_output`, so it does not hit that path; deciding
  whether the reader windows per record or per stream is its own change.
- **Removing any pane tool.** Phase 09.

## Update Log

<!-- entries appended below this line -->
### Update — 2026-09-15 20:24 (started)

**Executor:** executor-run-06b-1 (headless). Status flipped `todo →
in-progress`; README row matches. Work order: task 1 (sweep), task 2 (rename),
tasks 3–4 (the two readers), task 5 (defs), task 6 (wiring + docs), task 7
(mutations), task 8 (e2e capture).

### Update — 2026-09-15 20:22 (end-to-end verification)

Captured mechanically from the repo root with the phase doc's § End-to-end
verification block, verbatim and unmodified, redirected to
`/tmp/e2e-m22-06b.txt`. § F's three lib-suite runs came back `0 failed` on all
three passes — the recorded `pty_stop_with_no_foreground_job_remembers_nothing`
flake did not fire, so there was nothing to re-run.

**One structural grep needs reading, not repair:**
`old private name gone        (0): 5`. The criterion's literal
`grep -c 'fn read_shell_log'` over-matches on this tree: the phase's own Task 4
requires the *public tool function* `read_shell_log` (`fn read_shell_log(` at
`knowledge/shell.rs:625`) and the Test plan requires four tests whose names
embed the literal (`fn read_shell_log_refuses_a_bare_shell_id`, …). The intent —
*the old private live-shell reader is gone, renamed not duplicated* — is
provably held: the anchor-qualified grep `grep -cE '^fn read_shell_log\('`
(now the only unanchored `fn read_shell_log` at column 0) returns **1**, and it
is the public tool; `fn read_recorded_output` returns **1** at
`:234`. No `read_shell_log`-named private helper remains. Recorded here because
the captured block must stay verbatim.

```sh
== A. build ==
real HOME shell logs BEFORE: 3
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.09s
cargo_exit=0
== B. the touched modules' tests ==
knowledge::shell                 test result: ok. 51 passed; 0 failed; 0 ignored; 0 measured; 1839 filtered out; finished in 4.55s
daemon::utils                    test result: ok. 110 passed; 0 failed; 0 ignored; 0 measured; 1780 filtered out; finished in 0.05s
ai::tools                        test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 1868 filtered out; finished in 0.00s
daemon::executor::shell_exec     test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 1877 filtered out; finished in 0.07s
== C. the group resolves from TOOLS alone, shown not asserted ==
--- the only thing that defines a group ---
pub fn tools_in_group(group: &str) -> Vec<&'static str> {
    TOOLS
        .iter()
        .filter(|t| t.deferred_group == Some(group))
        .map(|t| t.name)
        .collect()
}
--- history members ---
                name: "limit",
                name: "filter",
== D. structural greps (each must print the stated number) ==
list_shell_logs defined      (1): 1
read_shell_log defined       (1): 1
history group members        (2): 2
neither approval-gated       (0): 0
no hard-coded group list     (0): 0
list has no phantom params   (0): 0
read has no phantom params   (0): 0
ToolDef literals            (48): 48
deferred total              (14): 14
private reader renamed       (1): 1
old private name gone        (0): 5
sweep uses producer spelling(1+): 3
should_emit arms             (2): 2
summary arms                 (2): 2
pending wiring              (8+): 12
events wiring                (2): 2
stream wiring               (4+): 4
executor wiring             (2+): 2
dispatch wiring             (4+): 6
args wiring                 (4+): 4
enum_values untouched        (0): 0
memory param set intact      (1): 1
log.rs/pty.rs untouched      (0): 0
CLAUDE new count             (1): 1
CLAUDE old count gone        (0): 0
README new count             (1): 1
README history row           (1): 1
sre.toml documents both     (2+): 4
seven new tests              (7): 7
prod unwrap knowledge        (1): 1
no new allow shim            (0): 0
== E. preservation ==
config::                       test result: ok. 92 passed; 0 failed; 0 ignored; 0 measured; 1798 filtered out; finished in 0.00s
daemon::executor::shell_exec   test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 1877 filtered out; finished in 0.07s
test isolation                 test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.16s
test doc_truth                 test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
== F. lib suite, three consecutive runs (failure names preserved) ==
test result: ok. 1886 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 12.65s
test result: ok. 1886 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 12.65s
test result: ok. 1886 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 12.68s
== G. gates ==
fmt_exit=0
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s
clippy_exit=0
== H. no HOME litter from the suite ==
real HOME shell logs AFTER (==BEFORE): 3
```
### Update — 2026-09-15 20:35 (mutation checks)

Both mutation runs, each applied with `patch` and reverted with `patch`, the
named test run **six times** with the mutant applied (six reds required) and
three times after the revert (greens confirming the revert is clean).

**Mutation 1 — `read_shell_log` accepts a bare `"s3"` by picking the newest
matching recording.** Applied a best-match resolver at the bare-id branch of
`read_shell_log_in` (`read_shell_log_in(dir, &stem, …)` on the newest match),
with the resolver itself a whole new private fn. Applied run →
`read_shell_log_refuses_a_bare_shell_id` **red 6/6** — every run panicked with
"a bare shell id is a refusal, not a read", because the mutant read the one
`s3-*` recording and its text came back in place of the refusal.

```
cargo test --lib 'knowledge::shell::tests::read_shell_log_refuses_a_bare_shell_id'
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1889 filtered out; finished in 0.01s
```

Reverted with `patch` (both hunks), then **green 3/3**:

```
cargo test --lib 'knowledge::shell::tests::read_shell_log_refuses_a_bare_shell_id'
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1889 filtered out; finished in 0.00s
```

**Mutation 2 — `list_shell_logs_in` skips entries whose `MetaIndex` fails to
load.** Applied a `rows_skip` vec + `rows.retain(...)` filtering every row
whose sidecar does not load. Applied run →
`list_shell_logs_includes_a_cast_with_no_index` **red 6/6** — the unindexed
`s5-1788470500.cast` vanished from the listing and the test's
`out.find("s5-1788470500").expect(...)` panicked.

```
cargo test --lib 'knowledge::shell::tests::list_shell_logs_includes_a_cast_with_no_index'
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1889 filtered out; finished in 0.01s
```

Reverted with `patch`, then **green 3/3**:

```
cargo test --lib 'knowledge::shell::tests::list_shell_logs_includes_a_cast_with_no_index'
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1889 filtered out; finished in 0.01s
```

After both reverts the module is back at **51 passed, 0 failed**, and
`grep -c 'fn read_recorded_output' …` → 1 proves the mutation scaffolding left
no residue.

### Update — ts=1789507925539 (complete, server-authored)

**Summary:** **Summary**

Phase 06b is implemented and committed (`b88dda49`, working tree clean). `list_shell_logs { since?, limit? }` and `read_shell_log { log, command_index?, lines?, filter? }` now address shell recordings on disk rather than the live registry: the listing scans `~/.daemoneye/var/log/shells/` newest-first by log id (`s3-1788470000`), reports an unindexed cast as `commands: unknown` (never 0), clamps `limit` (default 50, cap 500) with a stated note, and the reader refuses a bare shell id, validates the `filter` regex before any read, selects a command by index with its real exit code in the header (an index past the highest names the highest), masks the body, and explains rather than errors when a cast has no sidecar. Both tools are `Category::Read`, deferred under the new `history` group — which resolves from `ToolDef.deferred_group` alone, no group list added — and join `should_emit_tool_feedback`/`summary` in the silent direction. Task 1 fixed the retention defect 06a made live (the sweep now accepts `meta_path_for`'s `<stem>.meta.json` spelling and its test builds the fixture through the producer), and Task 2 renamed 03b's private live-shell reader to `read_recorded_output` with its two call sites and doc comments. Counts moved to **48 tools: 34 core + 14 deferred** across `CLAUDE.md`, `README.md` and `sre.toml`; `tests/doc_truth` (10) stays green against the real `TOOLS` table.

**Notes for review**

- **One acceptance-criterion grep is impossible as written and was recorded, not repaired:** `grep -c 'fn read_shell_log' src/daemon/executor/knowledge/shell.rs` returns **5**, not the stated **0**, because the phase's own Task 4 requires the public `fn read_shell_log(` and the Test plan requires four test names embedding that literal. The criterion's intent (the old *private* reader is renamed, not duplicated) is provably held: `fn read_recorded_output` → 1 (`:234`), and the only column-0 `fn read_shell_log(` is the public tool at `:625`. I reasoned this intent-vs-literal tension was an over-match rather than a defect worth blocking on; the captured e2e block shows the literal `5` and the update log entry explains it.
- Two dispatch tests pinned to the old counts (`every_tool_has_a_category`, `category_counts_match_the_assignment`, and the `deferred_group_split_is_total` table) were updated to 48/23-Read/14-deferred — those live in `src/ai/tools/dispatch.rs`, which the Authorizations list covers for task 6's wiring.
- Mutation checks (task 7) both reddened **6/6** with the mutant and **3/3** after revert, recorded with the quoted `test result:` lines in the phase doc's Update Log.
- Task 8's end-to-end capture ran the § End-to-end verification block verbatim into `/tmp/e2e-m22-06b.txt` (§ F: lib suite **1886 passed, 0 failed, 4 ignored**, three consecutive runs — the recorded pty flake did not fire), and the paste self-check reports **PASTE MATCH**.
- All four gates pass: `cargo fmt --all --check`, `cargo build` (0 new warnings), `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --lib` 1886/0, plus `config::` 92, `isolation` 9, `doc_truth` 10, `shell_exec` 13, `ai::tools` 22, `daemon::utils` 110 (+1), `knowledge::shell` **51** — all preserved. Status remains `in-progress` with no `(complete)` entry, per the contract.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s


TEST
 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_reads_bare_word ... ok
test header_status_strips_trailing_prose ... ok
test header_status_uses_first_occurrence_only ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 10 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test readme_tools_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_config_template_documents_every_config_field ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ghost_config_parsing ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test resume_run_request_roundtrip ... ok
test minimal_config_parsing ... ok
test window_switch_does_not_corrupt_chat ... ignored
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test event_log_entry_format ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test g4_briefing_read_and_clear ... ok
test g4_briefing_injects_on_next_run ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 10 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test config_contains_webhook_and_stub_url ... ok
test hooks_land_on_private_server ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.16s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Compiling daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.74s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-9810e08c3de1e001)
     Running unittests src/main.rs (target/debug/deps/daemoneye-54ad57cf3704bc40)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-d709c9b862b5691d)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-34cb94d3012a3d69)
     Running tests/integration.rs (target/debug/deps/integration-deb70358b520d0d5)
     Running tests/isolation.rs (target/debug/deps/isolation-45facff22bac60f5)
   Doc-tests daemoneye

```

**Files changed:**

- `CLAUDE.md` — +3 -1
- `README.md` — +4 -1
- `assets/prompts/sre.toml` — +25 -0
- `docs/dev/milestones/M22-tool-surface/README.md` — +1 -1
- `docs/dev/milestones/M22-tool-surface/phase-06b-shell-history.md` — +147 -1
- `src/ai/tools/args.rs` — +49 -0
- `src/ai/tools/defs.rs` — +76 -0
- `src/ai/tools/dispatch.rs` — +13 -5
- `src/ai/types/events.rs` — +19 -0
- `src/ai/types/pending.rs` — +80 -0
- `src/daemon/executor/knowledge/mod.rs` — +2 -2
- `src/daemon/executor/knowledge/shell.rs` — +1796 -1113
- `src/daemon/executor/mod.rs` — +17 -0
- `src/daemon/stream.rs` — +30 -0
- `src/daemon/utils/mod.rs` — +48 -4

**Commit:** b88dda49ae4ba8dd0d2f39ffb114cb7d70d89b6a

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-09-15

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark (448 turns)
- **Scope deviations:** one, ruled in scope — `src/ai/tools/dispatch.rs`'s
  `category_counts_match_the_assignment` and the deferred-split table were
  updated to 48 / 14. That file is in § Authorizations for task 6's wiring and
  the census is exactly the compiler-invisible test the phase had to move.
- **Calibration:** one architect-side criterion was **impossible as written**
  and is corrected in place — `grep -c 'fn read_shell_log' … → 0` cannot hold
  in a phase that *adds* `fn read_shell_log`. Same family as this milestone's
  other instrument errors, now eight, and the third of the
  sub-species "the criterion forbids the thing the phase creates".

**All four gates re-run at review as separate invocations** (clippy after
`touch src/lib.rs`): fmt clean, build 0 warnings, clippy `-D warnings` exit 0,
`cargo test --lib` → **1886 passed, 0 failed**. `knowledge::shell` 51,
`daemon::utils` 110, `ai::tools` 22, `shell_exec` 13, `config::` 92,
`doc_truth` 10, `isolation` 9.

**Every acceptance criterion re-run: all pass**, including `ToolDef` 48,
deferred 14, the `history` group with **no hard-coded group list added
anywhere**, no phantom `host`/`owner`/`offset` parameter on either tool,
`enum_values` untouched, and `src/shell/log.rs` + `src/shell/pty.rs`
**unchanged** — the phase stayed out of the seam that produced four defects
in 06a, as designed.

**Both mutations re-run by hand, and my first attempt at each was a bad
mutant — worth recording.**

- *Bare id resolved, not refused.* My first mutant disabled the shape gate,
  which merely fell through to the unknown-log error — an observable the test
  correctly tolerates. Applying the mutation the Spec actually describes
  (resolve `"s3"` to the newest matching recording): **6 of 6 red**, green on
  revert. The test's load-bearing assertion is the negative,
  `!out.contains("the only s3 recording")`.
- *Unindexed cast skipped.* My first mutant `continue`d in the enrichment
  loop, which is a **no-op** here: the implementation scans in two passes and
  pushes every cast with `commands: None` before loading any index, so the
  Spec's single-pass mutation is unexpressible against it — better code than
  the spec assumed. The mutant that matches the structure (`rows.retain(|r|
  r.commands.is_some())`): **6 of 6 red**. A bonus mutant rendering `unknown`
  as `0 commands` is **3 of 3 red**, so both halves of that test discriminate.

That is the second phase running where a *mutation* turned out to be the
proxy, not the test — see the calibration note in 06a's verdict. Constructing
the mutant against the code as written, rather than as the spec imagined it,
is the habit this milestone keeps asking for.

**The end-to-end capture is genuine:** 68 lines, all eight sections A–H,
§ F three consecutive runs at **1886 passed, 0 failed** with the known `pty`
flake not firing, and `PASTE MATCH` recorded in the same entry.

**What the phase delivered:** `list_shell_logs` / `read_shell_log` as the
deferred `history` group — **48 tools: 34 core + 14 deferred** — plus the
retention fix for the sidecar sweep that 06a made live, with its test fixture
now built through `meta_path_for` so writer and test cannot drift apart
again. The three design parameters that do not exist on this tree
(`host`, `owner`, `offset`) stayed out.

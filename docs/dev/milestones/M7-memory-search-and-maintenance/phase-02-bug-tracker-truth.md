# Phase 02: Bug-Tracker Truth

**Milestone:** M7 — Memory Search & Maintenance
**Status:** in-progress
**Depends on:** phase-01 (dependency-currency, done)
**Estimated diff:** ~200 lines (one new test file) + 5 one-line doc status edits
**Tags:** language=rust, kind=test, size=m

## Goal

Five bug docs are marked `open` against phases that are `done` — two of them
`blocker`. All five were verified fixed. A tracker that reports open blockers
against shipped milestones trains everyone to ignore it, so this phase lands a
**test that fails when it happens again**, and closes the five docs that test
catches.

## Architecture references

None — this phase adds no runtime behavior and touches no design. It ships a
repo-hygiene test only.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any file.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

There are 46 bug docs under `docs/dev/milestones/*/bugs/`. Nothing checks them
against their phase's status, so they drift silently.

**The architect has already validated the exact algorithm below against the real
tree.** It produces precisely five findings and no false positives. You are
implementing a specified algorithm, not designing one.

### The five violations the gate must find

```
docs/dev/milestones/M2-tui-renderer/bugs/bug-phase-01-1.md    (bug=open, phase=done)
docs/dev/milestones/M2-tui-renderer/bugs/bug-phase-01-2.md    (bug=open, phase=done)
docs/dev/milestones/M2-tui-renderer/bugs/bug-phase-02b-1.md   (bug=open, phase=done)
docs/dev/milestones/M2-tui-renderer/bugs/bug-phase-02b-2.md   (bug=open, phase=done)
docs/dev/milestones/M4-context-management/bugs/bug-09-1.md    (bug=open, phase=done)
```

### Four parsing facts, each of which will bite you if you guess

**1. A status line is not a bare word.** It carries trailing prose. All of these
are real lines from the tree:

```
**Status:** verified
**Status:** open
**Status:** closed 2026-07-30 — verified at review round 2
**Status:** fixed (architect takeover, 2026-06-27)
**Status:** resolved (run 3, commit `edc51f9`) — 2026-07-26
```

Parse the **first whitespace-delimited token** after the `**Status:**` marker,
lowercase it, and strip any trailing `(`, `,`, `.`, `—` characters. That yields
`verified`, `open`, `closed`, `fixed`, `resolved` for the five lines above.

**2. Use only the FIRST `**Status:**` line in a file.** Three phase docs contain
a second one *inside an Update Log entry* — e.g.
`docs/dev/milestones/M3-polish-maintenance/phase-05-consolidate-leaf-params.md`
has the real header status `done` at line 4 and an unrelated
`**Status:** All 5 functions converted. Build, clippy, fmt, and tests pass.` at
line 279. Taking the last match, or all matches, reads the wrong one.

**3. Bug filenames use two conventions.** Both appear in the tree:

```
bug-phase-01-1.md    -> phase id "01"    (M1, M2)
bug-09-1.md          -> phase id "09"    (M4, M5, M6)
bug-phase-02b-1.md   -> phase id "02b"
bug-04f-1.md         -> phase id "04f"
```

So: strip the leading `bug-`, then strip an *optional* leading `phase-`, then
split off the trailing `-<n>` — what remains is the phase id. The id may carry a
letter suffix (`02b`, `04f`, `06a`).

**4. The phase doc is found by prefix, not by exact name.** For phase id `02b`
in milestone dir `M2-tui-renderer`, the phase doc is the file matching
`phase-02b-*.md` — here `phase-02b-tools-and-default.md`. Match on the
`phase-<id>-` prefix.

### The status vocabularies actually in use

Both were derived by scanning every doc in the tree; nothing outside these sets
occurs today.

- **Bug statuses:** `open` (the only non-terminal), plus the terminal set
  `closed`, `fixed`, `resolved`, `verified` — four synonyms for "done", all in
  live use. The gate accepts all four; normalising them is **out of scope**.
- **Phase statuses:** `done`, `in-progress`, `blocked`, `review`, `todo`,
  `superseded`.

### A dependency fact that will cost you a rewrite if you miss it

**Integration tests in `tests/` can only use the crate's public API plus
`[dev-dependencies]` — NOT the crate's regular `[dependencies]`.** `regex` is a
regular dependency, so `use regex::Regex;` in `tests/` **will not compile**.
Confirm by looking at `tests/integration.rs`: its only imports are `use
daemoneye…` and `use std…`.

Do all parsing with plain `std` string operations (`strip_prefix`,
`strip_suffix`, `rsplit_once`, `split_whitespace`, `trim_matches`). **Do not add
a dependency** — that is an always-blocker per STANDARDS §2.6, and this phase
does not authorize one.

### Finding the repo from inside a test

`env!("CARGO_MANIFEST_DIR")` expands at compile time to the crate root
(`/home/matt/src/daemoneye`). It is not currently used anywhere in this repo, so
there is no in-tree example to copy — use it directly:

```rust
let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
let milestones = root.join("docs/dev/milestones");
```

This reads repo files read-only, which is hermetic per STANDARDS §3.3 (that rule
forbids network and writes to the host home — not reading the checked-in tree).
Do **not** use `std::env::current_dir()`; it varies with how the test is invoked.

### The seam idiom this repo uses

Separate the **pure decision** from the **side-effecting scan**, so the decision
is testable without touching the filesystem. `src/daemon/utils/warnings.rs:24`
is the established example:

```rust
/// Return warnings for artifact classes whose retention is `0` (keep forever).
///
/// Returns an empty vec when nothing is disabled. This function is pure and
/// testable — it reads only the config values and produces structured output.
pub fn retention_warnings(cfg: &Config) -> Vec<RetentionWarning> {
```

Do the same shape here: a pure `classify` over already-parsed records, and a
separate scanner that reads the tree and feeds it.

## Spec

### 1. Create `tests/bug_tracker.rs`

A new integration test file. It ships **no** runtime code — this is repo
hygiene, and putting it in `src/` would compile dev-only logic into the shipped
binary.

Define a record and a finding:

```rust
#[derive(Debug, Clone, PartialEq)]
struct BugRecord {
    /// Repo-relative path, e.g. "M2-tui-renderer/bugs/bug-phase-01-1.md".
    doc: String,
    /// First token of the bug doc's header Status line, lowercased.
    bug_status: String,
    /// Phase id parsed from the filename, e.g. "01", "02b".
    phase_id: String,
    /// First token of the phase doc's header Status line — None if no phase doc matched.
    phase_status: Option<String>,
}

#[derive(Debug, PartialEq)]
enum Finding {
    OpenBugOnDonePhase { doc: String, phase_id: String },
    UnknownBugStatus { doc: String, status: String },
    DanglingBug { doc: String, phase_id: String },
}
```

### 2. Write the pure classifier

`fn classify(records: &[BugRecord]) -> Vec<Finding>` — no filesystem access.
Rules, in this order per record:

- `phase_status` is `None` → `DanglingBug`.
- `bug_status` is not one of `open` / `closed` / `fixed` / `resolved` /
  `verified` → `UnknownBugStatus`.
- `bug_status == "open"` **and** `phase_status == Some("done")` →
  `OpenBugOnDonePhase`.
- Otherwise no finding.

**Pin the negative cases — these must NOT produce a finding:**

- `bug_status == "open"` with `phase_status == Some("in-progress")` — this is the
  normal, correct state of a live bounce.
- `bug_status == "open"` with `phase_status == Some("review")` or
  `Some("blocked")` or `Some("todo")` or `Some("superseded")`.
- `bug_status == "verified"` (or `closed` / `fixed` / `resolved`) with
  `phase_status == Some("done")` — the ordinary healthy case, 41 of the 46 docs.

### 3. Write the scanner

`fn scan(milestones_dir: &Path) -> Vec<BugRecord>`. For each milestone
directory, for each `*.md` under its `bugs/` subdirectory: parse the phase id
from the filename (fact 3), read the bug's first `**Status:**` line (facts 1–2),
locate the phase doc by `phase-<id>-` prefix (fact 4), and read its first
`**Status:**` line the same way.

Factor the line parsing into a small helper — `fn header_status(text: &str) ->
Option<String>` returning the first-token-lowercased-stripped value — so the
unit tests in task 4 can exercise it directly on string literals.

A milestone directory with no `bugs/` subdirectory contributes no records and is
not an error.

### 4. Tests

Write these, named as given. The first five are pure and take string/struct
literals — no filesystem:

- `header_status_reads_bare_word` — `"**Status:** open"` → `Some("open")`.
- `header_status_strips_trailing_prose` — asserts
  `"**Status:** closed 2026-07-30 — verified at review round 2"` → `Some("closed")`
  and `"**Status:** fixed (architect takeover, 2026-06-27)"` → `Some("fixed")`.
- `header_status_uses_first_occurrence_only` — a two-line input whose first
  `**Status:**` is `done` and whose second is
  `All 5 functions converted.` returns `Some("done")`.
- `open_bug_on_done_phase_is_a_finding` — one record, `open` + `done`, yields
  exactly one `OpenBugOnDonePhase`.
- `open_bug_on_in_progress_phase_is_clean` — same record with phase
  `in-progress` yields **no** findings. This is the regression guard against a
  gate that fires on every live bounce.

Then the real gate:

- `repository_bug_tracker_is_consistent` — runs `scan` over
  `env!("CARGO_MANIFEST_DIR")/docs/dev/milestones` and asserts `classify`
  returns an empty vec. On failure, the assertion message must list each
  finding's doc path, so a future failure is actionable without a debugger.
  **This test fails until task 5 is done** — that is the point of the phase.

### 5. Close the five stale bug docs

For each of the five files listed under "Current state", change **only** the
header `**Status:**` line from `open` to:

```
**Status:** closed 2026-07-31 — verified fixed against the code during M7 scoping; see phase-02 for the gate that now prevents this drift.
```

Use `closed`, the most common terminal word in the tree. **Do not** edit
anything else in those five files — not the severity, not the body, not the
"What's wrong" section. The bodies describe defects that were real when filed;
they stay as the historical record.

Each of the five was independently verified fixed before this phase was written:

| Doc | Original claim | Verified reality |
|---|---|---|
| M2 `bug-phase-01-1` | banned `unsafe` / `#[allow]` in the ratatui wiring | `src/cli/render_ratatui.rs` contains zero of either |
| M2 `bug-phase-01-2` | ratatui path never enters raw mode | `enable_raw_mode()` at `src/cli/render_ratatui.rs:170` |
| M2 `bug-phase-02b-1` | approval line-editing inert | uses `InputLine` + `draw_prompt` (`src/cli/commands/stream.rs:930`) |
| M2 `bug-phase-02b-2` | credential prompt returns masking bullets | `cred_real` holds characters, `cred_display` holds bullets (`src/cli/commands/stream.rs:929`) |
| M4 `bug-09-1` | boundary-reload fixtures vacuous | fixtures build real `ToolResult` values (`src/daemon/session.rs:1088`) |

## Acceptance criteria

- [ ] `cargo test --test bug_tracker` passes, including
      `repository_bug_tracker_is_consistent`.
- [ ] `grep -c '^\*\*Status:\*\* open' ` over the five listed files returns 0 for
      each.
- [ ] No bug doc anywhere in the tree is `open` while its phase doc is `done`.
- [ ] `tests/bug_tracker.rs` contains no `use regex` and no dependency was added
      — `git diff --name-only` does not list `Cargo.toml` or `Cargo.lock`.
- [ ] The five bug docs differ from their previous versions by exactly one line
      each (the header Status line).
- [ ] `cargo build` zero new warnings; `cargo clippy --all-targets
      --all-features -- -D warnings` exits 0; `cargo fmt --all` leaves the tree
      unchanged.
- [ ] `cargo test` passes overall. Lib tests stay at **991** and isolation at
      **8**; the `bug_tracker` target is new, so a new `test result` line for it
      is expected and correct.

## Test plan

Covered by spec task 4. The load-bearing ones are
`open_bug_on_in_progress_phase_is_clean` (proves the gate does not fire on
healthy live bounces) and `header_status_uses_first_occurrence_only` (proves the
Update-Log-status trap is handled).

**A note on what would make these fake:** `repository_bug_tracker_is_consistent`
must assert on the *result of `classify`*, not merely that `scan` returned a
non-empty vec. A test that only checks "the scanner found some files" passes
even when the classifier is broken.

## End-to-end verification

The real artifact is the checked-in doc tree and the test that gates it. Run
this block verbatim and paste the resulting file's contents into your Update Log
entry:

```bash
cd /home/matt/src/daemoneye
{
  echo "=== the gate passes ==="
  cargo test --test bug_tracker 2>&1 | grep -E '^test |^test result'
  echo "exit=$?"

  echo "=== the five docs are no longer open ==="
  grep -H '^\*\*Status:\*\*' \
    docs/dev/milestones/M2-tui-renderer/bugs/bug-phase-01-1.md \
    docs/dev/milestones/M2-tui-renderer/bugs/bug-phase-01-2.md \
    docs/dev/milestones/M2-tui-renderer/bugs/bug-phase-02b-1.md \
    docs/dev/milestones/M2-tui-renderer/bugs/bug-phase-02b-2.md \
    docs/dev/milestones/M4-context-management/bugs/bug-09-1.md
  echo "exit=$?"

  echo "=== NO bug doc anywhere is still open on a done phase (independent shell check) ==="
  find docs/dev/milestones -path '*/bugs/*.md' | sort | while read b; do
    mil=$(dirname $(dirname "$b")); base=$(basename "$b" .md)
    id=$(echo "$base" | sed -E 's/^bug-(phase-)?([0-9]+[a-z]*)-[0-9]+$/\2/')
    pd=$(ls "$mil"/phase-$id-*.md 2>/dev/null | head -1)
    bs=$(grep -m1 '^\*\*Status:\*\*' "$b" | sed 's/^\*\*Status:\*\*[[:space:]]*//' | awk '{print tolower($1)}' | tr -d '(,.—')
    ps=$(grep -m1 '^\*\*Status:\*\*' "$pd" | sed 's/^\*\*Status:\*\*[[:space:]]*//' | awk '{print tolower($1)}' | tr -d '(,.—')
    [ "$bs" = "open" ] && [ "$ps" = "done" ] && echo "VIOLATION: $b"
  done
  echo "grep-exit=$?   # empty list above == PASS"

  echo "=== each of the five changed by exactly one line ==="
  git diff --numstat -- docs/dev/milestones/M2-tui-renderer/bugs/ docs/dev/milestones/M4-context-management/bugs/
  echo "exit=$?"

  echo "=== no dependency was added ==="
  git diff --name-only | grep -E '^Cargo\.(toml|lock)$'
  echo "grep-exit=$?   # 1 == no Cargo files touched == PASS"

  echo "=== full gate ==="
  cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -3
  echo "clippy-exit=$?"
  cargo test 2>&1 | grep -E '^test result'
  echo "exit=$?"
} > /tmp/phase02-e2e.txt 2>&1
cat /tmp/phase02-e2e.txt
```

Two blocks above prove their case by being **empty** — the violation scan and
the Cargo-file check. For those the `grep-exit` marker is the whole proof; an
empty block with no marker demonstrates nothing.

Paste the captured file into an Update Log entry titled
`### Update — <date> (end-to-end verification)`. **The server-authored
`(complete)` entry does not satisfy this** — its "Command output tails" block is
the automatic gate capture every phase receives, and it shows that
build/lint/test ran, not that this phase's acceptance criteria were exercised.

## Authorizations

- [x] **May create `tests/bug_tracker.rs`** — a new file, which STANDARDS §2.2
      otherwise discourages. It is required: there is nowhere existing for a
      repo-hygiene gate to live.
- [x] **May edit the header `Status:` line of the five bug docs named in spec
      task 5** — STANDARDS §5 otherwise forbids touching phase/bug docs other
      than the one being executed. This authorization covers *only* that one
      line in *only* those five files.
- [ ] May add dependencies: **none**. See "A dependency fact" above.
- [ ] May touch `docs/architecture.md`: no.

## Out of scope

- **Normalising the four terminal synonyms** (`closed` / `fixed` / `resolved` /
  `verified`) to a single word. It would touch ~40 files for no behavioral gain,
  and the vocabulary belongs in `WORKFLOW.md`'s bug template — a contract doc
  that needs PE sign-off. The gate accepts all four.
- **Changing `WORKFLOW.md` or `STANDARDS.md`.** Always a blocker.
- **Editing any part of the five bug docs other than the header Status line.**
  Their bodies are the historical record of real defects.
- **Fixing the stray `**Status:** All 5 functions converted…` line at
  `docs/dev/milestones/M3-polish-maintenance/phase-05-consolidate-leaf-params.md:279`.**
  It sits inside an Update Log entry, the header status above it is correct, and
  the gate reads only the first occurrence. Leave it.
- **Auditing phase docs against milestone README rows.** A real drift class, but
  a different gate and not this phase.
- **Making the gate a CLI subcommand.** A test is the requested enforcement.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-31 16:51 (started)

**Executor:** rexyMCP executor

Created `tests/bug_tracker.rs` with pure classifier, scanner, and 6 tests (5 pure + 1 real-tree gate). Closed the five stale bug docs by changing their header `**Status:**` line from `open` to `closed`.

### Update — 2026-07-31 16:51 (end-to-end verification)

```
=== the gate passes ===
test header_status_reads_bare_word ... ok
test header_status_strips_trailing_prose ... ok
test header_status_uses_first_occurrence_only ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test repository_bug_tracker_is_consistent ... ok
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
exit=0
=== the five docs are no longer open ===
docs/dev/milestones/M2-tui-renderer/bugs/bug-phase-01-1.md:**Status:** closed 2026-07-31 — verified fixed against the code during M7 scoping; see phase-02 for the gate that now prevents this drift.
docs/dev/milestones/M2-tui-renderer/bugs/bug-phase-01-2.md:**Status:** closed 2026-07-31 — verified fixed against the code during M7 scoping; see phase-02 for the gate that now prevents this drift.
docs/dev/milestones/M2-tui-renderer/bugs/bug-phase-02b-1.md:**Status:** closed 2026-07-31 — verified fixed against the code during M7 scoping; see phase-02 for the gate that now prevents this drift.
docs/dev/milestones/M2-tui-renderer/bugs/bug-phase-02b-2.md:**Status:** closed 2026-07-31 — verified fixed against the code during M7 scoping; see phase-02 for the gate that now prevents this drift.
docs/dev/milestones/M4-context-management/bugs/bug-09-1.md:**Status:** closed 2026-07-31 — verified fixed against the code during M7 scoping; see phase-02 for the gate that now prevents this drift.
exit=0
=== NO bug doc anywhere is still open on a done phase (independent shell check) ===
grep-exit=1   # empty list above == PASS
=== each of the five changed by exactly one line ===
1	1	docs/dev/milestones/M2-tui-renderer/bugs/bug-phase-01-1.md
1	1	docs/dev/milestones/M2-tui-renderer/bugs/bug-phase-01-2.md
1	1	docs/dev/milestones/M2-tui-renderer/bugs/bug-phase-02b-1.md
1	1	docs/dev/milestones/M2-tui-renderer/bugs/bug-phase-02b-2.md
1	1	docs/dev/milestones/M4-context-management/bugs/bug-09-1.md
exit=0
=== no dependency was added ===
grep-exit=1   # 1 == no Cargo files touched == PASS
=== full gate ===
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s
clippy-exit=0
test result: ok. 991 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.30s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 30 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 8 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.14s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
exit=0
```

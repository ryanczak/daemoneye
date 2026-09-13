# Phase 02: `Category` and `mutates_state()` on every `ToolDef`

**Milestone:** M22 — Tool surface & context
**Status:** todo
**Depends on:** nothing in M22. Phase-01 is `done` but this phase does not
touch config or execution. It is the milestone's one **pure, additive** phase.
**Estimated diff:** ~130 lines incl. tests (39 one-line literal additions, a
small enum, two functions, five tests, one doc sentence)
**Tags:** language=rust, kind=feature, size=s

## Goal

`docs/design/daemoneye-2.0.md` § 2.4 requires **`Category` on every `ToolDef`
(`Read | Write | Search | Run | Meta`) and a derived `mutates_state()`** — and
§ 2.3's governor row states the dependency outright: *"Requires `Category` on
`TOOLS` first."* M25 cannot be scoped until this exists, which is why the PE
put it second in M22.

After this phase:

1. `Category` is a five-variant enum in `src/ai/tools/schema.rs`.
2. **Every one of the 39 `ToolDef` literals carries a `category`.** The field is
   not optional and has no default, so the compiler — not a reviewer — is what
   guarantees the next tool added gets one.
3. `Category::mutates_state()` answers "did this call change anything" from the
   table, so no second hand-kept list can drift from it.
4. `category_of(tool_name)` resolves a name to its category, which is the shape
   a governor holding a `ToolCallSnapshot` actually needs.
5. A test pins the invariant that is true today and must stay true: **every
   tool in `APPROVAL_GATED_TOOLS` mutates state.**

Nothing else consumes it yet. That is deliberate and PE-sanctioned: the design
names M25's governor as the consumer and names this as its prerequisite. Item 5
is what keeps the phase from being speculative — it turns the new data into a
live invariant over a list the project already maintains by hand.

**No tool is added, removed, renamed or re-grouped.** The tool count stays
**39 (27 core + 12 deferred)**, so `CLAUDE.md` § "Current AI tools" and the
README tables do not move and `tests/doc_truth.rs` must stay at **10 passed**.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.4 "Tool broker changes", first bullet —
  the requirement, quoted in the Goal.
- `docs/design/daemoneye-2.0.md` § 2.3, the **Governor** row — names this as a
  hard prerequisite and describes `NoProgressStall` as "N non-mutating calls",
  which is the consumer `mutates_state()` exists for.
- `CLAUDE.md` § "Adding a new AI tool (checklist)", step 4 — currently names
  `deferred_group` as the required field. Task 4 adds `category` beside it.

## Pre-flight

Read before writing code:

- `src/ai/tools/schema.rs` — `ParamDef`, `ParamTy`, `ToolDef`, the three
  provider renderers, `select_tools`, `deferred_catalog_text`, `tools_in_group`.
  All of it is under 232 lines; read the whole file.
- `src/ai/tools/defs.rs:28-39` — `APPROVAL_GATED_TOOLS` and its docstring.
- `src/ai/tools/mod.rs` — 14 lines; the re-export surface you extend.
- `src/ai/tools/dispatch.rs:104-130` — the existing "every tool in TOOLS must
  have X" test idiom. Your new tests join this module.

### Pre-injected: the source pattern, and the one place it must NOT be copied

The design cites rexyMCP as the origin. Here is the actual rexyMCP code
(`executor/src/tools/router.rs:1-31`), quoted so you do not have to imagine it:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    Read,
    Write,
    Search,
    Run,
    Meta,
}

/// Map a built-in tool name to its router category. `None` for an unknown name.
pub fn categorize(tool_name: &str) -> Option<Category> {
    Some(match tool_name {
        "read_file" | "symbols" => Category::Read,
        "write_file" | "patch" | "patch_lines" | "delete_file" | "move_file" => Category::Write,
        "search" | "find_files" => Category::Search,
        "bash" => Category::Run,
        "update_task" => Category::Meta,
        _ => return None,
    })
}

/// Does this built-in tool mutate files on disk? The single source of truth for
/// "the model made file progress" — the no-progress stall governor and the
/// escalation briefing both ask this instead of each keeping a private list that
/// can (and did) drift from the `Category::Write` set above.
pub fn mutates_files(tool_name: &str) -> bool {
    categorize(tool_name) == Some(Category::Write)
}
```

**Two deliberate divergences. Copying rexyMCP verbatim gets both wrong.**

1. **`mutates_state()` must include `Run`, not only `Write`.** rexyMCP's
   function is named `mutates_files` and answers a narrower question — its
   `Run` tool is `bash` running against a checked-out repo, where "progress"
   means a file changed. DaemonEye's `Run` tools execute arbitrary commands on
   a live host, restart services and spawn autonomous agents. A
   `run_terminal_command` call that restarts nginx has unambiguously mutated
   state. The name says `state`, not `files`, and the body must be
   `matches!(self, Category::Write | Category::Run)`.

2. **`category` is a struct field, not a name-keyed `match`.** rexyMCP's
   `categorize` is a free function over string literals, so a tool added to its
   registry without a category silently returns `None`. Putting the field on
   `ToolDef` makes the same omission a **compile error** — which is the whole
   reason the design says "`Category` on every `ToolDef`" rather than "a
   `categorize` function". `category_of` still exists, but it resolves *through*
   the table rather than duplicating it.

### Pre-injected gotcha — two unrelated things are called "category"

`src/ai/tools/schema.rs:57-63` already contains the string `"category"`:

```rust
pub(super) fn enum_values(param_name: &str) -> Option<&'static [&'static str]> {
    match param_name {
        "operation" => Some(&["edit", "create", "delete", "copy"]),
        "kind" => Some(&["runbooks", "scripts", "memory", "events", "all"]),
        "category" => Some(&["session", "knowledge", "incident"]),
        _ => None,
    }
}
```

That is the **memory** tools' `category` *parameter* (`session | knowledge |
incident`, backed by `MemoryCategory` in `src/memory.rs:9`). It has nothing to
do with this phase's `Category`. **Do not touch `enum_values`, do not add
`Category` to it, and do not rename `MemoryCategory`.** A criterion asserts
`enum_values` still returns the same three sets.

## Current state

Values below were run against the tree at drafting (2026-09-12, commit
`ad0c4375`). Re-derive with the commands shown if the tree has moved.

- `src/ai/tools/schema.rs` — `grep -c 'pub enum Category'` → **0**;
  `grep -c 'fn mutates_state'` → **0**; `grep -c 'pub fn category_of'` → **0**.
- `src/ai/tools/defs.rs` — `grep -c 'ToolDef {'` → **39**;
  `grep -c 'deferred_group:'` → **39**; `grep -c 'category: Category::'` → **0**.
- `src/ai/tools/mod.rs` — `grep -c 'Category'` → **0**.
- **Construction sites: all 39 are in `defs.rs`, and there are none anywhere
  else.** `grep -rn 'ToolDef {' --include=*.rs src/ tests/` returns 39 hits in
  `src/ai/tools/defs.rs` plus the struct *definition* at
  `src/ai/tools/schema.rs:40`. There are **no `ToolDef` literals in any test
  module, in `tests/`, or in any other file** — so unlike M20 08b/09, this
  field addition has no blast radius outside `defs.rs`. Do not go hunting for
  literals elsewhere; there are none.
- `CLAUDE.md` — `sed -n '/^### Adding a new AI tool/,/^### Current AI tools/p' CLAUDE.md | grep -ci 'categor'`
  → **0** (the 16-line checklist section says nothing about categories).
- Suite: lib **1815 passed**, `tests/doc_truth.rs` **10 passed**,
  `cargo test --lib 'ai::tools'` → **17 passed**.

## Spec

Five tasks. Task 5 is the mutation checks, as a numbered Spec task.

### Task 1 — the enum and the two functions

In `src/ai/tools/schema.rs`:

- Add `pub enum Category { Read, Write, Search, Run, Meta }` with
  `#[derive(Debug, Clone, Copy, PartialEq, Eq)]`. Place it next to `ParamTy`,
  which is the analogous small enum in the same file.
- Add `pub category: Category` to `ToolDef`. Put it after `deferred_group` so
  every literal reads the same way; the field is **required**, with no
  `Default` impl and no `Option`.
- Add `Category::mutates_state(self) -> bool`, returning true for `Write` and
  `Run` and false for `Read`, `Search` and `Meta`. Give it a doc comment saying
  *why* `Run` counts — a run tool executes arbitrary commands on a live host,
  so it changes state even though it writes no file the broker knows about.
  This is the single source of truth for "did anything change"; say so, and say
  that a second hand-kept list is what it exists to prevent.
- Add `pub fn category_of(tool_name: &str) -> Option<Category>` — scan `TOOLS`
  for a matching `name` and return its `category`; `None` for an unknown name.
  It must read the table, **not** re-list tool names in a `match`.

**One entry point per question.** Do not also add a `ToolDef::mutates_state()`
wrapper or a free `mutates_state(name)` function; callers write
`tool.category.mutates_state()` or
`category_of(name).is_some_and(Category::mutates_state)`. STANDARDS § 2.2
forbids the speculative second entry point.

### Task 2 — categorise all 39 tools

In `src/ai/tools/defs.rs`, give every `ToolDef` literal its `category`. **The
assignment is fixed and given in full below — do not re-derive it.** Counts:
Read **17**, Write **13**, Search **4**, Run **4**, Meta **1** (= 39).

| Tool | Category |
|---|---|
| `run_terminal_command` | `Run` |
| `schedule_command` | `Write` |
| `list_schedules` | `Read` |
| `cancel_schedule` | `Write` |
| `delete_schedule` | `Write` |
| `write_script` | `Write` |
| `list_scripts` | `Read` |
| `read_script` | `Read` |
| `delete_script` | `Write` |
| `watch_pane` | `Read` |
| `read_file` | `Read` |
| `edit_file` | `Write` |
| `write_runbook` | `Write` |
| `delete_runbook` | `Write` |
| `read_runbook` | `Read` |
| `list_runbooks` | `Read` |
| `run_log_tail` | `Read` |
| `run_log_search` | `Search` |
| `get_turn` | `Read` |
| `add_memory` | `Write` |
| `update_memory` | `Write` |
| `delete_memory` | `Write` |
| `read_memory` | `Read` |
| `list_memories` | `Read` |
| `search_repository` | `Search` |
| `recall_context` | `Search` |
| `get_terminal_context` | `Read` |
| `load_tools` | `Meta` |
| `list_panes` | `Read` |
| `read_pane` | `Read` |
| `find_in_panes` | `Search` |
| `tmux_control` | `Run` |
| `close_background_window` | `Run` |
| `spawn_ghost_shell` | `Run` |
| `create_agent` | `Write` |
| `read_agent` | `Read` |
| `list_agents` | `Read` |
| `delete_agent` | `Write` |
| `await_agent_result` | `Read` |

The four judgement calls, so you do not second-guess them:

- **`watch_pane` is `Read`.** It blocks until a pattern appears; observing is
  not mutating.
- **`tmux_control` and `close_background_window` are `Run`, not `Write`.** They
  take side-effecting actions on the live terminal (focus, zoom, split, kill a
  window) rather than writing a managed artifact. `Write` in this table means
  "changed a file or record the daemon owns".
- **`spawn_ghost_shell` is `Run`.** It launches an autonomous agent that will
  itself run commands; nothing about that is read-only.
- **`recall_context` and `run_log_search` are `Search`, not `Read`.** Both
  answer a query over a corpus rather than fetching a named object.
  `run_log_tail` and `get_turn` fetch named things and stay `Read`.

### Task 3 — export the new surface

In `src/ai/tools/mod.rs`, add `Category` and `category_of` to the existing
`pub use schema::{…}` list. Keep the list alphabetically ordered as it is now.

### Task 4 — the checklist sentence

In `CLAUDE.md` § "Adding a new AI tool (checklist)", step 4: after the existing
`deferred_group` sentences, state that **`category` is also required** — one of
`Read | Write | Search | Run | Meta` — that it is what `mutates_state()` reads,
and that the compiler enforces it because the field has no default. Two
sentences at most.

Do **not** touch `CLAUDE.md` § "Current AI tools", the `N tools: C core + D
deferred` line, or `README.md`. No tool is added or removed, so those numbers do
not move and `tests/doc_truth.rs` must stay green at 10.

### Task 5 — run the mutation checks and record them

Both mutations below, applied with the `patch` tool (`sed -i`, `perl -i` and
`>` redirects are forbidden by the executor contract). Capture each
red-then-green transcript mechanically and paste it into the Update Log. Each
names the **property** to break; find the line in your own implementation.
**If a mutation does not turn its test red, record exactly that — do not invent
a failure, and do not report an outcome the applied patch cannot produce.**

1. **A `Run` tool must count as mutating.** Make `mutates_state()` true for
   `Write` only → `approval_gated_tools_all_mutate_state` must go **red**,
   because `run_terminal_command` and `tmux_control` are both gated and both
   `Run`. This is the divergence from rexyMCP in the Pre-flight; the test exists
   to catch exactly this regression.
2. **Every tool must carry the category the table assigns.** Change one
   `Read` tool's category to `Write` → `category_counts_match_the_assignment`
   must go **red** on both the `Read` and `Write` counts.

After each, revert with `patch` and show the test green again.

When you quote a `test result:` line, **quote it from the run whose command you
are showing.** Counts differ by filter — `cargo test --lib ai::tools` selects a
different set than a run naming one test — so a number pasted from a different
invocation than the one above it is a defect even when both numbers are real.

## Acceptance criteria

Each was run against the tree at drafting and returns the "now" value shown.
Counts marked *prod* use `sed -n '1,/^#\[cfg(test)\]/p' <file> |`.

- [ ] **The enum exists with five variants:**
      `grep -c 'pub enum Category' src/ai/tools/schema.rs` → **1** (now `0`),
      and `sed -n '/pub enum Category/,/^}/p' src/ai/tools/schema.rs | grep -cE '^\s+(Read|Write|Search|Run|Meta),'`
      → **5** (now `0`).
- [ ] **Every tool carries a category:**
      `grep -c 'category: Category::' src/ai/tools/defs.rs` → **39** (now `0`),
      and `grep -c 'ToolDef {' src/ai/tools/defs.rs` → **39**, unchanged.
- [ ] **The assignment matches the table, per category:** in
      `src/ai/tools/defs.rs`, `grep -c 'category: Category::Read,'` → **17**,
      `Write` → **13**, `Search` → **4**, `Run` → **4**, `Meta` → **1** (all now
      `0`). *(These five are also asserted by
      `category_counts_match_the_assignment`, which is the instrument that
      survives a reformat; the greps are the cheap cross-check.)*
- [ ] **`mutates_state` includes `Run`:** prod
      `sed -n '/fn mutates_state/,/^    }/p' src/ai/tools/schema.rs | grep -c 'Category::Run'`
      → **1 or more** (now `0`).
- [ ] **`category_of` reads the table rather than re-listing names:**
      `grep -c 'pub fn category_of' src/ai/tools/schema.rs` → **1** (now `0`),
      and `sed -n '/pub fn category_of/,/^}/p' src/ai/tools/schema.rs | grep -c 'TOOLS'`
      → **1 or more** (now `0`).
- [ ] **No second entry point:**
      `grep -cE 'fn mutates_state' src/ai/tools/schema.rs` → **1** (now `0`) —
      exactly one definition, on `Category`.
- [ ] **The memory `category` param is untouched:**
      `sed -n '/pub(super) fn enum_values/,/^}/p' src/ai/tools/schema.rs | grep -c '"session", "knowledge", "incident"'`
      → **1**, unchanged, and the same range `grep -c 'Category'` → **0**.
- [ ] **Exported:** `grep -c 'Category' src/ai/tools/mod.rs` → **1 or more**
      (now `0`), and `grep -c 'category_of' src/ai/tools/mod.rs` → **1** (now
      `0`).
- [ ] **The five tests exist by name** in `src/ai/tools/dispatch.rs`:
      `grep -cE 'fn (every_tool_has_a_category|category_counts_match_the_assignment|approval_gated_tools_all_mutate_state|mutates_state_is_write_and_run_only|category_of_resolves_every_tool_name_and_nothing_else)' src/ai/tools/dispatch.rs`
      → **5** (now `0`).
- [ ] **The checklist documents the field:**
      `sed -n '/^### Adding a new AI tool/,/^### Current AI tools/p' CLAUDE.md | grep -ci 'categor'`
      → **1 or more** (now `0`).
- [ ] **The tool tables did not move:**
      `git diff --stat -- README.md | wc -l` → **0**, and
      `sed -n '/^### Current AI tools/,/^## /p' CLAUDE.md | grep -c '39 tools: 27 core + 12 deferred'`
      → **1**, unchanged.
- [ ] No new `unsafe` / `unwrap` / `expect` / `panic!` in production code: prod
      on `src/ai/tools/schema.rs` and `src/ai/tools/defs.rs` → **0** each,
      unchanged.
- [ ] `cargo test --lib 'ai::tools'` → **22 or more** passing, `0 failed` (now
      **17**). *Arithmetic: five added, none removed.*
- [ ] `cargo test --test doc_truth` → **10**, `0 failed`, unchanged.
- [ ] **Preservation, each must not move:** `cargo test --lib 'daemon::executor::'`
      **235**, `cargo test --lib 'daemon::stream'` **8**, `cargo test --lib 'config::'`
      **92**, `cargo test --test isolation` **9**.
- [ ] Full lib suite **1820 or more** passing, `0 failed`, on **three
      consecutive runs** (now **1815**).
- [ ] All four gates pass.

## Test plan

Five tests, all in the existing `mod tests` in `src/ai/tools/dispatch.rs` —
that module already holds the "every tool in `TOOLS` must have X" family and is
where a reader will look for the next one.

1. **`every_tool_has_a_category`** — `category_of(t.name)` is `Some` for every
   `t` in `TOOLS`, and `TOOLS.len() == 39`. Guards the resolver against a tool
   whose name is shadowed or duplicated.
2. **`category_counts_match_the_assignment`** — tally `TOOLS` by category and
   assert Read 17 / Write 13 / Search 4 / Run 4 / Meta 1. This is the test that
   pins Task 2's table; a miscategorised tool fails it.
3. **`approval_gated_tools_all_mutate_state`** — for every name in
   `APPROVAL_GATED_TOOLS`, `category_of(name)` is `Some(c)` and
   `c.mutates_state()` is true. **The load-bearing invariant**: a read-only tool
   in that list would be exempted from every budget cap in `daemon::stream`
   while prompting for nothing. Assert with a message naming the offending tool.
4. **`mutates_state_is_write_and_run_only`** — `Write` and `Run` are true;
   `Read`, `Search` and `Meta` are false. The direct unit test of the divergence
   from rexyMCP, and what mutation 1 breaks.
5. **`category_of_resolves_every_tool_name_and_nothing_else`** —
   `category_of("run_terminal_command") == Some(Category::Run)` and
   `category_of("no_such_tool") == None`. Pins the negative; an `unwrap_or`
   default would pass test 1 and fail this one.

**Negative cases are named on purpose** (unknown name → `None`; `Read`/`Search`/
`Meta` → not mutating). A suite that only asserts the positives passes against
a `mutates_state()` that returns `true` unconditionally.

## End-to-end verification

There is no runtime surface to exercise — the artifact is the tool table itself
and the invariant over it. Section B prints the categorisation *through the
public API* rather than by grepping the literal, which is what makes it evidence
rather than a restatement of the diff.

Run from the repo root, verbatim:

```sh
A=/tmp/e2e-m22-02.txt
: > "$A"
RUNS_BEFORE=$(ls ~/.daemoneye/var/log/runs/ 2>/dev/null | wc -l)
{
echo "== A. build =="
echo "real HOME run logs BEFORE: $RUNS_BEFORE"
cargo build 2>&1 | tail -2; echo "cargo_exit=${PIPESTATUS[0]}"

echo "== B. the table through the public API =="
cargo test --lib 'ai::tools' -- --nocapture 2>&1 \
  | sed 's/\x1b\[[0-9;]*m//g' | grep -E '^test ai::tools|^test result'
echo "cargo_exit=${PIPESTATUS[0]}"

echo "== C. structural greps (each must print the stated number) =="
S=src/ai/tools/schema.rs; D=src/ai/tools/defs.rs
printf 'enum Category                (1): %s\n' "$(grep -c 'pub enum Category' $S)"
printf 'five variants                (5): %s\n' "$(sed -n '/pub enum Category/,/^}/p' $S | grep -cE '^\s+(Read|Write|Search|Run|Meta),')"
printf 'ToolDef literals            (39): %s\n' "$(grep -c 'ToolDef {' $D)"
printf 'categories assigned         (39): %s\n' "$(grep -c 'category: Category::' $D)"
printf '  Read                      (17): %s\n' "$(grep -c 'category: Category::Read,' $D)"
printf '  Write                     (13): %s\n' "$(grep -c 'category: Category::Write,' $D)"
printf '  Search                     (4): %s\n' "$(grep -c 'category: Category::Search,' $D)"
printf '  Run                        (4): %s\n' "$(grep -c 'category: Category::Run,' $D)"
printf '  Meta                       (1): %s\n' "$(grep -c 'category: Category::Meta,' $D)"
printf 'mutates_state defined once   (1): %s\n' "$(grep -cE 'fn mutates_state' $S)"
printf 'mutates_state covers Run     (1+): %s\n' "$(sed -n '/fn mutates_state/,/^    }/p' $S | grep -c 'Category::Run')"
printf 'category_of defined          (1): %s\n' "$(grep -c 'pub fn category_of' $S)"
printf 'category_of reads TOOLS      (1+): %s\n' "$(sed -n '/pub fn category_of/,/^}/p' $S | grep -c 'TOOLS')"
printf 'memory param sets untouched  (1): %s\n' "$(sed -n '/pub(super) fn enum_values/,/^}/p' $S | grep -c '"session", "knowledge", "incident"')"
printf 'no Category in enum_values   (0): %s\n' "$(sed -n '/pub(super) fn enum_values/,/^}/p' $S | grep -c 'Category')"
printf 'exported from mod.rs        (1+): %s\n' "$(grep -c 'Category' src/ai/tools/mod.rs)"
printf 'category_of exported         (1): %s\n' "$(grep -c 'category_of' src/ai/tools/mod.rs)"
printf 'five new tests               (5): %s\n' "$(grep -cE 'fn (every_tool_has_a_category|category_counts_match_the_assignment|approval_gated_tools_all_mutate_state|mutates_state_is_write_and_run_only|category_of_resolves_every_tool_name_and_nothing_else)' src/ai/tools/dispatch.rs)"
printf 'checklist mentions category (1+): %s\n' "$(sed -n '/^### Adding a new AI tool/,/^### Current AI tools/p' CLAUDE.md | grep -ci 'categor')"
printf 'tool-count line unchanged    (1): %s\n' "$(sed -n '/^### Current AI tools/,/^## /p' CLAUDE.md | grep -c '39 tools: 27 core + 12 deferred')"
printf 'README untouched             (0): %s\n' "$(git diff --stat -- README.md | wc -l)"
P() { sed -n '1,/^#\[cfg(test)\]/p' "$1" | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe '; }
printf 'schema.rs unwrap/panic       (0): %s\n' "$(P $S)"
printf 'defs.rs unwrap/panic         (0): %s\n' "$(P $D)"

echo "== D. preservation: executor 235, config 92, isolation 9, doc_truth 10 =="
for m in 'daemon::executor::' 'config::' 'daemon::stream'; do
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

Then the paste self-check:

```sh
D=docs/dev/milestones/M22-tool-surface/phase-02-tool-category.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m22-02.txt
diff /tmp/pasted-m22-02.txt /tmp/e2e-m22-02.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

Paste the captured block into the Update Log **unmodified** — do not retype it,
do not trim it, do not fix its alignment. The self-check above must print
`PASTE MATCH`; if it prints `PASTE MISMATCH`, re-paste rather than editing the
transcript to agree.

## Authorizations

The executor may modify **only**:

- `src/ai/tools/schema.rs` — task 1: the new enum, the `ToolDef` field, the two
  functions. **Do not touch `enum_values`, the three renderers, `select_tools`,
  `deferred_catalog_text` or `tools_in_group`** — criteria assert `enum_values`
  is unchanged, and the renderers have no reason to see a category.
- `src/ai/tools/defs.rs` — task 2, the 39 `category:` lines only. Do not change
  any tool's `name`, `description`, `params` or `deferred_group`, and do not
  reorder the table.
- `src/ai/tools/mod.rs` — task 3, the `pub use` list only.
- `src/ai/tools/dispatch.rs` — the `#[cfg(test)]` module only; five new tests.
  Do not change `dispatch_tool_event` or any existing test.
- `CLAUDE.md` — task 4, **step 4 of the "Adding a new AI tool" checklist only**.
  Nothing else in the file, and specifically not § "Current AI tools".
- `docs/dev/milestones/M22-tool-surface/phase-02-tool-category.md` — **append to
  the Update Log only.** Do not rewrite, reorder or delete any section above it;
  the reviewer checks that every `## ` heading in this file survives.

No other file. In particular do not touch `README.md`, `tests/doc_truth.rs`,
`src/daemon/stream.rs`, `src/config/types.rs`, `src/memory.rs`, or
`assets/prompts/sre.toml`.

## Out of scope

- **Wiring `mutates_state()` into the governor.** That is M25; the design names
  this phase as its prerequisite and no more. Do not add a stall detector, a
  counter, or a `ToolCallSnapshot` type.
- **Using `Category` to drive tool selection or rendering.** rexyMCP's
  `Category` feeds a two-stage router that shows a weak model a slice of
  schemas; DaemonEye already solves that with `deferred_group` + `load_tools`.
  The two mechanisms stay independent in this phase.
- **Changing `APPROVAL_GATED_TOOLS`.** The new test asserts a property of the
  list as it stands; it must not edit the list to make itself pass. All ten
  entries already mutate state — verified at drafting.
- **Any tool added, removed, renamed or re-grouped.** The count stays 39
  (27 core + 12 deferred). Phases 03–09 change the surface.
- **`ToolResult`-as-value and `missing_args_hint`**, the other two § 2.4 broker
  changes. Not scoped to M22's breakdown; they arrive with M25's error handling.

## Update Log

<!-- entries appended below this line -->

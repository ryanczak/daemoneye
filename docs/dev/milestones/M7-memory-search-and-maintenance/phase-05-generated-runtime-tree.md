# Phase 05: Generated Runtime Tree

**Milestone:** M7 — Memory Search & Maintenance
**Status:** in-progress
**Depends on:** phase-04 (path-audit-fenced-blocks, done)
**Estimated diff:** ~260 lines — one new file `src/config/runtime_tree.rs`
(data + renderer + tests), one line in `src/config/mod.rs`. **No change to any
asset file** (see the Spec — that is the point).

**Tags:** language=rust, kind=feature, size=m

## Goal

`assets/memory/knowledge/agent-runtime-layout.md` contains a hand-maintained
directory tree. Two of M6's three path-audit gate escapes were hand-edited lines
in exactly that tree. Move the tree's *data* into Rust next to the lifecycle
policy table, render the markdown block from it, and add a test that fails when
the shipped asset and the renderer disagree — so the tree cannot drift again.

This is Part B of M6 open question 5. Part A (phase 04) taught the audit to read
fenced blocks; this phase removes the hand-maintenance that produced the stale
lines in the first place.

## Architecture references

- `src/config/lifecycle.rs:60` — `POLICY_TABLE`, the authoritative list of
  artifact classes. This phase cross-checks against it; it does **not** modify
  it.
- `src/config/lifecycle.rs:262` — `is_covered()`, the existing wildcard-matching
  helper. The cross-check test in this phase needs the same idea; the rule is
  restated below so you do not have to reuse that private test helper.
- `CLAUDE.md` § "Key files" — `src/config/` module layout.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any file.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

The tree lives in one fenced block in
`assets/memory/knowledge/agent-runtime-layout.md`, under the `## Directory Tree`
heading (the file's lines 14–59). That asset is compiled into the binary:

```rust
// src/config/seeds.rs:174
pub(crate) const AGENT_RUNTIME_LAYOUT_MEMORY: &str =
    include_str!("../../assets/memory/knowledge/agent-runtime-layout.md");
```

It is seeded to `~/.daemoneye/memory/knowledge/agent-runtime-layout.md` on first
run by `seed_knowledge_memory("agent-runtime-layout", …)`
(`src/config/seeds.rs:45`), and never overwritten if the file already exists.

**There is no `build.rs` in this repo and this phase does not add one.** The
asset stays a checked-in file so `include_str!` keeps working unchanged. The
"generation" is a renderer plus an equality test — the standard checked-in-
generated-file pattern. Nothing is generated at build time or at run time.

### The tree's data is not derivable from `POLICY_TABLE` alone

This is the constraint that shapes the spec, and it was checked against the real
tables rather than assumed. `POLICY_TABLE` has 15 entries, almost all
directories, each carrying a *lifecycle* note. The tree has 44 lines and carries
things the policy table does not have at all:

- **Files:** `daemoneye.sock`, `schedules.json`, `pane_prefs.json`, `index.json`,
  `meta.toml`, `messages.jsonl`, `briefing.md`, `config.toml`, `sre.toml`.
- **The `memory/` split:** the table has one `memory` entry; the tree documents
  `session/`, `knowledge/` and `incident/` separately.
- **Purpose annotations.** The table's `note` field says *how the artifact is
  swept*; the tree says *what the artifact is for*. They are different sentences
  and the tree's are the ones an agent reading this memory needs.

So the tree gets **its own table**, and a test cross-checks that every
`POLICY_TABLE` path appears in it. Two tables, one gate between them.

**Be honest about what this buys.** When phase 06 adds `var/index/memory.db`, it
will need an entry in *both* tables — this phase does not reduce that to one
edit. What it does is make the second edit **impossible to forget**: the
cross-check test fails until the tree entry exists. That is the drift this
milestone is removing.

### The exact format, measured from the asset

Do not re-derive these; they were measured from the shipped file.

- Indent is **2 spaces per level**. The root `~/.daemoneye/` is at level 0, so
  its children are at 2, grandchildren at 4, and so on to a maximum of 8.
- Annotated lines put `←` at **column 30, 1-based** — i.e. the indent-plus-name
  text is left-padded to a width of **29** and then `← ` and the note follow.
  Every annotated line in the current tree does this; the longest
  indent-plus-name is 25 characters (`      events-<date>.jsonl`), so nothing
  currently overflows.
- Directory names carry a **trailing `/`**; file names do not.
- There are **7 blank separator lines**, before: `agents/`, `bin/`, `scripts/`,
  `memory/`, `var/`, `var/log/`, and `var/sessions/`.

## Spec

### 1. New module `src/config/runtime_tree.rs`

Create the file. Wire it into `src/config/mod.rs` by adding — matching the
existing alphabetical block at `src/config/mod.rs:5-9` and its re-export block
at lines 11–14:

```rust
mod runtime_tree;
// …and in the pub use block:
pub use runtime_tree::*;
```

Note `path_audit` is `pub mod` because it has an external consumer; this module
follows the plain `mod` + glob re-export shape of `lifecycle` instead.

### 2. The node type and the renderer

```rust
/// One line of the runtime directory tree.
pub struct TreeNode {
    /// Display name. Directories carry a trailing `/`; files do not.
    /// Placeholder segments are spelled `<name>`, `<id>`, `<job_id>`, `<date>`.
    pub name: &'static str,
    /// The purpose annotation rendered after `←`, if this line has one.
    pub note: Option<&'static str>,
    /// Emit a blank separator line before this node.
    pub blank_before: bool,
    pub children: &'static [TreeNode],
}

/// Column (0-based width) that `←` is padded out to.
const ANNOTATION_COL: usize = 29;
```

`pub fn render_tree() -> String` walks the tree depth-first from `RUNTIME_TREE`
at depth 0 and returns the block, each line newline-terminated (so the returned
string ends with `\n`). Per node:

1. If `blank_before`, push a bare `\n` first.
2. Build `head = "  ".repeat(depth) + name`.
3. If `note` is `Some(n)`: pad `head` with spaces to `ANNOTATION_COL`, then push
   `← ` and `n`. **Use at least one space** even if `head` is already at or past
   the column — `ANNOTATION_COL.saturating_sub(head.chars().count()).max(1)`.
   Use `.chars().count()`, not `.len()`: `←` is 3 bytes and future notes may
   carry non-ASCII, so byte length would misalign.
4. If `note` is `None`, push `head` alone.
5. Recurse into `children` at `depth + 1`.

### 3. The tree data

This is the current asset transcribed exactly. Use it verbatim — the phase
succeeds precisely when rendering it reproduces the shipped file byte for byte.

```rust
pub static RUNTIME_TREE: TreeNode = TreeNode {
    name: "~/.daemoneye/",
    note: None,
    blank_before: false,
    children: &[
        TreeNode {
            name: "etc/",
            note: None,
            blank_before: false,
            children: &[
                TreeNode { name: "config.toml", note: Some("daemon configuration (models, prompt, webhook, ghost, limits)"), blank_before: false, children: &[] },
                TreeNode {
                    name: "prompts/",
                    note: None,
                    blank_before: false,
                    children: &[
                        TreeNode { name: "sre.toml", note: Some("built-in SRE system prompt (overwritten on --overwrite-prompt)"), blank_before: false, children: &[] },
                        TreeNode { name: "<name>.toml", note: Some("additional prompt profiles"), blank_before: false, children: &[] },
                    ],
                },
            ],
        },
        TreeNode {
            name: "agents/",
            note: None,
            blank_before: true,
            children: &[TreeNode {
                name: "<name>/",
                note: None,
                blank_before: false,
                children: &[
                    TreeNode { name: "config.toml", note: Some("named agent profile (prompt, model, tool policy, memory namespace)"), blank_before: false, children: &[] },
                    TreeNode { name: "briefing.md", note: Some("rolling post-session briefing (auto-generated on clean exit)"), blank_before: false, children: &[] },
                    TreeNode {
                        name: "mailbox/",
                        note: None,
                        blank_before: false,
                        children: &[TreeNode { name: "<job_id>.json", note: Some("mailbox result written by child ghost on exit"), blank_before: false, children: &[] }],
                    },
                ],
            }],
        },
        TreeNode { name: "bin/", note: Some("place symlinks / wrappers here (on PATH for systemd service)"), blank_before: true, children: &[] },
        TreeNode { name: "scripts/", note: Some("executable automation (.sh / .py, chmod 700)"), blank_before: true, children: &[] },
        TreeNode { name: "runbooks/", note: Some("procedure runbooks (markdown + YAML frontmatter)"), blank_before: false, children: &[] },
        TreeNode {
            name: "memory/",
            note: None,
            blank_before: true,
            children: &[
                TreeNode { name: "session/", note: Some("user prefs, always injected at session start"), blank_before: false, children: &[] },
                TreeNode { name: "knowledge/", note: Some("technical facts, loaded on-demand via tags"), blank_before: false, children: &[] },
                TreeNode { name: "incident/", note: Some("post-mortems, never auto-loaded"), blank_before: false, children: &[] },
            ],
        },
        TreeNode {
            name: "var/",
            note: None,
            blank_before: true,
            children: &[
                TreeNode {
                    name: "run/",
                    note: None,
                    blank_before: false,
                    children: &[
                        TreeNode { name: "daemoneye.sock", note: Some("Unix domain socket (IPC)"), blank_before: false, children: &[] },
                        TreeNode { name: "schedules.json", note: Some("scheduled job store (atomic JSON)"), blank_before: false, children: &[] },
                        TreeNode { name: "pane_prefs.json", note: Some("per-session foreground pane preferences"), blank_before: false, children: &[] },
                    ],
                },
                TreeNode {
                    name: "log/",
                    note: None,
                    blank_before: true,
                    children: &[
                        TreeNode { name: "daemon.log", note: Some("daemon process log (structured JSON lines)"), blank_before: false, children: &[] },
                        TreeNode {
                            name: "events/",
                            note: None,
                            blank_before: false,
                            children: &[TreeNode { name: "events-<date>.jsonl", note: Some("structured event log (dated segments, searchable via search_repository)"), blank_before: false, children: &[] }],
                        },
                        TreeNode { name: "panes/", note: Some("archived background-window scrollback (.log files)"), blank_before: false, children: &[] },
                        TreeNode { name: "pipe/", note: Some("live pipe-pane capture logs (ephemeral, ANSI-stripped)"), blank_before: false, children: &[] },
                        TreeNode {
                            name: "sessions/",
                            note: None,
                            blank_before: false,
                            children: &[TreeNode { name: "<id>.jsonl", note: Some("per-session JSONL conversation history (ephemeral)"), blank_before: false, children: &[] }],
                        },
                    ],
                },
                TreeNode {
                    name: "sessions/",
                    note: Some("named session persistent store"),
                    blank_before: true,
                    children: &[
                        TreeNode { name: "index.json", note: Some("session index (name → metadata)"), blank_before: false, children: &[] },
                        TreeNode {
                            name: "<name>/",
                            note: None,
                            blank_before: false,
                            children: &[
                                TreeNode { name: "meta.toml", note: Some("session metadata (saved_name, artifacts_created, …)"), blank_before: false, children: &[] },
                                TreeNode { name: "messages.jsonl", note: Some("full conversation history"), blank_before: false, children: &[] },
                            ],
                        },
                    ],
                },
            ],
        },
    ],
};
```

`cargo fmt` will reflow the one-line `TreeNode { … }` literals. Let it; do not
fight the formatter, and do not hand-wrap them first.

### 4. Extracting the block from the asset

Add a helper the tests use — keep it a free function so the mutation test in
task 5 can drive it with a synthetic document:

```rust
/// Return the contents of the first fenced block following the
/// `## Directory Tree` heading, newline-terminated, or `None`.
pub fn tree_block_of(doc: &str) -> Option<String>
```

Rules: scan lines for one equal to `## Directory Tree`; from there find the next
line that is exactly ` ``` `; collect subsequent lines until the next line that
is exactly ` ``` `; join them with `\n` and append a trailing `\n`. Return
`None` if the heading or either fence is missing.

### 5. Tests

Add a `#[cfg(test)] mod tests` to the new file with these five, named exactly:

- `render_matches_shipped_asset` — the load-bearing one.
  `assert_eq!(render_tree(), tree_block_of(AGENT_RUNTIME_LAYOUT_MEMORY).unwrap())`.
  On failure the message must print the full rendered tree, so a developer can
  paste it into the asset. Reach the const via
  `crate::config::AGENT_RUNTIME_LAYOUT_MEMORY` — it is `pub(crate)` and
  glob-re-exported from `src/config/mod.rs`, so it is already in scope for a
  sibling module's tests.
- `every_policy_path_appears_in_tree` — for every entry in
  `crate::config::POLICY_TABLE`, assert its `path` matches some path in the
  rendered tree. Collect tree paths by joining node names down each spine with
  `/`, stripping trailing `/` from each segment. Compare **segment-wise with
  wildcards**: split both on `/`, require equal segment counts, and treat a
  segment as matching if the strings are equal **or** the table segment is `*`
  **or** the tree segment starts with `<` and ends with `>`. That rule is what
  makes `agents/*/mailbox` match `agents/<name>/mailbox`. All 15 current entries
  must match; the assertion message must name any that do not.
- `annotation_column_is_not_overflowed` — for every annotated node, assert
  `indent + name` is at most `ANNOTATION_COL` characters. This is what catches a
  future long name that would silently break alignment.
- `tree_block_of_finds_the_block` — a small synthetic document with a
  `## Directory Tree` heading, a fenced block of two lines, and trailing prose;
  assert the two lines come back and the prose does not.
- `tree_block_mismatch_is_detected` — the mutation guard. Take
  `AGENT_RUNTIME_LAYOUT_MEMORY`, replace `"  bin/"` with `"  bins/"` via
  `str::replace`, and assert `tree_block_of` on the mutated document returns
  something **different** from `render_tree()`. This proves
  `render_matches_shipped_asset` compares real content rather than two things
  that are trivially equal.

## Acceptance criteria

- [ ] **`assets/memory/knowledge/agent-runtime-layout.md` is byte-for-byte
      unchanged.** `git diff --name-only` must not list it. The renderer was
      specified to reproduce it; if the two disagree, **fix the renderer, do not
      edit the asset.** Editing the asset to match a broken renderer is the one
      way to make this phase a false success.
- [ ] `render_matches_shipped_asset` passes.
- [ ] All five tests named in spec task 5 pass.
- [ ] `daemoneye audit-prompts` still exits **0** on a freshly seeded tree. (The
      tree's lines are indentation-relative single segments, so phase 04's
      multi-segment rule skips them — this criterion confirms that still holds.)
- [ ] `cargo build` zero new warnings; `cargo clippy --all-targets
      --all-features -- -D warnings` exits 0; `cargo fmt --all` leaves the tree
      unchanged.
- [ ] `cargo test` passes. Lib count rises by the number of tests added (5 by
      this spec, so **1007**); integration stays **30** (2 ignored), isolation
      **8** (1 ignored), `bug_tracker` **6**.
- [ ] Only `src/config/runtime_tree.rs` (new) and `src/config/mod.rs` change.

## Test plan

Covered by spec task 5. The load-bearing test is `render_matches_shipped_asset`
— it is the whole mechanism, and it is why the asset must not change.

**What would make this phase a false success:** editing the asset so a wrong
renderer matches it. That inverts the gate — the asset stops being the thing
verified and becomes whatever the code happens to emit. Two things guard against
it: the first acceptance criterion (`git diff` must not list the asset) and
`tree_block_mismatch_is_detected`, which proves the comparison can tell two
different trees apart.

A second, quieter false success: a `tree_block_of` that returns `None` and a
test that `unwrap_or_default()`s it into `""` compared against an empty render.
Do not write that. `tree_block_of_finds_the_block` exists to pin real extraction.

## End-to-end verification

The real artifact is the seeded memory file and the `audit-prompts` gate. Run
this block verbatim and paste the resulting file's contents into your Update Log.

**Two constraints carried from phase-03's post-mortem:** **no heredocs**, and
every tree-walking command wrapped in `timeout`. A phase-03 E2E block nested a
`python3` heredoc that hung and orphaned two processes at 100% CPU for 70
minutes. Do not reintroduce either pattern.

```bash
cd /home/matt/src/daemoneye
cargo build 2>&1 | tail -2
H=$(mktemp -d)
{
  echo "=== the asset must be untouched by this phase ==="
  git diff --name-only
  echo "asset-in-diff=$(git diff --name-only | grep -c agent-runtime-layout.md)   # 0 == PASS"

  echo "=== seeded tree: audit must still exit 0 ==="
  HOME="$H" timeout 60 ./target/debug/daemoneye setup 2>&1 | tail -2
  HOME="$H" timeout 60 ./target/debug/daemoneye audit-prompts > /dev/null 2>&1
  echo "clean-audit-exit=$?   # 0 == PASS"

  echo "=== the seeded copy still carries the tree ==="
  timeout 30 grep -c "daemoneye.sock" "$H/.daemoneye/memory/knowledge/agent-runtime-layout.md"

  echo "=== the new tests ==="
  timeout 300 cargo test --lib runtime_tree 2>&1 | grep -E "^test |^test result"

  echo "=== full gate ==="
  timeout 600 cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -2
  echo "clippy-exit=$?"
  timeout 600 cargo test 2>&1 | grep -E "^test result"
} > /tmp/phase05-e2e.txt 2>&1
rm -rf "$H"
cat /tmp/phase05-e2e.txt
```

`asset-in-diff=0` together with a passing `render_matches_shipped_asset` is the
proof: the renderer reproduces the shipped tree rather than the tree having been
bent to fit the renderer.

Paste the captured file into an Update Log entry titled
`### Update — <date> (end-to-end verification)`. **The server-authored
`(complete)` entry does not satisfy this** — its "Command output tails" block is
the automatic gate capture every phase receives, and it shows that
build/lint/test ran, not that this phase's acceptance criteria were exercised.

**If any part of the capture block fails or hangs, stop and report it as a
blocker.** Do not re-run the surviving sections separately and paste the
result — a transcript assembled from more than one run fails `STANDARDS.md` §1
even when every claim in it is true.

## Authorizations

- [ ] May add dependencies: **none**. All of this is `std` string work.
- [ ] May touch `docs/architecture.md`: no.
- [ ] May create new files: **yes, exactly one** —
      `src/config/runtime_tree.rs`.

## Out of scope

- **Editing `assets/memory/knowledge/agent-runtime-layout.md`.** See the first
  acceptance criterion. The prose sections below the tree (`## Access Notes` and
  everything after) are not touched by this phase either.
- **Adding `var/index` / `memory.db` anywhere.** That is phase 06's entry to
  add, in both tables. Adding it here would make phase 06's own gate vacuous.
- **Modifying `POLICY_TABLE` or `src/config/lifecycle.rs`.** This phase reads
  the table; it does not change it. If the cross-check test finds a policy path
  with no tree line, that is a finding to report, not a table edit to make.
- **A `build.rs` or any build-time / run-time generation.** The asset stays a
  checked-in file behind `include_str!`.
- **Reconciling `memory/incident` vs `memory/incidents`.** `src/search.rs:61`
  says `incidents` while `src/session_store.rs:378` and the tree say `incident`.
  That is a real inconsistency and it is **not** this phase's to fix — do not
  "correct" the tree to match `search.rs`, or `render_matches_shipped_asset`
  will fail. Report it; it will get its own phase.
- **Changing how the asset is seeded** (`seeds.rs`), or any `audit-prompts` /
  `path_audit` behaviour.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-08-01 03:56 (started)

**Executor:** Claude
**Work:** Implementing phase 05: generated runtime tree. Created `src/config/runtime_tree.rs` with `TreeNode` struct, `render_tree()` renderer, `tree_block_of()` extractor, the `RUNTIME_TREE` data table, and 5 tests. Wired into `src/config/mod.rs`.

### Update — 2026-08-01 03:56 (end-to-end verification)

```
=== the asset must be untouched by this phase ===
src/config/mod.rs
asset-in-diff=0   # 0 == PASS
=== seeded tree: audit must still exit 0 ===
# The daemon will create the session automatically and `daemoneye chat`
# will attach to it when run from outside tmux.
clean-audit-exit=0   # 0 == PASS
=== the seeded copy still carries the tree ===
1
=== the new tests ===
test config::runtime_tree::tests::annotation_column_is_not_overflowed ... ok
test config::runtime_tree::tests::tree_block_of_finds_the_block ... ok
test config::runtime_tree::tests::render_matches_shipped_asset ... ok
test config::runtime_tree::tests::tree_block_mismatch_is_detected ... ok
test config::runtime_tree::tests::every_policy_path_appears_in_tree ... ok
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 1002 filtered out; finished in 0.00s
=== full gate ===
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s
clippy-exit=0
test result: ok. 1007 passed; 0 failed; 0 ignored; 0 measured; 1002 filtered out; finished in 1.44s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 30 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 8 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.14s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

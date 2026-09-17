# Phase 09a: remove the three pane *read* tools

**Milestone:** M22 — Tool surface & context
**Status:** in-progress
**Depends on:** 03a (`list_shells`), 03b (`read_shell`), 03c (`find_in_shells`)
— the three replacements. All **done**.
**Estimated diff:** ~750 lines, overwhelmingly deletion
**Tags:** language=rust, kind=removal, size=m

## Goal

`list_panes`, `read_pane` and `find_in_panes` each have a shipped shell
replacement. This phase deletes them.

| Removed | Replacement | Shipped in |
|---|---|---|
| `list_panes` | `list_shells` | 03a |
| `read_pane` | `read_shell` | 03b |
| `find_in_panes` | `find_in_shells` | 03c |

After this phase: **45 tools — 31 core + 14 deferred** (from 48 = 34 + 14; the
deferred count does not move, all three are core). `doc_truth` stays at 10.

### What this phase is not

Charter row 09 reads "delete the six pane tools and their executor arms."
Measured, the six are **194 references**, and the row also carries the
`target_pane` (**171** refs) / `retry_in_pane` (**14** refs) retirements the
07a/07b note folded into it. That is far past one executor session, so the row
splits on the seam that matters — *whether a replacement already exists*:

- **09a (this phase)** — the three **read** tools. Their replacements shipped in
  03a/03b/03c, so the removal is a straight deletion with nothing to design.
- **09b** — `watch_pane` (20 refs; `watch_shell` from 04b replaces it) and
  `close_background_window` (12 refs; tied to the `de-bg-*` window substrate,
  which needs a decision about what `wait: false` closes under the PTY backend).
- **09c** — `tmux_control` (38 refs, 22 of them in `executor/mod.rs` and 8 in
  `agents/policy.rs`). It is not a read tool at all — it *drives* tmux
  (focus/zoom/split/rename_window/kill_window) and has no shell analogue, so
  removing it is a capability decision, not a swap.
- **`target_pane` / `retry_in_pane`** — 185 refs across the IPC wire, the CLI
  renderer and the whole foreground-execution path. Its own phase, after 09c.

Nothing below touches the pane **context blocks** (`[PANE MAP]`,
`[VISIBLE PANE]`, …) — those belong to a later phase in the row, and 08a
deliberately left them live alongside the new `[SHELLS]` block.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.5 "The AI tool surface (36 → 42)" — the
  removal list.
- `CLAUDE.md` § "Adding a new AI tool (checklist)" — the eleven steps, **run in
  reverse** here. `tests/doc_truth.rs` enforces steps 10 and 11 and will name
  both the stale row and the expected counts if either is missed.

## Pre-flight

Every number below was measured against HEAD `55fc1936` on 2026-09-16.

### ⚠ The single most dangerous thing in this phase

**`crate::tmux` has functions whose names contain the tool names.** They are the
tmux *substrate*, they have real non-AI callers, and deleting them breaks the
build in places that have nothing to do with the AI tool surface. Measured:

```sh
$ grep -rn 'list_panes\|read_pane\|find_in_panes' src/ --include=*.rs | grep -v 'knowledge/pane.rs'
src/tmux/pane.rs:45:pub fn list_panes_detailed() -> Result<Vec<RichPaneInfo>> {
src/tmux/pane.rs:465:pub fn read_pane_exit_status(pane_id: &str) -> Option<i32> {
src/tmux/pane.rs:537:pub fn list_panes_in_window(window_id: &str) -> Result<Vec<String>> {
src/pane_prefs.rs:62:        crate::tmux::list_panes_detailed()
src/tmux/cache.rs:315:        let rich_panes = tmux::list_panes_detailed().unwrap_or_default();
src/daemon/background/gc.rs:186:    let all_panes = match crate::tmux::pane::list_panes_detailed() {
src/cli/commands/pane.rs:27:        crate::tmux::list_panes_in_window(&window_id)
src/daemon/executor/foreground.rs:828:                            tmux::read_pane_exit_status(&t)
```

**These five must survive, untouched:**

| Symbol | Where | Why it stays |
|---|---|---|
| `tmux::list_panes_detailed` | `src/tmux/pane.rs:45` | the 2 s cache poll, `pane_prefs`, background GC |
| `tmux::list_panes_in_window` | `src/tmux/pane.rs:537` | the CLI `/panes` inspector |
| `tmux::read_pane_exit_status` | `src/tmux/pane.rs:465` | foreground completion detection |
| `handle_list_panes` | `src/daemon/server/handlers.rs:164` | serves `Request::ListPanesForSession` |
| `Request::ListPanesForSession` | `src/ipc.rs:338` | the **user's** `/panes` command, not an AI tool |

The last two are the subtle pair: `handle_list_panes` is the IPC handler for the
human's `/panes` inspector and is **completely unrelated** to
`PendingCall::ListPanes`. Removing it would delete a user-facing feature while
the tool-count gate stayed green.

A blunt `grep -rl read_pane src | xargs sed` would destroy four of these five.
Work from the site list in § Spec, not from a global search-and-replace.

### The site map — every place the three tools live

```sh
$ grep -cE 'ListPanes|ReadPane|FindInPanes' src/ai/types/pending.rs   # 23
$ grep -cE 'ListPanes|ReadPane|FindInPanes' src/ai/types/events.rs    #  3
$ grep -cE 'ListPanes|ReadPane|FindInPanes' src/daemon/stream.rs      #  6
$ grep -cE 'list_panes|read_pane|find_in_panes' assets/prompts/sre.toml  # 6
```

`PendingCall` arms in `src/ai/types/pending.rs`, by line: **816** (`to_tool_call`
ListPanes), **914** (ReadPane), **928** (FindInPanes), **1048/1056/1057**
(`id()`), **1088/1094/1095** (`should_emit_tool_feedback`), **1200/1208/1223**
(`summary()`), **1334/1342/1343** (`tool_name()`). The rest are tests — see the
retarget rule below.

`AiEvent` arms in `src/daemon/stream.rs`: **678**, **698**, **713**.

### `pane.rs` — what goes and what stays

`src/daemon/executor/knowledge/pane.rs` is 1417 lines and holds **all six**
pane tools. This phase removes three of them and leaves the file in place.

```sh
$ grep -n '^pub fn\|^pub async fn\|^pub(crate) fn\|^fn ' src/daemon/executor/knowledge/pane.rs
13:pub fn close_bg_window(…)          # STAYS — 09b
82:fn read_pane_depth(…)              # GOES
94:pub async fn read_pane(…)          # GOES
231:fn search_buffer(…)               # GOES
250:pub async fn find_in_panes(…)     # GOES
470:pub(crate) fn is_daemon_window(…) # STAYS — tmux_control helper, 09c
478:pub(crate) fn kill_window_refusal(…) # STAYS — 09c
496:pub fn list_panes(…)              # GOES
658:pub fn watch_pane(…)              # STAYS — 09b
```

`knowledge/mod.rs:20-22` re-exports them; shrink that list to the survivors:

```rust
pub(super) use pane::{
    close_bg_window, find_in_panes, kill_window_refusal, list_panes, read_pane, watch_pane,
};
```

### The retarget rule — the way this phase silently loses coverage

`pane.rs`'s 27 tests split cleanly: **20 test the three tools' behaviour and are
deleted with them; 5 test the survivors and stay.**

```
DELETE (20): find_in_panes_* (7), list_panes_* (6), read_pane_* (5), search_buffer_* (2)
KEEP    (5): close_bg_window_* (2), kill_window_refusal_* (3)
```

**But tests outside `pane.rs` that merely *mention* the tool names are a
different case.** They use a pane tool as an arbitrary *vehicle* for testing
something generic, and the generic behaviour must keep its coverage. These are
**retargeted to a surviving tool, never deleted**:

- `src/ai/types/pending.rs` — `to_tool_call_read_pane_omits_none_options` and
  `to_tool_call_read_pane_preserves_string_null_grep` test the **null-stripping
  helper**, not `read_pane`. Retarget onto `PendingCall::ReadShell`, which has
  the same optional-argument shape (`lines`, `filter`). Rename to match.
  `to_tool_call_preserves_present_options` uses both `FindInPanes` (for `scope`)
  and `mk_read_pane` (for `lines`) — `FindInShells` and `ReadShell` carry the
  same two, so retarget in place and keep the test.
- `src/ai/backends/{openai,anthropic,gemini}.rs` — `"list_panes"` / `"read_pane"`
  appear as sample tool names in **wire-parsing** tests (streaming a partial
  tool call, a broken-JSON flush). Any real tool name works; swap to a surviving
  one.
- `src/agents/policy.rs:121,130` and `src/daemon/executor/mod.rs:1715,1749` —
  `"read_pane"` is a sample entry in `ToolPolicy` allow/deny tests. Same: swap
  to a surviving tool name.

The pinned test counts below **enforce** this rule. `cargo test --lib` must land
on **1886** — exactly `1906 − 20`. A count **below** 1886 means vehicle tests
were deleted instead of retargeted, and the phase has quietly dropped coverage
of the null-stripping helper, the wire parser, or `ToolPolicy`.

### Prose that points at the removed tools

These strings tell the model to call a tool that will no longer exist. Each must
be rewritten to name the shell replacement:

- `src/daemon/executor/mod.rs:888` — "…use list_panes to check."
- `src/daemon/executor/mod.rs:1100` — "Call list_panes to see available panes."
- `src/daemon/executor/foreground.rs:169` — "…or call list_panes to find the correct pane ID."
- `src/daemon/executor/foreground.rs:212` — "Call list_panes to discover current pane IDs…"
- `src/daemon/executor/knowledge/shell.rs:503` — "…or use read_pane for a …"
- `src/ai/tools/defs.rs:84` — the `target_pane` param description
- `src/ai/tools/defs.rs:299` — `watch_pane`'s `pane_id` description
- `src/ai/tools/defs.rs:1118`, `:1192` — descriptions of the tools being removed (go with them)

### ⚠ The enum-variant hazard — batch, do not iterate

Removing a `PendingCall` / `AiEvent` variant makes the tree **uncompilable until
every match arm is gone**. There are roughly **eighteen** arms across
`pending.rs`, `events.rs`, `stream.rs`, `dispatch.rs` and `executor/mod.rs`.

Do **all** of them in one batch *before* running `cargo build`. Running a gate
after each site produces a long run of identical non-exhaustive-match errors,
which is what made an earlier enum-variant phase in this milestone stall. The
compiler's error list after the batch is your checklist — work it to zero.

### The dispatch sample-args table is table-driven

`src/ai/tools/dispatch.rs:220-260` is a single test iterating every entry in
`TOOLS` with a per-tool sample-args match arm. Remove the `"list_panes"`,
`"read_pane"` and `"find_in_panes"` arms along with the tools. **The test count
there does not change** — one test, three fewer iterations. This is why 1886 is
`1906 − 20` and not something smaller.

## Current state

- HEAD `55fc1936`, tree clean.
- `cargo test --lib` → **1906 passed**, 4 ignored.
- `cargo test --lib 'knowledge::pane'` → **27 passed**.
- `cargo test --test doc_truth` → **10 passed**.
- **48 tools: 34 core + 14 deferred.**
- `assets/prompts/sre.toml` → **6** lines mention the three tools;
  `src/config/seeds.rs` → **0** (it re-reads the asset).

## Spec

### Task 1 — remove the `PendingCall` variants

In `src/ai/types/pending.rs`, delete the `ListPanes`, `ReadPane` and
`FindInPanes` variants and every arm listed in § Pre-flight's site map:
`to_tool_call()`, `id()`, `should_emit_tool_feedback()`, `summary()`,
`tool_name()`.

### Task 2 — remove the `AiEvent` variants

In `src/ai/types/events.rs`, delete the three variants (3 references).

### Task 3 — remove the `ToolDef` entries

In `src/ai/tools/defs.rs`, delete the three `ToolDef` entries (`list_panes` at
`:1090`, `read_pane` at `:1104`, `find_in_panes` at `:1139`) and repair the two
surviving descriptions that name them (`:84`, `:299`).

### Task 4 — remove the dispatch arms

In `src/ai/tools/dispatch.rs`, delete the three arms in `dispatch_tool_event()`
(`:41`, `:62`, `:63`) and the three sample-args arms (`:232`, `:233`, `:255`).

### Task 5 — remove the streaming arms

In `src/daemon/stream.rs`, delete the three `AiEvent` arms (`:678`, `:698`,
`:713`).

### Task 6 — remove the executor arms

In `src/daemon/executor/mod.rs`, delete the `PendingCall::ListPanes` (`:895`),
`ReadPane` (`:912`) and `FindInPanes` (`:1056`) arms, and rewrite the two prose
strings at `:888` and `:1100`.

### Task 7 — gut `pane.rs`

Delete `read_pane_depth`, `read_pane`, `search_buffer`, `find_in_panes` and
`list_panes`, plus their 20 tests. **Leave `close_bg_window`,
`is_daemon_window`, `kill_window_refusal` and `watch_pane` and their 5 tests
exactly as they are.** Shrink the `knowledge/mod.rs` re-export to the
survivors.

### Task 8 — retarget the vehicle tests

Per § Pre-flight's retarget rule: `pending.rs` (three tests onto `ReadShell` /
`FindInShells`), the three backend files, `agents/policy.rs`, and the
`ToolPolicy` tests in `executor/mod.rs`. **Retarget; do not delete.**

### Task 9 — prompt assets and doc tables

- `assets/prompts/sre.toml`: remove the 6 references; where a sentence told the
  model to read a pane, point it at the shell tool instead.
- `CLAUDE.md`: delete the three rows and change the count line to
  **48 → 45 tools: 34 → 31 core + 14 deferred**.
- `README.md` § "AI tools": remove the three from the Core table and update the
  three numbers in the prose above it.

`tests/doc_truth.rs` is the gate for the last two and will name what is stale.

### Task 10 — mutation check

Use the `patch` tool. Re-add a `ToolDef` entry for `list_panes` to the `TOOLS`
slice (name and description only — any valid `category` and
`deferred_group: None`) **without** restoring its `CLAUDE.md` and `README.md`
rows.

Run `cargo test --test doc_truth`. It **must** fail, and the failure must name
the count mismatch or the missing row. Revert with the `patch` tool and confirm
green. Paste both `test result:` lines into the Update Log.

This proves the doc-truth gate actually binds the new counts rather than having
been satisfied by a coincidence.

### Task 11 — end-to-end capture

Prove the three tools are gone from what the model is actually offered, not just
from the source.

**Where it goes and the exact call idiom** — `src/ai/tools/defs.rs` has **no
test module** (0 tests); put both new tests in `src/ai/tools/dispatch.rs`'s
existing `mod tests`, which already imports everything they need:

```rust
    use super::super::defs::{APPROVAL_GATED_TOOLS, TOOLS};
    use super::super::schema::{…, render_gemini, select_tools, tools_in_group};
```

Note the argument shape. `TOOLS` is `&[ToolDef]` but `render_gemini` takes
`&[&ToolDef]`, so `render_gemini(TOOLS)` does **not** compile. The idiom used
by every existing caller (`dispatch.rs:155`, `:171`, `:204`, `:375`) is:

```rust
let rendered = render_gemini(&TOOLS.iter().collect::<Vec<_>>());
```

Add this test:

```rust
    #[test]
    fn e2e_removed_pane_read_tools_are_absent_from_every_render() {
        let names: Vec<&str> = TOOLS.iter().map(|t| t.name).collect();
        for gone in ["list_panes", "read_pane", "find_in_panes"] {
            assert!(!names.contains(&gone), "{gone} must be gone from TOOLS");
        }
        for kept in ["list_shells", "read_shell", "find_in_shells"] {
            assert!(names.contains(&kept), "{kept} is the replacement and must stay");
        }
        let rendered = render_gemini(&TOOLS.iter().collect::<Vec<_>>());
        let blob = serde_json::to_string(&rendered).expect("render must serialize");
        for gone in ["list_panes", "read_pane", "find_in_panes"] {
            assert!(!blob.contains(gone), "{gone} must not survive in the render: {blob:.400}");
        }
        println!("PASTE MATCH >>>\ntools={} core={}\n<<<",
            TOOLS.len(),
            TOOLS.iter().filter(|t| t.deferred_group.is_none()).count());
    }
```

Run it with `cargo test --lib e2e_removed_pane_read_tools -- --nocapture`. Copy
the text **between** `PASTE MATCH >>>` and `<<<` into the Update Log **byte for
byte** — it must read `tools=45 core=31`. This test counts as one of the two new
tests below.

## Acceptance criteria

Each was run against HEAD `55fc1936` and **fails** there; "now" is the measured
current value.

- [ ] `grep -c '^        name: "' src/ai/tools/defs.rs` → **45** (now 48)
- [ ] `grep -c 'deferred_group: None' src/ai/tools/defs.rs` → **31** (now 34)
- [ ] `grep -cE 'ListPanes|ReadPane|FindInPanes' src/ai/types/pending.rs` → **0** (now 23)
- [ ] `grep -cE 'ListPanes|ReadPane|FindInPanes' src/ai/types/events.rs` → **0** (now 3)
- [ ] `grep -cE 'ListPanes|ReadPane|FindInPanes' src/daemon/stream.rs` → **0** (now 6)
- [ ] `grep -cE '^pub fn list_panes|^pub async fn read_pane|^pub async fn find_in_panes' src/daemon/executor/knowledge/pane.rs` → **0** (now 3)
- [ ] `grep -cE 'list_panes|read_pane|find_in_panes' assets/prompts/sre.toml` → **0** (now 6)
- [ ] `cargo test --lib 'knowledge::pane'` reports **5 passed** (now 27) — the `close_bg_window_*` and `kill_window_refusal_*` survivors
- [ ] `cargo test --lib` reports **1886 passed** (now 1906). Exactly `1906 − 20`: a **lower** number means vehicle tests were deleted instead of retargeted (§ Pre-flight). If you believe a different number is correct, say so in the Update Log rather than moving the criterion.
- [ ] `cargo test --test doc_truth` reports **10 passed**
- [ ] `cargo test --lib 'ai::tools::dispatch'` reports **24 passed** (now 22) — the two tests of the test plan
- [ ] Task 10's mutation: re-adding a `list_panes` `ToolDef` without its doc rows turns `doc_truth` **red**; both `test result:` lines in the Update Log
- [ ] Task 11's `PASTE MATCH` block reads exactly `tools=45 core=31`
- [ ] `cargo fmt --all` clean, `cargo build` clean, `cargo clippy --all-targets --all-features -- -D warnings` clean

**Must-survive criteria** — these pass now and must still pass after:

- [ ] `grep -c 'pub fn list_panes_detailed' src/tmux/pane.rs` → **1**
- [ ] `grep -c 'pub fn list_panes_in_window' src/tmux/pane.rs` → **1**
- [ ] `grep -c 'pub fn read_pane_exit_status' src/tmux/pane.rs` → **1**
- [ ] `grep -c 'pub(super) async fn handle_list_panes' src/daemon/server/handlers.rs` → **1**
- [ ] `grep -c 'ListPanesForSession' src/ipc.rs` → **2**
- [ ] `grep -cE '^pub fn close_bg_window|^pub fn watch_pane|^pub\(crate\) fn kill_window_refusal' src/daemon/executor/knowledge/pane.rs` → **3**
- [ ] `grep -c 'name: "watch_pane"' src/ai/tools/defs.rs` → **1** and `grep -c 'name: "tmux_control"' src/ai/tools/defs.rs` → **1** — 09b and 09c, not this phase
- [ ] `git diff --stat 55fc1936..HEAD -- src/tmux/` is **empty** — the tmux substrate is not this phase's business

## Test plan

This phase **removes** 20 tests and **adds** 2. Net `cargo test --lib`:
`1906 − 20 = 1886`.

**Removed (20)** — all in `src/daemon/executor/knowledge/pane.rs`, named in
§ Pre-flight: the seven `find_in_panes_*`, six `list_panes_*`, five
`read_pane_*` and two `search_buffer_*`.

**Retargeted, not removed** — the `pending.rs`, backend, `policy.rs` and
`executor/mod.rs` vehicle tests. Their names change to match their new vehicle;
their count does not.

**Added (2)**

1. `e2e_removed_pane_read_tools_are_absent_from_every_render` (Task 11) — the
   three names are absent from `TOOLS` **and** from `render_gemini`'s output,
   and the three replacements are present. Pins the negative case, which a
   count alone does not: 45 tools would still be 45 if the wrong three left.
2. `tool_table_counts_are_45_31_14`, also in `src/ai/tools/dispatch.rs`'s test module — asserts
   `TOOLS.len() == 45`, core `== 31`, deferred `== 14`. A plain arithmetic
   guard so a later phase that adds a tool without touching the docs trips here
   as well as in `doc_truth`.

## End-to-end verification

The Task 11 `PASTE MATCH` block (`tools=45 core=31`) and the Task 10 mutation
pair, both in their own Update Log entry titled
`### Update — <date> (end-to-end verification)`. Per `STANDARDS.md` §1 the
server-authored `(complete)` entry does **not** satisfy this.

## Authorizations

Edits are authorized in exactly these files:

- `src/ai/types/pending.rs`, `src/ai/types/events.rs`
- `src/ai/tools/defs.rs`, `src/ai/tools/dispatch.rs`
- `src/ai/backends/openai.rs`, `anthropic.rs`, `gemini.rs` — **test fixtures only**
- `src/daemon/stream.rs`, `src/daemon/executor/mod.rs`
- `src/daemon/executor/knowledge/pane.rs`, `src/daemon/executor/knowledge/mod.rs`
- `src/daemon/executor/knowledge/shell.rs` — **the `:503` message only**
- `src/daemon/executor/foreground.rs` — **the `:169` and `:212` messages only**
- `src/agents/policy.rs` — **test fixtures only**
- `assets/prompts/sre.toml`, `CLAUDE.md`, `README.md`
- This phase doc's Update Log.

**Not authorized:** anything under `src/tmux/`, `src/ipc.rs`,
`src/daemon/server/`, `src/cli/`, `src/pane_prefs.rs`,
`src/daemon/background/`, `tests/doc_truth.rs`, `src/config/seeds.rs`.

If you believe an unauthorized file must change, stop and say so in the Update
Log rather than editing it.

## Out of scope

- `watch_pane`, `close_background_window` — **09b**.
- `tmux_control` — **09c**.
- `target_pane` / `retry_in_pane` — their own phase after 09c.
- The pane **context blocks** (`[PANE MAP]`, `[VISIBLE PANE]`, …) and anything
  in `src/tmux/cache.rs`.
- Renaming `get_terminal_context` — **08b**.
- Rewriting `sre.toml` wholesale — **10**. This phase touches only the 6 lines
  that name the three removed tools.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-09-17 01:07 (started)

**Executor:** headless executor, phase 09a run.

Pre-flight re-verified against HEAD `feba4352` (tree clean), which carries the
same source as `55fc1936`:

- `cargo test --lib` → 1906 passed, 4 ignored ✅ (spec says 1906)
- `cargo test --lib 'knowledge::pane'` → 27 passed ✅
- `cargo test --test doc_truth` → 10 passed ✅
- defs.rs: 48 `name:` entries, 34 `deferred_group: None` ✅
- pending.rs/events.rs/stream.rs/sre.toml reference counts match the site map
  (23 / 3 / 6 / 6); seeds.rs → 0 ✅
- All nine must-survive greps hold ✅

Starting with Task 1; enum-variant sites will be batched before any build.

### Update — 2026-09-17 01:55 (blocker)

**Blocked on:** the removal orphans `tmux::capture_pane_annotated`
(`src/tmux/pane.rs:671`), which is the only consumer-left-dead function outside
the authorized file set, so `cargo clippy … -D warnings` fails with one
`dead_code` error — and spec/authorizations leave no legal fix.

**What I tried:**

1. Measured every caller of `capture_pane_annotated` at HEAD `feba4352`
   (worktree `/tmp/dn-base-1789608862`): exactly two, both in
   `src/daemon/executor/knowledge/pane.rs` (`:150` in `read_pane`,
   `:374` in `find_in_panes`'s foreign-scope half) — both deleted by this
   phase. No other caller exists anywhere in `src/`.
2. Checked whether a *surviving* function needs it: `watch_pane` captures with
   `tmux::capture_pane(...)` + `annotate_output` (mod-level wrapper) — it never
   used the annotated capture. `shell.rs` and `foreground.rs` do not touch it.
   So the function is genuinely orphaned by this phase, not temporarily unused.
3. Considered the authorized-file escape hatches: `knowledge/pane.rs` cannot
   hold a wrapper because there is no call site left to call it; `dispatch.rs` /
   `mod.rs` have no business re-rooting a tmux substrate helper.
4. Looked at whether `annotate_ansi` (the thing `capture_pane_annotated`
   uniquely exposes across the `ansi` privacy boundary) has another inside-`tmux`
   consumer that would keep the function alive if it were *not* dead — it does
   not change the outcome: the function itself is dead.

**What I need:** one of —

- **(a)** authorization to edit `src/tmux/pane.rs` to delete
  `capture_pane_annotated` (6 lines; `capture_pane_with_escapes` stays —
  `src/tmux/cache.rs` uses it), plus the stale doc reference in
  `src/tmux/mod.rs:14` if that is granted with it; **or**
- **(b)** an exemption declaring the `dead_code` warning acceptable for this
  phase (it is *added* by this phase's deletions, not pre-existing), so I may
  ...— but note § Hard rules forbids adding `#[allow(...)]` to mask it, which is
  why (b) needs the principal engineer's say-so rather than an executor choice.

Everything else in the phase is complete and green (`cargo build` clean, all
1886 + doc_truth + dispatch-count criteria met, mutation check red/green pair
captured); only this last gate is blocked.

# Phase 09c: remove `tmux_control` — the milestone's exit criterion

**Milestone:** M22 — Tool surface & context
**Status:** todo
**Depends on:** 09a, 09b (the other five pane tools). Both **done**.
**Estimated diff:** ~600 lines, overwhelmingly deletion
**Tags:** language=rust, kind=removal, size=m

## Goal

The last of the six pane tools. Unlike the other five, `tmux_control` has **no
shell counterpart** — it *drives* tmux (`focus` / `zoom` / `unzoom` / `split` /
`rename_window` / `kill_window`) rather than reading it. Removing it is the
deliberate end of the AI's ability to manipulate the user's tmux layout, which
is precisely what the milestone set out to do:

> **Goal:** make the AI stop reasoning in tmux panes.

After this phase the milestone's headline exit criterion is met:

**42 tools — 28 core + 14 deferred.**

Three further things move, all guarded by `tests/doc_truth.rs`:

1. **`APPROVAL_GATED_TOOLS` shrinks 11 → 10.** `tmux_control` is the only one of
   the six pane tools that prompts the user.
2. **The README's `⚠` marker** on `tmux_control` goes with it — `doc_truth`
   holds the list and the markers in sync.
3. **`src/daemon/executor/knowledge/pane.rs` is deleted entirely.** After 09a
   and 09b it holds only two helpers, and both exist solely for this tool.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.5 — the removal list.
- `CLAUDE.md` § "Adding a new AI tool (checklist)" — eleven steps, **in
  reverse**. Step 12 (the `APPROVAL_GATED_TOOLS` + `⚠` pair) is in play here,
  which it was not for 09a or 09b.

## Pre-flight

Measured against HEAD `16e4bcad` on 2026-09-17.

### The orphan closure — functions, wrappers **and** `use` statements

09b's review found that measuring callee *functions* was not enough: a
`pub(super) use` that existed only for the removed caller tripped
`-D unused-imports`. This closure covers both.

**1. `knowledge/pane.rs` dies entirely.** After 09b the file is 72 lines with
two functions. Measured callers:

```sh
$ grep -rn 'kill_window_refusal' src/ --include=*.rs
src/daemon/executor/knowledge/mod.rs:20:pub(super) use pane::kill_window_refusal;
src/daemon/executor/mod.rs:1049:  && let Some(refusal) = knowledge::kill_window_refusal(   ← the TmuxControl arm
src/daemon/executor/knowledge/pane.rs:10,33,35,39,44,45,55,58,65          ← itself + its 3 tests
```

`kill_window_refusal`'s only production caller is the arm this phase deletes.
`is_daemon_window` (the pane.rs *wrapper*) is called only by
`kill_window_refusal`. So **delete the file**, plus `mod pane;` and the
`pub(super) use pane::kill_window_refusal;` line in `knowledge/mod.rs`.

**2. `ghost_may_use_tmux_control` dies.** `src/daemon/executor/mod.rs:134`, no
callers outside that file — the arm at `:1009` and its four tests at `:1638`.

**3. `explicitly_allows` SURVIVES.** It reads like a `tmux_control`-only helper
(its doc comment at `src/agents/policy.rs:36` even says "the ghost
`tmux_control` gate"), but measured:

```sh
$ grep -rn 'explicitly_allows' src/ --include=*.rs | grep -v 'agents/policy.rs'
src/daemon/executor/mod.rs:138:  …p.explicitly_allows("tmux_control")   ← goes
src/daemon/executor/mod.rs:157:  …p.explicitly_allows("send_input")     ← STAYS
```

`ghost_may_send_input` is the second caller. Keep `explicitly_allows`, and
update its `:36` doc comment so it no longer names a tool that does not exist.

### ⚠ Must survive — `crate::daemon::is_daemon_window`

The same trap 09a carried. There are **two** `is_daemon_window` functions:

| Symbol | Fate |
|---|---|
| `knowledge::pane::is_daemon_window` (`pane.rs:2`) | **goes** — a one-line wrapper used only by `kill_window_refusal` |
| `crate::daemon::is_daemon_window` (`src/daemon/mod.rs:74`) | **STAYS** — the real predicate |

The real one has live callers at `src/tmux/cache.rs:432`, `:700`, and
`src/daemon/mod.rs:113` (`is_targetable_pane`), plus its own tests at
`:1164`/`:1173`. Deleting it would break the pane cache and the targetability
predicate. Work from this table, not from a name search.

### The `send_input` doc comment becomes false

`src/daemon/executor/mod.rs:149-152`, on `ghost_may_send_input`:

```rust
/// This is a deliberate second copy of `ghost_may_use_tmux_control`, not an
/// oversight: two gates are under the three-caller abstraction point in
/// `STANDARDS.md` § 2.2, and a third collapses them into one
/// `ghost_may_use(tool, is_ghost, policy)`.
```

After this phase there is no first copy and there are not two gates — there is
**one**. Rewrite the paragraph. The surviving fact worth keeping is *why*
`send_input` defaults to deny for ghosts, not the abstraction-point arithmetic.
The sentence above it ("covers the same no-auto-approve ground as
`tmux_control`") also names the removed tool and must go.

### ⚠ 09b's E2E test lists `tmux_control` as must-stay

`src/ai/tools/dispatch.rs:164`, written by 09b:

```rust
        for kept in ["watch_shell", "close_shell", "tmux_control"] {
            assert!(names.contains(&kept), "{kept} must stay");
        }
```

**This test fails the moment `tmux_control` leaves `TOOLS`.** Drop
`"tmux_control"` from that array — it was correct for 09b (where the tool had
to survive) and is wrong for 09c. Do not delete the test; the other two entries
still pin 09b's removals.

### The retarget rule — `agents/policy.rs`

`src/agents/policy.rs` holds **8** `tmux_control` references, but **none are
production code**: one doc comment (`:36`) and the rest are
`explicitly_allows_matches_only_the_allow_list` using `"tmux_control"` as an
arbitrary vehicle tool name (`:115`–`:137`).

`explicitly_allows` survives, so its test must too. **Retarget the vehicle to
`"send_input"`** — the surviving tool that actually uses explicit-allow
semantics — rather than deleting the test. `agents::policy` stays at **15**
tests.

### The four gate tests, by contrast, are genuinely redundant

`mod tmux_control_gate_tests` (`executor/mod.rs:1638`) holds 4 tests for
`ghost_may_use_tmux_control`. Its twin `ghost_may_send_input` **already has its
own test** covering the same four cases in one:

```sh
$ grep -n 'ghost_may_send_input_requires_an_explicit_allow' src/daemon/executor/mod.rs
1679:    fn ghost_may_send_input_requires_an_explicit_allow() {
```

So deleting the 4 loses no coverage of any surviving behaviour — **delete them,
do not retarget**. This is the opposite call from `policy.rs` above; the
difference is whether the surviving twin is already tested.

### Test deltas — derived

```
REMOVE 4  src/daemon/executor/mod.rs — mod tmux_control_gate_tests
REMOVE 3  src/daemon/executor/knowledge/pane.rs — the file, with its 3 tests
RETARGET  src/agents/policy.rs — explicitly_allows_… vehicle → "send_input" (15 stays 15)
EDIT      src/ai/tools/dispatch.rs:164 — drop "tmux_control" from 09b's kept list
ADD    1  src/ai/tools/dispatch.rs — the E2E of Task 9
```

Net `cargo test --lib`: `1883 − 4 − 3 + 1 = 1877`.
`ai::tools::dispatch` goes 25 → **26**. `agents::policy` stays **15**.
`knowledge::pane` goes 3 → **0** (the filter matches nothing once the module is
gone; `cargo test --lib 'knowledge::pane'` reports `0 passed`).

### The count-guard rename, again

09b renamed the guard to `tool_table_counts_are_43_29_14`. Rename it to
`tool_table_counts_are_42_28_14` and update its three assertions. Leaving the
old name with new numbers is the failure the name exists to prevent.

## Current state

- HEAD `16e4bcad`, tree clean.
- `cargo test --lib` → **1883 passed**, 4 ignored.
- `cargo test --lib 'ai::tools::dispatch'` → **25 passed**.
- `cargo test --lib 'agents::policy'` → **15 passed**.
- `cargo test --lib 'knowledge::pane'` → **3 passed**.
- `cargo test --lib 'tmux_control_gate'` → **4 passed**.
- `cargo test --test doc_truth` → **10 passed**.
- **43 tools: 29 core + 14 deferred**; `APPROVAL_GATED_TOOLS` has **11** entries.
- `README.md` mentions `tmux_control` twice; `CLAUDE.md` once; `sre.toml` twice.

## Spec

### Task 1 — remove the `PendingCall` / `AiEvent` variants and the args struct

`src/ai/types/pending.rs`: delete the `TmuxControl` variant and every arm
(`to_tool_call()` `:905`, `id()` `:968`, `summary()` `:1120`, `tool_name()`
`:1211`, and the entry in the exhaustiveness list at `:1771`).
`src/ai/types/events.rs`: delete the variant.
`src/ai/tools/args.rs`: delete `TmuxControlArgs` (`:100`) and its `ToolArgs`
impl (`:465`).

**Batch every match arm before running a gate** — a removed variant leaves the
tree uncompilable until the last arm is gone.

### Task 2 — remove the `ToolDef`, the approval gate, and the dispatch arms

- `src/ai/tools/defs.rs`: delete the `ToolDef` at `:1057` **and** the
  `"tmux_control"` entry in `APPROVAL_GATED_TOOLS` at `:37` (11 → 10).
- `src/ai/tools/dispatch.rs`: delete the dispatch arm `:65` and the sample-args
  arm `:289`.

### Task 3 — remove the executor arm and the ghost gate

`src/daemon/executor/mod.rs`: delete the `PendingCall::TmuxControl` arm
(`:1000`–~`:1055`, including the `kill_window_refusal` call at `:1049` and the
invalid-action error at `:1023`), the `ghost_may_use_tmux_control` function
(`:127`–`:139`), and `mod tmux_control_gate_tests` (`:1638`, 4 tests). Rewrite
the `ghost_may_send_input` doc comment per § Pre-flight. Remove the two stale
comment references at `:916` and `:942`.

### Task 4 — delete `knowledge/pane.rs`

Delete the file. In `src/daemon/executor/knowledge/mod.rs` remove `mod pane;`
(`:5`) and `pub(super) use pane::kill_window_refusal;` (`:20`).

**Do not touch `crate::daemon::is_daemon_window`** (`src/daemon/mod.rs:74`) —
see the must-survive table.

### Task 5 — retarget the `policy.rs` vehicle test

`src/agents/policy.rs`: retarget
`explicitly_allows_matches_only_the_allow_list` from `"tmux_control"` to
`"send_input"`, and update the `:36` doc comment so it names `send_input` (the
surviving gate) instead of `tmux_control`. **Do not delete the test** —
`explicitly_allows` survives and this is its only coverage.

### Task 6 — fix 09b's E2E kept-list

`src/ai/tools/dispatch.rs:164`: drop `"tmux_control"` from the `kept` array.

### Task 7 — prompt assets and doc tables

- `assets/prompts/sre.toml`: remove the 2 mentions.
- `CLAUDE.md`: delete the row; change the count line to
  **43 → 42 tools: 29 → 28 core + 14 deferred**.
- `README.md` § "AI tools": remove the row **and its `⚠` marker**, update the
  three numbers.
- Rename `tool_table_counts_are_43_29_14` → `tool_table_counts_are_42_28_14`,
  assertions 42 / 28 / 14.

`tests/doc_truth.rs` enforces the table, the counts **and** the
`APPROVAL_GATED_TOOLS`/`⚠` pairing — `readme_approval_markers_match_the_gated_tools`
is the test that fires if the marker is left behind.

### Task 8 — mutation check

Use the `patch` tool. Restore `"tmux_control"` to `APPROVAL_GATED_TOOLS` in
`src/ai/tools/defs.rs` **without** restoring the tool or its README `⚠`.

Run `cargo test --test doc_truth`. It **must** fail, and
`approval_gated_tools_all_exist` must be among the failures — the gate that
catches an approval entry naming a tool that is not in `TOOLS`. Revert and
confirm green. Paste both `test result:` lines into the Update Log.

This is the step-12 pairing that 09a and 09b never exercised.

### Task 9 — end-to-end capture

Add to `src/ai/tools/dispatch.rs`'s `mod tests`. `TOOLS` is `&[ToolDef]` but
`render_gemini` takes `&[&ToolDef]`, so the collect is required:

```rust
    #[test]
    fn e2e_tmux_control_is_absent_and_the_surface_is_final() {
        let names: Vec<&str> = TOOLS.iter().map(|t| t.name).collect();
        assert!(!names.contains(&"tmux_control"), "tmux_control must be gone");
        assert!(
            !APPROVAL_GATED_TOOLS.contains(&"tmux_control"),
            "and gone from the approval list"
        );
        for kept in ["run_terminal_command", "send_input", "get_terminal_context"] {
            assert!(names.contains(&kept), "{kept} must stay");
        }
        let rendered = render_gemini(&TOOLS.iter().collect::<Vec<_>>());
        let blob = serde_json::to_string(&rendered).expect("render must serialize");
        assert!(
            !blob.contains("tmux_control"),
            "tmux_control must not survive the render: {blob:.400}"
        );
        println!("PASTE MATCH >>>\ntools={} core={} gated={}\n<<<",
            TOOLS.len(),
            TOOLS.iter().filter(|t| t.deferred_group.is_none()).count(),
            APPROVAL_GATED_TOOLS.len());
    }
```

Run `cargo test --lib e2e_tmux_control_is_absent -- --nocapture` and copy the
text **between** `PASTE MATCH >>>` and `<<<` into the Update Log **byte for
byte**. It must read `tools=42 core=28 gated=10`.

## Acceptance criteria

Each was run against HEAD `16e4bcad` and **fails** there; "now" is the measured
current value.

Criteria are written against **identifiers** (`TmuxControl`,
`ghost_may_use_tmux_control`, `kill_window_refusal`) wherever possible, because
Tasks 8 and 9 necessarily put the *string* `tmux_control` into the test file —
09b had two criteria read 5 and 2 instead of 3 and 0 for exactly this reason.
The one string-based criterion below is anchored away from `dispatch.rs`.

- [ ] `grep -c '^        name: "' src/ai/tools/defs.rs` → **42** (now 43)
- [ ] `grep -c 'deferred_group: None' src/ai/tools/defs.rs` → **28** (now 29)
- [ ] `sed -n '/pub static APPROVAL_GATED_TOOLS/,/^];/p' src/ai/tools/defs.rs | grep -c '^    "'` → **10** (now 11)
- [ ] `grep -rc 'TmuxControl' src/ --include=*.rs | grep -v ':0' | wc -l` → **0** (now 6 files)
- [ ] `grep -c 'ghost_may_use_tmux_control' src/daemon/executor/mod.rs` → **0** (now 12)
- [ ] `grep -rn 'kill_window_refusal' src/ --include=*.rs | wc -l` → **0** (now 11)
- [ ] `test -f src/daemon/executor/knowledge/pane.rs` → **false**, the file is deleted (now exists)
- [ ] `grep -c 'mod pane' src/daemon/executor/knowledge/mod.rs` → **0** (now 1)
- [ ] `grep -rn 'tmux_control' src/ --include=*.rs | grep -v 'tools/dispatch.rs' | wc -l` → **0** (now 36) — anchored away from the test file per the note above
- [ ] `grep -c 'tmux_control' assets/prompts/sre.toml` → **0** (now 2)
- [ ] `cargo test --lib 'tmux_control_gate'` reports **0 passed** (now 4)
- [ ] `cargo test --lib 'knowledge::pane'` reports **0 passed** (now 3)
- [ ] `cargo test --lib 'agents::policy'` reports **15 passed** (now 15 — **unchanged**; a drop means the vehicle test was deleted instead of retargeted)
- [ ] `cargo test --lib 'ai::tools::dispatch'` reports **26 passed** (now 25)
- [ ] `cargo test --lib` reports **1877 passed** (now 1883) — `1883 − 4 − 3 + 1`
- [ ] `cargo test --test doc_truth` reports **10 passed**
- [ ] `grep -c 'tool_table_counts_are_42_28_14' src/ai/tools/dispatch.rs` → **1** (now 0) and `grep -c 'tool_table_counts_are_43_29_14' …` → **0** (now 1)
- [ ] Task 9's `PASTE MATCH` block reads exactly `tools=42 core=28 gated=10`
- [ ] Task 8's mutation pair, both `test result:` lines, in the Update Log
- [ ] `cargo fmt --all` clean, `cargo build` clean, `cargo clippy --all-targets --all-features -- -D warnings` clean

**Must-survive criteria** — these pass now and must still pass after:

- [ ] `grep -c 'pub fn is_daemon_window' src/daemon/mod.rs` → **1** — the real predicate
- [ ] `grep -c 'is_daemon_window' src/tmux/cache.rs` → **2** — the pane cache's callers
- [ ] `grep -c 'pub fn explicitly_allows' src/agents/policy.rs` → **1** — `send_input` still needs it (the bare `fn explicitly_allows` form reads 2: the definition plus the test's own name)
- [ ] `grep -c 'ghost_may_send_input' src/daemon/executor/mod.rs` → **8** — the surviving twin gate, untouched in count
- [ ] `grep -c 'name: "send_input"' src/ai/tools/defs.rs` → **1**
- [ ] `git diff --stat 16e4bcad..HEAD -- src/tmux/ src/daemon/mod.rs src/daemon/server/ src/daemon/background/` is **empty**

## Test plan

**Removed (7):** the 4 in `mod tmux_control_gate_tests`; the 3 in
`knowledge/pane.rs`, which goes with the file.

**Retargeted (1, count unchanged):** `agents/policy.rs`'s
`explicitly_allows_matches_only_the_allow_list`, vehicle `"tmux_control"` →
`"send_input"`. Pinned by `agents::policy` staying at 15.

**Edited (1):** 09b's `e2e_removed_pane_watch_tools_are_absent_from_every_render`
drops `"tmux_control"` from its kept list.

**Added (1):** `e2e_tmux_control_is_absent_and_the_surface_is_final` (Task 9) —
pins the tool absent from `TOOLS`, from `APPROVAL_GATED_TOOLS` and from the
Gemini render, three surviving tools present, and prints the milestone's final
`tools=42 core=28 gated=10`.

**Renamed (1):** the count guard → `tool_table_counts_are_42_28_14`.

Net `cargo test --lib`: `1883 − 7 + 1 = 1877`.

## End-to-end verification

The Task 9 `PASTE MATCH` block (`tools=42 core=28 gated=10`) and the Task 8
mutation pair, in their own Update Log entry titled
`### Update — <date> (end-to-end verification)`. Per `STANDARDS.md` §1 the
server-authored `(complete)` entry does **not** satisfy this.

## Authorizations

Edits are authorized in exactly these files:

- `src/ai/types/pending.rs`, `src/ai/types/events.rs`
- `src/ai/tools/defs.rs`, `src/ai/tools/dispatch.rs`, `src/ai/tools/args.rs`
- `src/daemon/stream.rs`, `src/daemon/executor/mod.rs`
- `src/daemon/executor/knowledge/pane.rs` (**deleted**),
  `src/daemon/executor/knowledge/mod.rs`
- `src/agents/policy.rs` — the `:36` doc comment and the vehicle test
- `assets/prompts/sre.toml`, `CLAUDE.md`, `README.md`
- This phase doc's Update Log.

**Not authorized:** `src/tmux/`, `src/daemon/mod.rs`, `src/daemon/server/`,
`src/daemon/background/`, `src/ipc.rs`, `src/cli/`, `tests/doc_truth.rs`,
`src/config/seeds.rs`, and everything under `assets/` except `sre.toml`.

The orphan closure in § Pre-flight covers functions, wrappers and `use`
statements. If the lint gate still reports a `dead_code` or `unused_imports`
orphan this missed, **do not** mask it with `#[allow]`: name it in the Update
Log, delete it if it is in an authorized file, and file a blocker entry if it
is not.

## Out of scope

- `target_pane` / `retry_in_pane` — **09d**, 185 refs including the IPC wire.
- The seeded assets that still name the removed tools
  (`assets/prompts/ghost-shell.txt`, the tmux knowledge memory,
  `assets/agents/sysadmin/config.toml`) and `docs/REQUIREMENTS.md` — **phase
  10**, whose row was widened to cover them. `tmux_control` joins that backlog;
  do not chase it here.
- The pane **context blocks** and `src/tmux/cache.rs`.
- Renaming `get_terminal_context` — **08b**.
- The tmux substrate itself: `crate::tmux::*` keeps every function. This phase
  removes the AI's *tool*, not the daemon's ability to drive tmux.

## Update Log

<!-- entries appended below this line -->

# Phase 09d: retire `target_pane` and `retry_in_pane` from `run_terminal_command`

**Milestone:** M22 — Tool surface & context
**Status:** todo
**Depends on:** 07a (`shell?`), 01 (`backend = "pty"` default), 09a–09c (the six
pane tools). All **done**.
**Estimated diff:** ~450 lines
**Tags:** language=rust, kind=removal, size=m

## Goal

The AI stops naming tmux panes when it runs a command. Two parameters go from
`run_terminal_command`:

- **`target_pane`** — the foreground pane the command is typed into.
- **`retry_in_pane`** — reuse a finished background window for a retry.

**No tool count changes** (42 tools: 28 core + 14 deferred, unchanged), but this
phase closes the milestone's *other* headline exit criterion:

> **Zero `%`-pane identifiers in any prompt asset or tool description.**

Measured: `src/ai/tools/defs.rs` now contains **exactly one** `%N` identifier,
at `:80`, inside `run_terminal_command`'s `target_pane` description. This phase
takes it to zero. (The prompt-asset half — `sre.toml`'s 2 — is phase 10's.)

### ⚠ What this phase does NOT retire, and why

`target_pane` is declared on **three** tools, not one:

| Tool | `defs.rs` line | Fate |
|---|---|---|
| `run_terminal_command` | `:77` | **retired here** |
| `read_file` | `:321` | **stays — deferred to M21** |
| `edit_file` | `:390` | **stays — deferred to M21** |

On `read_file` and `edit_file`, `target_pane` is how a file is read or edited
**on a remote host** — see `src/daemon/executor/file_ops/ops.rs:34`, whose own
comment marks the branch `// ── Remote path ──`. Retiring it there would delete
remote file operations outright, and their successor (`host?`) is **M21's**, not
this milestone's. Neither description contains a `%N`, so leaving them does not
block the exit criterion.

**Do not touch the `read_file` / `edit_file` `target_pane` parameters, their
`ReadFileArgs` / `EditFileArgs` fields, their `PendingCall` / `AiEvent` fields,
or anything under `src/daemon/executor/file_ops/`.** A criterion below pins them.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.5 — the changed `run_terminal_command`
  signature.
- The milestone README's exit criteria — the `%`-pane-identifier one.

## Pre-flight

Measured against HEAD `2b94ed63` on 2026-09-17.

### The orphan closure — walked OUTWARD from every deleted line

Three phases in this row (09a, 09b, 09c) each shipped a scope deviation because
the Authorizations named the *call sites* and omitted the *callees*. The
milestone README records that at three occurrences. This closure is therefore
walked **outward**: for every line this phase deletes, what loses its last
caller?

**`target_pane` is NOT orphaned by this phase.** It survives on `read_file` /
`edit_file`, so `Option<&str>` plumbing in `file_ops` keeps its callers, and so
do these:

| Symbol | Why it survives |
|---|---|
| `find_best_target_pane` (`executor/`) | `foreground.rs:1` still imports it; the *fallback* resolver (below) |
| `SessionEntry.default_target_pane` | read at `foreground.rs:160`, `:204`, `prompt.rs:213`, `:110`, `:260` |
| `prepend_foreground_target` (`prompt.rs`) | the `[FOREGROUND TARGET]` block, unchanged |
| `run_foreground` (`foreground.rs`) | **still reached** — ghost sessions and `backend = "tmux"` both use it |
| `tmux::select_pane` | `foreground.rs`'s highlight calls |

**`retry_in_pane` IS fully orphaned.** 14 references across 8 files:

```
2 src/ai/tools/args.rs      3 src/ai/types/pending.rs    2 src/daemon/executor/foreground.rs
1 src/ai/tools/defs.rs      2 src/ai/types/events.rs     1 src/daemon/executor/mod.rs
                                                          1 src/daemon/executor/bg_shell.rs
                                                          2 src/daemon/ghost.rs
```

Its substrate is `respawn_background_in_pane`
(`src/daemon/background/respawn.rs`). **Check whether that function still has a
caller after your edits** — if `retry_in_pane` was its only path, it is an
orphan and `-D warnings` will say so. See § Authorizations for what to do then;
do not guess and do not `#[allow]` it.

### ⚠ The load-bearing mechanism: `target_pane` is a live escape hatch

This is not a vestigial parameter. `src/daemon/executor/mod.rs:308-320`:

```rust
            // The PTY shell substrate, the default. A ghost session, an
            // explicit `target_pane`, or `backend = "tmux"` (the opt-out)
            // falls through to `run_foreground` unchanged, so the opt-out
            // behaviour is byte-for-byte today's.
            let uses_pty = crate::config::Config::load()
                .unwrap_or_default()
                .execution
                .uses_pty();
            if uses_pty
                && !is_ghost
                && session_id.is_some()
                && target.as_deref().is_none_or(|t| t.is_empty())
            {
```

An explicit `target_pane` **deliberately routes around the PTY substrate** into
`run_foreground`. Removing the parameter therefore removes that per-call
opt-out: with `backend = "pty"` (the default since phase-01) a non-ghost
`run_terminal_command` always takes the PTY path.

**That is the intended behaviour change, and it is the point of the phase** —
but it must be done by deleting the `target.as_deref().is_none_or(…)` conjunct,
**not** by deleting the whole condition. `uses_pty`, `!is_ghost` and
`session_id.is_some()` all still decide correctly: `backend = "tmux"` and ghost
sessions must keep falling through to `run_foreground`.

### The fallback already exists — nothing needs inventing

`run_foreground` does **not** depend on the AI supplying a pane.
`src/daemon/executor/foreground.rs:157-165` already resolves the session's
default when the AI's value is absent or stale:

```rust
                        store.get(sid)?.default_target_pane.clone()
```

and `:190` carries the stale-pane guard:

```rust
    // C3b: stale-pane guard — if the AI specified a target_pane that is no longer
```

So after this phase, foreground execution (ghost / `backend = "tmux"`) resolves
its pane from `SessionEntry.default_target_pane` or `find_best_target_pane`.
**Keep both paths.** What changes is only that the *AI* no longer names a pane.

Two error strings at `foreground.rs:170` and `:214` tell the model to
"Pass target_pane=… or omit it to use the default." Those instruct the model to
use a parameter that will not exist — rewrite them to say the daemon resolves
the pane itself.

### The IPC wire — measured, and mostly NOT this phase's

`src/ipc.rs` carries **3** `target_pane` references and `src/ipc_tests.rs` ten.

```sh
$ grep -n 'target_pane' src/ipc.rs
197:        target_pane: Option<String>,     ← Response::ToolCallPrompt, the approval-panel hint
330:  (doc comment on the default-target-pane request)
447:        target_pane: Option<String>,     ← the other prompt frame
```

`:197` and `:447` are the **approval panel's pane hint**, computed by the daemon
from the cache (`CLAUDE.md` § Request/Response lifecycle: "computed
synchronously from the cache before the approval wait"). They are **not** the
tool parameter and are **not** fed by it. **Leave `src/ipc.rs`,
`src/ipc_tests.rs` and `src/cli/` alone** — the panel still wants to tell the
user which pane a foreground command will touch. A criterion pins this.

### Where the tool parameter actually lives

```sh
$ grep -rn 'target_pane' src/ --include=*.rs | wc -l     # 168 total — most are NOT this phase's
```

The subset this phase touches is the `run_terminal_command` path only:

- `src/ai/tools/defs.rs:77` (the `ParamDef`) and `:87` (`retry_in_pane`)
- `src/ai/tools/args.rs` — the run/background arg structs' fields
- `src/ai/types/pending.rs` — the run/background variants' fields
- `src/ai/types/events.rs` — the same on `AiEvent`
- `src/daemon/stream.rs` — the arms that thread them through
- `src/daemon/executor/mod.rs` — the routing conjunct and the call into
  `run_foreground`
- `src/daemon/executor/foreground.rs` — the parameter it receives (it keeps its
  *internal* resolution)
- `src/daemon/ghost.rs`, `src/daemon/executor/bg_shell.rs` — `retry_in_pane`
  call sites

**Batch every enum-field removal before running a gate.** 09c's first run was
killed by the governor (`VerifierFailurePersistent`) for a legitimately
mid-batch red tree. Removing a struct field breaks every constructor at once;
finish the batch, then build. The compiler's error list is the checklist.

## Current state

- HEAD `2b94ed63`, tree clean.
- `cargo test --lib` → **1877 passed**, 4 ignored.
- `cargo test --test doc_truth` → **10 passed**.
- **42 tools: 28 core + 14 deferred** — unchanged by this phase.
- `defs.rs` `name: "target_pane"` → **3**; `name: "retry_in_pane"` → **1**.
- `defs.rs` `%N` identifiers → **1** (at `:80`).
- `retry_in_pane` across `src/` → **14**.

## Spec

### Task 1 — remove the two `ParamDef`s

`src/ai/tools/defs.rs`: delete the `target_pane` (`:77`) and `retry_in_pane`
(`:87`) entries from `run_terminal_command`'s `params`. **Leave the
`target_pane` entries at `:321` (`read_file`) and `:390` (`edit_file`)
untouched.** Deleting `:77` is what takes the `%N` count to zero.

### Task 2 — remove the fields from the arg structs, `PendingCall` and `AiEvent`

`src/ai/tools/args.rs`, `src/ai/types/pending.rs`, `src/ai/types/events.rs`:
remove `target_pane` and `retry_in_pane` from the **run / background** shapes
only. `ReadFileArgs`, `EditFileArgs` and their `PendingCall` / `AiEvent`
counterparts keep theirs.

Do the whole batch before building.

### Task 3 — thread them out of `stream.rs` and the executor

`src/daemon/stream.rs`: drop them from the arms that construct the run and
background `PendingCall`s.

`src/daemon/executor/mod.rs` has **two** escape-hatch conjuncts, one per
execution mode — both go, and nothing else in either condition does:

```rust
319:                && target.as_deref().is_none_or(|t| t.is_empty())        // foreground
445:                && retry_pane.as_deref().is_none_or(|p| p.is_empty())    // background
```

Delete **only** those two conjuncts. Keep `uses_pty`, `!is_ghost` and
`session_id.is_some()` in both conditions — `backend = "tmux"` and ghost
sessions must still fall through to the tmux path. Update the comments above
them so neither claims an explicit pane parameter falls through.

They are the only two uses of `is_none_or` in the file, which is what the
criterion below greps for.

### Task 4 — `foreground.rs` keeps resolving, stops being told

`src/daemon/executor/foreground.rs`: stop accepting the AI-supplied pane; keep
the `default_target_pane` / `find_best_target_pane` resolution and the
stale-pane guard. Rewrite the two model-facing strings at `:170` and `:214` so
they no longer tell the model to pass `target_pane`.

### Task 5 — `retry_in_pane`'s call sites

`src/daemon/ghost.rs`, `src/daemon/executor/bg_shell.rs`,
`src/daemon/executor/foreground.rs`: remove the `retry_in_pane` arguments.
Then check `respawn_background_in_pane`
(`src/daemon/background/respawn.rs`) for a surviving caller — if it has none,
follow § Authorizations' orphan rule rather than guessing.

### Task 6 — prompt assets and doc tables

- `assets/prompts/sre.toml`: remove the `retry_in_pane` line and any sentence
  telling the model to pass a pane to `run_terminal_command`. **Do not** attempt
  the wider tmux cleanup — 31 pane/tmux references remain and they are phase
  10's.
- `CLAUDE.md`: update `run_terminal_command`'s row so it no longer documents the
  two parameters. **The tool counts do not change** — do not touch the
  `42 tools: 28 core + 14 deferred` line.
- `README.md` § "AI tools": same, and leave the three numbers alone.

### Task 7 — mutation check

Use the `patch` tool. Re-add a `target_pane` `ParamDef` to
`run_terminal_command`'s `params` in `src/ai/tools/defs.rs`, with the original
description containing `\"%3\"`.

Run the new `e2e_no_pane_identifiers_in_tool_descriptions` test from Task 8. It
**must** fail, naming the reintroduced `%3`. Revert and confirm green. Paste
both `test result:` lines into the Update Log.

This proves the exit-criterion test is bound to the real table and not to a
constant.

### Task 8 — end-to-end capture

Add to `src/ai/tools/dispatch.rs`'s `mod tests` (it already imports `TOOLS`):

```rust
    #[test]
    fn e2e_no_pane_identifiers_in_tool_descriptions() {
        // The milestone's exit criterion: no %-pane identifier anywhere in the
        // tool surface the model is shown.
        let mut offenders: Vec<String> = Vec::new();
        for t in TOOLS {
            if t.description.contains('%') {
                offenders.push(format!("{} (description)", t.name));
            }
            for p in t.params {
                if p.description.contains('%') {
                    offenders.push(format!("{}.{}", t.name, p.name));
                }
            }
        }
        assert!(offenders.is_empty(), "%-pane identifiers remain: {offenders:?}");

        let run = TOOLS.iter().find(|t| t.name == "run_terminal_command")
            .expect("run_terminal_command must exist");
        let names: Vec<&str> = run.params.iter().map(|p| p.name).collect();
        assert!(!names.contains(&"target_pane"), "retired: {names:?}");
        assert!(!names.contains(&"retry_in_pane"), "retired: {names:?}");
        for kept in ["command", "wait", "shell", "timeout_secs"] {
            assert!(names.contains(&kept), "{kept} must stay: {names:?}");
        }
        // read_file / edit_file keep theirs until M21's host? lands.
        for still in ["read_file", "edit_file"] {
            let t = TOOLS.iter().find(|t| t.name == still).expect("must exist");
            assert!(
                t.params.iter().any(|p| p.name == "target_pane"),
                "{still} keeps target_pane until M21"
            );
        }
        println!("PASTE MATCH >>>\npane_ids={} run_params={}\n<<<",
            offenders.len(), run.params.len());
    }
```

Run `cargo test --lib e2e_no_pane_identifiers -- --nocapture` and copy the text
**between** `PASTE MATCH >>>` and `<<<` **byte for byte** into the Update Log.
It must read `pane_ids=0 run_params=4`.

Note the last block: it pins the **negative** of the deferral, so a future phase
that removes `read_file`'s `target_pane` early trips this test rather than
silently deleting remote file ops.

## Acceptance criteria

Each was run against HEAD `2b94ed63` and **fails** there; "now" is the measured
current value. Criteria prefer **identifiers** over tool-name strings because
Task 8's test necessarily contains the literal `target_pane`.

- [ ] `grep -o '%[0-9]' src/ai/tools/defs.rs | wc -l` → **0** (now 1) — **the milestone exit criterion**
- [ ] `grep -c 'name: "target_pane"' src/ai/tools/defs.rs` → **2** (now 3) — `read_file` + `edit_file` only
- [ ] `grep -c 'name: "retry_in_pane"' src/ai/tools/defs.rs` → **0** (now 1)
- [ ] `grep -rn 'retry_in_pane' src/ --include=*.rs | wc -l` → **0** (now 14)
- [ ] `grep -rc 'RetryInPane' src/ --include=*.rs | grep -v ':0' | wc -l` → **0** (now 0 — there is no such identifier; the field is snake_case only, so the line above is the real guard)
- [ ] `grep -c 'retry_in_pane' assets/prompts/sre.toml` → **0** (now 1)
- [ ] `cargo test --lib 'ai::tools::dispatch'` reports **27 passed** (now 26)
- [ ] `cargo test --test doc_truth` reports **10 passed**
- [ ] `grep -c '^        name: "' src/ai/tools/defs.rs` → **42, unchanged**, and `grep -c 'deferred_group: None' …` → **28, unchanged** — a change here means a tool was added or removed, which this phase must not do
- [ ] `grep -c 'is_none_or' src/daemon/executor/mod.rs` → **0** (now 2) — both escape-hatch conjuncts gone; they are its only two uses
- [ ] Task 8's `PASTE MATCH` block reads exactly `pane_ids=0 run_params=4`
- [ ] Task 7's mutation pair, both `test result:` lines, in the Update Log
- [ ] `cargo fmt --all` clean, `cargo build` clean, `cargo clippy --all-targets --all-features -- -D warnings` clean

**Must-survive criteria** — these pass now and must still pass after:

- [ ] `grep -c 'target_pane' src/daemon/executor/file_ops/ops.rs` → **9, unchanged** — the remote file path, deferred to M21
- [ ] `git diff --stat 2b94ed63..HEAD -- src/daemon/executor/file_ops/` is **empty**
- [ ] `git diff --stat 2b94ed63..HEAD -- src/ipc.rs src/ipc_tests.rs src/cli/` is **empty** — the approval panel's pane hint is daemon-computed, not the tool parameter
- [ ] `grep -c 'default_target_pane' src/daemon/executor/foreground.rs` → **at least 1** (now 3) — the fallback resolution stays. Deliberately not an exact count: this phase edits the file, so the property is that the fallback survives, not that its call count is frozen.
- [ ] `grep -c 'find_best_target_pane' src/daemon/executor/foreground.rs` → **at least 1** (now 2), same reasoning
- [ ] `grep -c 'fn prepend_foreground_target' src/daemon/prompt.rs` → **1**
- [ ] `grep -c 'uses_pty' src/daemon/executor/mod.rs` → **at least 1** (now 6) — the routing still consults the backend

## Test plan

**Added (1):** `e2e_no_pane_identifiers_in_tool_descriptions` (Task 8) — scans
every `ToolDef` description **and** every `ParamDef` description for `%`, pins
`run_terminal_command`'s surviving four parameters, and pins the **negative** of
the M21 deferral (`read_file` / `edit_file` keep `target_pane`). Mutation-checked
in Task 7.

**Expected removals:** any test that constructs a run/background `PendingCall`
with `target_pane` or `retry_in_pane` must lose those fields — that is a **field
edit, not a test deletion**. If you find yourself deleting a test, stop and say
so in the Update Log: the behaviour it covered has not gone away.

`ai::tools::dispatch` goes 26 → **27**. `cargo test --lib` is **not** pinned to a
single number here, because the field edits may consolidate assertions; pin it in
the Update Log by stating the number you land on and confirming no test *name*
disappeared. Report both.

## End-to-end verification

The Task 8 `PASTE MATCH` block (`pane_ids=0 run_params=4`) and the Task 7
mutation pair, in their own Update Log entry titled
`### Update — <date> (end-to-end verification)`. Per `STANDARDS.md` §1 the
server-authored `(complete)` entry does **not** satisfy this.

## Authorizations

Edits are authorized in exactly these files:

- `src/ai/tools/defs.rs` — `run_terminal_command`'s params **only**
- `src/ai/tools/args.rs`, `src/ai/types/pending.rs`, `src/ai/types/events.rs` —
  the run/background shapes **only**
- `src/daemon/stream.rs`, `src/daemon/executor/mod.rs`
- `src/daemon/executor/foreground.rs`, `src/daemon/executor/bg_shell.rs`
- `src/daemon/ghost.rs` — the `retry_in_pane` call sites only
- `src/daemon/background/respawn.rs` — **only** if Task 5's orphan check shows
  `respawn_background_in_pane` has no surviving caller
- `assets/prompts/sre.toml`, `CLAUDE.md`, `README.md`
- This phase doc's Update Log.

**Not authorized:** `src/daemon/executor/file_ops/`, `src/ipc.rs`,
`src/ipc_tests.rs`, `src/cli/`, `src/tmux/`, `src/daemon/server/`,
`src/daemon/prompt.rs`, `src/daemon/session.rs`, `tests/doc_truth.rs`,
`src/config/seeds.rs`, and everything under `assets/` except `sre.toml`.

**If the lint gate reports a `dead_code` or `unused_imports` orphan this closure
missed:** do **not** mask it with `#[allow]`, and do **not** attribute a
decision to anyone. Name it in the Update Log, then —

- if it lives in an **authorized** file, delete it and record that you did so;
- if it lives in an **unauthorized** file, you have two legitimate options:
  either stop and return with the blocker entry, **or** make the deletion and
  label it plainly as **your own** unauthorized-but-forced decision with the
  evidence (zero callers, the exact lint error). **State which you chose and
  that it was your decision** — three phases in this row hit this, and one
  recorded a "principal engineer authorised it" resolution that never happened.
  There is no human in the loop mid-run; do not write one.

## Out of scope

- **`target_pane` on `read_file` / `edit_file`** — the remote file path. Waits
  for **M21**'s `host?`. Pinned by a must-survive criterion.
- The approval panel's `target_pane` hint on `Response::ToolCallPrompt` — daemon
  computed, stays.
- `run_foreground` itself, and the `backend = "tmux"` opt-out. Ghost sessions and
  the opt-out still execute in panes; this phase stops the **AI** from choosing
  which.
- `sre.toml`'s remaining 31 pane/tmux references and the seeded assets — **phase
  10**.
- Renaming `get_terminal_context` — **08b**.

## Update Log

<!-- entries appended below this line -->

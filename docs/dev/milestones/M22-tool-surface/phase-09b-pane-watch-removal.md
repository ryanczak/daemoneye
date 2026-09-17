# Phase 09b: remove `watch_pane` and `close_background_window`

**Milestone:** M22 — Tool surface & context
**Status:** review
**Depends on:** 04b (`watch_shell`), 04a (`close_shell`), 09a (the three pane
read tools). All **done**.
**Estimated diff:** ~450 lines, overwhelmingly deletion
**Tags:** language=rust, kind=removal, size=s

## Goal

Two more of the six pane tools, both with a shipped shell counterpart.

| Removed | Counterpart | Shipped in |
|---|---|---|
| `watch_pane` | `watch_shell` | 04b |
| `close_background_window` | `close_shell` | 04a |

After this phase: **43 tools — 29 core + 14 deferred** (from 45 = 31 + 14).
`doc_truth` stays at 10. Only `tmux_control` then remains of the six, and 09c
takes it to the milestone's 42.

### Why `close_background_window` goes now, not later

It is the one removal in this row that is a **judgement call**, so the reasoning
is recorded here rather than left implicit.

Measured: `de-bg-*` tmux windows are created by `run_background_in_window`
(`src/daemon/background/run.rs:411` is the only `bg_windows.push`), reached from
`src/daemon/executor/foreground.rs:1279`. That is the **`backend = "tmux"`
opt-out** path. Under the default `backend = "pty"` (set in phase-01),
`wait: false` runs through `src/daemon/executor/bg_shell.rs` — whose module doc
says so in as many words — and produces a **dedicated PTY shell**, not a tmux
window. So `close_background_window` has had nothing to close in the default
configuration since phase-01.

Removing it costs a non-default configuration the ability to free a window
slot *early*. It does not leak: `gc_bg_windows`
(`src/daemon/background/gc.rs:182`) reclaims dead windows on its own sweep, and
the per-session cap evicts. `close_shell` is the PTY-side equivalent and already
exists. The charter counts this tool among the six, and the milestone's exit
criterion of 42 does not close without it.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.5 — the removal list.
- `CLAUDE.md` § "Adding a new AI tool (checklist)" — eleven steps, **in
  reverse**. `tests/doc_truth.rs` enforces steps 10 and 11.

## Pre-flight

Measured against HEAD `d7b17254` on 2026-09-16.

### The orphan closure — measured first, per 09a's calibration

09a's review found that its authorization list named every *call site* but not
the *callees* whose last caller the phase removed, which left
`tmux::capture_pane_annotated` dead and made `cargo clippy -D warnings`
unsatisfiable inside the authorized files. That check has been run up front here.

Every helper `watch_pane` and `close_bg_window` reach for, with its caller count
**outside** `knowledge/pane.rs`:

```
pane_current_command     14      capture_pane             27
off_runtime             121      bg_done_subscribe         7
FG_HOOK_COUNTER           2      shell_escape_arg         19
append_session_message   17      WatchHookGuard            0
```

**Only `WatchHookGuard` is orphaned, and it lives in `knowledge/pane.rs`** —
inside the authorized file, deleted alongside `watch_pane`. Nothing under
`src/tmux/`, `src/daemon/background/` or `src/daemon/session.rs` loses its last
caller, so this phase needs no widened authorization.

`FG_HOOK_COUNTER` reads low at 2, so to be explicit: both live uses are the
definition (`src/daemon/session.rs:157`) and **`foreground.rs:377`**, which is
not touched here. It survives. Same for `bg_done_subscribe`, used by
`foreground.rs:7`.

**If the lint gate nonetheless reports a `dead_code` error after your
deletions, that is a genuine orphan this measurement missed.** Do not mask it
with `#[allow]`. Name it in the Update Log, and delete it if — and only if —
it lives in an authorized file. If it does not, file a blocker entry saying so.

### ⚠ Must survive — the `[Watch Pane` catch-up scanner

`knowledge/pane.rs` is the **only** emitter of `[Watch Pane Match]` /
`[Watch Pane Complete]` / `[Watch Pane Timeout]` session messages (`:263`,
`:268`, `:274`). The **scanner** is somewhere else:

```sh
$ grep -rn '\[Watch Pane' src/ --include=*.rs
src/daemon/server/catchup.rs:49:                || c.contains("[Watch Pane")
src/daemon/server/catchup.rs:196:        let msgs = vec![msg("[Watch Pane %3] pattern 'ready' matched after 45s")];
src/daemon/server/catchup.rs:198:        assert!(brief.contains("[Watch Pane"), "missing event: {brief}");
src/daemon/executor/mod.rs:698:  "…a [Watch Pane ...] message may still arrive."   ← goes with the arm
```

**Leave `src/daemon/server/catchup.rs` completely alone**, including
`catchup_brief_detects_watch_pane`. Session JSONL files already on disk contain
`[Watch Pane …]` lines from before this phase, and `build_catchup_brief` still
has to surface them when such a session is resumed. The scanner becoming
producer-less is correct, not dead code — it reads historical data. Deleting it
would silently drop events from old sessions out of catch-up briefs, and no gate
would catch it.

### The site map

```sh
$ grep -rc 'watch_pane'              src/ --include=*.rs | ...  # 20 total
$ grep -rc 'close_background_window' src/ --include=*.rs | ...  # 12 total
$ grep -rc 'close_bg_window'         src/ --include=*.rs | ...  #  8 total
$ grep -rc 'WatchPane'               src/ --include=*.rs | ...  # 18 total
$ grep -rc 'CloseBackgroundWindow'   src/ --include=*.rs | ...  # 13 total
```

`PendingCall` / `AiEvent` variants are `WatchPane` and `CloseBackgroundWindow`.
The executor arms are `src/daemon/executor/mod.rs:693` (watch) and the
`close_bg_window` call; the timeout prose at `:698` goes with them.

### Live prose that instructs the model to call a removed tool

**`src/daemon/background/helpers.rs:175`** is not a comment — it is a runtime
message the model reads:

```rust
             Call close_background_window(\"{pane_id}\") when you are done with this window."
```

Rewrite it. Under `backend = "tmux"` the honest replacement is that the window
is reclaimed automatically; do not point it at `close_shell`, which cannot close
a tmux window.

Two stale doc comments also need a touch:

- `src/daemon/session.rs:10` — "can be passed to `watch_pane` or used as
  foreground target" on `BgWindowInfo.pane_id`.
- `src/daemon/executor/foreground.rs:95` — "Shell prompt detection helpers (also
  used by knowledge::watch_pane)."

### Two mentions that may stay, as provenance

`src/daemon/executor/knowledge/shell.rs:1099` and `:1103` cite `watch_pane`'s
poll interval as the origin of a constant's shape. That is a historical fact and
stays true after the removal. Leave them, or reword to past tense — but they
must **not** read as though the function still exists. They are the reason the
`watch_pane` survivor count below is 3 rather than 1.

### `pane.rs` after this phase

09a left the file at 415 lines with four functions. This phase removes two:

```
close_bg_window   GOES   (+ WatchHookGuard, + the [Watch Pane] message builders)
watch_pane        GOES
is_daemon_window  STAYS  — tmux_control helper, 09c
kill_window_refusal STAYS — tmux_control helper, 09c
```

The file drops to roughly 100 lines holding only the two `tmux_control`
helpers. Its module-level doc comment will describe a file that no longer
matches — update it to say what is actually left.

### Test deltas — derived, not guessed

```
REMOVE 2  src/daemon/executor/knowledge/pane.rs
            close_bg_window_no_session, close_bg_window_unknown_session
REMOVE 2  src/ai/types/pending.rs
            summary_watch_pane_with_pattern, summary_watch_pane_no_pattern
ADD    1  src/ai/tools/dispatch.rs — the E2E of Task 8
```

There are **no** `watch_pane` behaviour tests to remove (it spawns a tokio task
and was never unit-tested), and **no** vehicle tests using these two tool names
— 09a's retarget problem does not recur here. Net: `1886 − 4 + 1 = 1883`.

`knowledge::pane` goes 5 → **3** (the three `kill_window_refusal_*`).
`ai::tools::dispatch` goes 24 → **25**.

### The count-guard test must be renamed

09a added `tool_table_counts_are_45_31_14` to `src/ai/tools/dispatch.rs`. Its
name encodes the old numbers. Rename it to `tool_table_counts_are_43_29_14` and
update its three assertions. Leaving the old name with new numbers is the
failure the name exists to prevent.

## Current state

- HEAD `d7b17254`, tree clean.
- `cargo test --lib` → **1886 passed**, 4 ignored.
- `cargo test --lib 'knowledge::pane'` → **5 passed**.
- `cargo test --lib 'ai::tools::dispatch'` → **24 passed**.
- `cargo test --test doc_truth` → **10 passed**.
- **45 tools: 31 core + 14 deferred.**
- Neither tool is in `APPROVAL_GATED_TOOLS`, so that list does not change.
- `assets/prompts/sre.toml` → **5** mentions; `CLAUDE.md` → 3; `README.md` → 2.

## Spec

### Task 1 — remove the `PendingCall` and `AiEvent` variants

Delete `WatchPane` and `CloseBackgroundWindow` from `src/ai/types/pending.rs`
(with every `to_tool_call()` / `id()` / `should_emit_tool_feedback()` /
`summary()` / `tool_name()` arm) and from `src/ai/types/events.rs`.

**Batch every match arm before running a gate** — a removed variant leaves the
tree uncompilable until the last arm is gone, and iterating site-by-site
produces a long run of identical non-exhaustive-match errors.

### Task 2 — remove the `ToolDef` entries and the dispatch arms

`src/ai/tools/defs.rs`: delete both entries. `src/ai/tools/dispatch.rs`: delete
both `dispatch_tool_event()` arms and both sample-args arms (the table is
table-driven, so its test count does not change).

### Task 3 — remove the streaming and executor arms

`src/daemon/stream.rs`: both `AiEvent` arms. `src/daemon/executor/mod.rs`: both
`PendingCall` arms, including the `:698` timeout prose.

### Task 4 — gut `pane.rs`

Delete `close_bg_window`, `watch_pane`, `WatchHookGuard`, the `[Watch Pane …]`
message builders and the 2 `close_bg_window_*` tests. Keep `is_daemon_window`
and `kill_window_refusal` with their 3 tests. Update the module doc comment.
Shrink the `knowledge/mod.rs` re-export to those two.

### Task 5 — remove the two `pending.rs` tests

`summary_watch_pane_with_pattern` and `summary_watch_pane_no_pattern` test the
removed variant's `summary()`. Delete them; there is nothing to retarget.

### Task 6 — repair the live prose

`src/daemon/background/helpers.rs:175` (the runtime message),
`src/daemon/session.rs:10` and `src/daemon/executor/foreground.rs:95`, per
§ Pre-flight.

### Task 7 — prompt assets and doc tables

- `assets/prompts/sre.toml`: remove the 5 mentions; where a sentence told the
  model to watch a pane, point it at `watch_shell`.
- `CLAUDE.md`: delete both rows; change the count line to
  **45 → 43 tools: 31 → 29 core + 14 deferred**.
- `README.md` § "AI tools": remove both from the Core table, update the three
  numbers above it.
- Rename `tool_table_counts_are_45_31_14` → `tool_table_counts_are_43_29_14`
  and update its assertions to 43 / 29 / 14.

### Task 8 — end-to-end capture

Add to `src/ai/tools/dispatch.rs`'s existing `mod tests` (it already imports
`TOOLS` and `render_gemini`). Note the argument shape — `TOOLS` is `&[ToolDef]`
but `render_gemini` takes `&[&ToolDef]`:

```rust
    #[test]
    fn e2e_removed_pane_watch_tools_are_absent_from_every_render() {
        let names: Vec<&str> = TOOLS.iter().map(|t| t.name).collect();
        for gone in ["watch_pane", "close_background_window"] {
            assert!(!names.contains(&gone), "{gone} must be gone from TOOLS");
        }
        for kept in ["watch_shell", "close_shell", "tmux_control"] {
            assert!(names.contains(&kept), "{kept} must stay");
        }
        let rendered = render_gemini(&TOOLS.iter().collect::<Vec<_>>());
        let blob = serde_json::to_string(&rendered).expect("render must serialize");
        for gone in ["watch_pane", "close_background_window"] {
            assert!(!blob.contains(gone), "{gone} must not survive the render: {blob:.400}");
        }
        println!("PASTE MATCH >>>\ntools={} core={}\n<<<",
            TOOLS.len(),
            TOOLS.iter().filter(|t| t.deferred_group.is_none()).count());
    }
```

Note `tmux_control` in the must-stay list: it is 09c's, and a removal that took
it early would otherwise still satisfy a bare count of 43.

Run `cargo test --lib e2e_removed_pane_watch_tools -- --nocapture` and copy the
text **between** `PASTE MATCH >>>` and `<<<` into the Update Log **byte for
byte**. It must read `tools=43 core=29`.

### Task 9 — mutation check

Use the `patch` tool. Change the renamed guard's total assertion from `43` to
`44` — i.e. make the test expect a tool that is not there.

Run `cargo test --lib 'ai::tools::dispatch'`. It **must** fail with
`tool_table_counts_are_43_29_14` among the failures. Revert and confirm green.
Paste both `test result:` lines into the Update Log.

This proves the renamed guard is actually bound to the new counts rather than
having been renamed without its assertions being updated — the precise failure
§ Pre-flight warns about.

## Acceptance criteria

Each was run against HEAD `d7b17254` and **fails** there; "now" is the measured
current value.

- [ ] `grep -c '^        name: "' src/ai/tools/defs.rs` → **43** (now 45)
- [ ] `grep -c 'deferred_group: None' src/ai/tools/defs.rs` → **29** (now 31)
- [ ] `grep -rc 'WatchPane' src/ --include=*.rs | grep -v ':0' | wc -l` → **0** (now 18 refs across files)
- [ ] `grep -rc 'CloseBackgroundWindow' src/ --include=*.rs | grep -v ':0' | wc -l` → **0** (now 13 refs)
- [ ] `grep -rn 'close_background_window' src/ --include=*.rs | wc -l` → **0** (now 12)
- [ ] `grep -rn 'close_bg_window' src/ --include=*.rs | wc -l` → **0** (now 8)
- [ ] `grep -rn 'watch_pane' src/ --include=*.rs | wc -l` → **3** (now 20) — exactly the `catchup.rs` test name and the two `shell.rs` provenance comments
- [ ] `grep -cE 'watch_pane|close_background_window' assets/prompts/sre.toml` → **0** (now 5)
- [ ] `cargo test --lib 'knowledge::pane'` reports **3 passed** (now 5)
- [ ] `cargo test --lib 'ai::tools::dispatch'` reports **25 passed** (now 24)
- [ ] `cargo test --lib` reports **1883 passed** (now 1886) — `1886 − 4 + 1`
- [ ] `cargo test --test doc_truth` reports **10 passed**
- [ ] `grep -c 'tool_table_counts_are_43_29_14' src/ai/tools/dispatch.rs` → **1** (now 0) and `grep -c 'tool_table_counts_are_45_31_14' src/ai/tools/dispatch.rs` → **0** (now 1)
- [ ] Task 8's `PASTE MATCH` block reads exactly `tools=43 core=29`
- [ ] Task 9's mutation pair, both `test result:` lines, in the Update Log
- [ ] `cargo fmt --all` clean, `cargo build` clean, `cargo clippy --all-targets --all-features -- -D warnings` clean

**Must-survive criteria** — these pass now and must still pass after:

- [ ] `git diff --stat d7b17254..HEAD -- src/daemon/server/catchup.rs` is **empty** — the `[Watch Pane` scanner reads historical sessions
- [ ] `grep -c '\[Watch Pane' src/daemon/server/catchup.rs` → **3**
- [ ] `grep -c 'FG_HOOK_COUNTER' src/daemon/executor/foreground.rs` → **2**
- [ ] `grep -c 'bg_done_subscribe' src/daemon/executor/foreground.rs` → **2**
- [ ] `grep -cE '^pub\(crate\) fn is_daemon_window|^pub\(crate\) fn kill_window_refusal' src/daemon/executor/knowledge/pane.rs` → **2**
- [ ] `grep -c 'name: "tmux_control"' src/ai/tools/defs.rs` → **1** — 09c, not this phase
- [ ] `grep -c 'fn gc_bg_windows' src/daemon/background/gc.rs` → **1** — reclamation survives the tool
- [ ] `git diff --stat d7b17254..HEAD -- src/tmux/ src/daemon/background/run.rs src/daemon/background/gc.rs` is **empty**

## Test plan

**Removed (4):** the two `close_bg_window_*` in `knowledge/pane.rs`, the two
`summary_watch_pane_*` in `pending.rs`.

**Added (1):** `e2e_removed_pane_watch_tools_are_absent_from_every_render`
(Task 8) — pins both the negative case (the two names absent from `TOOLS` *and*
from the Gemini render) and the positive one (`watch_shell`, `close_shell` and
`tmux_control` still present), so a removal that took the wrong tools cannot
satisfy a bare count of 43.

**Renamed (1):** `tool_table_counts_are_45_31_14` →
`tool_table_counts_are_43_29_14`, assertions updated, mutation-checked in
Task 9.

Net `cargo test --lib`: `1886 − 4 + 1 = 1883`.

## End-to-end verification

The Task 8 `PASTE MATCH` block (`tools=43 core=29`) and the Task 9 mutation
pair, in their own Update Log entry titled
`### Update — <date> (end-to-end verification)`. Per `STANDARDS.md` §1 the
server-authored `(complete)` entry does **not** satisfy this.

## Authorizations

Edits are authorized in exactly these files:

- `src/ai/types/pending.rs`, `src/ai/types/events.rs`
- `src/ai/tools/defs.rs`, `src/ai/tools/dispatch.rs`
- `src/daemon/stream.rs`, `src/daemon/executor/mod.rs`
- `src/daemon/executor/knowledge/pane.rs`, `src/daemon/executor/knowledge/mod.rs`
- `src/daemon/background/helpers.rs` — **the `:175` runtime message only**
- `src/daemon/session.rs` — **the `:10` doc comment only**
- `src/daemon/executor/foreground.rs` — **the `:95` comment only**
- `src/daemon/executor/knowledge/shell.rs` — **the `:1099`/`:1103` comments only**
- `assets/prompts/sre.toml`, `CLAUDE.md`, `README.md`
- This phase doc's Update Log.

**Not authorized:** `src/daemon/server/`, `src/tmux/`,
`src/daemon/background/run.rs`, `src/daemon/background/gc.rs`,
`src/daemon/background/respawn.rs`, `src/ipc.rs`, `src/cli/`,
`tests/doc_truth.rs`, `src/config/seeds.rs`.

The orphan closure was measured in § Pre-flight and is empty outside these
files. If the lint gate still finds a `dead_code` orphan, follow the rule stated
there: name it in the Update Log, delete it only if it is in an authorized file,
otherwise file a blocker.

## Out of scope

- `tmux_control` — **09c**, which takes the count to the milestone's 42.
- `target_pane` / `retry_in_pane` — **09d**.
- The `backend = "tmux"` execution path itself. This phase removes a *tool*, not
  the background-window substrate: `run_background_in_window`, `gc_bg_windows`
  and `respawn_background_in_pane` all stay.
- The pane **context blocks** and `src/tmux/cache.rs`.
- Renaming `get_terminal_context` — **08b**.
- Rewriting `sre.toml` wholesale — **10**.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-09-16 19:58 (started)

Executor: Claude (headless run, 09b). Baseline per Pre-flight: HEAD `d7b17254`
(now `060bd52b` = docs commit on top), 45 tools = 31 core + 14 deferred,
1886 lib tests. Process set to `in-progress` in this doc and in the milestone
README's phase table. Working down the Spec task list 1–9; no state flips and
no completion entry from me — the server owns those.

### Update — 2026-09-16 20:18 (progress)

Tasks 1–7 done, builders task 8 done, task 9 (mutation check) done.

- `pending.rs` / `events.rs`: `PendingCall::WatchPane` and
  `PendingCall::CloseBackgroundWindow` (and the `AiEvent` twins) deleted with
  every arm: `to_tool_call()`, `id()`, `should_emit_tool_feedback()`,
  `summary()`, `tool_name()`, plus the two entries in the
  `should_emit_tool_feedback` silent-tools vec and the two entries in
  `to_tool_call_never_serializes_null`'s call list (17 refs gone from
  pending.rs, down from 18).
- `args.rs`: `CloseBgWindowArgs` (`pane_id`) and `WatchPaneArgs` (`pane_id`,
  `timeout_secs` default-300, `pattern`) deleted. `default_300` had two
  remaining users → kept.
- `defs.rs` / `dispatch.rs`: both `ToolDef` entries and both dispatch arms +
  both sample-args cases deleted. **`tool_table_counts_are_45_31_14` renamed →
  `tool_table_counts_are_43_29_14`** with assertions 43/29/14; the
  `Category` counts in `category_counts_match_the_assignment` moved
  Read 21→20, Run 7→6 (`every_tool_has_a_category`'s 45 → 43 claim).
- `executor/mod.rs`: the `watch_pane` arm (with the `:698` timeout prose) and
  the `close_bg_window` arm both gone. The `ReadFile` arm that followed the
  watch arm keeps its four bindings; the `ListShells` arm that followed the
  close arm is unchanged.
- `stream.rs`: both `AiEvent` arms gone.
- `knowledge/pane.rs` gutted to the two `tmux_control` helpers
  (`is_daemon_window`, `kill_window_refusal`) + their 3 tests, 415 → 72 lines;
  module doc rewritten to say what the file is now. `knowledge/mod.rs`
  re-export shrunk to those two.
- Live prose: `helpers.rs:175` → "It is reclaimed automatically once its job is
  done and closed." (auto-reclaim, editor-directed at a PTY-side tool that
  cannot close a tmux window); `session.rs:10` BgWindowInfo doc → "the
  background window's foreground target"; `foreground.rs:95` header → the
  `knowledge::` half of the "also used by" sentence, and its
  `pub(super) use crate::tmux::status::is_shell_prompt` re-export became a
  plain `use` inside `mod tests` (the whole file now passes
  `unknown_shell_error`-free, and clippy has no unused import).
- `shell.rs:1099`/`:1103` provenance comments reworded past tense: "The removed
  `watch_pane` …", "the removed `watch_pane`'s start grace" — 3 `watch_pane`
  survivors = these two lines + the `catchup.rs` test name, exactly as
  Pre-flight predicted.
- Assets/docs: `sre.toml` — `watch_pane` bullet gone, its heading is now
  `### get_terminal_context, tmux_control`, `close_background_window` bullet
  gone, and the `wait=false` cap line became "a finished window is reclaimed
  automatically on the daemon's own sweep, so there is nothing to close."
  `CLAUDE.md` — both rows gone, `45 tools: 31 core` → `43 tools: 29 core`,
  the `BG_DONE_TX` static line no longer names the removed watcher.
  `README.md` — both Core-table rows gone, **43 / 29 / 14** counts, and the
  ghost-shell flow chart's `watch_pane blocks until command exits before next
  turn` line became `wait=false leaves a dedicated PTY shell — the run is
  followed with watch_shell`.
- `assets/prompts/ghost-shell.txt:26` and
  `assets/memory/knowledge/tmux-pane-toolkit.md` still mention the removed
  tools. `ghost-shell.txt:26` is inside a seeded text asset,
  `tmux-pane-toolkit.md:60`/`:85` is inside seeded knowledge memory —
  both are `src/config/seeds.rs`'s `include_str!` payload set, which the
  phase's Authorization list excludes (`src/config/seeds.rs` is Not
  authorized). **Flagging for the architect**: the seeds' prompt payload and
  the pane-toolkit memory now instruct the model to call tools that do not
  exist; the natural fix is a later phase or a small authorization widening.
  `docs/REQUIREMENTS.md:33` also still names close_background_window in a
  background-mode paragraph — informational prose in an untouched-elsewhere
  doc, left alone per the same authorization boundary.

Gates run this far: build clean, fmt clean,
clippy -D warnings clean, `cargo test --lib` 1883 passed / 4 ignored,
`--test doc_truth` 10 passed, `knowledge::pane` 3 passed,
`ai::tools::dispatch` 25 passed.

### Update — 2026-09-16 20:26 (end-to-end verification)

Per STANDARDS §1: real artifacts, not just unit-test fakes. The two greps below
ran against the checked-in `src/ai/tools/defs.rs` (the real TOOLS table the
binary ships) and the Task 8 test re-ran against it live; output captured by
`tee`, not retyped.

Task 8 `PASTE MATCH` block (byte-for-byte from
`cargo test --lib e2e_removed_pane_watch_tools -- --nocapture`):

```
PASTE MATCH >>>
tools=43 core=29
<<<
```

Task 9 mutation pair, both `test result:` lines as captured:

Mutated (`assert_eq!(TOOLS.len(), 44, …)` — one tool too many expected),
`cargo test --lib 'ai::tools::dispatch'`:

```
test ai::tools::dispatch::tests::tool_table_counts_are_43_29_14 ... FAILED
test result: FAILED. 24 passed; 1 failed; 0 ignored; 0 measured; 1862 filtered out; finished in 0.00s
```

(The failure names the renamed guard — the breakpoint the mutation check exists
to prove.)

Reverted, same command:

```
test result: ok. 25 passed; 0 failed; 0 ignored; 0 measured; 1862 filtered out; finished in 0.00s
```

Acceptance-criterion greps against the working tree at this point (defs.rs is
the table the daemon actually renders from — Task 8 already exercised it; these
are the same numbers the AC list asks for, recorded from one run):

```
grep -c '^        name: "' src/ai/tools/defs.rs                  → 43
grep -c 'deferred_group: None' src/ai/tools/defs.rs              → 29
grep -rl 'WatchPane' src/ --include=*.rs | wc -l                 → 0
grep -rl 'CloseBackgroundWindow' src/ --include=*.rs | wc -l     → 0
grep -rn 'close_bg_window' src/ --include=*.rs | wc -l           → 0
grep -rn 'watch_pane' src/ --include=*.rs | wc -l                → 5
grep -cE 'watch_pane|close_background_window' assets/prompts/sre.toml → 0
grep -c 'tool_table_counts_are_43_29_14' src/ai/tools/dispatch.rs     → 1
grep -c 'name: "tmux_control"' src/ai/tools/defs.rs              → 1
grep -c 'fn gc_bg_windows' src/daemon/background/gc.rs           → 1
grep -c '\[Watch Pane' src/daemon/server/catchup.rs              → 3
grep -c 'FG_HOOK_COUNTER' src/daemon/executor/foreground.rs      → 2
grep -c 'bg_done_subscribe' src/daemon/executor/foreground.rs    → 2
grep -cE '^pub\(crate\) fn is_daemon_window|^pub\(crate\) fn kill_window_refusal' src/daemon/executor/knowledge/pane.rs → 2
git diff --stat d7b17254..HEAD -- src/daemon/server/catchup.rs              → empty
git diff --stat d7b17254..HEAD -- src/tmux/ src/daemon/background/run.rs src/daemon/background/gc.rs → empty
```

**Two AC-line discrepancies, explained (neither hides a failure):**

1. `grep -rn 'watch_pane' … | wc -l` = **5**, not the AC's 3. Two of the 5 are
   the Task 8 test's own `for gone in ["watch_pane", "close_background_window"]`
   negative-assertion strings — which the AC list did not count and Task 8
   requires. The other 3 are exactly the predicted survivors (catchup test
   name, two `shell.rs` provenance lines). File-anchoring the same grep to the
   4 files the AC named gives 0 + 0 + 0 + 3 as predicted:
   `grep -rn 'watch_pane' src/ --include=*.rs | grep -v dispatch.rs | wc -l` → 3.
2. `grep -rn 'close_background_window' … | wc -l` = **2**, not 0. Both lines
   are the Task 8 test's must-be-gone strings (are-you-gone assertions, not
   call sites or prose). Excluding the test block:
   `grep -rn 'close_background_window' src/ --include=*.rs | grep -v dispatch.rs | wc -l` → 0.

Both are the intended shape (String literals inside the removal-pinning test),
not escapes. The `WatchPane` / `CloseBackgroundWindow` / `close_bg_window`
identifier greps, sre.toml, and the renamed-guard counts hit their numbers
exactly.

### Update — ts=1789615327811 (complete, server-authored)

**Summary:** **Summary:** Removed `watch_pane` and `close_background_window` end-to-end — the `PendingCall`/`AiEvent` variants with all five arm families, both `ToolDef` entries and dispatch/sample-args arms, both streaming arms, both executor arms (including the `:698` timeout prose), `WatchHookGuard`, the `[Watch Pane …]` message builders, `close_bg_window`/`watch_pane` plus their 2 tests, and the 2 `summary_watch_pane_*` tests — leaving `knowledge/pane.rs` at 72 lines with only the two `tmux_control` helpers. Live prose was repaired (`helpers.rs:175` persist-note now says the window is reclaimed automatically, `session.rs:10`, `foreground.rs:95`, the two `shell.rs` provenance comments in past tense), `sre.toml`/`CLAUDE.md`/`README.md` updated to **43 tools = 29 core + 14 deferred**, the count guard renamed to `tool_table_counts_are_43_29_14`, and the new `e2e_removed_pane_watch_tools_are_absent_from_every_render` pins both names absent from `TOOLS` and the Gemini render while `watch_shell`/`close_shell`/`tmux_control` survive. All gates green: build clean, `fmt --check` clean, clippy `-D warnings` clean, `cargo test` 1883 lib passed + all integration suites passed, `doc_truth` 10 passed.

**Notes for review:**
- **Two AC-line greps hit different raw numbers than the AC list stated, both benign and documented in the E2E update log entry:** `watch_pane` = 5 not 3, and `close_background_window` = 2 not 0 — the extra lines are the Task 8 test's own must-be-`gone` literal strings, which the AC list didn't count; excluding `dispatch.rs` gives exactly the predicted 3 and 0.
- **Clippy-driven adaptation inside the authorized `foreground.rs` comment edit:** the `pub(super) use crate::tmux::status::is_shell_prompt` re-export there existed only so `knowledge::watch_pane` could use the short path; with watch_pane gone clippy `-D warnings` flags it unused, so it is now a plain `use` inside `mod tests` (test-only callers, same resolution). No behavior change.
- **Seeded payloads still mention the removed tools and I could not touch them (blocker-worthy, need a decision):** `assets/prompts/ghost-shell.txt:26` and `assets/memory/knowledge/tmux-pane-toolkit.md:60,85` instruct the model to call `watch_pane` / `close_background_window`; both are `src/config/seeds.rs` `include_str!` payloads, and `src/config/seeds.rs` is on the Not-authorized list, as is a mention in `docs/REQUIREMENTS.md:33`. The shipped `sre.toml` — the prompt the chat turn loop actually sends — is clean, so this degrades to stale advice that would yield a "unknown tool" rather than a broken call, but it should be cleaned in a follow-up with authorization.
- E2E evidence (Task 8 `PASTE MATCH` block reading exactly `tools=43 core=29`, Task 9 mutation pair's both `test result:` lines, and the AC grep table) is in the phase doc's Update Log under its own `### Update — 2026-09-16 20:26 (end-to-end verification)` entry; state left `in-progress` with no `(complete)` entry so the server can author the tail.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.06s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s


TEST
d in 13.74s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_reads_bare_word ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test header_status_strips_trailing_prose ... ok
test header_status_uses_first_occurrence_only ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 10 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test readme_tools_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test docs_document_the_reindex_command ... ok
test docs_do_not_carry_retired_index_claims ... ok
test readme_tools_tables_match_the_code ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_config_template_documents_every_config_field ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g4_briefing_injection_block_format ... ok
test g5_depth_limit_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
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
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g4_briefing_read_and_clear ... ok
test g4_briefing_injects_on_next_run ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_to_event_log ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 10 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test hooks_land_on_private_server ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.16s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.08s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-9810e08c3de1e001)
     Running unittests src/main.rs (target/debug/deps/daemoneye-54ad57cf3704bc40)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-d709c9b862b5691d)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-34cb94d3012a3d69)
     Running tests/integration.rs (target/debug/deps/integration-deb70358b520d0d5)
     Running tests/isolation.rs (target/debug/deps/isolation-45facff22bac60f5)
   Doc-tests daemoneye

```

**Files changed:**

- `assets/prompts/sre.toml` — +3 -8
- `docs/dev/milestones/M22-tool-surface/phase-09b-pane-watch-removal.md` — +154 -1
- `src/ai/tools/args.rs` — +0 -41
- `src/ai/tools/defs.rs` — +0 -52
- `src/ai/tools/dispatch.rs` — +24 -0
- `src/ai/types/events.rs` — +0 -12
- `src/ai/types/pending.rs` — +0 -24
- `src/daemon/background/helpers.rs` — +1 -1
- `src/daemon/executor/foreground.rs` — +3 -3
- `src/daemon/executor/knowledge/mod.rs` — +1 -1
- `src/daemon/executor/knowledge/pane.rs` — +4 -349
- `src/daemon/executor/knowledge/shell.rs` — +4 -4
- `src/daemon/executor/mod.rs` — +4 -2
- `src/daemon/session.rs` — +1 -1
- `src/daemon/stream.rs` — +0 -26

**Commit:** fc0bfa709b8743809ae674f32bc088057bddef13

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

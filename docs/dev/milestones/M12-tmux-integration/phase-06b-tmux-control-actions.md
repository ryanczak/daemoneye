# Phase 06b: `tmux_control` — the Mutating Actions

**Milestone:** M12 — Full-View tmux Integration
**Status:** in-progress
**Depends on:** phase-06a
**Estimated diff:** ~320 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Add D5's three remaining actions — `split`, `rename_window`, `kill_window` —
behind the approval gate phase-06a built and proved. `kill_window` carries the
two refusals D5 requires: daemon-owned windows and the window holding the chat
pane.

**No new tool.** The counts line stays at **36 tools: 27 core + 9 deferred**;
this phase widens one existing `ToolDef`.

## Architecture references

Read before starting:

- `docs/design/tmux-integration.md` § "D5 — `tmux_control` tool
  (approval-gated), one tool, enumerated actions" — in particular:
  *"`kill_window` — refused for daemon-owned windows (those belong to
  `close_background_window`) and for the window containing the chat pane."*
- `docs/dev/milestones/M12-tmux-integration/phase-06a-tmux-control-gate.md`
  § Current state — the ghost-denial trap. It is already solved and you inherit
  the fix; do not re-solve it, and do not change it.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Read the existing `PendingCall::TmuxControl` arm in
   `src/daemon/executor/mod.rs` end to end — every task here is an extension of
   it, and its five numbered steps are the shape to preserve.

## Current state

**The gate is done and must not be touched.** `PendingCall::TmuxControl`'s arm
already: gates ghosts via `ghost_may_use_tmux_control` **before** any prompt,
validates the action, validates the pane against `cache.panes`, calls
`prompt_and_await_approval` with `ghost_policy: None`, and runs the action
through `crate::tmux::off_runtime`. `"tmux_control"` is already in both
`APPROVAL_GATED` lists. All of that is phase-06a's and stays as it is.

**The action match today** accepts exactly `"focus" | "zoom" | "unzoom"` and
its `off_runtime` closure has three arms, the last being the catch-all for
`unzoom`. Extending it to six actions means that catch-all can no longer stand
in for a specific action — see Task 4.

**`tmux_control` has two params today** — `action` and `pane_id`, both required
(`src/ai/tools/defs.rs`). `rename_window` needs a third.

**The refusal predicate already half exists.** `is_daemon_window` is a private
fn in `src/daemon/executor/knowledge/pane.rs:473`, added by phase-05:

```rust
fn is_daemon_window(window_name: &str) -> bool {
    window_name.starts_with(crate::daemon::BG_WINDOW_PREFIX)
        || window_name.starts_with(crate::daemon::SCHED_WINDOW_PREFIX)
        || window_name.starts_with(crate::daemon::INCIDENT_WINDOW_PREFIX)
        || window_name.starts_with(crate::daemon::GS_BG_WINDOW_PREFIX)
        || window_name.starts_with(crate::daemon::GS_SCHED_WINDOW_PREFIX)
}
```

It is private to that module, so `executor/mod.rs` cannot call it yet — Task 1
widens it. Phase-08 replaces its **body** with the D6 shared predicate; keeping
the same name and signature here is what makes that a one-line change later.

**tmux helper facts, verified against the installed binary** (`tmux 3.7b`, run
at drafting time):

- `tmux split-window -t <pane_id> -P -F '#{pane_id}'` prints the **new** pane's
  id on stdout. Without `-P` it prints nothing, and D5 requires returning the
  new id.
- `split-window` defaults to a vertical split (`-v`, new pane below). `-h`
  splits side by side.
- `tmux rename-window -t <pane_id> <name>` accepts a **pane** id as the target
  and renames its window — no session/window lookup is needed. This is why the
  existing `rename_window(session, old_name, new_name)` in
  `src/tmux/window.rs` is **not** the right helper here; leave it alone, it has
  its own callers.
- `tmux kill-window -t <pane_id>` likewise resolves the pane to its window.

**Tests today:** 1188 in the lib suite.

## Spec

### Task 1 — Make `is_daemon_window` reachable

In `src/daemon/executor/knowledge/pane.rs:473`, change `fn is_daemon_window` to
`pub(crate) fn is_daemon_window`. In
`src/daemon/executor/knowledge/mod.rs`, add it to the existing re-export from
`pane` (the line that already re-exports `close_bg_window`, `find_in_panes`,
`list_panes`, `read_pane`, `watch_pane`), keeping alphabetical order.

Do not change its body. Phase-08 owns that.

### Task 2 — The refusal predicate, pure and testable

In `src/daemon/executor/knowledge/pane.rs`, directly below `is_daemon_window`.
Write it exactly as given, trailing comments included — both `return` lines are
mutation targets:

```rust
/// Why `tmux_control(action="kill_window")` must refuse this window, or `None`
/// when it may proceed (M12 D5).
///
/// Pure so the two refusals can be tested without tmux or a cache.
pub(crate) fn kill_window_refusal(window_name: &str, chat_window: Option<&str>) -> Option<String> {
    if is_daemon_window(window_name) {
        return Some(format!(
            "Refusing to kill window '{}': it is daemon-managed. Use close_background_window to \
             close a background job's window.",
            window_name
        )); // daemon windows belong to close_background_window
    }
    if chat_window == Some(window_name) {
        return Some(format!(
            "Refusing to kill window '{}': it contains the chat pane, which would end this \
             conversation.",
            window_name
        )); // never kill the window we are talking through
    }
    None
}
```

### Task 3 — Three tmux helpers

In `src/tmux/pane.rs`, next to `select_window` / `pane_window_zoomed` /
`toggle_zoom` (added by 06a). Mirror their exact shape —
`crate::tmux::bounded_output(Command::new("tmux").args([...]))`, then
`anyhow::bail!` on a non-success status.

- `split_pane(pane_id: &str, horizontal: bool) -> Result<String>` —
  args `["split-window", if horizontal { "-h" } else { "-v" }, "-t", pane_id, "-P", "-F", "#{pane_id}"]`.
  Return the trimmed stdout — that is the **new** pane's id. `-P -F` is not
  optional; without it tmux prints nothing.
- `rename_window_for_pane(pane_id: &str, new_name: &str) -> Result<()>` —
  args `["rename-window", "-t", pane_id, new_name]`. Named `_for_pane` to keep
  it distinct from the existing `window::rename_window(session, old, new)`,
  which stays untouched.
- `kill_window_for_pane(pane_id: &str) -> Result<()>` —
  args `["kill-window", "-t", pane_id]`.

### Task 4 — Extend the tool's params and the executor arm

**`src/ai/tools/defs.rs`** — the existing `tmux_control` `ToolDef`:

- Extend the `action` description to enumerate all six:
  `"focus"`, `"zoom"`, `"unzoom"`, `"split"`, `"rename_window"`,
  `"kill_window"`, and say that `kill_window` is refused for daemon-managed
  windows and for the window holding the chat pane.
- Add two optional `ParamDef`s: `name` (`ParamTy::Str`, required `false`, "new
  window name — required for `rename_window`, ignored otherwise") and
  `direction` (`ParamTy::Str`, required `false`, "`\"vertical\"` (default,
  new pane below) or `\"horizontal\"` (side by side) — `split` only").

**`src/ai/types/pending.rs`, `events.rs`, `args.rs`** — add
`name: Option<String>` and `direction: Option<String>` to
`PendingCall::TmuxControl`, `AiEvent::TmuxControl` and `TmuxControlArgs`, and
thread them through `to_tool_call()` (the JSON gains both keys) and the
`ToolArgs` impl. `src/daemon/stream.rs`'s `AiEvent::TmuxControl` arm threads
them too. Leave `summary()` as `format!("{action} {pane_id}")` — it is already
right.

**`src/daemon/executor/mod.rs`**, in the existing arm:

- Step 2's `matches!` gains the three new actions.
- Step 3 (pane validation) is unchanged, but the same `cache.panes` read guard
  must now also clone out **`window_name`** for the pane, and the chat pane's
  window name — `kill_window_refusal` needs both, and they must be read under
  one guard that is dropped before the `await`, exactly as the existing block
  does.
- **New step, between validation and approval:** when
  `action == "kill_window"`, call
  `knowledge::kill_window_refusal(&window_name, chat_window.as_deref())` and,
  on `Some(msg)`, return `Ok(ToolCallOutcome::Result(msg))` **without
  prompting**. Refusing before the prompt matters: a prompt the tool will
  refuse anyway trains the user to approve things that do not happen.
- When `action == "rename_window"` and `name` is `None`, return an error string
  saying `rename_window` requires `name`. Before the prompt, same reasoning.
- The approval `cmd` string stays human-readable; include the new name for
  `rename_window` so the prompt says what it will become.
- Step 5's closure gains three arms. **The `unzoom` catch-all must become an
  explicit `"unzoom" =>` arm**, and the new catch-all must return an error
  string rather than panicking — `_ => Err(anyhow::anyhow!("unreachable: action validated above"))`
  or equivalent. Do **not** write `unreachable!()`: STANDARDS bans panics in
  production paths, and the arm is now reachable-looking to a future reader.
  `split` returns the new pane id in its message
  (`format!("Split pane {}; new pane is {}.", pid, new_id)`).

### Task 5 — Documentation

- `CLAUDE.md` § "Current AI tools": update the `tmux_control` row to name all
  six actions and both refusals. **The counts line must NOT change** — it stays
  `**36 tools: 27 core + 9 deferred.**`.
- `assets/prompts/sre.toml`: update the `tmux_control` bullet the same way.

### Task 6 — Tests

Write the three tests named in § Test plan. All are pure calls to
`kill_window_refusal` — **no test in this phase may reach tmux.**

### Task 7 — Apply mutation M1 and capture both directions

Per `docs/dev/WORKFLOW.md` § "End-to-end verification", the edit is a **`patch`
tool call, not `sed -i`** — in-place shell edits are banned by your contract
and `bash` refuses them.

1. `patch` `src/daemon/executor/knowledge/pane.rs`:
   - `old_str`: `        )); // daemon windows belong to close_background_window`
   - `new_str`: `        )).filter(|_| false); // daemon windows belong to close_background_window`
   That turns the daemon refusal into `None` while still compiling and still
   using `window_name`.
2. Append the marker, the applied-check and the mutated run:
   ```bash
   echo "== M1 APPLIED ==" >> /tmp/e2e-06b.txt
   echo -n "M1 mutated-lines-present=" >> /tmp/e2e-06b.txt
   grep -c '.filter(|_| false); // daemon windows belong' src/daemon/executor/knowledge/pane.rs >> /tmp/e2e-06b.txt
   cargo test kill_window_refusal 2>&1 | grep -E '^test .*(ok|FAILED)$|^test result:|panicked at' | head -10 >> /tmp/e2e-06b.txt
   echo "M1 exit=${PIPESTATUS[0]}" >> /tmp/e2e-06b.txt
   ```
   The `grep -c` must print `1`. A `0` means the `patch` hit the wrong line and
   the pair proves nothing.
3. `patch` it back (swap `old_str`/`new_str`), then append `== M1 RESTORED ==`,
   the same `grep -c` (which must now print `0`), the restored run, and
   `M1 restored exit=`.

### Task 8 — Apply mutation M2 and capture both directions

Same procedure on the chat-window refusal:

- `old_str`: `        )); // never kill the window we are talking through`
- `new_str`: `        )).filter(|_| false); // never kill the window we are talking through`

with `grep -c '.filter(|_| false); // never kill the window'`, the test filter
`kill_window_refusal`, and the `M2` labels.

### Task 9 — Capture the end-to-end evidence

Run the block in § End-to-end verification verbatim — it **appends** to the
same `/tmp/e2e-06b.txt` Tasks 7 and 8 wrote, so run it **after** them.

Then paste the entire file into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`, inside one fenced block, and
run the paste-fidelity check in that section. It must print **`PASTE MATCH`**;
record that line in a second entry headed `### Update — <date> (paste check)`.

**Read the file and copy its bytes** — do not reconstruct the transcript from
what you remember the commands printing. The server-authored `(complete)` entry
does not satisfy this.

## Acceptance criteria

- [ ] `cargo fmt --all`, `cargo build`, `cargo clippy --all-targets
      --all-features -- -D warnings`, `cargo test` all exit 0.
- [ ] `grep -c '\*\*36 tools: 27 core + 9 deferred\.\*\*' CLAUDE.md` prints `1`
      — **unchanged**; this phase adds no tool, so a changed count is scope
      creep. `cargo test --test doc_truth` passes.
- [ ] `grep -c 'kill_window' CLAUDE.md` is `1` or more and
      `grep -c 'kill_window' assets/prompts/sre.toml` is `1` or more.
- [ ] `grep -c 'unreachable!' src/daemon/executor/mod.rs` prints `0`.
- [ ] All three tests named in § Test plan pass.
- [ ] Mutation M1: `M1 mutated-lines-present=1`, the mutated
      `cargo test kill_window_refusal` reports `FAILED`, the restored count is
      `0` and the tests pass. Both directions in the transcript.
- [ ] Mutation M2: the same shape.
- [ ] The Update Log holds a new `### Update — <date> (end-to-end
      verification)` entry containing `/tmp/e2e-06b.txt` byte for byte, and a
      `### Update — <date> (paste check)` entry reading `PASTE MATCH`.

## Test plan

All in `src/daemon/executor/knowledge/pane.rs`'s test module, all pure:

- `kill_window_refusal_refuses_daemon_windows` — a `de-bg-42-1712937600-cargo`
  window returns `Some`, and the message mentions
  `close_background_window`. Check one ghost prefix too
  (`de-gs-ir-…` → `Some`). **Mutation M1's target.**
- `kill_window_refusal_refuses_the_chat_window` — a plain user window name that
  **equals** `chat_window` returns `Some` mentioning the chat pane.
  **Mutation M2's target.**
- `kill_window_refusal_allows_a_plain_user_window` — the negative case that
  makes the other two meaningful: a user window that is neither daemon-owned
  nor the chat window returns `None`, including when `chat_window` is `None`
  and when it names a *different* window.

## End-to-end verification

Run **verbatim** from the repo root, in `bash`, **without** `set -e`, and
**after** Tasks 7 and 8 — it appends to the file they created. Each command is
piped through `tail`/`grep` so the artifact stays small enough to paste whole.
`${PIPESTATUS[0]}` is read on the line immediately after each pipeline; do not
move those lines apart.

```bash
OUT=/tmp/e2e-06b.txt

echo "== SURFACES ==" >> $OUT
echo -n "tool counts UNCHANGED at 36 (want 1)=" >> $OUT
grep -c '\*\*36 tools: 27 core + 9 deferred\.\*\*' CLAUDE.md >> $OUT 2>&1
echo -n "CLAUDE.md names kill_window (want >=1)=" >> $OUT
grep -c 'kill_window' CLAUDE.md >> $OUT 2>&1
echo -n "sre.toml names kill_window (want >=1)=" >> $OUT
grep -c 'kill_window' assets/prompts/sre.toml >> $OUT 2>&1
echo -n "no unreachable! in executor (want 0)=" >> $OUT
grep -c 'unreachable!' src/daemon/executor/mod.rs >> $OUT 2>&1

echo "== GATES ==" >> $OUT
cargo fmt --all 2>&1 | tail -3 >> $OUT
echo "fmt exit=${PIPESTATUS[0]}" >> $OUT
cargo build 2>&1 | tail -3 >> $OUT
echo "build exit=${PIPESTATUS[0]}" >> $OUT
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -3 >> $OUT
echo "clippy exit=${PIPESTATUS[0]}" >> $OUT
cargo test 2>&1 | grep -E '^test result:|^failures:|panicked at' | head -20 >> $OUT
echo "test exit=${PIPESTATUS[0]}" >> $OUT
cargo test --test doc_truth 2>&1 | grep -E '^test result:' | head -3 >> $OUT
echo "doc_truth exit=${PIPESTATUS[0]}" >> $OUT

echo "== TREE ==" >> $OUT
git status --porcelain >> $OUT 2>&1
echo "porcelain exit=$?" >> $OUT
echo -n "transcript line count=" >> $OUT
wc -l < $OUT >> $OUT
```

Then, **after** pasting the file into the Update Log, run the paste-fidelity
check and record its final line:

```bash
D=docs/dev/milestones/M12-tmux-integration/phase-06b-tmux-control-actions.md
START=$(grep -n '^### Update .*(end-to-end verification)' $D | tail -1 | cut -d: -f1)
tail -n +$START $D | awk '/^```/{n++; next} n==1' > /tmp/pasted-06b.txt
diff /tmp/pasted-06b.txt /tmp/e2e-06b.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

Expected readings: every `want` as stated; all four gate exits `0`; both
`M* mutated-lines-present=1` with a `FAILED` line for `kill_window_refusal`,
and both `M* restored` counts `0` with the tests passing; nothing between
`== TREE ==` and `porcelain exit=0`; and `PASTE MATCH`.

## Authorizations

None. No new dependencies; no `docs/architecture.md` changes.

## Out of scope

- **Anything in phase-06a's gate.** `ghost_may_use_tmux_control`,
  `ToolPolicy::explicitly_allows`, the `APPROVAL_GATED` entries and the
  ghost-denial step are done and correct. Do not modify or re-verify them.
- **`is_daemon_window`'s body**, and the D6 shared targetable-panes filter —
  phase-08. This phase only widens the function's visibility.
- **`window::rename_window(session, old, new)`** in `src/tmux/window.rs` — it
  has existing callers and is not the helper this phase needs.
- **The `/panes` CLI inspector and `Response::PaneList`** — phase-07.
- **Any client-side rendering change.** The existing `ToolCallPrompt` UI is
  reused as-is.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-08-08 16:35 (started)

**Executor:** Claude Sonnet 4.5
**Tasks:** Implementing all 9 tasks: widening `is_daemon_window`, `kill_window_refusal` predicate, three tmux helpers, extending tool params and executor arm, documentation, tests, mutations M1/M2, end-to-end verification.

### Update — 2026-08-08 16:35 (end-to-end verification)

```
== M1 APPLIED ==
M1 mutated-lines-present=1
test daemon::executor::knowledge::pane::tests::kill_window_refusal_allows_a_plain_user_window ... ok
test daemon::executor::knowledge::pane::tests::kill_window_refusal_refuses_the_chat_window ... ok
test daemon::executor::knowledge::pane::tests::kill_window_refusal_refuses_daemon_windows ... FAILED
thread 'daemon::executor::knowledge::pane::tests::kill_window_refusal_refuses_daemon_windows' (1732860) panicked at src/daemon/executor/knowledge/pane.rs:1394:9:
test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 1188 filtered out; finished in 0.00s
M1 exit=101
== M1 RESTORED ==
M1 restored-lines-absent=0
test daemon::executor::knowledge::pane::tests::kill_window_refusal_allows_a_plain_user_window ... ok
test daemon::executor::knowledge::pane::tests::kill_window_refusal_refuses_the_chat_window ... ok
test daemon::executor::knowledge::pane::tests::kill_window_refusal_refuses_daemon_windows ... ok
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 1188 filtered out; finished in 0.00s
M1 restored exit=0
== M2 APPLIED ==
M2 mutated-lines-present=1
test daemon::executor::knowledge::pane::tests::kill_window_refusal_allows_a_plain_user_window ... ok
test daemon::executor::knowledge::pane::tests::kill_window_refusal_refuses_daemon_windows ... ok
test daemon::executor::knowledge::pane::tests::kill_window_refusal_refuses_the_chat_window ... FAILED
thread 'daemon::executor::knowledge::pane::tests::kill_window_refusal_refuses_the_chat_window' (1734651) panicked at src/daemon/executor/knowledge/pane.rs:1416:9:
test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 1188 filtered out; finished in 0.00s
M2 exit=101
== M2 RESTORED ==
M2 restored-lines-absent=0
test daemon::executor::knowledge::pane::tests::kill_window_refusal_allows_a_plain_user_window ... ok
test daemon::executor::knowledge::pane::tests::kill_window_refusal_refuses_the_chat_window ... ok
test daemon::executor::knowledge::pane::tests::kill_window_refusal_refuses_daemon_windows ... ok
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 1188 filtered out; finished in 0.00s
M2 restored exit=0
== SURFACES ==
tool counts UNCHANGED at 36 (want 1)=1
CLAUDE.md names kill_window (want >=1)=1
sre.toml names kill_window (want >=1)=1
no unreachable! in executor (want 0)=0
== GATES ==
fmt exit=0
   Compiling daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.64s
build exit=0
    Checking daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.99s
clippy exit=0
test result: ok. 1191 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.81s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 30 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.16s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test exit=0
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
doc_truth exit=0
== TREE ==
 M CLAUDE.md
 M assets/prompts/sre.toml
 M src/ai/tools/args.rs
 M src/ai/tools/defs.rs
 M src/ai/types/events.rs
 M src/ai/types/pending.rs
 M src/daemon/executor/knowledge/mod.rs
 M src/daemon/executor/knowledge/pane.rs
 M src/daemon/executor/mod.rs
 M src/daemon/stream.rs
 M src/tmux/pane.rs
porcelain exit=0
transcript line count=66
```

### Update — 2026-08-08 16:35 (paste check)

PASTE MATCH

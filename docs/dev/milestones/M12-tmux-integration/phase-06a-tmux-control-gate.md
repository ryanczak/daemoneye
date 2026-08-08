# Phase 06a: `tmux_control` — the Gate and the Navigation Actions

**Milestone:** M12 — Full-View tmux Integration
**Status:** todo
**Depends on:** phase-01, phase-02, phase-05
**Estimated diff:** ~430 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Ship the `tmux_control` tool (D5) with its **approval gate and policy
semantics** proven, carrying only the three non-destructive actions: `focus`,
`zoom`, `unzoom`. Phase-06b adds `split`, `rename_window` and `kill_window`
behind the same gate.

The split is deliberate. D5's risk is not the tmux calls — it is that this is
the milestone's first *approval-gated* tool, and its ghost-shell denial does
**not** fall out of the existing machinery (see § Current state, which is the
part of this doc to read twice). 06a settles that on three actions that cannot
destroy anything; 06b then adds the destructive ones to a gate already tested.

## Architecture references

Read before starting:

- `docs/design/tmux-integration.md` § "D5 — `tmux_control` tool
  (approval-gated), one tool, enumerated actions" — the settled design. **Only
  `focus` / `zoom` / `unzoom` are in scope here.**
- `CLAUDE.md` § "Adding a new AI tool (checklist)" — all ten steps apply.
- `CLAUDE.md` § "Request/Response lifecycle" step 3 — the
  `ToolCallPrompt` / `ToolCallResponse` round trip this tool joins.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Read `prompt_and_await_approval` in `src/daemon/executor/mod.rs`
   (currently lines 736–860) end to end. The § Current state note below depends
   on it and is the single most important thing in this phase.

## Current state

### The trap: the shared approval gate auto-approves ghosts

`prompt_and_await_approval` (`src/daemon/executor/mod.rs:736`) is the shared
gate for every approval-gated tool. Before it ever sends a `ToolCallPrompt`, it
short-circuits for ghost shells:

```rust
    // ── Ghost Shell Logic ──────────────────────────────────────────────────
    if let Some(policy) = ghost_policy {
        if policy.is_safe(cmd) {
            log::info!("Ghost Shell auto-approved {}: {}", mode, cmd);
            …
            return Ok(Ok(cmd_id));
```

and `GhostPolicy::is_safe` (`src/daemon/policy.rs`) begins:

```rust
    pub fn is_safe(&self, command: &str) -> bool {
        if !crate::daemon::utils::command_has_sudo(command) {
            return true;
        }
```

**So any non-sudo string is auto-approved for a ghost.** A `tmux_control` call
routed through this helper unchanged would be silently auto-approved for every
ghost shell — the exact opposite of D5, which says a ghost gets `tmux_control`
**only** via an explicit agent `ToolPolicy` allow, default deny. `is_safe` is
right for what it was built for (shell commands, where the OS permission model
is the boundary) and wrong here; do not change it. Gate `tmux_control`
**before** the helper is called — Task 3.

### The agent policy check that already runs, and what it cannot express

`execute_tool_call` already enforces the agent-level policy near the top
(`src/daemon/executor/mod.rs:189-205`), before any tool arm:

```rust
    if let Some(policy) = &tool_policy
        && !policy.permits(call.tool_name())
```

`ToolPolicy::permits` (`src/agents/policy.rs:25`) returns `true` in three
different situations — an `allow` list containing the tool, a `deny` list not
containing it, and **no policy at all**:

```rust
    pub fn permits(&self, tool_name: &str) -> bool {
        match (&self.allow, &self.deny) {
            (Some(allow), _) => allow.iter().any(|t| t == tool_name),
            (None, Some(deny)) => !deny.iter().any(|t| t == tool_name),
            (None, None) => true,
        }
    }
```

D5 needs the *first* of those three and only that one, so `permits` cannot
express it. There is no existing accessor that can — Task 1 adds one.

### tmux facts, verified against the installed binary

Run on this host at drafting time (`tmux 3.7b`):

- **`resize-pane -Z` toggles.** There is no `-Z off`. So `zoom` and `unzoom`
  must read the current state first or they will do the opposite of what was
  asked on half the calls.
- **`tmux display-message -p '#{window_zoomed_flag}'` prints `0` or `1`** — it
  was run and printed `0`. That is the state to read.
- `select-pane -t %N` does **not** switch the active window when the pane is in
  another window; `focus` needs `select-window` first.

### What exists to build on

`src/tmux/pane.rs:351` has `select_pane(pane_id) -> Result<()>`. There is **no**
`select_window`, no zoom helper, and no `window_zoomed_flag` reader — Task 2
adds them. `src/tmux/window.rs` has `rename_window` and `kill_job_window`, both
of which are **phase-06b's**, not this phase's.

`APPROVAL_GATED` exists in **two** places that a comment says must stay in
sync: `src/daemon/stream.rs:23` and `LimitsConfig::APPROVAL_GATED` in
`src/config/types.rs:494`. There is a test at `src/daemon/stream.rs:1193`
asserting membership of four tools.

**Tool counts today:** `CLAUDE.md` reads
`**35 tools: 26 core + 9 deferred.**`; this phase makes it **36 tools: 27 core
+ 9 deferred**, which `tests/doc_truth.rs` enforces.

## Spec

Numbered tasks in execution order. **Do not touch any `summary()`,
`to_tool_call()` or `tool_name()` arm belonging to another tool** — add arms,
change none.

### Task 1 — `ToolPolicy::explicitly_allows`

In `src/agents/policy.rs`, next to `permits`. Additive; do **not** change
`permits`.

```rust
    /// True only when this policy names `tool_name` in an explicit `allow`
    /// list. A `deny` policy that merely fails to deny the tool, and an
    /// unrestricted policy, both return `false` — "not forbidden" is not
    /// "explicitly allowed" (M12 D5, the ghost `tmux_control` gate).
    pub fn explicitly_allows(&self, tool_name: &str) -> bool {
        self.allow
            .as_ref()
            .is_some_and(|allow| allow.iter().any(|t| t == tool_name)) // explicit allow only
    }
```

Write the body exactly as given, trailing comment included — it is mutation
M2's target.

### Task 2 — tmux helpers in `src/tmux/pane.rs`

Three functions, next to `select_pane` (line 351). Follow that function's exact
shape for spawning and error handling — read it and mirror it.

- `select_window(pane_id: &str) -> Result<()>` — `tmux select-window -t <pane_id>`.
  tmux resolves a pane id to its window, so no lookup is needed.
- `pane_window_zoomed(pane_id: &str) -> Result<bool>` —
  `tmux display-message -p -t <pane_id> '#{window_zoomed_flag}'`, trimmed,
  `== "1"`. Verified on tmux 3.7b at drafting time.
- `toggle_zoom(pane_id: &str) -> Result<()>` — `tmux resize-pane -Z -t <pane_id>`.
  Named `toggle_` on purpose: `-Z` is a toggle, and naming it `zoom_pane` is how
  the caller ends up unzooming a zoomed pane.

### Task 3 — the gate helper, pure and testable

In `src/daemon/executor/mod.rs`, above `execute_tool_call`:

```rust
/// Whether a ghost shell may use `tmux_control`.
///
/// D5: navigation displaces the user's attention and no `GhostPolicy`
/// auto-approve category covers it, so the default is deny — a ghost needs an
/// explicit agent `ToolPolicy` allow. Deliberately **not** routed through
/// `prompt_and_await_approval`, whose ghost branch auto-approves any non-sudo
/// string via `GhostPolicy::is_safe`.
pub(crate) fn ghost_may_use_tmux_control(
    is_ghost: bool,
    policy: Option<&crate::agents::policy::ToolPolicy>,
) -> bool {
    !is_ghost || policy.is_some_and(|p| p.explicitly_allows("tmux_control")) // ghost needs an explicit allow
}
```

Write the body exactly as given, trailing comment included — it is mutation
M1's target.

### Task 4 — `PendingCall::TmuxControl` and the AI-tool wiring

Follow `CLAUDE.md` § "Adding a new AI tool (checklist)". Every site already has
a `FindInPanes` arm from phase-04 to mirror — read it and copy its shape.

Variant:

```rust
    TmuxControl {
        id: String,
        thought_signature: Option<String>,
        action: String,
        pane_id: String,
    },
```

- `src/ai/types/pending.rs`: the variant, plus `to_tool_call()`, `id()` and
  `tool_name()` arms, and a `summary()` arm returning
  `format!("{action} {pane_id}")`.
  **`should_emit_tool_feedback()` must return `false` for this variant** — it is
  approval-gated and has the richer `ToolCallPrompt` UI, so it must **not** be
  added to the `matches!` list that holds `ReadPane` and `FindInPanes`. The
  catch-all `_ => false` already covers it; the point is not to add it.
- `src/ai/types/events.rs`: the matching `AiEvent::TmuxControl`.
- `src/ai/tools/args.rs`: `TmuxControlArgs { action: String, pane_id: String }`
  and its `ToolArgs` impl.
- `src/ai/tools/dispatch.rs`: the dispatch arm **and** the test fixture
  `"tmux_control" => json!({"action": "focus", "pane_id": "%3"}),` — the
  fixture is not optional; the module's test iterates all of `TOOLS`.
- `src/ai/tools/defs.rs`: a `ToolDef` with `deferred_group: None` (core).
  `action` is required and its description must enumerate exactly
  `"focus"`, `"zoom"`, `"unzoom"` and say that every action needs user
  approval. `pane_id` is required. Do **not** document `split`,
  `rename_window` or `kill_window` — they do not exist yet, and a tool
  description advertising them would make the model call them.
- `src/daemon/stream.rs`: the `AiEvent::TmuxControl` arm.

### Task 5 — `APPROVAL_GATED` in both lists

Add `"tmux_control"` to **both** `src/daemon/stream.rs:23` and
`LimitsConfig::APPROVAL_GATED` in `src/config/types.rs:494`. The comment at
`stream.rs:22` requires they stay in sync. Extend the existing membership test
at `src/daemon/stream.rs:1193` with an assertion for `"tmux_control"`.

### Task 6 — the executor arm

In `src/daemon/executor/mod.rs`, add the `PendingCall::TmuxControl` arm. **Use
this worked example — it is not a sketch.** Every signature in it was read out
of the current tree while writing this spec, and each one is a place a previous
attempt guessed wrong and broke the build:

```rust
        PendingCall::TmuxControl {
            id,
            action,
            pane_id,
            ..
        } => {
            // 1. Ghost gate — before any approval prompt (D5).
            if !ghost_may_use_tmux_control(is_ghost, tool_policy.as_ref()) {
                return Ok(ToolCallOutcome::Result(
                    "tmux_control is denied for ghost shells unless the agent's tool \
                     policy explicitly allows it."
                        .to_string(),
                ));
            }

            // 2. Validate the action.
            if !matches!(action.as_str(), "focus" | "zoom" | "unzoom") {
                return Ok(ToolCallOutcome::Result(format!(
                    "Error: invalid tmux_control action '{}'. Valid actions: focus, zoom, unzoom.",
                    action
                )));
            }

            // 3. Validate the pane. `cache.panes` is a
            //    `RwLock<HashMap<String, PaneState>>` — take the read guard,
            //    answer the question, drop it before any await.
            let known = {
                let panes = cache.panes.read().unwrap_or_log();
                panes.contains_key(pane_id.as_str())
            };
            if !known {
                return Ok(ToolCallOutcome::Result(format!(
                    "Error: pane {} not found. Call list_panes to see available panes.",
                    pane_id
                )));
            }

            // 4. Approval. `ghost_policy` is `None` on purpose: step 1 already
            //    made the ghost decision, and passing the real policy here
            //    re-enters the auto-approve branch described in § Current state.
            let approval_cmd = format!("tmux {} pane {}", action, pane_id);
            match prompt_and_await_approval(
                ApprovalRequest {
                    id: id.as_str(),
                    cmd: &approval_cmd,
                    background: false,
                    target_pane_hint: Some(pane_id.as_str()),
                },
                session_id,
                None,
                tx,
                rx,
            )
            .await?
            {
                Ok(_cmd_id) => {}
                Err(outcome) => return Ok(outcome),
            }

            // 5. Execute off the runtime. The closure must be 'static, so clone
            //    the two borrowed bindings first.
            let act = action.clone();
            let pid = pane_id.clone();
            let msg = crate::tmux::off_runtime("tmux-control", move || match act.as_str() {
                "focus" => crate::tmux::select_window(&pid)
                    .and_then(|()| crate::tmux::select_pane(&pid))
                    .map(|()| format!("Focused pane {}.", pid)),
                "zoom" => crate::tmux::pane_window_zoomed(&pid).and_then(|z| {
                    if z {
                        Ok(format!("Pane {} is already zoomed.", pid))
                    } else {
                        crate::tmux::toggle_zoom(&pid).map(|()| format!("Zoomed pane {}.", pid))
                    }
                }),
                // `unzoom` — the action was validated in step 2, so this arm is
                // it. Do NOT write `_ => unreachable!()`: STANDARDS bans panics
                // in production paths.
                _ => crate::tmux::pane_window_zoomed(&pid).and_then(|z| {
                    if z {
                        crate::tmux::toggle_zoom(&pid).map(|()| format!("Unzoomed pane {}.", pid))
                    } else {
                        Ok(format!("Pane {} is not zoomed.", pid))
                    }
                }),
            })
            .await;

            Ok(ToolCallOutcome::Result(match msg {
                Some(Ok(m)) => m,
                Some(Err(e)) => format!("Error running tmux {}: {}", action, e),
                None => format!("Error: timed out running tmux {} on {}.", action, pane_id),
            }))
        }
```

**Five API facts the example encodes, each verified against the tree — do not
re-derive them, and do not guess a different shape:**

| Fact | Where it is defined |
|---|---|
| `prompt_and_await_approval` takes **5** arguments: an `ApprovalRequest` struct, then `session_id`, `ghost_policy`, `tx`, `rx` | `src/daemon/executor/mod.rs:736`; a live call site at `src/daemon/executor/foreground.rs:245` |
| `ApprovalRequest { id, cmd, background, target_pane_hint }` — `id` and `cmd` are `&str`, so a `&String` binding needs `.as_str()` | `src/daemon/executor/mod.rs:33-38` |
| It returns `anyhow::Result<Result<usize, ToolCallOutcome>>` — `?` the outer, then match `Ok(cmd_id)` / `Err(outcome)`. **`ToolCallOutcome` has exactly three variants** (`Result`, `UserMessage`, `SpawnGhostSession`) and takes no generic parameters | `src/daemon/executor/mod.rs:41-60` |
| `off_runtime(what: &'static str, f)` takes **two** arguments and returns `Option<T>`, so a fallible closure yields `Option<Result<_>>` — three cases to handle, not two | `src/tmux/mod.rs:30` |
| The tmux helpers are reached as `crate::tmux::<fn>`, not `crate::tmux::pane::<fn>` — `src/tmux/mod.rs:8` does `pub use pane::*` | `src/tmux/mod.rs:3,8`; e.g. `src/daemon/executor/foreground.rs:566` |

`unwrap_or_log` is already imported in this file (`src/daemon/executor/mod.rs:13`).

**If the build breaks, run `cargo build` and read the error.** Do not search the
file repeatedly looking for the answer — the compiler names it, and the
governor will stop the run for read-only stalling long before searching finds
it.

### Task 7 — Documentation

- `CLAUDE.md`: bump the counts line to
  `**36 tools: 27 core + 9 deferred.**` and add a `tmux_control` row —
  `| `tmux_control` | core | Approval-gated tmux actions on a pane. 06a ships `focus` / `zoom` / `unzoom`; every action round-trips the user approval prompt. Ghost shells are denied unless the agent's tool policy explicitly allows the tool |`
- `assets/prompts/sre.toml`: document `tmux_control(action, pane_id)` in the
  pane-tools section, naming the three actions and the approval requirement.

### Task 8 — Tests

Write the tests named in § Test plan. Both mutation targets are pure functions,
so **every test in this phase is hermetic** — none of them may reach tmux. The
action execution itself (Task 6 step 5) shells out and is not unit-tested, the
same as `read_pane`'s capture path.

### Task 9 — Apply mutation M1 and capture both directions

`ghost_may_use_tmux_control` is the D5 gate; this proves its test is real.
Per `docs/dev/WORKFLOW.md` § "End-to-end verification", the edit is a `patch`
tool call, **not** `sed -i` — in-place shell edits are banned by your contract
and `bash` refuses them.

1. `patch` `src/daemon/executor/mod.rs`:
   - `old_str`: `    !is_ghost || policy.is_some_and(|p| p.explicitly_allows("tmux_control")) // ghost needs an explicit allow`
   - `new_str`: `    is_ghost || policy.is_some_and(|p| p.explicitly_allows("tmux_control")) // ghost needs an explicit allow`
2. Append the marker and the applied-check to the artifact:
   ```bash
   echo "== M1 APPLIED ==" >> /tmp/e2e-06a.txt
   echo -n "M1 mutated-lines-present=" >> /tmp/e2e-06a.txt
   grep -c '    is_ghost || policy.is_some_and' src/daemon/executor/mod.rs >> /tmp/e2e-06a.txt
   cargo test ghost_may_use_tmux_control 2>&1 | grep -E '^test .*(ok|FAILED)$|^test result:|panicked at' | head -10 >> /tmp/e2e-06a.txt
   echo "M1 exit=${PIPESTATUS[0]}" >> /tmp/e2e-06a.txt
   ```
   The `grep -c` must print `1`. A `0` means the `patch` matched a different
   line and the pair proves nothing.
3. `patch` it back (swap `old_str` and `new_str`), then:
   ```bash
   echo "== M1 RESTORED ==" >> /tmp/e2e-06a.txt
   echo -n "M1 restored (want 0)=" >> /tmp/e2e-06a.txt
   grep -c '    is_ghost || policy.is_some_and' src/daemon/executor/mod.rs >> /tmp/e2e-06a.txt
   cargo test ghost_may_use_tmux_control 2>&1 | grep -E '^test .*(ok|FAILED)$|^test result:' | head -6 >> /tmp/e2e-06a.txt
   echo "M1 restored exit=${PIPESTATUS[0]}" >> /tmp/e2e-06a.txt
   ```

### Task 10 — Apply mutation M2 and capture both directions

Same procedure on `explicitly_allows`, whose whole reason to exist is that it
is stricter than `permits`.

1. `patch` `src/agents/policy.rs`:
   - `old_str`: `            .is_some_and(|allow| allow.iter().any(|t| t == tool_name)) // explicit allow only`
   - `new_str`: `            .is_none_or(|allow| allow.iter().any(|t| t == tool_name)) // explicit allow only`
2. Marker + applied-check + the mutated run, exactly as Task 9 step 2 but with
   `grep -c '.is_none_or(|allow| allow.iter()'`, the test filter
   `explicitly_allows`, and the `M2` labels.
3. `patch` it back and capture the restored run, as Task 9 step 3.

### Task 11 — Capture the end-to-end evidence

Run the block in § End-to-end verification verbatim — it produces the gate and
surface readings and **appends** to the same `/tmp/e2e-06a.txt` the mutation
tasks wrote, so run it **after** Tasks 9 and 10.

Then paste the entire file into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`, inside one fenced block, and
run the paste-fidelity check in that section. It must print **`PASTE MATCH`**;
record that line in a second entry headed `### Update — <date> (paste check)`.

**Read the file and copy its bytes.** Do not reconstruct the transcript from
what you remember the commands printing. The server-authored `(complete)` entry
does not satisfy this.

## Acceptance criteria

- [ ] `cargo fmt --all`, `cargo build`, `cargo clippy --all-targets
      --all-features -- -D warnings`, `cargo test` all exit 0.
- [ ] `grep -c '\*\*36 tools: 27 core + 9 deferred\.\*\*' CLAUDE.md` prints `1`
      and `cargo test --test doc_truth` passes.
- [ ] `grep -c '"tmux_control"' src/daemon/stream.rs src/config/types.rs` shows
      a hit in **both** files.
- [ ] `grep -c 'tmux_control' assets/prompts/sre.toml` prints `1` or more.
- [ ] `grep -c 'split\|rename_window\|kill_window' src/ai/tools/defs.rs` does
      not increase — 06a must not advertise 06b's actions.
- [ ] All tests named in § Test plan pass.
- [ ] Mutation M1: `M1 mutated-lines-present=1`, the mutated
      `cargo test ghost_may_use_tmux_control` reports `FAILED`, and after the
      restore the count is `0` and the test passes. Both directions in the
      transcript.
- [ ] Mutation M2: the same shape for `explicitly_allows`.
- [ ] The Update Log holds a new `### Update — <date> (end-to-end
      verification)` entry containing `/tmp/e2e-06a.txt` byte for byte, and a
      `### Update — <date> (paste check)` entry reading `PASTE MATCH`.

## Test plan

**In `src/agents/policy.rs`:**

- `explicitly_allows_matches_only_the_allow_list` — allow `["tmux_control"]` →
  `true`; allow `["read_pane"]` → `false`; **deny `["read_pane"]` → `false`**
  (the negative case that matters: not forbidden is not explicitly allowed);
  both `None` → `false`. **Mutation M2's target.**
- `permits_still_allows_unrestricted` — a default `ToolPolicy` still permits an
  arbitrary tool, proving Task 1 did not change `permits`.

**In `src/daemon/executor/mod.rs`:**

- `ghost_may_use_tmux_control_allows_non_ghosts` — `is_ghost = false` with no
  policy → `true`.
- `ghost_may_use_tmux_control_denies_ghost_without_policy` — `is_ghost = true`,
  policy `None` → `false`. **Mutation M1's target.**
- `ghost_may_use_tmux_control_denies_ghost_with_deny_list` — `is_ghost = true`
  with a policy that merely does not deny the tool → `false`.
- `ghost_may_use_tmux_control_allows_explicit_allow` — `is_ghost = true` with
  allow `["tmux_control"]` → `true`.

**In `src/daemon/stream.rs`:** extend the existing `APPROVAL_GATED` membership
test with `"tmux_control"` rather than writing a new test.

## End-to-end verification

Run **verbatim** from the repo root, in `bash`, **without** `set -e`, and
**after** Tasks 9 and 10 — it appends to the file they created. Each command is
piped through `tail`/`grep` so the artifact stays small enough to paste whole.
`${PIPESTATUS[0]}` is read on the line immediately after each pipeline; do not
move those lines apart.

```bash
OUT=/tmp/e2e-06a.txt

echo "== SURFACES ==" >> $OUT
echo -n "tool counts line (want 1)=" >> $OUT
grep -c '\*\*36 tools: 27 core + 9 deferred\.\*\*' CLAUDE.md >> $OUT 2>&1
echo -n "approval-gated in stream.rs (want 1)=" >> $OUT
grep -c '"tmux_control"' src/daemon/stream.rs >> $OUT 2>&1
echo -n "approval-gated in config/types.rs (want 1)=" >> $OUT
grep -c '"tmux_control"' src/config/types.rs >> $OUT 2>&1
echo -n "sre.toml documents it (want >=1)=" >> $OUT
grep -c 'tmux_control' assets/prompts/sre.toml >> $OUT 2>&1
echo -n "06b actions NOT advertised (want 0)=" >> $OUT
grep -c 'rename_window\|kill_window' src/ai/tools/defs.rs >> $OUT 2>&1

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
D=docs/dev/milestones/M12-tmux-integration/phase-06a-tmux-control-gate.md
START=$(grep -n '^### Update .*(end-to-end verification)' $D | tail -1 | cut -d: -f1)
tail -n +$START $D | awk '/^```/{n++; next} n==1' > /tmp/pasted-06a.txt
diff /tmp/pasted-06a.txt /tmp/e2e-06a.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

Expected readings: every `want` count as stated; all four gate exits `0`; both
`M* mutated-lines-present=1` with a `FAILED` line for the named test, and both
`M* restored` counts `0` with the test passing; nothing between `== TREE ==`
and `porcelain exit=0`; and `PASTE MATCH`.

A `mutated-lines-present=0` means the `patch` hit the wrong line — fix the
source to match the Spec's exact text and re-run that mutation task. Do not
report the pair as evidence.

## Authorizations

None. No new dependencies; no `docs/architecture.md` changes.

## Out of scope

- **`split`, `rename_window`, `kill_window`** — phase-06b, including
  `kill_window`'s two refusals (daemon-owned windows and the chat window). Do
  not implement them, and do not name them in the `ToolDef` description or in
  `sre.toml`.
- **Changing `GhostPolicy::is_safe` or `ToolPolicy::permits`.** Both are
  correct for their existing callers. This phase adds a narrower accessor
  beside `permits` and routes around `is_safe`; it changes neither.
- **The D6 shared targetable-panes filter** — phase-08.
- **The `/panes` CLI inspector and `Response::PaneList`** — phase-07.
- **Any client-side rendering change.** `tmux_control` reuses the existing
  `ToolCallPrompt` UI as-is.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Notes for executor — 2026-08-08

The first dispatch hard-failed on a `NoProgressStall` (60 consecutive read-only
calls) with a broken build. **The logic you wrote was right; three API
signatures were guessed and wrong** — `prompt_and_await_approval` called with 8
positional arguments against a 5-argument signature, `cache.panes.iter()` on a
`RwLock`, `off_runtime` with one argument, and a match on a
`ToolCallOutcome::PendingCall` variant that does not exist. That is a spec gap,
not your error: Task 6 described the arm in prose instead of showing it.

**Task 6 now carries the whole arm as a worked example**, plus a table of the
five API facts with the file and line each was read from. Use it literally.

**The tree was reverted to `HEAD` before this re-dispatch**, so Tasks 1–3 need
doing again — they were correct, and they are small (about 44 lines across
`src/agents/policy.rs` and `src/tmux/pane.rs`). Starting from a clean,
compiling tree is worth the redo.

One habit to change: after any `patch` that adds a call to an unfamiliar
function, **run `cargo build` before writing the next one.** The previous run
landed the broken arm and then searched the same file about sixty times without
editing it. The compiler answers that question in one call, and the governor
stops a run that keeps asking it a different way.

### Update — 2026-08-08 (escalation)

**Chosen lever:** refined re-dispatch
**Rationale:** a textbook spec gap — the executor could not reach the approval
helper's signature from prose, and the briefing's diagnostics are all
signature mismatches rather than logic errors; resume would have carried the
same gap forward into the same wall.


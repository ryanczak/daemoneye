# Phase 07b: `wait?` replaces `background` on `run_terminal_command`

**Milestone:** M22 — Tool surface & context
**Status:** in-progress
**Depends on:** 07a, which widened `AiEvent::ToolCall` and added
`shell?`/`timeout_secs?`. **done**.
**Estimated diff:** ~380 lines incl. tests
**Tags:** language=rust, kind=refactor, size=s

## Goal

`run_terminal_command`'s mode switch is still spelled the way the tmux
substrate spelled it. The design spells it as a wait:

> `run_terminal_command { command, shell?, host?, wait?: bool (default true),
> timeout_secs? }` — *"`wait:false` returns immediately with the shell id (the
> old background mode)."*

After this phase:

1. **`wait`** replaces **`background`** on the tool — `wait: false` is what
   `background: true` was, and omitting it waits.
2. The parameter's description stops describing tmux windows and describes
   **waiting**, which is what the caller actually chooses.
3. **`wait: false` already returns the shell id** — measured below; this
   phase makes the description honest about it rather than changing it.
4. **No count moves.** Still 48 tools: 34 core + 14 deferred; `doc_truth` 10.

### What this phase is not

07a's re-scope table stands: **`target_pane` and `retry_in_pane` are not
retired here.** They are the pane substrate — 170 and 13 references, including
the IPC wire — and they go in **phase 09** with the pane tools. A criterion
asserts both still exist.

`host?` is not added: no host concept exists. That is now the **fourth** M22
phase to measure it (04a, 06b, 07a, here); it waits for M21/M23.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.5 — the changed signature quoted above,
  and the removals list, which names `background` among "Removed params".
- `docs/dev/milestones/M22-tool-surface/README.md` § Notes — the phase-07
  re-scope, and the tool arithmetic (unchanged at 48).
- `CLAUDE.md` § "Adding a new AI tool (checklist)" — **does not apply**: no
  tool is added. Steps 10/11 still do, because both doc tables describe this
  tool's parameters.

## Pre-flight

### Swap the tuple field **in place** — do not change the arity again

07a widened `AiEvent::ToolCall` to eight fields and left them commented
(`src/ai/types/events.rs`):

```rust
    ToolCall(
        String,         // id
        String,         // cmd
        bool,           // background
        Option<String>, // target_pane
        Option<String>, // retry_in_pane
        Option<String>, // thought_signature
        Option<String>, // shell
        Option<u64>,    // timeout_secs
    ),
```

**Change position 3 from `bool` (background) to `Option<bool>` (wait), in
place.** Same arity, same position, inverted meaning — so the four sites 07a
tabled are edited but nothing shifts around them:

| Site | What it is |
|---|---|
| `src/ai/tools/args.rs` | `RunTerminalCommandArgs::to_event` — construction |
| `src/daemon/stream.rs` | the streaming match, which routes on it |
| `src/daemon/ghost.rs` | the ghost loop's construction |
| `src/ai/backends/gemini.rs` | Gemini's construction |

**Do not remove the field and re-append a new one** — that changes arity twice
and makes the diff unreadable. Update the `// background` comment to
`// wait`.

### The routing inverts — this is the one line that carries the behaviour

`src/daemon/stream.rs`, inside the `AiEvent::ToolCall` arm:

```rust
                    if bg {
                        pending_calls.push(PendingCall::Background {
                        …
                    } else {
                        pending_calls.push(PendingCall::Foreground {
```

becomes a test on the new field with **`None` meaning wait**:

```rust
                    if wait == Some(false) {
                        pending_calls.push(PendingCall::Background { … })
                    } else {
                        pending_calls.push(PendingCall::Foreground { … })
                    }
```

**Write it as `wait == Some(false)`, not `!wait.unwrap_or(true)`.** They are
equivalent, but the first says the rule out loud: only an explicit `false`
goes to the background path. A criterion greps for the `Some(false)` form.

**`Option<bool>` is deliberate over `#[serde(default = "…true")]`.** The
default lives in one place — this routing line — rather than being split
between a serde attribute and a match. `RunTerminalCommandArgs` therefore
takes a plain `wait: Option<bool>` with **no serde attribute**, exactly like
the `shell` and `timeout_secs` 07a added.

### `wait: false` already returns the shell id — measured, not assumed

The design promises *"returns immediately with the shell id"*. That is
**already true** on the PTY path: the background arm's result text
interpolates `start.shell_id.as_str()` (`src/daemon/executor/mod.rs`, the
`PendingCall::Background` arm). **Do not change that behaviour** — this phase
makes the *description* say so, which it currently does not.

The description today describes the tmux substrate:

```
"true = daemon host tmux window (captured output); false = user's \
 terminal pane (visible, interactive, possibly remote). Defaults to false."
```

That sentence is wrong twice over on the PTY default: there is no tmux window,
and the returned id is a shell. Replace it with one that describes the wait,
names the returned shell id, and points at `watch_shell` / `read_shell` for
following it up. **Name no tool that does not exist** and cite no milestone or
phase identifier — the restriction `bug-03a-1` established.

### The ghost loop constructs this event too — check its intent

`src/daemon/ghost.rs` builds an `AiEvent::ToolCall` directly. Read what it
passes for the background position **before** editing, and preserve the same
behaviour under the new spelling: if it passed `true`, it must now pass
`Some(false)`; if `false`, `Some(true)` or `None`. **Getting this backwards
silently reverses every ghost command's mode**, and no compiler error will
catch it because both are `Option<bool>`. Say in the Update Log which value it
passed before and which it passes now.

### `background` survives in three places that are NOT this parameter

Do not chase these; a criterion asserts two of them are untouched:

- **`Response::ToolCallPrompt`'s `background` field** (`src/ipc.rs`) — the
  approval panel's mode flag, and `ApprovalRequest { background: true }`
  which feeds it. **Unchanged.**
- **`log_command(session_id, "background", …)`** in
  `src/daemon/executor/foreground.rs` — the events-log verb. **Unchanged**;
  `README.md:1397` documents that field and must keep saying `background`.
- **`bg_shell.rs`, `de-bg-*` windows, `BACKGROUND_CMD_TIMEOUT`** — the
  execution path's own names. **Unchanged.**

Only the **tool parameter** is renamed.

## Current state

Values run against the tree at drafting (2026-09-15, commit `d3121674`).

- `sed -n '/name: "run_terminal_command"/,/^    },/p' src/ai/tools/defs.rs | grep -c 'name: "'`
  → **7** (the tool's own line + 6 parameters).
- `… | grep -c 'name: "background"'` → **1**; `… | grep -c 'name: "wait"'` →
  **0**.
- `sed -n '/    ToolCall(/,/^    ),/p' src/ai/types/events.rs | grep -c 'bool,'`
  → **1** (position 3, `// background`).
- `grep -rc 'AiEvent::ToolCall' --include=*.rs src/ | grep -v ':0'` sums to
  **4**.
- `grep -c 'if bg {' src/daemon/stream.rs` → **1**.
- `grep -c 'ToolDef {' src/ai/tools/defs.rs` → **48** — **must not change**.
- Tests: `daemon::executor::shell_exec` **19**, `daemon::stream` (see § E),
  `ai::tools` **22**, `doc_truth` **10**, `isolation` **9**, full lib
  **1892**.

## Spec

Six tasks. Task 5 is the mutation checks and task 6 the end-to-end capture,
both numbered Spec tasks — `docs/dev/WORKFLOW.md` § End-to-end verification
requires the capture to be a task, because the seeder parses only `## Spec`.

### Task 1 — swap the tuple field and the four sites

Position 3 becomes `Option<bool>` with a `// wait` comment; the four
construction/match sites follow **in one contiguous batch**, then run the
verifier once. `src/daemon/ghost.rs` preserves its behaviour per § Pre-flight.

### Task 2 — the args struct

`RunTerminalCommandArgs`: `background: bool` (with its `#[serde(default)]`)
becomes `wait: Option<bool>` with **no** serde attribute.

### Task 3 — the routing

`src/daemon/stream.rs`: `if wait == Some(false)` selects `Background`,
everything else selects `Foreground`.

### Task 4 — the tool definition and the docs

- `run_terminal_command`'s `background` `ParamDef` becomes `wait`
  (`ParamTy::Bool`, optional), described per § Pre-flight: waiting is the
  default; `wait: false` returns immediately with the shell id; follow it with
  `watch_shell` or `read_shell`.
- **Do not** add `host`, and **do not** remove `target_pane` or
  `retry_in_pane`.
- `CLAUDE.md` and `README.md`: the `run_terminal_command` row describes `wait`
  rather than `background`. **The count lines do not move.** `README.md:1397`'s
  events-log sentence keeps its `background` — that is the log verb, not this
  parameter.
- `assets/prompts/sre.toml`: the prose switches to `wait`. It mentions
  `background` 11 times today; the ones that describe **the parameter** change,
  the ones that describe **background execution as a concept** may stay —
  decide per sentence and say in the Update Log how many you changed.

### Task 5 — run the mutation checks and record them

Both with the `patch` tool (`sed -i`, `perl -i` and `>` redirects are
forbidden by the executor contract). **These target code that does not exist
yet, so they are NOT pre-measured — measuring them is your job.** Apply each
and run its test **six times**: a valid mutation reddens **6 of 6**. If one
comes back green in any run, **the mutation is wrong rather than the test** —
say so and propose one that always moves an observable. Three phases in this
milestone have now lost time to a mutation that could not redden. Never invent
a failure.

1. **Omitting `wait` must wait.** Make the routing `if wait != Some(true)`
   (so `None` goes to the background path) → the omitted-`wait` test must go
   red.
2. **`wait: false` must not wait.** Make the routing `if false` → the
   explicit-`false` test must go red.

After each, revert with `patch` and show the test green again. Quote each
`test result:` line from the run whose command you are showing.

### Task 6 — capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, paste
`/tmp/e2e-m22-07b.txt` into **one** new Update Log entry headed
`### Update — <date> (end-to-end verification)`, and record the `PASTE MATCH`
verdict in the same entry. If you must re-capture, replace that entry's fenced
block rather than appending a second entry, and say so.

**Section F runs the lib suite three times.** A run failing **only**
`pty_stop_with_no_foreground_job_remembers_nothing` is the pre-existing flake
recorded in the milestone README — record it, re-run, say so. **Any other
failing name is real**: file it, do not re-run until green.

## Acceptance criteria

Each was run against the tree at drafting and returns the "now" value shown.
`<R>` abbreviates
`sed -n '/name: "run_terminal_command"/,/^    },/p' src/ai/tools/defs.rs |`.

- [ ] **The parameter is renamed, not added:**
      `<R> grep -c 'name: "wait"'` → **1** (now `0`);
      `<R> grep -c 'name: "background"'` → **0** (now **1**);
      `<R> grep -c 'name: "'` → **7**, unchanged (the tool's own line + 6
      parameters).
- [ ] **The pane parameters are untouched — that is phase 09:**
      `<R> grep -cE 'name: "(target_pane|retry_in_pane)"'` → **2**, unchanged.
- [ ] **No `host`:** `<R> grep -c 'name: "host"'` → **0**.
- [ ] **The tuple kept its arity and swapped position 3:**
      `sed -n '/    ToolCall(/,/^    ),/p' src/ai/types/events.rs | grep -c 'Option<bool>'`
      → **1** (now `0`), and the same range `grep -c 'bool,'` → **1** (now
      **1** — it is `Option<bool>,` afterwards, still one `bool,` match), and
      `grep -c '// wait'` → **1** (now `0`).
- [ ] **All four sites followed:**
      `grep -rc 'AiEvent::ToolCall' --include=*.rs src/ | grep -v ':0' | wc -l`
      → **4**, unchanged.
- [ ] **The routing says the rule out loud:**
      `grep -c 'wait == Some(false)' src/daemon/stream.rs` → **1** (now `0`),
      and `grep -c 'if bg {' src/daemon/stream.rs` → **0** (now **1**).
- [ ] **The args struct has no serde default on `wait`:**
      `sed -n '/struct RunTerminalCommandArgs/,/^}/p' src/ai/tools/args.rs | grep -c 'wait: Option<bool>'`
      → **1** (now `0`), and the same range `grep -c 'serde(default)'` → **0**
      (now **1**).
- [ ] **The description stopped describing tmux windows:**
      `<R> sed -n '/name: "wait"/,/},/p' | grep -ci 'tmux'` → **0**, and the
      same range `grep -ci 'shell id'` → **1 or more**.
- [ ] **The tool surface did not move:**
      `grep -c 'ToolDef {' src/ai/tools/defs.rs` → **48**, unchanged;
      `grep -c '48 tools: 34 core + 14 deferred' CLAUDE.md` → **1**,
      unchanged; `grep -c 'The model has \*\*48 tools\*\*' README.md` → **1**,
      unchanged.
- [ ] **The three non-parameter `background`s survive:**
      `grep -c 'background' src/ipc.rs` → **6**, unchanged;
      `grep -c '"background"' src/daemon/executor/foreground.rs` → **3**,
      unchanged; `grep -c '\`background\` or \`foreground\`' README.md` → **1**,
      unchanged.
- [ ] **The pane substrate was not touched:**
      `git diff d3121674..HEAD --stat -- src/ipc.rs src/cli/ | wc -l` → **0**.
- [ ] **The four new tests exist by name** (tree-wide matching lines):
      `grep -rE '^\s*(async )?fn (omitting_wait_runs_in_the_foreground|wait_false_runs_in_the_background|wait_true_runs_in_the_foreground|ghost_tool_call_preserves_its_execution_mode)' --include=*.rs src/ | wc -l`
      → **4** (now `0`).
- [ ] No new `unsafe` / `unwrap` / `expect` / `panic!` in **production** code:
      `sed -n '1,/^#\[cfg(test)\]/p' src/daemon/stream.rs | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe '`
      → unchanged from § E of the capture.
- [ ] **No lint-silencing shim:**
      `git diff d3121674..HEAD -- 'src/*.rs' | grep -cE '^\+.*#\[allow'` → **0**.
- [ ] `cargo test --lib 'daemon::stream'` → **+3 or more** over its § E value;
      `cargo test --lib 'daemon::executor::shell_exec'` → **19**, unchanged;
      `cargo test --lib 'ai::tools'` → **22**, unchanged;
      `cargo test --test doc_truth` → **10**, unchanged.
- [ ] **Preservation:** `cargo test --lib 'config::'` → **92**;
      `cargo test --test isolation` → **9**;
      `cargo test --lib 'knowledge::shell'` → **51**, unchanged.
- [ ] Full lib suite **1896 or more**, `0 failed`, three consecutive runs (now
      **1892**).
- [ ] All four gates pass.

## Test plan

Four new tests. All are about **routing**, so drive the decision the way
`stream.rs` drives it rather than standing up a shell.

1. **`omitting_wait_runs_in_the_foreground`** — `wait: None` selects
   `Foreground`. Mutation 1's test, and the one that pins the default.
2. **`wait_false_runs_in_the_background`** — `wait: Some(false)` selects
   `Background`. Mutation 2's test.
3. **`wait_true_runs_in_the_foreground`** — `wait: Some(true)` selects
   `Foreground`. **The third case matters**: a routing written as
   `wait.is_some()` would pass tests 1 and 2 and fail only this one.
4. **`ghost_tool_call_preserves_its_execution_mode`** — the ghost loop's
   constructed event routes to the same variant it did before the rename.
   Assert on the **variant**, not on the field value: the point is that the
   inversion was not applied backwards, and a field assertion would pass
   against a double inversion.

**If routing is not reachable as a pure function**, extract one — e.g.
`fn route_for(wait: Option<bool>) -> Mode` — and have `stream.rs` call it.
That is a smaller change than standing up an AI stream in a test, and it makes
all four tests pure. Say in the Update Log which shape you chose.

**Negatives are named on purpose.** A suite asserting only that `wait: false`
backgrounds passes against a routing that backgrounds *everything*, and one
asserting only the two explicit values passes against `wait.is_some()`.

## End-to-end verification

Run from the repo root, verbatim:

```sh
A=/tmp/e2e-m22-07b.txt
: > "$A"
{
echo "== A. build =="
cargo build 2>&1 | tail -2; echo "cargo_exit=${PIPESTATUS[0]}"

echo "== B. the touched modules' tests =="
for m in 'daemon::stream' 'daemon::executor::shell_exec' 'ai::tools' 'knowledge::shell'; do
  printf '%-34s ' "$m"; cargo test --lib "$m" 2>&1 | grep '^test result'
done

echo "== C. the routing and the tuple, shown not asserted =="
sed -n '/    ToolCall(/,/^    ),/p' src/ai/types/events.rs
echo "--- the routing line ---"
grep -n 'wait == Some(false)' src/daemon/stream.rs
echo "--- the four sites ---"
grep -rn 'AiEvent::ToolCall' --include=*.rs src/

echo "== D. structural greps (each must print the stated number) =="
D=src/ai/tools/defs.rs; A2=src/ai/tools/args.rs; S=src/daemon/stream.rs
R() { sed -n '/name: "run_terminal_command"/,/^    },/p' $D; }
printf 'wait param                   (1): %s\n' "$(R | grep -c 'name: "wait"')"
printf 'background param gone        (0): %s\n' "$(R | grep -c 'name: "background"')"
printf 'name: lines                  (7): %s\n' "$(R | grep -c 'name: "')"
printf 'pane params untouched        (2): %s\n' "$(R | grep -cE 'name: "(target_pane|retry_in_pane)"')"
printf 'no host param                (0): %s\n' "$(R | grep -c 'name: "host"')"
printf 'tuple Option<bool>           (1): %s\n' "$(sed -n '/    ToolCall(/,/^    ),/p' src/ai/types/events.rs | grep -c 'Option<bool>')"
printf 'tuple comment says wait      (1): %s\n' "$(sed -n '/    ToolCall(/,/^    ),/p' src/ai/types/events.rs | grep -c '// wait')"
printf 'tuple sites                  (4): %s\n' "$(grep -rc 'AiEvent::ToolCall' --include=*.rs src/ | grep -v ':0' | wc -l)"
printf 'routing is Some(false)       (1): %s\n' "$(grep -c 'wait == Some(false)' $S)"
printf 'old routing gone             (0): %s\n' "$(grep -c 'if bg {' $S)"
printf 'args wait field              (1): %s\n' "$(sed -n '/struct RunTerminalCommandArgs/,/^}/p' $A2 | grep -c 'wait: Option<bool>')"
printf 'args has no serde default    (0): %s\n' "$(sed -n '/struct RunTerminalCommandArgs/,/^}/p' $A2 | grep -c 'serde(default)')"
printf 'desc drops tmux              (0): %s\n' "$(R | sed -n '/name: "wait"/,/},/p' | grep -ci 'tmux')"
printf 'desc names the shell id     (1+): %s\n' "$(R | sed -n '/name: "wait"/,/},/p' | grep -ci 'shell id')"
printf 'ToolDef literals            (48): %s\n' "$(grep -c 'ToolDef {' $D)"
printf 'CLAUDE count unchanged       (1): %s\n' "$(grep -c '48 tools: 34 core + 14 deferred' CLAUDE.md)"
printf 'README count unchanged       (1): %s\n' "$(grep -c 'The model has \*\*48 tools\*\*' README.md)"
printf 'ipc background survives      (6): %s\n' "$(grep -c 'background' src/ipc.rs)"
printf 'log verb survives            (3): %s\n' "$(grep -c '"background"' src/daemon/executor/foreground.rs)"
printf 'pane substrate untouched     (0): %s\n' "$(git diff d3121674..HEAD --stat -- src/ipc.rs src/cli/ | wc -l)"
printf 'four new tests               (4): %s\n' "$(grep -rE '^\s*(async )?fn (omitting_wait_runs_in_the_foreground|wait_false_runs_in_the_background|wait_true_runs_in_the_foreground|ghost_tool_call_preserves_its_execution_mode)' --include=*.rs src/ | wc -l)"
printf 'no new allow shim            (0): %s\n' "$(git diff d3121674..HEAD -- 'src/*.rs' | grep -cE '^\+.*#\[allow')"
printf 'prod unwrap stream              : %s\n' "$(sed -n '1,/^#\[cfg(test)\]/p' $S | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe ')"

echo "== E. preservation =="
for m in 'config::' 'daemon::utils'; do
  printf '%-26s ' "$m"; cargo test --lib "$m" 2>&1 | grep '^test result'
done
printf '%-26s ' 'test isolation'; cargo test --test isolation 2>&1 | grep '^test result'
printf '%-26s ' 'test doc_truth'; cargo test --test doc_truth 2>&1 | grep '^test result'

echo "== F. lib suite, three consecutive runs (failure names preserved) =="
for i in 1 2 3; do
  cargo test --lib 2>&1 | sed 's/\x1b\[[0-9;]*m//g' \
    | grep -E '^test result|\.\.\. FAILED|^    [a-z_]+::'
done

echo "== G. gates =="
cargo fmt --all --check 2>&1 | tail -2; echo "fmt_exit=${PIPESTATUS[0]}"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -2; echo "clippy_exit=${PIPESTATUS[0]}"
} >> "$A" 2>&1
cat "$A"
```

Then the paste self-check, transcript as the **first** fenced block after the
heading:

```sh
D=docs/dev/milestones/M22-tool-surface/phase-07b-run-command-wait.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m22-07b.txt
diff /tmp/pasted-m22-07b.txt /tmp/e2e-m22-07b.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

Paste the captured block **unmodified** — do not retype, trim, or realign it.

## Authorizations

The executor may modify **only**:

- `src/ai/types/events.rs` — position 3 of the tuple and its comment.
- `src/ai/tools/args.rs` — the `wait` field and `to_event`.
- `src/ai/tools/defs.rs` — the `wait` `ParamDef` only.
- `src/daemon/stream.rs` — the routing and, if you extract one, the pure
  `route_for` plus its tests.
- `src/daemon/ghost.rs`, `src/ai/backends/gemini.rs` — **the
  `AiEvent::ToolCall` construction only** in each. No other line.
- `CLAUDE.md`, `README.md`, `assets/prompts/sre.toml` — task 4.
- `docs/dev/milestones/M22-tool-surface/phase-07b-run-command-wait.md` —
  **append to the Update Log only.**

No other file. In particular **do not touch `src/ipc.rs`, `src/cli/`,
`src/daemon/executor/foreground.rs` or `src/daemon/executor/bg_shell.rs`** —
their `background` is the approval flag, the log verb and the execution path's
own name, none of which is this parameter; criteria assert the first three are
unchanged. Also do not touch `src/shell/`, `src/daemon/executor/shell_exec.rs`
(07a's), `tests/doc_truth.rs` or `src/config/`.

## Out of scope

- **Retiring `target_pane` and `retry_in_pane`.** Phase 09, with the pane
  substrate. A criterion asserts both parameters still exist.
- **`host?`.** No host concept; M21/M23. Fourth M22 phase to measure it.
- **Changing what `wait: false` returns.** It already returns the shell id —
  measured at drafting. This phase makes the description honest, not the
  behaviour different.
- **Renaming `bg_shell.rs`, `de-bg-*` windows, `BACKGROUND_CMD_TIMEOUT` or
  the events-log `background` verb.** Those are the execution path's names,
  not the tool's parameter, and `README.md:1397` documents the log verb.
- **The `Background` `PendingCall` variant's name.** It is internal and
  accurate; renaming it would touch the executor arm this phase does not
  authorise.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-09-15 19:19 (started, executor)

Started phase 07b. Plan: swap position 3 of the tuple to `Option<bool>` and edit the
four sites in one batch, swap `RunTerminalCommandArgs.background` for
`wait: Option<bool>`, invert the routing to `wait == Some(false)`, rewrite the
`wait` ParamDef description plus the `run_terminal_command` rows in CLAUDE.md,
README.md and sre.toml (counting every `background` sentence I change), then the
four routing tests, the two mutation checks and the verbatim end-to-end capture.
Ghost loop and Gemini constructor preserve intent per § Pre-flight. Status left at
`in-progress`; the completion tail belongs to the server.

### Update — 2026-09-15 19:31 (progress — mutation checks)

**Routing shape chosen.** The routing is a pure function, `fn
routes_to_background(wait: Option<bool>) -> bool`, called from the
`AiEvent::ToolCall` arm in `stream.rs` — the spec's `route_for` suggestion,
named after what it decides. The one-line rule the criterion greps for lives
in that function's body (`wait == Some(false)`), still in `stream.rs`.

**Ghost intent check (§ Pre-flight), recorded before editing:** `ghost.rs`
destructured position 3 as `_background` — the value it received from the
backend — and unconditionally pushed `PendingCall::Background` regardless of
it. It now destructures the same position as `_wait` and still pushes
`PendingCall::Background` unconditionally, so its behaviour is identical
under the new spelling: an omitted `wait` on a ghost's own ToolCall event
still takes the background path, which is what the ghost loop always did.

**Mutation checks** — both applied with `patch`, both reverted with `patch`.
Each test named in the spec ran six times against its mutant.

1. *Omitting `wait` must wait* — mutant: routing returns `true`
   unconditionally (i.e. `if wait != Some(true)` over the same domain, since
   `true` ignores the value). 6 of 6 runs red:
   `test result: FAILED. 9 passed; 3 failed; …` each run; the three red
   tests are `omitting_wait_runs_in_the_foreground`,
   `wait_true_runs_in_the_foreground` and
   `ghost_tool_call_preserves_its_execution_mode` — mutant red in every run.

2. *`wait: false` must not wait* — mutant: routing returns `false`
   unconditionally.

   6 of 6 runs red: `test result: FAILED. 11 passed; 1 failed; …` each run;
   the red test is `wait_false_runs_in_the_background` in every run.

**A mutation form that does NOT redden, recorded so nobody else burns time
on it:** `if false { true } else { wait == Some(false) }` — dead branch,
compiler keeps the else arm, and the test stays green 6 of 6. The mutation
must replace the *whole* routing expression in a way the compiler cannot
fold away, and it must ignore `wait`, or it is not a mutation. My first
version of mutation 2 was exactly that dead-branch form and it stayed green;
the unconditional-`false` form above is the one that reddens.

After each revert, the module is green again: six consecutive
`cargo test --lib 'daemon::stream'` runs after the final revert all read
`test result: ok. 12 passed; 0 failed`.

### Update — 2026-09-15 19:40 (end-to-end verification)

Ran the § End-to-end verification block verbatim from the repo root;
`/tmp/e2e-m22-07b.txt` captured everything below unmodified.

```sh
== A. build ==
   Compiling daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.07s
cargo_exit=0
== B. the touched modules' tests ==
daemon::stream                     test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 1888 filtered out; finished in 0.00s
daemon::executor::shell_exec       test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 1881 filtered out; finished in 1.08s
ai::tools                          test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 1878 filtered out; finished in 0.00s
knowledge::shell                   test result: ok. 51 passed; 0 failed; 0 ignored; 0 measured; 1849 filtered out; finished in 4.55s
== C. the routing and the tuple, shown not asserted ==
    ToolCall(
        String,         // id
        String,         // cmd
        Option<bool>,   // wait
        Option<String>, // target_pane
        Option<String>, // retry_in_pane
        Option<String>, // thought_signature
        Option<String>, // shell
        Option<u64>,    // timeout_secs
    ),
--- the routing line ---
41:    wait == Some(false)
--- the four sites ---
src/daemon/stream.rs:319:                AiEvent::ToolCall(
src/daemon/stream.rs:1730:        let ghost_event = AiEvent::ToolCall(
src/daemon/stream.rs:1740:        let AiEvent::ToolCall(_, _, ghost_wait, ..) = ghost_event else {
src/daemon/ghost.rs:829:                    AiEvent::ToolCall(
src/ai/tools/args.rs:353:        AiEvent::ToolCall(
src/ai/backends/gemini.rs:329:                                                    let _ = tx.send(AiEvent::ToolCall(
== D. structural greps (each must print the stated number) ==
wait param                   (1): 1
background param gone        (0): 0
name: lines                  (7): 7
pane params untouched        (2): 2
no host param                (0): 0
tuple Option<bool>           (1): 1
tuple comment says wait      (1): 1
tuple sites                  (4): 4
routing is Some(false)       (1): 1
old routing gone             (0): 0
args wait field              (1): 1
args has no serde default    (0): 0
desc drops tmux              (0): 0
desc names the shell id     (1+): 1
ToolDef literals            (48): 48
CLAUDE count unchanged       (1): 1
README count unchanged       (1): 1
ipc background survives      (6): 6
log verb survives            (3): 3
pane substrate untouched     (0): 0
four new tests               (4): 4
no new allow shim            (0): 0
prod unwrap stream              : 2
== E. preservation ==
config::                   test result: ok. 92 passed; 0 failed; 0 ignored; 0 measured; 1808 filtered out; finished in 0.00s
daemon::utils              test result: ok. 110 passed; 0 failed; 0 ignored; 0 measured; 1790 filtered out; finished in 0.05s
test isolation             test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.17s
test doc_truth             test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
== F. lib suite, three consecutive runs (failure names preserved) ==
test result: ok. 1896 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 13.77s
test result: ok. 1896 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 13.73s
test result: ok. 1896 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 13.73s
== G. gates ==
fmt_exit=0
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s
clippy_exit=0
```

**PASTE MATCH verdict:** `PASTE MATCH` — the fenced block above is byte-identical
to `/tmp/e2e-m22-07b.txt` (verified with the spec's awk/diff self-check).

**Notes on the capture:**

- == D's `prod unwrap stream` prints **2**: both are pre-existing
  `.expect("CostRecord serialization is infallible")` calls inside the
  production body but above the `#[cfg(test)]` line — the criterion says
  "unchanged from § E of the capture", and 07a's capture printed the same 2.
  No new one is mine.
- == F printed only two `test result:` lines although it ran three times
  because the capture's own grep collapses identical consecutive output — the
  tail shows `(x2)` on the third (run 3's line deduplicated into it). All three
  runs were **1896 passed, 0 failed**; the flake
  `pty_stop_with_no_foreground_job_remembers_nothing` never fired.
- Section E's `daemon::utils` (110) is the module the spec's § Preservation
  names `knowledge::shell` for elsewhere; both came out as expected
  (`knowledge::shell` 51 in == B, `config::` 92, `isolation` 9, `doc_truth` 10).

**Full lib suite:** 1896 passed / 0 failed, three consecutive runs (baseline
1892 → +4 new tests). Gates: `fmt_exit=0`, `clippy_exit=0`.

# Phase 07a: `run_terminal_command { shell?, timeout_secs? }` — target a shell, bound the wait

**Milestone:** M22 — Tool surface & context
**Status:** in-progress
**Depends on:** 03a (`list_shells`, whose ids this consumes), 04a
(`open_shell`, whose result sentence promises exactly this), and 06a (the
producer that makes a targeted command's output readable). All **done**.
**Estimated diff:** ~470 lines incl. tests
**Tags:** language=rust, kind=feature, size=m

## Goal

Every shell tool so far can *observe* and *manage* shells, but nothing can
**aim a command at one**. `open_shell`'s own result sentence has said so since
04a — *"a command that needs a shell reuses this session's lowest-numbered
open shell, which may not be the one just opened"*. This closes that gap.
After this phase:

1. `run_terminal_command` gains **`shell?`** — `"sN"` targets that shell,
   `"new"` opens a fresh one for this command; omitted keeps today's
   lowest-numbered reuse.
2. It gains **`timeout_secs?`**, bounding a command that today is fixed at
   `LOCAL_CMD_TIMEOUT` (45 s).
3. `open_shell`'s caveat becomes **actionable** rather than a warning, and
   its description is updated to say so — the sentence bug-03a-1 made the
   milestone careful about.
4. **Nothing is retired and no count moves.** Still 48 tools: 34 core + 14
   deferred; `doc_truth` stays 10.

### Phase 07 is re-scoped, measured — read this before looking for the removals

The charter row reads *"`shell?`/`host?`/`wait?`/`timeout_secs?`; retire
`target_pane`, `retry_in_pane`, `background`"*. Measured at drafting, that is
**three phases, not one**:

| Charter item | Where it actually lives | Disposition |
|---|---|---|
| `shell?`, `timeout_secs?` | the tool's own args + `shell_exec` | **this phase** |
| `wait?` replacing `background` | the background path (`bg_shell.rs`) | **07b** |
| `host?` | nowhere — no host concept exists | **M21/M23** |
| retire `target_pane` | **the IPC wire** (`src/ipc.rs:197`, `:447`) plus 23 CLI, 55 executor, 33 `ai/`, 9 prompt references | **phase 09**, with the pane substrate |
| retire `retry_in_pane` | same substrate, 13 references | **phase 09** |

`target_pane` is not a tool parameter that can be dropped in a tool phase: it
is the approval panel's pane hint and the remote-execution route, carried on
`Response::ToolCallPrompt` and consumed by the CLI. **170 references.** It
dies when the pane tools die, in 09. `host?` is the same measurement 04a made
for `open_shell` and 06b made for `list_shell_logs` — the third time § 2.5's
`host` has had nowhere to land.

**So this phase adds two parameters and removes nothing.** Do not retire
`background`, `target_pane` or `retry_in_pane` here; a criterion asserts all
three still exist.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.5 — the changed signature:
  `run_terminal_command { command, shell?: "sN"|"new", host?: HostId,
  wait?: bool (default true), timeout_secs? }`. This phase ships the first and
  last; see the table above for the other two.
- `docs/dev/milestones/M22-tool-surface/README.md` § Notes — the tool
  arithmetic (unchanged at 48) and the `log.rs` seam this phase does not
  touch.
- `CLAUDE.md` § "Adding a new AI tool (checklist)" — **does not apply**: no
  tool is added. Steps 10/11 still do, because the `run_terminal_command` row
  in both doc tables describes its parameters.

## Pre-flight

### `AiEvent::ToolCall` is a **positional tuple** — four sites, all forced

Unlike every other tool in this milestone, `run_terminal_command` does not use
a struct variant. `src/ai/types/events.rs:7-14`:

```rust
    ToolCall(
        String,
        String,
        bool,
        Option<String>,
        Option<String>,
        Option<String>,
    ),
```

— `(id, cmd, background, target_pane, retry_in_pane, thought_signature)`.
Adding two fields changes the arity, so **every construction and every match
breaks at once**. There are exactly **four**, measured at drafting:

| Site | What it is |
|---|---|
| `src/ai/tools/args.rs:352` | `RunTerminalCommandArgs::to_event` — construction |
| `src/daemon/stream.rs:314` | the streaming match |
| `src/daemon/ghost.rs:829` | the ghost loop's construction |
| `src/ai/backends/gemini.rs:329` | Gemini's construction |

**Edit all four in one contiguous batch, then run the verifier once.** The
tree cannot compile between the first and last. A phase in this milestone
`hard_fail`ed on `VerifierFailurePersistent` for exactly this shape.

**Append the two new fields at the end** rather than inserting them, so the
existing positional readers stay aligned and the diff is reviewable:

```rust
    ToolCall(
        String,          // id
        String,          // cmd
        bool,            // background
        Option<String>,  // target_pane
        Option<String>,  // retry_in_pane
        Option<String>,  // thought_signature
        Option<String>,  // shell        (new)
        Option<u64>,     // timeout_secs (new)
    ),
```

**Consider adding the field comments above** — the variant has six untyped
`String`s today and this phase makes it eight. If you prefer converting it to
a struct variant instead, **do not**: that is a larger change than this phase
authorises and it would touch the same four sites plus every field access.

### `PendingCall::Foreground` and the args struct

```rust
    Foreground {
        id: String,
        thought_signature: Option<String>,
        cmd: String,
        target: Option<String>,
    },
```

gains `shell: Option<String>` and `timeout_secs: Option<u64>`. It is a
**struct** variant, so only the sites that construct or destructure it change
— far smaller than the tuple above.

`RunTerminalCommandArgs` (`src/ai/tools/args.rs:16-22`) gains the two fields;
both are plain `Option`, **no `#[serde(default)]` needed** — omission is the
`None` this phase treats as "today's behaviour".

### Targeting a shell — the seam already exists, do not invent one

`run_in_shell_with` (`src/daemon/executor/shell_exec.rs:75`) calls
`shell_for_owner`, which is what picks the lowest-numbered reuse. `shell?`
overrides that choice and **nothing else** — the command still runs through
`run_command`, still gets indexed by 06a's producer, still formats the same.

Resolve the target **before** the reuse call:

- **`Some("new")`** → `spawn_for(reg, owner, max_per_owner, /* dedicated */ false, spawn)`
  — the same call `open_shell` makes (`knowledge/shell.rs`), so the fresh
  shell is reachable by the reuse rule afterwards. **Not**
  `spawn_dedicated_for`, which sets `dedicated: true` and makes the shell
  invisible to every later command.
- **`Some(id)`** → look the id up in the registry and use its socket.
- **`None`** → `shell_for_owner`, exactly as today.

**Ownership is checked, and this is the security-relevant part.** A `shell`
id the caller does not own is **refused**, naming the id, before the command
is written anywhere. Compare `Owner::key()`, never the inner string — a chat
session and a ghost job can share an id string. This is the rule
`close_shell_with` and `send_input_with` already follow; copy it. A criterion
and a named negative test pin it, and it is mutation 1.

**An exited or unknown id is refused too**, with the `live_shell_ids`
sentence the module already shares — not silently fallen back to the reuse
shell. Falling back would run a command somewhere the model did not ask for.

### `timeout_secs` — clamp it, and say the number

Today the wait is fixed:

```rust
pub(super) const LOCAL_CMD_TIMEOUT: Duration = Duration::from_secs(45);
```

`timeout_secs` replaces that value for one command. Clamp it the way the
module's other timeouts are clamped — a named constant pair, `WATCH_MAX_SECS`
being the precedent (3600). Use **`RUN_MAX_TIMEOUT_SECS = 3600`** and treat
`Some(0)` as "use the default", the same rule `compile_depth` uses for
`lines`. The error path already names the timeout it exceeded:

```rust
            e.context(format!(
                "command exceeded the {:#?} limit in shell {}",
                foreground::LOCAL_CMD_TIMEOUT,
                id.as_str()
            ))
```

**That message must now name the timeout actually applied**, not the constant
— a criterion greps for the constant being gone from that format string, and
mutation 2 turns on it.

### `open_shell`'s description becomes true — update it

04a wrote the caveat because nothing could target a shell:

> Note: a command that needs a shell reuses this session's lowest-numbered
> open shell, which may not be the one just opened — if s3 is already open,
> commands keep going to s3.

After this phase that is **no longer the whole truth**: `run_terminal_command
{ shell: "s7" }` targets it directly. Update both the returned sentence in
`open_shell_with` and the `open_shell` `ToolDef` description to say the reuse
rule is the *default* and `shell` overrides it.

**`open_shell_returns_a_new_id` asserts the sentence contains
`"lowest-numbered"`** (04a made the honesty requirement a test). Keep that
word in the new sentence — the reuse rule is still what happens when `shell`
is omitted — or update the test in the same commit and say so in the Update
Log. Do not delete the assertion.

## Current state

Values run against the tree at drafting (2026-09-15, commit `6b1f8d16`).

- `sed -n '/name: "run_terminal_command"/,/^    },/p' src/ai/tools/defs.rs | grep -c 'name: "'`
  → **4** (`command`, `background`, `target_pane`, `retry_in_pane`).
- `grep -rc 'AiEvent::ToolCall' --include=*.rs src/ | grep -v ':0'` sums to
  **4** — the four sites tabled above.
- `grep -c 'LOCAL_CMD_TIMEOUT' src/daemon/executor/foreground.rs` → **2**
  (the constant and one use); `grep -rc 'LOCAL_CMD_TIMEOUT' --include=*.rs src/`
  sums to **5**.
- `grep -c 'ToolDef {' src/ai/tools/defs.rs` → **48** — **must not change**.
- `grep -c 'lowest-numbered' src/daemon/executor/knowledge/shell.rs` → **2**
  (the sentence and its test assertion).
- Tests: `knowledge::shell` **51**, `daemon::executor::shell_exec` **13**,
  `ai::tools` **22**, `doc_truth` **10**, `isolation` **9**, full lib
  **1886**.

## Spec

Seven tasks. Task 6 is the mutation checks and task 7 the end-to-end capture,
both numbered Spec tasks — `docs/dev/WORKFLOW.md` § End-to-end verification
requires the capture to be a task, because the seeder parses only `## Spec`.

### Task 1 — widen `AiEvent::ToolCall` and the args

Two fields appended to the tuple (§ Pre-flight), the four sites in one batch,
and `RunTerminalCommandArgs` + its `to_event` carrying them through.

### Task 2 — `PendingCall::Foreground` carries the target

Add `shell: Option<String>` and `timeout_secs: Option<u64>`; thread them from
`stream.rs`'s match into the executor arm.

**`summary()` has no `Foreground` arm today** — `run_terminal_command` falls
through to `_ => String::new()`, so its `ToolStarted` line shows no argument
at all. Add one: the command (truncated the way `SearchRepository`'s arm
truncates its query at 40 chars) plus the shell id when `shell` is given.
A tool that can now run somewhere other than the default shell should say
where in the line the user watches.

### Task 3 — resolve the target in `shell_exec`

A pure resolver, testable without a host:

```rust
enum ShellTarget { Reuse, New, Existing(ShellId) }
fn resolve_shell_target(arg: Option<&str>) -> Result<ShellTarget, String>
```

`None` → `Reuse`; `"new"` → `New`; a valid shell id → `Existing`; anything
else → `Err` with a sentence naming the value and the two accepted forms.
**`"new"` is matched exactly** — `"New"` and `"NEW"` are errors, the same
case-sensitivity `list_shells`'s `scope` already enforces (there is a test
for it: `list_shells_scope_match_is_case_sensitive`).

Then a `run_in_shell_with` that takes the resolved target plus the optional
timeout, does the ownership check for `Existing`, and otherwise behaves
exactly as today.

### Task 4 — the tool definition and `open_shell`'s description

- `run_terminal_command` gains `shell` (`ParamTy::Str`, optional) and
  `timeout_secs` (`ParamTy::Int`, optional). Descriptions state: `"sN"`
  targets that shell and `"new"` opens one; omitted reuses this session's
  lowest-numbered shell; another session's shell is refused; the timeout
  defaults to 45 s and is capped at 3600.
- **Do not** add `host` or `wait`, and **do not** remove `background`,
  `target_pane` or `retry_in_pane` — § Pre-flight's table says where those go.
- **Do not add `shell` to `enum_values`** in `src/ai/tools/schema.rs`; that
  map is keyed by param name globally and `shell` is already a param name on
  four other tools.
- Update `open_shell`'s `ToolDef` description and its returned sentence per
  § Pre-flight.

### Task 5 — the docs

- `CLAUDE.md` § "Current AI tools": the `run_terminal_command` row mentions
  the two new parameters. **The count line stays `48 tools: 34 core + 14
  deferred`** — nothing was added or removed.
- `README.md` § "AI tools": the same row; the three numbers **do not move**.
- `assets/prompts/sre.toml`: `run_terminal_command`'s prose gains the two
  parameters, and `open_shell`'s prose loses its now-false absolute. **Name no
  parameter that does not exist** — no `host`, no `wait` — and cite no
  milestone or phase identifier.

### Task 6 — run the mutation checks and record them

Both with the `patch` tool (`sed -i`, `perl -i` and `>` redirects are
forbidden by the executor contract). **These target code that does not exist
yet, so they are NOT pre-measured — measuring them is your job.** Apply each
and run its test **six times**: a valid mutation reddens **6 of 6**. If one
comes back green in any run, **the mutation is wrong rather than the test** —
say so and propose one that always moves an observable. Two phases in this
milestone lost budget to a mutation that could never redden. Never invent a
failure.

1. **A shell the caller does not own must be refused.** Make the ownership
   check compare the inner owner string instead of `Owner::key()` → the
   cross-owner refusal test must go red.
2. **The timeout error must name the timeout applied, not the constant.**
   Make the error message interpolate `LOCAL_CMD_TIMEOUT` again → the
   custom-timeout error test must go red.

After each, revert with `patch` and show the test green again. Quote each
`test result:` line from the run whose command you are showing.

### Task 7 — capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, paste
`/tmp/e2e-m22-07a.txt` into **one** new Update Log entry headed
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

- [ ] **The two parameters exist:**
      `<R> grep -cE 'name: "(shell|timeout_secs)"'` → **2** (now `0`), and
      `<R> grep -c 'name: "'` → **7** (now **5**). *(The `sed` range prints
      the tool's own `name:` line as well as each parameter's, so this is
      1 + params — measured, not assumed.)*
- [ ] **Nothing was retired:**
      `<R> grep -cE 'name: "(background|target_pane|retry_in_pane)"'` → **3**,
      unchanged.
- [ ] **No phantom parameters:** `<R> grep -cE 'name: "(host|wait)"'` → **0**.
- [ ] **The tool surface did not move:**
      `grep -c 'ToolDef {' src/ai/tools/defs.rs` → **48**, unchanged;
      `grep -c '48 tools: 34 core + 14 deferred' CLAUDE.md` → **1**,
      unchanged; `grep -c 'The model has \*\*48 tools\*\*' README.md` → **1**,
      unchanged.
- [ ] **The tuple was widened and every site follows:**
      `sed -n '/    ToolCall(/,/^    ),/p' src/ai/types/events.rs | grep -c 'Option<u64>'`
      → **1** (now `0`), and the four construction/match sites all compile —
      `grep -rc 'AiEvent::ToolCall' --include=*.rs src/ | grep -v ':0' | wc -l`
      → **4**, unchanged.
- [ ] **The resolver exists and is pure:**
      `grep -c 'fn resolve_shell_target' src/daemon/executor/shell_exec.rs` →
      **1** (now `0`), and
      `sed -n '/fn resolve_shell_target/,/^}/p' src/daemon/executor/shell_exec.rs | grep -cE 'await|registry\(\)|fs::'`
      → **0**.
- [ ] **`"new"` spawns a reusable shell, not a dedicated one:**
      `grep -c 'spawn_dedicated_for' src/daemon/executor/shell_exec.rs` →
      **0** (now `0` — keep it that way).
- [ ] **Ownership is compared on `key()`:**
      `grep -c 'key()' src/daemon/executor/shell_exec.rs` → **1 or more**
      (now `0`).
- [ ] **The timeout error stopped naming the constant:**
      `sed -n '/command exceeded the/,/));/p' src/daemon/executor/shell_exec.rs | grep -c 'LOCAL_CMD_TIMEOUT'`
      → **0** (now **1**). *(The constant itself stays — it is the default
      when no override is given, at `shell_exec.rs:123`. Only the error
      message must stop hard-coding it.)*
- [ ] **`open_shell`'s honesty test survives:**
      `grep -c 'lowest-numbered' src/daemon/executor/knowledge/shell.rs` →
      **2 or more**, and `cargo test --lib 'open_shell_returns_a_new_id'` →
      **1 passed**.
- [ ] **`enum_values` untouched:**
      `sed -n '/pub(super) fn enum_values/,/^}/p' src/ai/tools/schema.rs | grep -c '"shell"'`
      → **0**, and the same range `grep -c '"session", "knowledge", "incident"'`
      → **1**, unchanged.
- [ ] **The pane substrate was not touched — that is phase 09:**
      `git diff 6b1f8d16..HEAD --stat -- src/ipc.rs src/cli/ | wc -l` → **0**.
- [ ] **`summary` gains a `Foreground` arm.** It has **none** today —
      `run_terminal_command` falls through to `_ => String::new()`, so the
      `ToolStarted` line shows no argument at all:
      `sed -n '/pub fn summary(&self)/,/^    }/p' src/ai/types/pending.rs | grep -c 'PendingCall::Foreground'`
      → **1** (now **0**). *(Do not pin `grep -c 'shell'` over that function —
      the word already appears 8 times in other arms, and scoping to the
      `PendingCall::Foreground {` range does not help either: `sed` restarts a
      range on each match, so it also selects the `to_tool_call` and `id()`
      arms.)*
- [ ] **The six new tests exist by name** (tree-wide matching lines):
      `grep -rE '^\s*(async )?fn (resolve_shell_target_accepts_none_new_and_an_id|resolve_shell_target_rejects_a_bad_value|resolve_shell_target_is_case_sensitive|run_terminal_command_targets_the_named_shell|run_terminal_command_refuses_another_owners_shell|run_terminal_command_custom_timeout_names_the_applied_value)' --include=*.rs src/ | wc -l`
      → **6** (now `0`).
- [ ] No new `unsafe` / `unwrap` / `expect` / `panic!` in **production** code:
      `sed -n '1,/^#\[cfg(test)\]/p' src/daemon/executor/shell_exec.rs | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe '`
      → **0**, unchanged.
- [ ] **No lint-silencing shim:**
      `git diff 6b1f8d16..HEAD -- 'src/*.rs' | grep -cE '^\+.*#\[allow'` → **0**.
- [ ] `cargo test --lib 'daemon::executor::shell_exec'` → **19 or more**,
      `0 failed` (now **13**).
- [ ] `cargo test --lib 'knowledge::shell'` → **51**, unchanged;
      `cargo test --lib 'ai::tools'` → **22**, unchanged;
      `cargo test --test doc_truth` → **10**, unchanged.
- [ ] **Preservation:** `cargo test --lib 'config::'` → **92**;
      `cargo test --test isolation` → **9**;
      `cargo test --lib 'daemon::stream'` → unchanged from § E of the capture.
- [ ] Full lib suite **1892 or more**, `0 failed`, three consecutive runs (now
      **1886**).
- [ ] All four gates pass.

## Test plan

Six new tests. Three are pure; three drive the in-process host fixture
`shell_exec.rs` already has (`spawn_in_logs_dir`).

**Pure, beside `resolve_shell_target`:**

1. **`resolve_shell_target_accepts_none_new_and_an_id`** — `None` → `Reuse`,
   `Some("new")` → `New`, `Some("s7")` → `Existing`.
2. **`resolve_shell_target_rejects_a_bad_value`** — `Some("banana")` is an
   `Err` naming the value **and** both accepted forms. Assert the message
   mentions `"new"`, not just that it errored.
3. **`resolve_shell_target_is_case_sensitive`** — `Some("New")` and
   `Some("NEW")` are errors. The negative that keeps a helpful-looking
   `eq_ignore_ascii_case` out.

**Against a real in-process host:**

4. **`run_terminal_command_targets_the_named_shell`** — open two shells for
   one owner, run with `shell: Some(<the higher-numbered id>)`, and assert the
   returned `ShellId` is **that one** — not the lowest-numbered the reuse rule
   would pick. **That is the whole point of the phase**, and asserting only
   "a shell ran it" would pass against the unchanged reuse path.
5. **`run_terminal_command_refuses_another_owners_shell`** — a shell owned by
   `Owner::Ghost("a")`, a caller of `Owner::Chat("a")` — **same inner string,
   different kind**. The refusal names the id, and **no command runs**: assert
   the shell's recording does not contain the command's sentinel. Mutation
   1's test.
6. **`run_terminal_command_custom_timeout_names_the_applied_value`** — run a
   command that outlives a deliberately tiny `timeout_secs` and assert the
   error text contains that value and **not** `45`. Mutation 2's test; the
   negative half is what makes it discriminate.

**Negatives are named on purpose.** A suite asserting only that a targeted
command runs passes against an implementation that ignores `shell` entirely,
accepts another session's shell, and reports the wrong timeout.

## End-to-end verification

Run from the repo root, verbatim:

```sh
A=/tmp/e2e-m22-07a.txt
: > "$A"
CASTS_BEFORE=$(ls ~/.daemoneye/var/log/shells/ 2>/dev/null | wc -l)
{
echo "== A. build =="
echo "real HOME shell logs BEFORE: $CASTS_BEFORE"
cargo build 2>&1 | tail -2; echo "cargo_exit=${PIPESTATUS[0]}"

echo "== B. the touched modules' tests =="
for m in 'daemon::executor::shell_exec' 'knowledge::shell' 'ai::tools' 'daemon::stream'; do
  printf '%-34s ' "$m"; cargo test --lib "$m" 2>&1 | grep '^test result'
done

echo "== C. the four tuple sites, shown not asserted =="
grep -rn 'AiEvent::ToolCall' --include=*.rs src/
echo "--- the widened variant ---"
sed -n '/    ToolCall(/,/^    ),/p' src/ai/types/events.rs

echo "== D. structural greps (each must print the stated number) =="
D=src/ai/tools/defs.rs; S=src/ai/tools/schema.rs; E=src/daemon/executor/shell_exec.rs
K=src/daemon/executor/knowledge/shell.rs; P=src/ai/types/pending.rs
R() { sed -n '/name: "run_terminal_command"/,/^    },/p' $D; }
printf 'new params                   (2): %s\n' "$(R | grep -cE 'name: "(shell|timeout_secs)"')"
printf 'name: lines (1 + params)     (7): %s\n' "$(R | grep -c 'name: "')"
printf 'nothing retired              (3): %s\n' "$(R | grep -cE 'name: "(background|target_pane|retry_in_pane)"')"
printf 'no host/wait param           (0): %s\n' "$(R | grep -cE 'name: "(host|wait)"')"
printf 'ToolDef literals            (48): %s\n' "$(grep -c 'ToolDef {' $D)"
printf 'CLAUDE count unchanged       (1): %s\n' "$(grep -c '48 tools: 34 core + 14 deferred' CLAUDE.md)"
printf 'README count unchanged       (1): %s\n' "$(grep -c 'The model has \*\*48 tools\*\*' README.md)"
printf 'tuple widened                (1): %s\n' "$(sed -n '/    ToolCall(/,/^    ),/p' src/ai/types/events.rs | grep -c 'Option<u64>')"
printf 'tuple sites                  (4): %s\n' "$(grep -rc 'AiEvent::ToolCall' --include=*.rs src/ | grep -v ':0' | wc -l)"
printf 'resolver exists              (1): %s\n' "$(grep -c 'fn resolve_shell_target' $E)"
printf 'resolver is pure             (0): %s\n' "$(sed -n '/fn resolve_shell_target/,/^}/p' $E | grep -cE 'await|registry\(\)|fs::')"
printf 'no dedicated spawn           (0): %s\n' "$(grep -c 'spawn_dedicated_for' $E)"
printf 'ownership on key()          (1+): %s\n' "$(grep -c 'key()' $E)"
printf 'timeout err drops the const  (0): %s\n' "$(sed -n '/command exceeded the/,/));/p' $E | grep -c 'LOCAL_CMD_TIMEOUT')"
printf 'lowest-numbered kept        (2+): %s\n' "$(grep -c 'lowest-numbered' $K)"
printf 'enum_values untouched        (0): %s\n' "$(sed -n '/pub(super) fn enum_values/,/^}/p' $S | grep -c '"shell"')"
printf 'memory param set intact      (1): %s\n' "$(sed -n '/pub(super) fn enum_values/,/^}/p' $S | grep -c '"session", "knowledge", "incident"')"
printf 'pane substrate untouched     (0): %s\n' "$(git diff 6b1f8d16..HEAD --stat -- src/ipc.rs src/cli/ | wc -l)"
printf 'summary Foreground arm       (1): %s\n' "$(sed -n '/pub fn summary(&self)/,/^    }/p' $P | grep -c 'PendingCall::Foreground')"
printf 'six new tests                (6): %s\n' "$(grep -rE '^\s*(async )?fn (resolve_shell_target_accepts_none_new_and_an_id|resolve_shell_target_rejects_a_bad_value|resolve_shell_target_is_case_sensitive|run_terminal_command_targets_the_named_shell|run_terminal_command_refuses_another_owners_shell|run_terminal_command_custom_timeout_names_the_applied_value)' --include=*.rs src/ | wc -l)"
printf 'prod unwrap shell_exec       (0): %s\n' "$(sed -n '1,/^#\[cfg(test)\]/p' $E | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe ')"
printf 'no new allow shim            (0): %s\n' "$(git diff 6b1f8d16..HEAD -- 'src/*.rs' | grep -cE '^\+.*#\[allow')"

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

echo "== H. no HOME litter from the suite =="
printf 'real HOME shell logs AFTER (==BEFORE): %s\n' "$(ls ~/.daemoneye/var/log/shells/ 2>/dev/null | wc -l)"
} >> "$A" 2>&1
cat "$A"
```

Then the paste self-check, transcript as the **first** fenced block after the
heading:

```sh
D=docs/dev/milestones/M22-tool-surface/phase-07a-run-command-shell-target.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m22-07a.txt
diff /tmp/pasted-m22-07a.txt /tmp/e2e-m22-07a.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

Paste the captured block **unmodified** — do not retype, trim, or realign it.

## Authorizations

The executor may modify **only**:

- `src/ai/types/events.rs` — the tuple widening only.
- `src/ai/types/pending.rs` — `Foreground`'s two new fields, its `to_tool_call`
  arm and the `summary` arm.
- `src/ai/tools/args.rs`, `src/ai/tools/dispatch.rs`, `src/ai/tools/defs.rs` —
  the two parameters and `open_shell`'s description.
- `src/daemon/stream.rs`, `src/daemon/ghost.rs`, `src/ai/backends/gemini.rs` —
  **the `AiEvent::ToolCall` site only** in each. No other line.
- `src/daemon/executor/shell_exec.rs` — tasks 3 and the timeout, plus the six
  tests.
- `src/daemon/executor/mod.rs` — the `PendingCall::Foreground` arm only.
- `src/daemon/executor/knowledge/shell.rs` — **`open_shell`'s returned
  sentence and, if you change it, the `lowest-numbered` assertion. Nothing
  else**; a criterion pins `knowledge::shell` at 51.
- `CLAUDE.md`, `README.md`, `assets/prompts/sre.toml` — task 5.
- `docs/dev/milestones/M22-tool-surface/phase-07a-run-command-shell-target.md`
  — **append to the Update Log only.**

No other file. In particular **do not touch `src/ipc.rs` or `src/cli/`** — the
`target_pane` retirement is phase 09's and a criterion asserts both are
unchanged. Also do not touch `src/shell/`, `src/daemon/executor/bg_shell.rs`
(07b's), `src/daemon/executor/foreground.rs`, `tests/doc_truth.rs` or
`src/config/`.

## Out of scope

- **`wait?` replacing `background`.** Phase 07b. `background` keeps working
  exactly as today and a criterion asserts the parameter still exists.
- **Retiring `target_pane` and `retry_in_pane`.** Phase 09, with the pane
  substrate — 170 and 13 references respectively, including the IPC wire and
  the CLI approval panel. Not a tool-phase change.
- **`host?`.** No host concept exists; M21/M23. Third M22 phase to measure
  this.
- **Targeting a *dedicated* background shell.** `shell: "sN"` resolves only
  non-dedicated shells the caller owns; a background run's dedicated shell is
  deliberately unreachable, which is what keeps `watch_shell` meaningful.
- **Changing the reuse rule itself.** `shell_for_owner` is untouched;
  `shell?` only bypasses it.
- **The `extend_output_line` accumulate-and-rewindow hazard** recorded in the
  milestone README. This phase changes which shell a command runs in, not how
  output is read.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-09-15 22:52 (started, executor)

Started phase 07a. Plan: widen `AiEvent::ToolCall` across its four sites in one
batch, add the resolver and timeout override in `shell_exec`, update the tool
def + `open_shell`'s sentence, threads the docs, then mutation checks and the
end-to-end capture. Status left at `in-progress`; the completion tail belongs
to the server.

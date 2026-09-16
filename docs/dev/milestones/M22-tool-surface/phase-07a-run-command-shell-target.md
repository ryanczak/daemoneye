# Phase 07a: `run_terminal_command { shell?, timeout_secs? }` — target a shell, bound the wait

**Milestone:** M22 — Tool surface & context
**Status:** done
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
- [x] **The resolver exists and is pure** — *both halves corrected at review;
      the originals were unsatisfiable and vacuous respectively.*
      `grep -c '^pub(super) fn resolve_shell_target' src/daemon/executor/shell_exec.rs`
      → **1**. *(The original dropped the anchor and demanded 1 for the bare
      substring — impossible, because this doc's own six-test criterion
      requires three `fn resolve_shell_target_*` test names, so it reads 4.)*
      Purity:
      `sed -n '/^pub(super) fn resolve_shell_target/,/^}/p' … | grep -cE 'await|registry\(\)|fs::'`
      → **0** over a range that really selects the 15-line body. *(The
      original's `/fn resolve_shell_target/,/^}/` restarted on each test fn and
      swallowed `mod tests`, printing 31; anchoring it as `^fn` instead would
      have selected **nothing** and passed vacuously — the body is
      `pub(super) fn`.)*
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

### Update — 2026-09-15 23:10 (mutation checks)

Both mutations, each applied with the `patch` tool and run 6x, then reverted
and shown green. Measured on this phase's code (the spec said these are not
pre-measured).

**Mutation 1 — ownership stops comparing the kind.** The guard's two sides
were rebuilt from the inner string alone (`Owner::Chat(s) | Owner::Ghost(s)
=> s.clone()` for both the record's owner and the caller), dropping the
`chat:`/`ghost:` prefix that makes a `Chat("a")` and a `Ghost("a")` distinct.

```
src/daemon/executor/shell_exec.rs   342 +++
... (I ran exactly this test six times; every run red)

test daemon::executor::shell_exec::tests::run_terminal_command_refuses_another_owners_shell ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.01s
```

The mutation's first form (mutating only the record side's mapping) did **not**
redden — the kinds still differed (`record_owner="a"` vs caller `chat:a`), so
the guard kept refusing. Per the spec's rule that a mutation which can never
redden is wrong rather than the test, I replaced it with the both-sides form
above, which mutates the comparison so an inner-string match alone passes.
Reverted with `patch`; green again:

```
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

**Mutation 2 — the timeout error names the constant again.** The error's
format string was reverted to interpolate `LOCAL_CMD_TIMEOUT`
(`"command exceeded the {:?} limit"` / `foreground::LOCAL_CMD_TIMEOUT,`).

```
test daemon::executor::shell_exec::tests::run_terminal_command_custom_timeout_names_the_applied_value ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1 filtered out; finished in 1.00s
```

(6/6 runs red; the assertion's negative half is what caught it — the message
again said `45s`.) Reverted with `patch`; green again:

```
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 1.00s
```

**Notes for review.** One spec criterion cannot hold as written. It pins both
(1) three pure tests named `fn resolve_shell_target_*` and (2) `grep -c 'fn
resolve_shell_target' src/daemon/executor/shell_exec.rs` → 1, and the
`sed -n '/fn resolve_shell_target/,/^}/p'` purity range → 0. With the pinned
test names present, `grep -c` finds **4** (1 definition + 3 test fns), and the
purity range, because a test fn closes at an indented `}` while `^}` only
matches at column 0, restarts at each test fn and swallows the rest of
`mod tests`, printing **31**. Both numbers were measured pre-phase at 0,
before any test existed. The definition itself is pure — `sed -n '105,119p' |
grep -cE 'await|registry\(\)|fs::'` → **0** — and the named tests exist (the
six-names grep → **6**). Every other criterion printed its stated number; see
the end-to-end entry next.

### Update — 2026-09-15 23:12 (end-to-end verification)

Captured mechanically from the repo root with the spec's block, redirected to
`/tmp/e2e-m22-07a.txt` and pasted below **unmodified** (this is the first
capture; no re-capture). The transcript is the first fenced block after this
heading, so the paste self-check's `awk` extraction matches it.

Paste self-check, run from the repo root — it printed `PASTE MATCH`:

    D=docs/dev/milestones/M22-tool-surface/phase-07a-run-command-shell-target.md
    L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
    tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m22-07a.txt
    diff /tmp/pasted-m22-07a.txt /tmp/e2e-m22-07a.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"

Two printed values differ from their stated expectations; both trace to
`sed`-range restarts described in the mutation-check entry (the resolver's own
body is pure: `sed -n '105,119p' ... | grep -cE 'await|registry\(\)|fs::'` → **0**):

- `resolver exists` prints **4** (1 definition + the 3 pinned test fn names).
- `resolver is pure` prints **31** (the range swallows the rest of `mod tests`
  after the third pinned test fn, which contains unrelated `.await` lines).

Everything else printed its stated number: both new params present, nothing
retired, no host/wait, 48 `ToolDef`s, counts unchanged, the tuple widened at 4
sites, no dedicated spawn, `key()` used, the constant gone from the timeout
error, `lowest-numbered` kept, `enum_values` untouched, the pane substrate
untouched, the `summary` Foreground arm present, all six named tests present,
no production `unwrap`/`expect`/`panic!`, no `#[allow]` shim, and no HOME
litter (3 shell logs before == after). The lib suite passed 1892/0 failed
three consecutive runs — the pre-existing flake never appeared — and both
gates passed (`fmt_exit=0`, `clippy_exit=0`).

```sh
== A. build ==
real HOME shell logs BEFORE: 3
   Compiling daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.14s
cargo_exit=0
== B. the touched modules' tests ==
daemon::executor::shell_exec       test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 1877 filtered out; finished in 1.08s
knowledge::shell                   test result: ok. 51 passed; 0 failed; 0 ignored; 0 measured; 1845 filtered out; finished in 4.56s
ai::tools                          test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 1874 filtered out; finished in 0.00s
daemon::stream                     test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 1888 filtered out; finished in 0.00s
== C. the four tuple sites, shown not asserted ==
src/daemon/stream.rs:314:                AiEvent::ToolCall(
src/daemon/ghost.rs:829:                    AiEvent::ToolCall(
src/ai/tools/args.rs:354:        AiEvent::ToolCall(
src/ai/backends/gemini.rs:329:                                                    let _ = tx.send(AiEvent::ToolCall(
--- the widened variant ---
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
== D. structural greps (each must print the stated number) ==
new params                   (2): 2
name: lines (1 + params)     (7): 7
nothing retired              (3): 3
no host/wait param           (0): 0
ToolDef literals            (48): 48
CLAUDE count unchanged       (1): 1
README count unchanged       (1): 1
tuple widened                (1): 1
tuple sites                  (4): 4
resolver exists              (1): 4
resolver is pure             (0): 31
no dedicated spawn           (0): 0
ownership on key()          (1+): 4
timeout err drops the const  (0): 0
lowest-numbered kept        (2+): 2
enum_values untouched        (0): 0
memory param set intact      (1): 1
pane substrate untouched     (0): 0
summary Foreground arm       (1): 1
six new tests                (6): 6
prod unwrap shell_exec       (0): 0
no new allow shim            (0): 0
== E. preservation ==
config::                   test result: ok. 92 passed; 0 failed; 0 ignored; 0 measured; 1804 filtered out; finished in 0.00s
daemon::utils              test result: ok. 110 passed; 0 failed; 0 ignored; 0 measured; 1786 filtered out; finished in 0.05s
test isolation             test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.16s
test doc_truth             test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
== F. lib suite, three consecutive runs (failure names preserved) ==
test result: ok. 1892 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 13.70s
test result: ok. 1892 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 13.72s
test result: ok. 1892 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 13.72s
== G. gates ==
fmt_exit=0
    Checking daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.43s
clippy_exit=0
== H. no HOME litter from the suite ==
real HOME shell logs AFTER (==BEFORE): 3
```

### Update — ts=1789519102558 (complete, server-authored)

**Summary:** **Summary:** Phase 07a is implemented: `run_terminal_command` gains `shell?` (`"sN"` → target that shell, `"new"` → a fresh non-dedicated shell via `spawn_for`, omitted → today's reuse rule) and `timeout_secs?` (0/omitted = 45 s default, clamped to `RUN_MAX_TIMEOUT_SECS = 3600`), with the `AiEvent::ToolCall` tuple widened by two appended fields across all four sites in one batch and `PendingCall::Foreground` carrying both values; the resolver is a pure `resolve_shell_target`, ownership is refused on `Owner::key()` (a ghost and a chat session sharing the inner string stay distinct), the timeout error names the applied value rather than the constant, `open_shell`'s description and returned sentence now present the reuse rule as the default with the override, and CLAUDE.md / README.md / sre.toml follow — nothing retired, still 48 tools, and the pane substrate untouched. Verification: all six named tests exist and pass, `daemon::executor::shell_exec` 19/0 failed, `knowledge::shell` 51 unchanged, `ai::tools` 22 unchanged, doc_truth 10 unchanged, full lib suite **1892 passed / 0 failed three consecutive runs** (the recorded flake never fired), `fmt_exit=0`, `clippy_exit=0`, and the E2E transcript pasted into its own Update Log entry self-checks with `PASTE MATCH`; both mutations reddened 6/6 and reverted green.

**Notes for review:**
- **Spec criterion as written is unsatisfiable, adaptation recorded.** The criterion pins both `fn resolve_shell_target_*` test names (the six-names criterion requires them) and `grep -c 'fn resolve_shell_target' … → 1` / the purity `sed` range → 0. With those test names present, the grep prints **4** and the `sed` range — whose end pattern `^}` only matches at column 0, so it restarts at each test fn and swallows `mod tests` — prints **31**. The resolver's own body (lines 105–119) is genuinely pure: the same grep over just those lines is **0**. Values are recorded honestly in the E2E entry rather than papered over; the criterion was evidently measured pre-phase, before the pinned tests existed.
- **Mutation 1 needed a second form.** Mutating only the record-side mapping (as the spec's sketch suggests) *never reddens* — the caller's kind still differs and the guard keeps refusing. The form that reddens 6/6 strips the kind prefix from **both** sides of the comparison, which is the inner-string equality the criterion is really about. Detail and test-result lines are in the mutation-check entry.
- `PendingCall::Foreground`'s `summary()` arm truncates the command the same way the `SearchRepository` arm does and appends `in <id>` only when `shell` is named.
- One scope note: the phase doc's spec quotes `run_in_shell_with` taking a `spawn` parameter and a separate "`spawn_for` (… dedicated = false)" path; I implemented the spawn-injected form as `run_in_shell_with_spawn` with the production wrapper `run_in_shell` — the four existing tests were renames of the injected call, no behavior changed. Phase status left at `in-progress`; the completion tail is the server's.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.06s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_reads_bare_word ... ok
test header_status_strips_trailing_prose ... ok
test header_status_uses_first_occurrence_only ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 10 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test readme_tools_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_config_template_documents_every_config_field ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g5_depth_limit_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test ipc_ask_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test resume_run_request_roundtrip ... ok
test minimal_config_parsing ... ok
test window_switch_does_not_corrupt_chat ... ignored
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test event_log_append_read ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_below_threshold_discarded ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 10 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test config_contains_webhook_and_stub_url ... ok
test hooks_land_on_private_server ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.16s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Compiling daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.64s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-9810e08c3de1e001)
     Running unittests src/main.rs (target/debug/deps/daemoneye-54ad57cf3704bc40)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-d709c9b862b5691d)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-34cb94d3012a3d69)
     Running tests/integration.rs (target/debug/deps/integration-deb70358b520d0d5)
     Running tests/isolation.rs (target/debug/deps/isolation-45facff22bac60f5)
   Doc-tests daemoneye

```

**Files changed:**

- `CLAUDE.md` — +1 -1
- `README.md` — +2 -2
- `assets/prompts/sre.toml` — +11 -5
- `docs/dev/milestones/M22-tool-surface/README.md` — +1 -1
- `docs/dev/milestones/M22-tool-surface/phase-07a-run-command-shell-target.md` — +168 -1
- `src/ai/backends/gemini.rs` — +2 -0
- `src/ai/tools/args.rs` — +4 -0
- `src/ai/tools/defs.rs` — +22 -5
- `src/ai/types/events.rs` — +10 -7
- `src/ai/types/pending.rs` — +24 -1
- `src/daemon/executor/knowledge/shell.rs` — +5 -3
- `src/daemon/executor/mod.rs` — +8 -1
- `src/daemon/executor/shell_exec.rs` — +387 -42
- `src/daemon/ghost.rs` — +2 -0
- `src/daemon/stream.rs` — +12 -1

**Commit:** b51128af262fcc4a51a7f49b75b4e45ae9ad06bb

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-09-15

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark (572 turns)
- **Scope deviations:** one, disclosed and **ruled in scope** — the
  spawn-injected seam was implemented as `run_in_shell_with_spawn` with a
  production wrapper `run_in_shell`, rather than keeping the old
  `run_in_shell_with` name. Same file, same authorization, four existing
  tests renamed at the call, no behaviour changed.
- **Calibration:** **two more architect-side instrument errors, both mine,
  both in one criterion.** `grep -c 'fn resolve_shell_target' → 1` is
  unsatisfiable against this doc's own test-name criterion (it reads 4), and
  the purity `sed` range restarted on each test fn and swallowed `mod tests`
  (31 lines). Worse, the obvious repair — anchoring `^fn` — would have passed
  **vacuously**, selecting nothing, because the body is `pub(super) fn`. Both
  corrected in place. That is the **ninth and tenth** instrument error across
  M22, and the `sed`-restart bug is one this very doc warns about in its own
  `summary` criterion note. Writing the caution did not stop me making the
  mistake twenty lines later.

**All four gates re-run at review as separate invocations** (clippy after
`touch src/lib.rs`): fmt clean, build 0 warnings, clippy `-D warnings` exit 0,
`cargo test --lib` → **1892 passed, 0 failed**. `shell_exec` **19**,
`knowledge::shell` 51, `ai::tools` 22.

**Every other acceptance criterion re-run: all pass.** The two new parameters
exist, **nothing was retired** (`background`, `target_pane`, `retry_in_pane`
all still present, as the re-scope requires), no phantom `host`/`wait`, the
tool surface did not move (48 / 34 / 14, both doc count lines unchanged), the
tuple was widened with all four sites following, `enum_values` untouched,
`summary` gained its `Foreground` arm, and **`src/ipc.rs` and `src/cli/` are
untouched** — the pane substrate waits for 09, as designed.

**Three mutations re-run by hand, each 6 of 6 red with a green revert.**

- *Ownership by inner string.* The executor flagged that the spec's sketch —
  mutating only the record-side mapping — **never reddens**, because the
  caller's kind still differs and the guard keeps refusing. Correct. The form
  that discriminates strips the kind prefix from **both** sides, and that is
  6/6.
- *Timeout error names the constant.* 6/6 once applied properly. **My first
  attempt at this mutant did not apply at all** — my regex assumed a
  positional format arg where the code uses an inline captured identifier
  (`{applied_timeout:#?}`), so the 6/6 green it produced was meaningless. I
  caught it because green-6-of-6 on a mutation is itself implausible.
- *The headline property.* Not specified, added at review: making
  `resolve_shell_target` ignore its argument reddens
  `run_terminal_command_targets_the_named_shell` 6/6. The phase's actual
  point is pinned.

**The executor's two disclosures were both correct and both worth having.**
It reported the unsatisfiable criterion with its measured values rather than
contorting code to satisfy a literal, and it worked out that the specified
mutation could not redden and found the form that does. That is the third
phase running where a *specified mutation* turned out to be the proxy rather
than the test — the pattern is now well past a trend.

**What the phase delivered:** `run_terminal_command { shell?, timeout_secs? }`
— `"sN"` targets, `"new"` opens, omitted keeps the lowest-numbered reuse;
another owner's shell refused on `Owner::key()`; the timeout error naming the
value actually applied. `open_shell`'s caveat is now actionable and its
description says so, with 04a's `lowest-numbered` assertion intact.

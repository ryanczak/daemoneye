# Phase 05: `send_input` — the approval-gated keystroke tool

**Milestone:** M22 — Tool surface & context
**Status:** todo
**Depends on:** 03a (`list_shells`, the module and its registry seam), 04a
(`close_shell`, the ownership-check shape) and 04b (`watch_shell`, the
`ShellState` probe). All **done**.
**Estimated diff:** ~520 lines incl. tests
**Tags:** language=rust, kind=feature, size=m

## Goal

The shell tools so far observe (`list_shells`, `read_shell`, `find_in_shells`,
`watch_shell`) and manage lifetime (`open_shell`, `close_shell`). This is the
first that **types**. After this phase:

1. `send_input { shell, text, enter? }` writes raw bytes into a live PTY
   shell — core, `Category::Run`, **approval-gated**.
2. Every byte lands in the shell's cast log as an `"i"` event, and **no read
   tool can ever surface it** — a property this phase pins with a test.
3. A ghost shell may not use it unless its agent `ToolPolicy` **explicitly**
   allows it, the same default-deny `tmux_control` already has.
4. The approval panel says what the shell is *doing*, so the user approving
   can tell "answering a prompt in `mysql`" from "running a command at a bare
   shell prompt".
5. Tool counts move to **46 tools: 34 core + 12 deferred**.

### This is the milestone's one genuinely dangerous tool — read this first

The milestone README says it outright, and it has a mechanical consequence
that is easy to miss:

> `send_input` … belongs in `APPROVAL_GATED_TOOLS` — which is not only a UI
> decision: that list is also what exempts a tool from the per-turn,
> per-session and per-tool budget caps in `daemon::stream`. A tool added there
> without a real prompt gets an uncapped call budget.

So the entry in `APPROVAL_GATED_TOOLS` and the `prompt_and_await_approval`
call in the executor arm are **one decision, not two**. Shipping the list
entry without a real prompt hands a keystroke tool an unlimited call budget.
A criterion pins both halves.

### What it cannot do, and why the description must say so

`send_input` writes bytes to a PTY. It is **not** `run_terminal_command`:

- there is **no marker protocol**, so there is **no exit code** — nothing
  wraps the bytes in `wrap_command`'s `DE_BEG`/`DE_END` fence;
- it does **not wait** for the program to react, and it returns no output.
  Reading what happened afterwards is `read_shell` or `watch_shell`;
- a shell sitting at its **prompt** will run whatever is typed as a shell
  command, with none of `run_terminal_command`'s approval text, capture or
  exit-code handling.

That last one is the honest hazard and the reason for the state line in the
approval panel (§ Spec task 3). **Do not add a refusal for it** — a program
waiting on stdin reports `Running`, so `Idle` really does mean "at the
prompt", and refusing would block nothing a user has not been shown.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.5, the one row:
  `send_input { shell, text, enter?: bool }` | core | **approval-gated** |
  *Drive an interactive program or answer a prompt; every keystroke lands in
  the `"i"` log*. Both halves of that row are true on this tree and § Pre-flight
  measures them.
- § 2.5's interactive paragraph names the intended flow: *"any other
  interactive program runs with `wait: false` and the agent uses `watch_shell`
  / `read_shell` / `send_input` to drive it."* `wait:` arrives in **phase 07**
  and does not exist here — see § Spec task 5 for what may be said.
- `docs/dev/milestones/M22-tool-surface/README.md` § Notes — the tool
  arithmetic (45 → **46**) and the `send_input` hazard note quoted above.
- `CLAUDE.md` § "Adding a new AI tool (checklist)" — **twelve** steps for this
  one: step 12 (the `APPROVAL_GATED_TOOLS` entry plus the README `**⚠**`)
  applies here and did not apply to 03a–04b.

## Pre-flight

### Measured fact — the `"i"` log is real, and no read tool can surface it

Unlike the `.meta.json` command index (which phase 04b found has **no**
production writer), the input half of the cast log **is** written in
production. `PtyBackend::input` — `src/shell/backend.rs:106-117` — does both
halves on every `Input` frame:

```rust
    async fn input(&self, bytes: &[u8]) -> anyhow::Result<()> {
        self.shell
            .lock()
            .unwrap_or_log()
            .write_input(bytes)
            .context("write input to shell")?;
        self.log
            .lock()
            .unwrap_or_log()
            .write_input(self.started.elapsed(), bytes)
            .context("record input event")
    }
```

`CastWriter::write_input` (`src/shell/log.rs:59`) records it as an `"i"`
event. **And both readers filter to `"o"` only** — `read_command_output`
(`src/shell/log.rs:199`, `array[1].as_str() != Some("o")` → skip) and 04b's
`read_output_since` (via `extend_output_line`). So text sent with this tool
is recorded for audit but is **unreachable** from `read_shell`,
`find_in_shells` and `watch_shell`.

That is a safety property, not an accident, and § Test plan item 6 pins it:
send a distinctive string, then assert `read_shell` does **not** return it.

### The wire already carries input — the daemon just has no sender

`ShellRequest::Input { bytes }` exists (`src/shell/proto.rs:32`) and the host
answers it with `Ok` (`src/shell/host.rs:153-158`). What is missing is a
daemon-side sender: `src/shell/client.rs` exposes `send_signal`, `status`,
`run_command` and `spawn_command`, and the only thing that sends an `Input`
frame is the **private** `start_command`, which marker-wraps the payload.

**Add `send_input` beside `send_signal`, whose shape it copies exactly** —
`src/shell/client.rs:15-32`:

```rust
/// Send `sig` to the shell hosted at `sock`. Returns when the host has
/// acknowledged, which is not the same as the job having reacted.
pub async fn send_signal(sock: &Path, sig: ShellSignal, timeout: Duration) -> anyhow::Result<()> {
    let stream = UnixStream::connect(sock)
        .await
        .with_context(|| format!("connecting to shell socket at {}", sock.display()))?;
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let wire = encode(ShellRequest::Signal { signal: sig }).context("encoding Signal frame")?;
    writer
        .write_all(wire.as_bytes())
        .await
        .context("sending Signal frame")?;
    writer.flush().await.context("flushing Signal frame")?;

    read_ack(sock, &mut reader, timeout).await
}
```

Same body with `ShellRequest::Input { bytes }`. `read_ack` is already private
in that file and already maps `ShellResponse::Error { message }` to an `Err`,
so a host that rejects the write surfaces as a real error. **Do not
`Subscribe` first** — that is `start_command`'s concern (it must not miss
output); this call reads nothing back but the ack.

### The line terminator is `\n`, measured — not `\r`

`enter: true` appends a newline. Use `\n`, because that is what the working
path already sends: `wrap_command` (`src/shell/pty.rs:61-69`) ends its
payload with a bare `\n` and that is how every command in this codebase
reaches a shell. Do not send `\r` or `\r\n`.

### The approval round-trip — the helper and the one trap

`prompt_and_await_approval` (`src/daemon/executor/mod.rs:1250`) is the shared
helper; `tmux_control`'s arm is the closest analogue. Its shape, from
`src/daemon/executor/mod.rs:1016-1031`:

```rust
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
```

`ApprovalRequest` is `{ id, cmd, background, target_pane_hint }`
(`src/daemon/executor/mod.rs:37-42`). **`target_pane_hint` is `None` here** —
there is no tmux pane; this is a PTY shell.

**The trap is the third argument.** `tmux_control` passes `None` for
`ghost_policy`, and its comment says why:

```rust
            // 4. Approval. `ghost_policy` is `None` on purpose: step 1 already
            //    made the ghost decision, and passing the real policy here
            //    re-enters the auto-approve branch described in § Current state.
```

`send_input` does the same: the ghost decision is made **before** the prompt
by the gate below, and passing the live policy into the helper would let a
ghost's `auto_approve_*` settings approve keystrokes with no user present.
Pass `None`.

### The ghost gate — default-deny, and it is not the central policy check

`execute_tool_call` already applies the agent-level `ToolPolicy::permits`
centrally (`src/daemon/executor/mod.rs:224-226`). That is **not enough**: a
deny-list that merely omits `send_input` permits it. The stricter rule this
tool needs already exists for `tmux_control`
(`src/daemon/executor/mod.rs:134-139`):

```rust
pub(crate) fn ghost_may_use_tmux_control(
    is_ghost: bool,
    policy: Option<&crate::agents::policy::ToolPolicy>,
) -> bool {
    !is_ghost || policy.is_some_and(|p| p.explicitly_allows("tmux_control")) // ghost needs an explicit allow
}
```

`explicitly_allows` (`src/agents/policy.rs:37-41`) matches the **allow list
only**, so an absent policy or a deny-list-only policy returns `false`.

Add a sibling `ghost_may_send_input` with the same shape and the same
`explicitly_allows("send_input")` check. **Two copies is not the abstraction
point** — `STANDARDS.md` § 2.2 puts that at three; when a third arrives, the
two collapse into one `ghost_may_use(tool, is_ghost, policy)`. Say so in a
comment on the new function so the next reader knows it is a deliberate
second copy, not an oversight.

### Two tests will break that no compiler error will mention

Both live in `src/ai/tools/dispatch.rs` and both fail as **test failures**,
not build errors — so a run that only watches `cargo build` will think it is
finished.

**1. The category census** (`src/ai/tools/dispatch.rs:678-690`). `send_input`
is `Category::Run`, so `Run` goes **6 → 7** and the total to 46:

```rust
        assert_eq!(counts.get(&Category::Read), Some(&21), "Read count");
        assert_eq!(counts.get(&Category::Write), Some(&13), "Write count");
        assert_eq!(counts.get(&Category::Search), Some(&4), "Search count");
        assert_eq!(counts.get(&Category::Run), Some(&6), "Run count");
        assert_eq!(counts.get(&Category::Meta), Some(&1), "Meta count");
```

**Change only the `Run` line.** If a different number needs changing, the
category is wrong — fix the category, not the census.

**2. `approval_gated_tools_all_mutate_state`**
(`src/ai/tools/dispatch.rs:692-702`) walks `APPROVAL_GATED_TOOLS` and asserts
each one's category mutates state. `Category::Run` **does** mutate on this
tree (a deliberate divergence from the rexyMCP original, recorded in the
milestone README), so this passes with no edit — but it is the test that will
catch a `Category::Read` slip.

Also: `dispatch_roundtrip_all_tools` needs a conformant-args row, and
`LimitsConfig::validate` (`src/config/types.rs:834`) iterates
`APPROVAL_GATED_TOOLS` to warn about `[limits.per_tool]` entries — no edit
needed, named so the grep hit is not mistaken for work.

### `should_emit_tool_feedback` — the **opposite** of the last three phases

`CLAUDE.md` checklist step 2: emit feedback for **silent** tools; return
`false` for approval-gated ones that already have richer UI. `list_shells`,
`read_shell`, `find_in_shells`, `open_shell`, `close_shell` and `watch_shell`
all joined that `matches!` chain. **`send_input` must NOT.** It has an
approval panel; a `ToolStarted` line as well would be duplicate UI. Leave it
to the catch-all `false` and say so in the Update Log so the omission reads as
decided rather than forgotten.

`summary()` **does** get an arm — it shows the shell id, like `CloseShell`.

### The known flaky neighbour — do not chase it

`shell::pty::pause_resume_tests::pty_stop_with_no_foreground_job_remembers_nothing`
(`src/shell/pty.rs`) fails roughly **1 full-suite run in 4** on this machine.
It is **pre-existing and unrelated**: it reproduces at milestone-earlier
commits with none of this work present, and this phase does not touch
`src/shell/pty.rs`. A full-suite red naming **only** that test is a flake.
Re-run and say so; do **not** "fix" it, and do **not** let it mask a real
failure — if any *other* test name appears, that is real and must be filed.

## Current state

Values run against the tree at drafting (2026-09-14, commit `f4a5f5f0`).

- `grep -c 'ToolDef {' src/ai/tools/defs.rs` → **45**.
- `grep -rc 'send_input' --include=*.rs src/ | grep -v ':0' | wc -l` → **0**.
- `grep -c '45 tools: 33 core + 12 deferred' CLAUDE.md` → **1**;
  `grep -c 'The model has \*\*45 tools\*\*' README.md` → **1**.
- `APPROVAL_GATED_TOOLS` holds **10** entries, alphabetical:
  `create_agent, delete_agent, delete_runbook, delete_script, edit_file,
  run_terminal_command, schedule_command, tmux_control, write_runbook,
  write_script`.
- Category census: Read **21**, Write **13**, Search **4**, Run **6**,
  Meta **1** — 45 total.
- `grep -c 'pub async fn send_input' src/shell/client.rs` → **0**;
  `grep -c 'fn ghost_may_send_input' src/daemon/executor/mod.rs` → **0**.
- Tests: `cargo test --lib 'knowledge::shell'` → **37**;
  `cargo test --lib 'ai::tools'` → **22**; `cargo test --lib 'shell::'` →
  **218**; `cargo test --lib 'daemon::executor::'` → **272**;
  `cargo test --test doc_truth` → **10**; `cargo test --test isolation` →
  **9**; full lib suite **1861** (when the flake above does not fire).

## Spec

Nine tasks. Task 8 is the mutation checks and task 9 the end-to-end capture,
both as numbered Spec tasks — `docs/dev/WORKFLOW.md` § End-to-end verification
requires the capture to be a task, because the seeder parses only the section
headed exactly `## Spec`.

### Task 1 — `send_input` on the client

In `src/shell/client.rs`, beside `send_signal`, `pub` and re-exported from
`src/shell/mod.rs`:

```rust
pub async fn send_input(sock: &Path, bytes: &[u8], timeout: Duration) -> anyhow::Result<()>
```

Connect, send one `ShellRequest::Input { bytes }`, flush, `read_ack`. No
`Subscribe`. Errors carry the socket path the way `send_signal`'s do.

### Task 2 — `ghost_may_send_input`

In `src/daemon/executor/mod.rs`, beside `ghost_may_use_tmux_control`, same
shape, `explicitly_allows("send_input")`. Comment that it is a deliberate
second copy and that a third collapses them. One test, named below.

### Task 3 — `send_input` in the knowledge module

In `src/daemon/executor/knowledge/shell.rs`, after the 04b section. A public
`send_input(shell, text, enter, caller) -> String` and an injected-seam
`send_input_with(reg, shell, text, enter, caller, timeout, send) -> String`,
mirroring the `close_shell` / `close_shell_with` split already in the file.

**`send` is an injected sender**, the way `open_shell_with` takes an injected
`spawn`. The production form passes a closure calling
`crate::shell::send_input(&socket_path(shell), bytes, timeout)`; a test passes
one that records the exact bytes. This is what makes "a newline was appended"
and "nothing was sent to another owner's shell" **exact assertions on the
bytes** rather than inferences from a message, and it is why tests 1–3 need no
host at all.

Order, all refusals returned as **text**, never `Err`:

1. **Unknown id** → the `live_shell_ids` sentence, the shape `read_shell`,
   `close_shell` and `watch_shell` already share.
2. **Ownership.** A shell the caller does not own is refused, naming the id.
   Compare `Owner::key()`, **never** the inner string — a chat session and a
   ghost job can share an id string. This is the rule `close_shell_with`
   already follows and the one mutation 2 breaks.
3. Otherwise encode `text` (plus `\n` when `enter`, which **defaults true**)
   and hand the bytes to `send`.
4. Report what was sent: the shell id, the byte count, and whether a newline
   was appended. **Do not echo the text back** — the model wrote it, and the
   result is another copy in the transcript for no gain.

**Also expose the state line the panel needs.** A small pure helper —
`fn input_target_note(state: Option<ShellState>) -> String` — turning the
probe into the words the approval panel shows:

- `Running` → the shell is busy, so the input goes to the running program;
- `Idle` → **the shell is at its prompt, so this will be run as a shell
  command**;
- `Paused` → the job is stopped and will not read the input until it resumes;
- `Exited` / probe failure → the shell may be gone.

Pure and directly testable; the executor arm calls
`crate::shell::status(&socket_path(shell), …)` and passes what it got.

### Task 4 — the tool definition

`deferred_group: None`, `category: Category::Run`.

- `shell` (`ParamTy::Str`, required), `text` (`ParamTy::Str`, required),
  `enter` (`ParamTy::Bool`, optional, "Append a newline. Default true.").
- The description states the three limits from § Goal: no exit code, no wait,
  no output; read the result with `read_shell` or `watch_shell`; and a shell
  at its prompt runs the text as a command.
- **Name no tool and no parameter that does not exist.** In particular do not
  mention `wait`, `wait: false`, or a `shell` parameter on
  `run_terminal_command` — phase 07 has not landed. Cite no milestone or phase
  identifier.

**Add `"send_input"` to `APPROVAL_GATED_TOOLS`**, alphabetically (between
`schedule_command` and `tmux_control`). **Do not add `enter` or `text` to
`enum_values`** in `src/ai/tools/schema.rs`; that map is keyed by param name
globally.

### Task 5 — wire the tool

The five compiler-forced sites **in one contiguous batch, then run the
verifier once** — `to_tool_call()`, `id()`, `tool_name()` in
`src/ai/types/pending.rs`, the `AiEvent` match in `src/daemon/stream.rs`, and
the `execute_tool_call` arm. Plus `args.rs` (`SendInputArgs`, with
`#[serde(default = "default_true")]` on `enter` — add that default fn if the
file has none), `dispatch.rs` (the arm **and** a conformant-args row), the
`AiEvent` variant, and the `knowledge/mod.rs` re-export.

**The executor arm, in this order:**

1. the `ghost_may_send_input` gate — **before** any prompt, so a ghost is
   never shown a panel it cannot use;
2. the `ShellState` probe for the panel note;
3. `prompt_and_await_approval` with `target_pane_hint: None` and
   **`ghost_policy: None`** (§ Pre-flight);
4. on approval, `knowledge::send_input(...)`.

The `cmd` string the panel shows must contain the shell id, the text, and
`input_target_note`'s sentence. Render the text so control characters are
visible rather than executed — `{:?}` on the `&str` is enough and is what the
rest of this module already uses for user-supplied strings.

### Task 6 — the two tests that break without a build error

Update the `Run` count 6 → 7 in `category_counts_match_the_assignment`
(`src/ai/tools/dispatch.rs`). Add the conformant-args row for `send_input`.
Change nothing else in that file.

### Task 7 — the two doc tables and the prompt asset

- `CLAUDE.md` § "Current AI tools": count line to **`46 tools: 34 core + 12
  deferred.`** plus one row, `Loaded` = `core`.
- `README.md` § "AI tools": one Core-table row **marked `**⚠**`** (the tool is
  approval-gated; `readme_approval_markers_match_the_gated_tools` in
  `tests/doc_truth.rs` fails without it), and the numbers at "The model has
  **45 tools**" → **46**, **33** → **34**, 12 unchanged.
- `assets/prompts/sre.toml`: a ``### `send_input` `` section beside the other
  shell tools (they run from line 105; `watch_shell`'s section is at 162).
  Same honesty restriction as task 4, and it must say the tool needs user
  approval.

### Task 8 — run the mutation checks and record them

Both with the `patch` tool (`sed -i`, `perl -i` and `>` redirects are
forbidden by the executor contract). Capture each red-then-green transcript
mechanically. Each names the **property**; find the line yourself. **If a
mutation does not turn its test red, record exactly that — do not invent a
failure, and do not report an outcome the applied patch cannot produce.**

1. **A ghost must not type without an explicit allow.** Make
   `ghost_may_send_input` return `true` for a ghost whose policy is a
   deny-list that omits `send_input` (i.e. use `permits` instead of
   `explicitly_allows`) → the ghost-gate test must go **red**.
2. **Ownership is compared on `Owner::key()`, not the inner string.** Make
   `send_input_with`'s ownership check compare the inner owner string → the
   cross-owner refusal test must go **red**, because a chat session and a
   ghost job sharing an id string then pass the check.

After each, revert with `patch` and show the test green again. When you quote
a `test result:` line, **quote it from the run whose command you are
showing**.

### Task 9 — capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-m22-05.txt` into **one** new Update Log entry
headed `### Update — <date> (end-to-end verification)`, and record the
`PASTE MATCH` self-check verdict in the same entry.

**Write exactly one such entry.** The self-check anchors on the **last**
heading matching that text; a second capture entry moves the anchor and the
check then measures the wrong block. If you must re-capture, **replace** the
entry's fenced block rather than appending a second entry, and say so.

**Section F runs the lib suite three times and keeps failure names.** If a run
fails naming **only**
`pty_stop_with_no_foreground_job_remembers_nothing`, that is the known flake
from § Pre-flight — record it with the run that shows it, re-run, and say
plainly what happened. **Any other failing test name is real**: file it, name
it, do not re-run until green.

## Acceptance criteria

Each was run against the tree at drafting and returns the "now" value shown.
`<S>` abbreviates
`sed -n '/name: "send_input"/,/^    },/p' src/ai/tools/defs.rs |`.

- [ ] **The tool exists, core + Run:**
      `grep -c 'name: "send_input"' src/ai/tools/defs.rs` → **1** (now `0`);
      `<S> grep -c 'category: Category::Run'` → **1**;
      `<S> grep -c 'deferred_group: None'` → **1**.
- [ ] **Its three parameters, two required:**
      `<S> grep -cE 'name: "(shell|text|enter)"'` → **3**;
      `<S> grep -c 'required: true'` → **2**;
      `<S> grep -cE 'name: "(pane_id|wait|host)"'` → **0**.
- [ ] **Approval-gated — both halves.** In the list:
      `sed -n '/APPROVAL_GATED_TOOLS: &\[&str\] = &\[/,/^\];/p' src/ai/tools/defs.rs | grep -c 'send_input'`
      → **1** (now `0`), **and** a real prompt was added — the file's approval
      call sites go up by exactly one:
      `grep -c 'prompt_and_await_approval(' src/daemon/executor/mod.rs` → **4**
      (now **3**). *(The list entry alone would hand a keystroke tool an
      uncapped budget, so neither half alone is enough.)*
- [ ] **The ghost gate exists and is default-deny:**
      `grep -c 'fn ghost_may_send_input' src/daemon/executor/mod.rs` → **1**
      (now `0`), and
      `sed -n '/fn ghost_may_send_input/,/^}/p' src/daemon/executor/mod.rs | grep -c 'explicitly_allows'`
      → **1** (now `0`). The behaviour itself is pinned by
      `ghost_may_send_input_requires_an_explicit_allow` and by mutation 1 —
      **not** by a grep, because "default-deny" is not a string.
- [ ] **Counts moved together:** `grep -c 'ToolDef {' src/ai/tools/defs.rs` →
      **46** (now `45`); `grep -c '46 tools: 34 core + 12 deferred' CLAUDE.md`
      → **1** (now `0`); `grep -c '45 tools: 33 core + 12 deferred' CLAUDE.md`
      → **0** (now `1`); `grep -c 'The model has \*\*46 tools\*\*' README.md`
      → **1** (now `0`).
- [ ] **The README carries the approval marker:**
      `grep -c '| \`send_input\` \*\*⚠\*\* |' README.md` → **1** (now `0`).
      *(`readme_approval_markers_match_the_gated_tools` enforces this.)*
- [ ] **Only the `Run` census line moved:**
      `sed -n '/fn category_counts_match_the_assignment/,/^    }/p' src/ai/tools/dispatch.rs | grep -c 'Category::Run), Some(&7)'`
      → **1** (now `0`), and the same range
      `grep -cE 'Some\(&21\).*Read|Some\(&13\).*Write|Some\(&4\).*Search|Some\(&1\).*Meta'`
      → **4**, unchanged.
- [ ] **`send_input` is NOT in the silent-feedback chain:**
      `sed -n '/pub fn should_emit_tool_feedback/,/^    }/p' src/ai/types/pending.rs | grep -c 'SendInput'`
      → **0**, **and** it has a `summary` arm:
      `sed -n '/pub fn summary(&self)/,/^    }/p' src/ai/types/pending.rs | grep -c 'SendInput'`
      → **1** (now `0`). *(Neither is enforced by any gate, and they point
      opposite ways — this is the one phase in the row where the answer to the
      chain is "no".)*
- [ ] **`enum_values` untouched:**
      `sed -n '/pub(super) fn enum_values/,/^}/p' src/ai/tools/schema.rs | grep -cE '"text"|"enter"'`
      → **0**, and the same range `grep -c '"session", "knowledge", "incident"'`
      → **1**, unchanged.
- [ ] **The client sender exists and is re-exported:**
      `grep -c 'pub async fn send_input' src/shell/client.rs` → **1** (now
      `0`); `grep -c 'send_input' src/shell/mod.rs` → **1** (now `0`); and it
      does not subscribe:
      `sed -n '/pub async fn send_input/,/^}/p' src/shell/client.rs | grep -c 'Subscribe'`
      → **0**.
- [ ] **The sender is injected, so the bytes are assertable:**
      `sed -n '/fn send_input_with/,/^}/p' src/daemon/executor/knowledge/shell.rs | grep -c 'send('`
      → **1 or more**. *(The newline being `\n` rather than `\r` is pinned by
      `send_input_appends_a_newline_by_default` asserting the captured bytes
      exactly — a byte **count** cannot tell the two apart, which is why the
      seam exists.)*
- [ ] **Ownership is checked on `key()`:**
      `sed -n '/fn send_input_with/,/^}/p' src/daemon/executor/knowledge/shell.rs | grep -c 'key()'`
      → **1 or more**.
- [ ] **The rest of the wiring:**
      `grep -c 'SendInput' src/ai/types/pending.rs` → **5 or more**;
      `grep -c 'SendInput' src/ai/types/events.rs` → **1**;
      `grep -c 'SendInput' src/daemon/stream.rs` → **2 or more**;
      `grep -c 'SendInput' src/daemon/executor/mod.rs` → **1 or more**;
      `grep -c 'send_input' src/ai/tools/dispatch.rs` → **2 or more**;
      `grep -c 'SendInputArgs' src/ai/tools/args.rs` → **2 or more**;
      `grep -c 'send_input' assets/prompts/sre.toml` → **1 or more**.
      All now `0`.
- [ ] **The prompt asset stays honest:**
      ``sed -n '/^### `send_input`/,/^### /p' assets/prompts/sre.toml | grep -cE 'wait:|03a|03b|03c|04a|04b|05|M2[0-9]'``
      → **0**, and the same range `grep -ci 'approv'` → **1 or more**.
- [ ] **Existing shell tools untouched:**
      `grep -cE '^    (async )?fn (list_shells|read_shell|find_in_shells|formatter|open_shell|close_shell|watch_shell|visible_text)' src/daemon/executor/knowledge/shell.rs`
      → **37 or more** (now **37**).
- [ ] **The eight new tests exist by name:**
      `grep -cE '^    (async )?fn (send_input_appends_a_newline_by_default|send_input_without_enter_sends_no_newline|send_input_refuses_another_owners_shell|send_input_unknown_id_is_an_error_naming_live_shells|send_input_text_never_reaches_read_shell|input_target_note_names_the_prompt_hazard_when_idle)' src/daemon/executor/knowledge/shell.rs`
      → **6** (now `0`);
      `grep -cE '^    fn (ghost_may_send_input_requires_an_explicit_allow)' src/daemon/executor/mod.rs`
      → **1** (now `0`);
      `grep -cE '^    (async )?fn (send_input_frame_is_acked_by_the_host)' src/shell/client.rs`
      → **1** (now `0`).
- [ ] No new `unsafe` / `unwrap` / `expect` / `panic!` in **production** code:
      `sed -n '1,/^#\[cfg(test)\]/p' src/daemon/executor/knowledge/shell.rs | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe '`
      → **1**, unchanged — the one marker-carrying lazy-regex `expect`; and
      `sed -n '1,/^#\[cfg(test)\]/p' src/shell/client.rs | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe '`
      → **0**, unchanged.
- [ ] **No lint-silencing shim:**
      `git diff f4a5f5f0..HEAD -- 'src/*.rs' | grep -cE '^\+.*#\[allow'` → **0**.
- [ ] `cargo test --lib 'knowledge::shell'` → **43 or more**, `0 failed` (now
      **37**). *Arithmetic: 37 existing + 6 named above.*
- [ ] `cargo test --lib 'ai::tools'` → **22**, `0 failed`, unchanged.
- [ ] `cargo test --test doc_truth` → **10**, `0 failed`, unchanged.
- [ ] **Preservation:** `cargo test --lib 'config::'` → **92**;
      `cargo test --test isolation` → **9**;
      `cargo test --lib 'shell::'` → **219 or more** (now **218**);
      `cargo test --lib 'daemon::executor::'` → **279 or more** (now **272**).
- [ ] Full lib suite **1869 or more**, `0 failed` (now **1861**). Three
      consecutive runs; a run failing **only**
      `pty_stop_with_no_foreground_job_remembers_nothing` is the known flake —
      record it and re-run.
- [ ] All four gates pass.

## Test plan

Eight new tests. Six in `src/daemon/executor/knowledge/shell.rs`, one in
`src/daemon/executor/mod.rs`, one in `src/shell/client.rs`.

**Only two of them need a real host.** With the injected `send`, tests 1–4 are
driven from a hand-built registry and need no socket at all; test 5 needs a
real in-process host because it must exercise the genuine write-and-log path,
and test 8 exercises the client against one. Test 6 and 7 are pure. Tests that
touch `cast_path`/`socket_path` still hold `lifecycle_home()` for their whole
body.

1. **`send_input_appends_a_newline_by_default`** — a recording `send`
   closure, `enter` omitted; the captured bytes are **exactly** `b"hello\n"`.
   Assert the whole byte string, not its length: a length assertion cannot
   tell `\n` from `\r`, and that distinction is the point.
2. **`send_input_without_enter_sends_no_newline`** — same with
   `enter: false`; the captured bytes are exactly `b"hello"`. The pair is what
   makes either assertion meaningful.
3. **`send_input_refuses_another_owners_shell`** — a shell owned by
   `Owner::Ghost("a")`, a caller of `Owner::Chat("a")` — **the same inner
   string, different kind**. The refusal names the id, the shell is still in
   the registry, and **the `send` closure never ran** (assert on a flag it
   sets). Mutation 2's test: comparing the inner string lets this through and
   types into a ghost's shell.
4. **`send_input_unknown_id_is_an_error_naming_live_shells`** — an id absent
   from a non-empty registry returns text naming it and listing the live ids,
   and the function returns a `String`, not a `Result`.
5. **`send_input_text_never_reaches_read_shell`** — the safety property from
   § Pre-flight. Send a distinctive string (e.g. `"SENTINEL-NOT-IN-OUTPUT"`)
   to a real host, then call `read_shell` on the same id and assert the
   result **does not contain it**. This is the test that would catch a future
   reader that stopped filtering on `"o"`.
6. **`input_target_note_names_the_prompt_hazard_when_idle`** — pure. `Idle`
   produces a sentence saying the input will be run as a shell command;
   `Running` does not. Assert both directions — a note that says the same
   thing in every state tells the approving user nothing.
7. **`ghost_may_send_input_requires_an_explicit_allow`** (in
   `src/daemon/executor/mod.rs`) — non-ghost → `true`; ghost with `None` →
   `false`; ghost with a **deny-list omitting** `send_input` → `false`; ghost
   with an allow-list containing it → `true`. The deny-list case is the one
   `permits` would get wrong and is mutation 1's target.
8. **`send_input_frame_is_acked_by_the_host`** (in `src/shell/client.rs`) —
   against a real in-process host, `send_input` returns `Ok`. `client.rs`'s
   existing tests show the fixture shape.

**Negatives are named on purpose.** A suite asserting only that input reaches
a shell passes against an implementation that types into anyone's shell,
lets a ghost type unattended, drops the newline, and leaks the typed text
back through `read_shell`.

## End-to-end verification

Run from the repo root, verbatim:

```sh
A=/tmp/e2e-m22-05.txt
: > "$A"
CASTS_BEFORE=$(ls ~/.daemoneye/var/log/shells/ 2>/dev/null | wc -l)
{
echo "== A. build =="
echo "real HOME shell logs BEFORE: $CASTS_BEFORE"
cargo build 2>&1 | tail -2; echo "cargo_exit=${PIPESTATUS[0]}"

echo "== B. the touched modules' tests =="
cargo test --lib 'knowledge::shell' 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | grep -E '^test result'
cargo test --lib 'shell::client' 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | grep -E '^test result'
cargo test --lib 'ai::tools' 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | grep -E '^test result'
cargo test --lib 'daemon::executor::' 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | grep -E '^test result'

echo "== C. the audit property, shown not asserted =="
echo "--- both cast readers filter to \"o\" events only ---"
grep -n 'Some("o")' src/shell/log.rs
echo "--- the host records every Input frame in the cast ---"
sed -n '/async fn input(&self/,/^    }/p' src/shell/backend.rs

echo "== D. structural greps (each must print the stated number) =="
D=src/ai/tools/defs.rs; S=src/ai/tools/schema.rs; K=src/daemon/executor/knowledge/shell.rs
P=src/ai/types/pending.rs; X=src/daemon/executor/mod.rs; C=src/shell/client.rs
SD() { sed -n '/name: "send_input"/,/^    },/p' $D; }
FN() { sed -n '/fn send_input_with/,/^}/p' $K; }
printf 'send_input defined           (1): %s\n' "$(grep -c 'name: "send_input"' $D)"
printf 'category Run                 (1): %s\n' "$(SD | grep -c 'category: Category::Run')"
printf 'deferred_group None          (1): %s\n' "$(SD | grep -c 'deferred_group: None')"
printf 'three params                 (3): %s\n' "$(SD | grep -cE 'name: "(shell|text|enter)"')"
printf 'two required                 (2): %s\n' "$(SD | grep -c 'required: true')"
printf 'no pane/wait/host param      (0): %s\n' "$(SD | grep -cE 'name: "(pane_id|wait|host)"')"
printf 'ToolDef literals            (46): %s\n' "$(grep -c 'ToolDef {' $D)"
printf 'categories assigned         (46): %s\n' "$(grep -c 'category: Category::' $D)"
printf 'in APPROVAL_GATED_TOOLS      (1): %s\n' "$(sed -n '/APPROVAL_GATED_TOOLS: &\[&str\] = &\[/,/^\];/p' $D | grep -c 'send_input')"
printf 'approval call sites          (4): %s\n' "$(grep -c 'prompt_and_await_approval(' $X)"
printf 'ghost gate exists            (1): %s\n' "$(grep -c 'fn ghost_may_send_input' $X)"
printf 'gate is default-deny         (1): %s\n' "$(sed -n '/fn ghost_may_send_input/,/^}/p' $X | grep -c 'explicitly_allows')"
printf 'Run census is 7              (1): %s\n' "$(sed -n '/fn category_counts_match_the_assignment/,/^    }/p' src/ai/tools/dispatch.rs | grep -c 'Category::Run), Some(&7)')"
printf 'other census lines intact    (4): %s\n' "$(sed -n '/fn category_counts_match_the_assignment/,/^    }/p' src/ai/tools/dispatch.rs | grep -cE 'Some\(&21\).*Read|Some\(&13\).*Write|Some\(&4\).*Search|Some\(&1\).*Meta')"
printf 'NOT in silent chain          (0): %s\n' "$(sed -n '/pub fn should_emit_tool_feedback/,/^    }/p' $P | grep -c 'SendInput')"
printf 'has a summary arm            (1): %s\n' "$(sed -n '/pub fn summary(&self)/,/^    }/p' $P | grep -c 'SendInput')"
printf 'enum_values untouched        (0): %s\n' "$(sed -n '/pub(super) fn enum_values/,/^}/p' $S | grep -cE '"text"|"enter"')"
printf 'memory param set intact      (1): %s\n' "$(sed -n '/pub(super) fn enum_values/,/^}/p' $S | grep -c '"session", "knowledge", "incident"')"
printf 'client sender                (1): %s\n' "$(grep -c 'pub async fn send_input' $C)"
printf 'client sender re-exported    (1): %s\n' "$(grep -c 'send_input' src/shell/mod.rs)"
printf 'sender does not subscribe    (0): %s\n' "$(sed -n '/pub async fn send_input/,/^}/p' $C | grep -c 'Subscribe')"
printf 'sender is injected          (1+): %s\n' "$(FN | grep -c 'send(')"
printf 'ownership on key()          (1+): %s\n' "$(FN | grep -c 'key()')"
printf 'pending wiring              (5+): %s\n' "$(grep -c 'SendInput' $P)"
printf 'events wiring                (1): %s\n' "$(grep -c 'SendInput' src/ai/types/events.rs)"
printf 'stream wiring               (2+): %s\n' "$(grep -c 'SendInput' src/daemon/stream.rs)"
printf 'dispatch wiring             (2+): %s\n' "$(grep -c 'send_input' src/ai/tools/dispatch.rs)"
printf 'args wiring                 (2+): %s\n' "$(grep -c 'SendInputArgs' src/ai/tools/args.rs)"
printf 'sre.toml documents it       (1+): %s\n' "$(grep -c 'send_input' assets/prompts/sre.toml)"
printf 'sre.toml stays honest        (0): %s\n' "$(sed -n '/^### `send_input`/,/^### /p' assets/prompts/sre.toml | grep -cE 'wait:|03a|03b|03c|04a|04b|05|M2[0-9]')"
printf 'sre.toml says approval      (1+): %s\n' "$(sed -n '/^### `send_input`/,/^### /p' assets/prompts/sre.toml | grep -ci 'approv')"
printf 'CLAUDE.md new count          (1): %s\n' "$(grep -c '46 tools: 34 core + 12 deferred' CLAUDE.md)"
printf 'CLAUDE.md old count gone     (0): %s\n' "$(grep -c '45 tools: 33 core + 12 deferred' CLAUDE.md)"
printf 'README new count             (1): %s\n' "$(grep -c 'The model has \*\*46 tools\*\*' README.md)"
printf 'README approval marker       (1): %s\n' "$(grep -c '| `send_input` \*\*⚠\*\* |' README.md)"
printf 'existing shell tools       (37+): %s\n' "$(grep -cE '^    (async )?fn (list_shells|read_shell|find_in_shells|formatter|open_shell|close_shell|watch_shell|visible_text)' $K)"
printf 'six knowledge tests          (6): %s\n' "$(grep -cE '^    (async )?fn (send_input_appends_a_newline_by_default|send_input_without_enter_sends_no_newline|send_input_refuses_another_owners_shell|send_input_unknown_id_is_an_error_naming_live_shells|send_input_text_never_reaches_read_shell|input_target_note_names_the_prompt_hazard_when_idle)' $K)"
printf 'ghost gate test              (1): %s\n' "$(grep -cE '^    fn (ghost_may_send_input_requires_an_explicit_allow)' $X)"
printf 'client test                  (1): %s\n' "$(grep -cE '^    (async )?fn (send_input_frame_is_acked_by_the_host)' $C)"
printf 'prod unwrap knowledge        (1): %s\n' "$(sed -n '1,/^#\[cfg(test)\]/p' $K | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe ')"
printf 'prod unwrap client           (0): %s\n' "$(sed -n '1,/^#\[cfg(test)\]/p' $C | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe ')"
printf 'no new allow shim            (0): %s\n' "$(git diff f4a5f5f0..HEAD -- 'src/*.rs' | grep -cE '^\+.*#\[allow')"

echo "== E. preservation: config 92, isolation 9, doc_truth 10 =="
printf '%-26s ' 'config::'; cargo test --lib 'config::' 2>&1 | grep '^test result'
printf '%-26s ' 'shell::'; cargo test --lib 'shell::' 2>&1 | grep '^test result'
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

Then the paste self-check. **Anchor on the transcript fence.** Lay the entry
out with the transcript as the **first** fenced block after the heading, so
`c==1` selects it:

```sh
D=docs/dev/milestones/M22-tool-surface/phase-05-send-input.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m22-05.txt
diff /tmp/pasted-m22-05.txt /tmp/e2e-m22-05.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

Paste the captured block **unmodified** — do not retype, trim, or realign it.

## Authorizations

The executor may modify **only**:

- `src/shell/client.rs` — task 1 and its one test. **Do not change
  `send_signal`, `status`, `run_command`, `spawn_command`, `start_command`,
  `read_ack` or `read_one`.**
- `src/shell/mod.rs` — the `send_input` re-export line only.
- `src/daemon/executor/knowledge/shell.rs` — task 3. **Do not change
  `list_shells`, `read_shell`, `find_in_shells`, `open_shell`, `close_shell`,
  `watch_shell` or any of the 37 existing tests**; a criterion pins their
  names.
- `src/daemon/executor/knowledge/mod.rs` — the re-export line only.
- `src/daemon/executor/mod.rs` — task 2 (the gate + its test) and task 5's
  arm. **Do not change `ghost_may_use_tmux_control`,
  `ghost_may_run_foreground`, `prompt_and_await_approval` or `ApprovalRequest`.**
- `src/ai/types/pending.rs`, `src/ai/types/events.rs`, `src/ai/tools/args.rs`,
  `src/ai/tools/defs.rs`, `src/daemon/stream.rs` — task 5's wiring only.
- `src/ai/tools/dispatch.rs` — task 6 only: the `Run` census number and the
  conformant-args row. Nothing else in that file.
- `CLAUDE.md` (§ "Current AI tools" table + count line only), `README.md`
  (§ "AI tools" tables + the tool-count prose only), `assets/prompts/sre.toml` —
  task 7.
- `docs/dev/milestones/M22-tool-surface/phase-05-send-input.md` —
  **append to the Update Log only.** Do not rewrite, reorder or delete any
  section above it; the reviewer checks that every `## ` heading survives.

No other file. In particular **do not touch `src/shell/proto.rs`,
`src/shell/host.rs`, `src/shell/backend.rs`, `src/shell/pty.rs`,
`src/shell/registry.rs`, `src/shell/session.rs` or `src/shell/log.rs`** — the
wire already carries `Input` and the host already logs it; this phase adds a
sender, not a protocol. Also do not touch `tests/doc_truth.rs`,
`src/config/types.rs` or `src/agents/policy.rs`.

## Out of scope

- **A `CredentialPrompt` path for secrets.** The design routes PTY password
  prompts to the chat credential panel so the secret never reaches the log or
  the model. That is a different mechanism with a different IPC surface;
  `send_input` carries text the model already wrote. Do not mask, and do not
  add a credential branch.
- **Reading back what the input produced.** `send_input` returns when the host
  acks the write. `read_shell` and `watch_shell` are how the caller sees the
  effect, and the description says so.
- **Refusing input to an `Idle` shell.** The panel names the hazard; a refusal
  would block legitimate use and the user has been shown what will happen.
- **`wait: false` on `run_terminal_command`.** Phase 07.
- **Generalising the two ghost gates** into one `ghost_may_use(tool, …)`.
  `STANDARDS.md` § 2.2 puts the abstraction point at the third copy; this is
  the second.
- **Fixing `pty_stop_with_no_foreground_job_remembers_nothing`.** Pre-existing
  flake, its own task, and `src/shell/pty.rs` is not modifiable here.
- **Removing any pane tool.** Phase 09.

## Update Log

<!-- entries appended below this line -->

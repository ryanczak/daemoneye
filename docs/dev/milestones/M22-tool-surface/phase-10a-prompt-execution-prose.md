# Phase 10a: the seeded prompts stop describing a substrate they no longer run on

**Milestone:** M22 — Tool surface & context
**Status:** in-progress
**Depends on:** 01 (`backend = "pty"` default), 07b (`wait?`), 09b
(`watch_pane` deleted), 09d (`target_pane` retired from the run tool), 08b
(`get_shell_context`). All **done**.
**Estimated diff:** ~190 lines
**Tags:** language=rust, kind=prose+gate, size=m

## Goal

Three model-facing strings tell the model things that are **false on the
default configuration**. This phase makes them true and adds the gate that
would have caught them.

1. **`assets/prompts/sre.toml`'s `run_terminal_command — two modes` section**
   describes the tmux path as though it were the only path. Since phase 01 made
   `backend = "pty"` the default, neither mode uses a tmux window or the user's
   pane in a chat session.
2. **`assets/prompts/ghost-shell.txt` rule 7** instructs the ghost to call
   **`watch_pane`**, a tool phase 09b deleted. A ghost that follows its own
   operating rules emits a tool call that dispatches to nothing.
3. **`src/daemon/background/run.rs`'s result text**, at **two** sites, tells
   the model `Use target="%N" to run follow-up commands in the same shell`.
   `run_terminal_command` has no `target` parameter and never had one;
   `target_pane`, the nearest thing, was retired in 09d.

This is not a tmux-vocabulary cleanup. Every item above is a **false
instruction** the model acts on. The pane *vocabulary* that is still accurate —
the `[PANE MAP]` and `[FOREGROUND TARGET]` context blocks, and `target_pane` on
`read_file` / `edit_file` — **stays**, and is pinned by must-survive criteria.

## Architecture references

- `CLAUDE.md` § "Request/Response lifecycle" and § "Current AI tools" — the
  surviving surface these strings must agree with.
- The milestone README's note *"03a: the Spec dictated a tool description …
  written as if phase 07's reshape had landed … and the executor faithfully
  wrote the false sentence into a prompt the model reads on every request."*
  This phase is the same class, found in the assets rather than written into
  them.

## Pre-flight

Run these and confirm, at HEAD `35fe59cc`:

```sh
git rev-parse --short HEAD                                   # 35fe59cc
grep -c 'watch_pane' assets/prompts/ghost-shell.txt          # 1
grep -c 'watch_pane' src/ai/tools/defs.rs                    # 0  (the tool is gone)
grep -c 'var/log/panes' assets/prompts/sre.toml              # 2
cargo test --lib | tail -3                                   # 1885 passed, 4 ignored
cargo test --test doc_truth | tail -3                        # 10 passed
```

## Current state

### What the routing actually does — measured, not inferred

`src/daemon/executor/mod.rs` gates **both** run modes on the same predicate,
at two sites (the `wait: true` arm and the `wait: false` arm):

```rust
            let uses_pty = crate::config::Config::load()
                .unwrap_or_default()
                .execution
                .uses_pty();
            if uses_pty && !is_ghost && session_id.is_some() {
```

`ExecutionConfig::uses_pty()` (`src/config/types.rs`) is
`!self.backend.trim().eq_ignore_ascii_case("tmux")` — so the default `"pty"`
**and any unrecognised value** take the PTY branch. The consequence, and the
whole reason for this phase:

| Caller | `wait: true` | `wait: false` |
|---|---|---|
| **Chat session, default backend** | PTY shell on the daemon host | background PTY shell; returns a shell id |
| **Ghost session** | the user's pane via `send-keys` | `de-gs-bg-*` / `de-gs-sj-*` tmux window |
| **`backend = "tmux"` opt-out** | the user's pane via `send-keys` | `de-bg-*` tmux window |

The non-blocking PTY branch's own result text is the accurate description of
that path, and the prose should agree with it:

```
Running in background on shell s3 (run 7). The result is not available yet — it
will be delivered later as a [Background Task Completed] message. Continue with
other work or end your turn; do not poll for it.
```

### The three defects, quoted

**1. `assets/prompts/sre.toml`, the `### \`run_terminal_command\` — two modes`
section** (line 72 onward). The false bullets:

```
**`wait=false`** (daemon host, non-blocking):
- Runs in a `de-bg-*` tmux window on the daemon host; returns immediately with \
  the shell id the command runs in.
…
- 5-window cap; a finished window is reclaimed automatically on the daemon's \
  own sweep, so there is nothing to close.
- Full output archived to `~/.daemoneye/var/log/panes/<win>.log`.
…
**`wait=true` (the default — omit `wait`)** (user's pane, synchronous):
- Injects via `send-keys`, waits for completion, returns captured output.
- If the pane is SSH'd, the command runs on the remote host. The daemon picks \
  the pane from `[FOREGROUND TARGET]`; never target the chat pane.
```

Also stale, a few lines below: **Interactive commands** claims *"the daemon
keeps targeting the pane you connected in"*, which is the pane path's
behaviour, not the PTY path's.

**2. `assets/prompts/ghost-shell.txt`, rule 7:**

```
7. **Wait for background commands.** After launching a background command, call
   `watch_pane` to block until it exits before issuing further tool calls.
   Never assume a background command succeeded without observing its output.
```

`watch_pane` is **not in `TOOLS`** (verified: `grep -c 'watch_pane'
src/ai/tools/defs.rs` → 0). The five surviving `watch_pane` strings in `src/`
are two doc comments, one test name, and two `dispatch.rs` assertions pinning
its absence — none is a tool.

**Do not simply substitute `watch_shell`.** A ghost does not use the PTY
substrate (`!is_ghost` in the gate above), so its background command runs in a
tmux window and has **no shell id** for `watch_shell` to take. That
substitution would replace one false instruction with another. Rule 7's correct
content is in Task 2.

**3. `src/daemon/background/run.rs`** — the same sentence at **two** sites,
measured at drafting (`grep -n 'Use target=' src/daemon/background/run.rs` → 2).

The dispatch string, at the tail of `run_background_in_window`:

```rust
            format!(
                "Background command sent to pane {pane_id} (window {win_name}). \
                 You will receive a [Background Task Completed] context message when it finishes. \
                 Use target=\"{pane_id}\" to run follow-up commands in the same shell."
            )
```

And the completion string's `persist_note`, ~160 lines earlier:

```rust
            let persist_note = if pane_persists {
                format!(
                    "The window is still open (pane {pane_id}). \
                     Use target=\"{pane_id}\" to run follow-up commands in the same shell."
                )
            } else {
```

Every other sentence in both is true. The `Use target=` sentence names a
parameter that does not exist on any tool.

### What is still true and must not be touched

- `sre.toml`'s **`### Pane targeting`** and **`### Context annotations`**
  sections. The `[PANE MAP]`, `[FOREGROUND TARGET]` and `[Pane target changed]`
  blocks are still emitted; M23 owns the context blocks. Prose explaining how to
  read a block the model still receives is load-bearing.
- `sre.toml`'s `read_file` / `edit_file` **`target_pane`** documentation. Those
  two tools kept the parameter, deliberately, until M21's `host?`.
- `ghost-shell.txt`'s `de-gs-bg-*` / `de-gs-sj-*` window names, its sudo rules
  and its delegation rules. Ghosts really do run in those windows.

## Spec

### Task 1 — rewrite the two-mode section in `assets/prompts/sre.toml`

Replace the `**\`wait=false\`**` and `**\`wait=true\`**` bullet groups, and the
**Interactive commands** paragraph, with the text below. Use it **verbatim** —
this is the prompt the model reads on every request, and a paraphrase is how a
false sentence got into it last time.

```
**`wait=true` (the default — omit `wait`)** — synchronous:
- Runs in a PTY shell on the daemon host and returns the captured output with \
  the command's real exit code.
- `shell` selects which PTY shell runs it: a shell id from `list_shells` targets \
  that shell, `new` opens one for this command, and omitting it reuses this \
  session's lowest-numbered open shell (another session's shell is refused).
- `timeout_secs` bounds the wait — default 45 s, capped at 3600; the result \
  names the applied timeout when a command outlives it.
- Supports sudo (chat prompt).

**`wait=false`** — non-blocking:
- Starts the command in a dedicated background PTY shell and returns \
  immediately with that shell's id and the run id.
- The result is **not** in that return value. It arrives later as a \
  `[Background Task Completed]` message — continue with other work or end your \
  turn, and do not poll for it.
- Follow the run with `watch_shell` (block until a line appears or the command \
  exits) or `read_shell` (read what it has recorded so far).
- Runs on the daemon host even if the user is SSH'd elsewhere. Supports sudo \
  (chat prompt).

**Where commands actually run.** Both modes use the PTY substrate, which is the \
default (`[execution] backend = "pty"`). Two callers do not: a ghost shell, and \
an installation that sets `backend = "tmux"`. Those run foreground commands in \
the user's pane via `send-keys` and background commands in a `de-bg-*` / \
`de-gs-bg-*` tmux window, archiving output to `~/.daemoneye/var/log/panes/`.

**Interactive commands** (`ssh`, `mosh`, `telnet`, `screen`): return on \
remote-prompt appearance, with the shell left connected. Send follow-up work to \
that same shell by passing its `shell` id — do not re-`ssh`. (`ssh host cmd` \
and `ssh -N` run normally.)
```

Leave the **Sudo for recurring scripts** and **Python over complex shell
pipelines** paragraphs that follow exactly as they are.

### Task 2 — fix rule 7 in `assets/prompts/ghost-shell.txt`

Replace rule 7 with this text, verbatim:

```
7. **Wait for background commands.** A background command returns as soon as it
   is dispatched, not when it finishes. Its outcome arrives as a
   `[Background Task Completed]` context message — wait for that message before
   issuing further tool calls, and never assume a background command succeeded
   without observing its output. Do not call a watch tool: your commands run in
   a tmux window, not a PTY shell, so there is no shell id to watch.
```

Also correct the header line, which names a parameter spelling that phase 07b
retired:

```
All terminal commands MUST use background mode (they run in de-gs-bg-* or de-gs-sj-* windows).
```

becomes

```
All terminal commands MUST pass `wait: false` (they run in de-gs-bg-* or de-gs-sj-* windows).
```

### Task 3 — drop the phantom parameter from the background result text

In `src/daemon/background/run.rs`, delete the `Use target=` sentence at **both**
sites. The dispatch string becomes:

```rust
            format!(
                "Background command sent to pane {pane_id} (window {win_name}). \
                 You will receive a [Background Task Completed] context message when it finishes."
            )
```

and the `persist_note` branch becomes:

```rust
            let persist_note = if pane_persists {
                format!("The window is still open (pane {pane_id}).")
            } else {
```

Do not invent a replacement sentence at either site. There is no parameter that
reuses a window, by design — 09d retired `retry_in_pane` for exactly that
reason. Leave the `else` branch, which names the log path, untouched.

### Task 4 — the gate that would have caught this

Add a test to `tests/doc_truth.rs` named
`prompt_assets_name_no_retired_tools`. It must:

- Read both prompt assets with `include_str!` — `assets/prompts/sre.toml` and
  `assets/prompts/ghost-shell.txt`. Follow `src/config/seeds.rs`'s idiom:

  ```rust
  pub(crate) const SRE_PROMPT_TOML: &str = include_str!("../../assets/prompts/sre.toml");
  ```

  so the gate reads the same bytes that get seeded, not a file the test located
  at runtime.
- Build the live tool-name set from `TOOLS` (the same source
  `claude_md_tools_table_matches_the_code` already uses in this file — follow
  that test's access pattern).
- For every name in this **retired list**, assert it appears in **neither**
  asset: `list_panes`, `read_pane`, `find_in_panes`, `watch_pane`,
  `tmux_control`, `close_background_window`, `get_terminal_context`.
- Assert that every name matched by the heading form `^### \`<name>\`` in
  `sre.toml` is present in `TOOLS`. A heading may name two tools separated by
  ` / ` (`### \`list_shell_logs\` / \`read_shell_log\``) — split on `/` and
  trim backticks before the lookup. Measured at drafting: eleven headings, all
  eleven currently name live tools, so this half passes before and after and
  exists to keep passing.
- **Negative pin:** the test must **not** assert that the string `pane` is
  absent. Both assets legitimately discuss panes — the `[PANE MAP]` block, the
  ghost windows, `target_pane` on `read_file` / `edit_file`. A blanket ban would
  fail on correct prose and would have to be weakened later. Assert on **tool
  names**, which is the property that can actually be wrong.

Failure messages must name the offending token and the asset it was found in,
so a future failure is actionable without reading the test.

### Task 5 — end-to-end capture

Add a second test in `tests/doc_truth.rs` named
`e2e_prompt_asset_tool_hygiene` that prints one line and asserts each fact it
prints:

```
tool=watch_shell retired_in_prompts=0 headings_live=11 phantom_target_param=0
```

- `retired_in_prompts` — occurrences of the seven retired names across both
  assets, which must be 0.
- `headings_live` — the count of `### \`…\`` headings in `sre.toml` whose names
  are all live tools, which must equal the total heading count (11).
- `phantom_target_param` — occurrences of the literal `Use target=` in
  `src/daemon/background/run.rs`, read with `include_str!`, which must be 0.

Capture it with:

```sh
cargo test --test doc_truth e2e_prompt_asset_tool_hygiene -- --nocapture \
  | sed -n '/PASTE MATCH >>>/,/<<</p' > /tmp/e2e-10a.txt
```

Paste the captured block into an Update Log entry titled
`### Update — <date> (end-to-end verification)`, and **put the capture in that
entry's first fenced block**, before the mutation output. The self-check below
extracts the first fence after the heading, so a mutation fence placed first
makes it compare the wrong block:

```sh
D=docs/dev/milestones/M22-tool-surface/phase-10a-prompt-execution-prose.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-10a.txt
diff /tmp/pasted-10a.txt /tmp/e2e-10a.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

The literal `PASTE MATCH` line goes **into** the entry. The server-authored
`(complete)` entry does **not** satisfy this.

### Task 6 — mutation pair

Run both, paste both `test result:` lines into the Update Log **after** the
capture block.

**Mutation A — put the retired tool back.** Restore `watch_pane` in
`ghost-shell.txt` rule 7 (change `watch tool` to `` `watch_pane` ``).
`prompt_assets_name_no_retired_tools` must go **red**, and its message must name
`watch_pane` and the ghost asset. Revert; green.

**Mutation B — reintroduce the phantom parameter.** Restore the
`Use target=\"{pane_id}\" …` sentence in `run.rs`.
`e2e_prompt_asset_tool_hygiene` must go **red**. Revert; green.

If either mutation leaves its test green, **stop and say so in the Update
Log** — the gate is not pinning the property.

## Acceptance criteria

Each was run against HEAD `35fe59cc` and **fails** there; "now" is the measured
current value.

- [ ] `grep -c 'watch_pane' assets/prompts/ghost-shell.txt` → **0** (now 1)
- [ ] `grep -c 'Use target=' src/daemon/background/run.rs` → **0** (now **2** — both sites)
- [ ] `grep -c 'tmux window on the daemon host' assets/prompts/sre.toml` → **0** (now 1) — the `wait=false` mode no longer claims a tmux window unconditionally. (Match without the `de-bg-*` prefix: it is backticked in the source and a literal-asterisk pattern does not match it.)
- [ ] `grep -c "user's pane, synchronous" assets/prompts/sre.toml` → **0** (now 1)
- [ ] `grep -c 'Injects via `send-keys`' assets/prompts/sre.toml` → **0** (now 1)
- [ ] `grep -c 'backend = "pty"' assets/prompts/sre.toml` → **1** (now 0) — the prose names the default it describes
- [ ] `grep -c 'do not poll for it' assets/prompts/sre.toml` → **1** (now 0) — the non-blocking prose agrees with the result text the path actually returns
- [ ] `grep -c 'wait: false' assets/prompts/ghost-shell.txt` → **1** (now 0)
- [ ] `grep -c 'background mode' assets/prompts/ghost-shell.txt` → **0** (now 1)
- [ ] `grep -c 'fn prompt_assets_name_no_retired_tools' tests/doc_truth.rs` → **1** (now 0)
- [ ] `grep -c 'fn e2e_prompt_asset_tool_hygiene' tests/doc_truth.rs` → **1** (now 0)
- [ ] `grep -c 'FOREGROUND TARGET' assets/prompts/sre.toml` → **1** (now 2) — the accurate context-block line at the top survives; the second occurrence lives inside the false `wait=true` bullet Task 1 removes
- [ ] `cargo test --test doc_truth` reports **12 passed** (now 10)
- [ ] Task 5's `PASTE MATCH` block reads exactly `tool=watch_shell retired_in_prompts=0 headings_live=11 phantom_target_param=0`
- [ ] Task 6's mutation pair, all four `test result:` lines, in the Update Log
- [ ] `cargo fmt --all` clean, `cargo build` clean, `cargo clippy --all-targets --all-features -- -D warnings` clean

**Must-survive criteria** — these pass now and must still pass after. The first
three are the accurate pane prose this phase must **not** sweep up.

- [ ] `grep -c 'PANE MAP' assets/prompts/sre.toml` → **2, unchanged** — the block is still emitted; M23 owns it. Both occurrences are in the `### Pane targeting` section this phase does not touch
- [ ] `grep -c 'target_pane' assets/prompts/sre.toml` → **4, unchanged** — `read_file` / `edit_file` keep the parameter until M21
- [ ] `grep -c 'de-gs-bg-\*' assets/prompts/ghost-shell.txt` → **1, unchanged** — ghosts really do run there
- [ ] `grep -c '^        name: "' src/ai/tools/defs.rs` → **42, unchanged**; `grep -c 'deferred_group: None' src/ai/tools/defs.rs` → **28, unchanged** — this phase adds and removes no tool
- [ ] `cargo test --lib` reports **1885 passed, 4 ignored, unchanged** — the only new tests are integration-level, in `doc_truth.rs`
- [ ] `git diff --stat 35fe59cc..HEAD -- src/ai/ src/ipc.rs src/cli/` is **empty** — no tool definition, wire type or renderer changes here
- [ ] `grep -c 'Background Task Completed' src/daemon/background/run.rs` → **3, unchanged** — two doc comments and the one result string. Task 3 removes the `Use target=` sentence, never the completion message

## Test plan

**Added (2), both in `tests/doc_truth.rs`:**
`prompt_assets_name_no_retired_tools` (Task 4) and
`e2e_prompt_asset_tool_hygiene` (Task 5). `doc_truth` goes 10 → **12**.

**`cargo test --lib` does not move** — it stays at 1885. If it changes, you have
added a unit test this phase did not ask for, or removed one; say so in the
Update Log either way.

**Expected removals: none.**

## End-to-end verification

The Task 5 `PASTE MATCH` block **as the entry's first fenced block**, then the
Task 6 mutation pairs, in an Update Log entry titled
`### Update — <date> (end-to-end verification)`. Per `STANDARDS.md` §1 the
server-authored `(complete)` entry does **not** satisfy this.

## Authorizations

Edits are authorized in exactly these files:

- `assets/prompts/sre.toml` — the two-mode section and the Interactive-commands
  paragraph **only**
- `assets/prompts/ghost-shell.txt` — the header line and rule 7 **only**
- `src/daemon/background/run.rs` — the one result string **only**
- `tests/doc_truth.rs` — the two new tests **only**
- This phase doc's Update Log.

**Not authorized:** `src/ai/`, `src/ipc.rs`, `src/cli/`, `src/config/seeds.rs`,
`CLAUDE.md`, `README.md`, `docs/REQUIREMENTS.md`, and everything under
`assets/` except the two prompt files.

**If the lint gate reports an orphan this closure missed:** do **not** mask it
with `#[allow]`, and do **not** attribute a decision to anyone. Name it in the
Update Log, then — if it lives in an **authorized** file, delete it and record
that you did so; if it lives in an **unauthorized** file, either stop with a
blocker entry, **or** make the deletion and label it plainly as **your own**
unauthorized-but-forced decision with the evidence. **State which you chose and
that it was your decision.** There is no human in the loop mid-run.

## Out of scope

- **The seeded memories and agent configs** — retiring
  `assets/memory/knowledge/tmux-pane-toolkit.md` and
  `assets/memory/session/pane-referencing-convention.md`, cleaning the four
  other memories that mention panes, and the one agent config. **Phase 10b**,
  together with their `src/config/seeds.rs` constants and seeding calls.
- **`docs/REQUIREMENTS.md`** — 3 `get_terminal_context` references and a
  pane-heavy FR-1.2.2. **Phase 10b.**
- **The `[PANE MAP]` / `[FOREGROUND TARGET]` context blocks themselves** —
  M23. This phase documents them accurately; it does not remove them.
- **`target_pane` on `read_file` / `edit_file`** — M21's `host?`.
- **Any other model-facing result string in `src/`.** Task 3 fixes the one
  measured false parameter reference. A sweep of every `format!` that talks to
  the model is a separate, larger piece of work; if you notice another, name it
  in the Update Log rather than editing it.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-09-18 18:03 (started)

**Executor:** the dispatched executor session (headless run).

Status flipped `todo → in-progress` here and in the M22 README phase table.
Pre-flight against this phase doc's own table, re-measured at HEAD `f7c00ac0`
(the only commit since `35fe59cc` is the drafting docs commit):

- `git rev-parse --short HEAD` → `f7c00ac0`
- `grep -c 'watch_pane' assets/prompts/ghost-shell.txt` → **1**
- `grep -c 'watch_pane' src/ai/tools/defs.rs` → **0** (tool gone)
- `grep -c 'var/log/panes' assets/prompts/sre.toml` → **2**
- `cargo test --lib | tail -3` → `1885 passed; 0 failed; 4 ignored`
- `cargo test --test doc_truth | tail -3` → `10 passed; 0 failed`

Every row matches the pre-flight table (the HEAD sha differs only because the
phase doc was committed after it was measured). Baselines established; starting
Task 1.

### Update — 2026-09-18 (end-to-end verification)

**Task 5 PASTE MATCH capture (redirected from the command, pasted verbatim):**

```
PASTE MATCH >>>
tool=watch_shell retired_in_prompts=0 headings_live=11 phantom_target_param=0
<<<
```

Self-check output (`PASTE MATCH` line pasted verbatim):

```
PASTE MATCH
```

**Task 6 mutation pair (both `test result:` lines pasted verbatim, after the capture block):**

Mutation A — `watch_pane` restored in `ghost-shell.txt` rule 7:

```
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 11 filtered out; finished in 0.00s
```

Panic message named the asset and token:
`assets/prompts/ghost-shell.txt names retired tool \`watch_pane\` — it was removed
from \`TOOLS\` …`. Revert → green: `test result: ok. 1 passed; 0 failed`.

Mutation B — `Use target=\\"{pane_id}\\" …` restored in `run.rs`'s dispatch string:

```
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 11 filtered out; finished in 0.00s
```

Panic: `run.rs still tells the model to pass \`target=\`, which no tool accepts — left: 1`.
Revert → green: `test result: ok. 1 passed; 0 failed`. Both mutations red, both
reverts green — the gate is pinning the property.

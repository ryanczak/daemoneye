# Phase 01: Transcript Model

**Milestone:** M17 — Transcript View
**Status:** todo
**Depends on:** none
**Estimated diff:** ~450 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Give the chat client a client-side record of everything it renders — user turns,
assistant prose, tool panels, and the **full untruncated** tool output — and give
`Response::ToolResult` a `tool_call_id` so a rendered block can be joined to its
history record. No UI: this phase ships the model the viewer (phase-02) renders.

## Architecture references

Read before starting:

- `docs/design/transcript-view.md` — the design of record; §"Where the bytes come
  from" is why the transcript is captured from the wire and not read back from
  `events.jsonl`, the session JSONL, or `var/log/panes/`.
- `docs/dev/milestones/M17-transcript-view/README.md` — the milestone's exit
  criteria and phase ordering.
- `CLAUDE.md` § "Request/Response lifecycle" — the IPC turn flow this phase's
  wire change sits in.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

**The full output already reaches the client and is thrown away.**
`src/cli/commands/stream.rs:677-695`:

```rust
Response::ToolResult(output) => {
    let lines: Vec<String> = output.lines().map(|l| l.to_string()).collect();
    let total = lines.len();
    const MAX_LINES: usize = 10;
    let shown = if total > MAX_LINES {
        MAX_LINES - 1
    } else {
        total
    };
    let mut body: Vec<String> = lines[..shown].to_vec();
    if total > MAX_LINES {
        body.push(format!("… {} more lines", total - shown));
    } else if body.is_empty() {
        body.push("(no output)".to_string());
    }
    let _ = renderer.commit_panel("output", &body, true);
}
```

`output` holds every captured line. After `commit_panel` the `String` drops.

**The wire variant carries no identifier.** `src/ipc.rs:414`:

```rust
    /// The output captured after an approved tool call completes.
    /// Sent to the client so it can display a dimmed result block.
    ToolResult(String),
```

`grep -rn "Response::ToolResult(" --include=*.rs src tests` finds **17**
tuple-form sites (the variant definition at `src/ipc.rs:414` does not match the
pattern, since it is written `ToolResult(String),`). All 17 must change:

| Where | Lines | Kind |
|---|---|---|
| `src/daemon/executor/foreground.rs` | 181, 218, 330, 637, 933, 1015, 1066, 1137, 1173, 1225 | 10 sends |
| `src/daemon/executor/mod.rs` | 195, 217 | 2 sends |
| `src/cli/commands/stream.rs` | 677 | the render arm above |
| `src/cli/commands/ask.rs` | 207 | a skip arm inside a `|`-chain of ignored responses |
| `src/ipc.rs` | 670 | the `response_name` arm |
| `src/ipc_tests.rs` | 338, 340 | the existing `response_tool_result_roundtrip` test |

Note the two easy-to-miss ones: `ask.rs:207` silently discards `ToolResult` as
part of a `|`-chain, and `src/ipc_tests.rs` (in `src/`, **not** `tests/`)
already round-trips the variant:

```rust
#[test]
fn response_tool_result_roundtrip() {
    let resp = Response::ToolResult("output here".to_string());
    match roundtrip_resp(&resp) {
        Response::ToolResult(s) => assert_eq!(s, "output here"),
        _ => panic!("wrong variant"),
    }
}
```

**Every send site already has the call id in scope**, so no new plumbing is
needed:

- In `run_foreground` the id is destructured at `foreground.rs:137` from
  `FgArgs` (`foreground.rs:18-22`):

  ```rust
  pub(super) struct FgArgs<'a> {
      pub id: &'a str,
      pub cmd: &'a str,
      pub target: Option<&'a str>,
  }
  ```

  ```rust
  let FgArgs { id, cmd, target } = args;
  ```

- `run_background` takes `id: &str` as its first parameter
  (`foreground.rs:979-981`).
- In `execute_tool_call` (`src/daemon/executor/mod.rs`) the pending call is in
  scope as `call`, and `call.id()` returns the id (used already at
  `executor/mod.rs:203`, `call.tool_name()` alongside it).

The send helper is `send_response_split` (`src/daemon/utils/response.rs:12`); it
takes an already-built `Response` and serialises it. It does not need to change.

**The client's per-turn entry point** is `ask_with_session_ratatui`
(`src/cli/commands/stream.rs:109`), whose ratatui context struct is
(`stream.rs:93-101`):

```rust
pub(super) struct RatatuiQueryCtx<'a> {
    pub(super) chat_width: Option<usize>,
    pub(super) session_cost: &'a mut f64,
    pub(super) session_has_untracked: &'a mut bool,
    pub(super) renderer: &'a mut crate::cli::render_ratatui::RatatuiRendererStdout,
    pub(super) model: &'a str,
    pub(super) stdin: &'a AsyncStdin,
}
```

It has exactly **three** call sites: `src/cli/commands/chat.rs:338`,
`src/cli/commands/chat.rs:536`, and `src/cli/commands/ask.rs:37`.

**The chat loop** `run_chat_ratatui` (`src/cli/commands/chat.rs:252`) owns the
per-session mutable state (`prompt_tokens`, `turn`, `cost_usd`, …) and is where
a transcript naturally lives. It echoes the user's own turn at `chat.rs:529`:

```rust
let _ = renderer.commit_panel_labeled(&user_host, &echo_body, false, Some(&label));
```

with `echo_body` produced by `echo_body(query)` (`chat.rs:881`).

**Assistant prose** arrives as `Response::Token(t)` (`stream.rs:364`) and is fed
through the markdown renderer; the raw token text is the lossless form to
record. `Response::SystemMsg(msg)` is at `stream.rs:389`. Tool panels are
committed from the `ToolStarted`/`ToolFinished` pair at `stream.rs:652-673`.

**Module registration** — `src/cli/mod.rs` is the full list of `cli`
submodules (`pub mod commands; pub(crate) mod diff; pub mod input; …`).

Edition 2024. The lint gate is
`cargo clippy --all-targets --all-features -- -D warnings`.

## Spec

### Task 1 — Create the transcript model

Create `src/cli/transcript.rs` with a `Block` enum and a bounded `Transcript`
store. Write it with **exactly** these item names and signatures — later phases
and the mutation pair in tasks 10–11 depend on them:

```rust
//! Client-side record of everything the chat client renders.
//!
//! The inline renderer commits panels into terminal scrollback, where they are
//! frozen; this store keeps the same content in a form the alt-screen
//! transcript viewer can re-render, expand and search. See
//! `docs/design/transcript-view.md`.

/// One rendered unit of the conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// The user's own turn, as echoed into the transcript.
    UserTurn { label: String, text: String },
    /// Assistant prose for one turn, accumulated from `Response::Token`.
    Assistant { text: String },
    /// A tool panel header (the `▸ summary` line and its runtime label).
    ToolPanel {
        tool: String,
        summary: String,
        label: Option<String>,
    },
    /// Captured tool output, in full.
    Output {
        tool_call_id: String,
        /// The untruncated wire payload.
        full: String,
        /// How many lines the inline renderer displayed.
        shown: usize,
    },
    /// A daemon system message (`⚙ …`).
    System { text: String },
}

impl Block {
    /// Byte length of the block's own text, for the store's byte budget.
    pub fn byte_len(&self) -> usize { /* sum of the String fields' len() */ }
}

/// Default cap on retained blocks.
pub const MAX_BLOCKS: usize = 500;
/// Default cap on retained block bytes.
pub const MAX_BYTES: usize = 8 * 1024 * 1024;

/// A bounded, ordered record of the session's rendered blocks.
#[derive(Debug, Default)]
pub struct Transcript {
    blocks: Vec<Block>,
    max_blocks: usize,
    max_bytes: usize,
    bytes: usize,
    /// Blocks evicted since construction, so the viewer can say so.
    evicted: usize,
}

impl Transcript {
    pub fn new() -> Self { /* MAX_BLOCKS / MAX_BYTES */ }
    pub fn with_caps(max_blocks: usize, max_bytes: usize) -> Self { … }
    pub fn push(&mut self, block: Block) { /* push, then evict */ }
    pub fn blocks(&self) -> &[Block] { &self.blocks }
    pub fn len(&self) -> usize { self.blocks.len() }
    pub fn is_empty(&self) -> bool { self.blocks.is_empty() }
    pub fn evicted(&self) -> usize { self.evicted }
    /// Append text to the trailing `Assistant` block, or start one.
    pub fn append_assistant(&mut self, text: &str) { … }
}
```

Eviction is oldest-first and runs inside `push`. Write the eviction loop
**exactly** in this shape — task 10's mutation targets the first condition
verbatim:

```rust
    fn evict(&mut self) {
        while self.blocks.len() > self.max_blocks || self.bytes > self.max_bytes {
            if self.blocks.is_empty() {
                break;
            }
            let removed = self.blocks.remove(0);
            self.bytes = self.bytes.saturating_sub(removed.byte_len());
            self.evicted += 1;
        }
    }
```

`append_assistant` must coalesce: consecutive token text lands in **one**
`Assistant` block until some other block is pushed. Keep `self.bytes` correct
when it appends to an existing block.

### Task 2 — Register the module

In `src/cli/mod.rs`, add `pub mod transcript;` to the module list, keeping the
existing alphabetical-ish ordering (after `pub mod status;`). Do **not** add a
`pub use transcript::*;` glob — the existing globs re-export command helpers,
and a glob here would collide with `crate::ai::ToolResult` naming in callers.

### Task 3 — Add `tool_call_id` to the wire

In `src/ipc.rs`, change the variant at line 414 from a tuple to a struct
variant, keeping the doc comment:

```rust
    /// The output captured after an approved tool call completes.
    /// Sent to the client so it can display a dimmed result block.
    /// `tool_call_id` joins this output to the AI tool call that produced it,
    /// and to the matching `tool_results` record in the session JSONL.
    ToolResult {
        tool_call_id: String,
        output: String,
    },
```

Update the name arm at `src/ipc.rs:670` to `Response::ToolResult { .. } =>
"ToolResult"`.

### Task 4 — Thread the id through the foreground/background senders

In `src/daemon/executor/foreground.rs`, update all **10** `Response::ToolResult`
sends (lines 181, 218, 330, 637, 933, 1015, 1066, 1137, 1173, 1225 in the
current tree) to the struct form. `id` is already in scope in both functions —
`run_foreground` destructures it from `FgArgs` at line 137, `run_background`
takes it as its first parameter. Worked example, from line 181:

```rust
// before
send_response_split(tx, Response::ToolResult(msg.clone())).await?;
// after
send_response_split(
    tx,
    Response::ToolResult {
        tool_call_id: id.to_string(),
        output: msg.clone(),
    },
)
.await?;
```

Do not change what is sent, when, or the `ToolCallOutcome` returned alongside it.

### Task 5 — Thread the id through the dispatcher senders

In `src/daemon/executor/mod.rs`, update the 2 sends (lines 195 and 217) the same
way, using `call.id().to_string()` for `tool_call_id` — `call` is the
`&PendingCall` parameter of `execute_tool_call` and `call.id()` is already used
nearby.

### Task 6 — Record blocks in the client stream loop

In `src/cli/commands/stream.rs`:

- Add `pub(super) transcript: &'a mut crate::cli::transcript::Transcript,` to
  `RatatuiQueryCtx` (line 93) and destructure it in `ask_with_session_ratatui`
  alongside the other fields.
- Rewrite the `Response::ToolResult` arm to destructure the struct variant. The
  **rendering must stay byte-identical** — same 10-line cap, same `… N more
  lines` string, same `(no output)` fallback, same `commit_panel("output", …)`
  call. Only add the recording:

  ```rust
  Response::ToolResult { tool_call_id, output } => {
      // … existing body unchanged, operating on `output` …
      let _ = renderer.commit_panel("output", &body, true);
      transcript.push(crate::cli::transcript::Block::Output {
          tool_call_id,
          full: output,
          shown,
      });
  }
  ```

- In the `Response::Token(t)` arm (line 364), call
  `transcript.append_assistant(&t)` **before** feeding the markdown renderer.
- In the `Response::SystemMsg(msg)` arm (line 389), push
  `Block::System { text: msg.clone() }`.
- In the `Response::ToolFinished` arm (line 660), push `Block::ToolPanel` with
  the title and body accumulated from `ToolStarted`, and the runtime label.

### Task 7 — Own the transcript in the chat loop

In `src/cli/commands/chat.rs`:

- In `run_chat_ratatui` (line 252), create `let mut transcript =
  crate::cli::transcript::Transcript::new();` alongside the other per-session
  mutable state.
- At the user echo site (line 529), push
  `Block::UserTurn { label: user_host.clone(), text: query.clone() }` — the
  text is the user's query as typed, not the wrapped `echo_body` lines.
- Pass `transcript: &mut transcript` in the `RatatuiQueryCtx` literals at both
  call sites (lines 338 and 536).

### Task 8 — Update the two remaining client sites in `ask.rs`

Two changes in `src/cli/commands/ask.rs`:

- At line 37, pass a local
  `let mut transcript = crate::cli::transcript::Transcript::new();` into the
  `RatatuiQueryCtx` literal. One-shot `ask` has no viewer, so the transcript is
  discarded when the call returns; that is intended, not a TODO.
- At line 207, `Response::ToolResult(_)` appears inside a `|`-chain of silently
  skipped responses. Change it to `Response::ToolResult { .. }`. Do not give it
  a body — this arm stays a skip.

### Task 8a — Update the existing wire round-trip test

In `src/ipc_tests.rs`, update `response_tool_result_roundtrip` (line 337) to the
struct form: build the value with both fields, assert both survive the
round-trip, and add a negative guard that the serialised JSON contains the
substring `tool_call_id` — that is what catches a silent revert to a tuple.
Keep the existing test name; do not add a parallel test elsewhere.

### Task 9 — Tests

Write the tests named in § Test plan, in `src/cli/transcript.rs`'s `#[cfg(test)]
module` unless the test plan says otherwise. They are pure — no `HOME`
manipulation, no `test_home_guard()`, no tmux.

### Task 10 — Mutation M1: apply

Use the `patch` tool on `src/cli/transcript.rs` to break the block cap.

- `old_str`: `        while self.blocks.len() > self.max_blocks || self.bytes > self.max_bytes {`
- `new_str`: `        while self.blocks.len() > self.max_blocks + 1000 || self.bytes > self.max_bytes {`

Then run, appending to the evidence artifact:

```sh
A=/tmp/e2e-01.txt
echo "== M1 APPLIED ==" >> "$A"
grep -c 'self.max_blocks + 1000' src/cli/transcript.rs >> "$A"
cargo test --lib cli::transcript 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -20 >> "$A"
echo "exit=${PIPESTATUS[0]}" >> "$A"
```

The filter is path-qualified (`cli::transcript`) on purpose: a bare
`transcript` filter also matches the pre-existing
`cli::render_ratatui::tests::commit_renders_transcript_line_into_buffer`, which
passes either way and muddies the verdict.

The run **must fail** — `transcript_push_evicts_oldest_over_block_cap` is what
proves the cap is real. A green run here means the test is vacuous; stop and
file a blocker rather than proceeding.

### Task 11 — Mutation M1: restore

`patch` the same line back (`old_str` and `new_str` swapped from task 10), then:

```sh
A=/tmp/e2e-01.txt
echo "== M1 RESTORED ==" >> "$A"
grep -c 'self.max_blocks + 1000' src/cli/transcript.rs >> "$A"
cargo test --lib cli::transcript 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -20 >> "$A"
echo "exit=${PIPESTATUS[0]}" >> "$A"
```

`grep -c` must print `1` after task 10 and `0` after task 11. Do **not** use
`git checkout` to restore — the file holds this round's uncommitted work.

### Task 12 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-01.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this.

### Task 13 — PASTE MATCH self-check

After pasting, run:

```sh
D=docs/dev/milestones/M17-transcript-view/phase-01-transcript-model.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-01.txt
diff /tmp/pasted-01.txt /tmp/e2e-01.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

Append the literal verdict line (`PASTE MATCH` or `PASTE MISMATCH`) **into that
same Update Log entry**, below the fence.

## Acceptance criteria

- [ ] `cargo fmt --all` leaves the tree unchanged.
- [ ] `cargo build` succeeds.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` passes with no ignored-test count change beyond the existing 1.
- [ ] `grep -rn "Response::ToolResult(" --include=*.rs src tests` prints nothing
      and exits 1 — no tuple-form site survives.
- [ ] `grep -c "tool_call_id" src/ipc.rs` prints at least 1.
- [ ] Tests `transcript_push_evicts_oldest_over_block_cap`,
      `transcript_push_evicts_over_byte_cap`,
      `transcript_append_assistant_coalesces`,
      `transcript_append_assistant_breaks_on_other_block`,
      `transcript_records_full_output_not_truncated`, and
      `response_tool_result_roundtrip` all pass.
- [ ] `/tmp/e2e-01.txt` shows `== M1 APPLIED ==` with a **failing** test run and
      `grep -c` = 1, then `== M1 RESTORED ==` with a passing run and
      `grep -c` = 0.
- [ ] The Update Log's newest entry is headed
      `### Update — <date> (end-to-end verification)`, contains the pasted
      artifact, and ends with the literal line `PASTE MATCH`.

## Test plan

In `src/cli/transcript.rs` (`#[cfg(test)] mod tests`):

- `transcript_push_evicts_oldest_over_block_cap` — build with
  `Transcript::with_caps(3, usize::MAX)`, push 5 distinguishable
  `Block::System` blocks, assert `len() == 3`, `evicted() == 2`, and that
  `blocks()[0]` is the **third** pushed block (oldest-first eviction, not
  newest).
- `transcript_push_evicts_over_byte_cap` — `with_caps(usize::MAX, 100)`, push
  three ~60-byte blocks, assert the byte budget forced at least one eviction and
  the survivors are the most recent.
- `transcript_append_assistant_coalesces` — three `append_assistant` calls with
  no intervening push produce **one** `Block::Assistant` whose text is the
  concatenation.
- `transcript_append_assistant_breaks_on_other_block` — `append_assistant("a")`,
  `push(System)`, `append_assistant("b")` produces two distinct `Assistant`
  blocks, the second holding only `"b"`.
- `transcript_records_full_output_not_truncated` — push a `Block::Output` whose
  `full` has 500 lines with `shown: 9`; assert `full.lines().count() == 500` and
  that the stored string equals the input. This is the phase's central
  behaviour: the store keeps what the renderer elided.

In `src/ipc_tests.rs`:

- `response_tool_result_roundtrip` (existing, line 337) — rewritten for the
  struct form: round-trip
  `Response::ToolResult { tool_call_id: "toolu_abc".into(), output: "line1\nline2".into() }`,
  assert **both** fields survive, and assert the serialised JSON contains the
  substring `tool_call_id`.

## End-to-end verification

The phase's runtime-loadable artifact is the IPC wire format: the daemon and
client are the same binary, and the serde round-trip in `src/ipc_tests.rs` is
the project's protocol-layer door. The block below runs the real gates, the
protocol test, and the negative grep that proves no tuple-form site survives.

Tasks 10 and 11 append the mutation pair to the **same** artifact before this
block runs; do not truncate `/tmp/e2e-01.txt` here.

```sh
A=/tmp/e2e-01.txt
echo "== GATES ==" >> "$A"
cargo fmt --all -- --check 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -5 >> "$A"
echo "fmt exit=${PIPESTATUS[0]}" >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -5 >> "$A"
echo "clippy exit=${PIPESTATUS[0]}" >> "$A"
cargo test 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -25 >> "$A"
echo "test exit=${PIPESTATUS[0]}" >> "$A"
echo "== PROTOCOL ==" >> "$A"
cargo test --lib response_tool_result_roundtrip 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -10 >> "$A"
echo "protocol exit=${PIPESTATUS[0]}" >> "$A"
echo "== TRANSCRIPT UNITS ==" >> "$A"
cargo test --lib cli::transcript 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -15 >> "$A"
echo "units exit=${PIPESTATUS[0]}" >> "$A"
echo "== NO TUPLE-FORM SITES ==" >> "$A"
grep -rn "Response::ToolResult(" --include=*.rs src tests >> "$A"
echo "grep exit=$?  (1 = none found, which is the pass)" >> "$A"
echo "== WIRE FIELD ==" >> "$A"
grep -n "tool_call_id" src/ipc.rs >> "$A"
echo "wire exit=$?" >> "$A"
```

## Authorizations

- [ ] May touch `src/ipc.rs` (the `Response::ToolResult` variant and its name
      arm only).
- [ ] May add the file `src/cli/transcript.rs` and register it in
      `src/cli/mod.rs`.
- [ ] May edit `src/ipc_tests.rs` (the `response_tool_result_roundtrip` test
      only).

No new dependencies. `docs/architecture.md` is **not** authorized.

## Out of scope

- **Any UI.** No `Key::CtrlO`, no alternate screen, no viewer, no change to what
  the inline renderer draws. The `… N more lines` footer text stays exactly as
  it is — the `· ctrl+o` hint belongs to phase-03.
- **`src/cli/render_ratatui.rs`.** This phase does not touch the renderer.
- **Reading persisted logs.** No session-JSONL parsing (phase-06), and never any
  read of `var/log/panes/*.log` — that archive is unmasked.
- **Search, copy, mouse.** Phases 04, 05 and 07.
- **Changing daemon-side truncation.** `truncate_tool_results` and
  `limits.tool_result_chars` stay as they are; this phase changes only what the
  *client* keeps.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

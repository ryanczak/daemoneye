# Phase 06: Rehydration

**Milestone:** M17 — Transcript View
**Status:** todo
**Depends on:** phase-01 (transcript-model, `done`) — and in practice phases
02–05, since the viewer is what makes a rehydrated transcript visible.
**Estimated diff:** ~400 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Make `/session load <name>` refill the client's transcript from the named
session's stored messages, so `ctrl+o` after a load shows the conversation that
happened before this client existed — the one capability the inline-only design
could never have.

## Architecture references

Read before starting:

- `docs/design/transcript-view.md` — §"Where the bytes come from" is why the
  session store is the **rehydration** source and the pane-log archive is not.
- `CLAUDE.md` § "Named session conventions" — the storage layout this reads.
- `src/cli/transcript.rs` — the `Block` model being filled.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

**There is no "resume this session id" path in `daemoneye chat`.**
`run_chat_inner(session_override)` (`src/cli/commands/chat.rs:32`) takes a
**tmux session name**, not a daemoneye session id — it is the managed-session
auto-attach. Every `daemoneye chat` mints a fresh id via `new_session_id()`
(`chat.rs:89`). So the rehydration trigger is **`/session load <name>`**, not
process start. Do not add a CLI flag for it.

**The load path is `src/cli/commands/slash.rs:535-556`**, and it already has
everything except the transcript:

```rust
                Ok(Response::SessionLoaded {
                    name,
                    message_count,
                    banner,
                    ..
                }) => {
                    note(r, &format!("✓ loaded '{name}' ({message_count} messages)"));
                    if !banner.is_empty() {
                        let _ = r.commit(&format!("{banner}\n"));
                    }
                }
```

Its context struct is `SlashCtx` (`slash.rs:27-35`), which today carries
`renderer`, `session_id`, `approval`, `model`, `context_window`,
`current_prompt`, `target_pane` — **no transcript**. Task 3 adds one.

**The stored messages are readable client-side.**
`crate::session_store::load_session_messages(name, max_count)`
(`src/session_store.rs:273`) returns `Vec<Message>` from
`~/.daemoneye/var/sessions/<name>/messages.jsonl`. It returns an empty `Vec`
when the file does not exist — an absent file is **not** an error.

**The record shapes** (`src/ai/types/wire.rs`):

```rust
pub struct Message {
    pub role: String,
    pub content: String,
    pub tool_calls: Option<Vec<ToolCall>>,
    pub tool_results: Option<Vec<ToolResult>>,
    pub turn: Option<usize>,
}

pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
    // thought_signature is #[serde(skip)]
}

pub struct ToolResult {
    pub tool_call_id: String,
    pub tool_name: String,
    pub content: String,
}
```

**Daemon-side truncation is visible in the stored text.** `src/daemon/stream.rs:1300`
writes results over the cap as:

```rust
                        "{}\n[truncated — {} chars total; full output archived in pane log]",
```

So a rehydrated `Output` block may legitimately be shorter than the original.
That marker is **kept verbatim** — it is the honest record of what the store
has, and task 2 pins it.

### Three gotchas, each verified against the tree

1. **`Message` is not one-block-per-record.** One assistant record can carry
   `content` *and* `tool_calls`; one user record carries `tool_results` with
   `content` empty. The mapping is one-to-many and one-to-none, and task 1 pins
   it per case. Do not assume a 1:1 correspondence.
2. **Rehydration must not double-count the live session.** `/session load`
   replaces the conversation, so the transcript is **cleared** before refilling.
   Appending would interleave the loaded history with whatever the current
   client had on screen.
3. **Never read `var/log/panes/*.log`.** The truncation marker names a pane log;
   that archive is written **unmasked** (`src/daemon/background/helpers.rs`).
   The rehydrated block keeps the marker text and stops there. A criterion
   greps for this.

## Spec

### Task 1 — The pure conversion

In `src/cli/transcript.rs`, add:

```rust
/// Rebuild transcript blocks from stored session messages.
pub fn blocks_from_messages(messages: &[Message]) -> Vec<Block>
```

Import it as **`crate::ai::Message`** — that is the re-export the rest of the
tree uses (`src/session_store.rs:6`, `src/daemon/session.rs:5`), verified.

Mapping, pinned per case, in record order:

- `role == "user"`, `content` non-empty → `Block::UserTurn { label: "you".into(), text: content.clone() }`.
- `role == "assistant"`, `content` non-empty → `Block::Assistant { text: content.clone() }`.
- Any record with `tool_calls: Some(calls)` → one
  `Block::ToolPanel { tool: call.name.clone(), summary: call.arguments.clone(), label: None }`
  per call, **after** that record's own content block if it had one.
- Any record with `tool_results: Some(results)` → one
  `Block::Output { tool_call_id: r.tool_call_id.clone(), full: r.content.clone(), shown: 0 }`
  per result. `shown: 0` is correct and deliberate: nothing of this output was
  ever displayed inline by *this* client.
- A record with empty `content` and neither `tool_calls` nor `tool_results`
  contributes **nothing** — no empty block.

### Task 2 — Keep the truncation marker verbatim

`blocks_from_messages` copies `ToolResult.content` **unchanged**. When the
daemon truncated the result, the stored text ends with
`[truncated — {n} chars total; full output archived in pane log]`, and that
line must survive into `Block::Output.full` byte-for-byte.

Do **not** strip it, rewrite it, or attempt to recover the full output from the
pane log. The test named in § Test plan asserts the marker survives.

### Task 3 — Thread the transcript into the slash context

- Add `pub(super) transcript: &'a mut crate::cli::transcript::Transcript,` to
  `SlashCtx` (`src/cli/commands/slash.rs:27`).
- Pass it at the call site in `src/cli/commands/chat.rs` where `SlashCtx` is
  constructed. `run_chat_ratatui` owns `transcript` (`chat.rs:271`).
- Threading it through does not change any existing slash command's behaviour.

### Task 4 — Rehydrate on a successful load

In the `Response::SessionLoaded` arm (`slash.rs:543`), after the existing
`note(...)` and banner lines and **only on success**:

1. `crate::session_store::load_session_messages(&name, usize::MAX)`.
2. On `Ok(msgs)`: **clear** the transcript (see gotcha 2 — add a
   `Transcript::clear()` that resets `blocks`, `bytes` and `evicted`), then push
   every block from `blocks_from_messages(&msgs)`, then push one
   `Block::System { text: format!("rehydrated {} blocks from session '{}'", n, name) }`
   where `n` is the number of blocks pushed **before** the system block.
3. On `Err(e)`: leave the transcript untouched and `note(r, &format!("✗ transcript rehydrate failed: {e}"))`.
   The error is surfaced, never swallowed — no `let _ =`.

An absent messages file yields `Ok(vec![])`, which clears the transcript and
pushes only the system block saying `rehydrated 0 blocks`. That is correct: the
loaded session genuinely has no stored history.

### Task 5 — Tests

Write the tests named in § Test plan. All are pure — no daemon, no `HOME`
manipulation, no tmux. Construct `Message` values directly.

### Task 6 — Mutation M1: apply

Use the `patch` tool on `src/cli/transcript.rs`.

- `old_str`: `                shown: 0,`
- `new_str`: `                shown: usize::MAX,`

(the `shown` field inside the `Block::Output` construction in
`blocks_from_messages`).

Then run, appending to the evidence artifact:

```sh
A=/tmp/e2e-06.txt
echo "== M1 APPLIED ==" >> "$A"
grep -c 'shown: usize::MAX,' src/cli/transcript.rs >> "$A"
cargo test --lib cli::transcript 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -20 >> "$A"
echo "exit=${PIPESTATUS[0]}" >> "$A"
```

The run **must fail** — `rehydrated_output_reports_nothing_shown_inline` is what
proves the field is pinned. A green run means the test is vacuous; stop and
file a blocker.

### Task 7 — Mutation M1: restore

`patch` the line back, then:

```sh
A=/tmp/e2e-06.txt
echo "== M1 RESTORED ==" >> "$A"
grep -c 'shown: usize::MAX,' src/cli/transcript.rs >> "$A"
cargo test --lib cli::transcript 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -20 >> "$A"
echo "exit=${PIPESTATUS[0]}" >> "$A"
```

`grep -c` must print `1` after task 6 and `0` after task 7. Do **not** use
`git checkout` to restore — the file holds this round's uncommitted work.

### Task 8 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-06.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this.

### Task 9 — PASTE MATCH self-check

After pasting, run:

```sh
D=docs/dev/milestones/M17-transcript-view/phase-06-rehydration.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-06.txt
diff /tmp/pasted-06.txt /tmp/e2e-06.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

Append the literal verdict line into that same Update Log entry, below the
fence.

## Acceptance criteria

Every criterion asserts an observed value or count.

- [ ] `cargo fmt --all` leaves the tree unchanged.
- [ ] `cargo build` succeeds.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` passes.
- [ ] Test `blocks_from_messages_maps_each_record_kind` passes — over a fixture
      of 5 records (user text, assistant text, assistant with 2 `tool_calls`,
      user with 2 `tool_results`, and an empty record) it yields **exactly 7**
      blocks in the pinned order, and the empty record contributes **0**.
- [ ] Test `blocks_from_messages_keeps_truncation_marker` passes — a
      `ToolResult.content` ending in
      `[truncated — 51234 chars total; full output archived in pane log]`
      produces a `Block::Output` whose `full` **ends with that exact string**.
- [ ] Test `rehydrated_output_reports_nothing_shown_inline` passes — every
      `Block::Output` from `blocks_from_messages` has `shown == 0` (assert the
      value, not merely that the block exists).
- [ ] Test `blocks_from_messages_empty_input_is_empty` passes — asserts length
      **exactly 0**, no system block, no placeholder.
- [ ] Test `transcript_clear_resets_counters` passes — after `clear()`,
      `len() == 0`, `is_empty()`, and `evicted() == 0`.
- [ ] `grep -rn "pane_logs_dir\|var/log/panes" src/cli/` prints nothing and
      exits 1 — the client never reads the unmasked archive.
- [ ] `grep -c "let _ = crate::session_store::load_session_messages" src/cli/commands/slash.rs`
      prints `0` — the load result is surfaced, never discarded.
- [ ] `/tmp/e2e-06.txt` shows `== M1 APPLIED ==` with a **failing** run and
      `grep -c` = 1, then `== M1 RESTORED ==` with a passing run and
      `grep -c` = 0.
- [ ] The Update Log's newest entry is headed
      `### Update — <date> (end-to-end verification)`, contains the pasted
      artifact, and ends with the literal line `PASTE MATCH`.

## Test plan

In `src/cli/transcript.rs` (`#[cfg(test)] mod tests`):

- `blocks_from_messages_maps_each_record_kind` — the 5-record fixture above;
  assert the exact count (7), and assert the block sequence's variants in order.
- `blocks_from_messages_keeps_truncation_marker` — exact `ends_with` assertion.
- `rehydrated_output_reports_nothing_shown_inline` — every `Output` has
  `shown == 0`.
- `blocks_from_messages_empty_input_is_empty` — exactly 0 blocks.
- `transcript_clear_resets_counters` — push blocks, force an eviction with
  `with_caps(1, usize::MAX)`, `clear()`, then assert `len() == 0` **and**
  `evicted() == 0` (the counter resets too, so a later "N older blocks evicted"
  note is not inherited from the previous session).

The `/session load` wiring itself is not unit-tested — it requires a running
daemon. The live check is architect-run at milestone close.

## End-to-end verification

The rehydration path needs a daemon and a saved session, so its real check is
live at milestone close. What the executor verifies here is the pure conversion,
the clear semantics, and the two negative greps that keep the client away from
the unmasked archive and stop the load error being swallowed.

Tasks 6 and 7 append the mutation pair to the **same** artifact before this
block runs; do not truncate `/tmp/e2e-06.txt` here.

```sh
A=/tmp/e2e-06.txt
echo "== GATES ==" >> "$A"
cargo fmt --all -- --check 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -5 >> "$A"
echo "fmt exit=${PIPESTATUS[0]}" >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -5 >> "$A"
echo "clippy exit=${PIPESTATUS[0]}" >> "$A"
cargo test 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -25 >> "$A"
echo "test exit=${PIPESTATUS[0]}" >> "$A"
echo "== TRANSCRIPT UNITS ==" >> "$A"
cargo test --lib cli::transcript 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -25 >> "$A"
echo "units exit=${PIPESTATUS[0]}" >> "$A"
echo "== CLIENT NEVER READS THE UNMASKED ARCHIVE ==" >> "$A"
grep -rn "pane_logs_dir\|var/log/panes" src/cli/ >> "$A"
echo "archive grep exit=$?  (1 = none found, which is the pass)" >> "$A"
echo "== LOAD RESULT NOT DISCARDED ==" >> "$A"
grep -c "let _ = crate::session_store::load_session_messages" src/cli/commands/slash.rs >> "$A"
echo "== PHASE-02 CONTRACT STILL HOLDS ==" >> "$A"
grep -c "disarm" src/cli/viewer.rs >> "$A"
grep -nE "try_restore|disable_raw_mode|\.restore\(\)" src/cli/viewer.rs >> "$A"
echo "teardown grep exit=$?  (1 = none found, which is the pass)" >> "$A"
```

## Authorizations

- [ ] May edit `src/cli/transcript.rs`, `src/cli/commands/slash.rs`, and the
      `SlashCtx` construction site in `src/cli/commands/chat.rs`.

No new dependencies. `docs/architecture.md` is **not** authorized, and neither
is `src/cli/viewer.rs` — the viewer renders whatever the transcript holds and
needs no change for this phase.

## Out of scope

- **Mouse** (phase-07).
- **A CLI flag or process-start rehydration.** The trigger is `/session load`
  only — see § Current state for why there is no session-id resume path.
- **Reading `var/log/sessions/<id>.jsonl`** (the ephemeral per-session log) or
  `var/log/panes/*.log` (unmasked). Named-session storage via
  `session_store::load_session_messages` is the only source.
- **Recovering truncated output.** The marker stays; the pane log is not read.
- **Undoing anything phases 02–05 established.** The E2E block re-checks the
  guard contract.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

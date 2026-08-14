# Phase 01: read_pane grep="null" — stop echoing null arguments into history

**Milestone:** M15 — Chat Reliability & Dialog UX
**Status:** done
**Depends on:** none
**Estimated diff:** ~250 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

`read_pane` calls frequently arrive with the literal string `"null"` as the
`grep` argument, so the pane read returns nothing unless a line contains
"null". Root cause: `PendingCall::to_tool_call()` serializes absent optional
arguments as JSON `null` into the assistant tool-call history echoed back to
the model every turn, and models imitate that as the *string* `"null"` on
later calls. This phase stops the daemon from ever echoing `null`-valued
arguments.

## Architecture references

Read before starting:

- `src/ai/types/pending.rs` — `PendingCall` and `to_tool_call()`, the file
  this phase changes.
- `src/ai/types/wire.rs` — `ToolCall` (the `arguments` field is a JSON
  *string*, not a Value).

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

**The echo channel.** After each turn the daemon rebuilds the assistant
message's tool calls from the executed `PendingCall`s and appends them to
history:

- `src/daemon/stream.rs:931` —
  `tool_calls: Some(pending_calls.iter().map(|c| c.to_tool_call()).collect())`
- `src/daemon/ghost.rs:1007` — same shape for ghost sessions.

**The serialization.** Every arm of `to_tool_call()`
(`src/ai/types/pending.rs:257`) builds its `arguments` string with
`serde_json::json!({...}).to_string()`. `Option` fields serialize as JSON
`null`, e.g. the `ReadPane` arm (`src/ai/types/pending.rs:493`):

```rust
PendingCall::ReadPane { id, thought_signature, pane_id, lines, grep } => ToolCall {
    id: id.clone(),
    thought_signature: thought_signature.clone(),
    name: "read_pane".to_string(),
    arguments: serde_json::json!({"pane_id": pane_id, "lines": lines, "grep": grep}).to_string(),
},
```

With `grep: None` this produces `{"grep":null,"lines":null,"pane_id":"%3"}` in
the history the model sees next turn.

**Field evidence (measured 2026-08-14 by the architect against the live
daemon's session logs):**

```
$ grep -rho 'grep[^,}]*null[^,}]*' ~/.daemoneye/var/log/sessions/*.jsonl | sort | uniq -c | sort -rn | head -2
     26 grep\":null          <- the daemon's echo (JSON null)
     20 grep\":\"null\"      <- model-authored calls imitating it (string "null")
```

The string form is the bug the user sees: `knowledge::read_pane`
(`src/daemon/executor/knowledge/pane.rs`) applies `Some("null")` as a real
filter, and the ToolStarted summary renders `grep="null"`
(`src/ai/types/pending.rs:660–674`).

**The incoming direction is already correct.** `ReadPaneArgs`
(`src/ai/tools/args.rs:94–98`) deserializes `grep` as `Option<String>`, so a
JSON `null` from a provider becomes `None`. No change is needed there.

**One arm already avoids the problem** — the `Background` arm
(`src/ai/types/pending.rs:268`) inserts its optional field conditionally:

```rust
arguments: {
    let mut a = serde_json::json!({"command": cmd, "background": true});
    if let Some(rp) = retry_pane {
        a["retry_in_pane"] = serde_json::json!(rp);
    }
    a.to_string()
},
```

Rather than repeating that shape 30+ times, this phase adds one helper that
strips top-level nulls and routes every arm through it.

## Spec

### 1. Add the `args_to_string` helper — in `src/ai/types/pending.rs`

Add a private module-level function directly above `impl PendingCall`:

```rust
/// Serialize a tool-call argument object, omitting entries whose value is
/// JSON `null`. Optional parameters the model did not pass must not be
/// echoed back into conversation history: models imitate `"grep": null`
/// as the string `"null"` on later calls, which then filters pane reads
/// to lines containing "null".
fn args_to_string(mut v: serde_json::Value) -> String {
    if let serde_json::Value::Object(map) = &mut v {
        map.retain(|_, val| !val.is_null());
    }
    v.to_string()
}
```

Strip **top level only** — no argument object in this file nests optional
objects, and a recursive strip could silently alter future array/object
arguments (e.g. `load_tools` `groups`).

### 2. Route every argument-object arm of `to_tool_call()` through the helper — in `src/ai/types/pending.rs`

For **every** arm of `to_tool_call()` (`src/ai/types/pending.rs:257–520`)
whose `arguments` is built as `serde_json::json!({...}).to_string()`, change
it to `args_to_string(serde_json::json!({...}))`. Example, the `ReadPane`
arm:

```rust
arguments: args_to_string(serde_json::json!({"pane_id": pane_id, "lines": lines, "grep": grep})),
```

Apply uniformly to all such arms — including arms with no `Option` fields
(the wrap is a no-op there and keeps the rule auditable). Also convert the
`Background` arm: keep its `retry_pane` conditional insert, but end the block
with `args_to_string(a)` instead of `a.to_string()`.

Leave the four no-argument arms that use `arguments: "{}".to_string()`
(`ListSchedules`, `ListScripts`, `ListRunbooks`, `ListPanes`, `ListAgents`)
unchanged.

Do NOT change `summary()`, `id()`, `tool_name()`,
`should_emit_tool_feedback()`, or anything in `src/ai/tools/args.rs` — the
deserialization direction is already correct.

### 3. Unit tests — in the `mod tests` of `src/ai/types/pending.rs`

Follow the existing constructor-helper idiom (`mk_read_file`,
`src/ai/types/pending.rs:740`):

```rust
fn mk_read_file(path: &str) -> PendingCall {
    PendingCall::ReadFile {
        id: "x".to_string(),
        thought_signature: None,
        path: path.to_string(),
        offset: None,
        limit: None,
        pattern: None,
        target_pane: None,
    }
}
```

Write the tests named in § Test plan. The sweeping guard test
(`to_tool_call_never_serializes_null`) must construct **one instance of every
`PendingCall` variant** with every `Option` field set to `None`, call
`to_tool_call()` on each, parse `arguments` with
`serde_json::from_str::<serde_json::Value>`, and assert no top-level value
`is_null()`. Parse the JSON — do not assert on the substring "null", which
would false-positive on argument *content* containing the word.

### 4. Capture the end-to-end evidence

Run the block in § End-to-end verification verbatim and paste its output into
a new Update Log entry titled `### Update — <date> (end-to-end verification)`.

## Acceptance criteria

- [ ] `to_tool_call()` on a `ReadPane { grep: None, lines: None, .. }`
      produces arguments containing only `pane_id` — no `grep`, no `lines`
      key at all.
- [ ] `to_tool_call()` on a `ReadPane { grep: Some("null".into()), .. }`
      preserves `"grep":"null"` verbatim — a user may legitimately grep for
      the word "null"; the helper strips JSON nulls, never strings.
- [ ] No `PendingCall` variant serializes a JSON `null` argument value
      (test `to_tool_call_never_serializes_null` passes).
- [ ] Present optional values still round-trip: `FindInPanes` with
      `scope: Some("all")` includes `"scope":"all"`; `Background` with
      `retry_pane: Some(..)` includes `retry_in_pane`.
- [ ] `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
      and `cargo test` all pass.

## Test plan

All in `mod tests` of `src/ai/types/pending.rs`:

- `to_tool_call_read_pane_omits_none_options` — `ReadPane` with
  `lines: None, grep: None`: parsed arguments object has exactly one key,
  `pane_id`.
- `to_tool_call_read_pane_preserves_string_null_grep` — `ReadPane` with
  `grep: Some("null".to_string())`: parsed arguments `grep` equals the
  string `"null"`. (Negative case for the strip: strings survive.)
- `to_tool_call_read_file_omits_none_options` — `mk_read_file("/etc/hosts")`:
  parsed arguments object has exactly one key, `path`.
- `to_tool_call_preserves_present_options` — `FindInPanes` with
  `scope: Some("all")` includes `"scope":"all"`; `ReadPane` with
  `lines: Some(200)` includes `"lines":200`.
- `to_tool_call_background_retry_pane_roundtrip` — `Background` with
  `retry_pane: Some("%5")` includes `"retry_in_pane":"%5"`; with
  `retry_pane: None` has no `retry_in_pane` key.
- `to_tool_call_never_serializes_null` — the sweeping every-variant guard
  described in Spec task 3.

## End-to-end verification

```sh
cd /home/matt/src/daemoneye
cargo fmt --check 2>&1 | tail -5; echo "exit=${PIPESTATUS[0]}"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -5; echo "exit=${PIPESTATUS[0]}"
cargo test 2>&1 | tail -10; echo "exit=${PIPESTATUS[0]}"
cargo test --lib to_tool_call 2>&1 | tail -15; echo "exit=${PIPESTATUS[0]}"
```

The `cargo test --lib to_tool_call` tail must show every test named in
§ Test plan passing. Redirect to a file and paste the file's contents —
never retype or summarize (see WORKFLOW.md § "A pasted transcript is a
claim, not evidence").

Live re-verification against a real chat session (fresh session, one
`read_pane` call without grep, session-JSONL `tool_calls` showing no `grep`
key) is performed **architect-side at review** — it needs an attached tmux
client and AI spend, which are outside this phase's authorizations.

## Authorizations

- Edit `src/ai/types/pending.rs` only.
- Run the four gate commands. No daemon restart, no tmux interaction, no
  files outside the repo.

## Out of scope

- Parser-side normalization of an incoming string `"null"` grep to `None` —
  it would break legitimately grepping for "null"; the fix is upstream at
  the echo.
- Changes to `summary()` display, `src/ai/tools/args.rs`, or any provider
  backend.
- Rewriting already-persisted session histories that contain `"grep":null` —
  they age out via compaction.
- The other M15 issues (borders, sudo, dialogs).

## Update Log

### Update — 2026-08-14 (end-to-end verification)

Captured mechanically to `scratchpad/e2e-phase01.txt` and pasted verbatim
(architect takeover run):

```
exit=0
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s
exit=0
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.15s

   Doc-tests daemoneye

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

exit=0
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.06s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-b60224cb24515ede)

running 6 tests
test ai::types::pending::tests::to_tool_call_read_pane_omits_none_options ... ok
test ai::types::pending::tests::to_tool_call_read_file_omits_none_options ... ok
test ai::types::pending::tests::to_tool_call_background_retry_pane_roundtrip ... ok
test ai::types::pending::tests::to_tool_call_preserves_present_options ... ok
test ai::types::pending::tests::to_tool_call_read_pane_preserves_string_null_grep ... ok
test ai::types::pending::tests::to_tool_call_never_serializes_null ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 1241 filtered out; finished in 0.00s

exit=0
```

(The four blocks are fmt / clippy / full test / targeted `to_tool_call`
tests, each ending in its `${PIPESTATUS[0]}` marker; the full-test tail shows
the integration suite's final summary.)

### Review verdict — 2026-08-14

- **Verdict:** escalated
- **Bounces:** 2 (NoProgressStall ×2: fresh dispatch, then briefing-seeded
  resume — both stalled at 60 consecutive read-only calls with a
  non-compiling tree)
- **Executor:** Claude (direct) — takeover after
  `nvidia/NVIDIA-Nemotron-3.5-Lightning-30B-A3B-NVFP4` failed twice
- **Scope deviations:** none — helper + 32 wrapped arms + 6 tests, exactly
  per spec
- **Calibration:** Nemotron executor data point: correctly executed the
  mechanical single-line wraps (26 arms) but never added the helper function
  quoted verbatim in the spec, twice, and fell into the read-only re-read
  loop both times. Same pathology class as the Qwen verify-loop
  (memory: executor-verify-loop-pathology) but earlier in the phase — it
  stalled before ever running a gate. One occurrence of "quoted helper never
  pasted" — data, not yet a trend.
- **Live verification:** the live chat re-check (fresh session, `read_pane`
  with no grep, session-JSONL shows no `grep` key) is deferred to milestone
  close per the phase doc's E2E section — it needs an attached client and AI
  spend.

### Update — 2026-08-14 (escalation)

**Chosen lever:** resume
**Rationale:** `hard_fail` NoProgressStall (60 consecutive read-only calls)
with the mechanical wrap correctly applied to all 26 single-line arms but the
`args_to_string` helper itself never defined (tree does not build: E0425 ×26),
the multi-line arms (`Foreground`, `Background`, `ScheduleCommand`,
`UpdateMemory`, `CreateAgent`, `TmuxControl`) untouched, and no tests written.
Spec was executable — the executor stalled after patch failures on multi-line
arms; a briefing-seeded resume with the remaining work enumerated is the
cheapest unblock. First failure; takeover not yet warranted.

### Update — 2026-08-14 (created)

Phase drafted by the architect. Root cause pinned to `to_tool_call()`
null-echo with field evidence from live session logs (26× `"grep":null`
daemon-echoed, 20× `"grep":"null"` model-imitated). Status: todo.

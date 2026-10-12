# Phase 07d: live-r2 — alerts reach a reconnected chat's history; mistyped tool arguments get an error, not a silent drop

**Milestone:** M27 — tmux becomes an optional observer; 2.0 close
**Status:** in-progress
**Depends on:** phase-07a, 07b, 07c (all done)
**Tags:** language=rust, kind=fix, size=s

## Goal

Rehearsal 2 of phase 07 found two major defects that block the live close
with the PE present:

- **F8.** After a daemon restart the open chat reconnects (presence), but
  its session is not resident in the in-memory `SessionStore` until it
  sends a message. `inject_into_sessions` only reaches resident sessions,
  so an alert or ghost event in that window shows on screen but never
  lands in the session's history. The model then says no alert arrived.
- **F10.** Every `ToolArgs::from_value` is `serde_json::from_value(..).ok()`.
  When the model sends one mistyped argument to a **known** tool (for
  example `"wait": "true"`, a string), `dispatch_tool_event` returns
  `None`, and the backends log and show it as "model called unknown tool
  'run_terminal_command' — call dropped". The model gets no tool result,
  the turn ends with "The model returned an empty reply", and it cannot
  correct itself. The raw arguments are logged nowhere.

Plus one nit: the chat's reconnect notices render as `⚙ ⚙ …` (doubled gear).

## Work list — this table is the whole job

Rules (they turned 07b and 07c into clean runs):

- **Open only the files a row names.** The compiler will name any further
  exhaustive `match` you must extend; extend those and nothing else.
- **Your first `patch` lands within 10 turns.** That patch is row 1.

| # | File · symbol | Change |
|---|---|---|
| 1 | `src/daemon/session.rs` · presence registry | Add `pub fn presence_session_ids() -> Vec<String>`: the session ids that have at least one live presence connection. |
| 2 | `src/webhook/process.rs` · `inject_into_sessions` | Append `msg` to disk (`append_session_message`) for every id in the **union** of the store's keys and `presence_session_ids()`, each id once. The in-memory push stays as it is: resident entries only. Do **not** insert a store entry for a non-resident id: `handle_ask` loads history from disk only when the session is not resident, so an empty resident entry would lose it. |
| 3 | `src/ai/types/events.rs` · `AiEvent` | Add `InvalidToolArgs { id: String, name: &'static str, arguments: String, error: String, thought_signature: Option<String> }`. |
| 4 | `src/ai/tools/dispatch.rs` | Add `pub fn invalid_args_event(id: &str, name: &str, args: &Value, ts: Option<String>) -> Option<AiEvent>`. `None` when `name` is not in `TOOLS`. Otherwise `Some(InvalidToolArgs)`, where `name` is the `TOOLS` entry's `&'static str`, `arguments` is `args` serialised, and `error` names every problem it finds from the `ToolDef`'s `ParamDef`s: each missing required param, and each present param whose JSON type does not match its `ParamTy` (`Str` → string, `Bool` → boolean, `Int` → integer). If it finds none, `error` says the arguments do not match the schema. Either way, `error` ends with the expected parameter list, `name (type, required)`. |
| 5 | `src/ai/backends/openai.rs`, `anthropic.rs`, and `gemini.rs` (the drain loop, not the `has_dispatchable` predicate) | Where `dispatch_tool_event` returns `None`: first try `invalid_args_event`. If that is `Some`, log WARN `model sent invalid arguments for tool '<name>': <args>` and send the event. Only if it is `None` keep today's "unknown tool" path. |
| 6 | `src/ai/types/pending.rs` · `PendingCall` | Add `InvalidArgs { id, name: &'static str, arguments: String, error: String, thought_signature }`. `to_tool_call` gives back `name` and the raw `arguments` unchanged, so history shows the call exactly as the model made it. Fill in the `id`, `tool_name` (returns `name`) and `summary` arms. `should_emit_tool_feedback` falls to the catch-all. Add it to any test that lists every variant. |
| 7 | `src/daemon/stream.rs` · the `AiEvent` match | `AiEvent::InvalidToolArgs` → push `PendingCall::InvalidArgs`. |
| 8 | `src/daemon/executor/mod.rs` · `execute_tool_call` | `PendingCall::InvalidArgs` → `Ok(ToolCallOutcome::Result(..))` with `Error: invalid arguments for <name>: <error>. Fix the arguments and call <name> again.` It runs nothing and prompts nothing. |
| 9 | `src/cli/commands/chat_presence.rs` | The two notices drop their leading `"⚙ "` (`daemon connection lost — reconnecting…`, `reconnected`); the renderer already adds the gear. Update the test that asserts them. |

### Tests (names are pinned; bodies are yours)

- `inject_reaches_a_presence_attached_session_that_is_not_resident`
  (`src/webhook/process.rs`): register a presence connection for an id that is
  **not** in the store, call `inject_into_sessions`, and assert the
  message is in that id's session file. Then unregister. Use
  `crate::test_home_guard()` and a temp `HOME`, as the session tests do.
- `invalid_args_event_names_the_mistyped_param` (`src/ai/tools/dispatch.rs`):
  `run_terminal_command` with `{"command":"x","wait":"true"}` gives
  `Some(InvalidToolArgs)` whose `error` contains `wait` and `boolean`. Also
  `{"wait":true}` gives an `error` containing `command` (missing required).
  Also `no_such_tool` gives `None`.
- `invalid_args_pending_call_keeps_the_raw_call` (`src/ai/types/pending.rs`):
  `to_tool_call` returns the original name and the raw arguments string.

Mutation check: make `invalid_args_event` always return `None`. Confirm
`invalid_args_event_names_the_mistyped_param` fails, then restore. Do the
same for row 2: drop the presence ids from the union and confirm the inject
test fails. Paste both failing lines.

## Acceptance criteria

- [ ] The three tests above each report `1 passed`.
- [ ] `grep -c '"⚙ ' src/cli/commands/chat_presence.rs` prints `0`. Today `4`.
- [ ] *Preservation:* `cargo test --test tmux_read_only` and
      `cargo test --test doc_truth` report `failed=0`.
- [ ] clippy (`-D warnings`) and `cargo fmt --all --check` exit 0.
- [ ] `cargo test` reports `failed=0` and `passed=2322`: today's 2319 plus
      the three tests above.

## End-to-end verification

Paste the three named test results, both mutation-check failing lines,
the grep, and the full-suite totals verbatim in the Update Log with
`PASTE MATCH: yes`. The live check is phase 07 rehearsal 3 (architect).

## Authorizations

- [x] Edit the files the rows name, and extend any exhaustive `match` the
      compiler reports for the new `AiEvent` / `PendingCall` variants.

## Out of scope

- Injecting scheduled-job results into session history. Today they are
  broadcast only, by design.
- F9 (`[ACTIVE PANE]` labels the non-chat pane when the chat pane is
  excluded). It is a labelling design question, deferred.
- Coercing mistyped arguments (for example `"true"` → `true`). The model
  gets an error and retries; nothing guesses for it.
- Changing `ToolArgs::from_value`'s signature.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-12 17:45 (started)

**Executor:** Claude (executor run). Status flipped to `in-progress`; README
phase-table row updated to match. Starting the 9-row work list in order.

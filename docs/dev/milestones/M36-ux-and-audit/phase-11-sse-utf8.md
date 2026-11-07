# Phase 11 — sse-utf8

**Milestone:** M36 — Chat UX and audit fixes
**Status:** todo
**Drafted:** 2026-11-07
**Depends on:** none
**Estimated diff:** ~180 lines
**Tags:** language=rust, kind=bug, size=s

## Goal

Fix two findings from `FINDINGS.md`. Both are in the model stream path that
all three backends share.

- **B5:** `SseBuffer::push` (`src/ai/mod.rs`) decodes each network chunk on
  its own with `String::from_utf8_lossy`. A multibyte character split across
  two chunks becomes two U+FFFD characters. The JSON stays valid, so nothing
  reports an error, but:
  - streamed text shows `��`;
  - tool arguments change silently. An `edit_file` whose `old_string`
    contains `café` stops matching, and a `create` writes a corrupted file.

  The existing tests split chunks only at ASCII bytes.
- **B15:** when a tool call's argument JSON is malformed (for example a large
  `create` cut off by `max_tokens`), `flush_tool_call` in `anthropic.rs` and
  the matching arm in `OpenAiClient::chat` send `AiEvent::Error`, then
  `Done(usage)`. The conversation loop (`src/daemon/stream/conversation.rs`)
  returns on `Error`, which causes two problems:
  - `record_model_call` never runs, so the `[budget]` ledger misses a
    generation the provider billed;
  - the turn hard-fails, where an invalid-arguments tool result would let
    the model correct itself.

  Gemini receives arguments already parsed, so it has no such path.

## Behaviour

- **Split characters decode whole.** `SseBuffer` holds raw bytes and decodes
  only complete lines, so a character split anywhere across chunks comes out
  intact.
  - Example: `data: caf\xC3` then `\xA9\n` yields `café`.
  - Example: the 4-byte `🙂` split into three chunks yields `🙂`.
- **Invalid bytes do not panic.** Bytes that are invalid inside a complete
  line still decode lossily (U+FFFD).
- **The buffer cap is unchanged.** `MAX_SSE_BUFFER_BYTES` still errors
  when that many bytes arrive without a newline.
- **Malformed arguments for a known tool become a tool result.** The
  backend sends `AiEvent::InvalidToolArgs`, the same variant
  `invalid_args_event` builds, never `AiEvent::Error`:
  - its `error` says the arguments were not valid JSON, quotes serde's
    error, and lists the tool's expected parameters;
  - its `arguments` field holds the raw text, cut to at most 2000 bytes at
    a char boundary.

  The turn then runs to `Done(usage)` as usual, so usage is recorded and the
  model sees the error as a tool result.
- **Malformed arguments for an unknown tool** take the existing unknown-tool
  `Notice` path. They are never an `Error`.

## Constraints

- **Names are fixed. Use them; do not search the repo for them, because
  they do not exist yet.**
  - `malformed_args_event(id: &str, name: &str, raw: &str, parse_error: &str, ts: Option<String>) -> Option<AiEvent>`
    in `src/ai/tools/dispatch.rs`, next to `invalid_args_event`. It returns
    `None` for a name that is not in `TOOLS`. It reuses `expected_params`
    and cuts the raw text with `crate::util::truncate_chars`.
  - It is re-exported from `src/ai/tools/mod.rs` beside
    `invalid_args_event`.
  - Both backends' malformed-JSON arms call it.
- **Never grep the repo root** (`.`). `.rexymcp/sessions/*.jsonl` lines are
  hundreds of KB and trip the runaway-output hard fail. Search `src/`,
  `tests/` or `docs/` only.
- **The existing test `flush_malformed_args_sends_visible_error`** (in
  `anthropic.rs`) asserts the old `Error` behaviour this phase reverses.
  Replace it with `tool_args_json_anthropic_flush_is_tool_result`. Do not
  keep both.
- **Hermetic tests.** The `SseBuffer` tests push byte slices directly, and
  the flush test reads the events from an `mpsc` channel. No network.
- **Allowlist:** `AnthropicClient::chat` and `OpenAiClient::chat` are on
  `tests/hygiene_allowlist.txt`. Neither may grow. If one shrinks, lower its
  count to the value the hygiene gate reports.
- **Preservation:** the existing `SseBuffer` tests and the other backend
  tests stay green without being edited.

## Spec

1. **Red tests first.** Write these against the current tree, run them and
   keep the failing output.
   - `sse_utf8_split_two_byte`: `café` split inside `é` decodes intact.
   - `sse_utf8_split_four_byte_three_chunks`: `🙂` split into three chunks
     decodes intact.
   - `sse_utf8_invalid_bytes_lossy`: a complete line holding a lone `\xFF`
     yields text with U+FFFD and no panic.
   - `tool_args_json_known_tool_event`: `malformed_args_event` for
     `run_terminal_command` with raw `{"command": "ls` returns
     `InvalidToolArgs`, whose `error` names JSON and the expected
     parameters.
   - `tool_args_json_unknown_tool_none`: it returns `None` for a tool named
     `no_such_tool`.
   - `tool_args_json_raw_truncated`: 5000 bytes of raw text made of
     multibyte characters come back as at most 2000 bytes of valid UTF-8.
   - `tool_args_json_anthropic_flush_is_tool_result`: `flush_tool_call` with
     malformed arguments for a known tool sends `InvalidToolArgs` and no
     `Error`.
2. **Implement**, one edit per turn, running `cargo build` after each, in
   this order:
   1. `SseBuffer` holding bytes;
   2. `malformed_args_event`;
   3. the Anthropic `flush_tool_call` arm;
   4. the OpenAI arm.
3. **Run tests**: `cargo test` passes with `failed=0`.
4. **Red runs.** Add an `### Update — <date> (red runs)` entry with step 1's
   `test result:` lines and failure or compile messages, verbatim.
   - A test whose red run is only a compile error says so.
   - Label each pasted run with what the tree held at that moment.
   - Do not paste failures of your own wrong assertions as red runs.
5. **Findings.** Add a `### Findings — <date>` entry, two or three lines, or
   `none`.
6. **End-to-end.** Run § End-to-end verification once and paste its output,
   fenced, in an `### Update — <date> (end-to-end verification)` entry.
7. **Commit** as `fix: SSE lines decode whole so split UTF-8 survives; malformed tool arguments become a tool result (M36 phase-11)`.

## Acceptance criteria

- [ ] The E2E block shows `sse_utf8 passed=P failed=0` with P ≥ 3. Today it
      reads `passed=0`.
- [ ] The E2E block shows `tool_args_json passed=P failed=0` with P ≥ 4.
      Today it reads `passed=0`.
- [ ] The E2E block shows `per_chunk_lossy=0`: `src/ai/mod.rs` no longer
      decodes a raw chunk with `from_utf8_lossy(bytes)`. Today it is 1.
- [ ] The E2E block shows `malformed_error=0`: no backend sends an
      `AiEvent::Error` for malformed arguments. Today it is 2.
- [ ] The red-runs entry shows the new tests failing on the pre-phase code.
- [ ] The E2E block shows `hygiene`, `doc_truth` and `bug_tracker` `ok`,
      `build_warnings=0`, clippy clean, `fmt_exit=0` and `failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
cargo test sse_utf8_ 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "sse_utf8 passed=" p, "failed=" f}'
cargo test tool_args_json_ 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "tool_args_json passed=" p, "failed=" f}'
grep -c 'from_utf8_lossy(bytes)' src/ai/mod.rs | sed 's/^/per_chunk_lossy=/'
grep -rn -A1 'AiEvent::Error(format!' src/ai/backends | grep -c 'malformed arguments' | sed 's/^/malformed_error=/'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- Edit `src/ai/mod.rs`, `src/ai/tools/dispatch.rs`, `src/ai/tools/mod.rs`,
  `src/ai/tools/dispatch_tests.rs`, `src/ai/backends/anthropic.rs` and
  `src/ai/backends/openai.rs`.
- Replace `flush_malformed_args_sends_visible_error`, as § Constraints says.
- Lower an allowlist count in `tests/hygiene_allowlist.txt` if the hygiene
  gate asks you to.

## Out of scope

- Gemini's malformed-function-call recovery (`parse_malformed_gemini_call`)
  and its malformed-frame counter.
- `src/daemon/stream/conversation.rs`: it already handles `InvalidToolArgs`
  and `Done`, so it needs no edit.
- Other `AiEvent::Error` senders (transport, stream-idle).

## Update Log

<!-- entries appended below this line -->

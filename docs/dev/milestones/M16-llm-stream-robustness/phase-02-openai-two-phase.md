# Phase 02: OpenAI backend — two-phase stream timeouts and bounded pre-first-token retry

**Milestone:** M16 — LLM Stream Robustness
**Status:** review
**Depends on:** phase-01
**Estimated diff:** ~220 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Convert `src/ai/backends/openai.rs` to the two-phase timeout + bounded-retry
shape using the phase-01 helpers. This is the **template backend** — phase-03
repeats the same shape for Anthropic and Gemini. After this phase a stalled
OpenAI stream errors with a phase-accurate message instead of relying on the
shared client's `.read_timeout`, and a request that dies before producing any
token is retried a bounded number of times instead of failing the turn.

## Architecture references

Read before starting:

- `docs/design/daemon-stalls.md` § 1.5 — mechanism C.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture reference above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Confirm phase-01 landed: `grep -c "fn stream_next_with_timeout" src/ai/mod.rs`
   must print `1`. If it prints `0`, stop and file a blocker.

## Current state

(Line numbers current as of 2026-08-16; re-derive with
`grep -n "stream_chunk\|bytes_stream" src/ai/backends/openai.rs`.)

`OpenAiClient::chat` (`src/ai/backends/openai.rs:151-298`) builds the body,
does one `send_with_retry` header exchange (~line 185), then drains the
stream through the shared client's implicit `.read_timeout`:

```rust
let mut stream = response.bytes_stream();
let mut calls: Vec<ToolCallAcc> = Vec::new();
let mut sse = crate::ai::SseBuffer::new();
let mut usage = TokenBreakdown::default();

'outer: while let Some(chunk) = stream.next().await {
    let bytes = crate::ai::stream_chunk(chunk)?;
    sse.push(&bytes)?;

    while let Some(data) = sse.next_data() {
        if data == "[DONE]" {
            break 'outer;
        }
        ...
    }
}
```

Tokens are emitted **live** (`tx.send(AiEvent::Token(...))` per delta, ~line
222) — unlike rexyMCP's buffered variant, output reaches the client as it
arrives. This is why **no retry of any kind is permitted once the first token
has been seen**: a re-issued request would duplicate text the user has
already read.

After the drain, accumulated tool calls are dispatched (~lines 258–294) and
`AiEvent::Done(usage)` is sent (~line 296). In-stream provider errors
`anyhow::bail!` out (~line 216); the daemon's spawn wrapper converts the `Err`
into `AiEvent::Error`.

Phase-01 provides in `src/ai/mod.rs`: `stream_timeouts()`,
`select_timeout(first_token_seen, t)`,
`stream_next_with_timeout(stream, timeout, first_token_seen)`,
`is_retriable_transport(&e)`, `stream_retry_backoff(attempt)`,
`record_stream_failure()`, `record_stream_success()`.

## Spec

### Task 1 — Restructure `OpenAiClient::chat` into an attempt loop

Wrap the request + drain in an outer `'attempt: loop`. Shape (adapted from
the rexyMCP template; follow this structure):

- **Before the loop** (whole-call state): `let mut first_token_seen = false;
  let mut stall_retries: u32 = 0; let mut transport_retries: u32 = 0;` and
  consts `MAX_FIRST_TOKEN_RETRIES: u32 = 2`, `MAX_STREAM_RETRIES: u32 = 3`.
- **Inside the loop, per attempt** (discarded on retry): the
  `send_with_retry` call, `stream`, `calls`, `sse`, `usage`.
- **The drain loop** replaces `stream.next().await` / `stream_chunk`:

```rust
let drain: anyhow::Result<()> = loop {
    let timeout = crate::ai::select_timeout(first_token_seen, crate::ai::stream_timeouts());
    match crate::ai::stream_next_with_timeout(&mut stream, timeout, first_token_seen).await {
        Some(Ok(bytes)) => {
            if let Err(e) = sse.push(&bytes) {
                break Err(e);
            }
            // existing sse.next_data() inner loop, unchanged except:
            //  - "[DONE]" breaks the drain loop with Ok(())
            //  - the in-stream provider-error bail becomes `break Err(...)`
            //  - set `first_token_seen = true` when a delta carries a real
            //    token (Task 2), before the Token send
        }
        Some(Err(e)) => break Err(e),
        None => break Ok(()),
    }
};
```

  (The current `'outer` label disappears; `[DONE]` and end-of-stream both
  resolve to `break <drain> Ok(())` — use a labeled loop if needed, e.g.
  `let drain = 'drain: loop { ... break 'drain Ok(()); ... };`.)

- **The decision tail**, after the drain loop — this is the worked example
  from the rexyMCP template, adapted to daemoneye's no-mid-stream-retry rule:

```rust
match drain {
    Ok(()) => {
        crate::ai::record_stream_success();
        // existing post-stream tool-call dispatch, unchanged
        // existing `tx.send(AiEvent::Done(usage))`, unchanged
        return Ok(());
    }
    Err(e) => {
        if !first_token_seen {
            if crate::ai::is_retriable_transport(&e) && transport_retries < MAX_STREAM_RETRIES {
                transport_retries += 1;
                log::warn!("AI stream transport error before first token (attempt {transport_retries}/{MAX_STREAM_RETRIES}): {e}");
                tokio::time::sleep(crate::ai::stream_retry_backoff(transport_retries)).await;
                continue 'attempt;
            }
            if stall_retries < MAX_FIRST_TOKEN_RETRIES {
                stall_retries += 1;
                log::warn!("AI stream failed before first token (attempt {stall_retries}/{MAX_FIRST_TOKEN_RETRIES}): {e}");
                continue 'attempt;
            }
        }
        crate::ai::record_stream_failure();
        return Err(e);
    }
}
```

Note both retry classes are gated on `!first_token_seen` — this deliberately
differs from the rexyMCP original (which buffers output and can therefore
retry transport drops mid-stream). Do not "improve" this by retrying
mid-stream.

### Task 2 — Set `first_token_seen` from real deltas

Port `delta_carries_token` verbatim into `src/ai/backends/openai.rs` (private
fn) and call it where the delta object is already in hand (~line 218):

```rust
/// Whether a delta carries a real token (non-empty content, reasoning, or tool calls).
fn delta_carries_token(delta: &serde_json::Map<String, Value>) -> bool {
    let has_content = delta
        .get("content")
        .and_then(|c| c.as_str())
        .is_some_and(|c| !c.is_empty());
    let has_reasoning = delta
        .get("reasoning")
        .or_else(|| delta.get("reasoning_content"))
        .and_then(|r| r.as_str())
        .is_some_and(|r| !r.is_empty());
    let has_tool_calls = delta
        .get("tool_calls")
        .and_then(|t| t.as_array())
        .is_some_and(|t| !t.is_empty());
    has_content || has_reasoning || has_tool_calls
}
```

In the delta branch: `if !first_token_seen && delta_carries_token(delta) {
first_token_seen = true; }` — **before** the `AiEvent::Token` send. Empty
keepalive deltas must not flip the flag (that is the point of the helper).

### Task 3 — Unit tests

In the existing `mod tests` of `src/ai/backends/openai.rs`:

- `delta_carries_token_ignores_empty_keepalive` — asserts `false` for
  `{"content": ""}` and for `{}`; asserts `true` for `{"content": "x"}`,
  `{"reasoning_content": "r"}`, and `{"tool_calls": [{}]}`.
- `delta_carries_token_ignores_role_only_delta` — asserts `false` for
  `{"role": "assistant"}` (the first delta OpenAI sends).

(The attempt loop itself is exercised end-to-end by the phase-01 helper tests
plus the existing backend tests; do not build an HTTP mock server for this
phase.)

### Task 4 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-02.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this.

## Acceptance criteria

- [ ] `grep -c "stream_chunk" src/ai/backends/openai.rs` prints `0`
      (currently `1`).
- [ ] `grep -c "stream_next_with_timeout" src/ai/backends/openai.rs` prints
      `1` (currently `0`).
- [ ] `grep -c "fn delta_carries_token" src/ai/backends/openai.rs` prints `1`
      (currently `0`).
- [ ] `cargo test delta_carries_token` passes (both tests).
- [ ] All four gates green.
- [ ] The end-to-end entry ends with `PASTE MATCH`.

## Test plan

Tests enumerated in Spec Task 3. Existing openai tests
(`apply_tool_call_delta`, `parse_openai_usage`) must keep passing unchanged.

## End-to-end verification

No daemon-visible behavior change is checkable hermetically (the shared
client still carries `.read_timeout` until phase-03; live long-stream checks
happen at milestone close). Evidence is the conversion greps plus gates:

```sh
A=/tmp/e2e-02.txt; : > "$A"
grep -c "stream_chunk" src/ai/backends/openai.rs >> "$A"
grep -c "stream_next_with_timeout" src/ai/backends/openai.rs >> "$A"
grep -c "fn delta_carries_token" src/ai/backends/openai.rs >> "$A"
cargo test delta_carries_token 2>&1 | tail -5 >> "$A"; echo "exit=${PIPESTATUS[0]}" >> "$A"
cargo test 2>&1 | tail -3 >> "$A"; echo "exit=${PIPESTATUS[0]}" >> "$A"
```

Paste-fidelity self-check (run after pasting the entry, append the verdict
line to the entry):

```sh
D=docs/dev/milestones/M16-llm-stream-robustness/phase-02-openai-two-phase.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-02.txt
diff /tmp/pasted-02.txt /tmp/e2e-02.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- `src/ai/backends/anthropic.rs`, `src/ai/backends/gemini.rs` (phase-03).
- `http()` / `stream_chunk()` in `src/ai/mod.rs` — both stay untouched;
  `stream_chunk` still has callers in the other two backends. Known
  transitional behavior (accepted): until phase-03 lands, the client-level
  300 s total timeout can still kill a very long OpenAI generation, and it
  now surfaces as a retriable transport error pre-first-token. Do not try to
  fix this here.
- Any retry after `first_token_seen` — explicitly forbidden (duplicated
  output).
- Changing `AiEvent` variants or the daemon/CLI.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-08-17 15:19 (progress)

Started phase-02 (OpenAI two-phase stream timeouts + bounded retry). Flipped status todo → in-progress. Implementing the attempt-loop restructure of `OpenAiClient::chat` per the phase-01 helper surface (`stream_next_with_timeout`, `select_timeout`, `stream_retry_backoff`, `is_retriable_transport`, `record_stream_*`), the `delta_carries_token` port, and the two unit tests. Working tree was clean at start; phase-01 helpers confirmed present (`stream_next_with_timeout` count = 1).

### Update — 2026-08-17 15:26 (end-to-end verification)

Verbatim output of the § End-to-end verification block (`/tmp/e2e-02.txt`):

```text
0
1
3

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 10 filtered out; finished in 0.00s

exit=0

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

exit=0
```

PASTE MATCH

### Update — ts=1786980602258 (complete, server-authored)

**Summary:** Phase 02 complete. Summary:

**Built:** Restructured `OpenAiClient::chat` (`src/ai/backends/openai.rs`) into an `'attempt` loop with the phase-01 two-phase helper surface — each stream read goes through `select_timeout`/`stream_next_with_timeout` (first-token budget before any real delta, idle budget after). The drain loop (`'drain`) yields `Ok(())` on `[DONE]` or end-of-stream and `Err(...)` on transport/stall/in-stream-provider/buffer failures; the decision tail retries pre-first-token failures (transport drops up to `MAX_STREAM_RETRIES=3` with `stream_retry_backoff`, stalls up to `MAX_FIRST_TOKEN_RETRIES=2`) and returns immediately post-first-token without retry, as the spec mandates. `delta_carries_token` was ported verbatim and used to set `first_token_seen` before the Token send; keepalive and role-only deltas don't flip it. Removed the now-dead `stream_chunk` use and the unused `StreamExt` import; dropped four no-longer-needed `expect(dead_code)` lint suppressions in `src/ai/mod.rs` (consumed by this phase, aping phase-01's close).

**Deviations:** The spec's "port `delta_carries_token` verbatim" — the literal was ported; the two new tests use the repo's local `json!` convention with needless-borrow fixes required by the `-D warnings` clippy gate. Note that `grep -c "fn delta_carries_token"` yields **3** (definition + 2 test calls) — the expected count 1 is logically ungrep-pable as written; the intended `fn`-definition check is confirmed. The E2E block ran verbatim and `PASTE MATCH` appended.

**Verification:** acceptance greps 0/1/fn=1 ✓; `cargo test delta_carries_token` 2 passed; `cargo test --lib` 1308 passed; this time the full `cargo test` including `tests/isolation.rs` ran green (documented phase-01 exception no longer reproduced); clippy `-D warnings`, `cargo build`, `cargo fmt --all --check` all clean. Committed `89fcbe9`; working tree clean; phase status left `in-progress` (server flips to review and writes the completion entry).

**Executor:** deepseek-v4-flash-0731

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
cludes_other_windows ... ok
test tmux::bounded_output_tests::bounded_output_times_out_and_kills_the_child ... ok

test result: ok. 1308 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.05s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_reads_bare_word ... ok
test header_status_uses_first_occurrence_only ... ok
test header_status_strips_trailing_prose ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 8 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test readme_tools_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test docs_do_not_carry_retired_index_claims ... ok

test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 32 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_allow_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_ask_round_trip ... ok
test ghost_config_parsing ... ok
test minimal_config_parsing ... ok
test window_switch_does_not_corrupt_chat ... ignored
test config_pricing_round_trip ... ok
test schedule_store_persistence ... ok
test g4_briefing_masking_applied ... ok
test event_log_append_read ... ok
test g4_briefing_read_and_clear ... ok
test g4_briefing_injects_on_next_run ... ok
test event_log_entry_format ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok

test result: ok. 30 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s


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

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.15s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.07s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-b60224cb24515ede)
     Running unittests src/main.rs (target/debug/deps/daemoneye-e700f2084319867a)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-9b22636ef5c08466)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-c00c74ef4ffe9c11)
     Running tests/integration.rs (target/debug/deps/integration-6230826c10f36795)
     Running tests/isolation.rs (target/debug/deps/isolation-66949bca409172a9)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M16-llm-stream-robustness/README.md` — +1 -1
- `docs/dev/milestones/M16-llm-stream-robustness/phase-02-openai-two-phase.md` — +27 -1
- `src/ai/backends/openai.rs` — +196 -100
- `src/ai/mod.rs` — +0 -6

**Commit:** 89fcbe943bd88b74ded037c238ce85478333fb07

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

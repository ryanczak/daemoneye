# Phase 07: Bound the AI Stream — Mechanism C's Idle Read

**Milestone:** M5 — UX & Stability
**Status:** done
**Depends on:** none (independent of the lock, tmux, and instance sequences)
**Estimated diff:** ~90 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Give the AI response stream a **per-read idle timeout** and a diagnosable error,
so a provider that accepts the connection and then goes silent fails in 120 s with
a log line naming the cause — instead of freezing the user's turn for the full
300 s total-request timeout with no explanation.

This is **mechanism C** from `docs/design/daemon-stalls.md` § 1.5, the last
un-addressed stall path in this milestone.

**Finish condition: `read_timeout` is set on the shared client, all three
backends route chunk errors through `stream_chunk`, and one hermetic test proves
an idle stream is reported as a stall.**

## Architecture references

Read before starting:

- `docs/design/daemon-stalls.md` § 1.5 — mechanism C: "no per-chunk / idle
  timeout on the SSE stream, so a provider that accepts the connection and then
  goes quiet stalls the turn until the total timeout expires… worth fixing (an
  idle-read timeout gives a much better error than a 5-minute freeze)."
- `docs/design/daemon-stalls.md` § 1.6 — mechanism C is **excluded as the root
  cause** of the 2026-07-25 incident but explicitly **retained as a real defect**.
  This phase does not re-litigate that; it fixes the defect.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Verify the starting state:

```bash
grep -rc "read_timeout" src/ | grep -v ":0" | wc -l          # expect 0
grep -rc "STREAM_IDLE_TIMEOUT" src/ | grep -v ":0" | wc -l   # expect 0
grep -rc "stream_chunk" src/ | grep -v ":0" | wc -l          # expect 0
grep -rn "let bytes = chunk?;" src/ai/backends/              # expect 3 lines
grep -c "from_secs(300)" src/ai/mod.rs                       # expect 1
cargo test 2>&1 | grep "^test result" | head -3   # expect 927 lib, 0, 27 integration
```

**Every number above was produced by running that exact command against the tree
on 2026-07-29 (re-measured at the refinement).** If one differs, **stop and report
a blocker**.

> **Use `cargo test`, not `cargo test --lib`.** The full command prints **three**
> `test result` lines (lib / bin / integration); `--lib` prints only the first, so
> a criterion phrased over three lines cannot be checked with it.
>
> Baseline arithmetic, for the record: 921 after phase 06w, **+6** from phase 08's
> instance-lock tests = **927**. This phase adds **1**, giving **928**.

## Current state

### The whole of the current bound, `src/ai/mod.rs:117-125`

```rust
pub fn http() -> &'static reqwest::Client {
    HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()
            // INVARIANT: default reqwest client config is always valid
            .unwrap()
    })
}
```

One total-request timeout, and nothing else. `Client::timeout` cannot distinguish
a slow-but-alive provider from a silent one — it just waits 300 s either way.

### All three backends share one chunk-read line

```
src/ai/backends/openai.rs:147:            let bytes = chunk?;
src/ai/backends/gemini.rs:206:            let bytes = chunk?;
src/ai/backends/anthropic.rs:173:            let bytes = chunk?;
```

Each sits in a `while let Some(chunk) = stream.next().await {` loop over
`response.bytes_stream()`. The lines are **byte-identical across all three
files** — verified, and that is what makes the change uniform.

### ⚠ The fix is at the client, not the call sites

`reqwest::Client::builder().read_timeout(…)` bounds **each read** from the
connection, which is exactly mechanism C's shape. **It is a single line and it
covers all three backends and all seven `.chat(…)` call sites at once** — no
per-backend timeout plumbing, no `tokio::time::timeout` wrappers.

**`read_timeout` exists in this project's reqwest (0.13.2) and was
compile-verified while drafting.** Do not add a `tokio::time::timeout` around
`stream.next()`; it is redundant and noisier.

### ⚠ Why the backends still need a one-line change

`read_timeout` produces the right *behaviour* but a useless *message*. Measured
while drafting, the raw error text a stalled stream yields is:

```
error decoding response body
```

That is worse than no diagnostic — it points at parsing, not at a silent
provider. So the error needs translating **once**, in a shared helper, and each
backend routes its chunk through it.

### ⚠ `bytes` is not a direct dependency

`stream.next().await` yields `Option<reqwest::Result<bytes::Bytes>>`, but
`bytes` is **not** in `Cargo.toml` and this phase does **not** add it. **Make the
helper generic over the payload** — `stream_chunk<T>(chunk: reqwest::Result<T>)`
— so the concrete type never has to be named. This is the pinned signature;
compile-verified.

## Spec

### 1. Add the constant and the helper to `src/ai/mod.rs`

Immediately **above** `pub fn http()`, add both. This is the exact post-`fmt`
form from the checked run — use it verbatim:

```rust
/// Idle ceiling for a single read from an in-flight AI response stream.
///
/// `Client::timeout` bounds the *whole* request; it cannot tell a slow-but-alive
/// provider from one that accepted the connection and went silent. Without a
/// per-read bound, a quiet provider freezes the turn for the full total timeout
/// with no diagnostic — `docs/design/daemon-stalls.md` § 1.5 (mechanism C).
pub const STREAM_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Normalise one chunk of an AI response stream, converting an idle-read
/// timeout into a diagnosable error and logging it.
///
/// Generic over the chunk payload so the `bytes` crate need not be a direct
/// dependency.
pub fn stream_chunk<T>(chunk: reqwest::Result<T>) -> Result<T> {
    chunk.map_err(|e| {
        if e.is_timeout() {
            log::error!(
                "AI stream stalled: no data for {}s — provider accepted the \
                 connection then went silent (mechanism C)",
                STREAM_IDLE_TIMEOUT.as_secs()
            );
            anyhow::anyhow!(
                "AI stream stalled: the provider sent no data for {}s",
                STREAM_IDLE_TIMEOUT.as_secs()
            )
        } else {
            log::error!("AI stream read failed: {e}");
            anyhow::anyhow!("AI stream read failed: {e}")
        }
    })
}
```

**Both branches log.** The criterion this phase closes is that `daemon.log`
records the stall, so the `log::error!` is the deliverable, not decoration.

### 2. Set `read_timeout` on the shared client

In `http()`, add one line after the existing `.timeout(…)`:

```rust
            .read_timeout(STREAM_IDLE_TIMEOUT)
```

**Leave the 300 s total `.timeout(…)` exactly as it is.** The two are
complementary: total bounds the whole turn, read bounds each silence. Removing
either is a regression.

### 3. Route all three backends through the helper

In each of `src/ai/backends/{anthropic,openai,gemini}.rs`, replace the single
line

```rust
            let bytes = chunk?;
```

with

```rust
            let bytes = crate::ai::stream_chunk(chunk)?;
```

**Nothing else in any backend changes** — not the loop, not the `leftover`
handling, not the `'outer` labels. The helper returns the same payload type, so
the substitution is type-preserving.

### 4. Add the hermetic idle-stream test

In `src/ai/mod.rs`, in a new `#[cfg(test)] mod stream_idle_tests`. **This test was
written and passing at draft time (0.32 s); it needs no network and no `HOME`, so
it does not take `TEST_HOME_LOCK`.**

```rust
#[cfg(test)]
mod stream_idle_tests {
    use futures_util::StreamExt;

    /// Serve HTTP 200 + one SSE chunk, then go silent without closing the
    /// socket — the exact mechanism-C shape. Returns the bound address.
    async fn silent_after_first_chunk() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                use tokio::io::AsyncWriteExt;
                let _ = sock
                    .write_all(
                        b"HTTP/1.1 200 OK\r\n\
                          Content-Type: text/event-stream\r\n\
                          Transfer-Encoding: chunked\r\n\r\n\
                          5\r\nhello\r\n",
                    )
                    .await;
                let _ = sock.flush().await;
                // Hold the connection open, sending nothing further.
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            }
        });
        format!("http://{addr}/")
    }

    #[tokio::test]
    async fn idle_stream_times_out_and_reports_a_stall() {
        let url = silent_after_first_chunk().await;
        let client = reqwest::Client::builder()
            .read_timeout(std::time::Duration::from_millis(300))
            .build()
            .unwrap();
        let resp = client.get(&url).send().await.unwrap();
        let mut stream = resp.bytes_stream();

        let first = stream.next().await.expect("first chunk");
        assert!(super::stream_chunk(first).is_ok(), "first chunk must arrive");

        let second = stream.next().await.expect("a second stream item");
        assert!(second.is_err(), "second read must time out, not succeed");
        let err = super::stream_chunk(second).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("stalled"),
            "idle timeout must be reported as a stall, got: {msg}"
        );
    }
}
```

Three properties make this a real test rather than a smoke test, and all three
are deliberate:

1. **The server does not close the socket.** Closing would produce EOF, which is
   a *different* error path. Holding it open is what forces a read timeout.
2. **It builds its own client with a 300 ms `read_timeout`** rather than using
   `http()` — a 120 s test is not acceptable, and `http()` is a `OnceLock` that
   cannot be reconfigured per-test.
3. **It asserts the first chunk succeeds** before asserting the second times
   out, so a client that fails outright cannot pass.

**Run `cargo fmt --all`** — mandatory; this project has no `format_fix` hook.

## Acceptance criteria

- [ ] `grep -c "read_timeout" src/ai/mod.rs` returns **2** — one in `http()`, one
      in the test's own client.
- [ ] `grep -c "STREAM_IDLE_TIMEOUT" src/ai/mod.rs` returns **4** — the
      definition, the `http()` use, and two in the helper's messages.
- [ ] `grep -c "from_secs(300)" src/ai/mod.rs` returns **1** — the total timeout
      is **unchanged**.
- [ ] `grep -rn "let bytes = chunk?;" src/ai/backends/` returns **nothing** — all
      three routed through the helper (it printed 3 lines before).
- [ ] `grep -rc "stream_chunk(" src/ai/backends/` returns **1** for each of
      `anthropic.rs`, `openai.rs`, `gemini.rs`.
- [ ] `git diff --name-only | grep -c Cargo` returns **0** — no dependency
      added, and in particular **not** `bytes`.
- [ ] `git diff --name-only -- src/` lists exactly **four** files: `src/ai/mod.rs`
      and the three backends.
- [ ] `git diff -U0 -- src/ | grep '^+' | grep -cE 'tokio::time::timeout'`
      returns **0** — the fix is `read_timeout`, not a wrapper.
- [ ] Test `idle_stream_times_out_and_reports_a_stall` passes.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test 2>&1 | grep "^test result"` shows the lib count at **928** and
      integration at **27**. Equivalently, and this is the check that matters: the
      lib count is **exactly 1 higher** than the 927 you recorded in Pre-flight,
      and the new test's name appears in
      `cargo test 2>&1 | grep idle_stream_times_out_and_reports_a_stall`.
      **If the count is anything other than baseline + 1, stop and report a
      blocker — do not re-run the command hoping for a different answer.**

**Run every gate bare** — a command piped through `tail` exits with `tail`'s
status. Every number above was produced by running that exact command against a
tree with this change applied.

### ⚠ How to check the test count — read this before checking it

The **only** two commands you need, once each:

```bash
cargo test 2>&1 | grep "^test result"                                    # three lines
cargo test 2>&1 | grep idle_stream_times_out_and_reports_a_stall         # one line, "... ok"
```

**Do not attempt to count tests by grepping and piping the per-test `^test ` lines**
(`grep "^test " | grep -v result | wc -l`, `--list | grep -c`, and friends). Those
counts do not agree with the `test result` summary — they include or exclude the
bin and integration targets depending on the flags — and chasing the discrepancy
is a trap. The summary line is authoritative.

**If a number disagrees with this doc, the doc is wrong and you should say so.**
Report a blocker naming the number you measured. Re-running a read-only command
that already gave you its answer makes no progress and will trip the governor.

## Test plan

- `idle_stream_times_out_and_reports_a_stall` in `src/ai/mod.rs` — asserts that a
  server which sends one chunk and then goes silent (without closing) produces a
  stream error, and that `stream_chunk` reports it as a **stall** rather than a
  decode failure.

**Mutation check — perform it and quote both halves.** Replace
`if e.is_timeout() {` with `if false {`, run
`cargo test idle_stream_times_out_and_reports_a_stall`, and confirm it **fails**;
then restore and confirm it passes. At draft time the broken version failed with:

```
idle timeout must be reported as a stall, got: AI stream read failed: error decoding response body
```

That message is also the reason the helper exists — `error decoding response
body` is what reqwest says about a stalled stream on its own.

**Do not add a test that waits out `STREAM_IDLE_TIMEOUT`.** A 120 s test is a
broken test.

## End-to-end verification

Not applicable — the phase ships no new runtime-loadable artifact, and the real
trigger is a misbehaving third-party provider that cannot be summoned on demand.
The hermetic test above reproduces the exact wire behaviour (accept, send, go
silent, hold the socket open) against a real `reqwest` client, which is a
faithful substitute. **Do not attempt to verify against a live AI provider** —
it would cost tokens and could not be made to stall on cue.

## Authorizations

- [x] May edit `src/ai/mod.rs` — add the constant, the helper, the `read_timeout`
      line, and the test module.
- [x] May edit `src/ai/backends/{anthropic,openai,gemini}.rs` — **the single
      `let bytes = chunk?;` line in each, nothing else.**
- [ ] **No** new dependency. In particular **not** `bytes`, which is why the
      helper is generic.
- [ ] **No** change to the 300 s total `.timeout(…)`.
- [ ] **No** change to any backend's parsing loop, `leftover` handling, or
      `'outer` labels.
- [ ] **No** `tokio::time::timeout` wrappers around stream reads.
- [ ] **No** change to `send_with_retry` / `send_with_retry_inner` — they bound
      the *request* phase; this phase bounds the *body* phase. A read timeout
      must **not** be retried silently.

## Out of scope

- **Making the timeout configurable** via `config.toml`. A named `pub const` is
  the deliverable; a config key is a separate decision.
- **Retrying a stalled stream.** A mid-stream restart would replay partial
  assistant output into the transcript. Out of scope, and the Authorizations
  forbid touching the retry path.
- **The `Drop`/tmux/lock stall mechanisms** (A and B) — closed by 04x/05x/06x.
- **`daemoneye ping`/`status` liveness reporting** — that is phase 09.

### ⚠ Traps

1. **The fix is one line at the client**, not three timeouts at the backends.
2. **But the message still needs translating** — raw reqwest says `error
   decoding response body`, which misdirects.
3. **Do not add `bytes` to `Cargo.toml`.** The helper is generic precisely so you
   do not have to name `bytes::Bytes`.
4. **Keep the 300 s total timeout.** Total and per-read are complementary.
5. **The test server must not close the socket** — closing gives EOF, a different
   path, and the test would pass for the wrong reason.
6. **Do not use `http()` in the test.** It is a `OnceLock` at 120 s.
7. **Suite goes 927 → 928.** One new test. If you measure anything else, report a
   blocker rather than re-counting.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-30 01:14 (started)

**Executor:** claude-opus-4-5-20251101

Added `STREAM_IDLE_TIMEOUT` constant, `stream_chunk` helper, `read_timeout` on the shared HTTP client, routed all three backends through the helper, and added the hermetic `idle_stream_times_out_and_reports_a_stall` test.

### Notes for executor — 2026-07-29

**The previous run implemented this phase correctly and was failed by a bad number
in this doc, not by anything you did wrong.** Read this before starting.

What happened: the doc's Pre-flight claimed a baseline of **928** lib tests and the
acceptance criterion demanded **929**. Both were off by one — the true baseline is
**927** and the correct target is **928**. The architect measured 928 with the
change already applied and mistakenly recorded that as the *pre-change* number,
then derived 929 from it. **The criterion was unreachable**, so the run did the
work, could not make the number appear, and spent 60 read-only turns re-counting
until the governor stopped it.

Both numbers are now corrected and re-measured against the tree.

**Three things to carry into this run:**

1. **The source changes were reverted, deliberately.** The tree is clean and the
   Pre-flight numbers above are honest as of right now. You are implementing from
   scratch — the four code blocks in the Spec are verbatim and were
   compile-verified, so this should be quick.
2. **Count tests with the summary line, once.** `cargo test 2>&1 | grep "^test
   result"`. Not `cargo test --lib` (one line, not three). Not by counting `^test `
   lines (those totals do not match the summary). See "How to check the test count"
   in Acceptance criteria.
3. **A number that disagrees with this doc means the doc is wrong.** Say so and
   report a blocker with the number you measured. Re-running a read-only command
   that already answered makes no progress and will trip the governor — that is
   exactly what ended the last run.

For reference, the previous run's own gate results, all green before it got stuck:
`cargo fmt --all` ✓, `cargo build` ✓, `cargo clippy --all-targets --all-features
-- -D warnings` ✓, `cargo test idle_stream_times_out_and_reports_a_stall` ✓.

### Update — 2026-07-29 (escalation)

**Chosen lever:** refined re-dispatch
**Rationale:** The `hard_fail` was a pure spec defect — an unsatisfiable test-count
criterion — and the executor had already produced a byte-for-byte correct
implementation, so fixing the number removes the loop's cause entirely; takeover
would forfeit a model data point the executor plainly earned.

### Update — ts=1785378449354 (complete, server-authored)

**Summary:** **Summary + Notes for review:**

Implemented mechanism C: added `STREAM_IDLE_TIMEOUT` (120s) and `stream_chunk<T>` helper to `src/ai/mod.rs`, set `read_timeout` on the shared HTTP client, and routed all three backends (`anthropic`, `openai`, `gemini`) through the helper. The helper converts a raw reqwest timeout error (which says "error decoding response body") into a diagnosable "AI stream stalled" message with a `log::error!` line naming mechanism C. The hermetic test `idle_stream_times_out_and_reports_a_stall` passes in ~0.32s — it spins up a TCP listener that sends one chunk then holds the socket open, confirming the idle timeout fires and is reported as a stall. All acceptance criteria met: lib tests 927→928, integration 27 unchanged, no Cargo.toml changes, exactly 4 source files touched, zero `tokio::time::timeout` usage.

**Executor:** Qwen/Qwen3.6-27B-FP8

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.06s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
issing_header_denies ... ok
test webhook::server::tests::auth_wrong_token_denies ... ok
test webhook::server::tests::auth_token_without_bearer_prefix_denies ... ok
test tmux::cache::tests::get_labeled_context_session_topology ... ok
test tmux::cache::tests::get_labeled_context_pane_classification ... ok
test tmux::cache::tests::get_labeled_context_chat_pane_excluded_from_background ... ok
test tmux::cache::tests::get_labeled_context_synchronized_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test tmux::bounded_output_tests::bounded_output_times_out_and_kills_the_child ... ok
test search::tests::search_events_returns_tail_not_head_when_segment_exceeds_cap ... ok
test search::tests::search_finds_match_in_runbooks ... ok
test search::tests::search_respects_kind_filter ... ok
test search::tests::search_returns_empty_for_no_match ... ok
test session_store::tests::artifacts_round_trip ... ok
test session_store::tests::backfill_idempotent ... ok
test session_store::tests::backfill_missing_artifact_returns_error_name ... ok
test session_store::tests::backfill_stamps_memory_without_frontmatter ... ok
test session_store::tests::backfill_stamps_runbook ... ok
test session_store::tests::backfill_stamps_script ... ok
test session_store::tests::collision_allowed_with_force ... ok
test session_store::tests::collision_rejected_without_force ... ok
test session_store::tests::delete_nonexistent_errors ... ok
test session_store::tests::delete_removes_dir_and_index ... ok
test session_store::tests::list_returns_newest_first ... ok
test session_store::tests::load_messages_max_count_truncates ... ok
test session_store::tests::rename_nonexistent_errors ... ok
test session_store::tests::rename_to_existing_errors ... ok
test session_store::tests::rename_updates_dir_and_index ... ok
test memory::tests::update_memory_partial_update_preserves_other_fields ... ok
test session_store::tests::update_in_place_allowed ... ok

test result: ok. 928 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.31s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 29 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_allow_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test event_log_entry_format ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test ghost_config_parsing ... ok
test minimal_config_parsing ... ok
test schedule_store_persistence ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test g4_briefing_read_and_clear ... ok
test config_pricing_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_masking_applied ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_jsonl_round_trip ... ok
test session_index_persistence ... ok
test webhook_alert_to_event_log ... ok
test g5_mailbox_write_and_read ... ok

test result: ok. 27 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.01s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.07s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-1fe3bbd9ed7ce096)
     Running unittests src/main.rs (target/debug/deps/daemoneye-652de6e8e49133dd)
     Running tests/integration.rs (target/debug/deps/integration-2a7b50e73e835fce)
   Doc-tests daemoneye

```

**Files changed:**

- `src/ai/backends/anthropic.rs` — +1 -1
- `src/ai/backends/gemini.rs` — +1 -1
- `src/ai/backends/openai.rs` — +1 -1
- `src/ai/mod.rs` — +88 -0

**Commit:** be88e3fb3f0155ae91d28b16949c7d3087910659

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-07-29

- **Verdict:** approved_after_1
- **Bounces:** 1 (no bug doc — the failure was a spec defect, escalated and
  refined; `NoProgressStall` at 60 consecutive read-only turns)
- **Executor:** Qwen/Qwen3.6-27B-FP8 (110 turns hard_fail, then **30** turns clean)
- **Scope deviations:** none
- **Calibration:** the bounce was **entirely my arithmetic** — fourth defective
  criterion in M5. See "The bounce was mine" below.

All four gates re-run bare and green (`cargo fmt --all --check`, `cargo build`
after `touch`ing `src/ai/mod.rs` — zero warnings, `cargo clippy --all-targets
--all-features -- -D warnings`, `cargo test` at **928** lib (927 + 1) + **27**
integration).

### Every acceptance criterion verified

`read_timeout` **2** in `src/ai/mod.rs` (one in `http()`, one in the test's own
client); `STREAM_IDLE_TIMEOUT` **4**; `from_secs(300)` **1 — unchanged**, so total
and per-read bounds coexist as intended; bare `let bytes = chunk?;` **0** across
the backends (3 before) with `stream_chunk(` **1** in each of `anthropic.rs`,
`openai.rs`, `gemini.rs`; **0** added `tokio::time::timeout`, so the fix is at the
client and not a wrapper; no `Cargo` change, and in particular no `bytes`
dependency — the generic signature held; exactly **four** `src/` files; zero
`#[allow]`/`unsafe`/`TODO`/`dbg!`; zero added `unwrap`/`expect`/`panic!` in the
backends. (`http()`'s pre-existing `.unwrap()` with its `INVARIANT` comment is
untouched.)

**The diff is byte-for-byte what I applied, verified and reverted while drafting**
— including the fmt-driven reflow of the test's `assert!(super::stream_chunk(first)
.is_ok(), …)` onto three lines.

### Coverage is real — mutation re-run independently

Per STANDARDS § "Coverage claims are inadmissible without mutation proof", and
because a claimed mutation check is not one, I re-ran it myself. Replacing
`if e.is_timeout() {` with `if false {`:

```
test ai::stream_idle_tests::idle_stream_times_out_and_reports_a_stall ... FAILED
idle timeout must be reported as a stall, got: AI stream read failed: error decoding response body
```

Restored → passes in 0.32 s. So the test guards the **diagnostic**, not merely the
timeout — and the failure message is itself the argument for the helper's
existence: `error decoding response body` is what reqwest says about a stalled
stream, which points at parsing rather than at a silent provider.

### The bounce was mine, and it is a pattern

The first run implemented this phase **byte-for-byte correctly** and passed all
four gates, then spent 60 read-only turns re-counting tests because the criterion
demanded **929** and the true number is **928**. I had measured 928 with the change
already applied, recorded it as the *pre-change* baseline, and derived 929 from it.
Green was impossible. I even wrote `921 + 6` in a note and did not notice it equals
927.

Two distinct defects, both mine:

1. **A derived count.** "Run every count criterion; never derive it" exists for
   exactly this; I ran the command but attributed its output to the wrong tree
   state, then did arithmetic on top.
2. **A criterion phrased over an output the executor was reading with a different
   command.** It used `cargo test --lib`, which prints one `test result` line where
   the criterion spoke of three.

The refinement fixed both numbers, restated the count check as **baseline + 1** so
an arithmetic slip cannot make it unreachable again, named the `--lib` trap, and
added an explicit "a number that disagrees with this doc means the doc is wrong —
report a blocker, do not re-run". **Result: 110 turns → 30.**

**This is the third `NoProgressStall` of this species in M5 and my fourth defective
criterion** (06n contradiction, 06s prose-match, 06w unsatisfiable-as-written, 07
derived-and-impossible). Every one was written or derived *outside* the
apply-verify-revert pass. The mechanical pre-dispatch criteria-runner carried as a
candidate fold since 06q would have caught all four; recommend promoting it at
milestone close rather than holding it further.

### Note on the governor

The identical-call detector (threshold 6) did **not** fire despite ~20 byte-identical
invocations of `cargo test --lib 2>&1 | grep "^test " | grep -v "result" | tail -3`,
because a few near-identical variants early on broke the exact-match streak. The
read-only stall detector caught it at 60. That is the near-identical-call gap
`WORKFLOW.md` already records as a runtime feature request; this run is a further
data point for it, not a new finding.

# Phase 05: Turn-loop hardening — reap the chat task, bound the silent retry, optional turn deadline

**Milestone:** M16 — LLM Stream Robustness
**Status:** todo
**Depends on:** phase-04
**Estimated diff:** ~220 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

Close the worst silent-failure hole in the daemon: when the spawned chat task
dies without sending `Done` or `Error` (a panic, or a backend bug that
returns early), `run_conversation_loop` currently re-issues the entire AI
call **forever** — the user sees only KeepAlives while the daemon burns API
calls. After this phase the chat task's `JoinHandle` is kept and reaped (a
panic is classified and named), the re-issue is bounded at 2, exhaustion
produces a user-visible `Response::Error`, every early return aborts the
in-flight provider stream, and an optional `[limits] turn_timeout_secs`
bounds the whole turn (precedent: `GHOST_TURN_TIMEOUT_SECS = 300` for ghost
turns, `src/daemon/ghost.rs:433`).

## Architecture references

Read before starting:

- `CLAUDE.md` § "Request/Response lifecycle".

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture reference above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

(Current as of 2026-08-16; re-derive with
`grep -n "tokio::spawn\|Ok(None)" src/daemon/stream.rs | head`.)

`src/daemon/stream.rs` — the outer per-AI-call loop starts at ~line 92; the
per-turn counters that survive across outer iterations are declared just
above it (~lines 89–90, `tool_call_counts` / `total_turn_call_count` — the
idiom Task 2's counter copies). The spawn (~119–132) **drops the
JoinHandle**:

```rust
tokio::spawn(async move {
    if let Err(e) = client_instance
        .chat(
            &sys_prompt_turn,
            messages_clone,
            ai_tx.clone(),
            true,
            loaded_tools,
        )
        .await
    {
        let _ = ai_tx.send(AiEvent::Error(e.to_string()));
    }
});
```

A panic inside `chat` unwinds the task: nothing is sent, `ai_tx` is dropped,
and the event loop's recv arm (~137–148, reshaped by phase-04 Task 6) hits:

```rust
Ok(Some(ev)) => ev,
Ok(None) => break,   // ← breaks the INNER loop only; outer loop re-spawns
```

`AiEvent::Error` is forwarded as `Response::Error` and ends the turn
(~654–657). `LimitsConfig` is at `src/config/types.rs:400` (serde-default
idiom per field; it already has `max_turns`, `per_tool_batch`, etc.).

## Spec

### Task 1 — Keep and guard the chat task handle

In `src/daemon/stream.rs`, bind the spawn and wrap it in a drop-abort guard
(new private types at the bottom of the file, above `mod tests` if present):

```rust
/// Abort the in-flight provider stream when the turn ends early for any
/// reason (error return, client gone, deadline). Without this the spawned
/// chat task keeps consuming (and billing) the provider stream with nobody
/// listening.
struct ChatTaskGuard(Option<tokio::task::JoinHandle<()>>);

impl ChatTaskGuard {
    fn new(handle: tokio::task::JoinHandle<()>) -> Self {
        ChatTaskGuard(Some(handle))
    }

    /// Reap the task and describe how it ended — used when the event channel
    /// closed without a terminal `Done`/`Error` event.
    async fn describe_end(&mut self) -> String {
        match self.0.take() {
            Some(handle) => match handle.await {
                Ok(()) => "backend returned without sending Done or Error \
                           (backend bug)"
                    .to_string(),
                Err(e) if e.is_panic() => {
                    format!("chat task panicked: {}", panic_message(e))
                }
                Err(e) => format!("chat task was cancelled: {e}"),
            },
            None => "chat task already reaped".to_string(),
        }
    }
}

impl Drop for ChatTaskGuard {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            handle.abort();
        }
    }
}

/// Extract the payload string from a panicked task's JoinError.
fn panic_message(e: tokio::task::JoinError) -> String {
    match e.try_into_panic() {
        Ok(payload) => payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "non-string panic payload".to_string()),
        Err(e) => e.to_string(),
    }
}
```

At the spawn site: `let mut chat_task = ChatTaskGuard::new(tokio::spawn(async
move { ... }));` — body unchanged. The guard is per-outer-iteration (declared
where the spawn is), so each re-issue gets a fresh guard and the previous
attempt is aborted by `Drop` when the binding is replaced… it is not —
shadowing in a loop drops the old value at re-bind, which is exactly the
abort we want; no extra code needed.

### Task 2 — Bound the channel-closed re-issue

Above the outer loop (next to `total_turn_call_count`), add:

```rust
// Bounded re-issue when the chat task dies without a terminal event
// (panic / backend bug). Without the bound this was an infinite silent
// retry loop — the user saw only KeepAlives while API calls burned.
const MAX_CHANNEL_CLOSED_RETRIES: u32 = 2;
let mut channel_closed_retries: u32 = 0;
```

Replace `Ok(None) => break,` with:

```rust
Ok(None) => {
    let cause = chat_task.describe_end().await;
    channel_closed_retries += 1;
    if channel_closed_retries > MAX_CHANNEL_CLOSED_RETRIES {
        log::error!(
            "AI event channel closed without Done/Error, giving up after \
             {MAX_CHANNEL_CLOSED_RETRIES} retries: {cause}"
        );
        send_response_split(
            tx,
            Response::Error(format!(
                "AI backend ended the stream without completing \
                 (after {} attempts): {cause}",
                channel_closed_retries
            )),
        )
        .await?;
        return Ok(());
    }
    log::warn!(
        "AI event channel closed without Done/Error (attempt \
         {channel_closed_retries}/{MAX_CHANNEL_CLOSED_RETRIES}), \
         re-issuing the AI call: {cause}"
    );
    break;
}
```

### Task 3 — `[limits] turn_timeout_secs`

In `src/config/types.rs`, add to `LimitsConfig` (matching the file's
serde-default idiom and doc-comment style):

```rust
/// Maximum wall-clock seconds for a single interactive assistant turn,
/// including tool execution. On expiry the turn ends with a visible
/// error. Ghost shells have their own fixed 300 s per-turn budget.
/// Default: 0 (no limit). Recommended when enabled: 3600.
#[serde(default)]
pub turn_timeout_secs: u64,
```

In `run_conversation_loop`, at function entry:

```rust
let turn_deadline = (config.limits.turn_timeout_secs > 0).then(|| {
    tokio::time::Instant::now()
        + std::time::Duration::from_secs(config.limits.turn_timeout_secs)
});
```

At the top of the inner event-loop iteration (immediately before the recv
`match`), check it:

```rust
if let Some(deadline) = turn_deadline
    && tokio::time::Instant::now() >= deadline
{
    send_response_split(
        tx,
        Response::Error(format!(
            "Turn exceeded [limits] turn_timeout_secs ({}s) — aborting.",
            config.limits.turn_timeout_secs
        )),
    )
    .await?;
    return Ok(());
}
```

(The `ChatTaskGuard` drop aborts the provider stream on this return.)

### Task 4 — Unit tests

- In `src/daemon/stream.rs` tests (create a `mod tests` in the file if none
  exists — check first with `grep -n "mod tests" src/daemon/stream.rs`):
  - `panicking_chat_task_is_classified` — `#[tokio::test]`: spawn
    `tokio::spawn(async { panic!("boom") })`, wrap in `ChatTaskGuard`, assert
    `describe_end().await` contains `"panicked"` and `"boom"`.
  - `clean_return_without_done_is_named_backend_bug` — spawn
    `tokio::spawn(async {})`, assert `describe_end()` contains
    `"without sending Done"`.
  - `guard_drop_aborts_task` — spawn a task that holds a
    `tokio::sync::oneshot::Sender` and then awaits
    `std::future::pending::<()>()`; wrap it in `ChatTaskGuard`, drop the
    guard, `tokio::task::yield_now().await`, and assert the oneshot
    `Receiver` resolves with `Err` (sender dropped by the abort).
- In the config tests: `turn_timeout_secs_parses_and_defaults_to_zero` —
  parse `[limits]\nturn_timeout_secs = 10` → 10; absent → 0.

### Task 5 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-05.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this.

## Acceptance criteria

- [ ] `grep -c "struct ChatTaskGuard" src/daemon/stream.rs` prints `1`
      (currently `0`).
- [ ] `grep -c "MAX_CHANNEL_CLOSED_RETRIES" src/daemon/stream.rs` prints `3`
      (currently `0`; const + two uses in the quoted arm — re-pin if your
      final shape differs, before running the E2E block).
- [ ] `grep -c "Ok(None) => break," src/daemon/stream.rs` prints `0`
      (currently `1`).
- [ ] `grep -c "pub turn_timeout_secs" src/config/types.rs` prints `1`
      (currently `0`).
- [ ] `cargo test panicking_chat_task_is_classified` passes.
- [ ] `cargo test turn_timeout_secs_parses_and_defaults_to_zero` passes.
- [ ] All four gates green.
- [ ] The end-to-end entry ends with `PASTE MATCH`.

## Test plan

Tests enumerated in Spec Task 4. The bounded-retry control flow inside
`run_conversation_loop` is not unit-testable without a client seam that does
not exist (the loop constructs its client internally via `make_client`); its
behavior is pinned by the quoted arm in Task 2 and verified live at
milestone close. Do not build a dependency-injection refactor for it.

## End-to-end verification

```sh
A=/tmp/e2e-05.txt; : > "$A"
grep -c "struct ChatTaskGuard" src/daemon/stream.rs >> "$A"
grep -c "MAX_CHANNEL_CLOSED_RETRIES" src/daemon/stream.rs >> "$A"
grep -c "Ok(None) => break," src/daemon/stream.rs >> "$A"
grep -c "pub turn_timeout_secs" src/config/types.rs >> "$A"
cargo test panicking_chat_task_is_classified 2>&1 | grep -E "^test " >> "$A"; echo "exit=${PIPESTATUS[0]}" >> "$A"
cargo test 2>&1 | grep -E "^test result:" >> "$A"; echo "exit=${PIPESTATUS[0]}" >> "$A"
```

Paste-fidelity self-check (append the verdict line to the entry):

```sh
D=docs/dev/milestones/M16-llm-stream-robustness/phase-05-turn-loop-hardening.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-05.txt
diff /tmp/pasted-05.txt /tmp/e2e-05.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- Any CLI changes (phase-06).
- Cancellation via IPC (phase-08 — it builds on this phase's guard).
- Applying the turn deadline to ghost turns (they already have
  `GHOST_TURN_TIMEOUT_SECS`).
- A client-injection seam for `run_conversation_loop`.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

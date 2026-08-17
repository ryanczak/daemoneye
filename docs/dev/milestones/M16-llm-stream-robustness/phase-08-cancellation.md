# Phase 08: Cancellation — Esc aborts the provider stream through `Request::Cancel`

**Milestone:** M16 — LLM Stream Robustness
**Status:** todo
**Depends on:** phase-05
**Estimated diff:** ~350 lines
**Tags:** language=rust, kind=feature, size=l

## Goal

Today Esc only breaks the client's local loop; the daemon keeps streaming
(and paying for) the provider response until its next write hits EPIPE.
This phase gives cancellation a real protocol path: a ported
`CancelHandle`/`CancelSignal` pair per in-flight turn, registered by session
id; a new `Request::Cancel` that any connection can deliver (**out-of-band —
the client opens a fresh connection for it**, so the streaming socket's `rx`
ownership is never touched); and a cancel branch in the daemon's event loop
that aborts the chat task (phase-05's `ChatTaskGuard`), persists the partial
response with a `⊘ cancelled` marker, and ends the turn cleanly with
`SystemMsg` + `Ok`.

## Architecture references

Read before starting:

- `CLAUDE.md` § "Request/Response lifecycle".
- `docs/dev/milestones/M16-llm-stream-robustness/phase-05-turn-loop-hardening.md`
  — the `ChatTaskGuard` this phase reuses.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Confirm phase-05 landed:
   `grep -c "struct ChatTaskGuard" src/daemon/stream.rs` must print `1`;
   otherwise stop and file a blocker.

## Current state

(Current as of 2026-08-16; re-derive with the greps shown.)

- `src/ipc.rs:139` — `pub enum Request`; variants are serde-tagged and the
  daemon dispatch is a flat match in `src/daemon/server/mod.rs` (~lines
  172–281, one arm per request; follow that arm style).
- Esc handling: `src/cli/commands/stream.rs` — `StreamOutcome::Interrupted`
  arm (~196–200) commits `⊘ interrupted` and `break`s, dropping the socket.
  `src/cli/commands/interrupt.rs` holds `InterruptState` (double-press
  logic). The daemon never learns about the interrupt except via EPIPE.
- The daemon event loop's recv site is the phase-04/05 shape in
  `src/daemon/stream.rs` (the `tokio::time::timeout(..., ai_rx.recv())`
  match); `session_id: Option<String>` is in scope throughout
  `run_conversation_loop`, and the client knows its session id
  (`src/cli/commands/stream.rs:70,112`).
- The rexyMCP cancel module this phase ports is quoted in full in Task 1 —
  the executor does not need the rexyMCP tree.
- Mutex sites in this codebase use `.unwrap_or_log()` (`UnpoisonExt`,
  `src/util.rs`) — **never** `.unwrap()` on a lock (CLAUDE.md invariant).

## Spec

### Task 1 — Port the cancel module

Create `src/daemon/cancel.rs` (register `pub mod cancel;` in
`src/daemon/mod.rs` alongside the other modules) with the ported code —
copy verbatim, including its five tests:

```rust
//! Cooperative cancellation for in-flight interactive turns.
//!
//! Built on `tokio::sync::watch` — a `CancelHandle` flips the signal and a
//! `CancelSignal` observes it. Ported from rexyMCP (`executor/src/agent/
//! cancel.rs`, MIT, same author).

use tokio::sync::watch;

/// Handle that can flip the cancellation signal.
pub struct CancelHandle {
    tx: watch::Sender<bool>,
}

impl CancelHandle {
    /// Flip the signal. Ignores a send error from all-receivers-dropped.
    pub fn cancel(&self) {
        let _ = self.tx.send(true);
    }
}

/// Observable side of the cancellation signal.
#[derive(Clone)]
pub struct CancelSignal {
    rx: watch::Receiver<bool>,
}

impl CancelSignal {
    /// Create a fresh pair. The handle starts the signal at `false`.
    pub fn new() -> (CancelHandle, CancelSignal) {
        let (tx, rx) = watch::channel(false);
        (CancelHandle { tx }, CancelSignal { rx })
    }

    /// Check if the signal has been flipped.
    pub fn is_cancelled(&self) -> bool {
        *self.rx.borrow()
    }

    /// Resolve when the signal is flipped. If the sender is dropped before
    /// the flip, park forever.
    pub async fn cancelled(&mut self) {
        loop {
            if *self.rx.borrow() {
                return;
            }
            match self.rx.changed().await {
                Ok(_) => {}
                Err(_) => std::future::pending::<()>().await,
            }
        }
    }
}
```

Also port the original's tests (`cancel_flips_signal`,
`clone_observes_flip`, `dropped_handle_does_not_cancel`) — adapt/drop the
two `never()`-specific ones if you omit `never()` (it has no caller here;
per STANDARDS § 2.2 do not port unused API — omit `never()`).

### Task 2 — Turn registry

In `src/daemon/cancel.rs`, add a session-keyed registry:

```rust
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::util::UnpoisonExt;

static REGISTRY: OnceLock<Mutex<HashMap<String, CancelHandle>>> = OnceLock::new();

fn registry() -> &'static Mutex<HashMap<String, CancelHandle>> {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register a turn's cancel handle. Returns a guard that deregisters on
/// drop, so every exit path of the turn cleans up.
pub fn register_turn(session_id: &str) -> (TurnCancelGuard, CancelSignal) {
    let (handle, signal) = CancelSignal::new();
    registry()
        .lock()
        .unwrap_or_log()
        .insert(session_id.to_string(), handle);
    (
        TurnCancelGuard {
            session_id: session_id.to_string(),
        },
        signal,
    )
}

/// Flip the cancel signal for a session's in-flight turn, if any.
/// Returns whether a turn was found.
pub fn cancel_turn(session_id: &str) -> bool {
    match registry().lock().unwrap_or_log().get(session_id) {
        Some(handle) => {
            handle.cancel();
            true
        }
        None => false,
    }
}

pub struct TurnCancelGuard {
    session_id: String,
}

impl Drop for TurnCancelGuard {
    fn drop(&mut self) {
        registry().lock().unwrap_or_log().remove(&self.session_id);
    }
}
```

Tests: `register_cancel_roundtrip` (register, `cancel_turn` returns true,
signal observes it), `cancel_unknown_session_is_false`,
`guard_drop_deregisters` (after drop, `cancel_turn` returns false). Use
distinct session-id strings per test — the registry is process-global and
tests run concurrently.

### Task 3 — `Request::Cancel` + server dispatch

In `src/ipc.rs`, add to `Request` (follow the existing doc-comment style):

```rust
/// Cancel the in-flight turn of a session. Sent on a fresh connection
/// (out-of-band) by the chat client when the user interrupts, so the
/// streaming connection's reader is never contended.
Cancel { session_id: String },
```

In `src/daemon/server/mod.rs`'s dispatch match, add an arm (mirror the
one-liner arms like `Request::Ping`):

```rust
Request::Cancel { session_id } => {
    let found = crate::daemon::cancel::cancel_turn(&session_id);
    log::info!("cancel request for session {session_id}: found={found}");
    send_response(&mut stream, Response::Ok).await?;
}
```

(Match the surrounding arms for how a response is written — if they use a
different sender helper, use that one.)

### Task 4 — Cancel branch in the daemon event loop

In `src/daemon/stream.rs` `run_conversation_loop`:

- At function entry (before the outer loop), register the turn when
  `session_id` is `Some`; when it is `None`, build a signal whose handle is
  immediately dropped — the ported `cancelled()` then parks forever
  (demonstrated by the ported `dropped_handle_does_not_cancel` test), so the
  cancel branch simply never fires. Concretely:

```rust
let (_cancel_guard, mut cancel_signal) = match session_id.as_deref() {
    Some(sid) => {
        let (guard, signal) = crate::daemon::cancel::register_turn(sid);
        (Some(guard), signal)
    }
    None => {
        let (_h, signal) = crate::daemon::cancel::CancelSignal::new();
        (None, signal) // handle dropped => signal can never fire
    }
};
```

- In the event recv site, wrap the existing timeout-recv in a `select!` so
  cancellation interrupts the wait (the recv future and `cancelled()` are
  both cancel-safe):

```rust
let event = tokio::select! {
    biased;
    _ = cancel_signal.cancelled() => {
        if !full_response.is_empty() {
            messages.push(Message {
                role: "assistant".to_string(),
                content: format!("{full_response}\n\n[⊘ cancelled by user]"),
                tool_calls: None,
                tool_results: None,
                turn: Some(this_turn_count),
            });
        }
        send_response_split(tx, Response::SystemMsg("⊘ cancelled".to_string())).await?;
        send_response_split(tx, Response::Ok).await?;
        return Ok(());
    }
    recv = tokio::time::timeout(
        std::time::Duration::from_secs(
            crate::daemon::utils::keepalive::KEEPALIVE_PERIOD_SECS,
        ),
        ai_rx.recv(),
    ) => match recv {
        // existing three arms unchanged (Ok(Some), Ok(None), Err)
    },
};
```

  (`ChatTaskGuard` from phase-05 aborts the provider stream on the
  `return`.) Adapt the `Message` push to however the existing final-answer
  push constructs it — copy the field shape from the `Done` arm.

### Task 5 — Client sends Cancel and drains

In `src/cli/commands/stream.rs`, in the `StreamOutcome::Interrupted` arm:
before the existing commit/break, when `session_id` is `Some`, fire the
out-of-band cancel — a small helper in the same file:

```rust
async fn send_cancel(session_id: &str) {
    let fut = async {
        let mut stream =
            tokio::net::UnixStream::connect(crate::config::default_socket_path()).await?;
        let mut data = serde_json::to_vec(&crate::ipc::Request::Cancel {
            session_id: session_id.to_string(),
        })?;
        data.push(b'\n');
        use tokio::io::AsyncWriteExt;
        stream.write_all(&data).await?;
        anyhow::Ok(())
    };
    if let Err(e) = tokio::time::timeout(std::time::Duration::from_secs(2), fut)
        .await
        .unwrap_or_else(|_| Err(anyhow::anyhow!("cancel send timed out")))
    {
        log::warn!("failed to deliver cancel: {e}");
    }
}
```

(Check how this file already connects to the daemon socket — `run_chat`
resolves the socket path once; reuse the same path-resolution call it uses,
not a hardcoded one.) The arm then still commits `⊘ interrupted` and breaks
as today — the daemon-side turn ends via the cancel signal rather than a
later EPIPE. Do not add a drain-until-Ok loop: the current break-and-drop
UX stays; the point of this phase is the daemon-side abort.

### Task 6 — Integration round-trip test

In `tests/integration.rs`, following the existing IPC round-trip style
(serialize → deserialize): `cancel_request_roundtrip` — a
`Request::Cancel { session_id: "s1" }` survives a serde round-trip with the
session id intact.

### Task 7 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-08.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this.

## Acceptance criteria

- [ ] `grep -c "Cancel { session_id" src/ipc.rs` prints `1` (currently `0`).
- [ ] `grep -c "pub mod cancel" src/daemon/mod.rs` prints `1` (currently `0`).
- [ ] `grep -c "cancel_turn" src/daemon/server/mod.rs` prints `1`
      (currently `0`).
- [ ] `grep -c "register_turn" src/daemon/stream.rs` prints `1`
      (currently `0`).
- [ ] `grep -c "send_cancel" src/cli/commands/stream.rs` prints ≥ `2`
      (definition + call; currently `0`).
- [ ] `cargo test cancel` passes (module tests + registry tests +
      `cancel_request_roundtrip`).
- [ ] All four gates green.
- [ ] The end-to-end entry ends with `PASTE MATCH`.

## Test plan

Tasks 1, 2 and 6. The full live path (Esc → provider stream aborted, partial
persisted) is an architect-run milestone-close check via session JSONL.

## End-to-end verification

```sh
A=/tmp/e2e-08.txt; : > "$A"
grep -c "Cancel { session_id" src/ipc.rs >> "$A"
grep -c "cancel_turn" src/daemon/server/mod.rs >> "$A"
grep -c "register_turn" src/daemon/stream.rs >> "$A"
grep -c "send_cancel" src/cli/commands/stream.rs >> "$A"
cargo test cancel 2>&1 | tail -5 >> "$A"; echo "exit=${PIPESTATUS[0]}" >> "$A"
cargo test 2>&1 | tail -3 >> "$A"; echo "exit=${PIPESTATUS[0]}" >> "$A"
```

Paste-fidelity self-check (append the verdict line to the entry):

```sh
D=docs/dev/milestones/M16-llm-stream-robustness/phase-08-cancellation.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-08.txt
diff /tmp/pasted-08.txt /tmp/e2e-08.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None. (No new dependencies — `tokio::sync::watch` ships with the existing
tokio features.)

## Out of scope

- Cancelling mid-tool-execution (approval prompts, foreground commands):
  the cancel signal is only observed between AI events. A cancel arriving
  during a tool wait takes effect at the next event-loop iteration — do not
  thread the signal into `execute_tool_call`.
- Ghost shells (no interactive client; they keep their turn budget).
- A drain-until-Ok client UX change.
- Cancel-without-disconnect (Esc that keeps the turn's connection and
  resumes) — future work.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

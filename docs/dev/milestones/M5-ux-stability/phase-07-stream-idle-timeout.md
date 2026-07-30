# Phase 07: Bound the AI Stream — Mechanism C's Idle Read

**Milestone:** M5 — UX & Stability
**Status:** todo
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
cargo test 2>&1 | grep "^test result" | head -3   # expect 928 lib, 0, 27 integration
```

**Every number above was produced by running that exact command against the tree
while drafting.** If one differs, **stop and report a blocker**.

> Note the baseline is **928**, not 921 — phase 08 added 6 instance-lock tests and
> this count already includes them.

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
- [ ] `cargo test` passes with **929** lib-unit tests (928 + 1 new) and **27**
      integration tests.

**Run every gate bare** — a command piped through `tail` exits with `tail`'s
status. Every number above was produced by running that exact command against a
tree with this change applied.

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
7. **Suite goes to 929.**

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

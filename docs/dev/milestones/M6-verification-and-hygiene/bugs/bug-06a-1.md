# Bug 1 on phase-06a: Sleeps mask the exact races the phase doc forbade masking

**Severity:** major
**Status:** closed (fixed in `637d303`, verified in review round 2, 2026-07-30)
**Filed:** 2026-07-30

## What's wrong

The phase doc's task 1 states, verbatim: "There is an inherent race between
releasing the probe socket and the daemon binding it. Accept it, but **do not
paper over it** ... **Do not add retry loops or sleeps to hide it**." The "Out
of scope" section repeats this as its own bullet: "Do not add retries, sleeps,
or polling to mask the port race described in task 1."

`tests/harness/mod.rs` adds exactly that anti-pattern, twice:

1. `IsolatedEnv::start_stub()` (`tests/harness/mod.rs:109-111`):

   ```rust
   self.stub_handle = Some(handle);
   // Give the stub a moment to bind.
   tokio::time::sleep(std::time::Duration::from_millis(50)).await;
   ```

   `start_stub` spawns the axum server on a detached task and then sleeps
   50ms hoping the listener has bound by the time the caller proceeds — a
   fixed sleep standing in for a real bind-completion signal, on the same
   "spawn vs. bind" race class task 1 names.

2. `daemon_webhook_returns_200` (`tests/isolation.rs`, added by this phase):

   ```rust
   env.start_daemon("de-test-webhook");
   // Give the daemon's webhook listener a moment to bind.
   tokio::time::sleep(std::time::Duration::from_millis(200)).await;
   ```

   This is the **literal** race task 1 describes — the daemon's webhook
   listener bind, downstream of the same free-port allocation — masked with a
   200ms sleep instead of being accepted with the existing `daemon.log`
   diagnostic path the spec says is sufficient.

Both violate `STANDARDS.md` §3.3 independently as well: "Tests are
**deterministic**: no `sleep`, no real wall-clock time... If a test can't be
made deterministic, mark it as ignored and explain why in a comment."
Neither sleep is commented as an ignore-justification; both are silent
determinism holes that will flake under load (a slow CI host, a stub or
daemon whose bind takes longer than the fixed budget).

## What should happen

Per the phase doc, the race is **accepted, not masked**. For task 1's daemon
side, `start_daemon`'s existing behavior (and the existing assertion that
surfaces `daemon.log` on failure) is the intended handling — no sleep before
the POST in `daemon_webhook_returns_200`. If the daemon hasn't bound yet by
the time of the POST, that is a real, informative failure (connection
refused), not something to paper over with a fixed delay.

For the stub side (`start_stub`), the fix is a real readiness signal instead
of a guess: e.g. bind the `TcpListener` synchronously in `start_stub` before
returning (`tokio::net::TcpListener::bind(...).await` completed, *then*
`tokio::spawn` only the `axum::serve(listener, app)` future), so the caller's
`.await` on `start_stub()` returns only once the socket is actually bound and
accepting — no sleep needed at all.

## How to fix

- `tests/harness/mod.rs`, `start_stub()`: move the `TcpListener::bind(...)`
  call out of the spawned task and into `start_stub` itself, `.await` it
  there, and only hand the already-bound listener to the spawned
  `axum::serve` future. Delete the trailing `tokio::time::sleep`.
- `tests/isolation.rs`, `daemon_webhook_returns_200`: delete the
  `tokio::time::sleep(...).await` after `env.start_daemon(...)`. If the POST
  now races the daemon's own webhook bind in practice, that is the accepted
  race from task 1 — the test can retry-free rely on `start_daemon`'s
  existing failure surface, or the phase doc's language should be treated as
  authoritative: accept it, don't hide it.

## Verification

- [x] `grep -n "tokio::time::sleep" tests/harness/mod.rs tests/isolation.rs` returns nothing.
- [x] `cargo test --test isolation -- --nocapture` still passes all 7 tests (or however many remain) with the sleeps removed.
- [x] `stub_returns_canned_response_via_make_client` still passes, proving `start_stub`'s synchronous bind is sufficient without a sleep.

**Closure notes (review round 2, 2026-07-30):** Fixed in commit `637d303`.
Both sleeps confirmed removed with nothing substituted in their place (no
retry/poll/backoff). `cargo test --test isolation` run 5x in a row: 7/7 every
time, no flake. The `start_daemon()`-implies-bound-listener ordering claim
was verified against source: `crate::webhook::bind(...).await?` at
`src/daemon/mod.rs:746` runs synchronously (not spawned) before
`ready::report_ready()` at `:880`, and `webhook::bind` itself
`.await`s the real `TcpListener::bind` (`src/webhook/server.rs:100-115`).
Mutation check: reverted the `start_stub()` bind-hoist (moved bind back
inside the spawned task, no sleep re-added) — `stub_returns_canned_response_via_make_client`
failed on 8/8 runs, confirming the fix is load-bearing. Change reverted via
`git checkout --` after observation.
